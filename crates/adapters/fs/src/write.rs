//! `edit_file` and `write_file`, in two phases around the user's approval.
//!
//! `prepare_*` reads the current content, computes the new one and its
//! unified diff (with `similar`) and returns a [`PendingChange`] without
//! touching the disk. [`apply`] re-resolves the path through the guard,
//! fails with [`FsError::Stale`] unless the file still holds exactly the
//! content that was diffed (or still does not exist, for a creation), and
//! writes atomically: a temporary file in the same directory, flushed to
//! disk and renamed over the target, so a reader never sees a half-written
//! file. Missing parent directories are created and the permissions of an
//! overwritten file are kept. Like the guard (see its TOCTOU note), the
//! stale check and the rename are separate calls, so a write racing in
//! between them is lost; it protects the approval, not against concurrent
//! writers.
//!
//! # Sensitive paths
//!
//! Writes are denied by the same sensitive-path rules as reads (`.git/`,
//! `.env*`, keys…), on both the requested path and the one its symlinks
//! resolve to: rewriting `.git/hooks/*` or `.git/config` would run code
//! outside the approval flow, and a file the model may not read is not one
//! it should overwrite blindly. This matches the in-memory fake.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use application::ports::fs::{
    EditFileRequest, FileChange, FsError, PendingChange, WriteFileRequest, replace_unique,
};
use domain::workspace::WorkspacePath;
use similar::TextDiff;
use tokio::io::AsyncWriteExt;

use crate::guard::WorkspaceGuard;
use crate::read::{io_error, read_text};

/// Lines of unchanged context around each hunk of a diff.
const DIFF_CONTEXT: usize = 3;

/// Computes the edit of `request` without writing it.
pub(crate) async fn prepare_edit(
    guard: &WorkspaceGuard,
    request: EditFileRequest,
) -> Result<PendingChange, FsError> {
    let (path, real) = guard.resolve_readable(request.path.as_str()).await?;
    let original = read_existing(&path, &real).await?;
    let new_content = replace_unique(&original, &request.old, &request.new)?;
    let diff = unified_diff(&path, Some(&original), &new_content);
    Ok(PendingChange::new(path, Some(original), new_content, diff))
}

/// Computes the creation or overwrite of `request` without writing it.
pub(crate) async fn prepare_write(
    guard: &WorkspaceGuard,
    request: WriteFileRequest,
) -> Result<PendingChange, FsError> {
    let (path, target) = resolve_writable(guard, &request.path).await?;
    let original = match read_existing(&path, &target).await {
        Ok(text) => Some(text),
        Err(FsError::NotFound { .. }) => None,
        Err(e) => return Err(e),
    };
    let diff = unified_diff(&path, original.as_deref(), &request.content);
    Ok(PendingChange::new(path, original, request.content, diff))
}

/// Writes a prepared change after re-checking the guards and that the file
/// did not change since it was prepared.
pub(crate) async fn apply(
    guard: &WorkspaceGuard,
    change: PendingChange,
) -> Result<FileChange, FsError> {
    let (path, target) = resolve_writable(guard, change.path()).await?;
    let current = match tokio::fs::metadata(&target).await {
        Ok(meta) if meta.is_dir() => return Err(FsError::IsDirectory { path }),
        Ok(meta) => {
            let bytes = tokio::fs::read(&target)
                .await
                .map_err(|e| io_error(&path, &target, &e))?;
            Some((bytes, meta.permissions()))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(io_error(&path, &target, &e)),
    };
    let current_bytes = current.as_ref().map(|(bytes, _)| bytes.as_slice());
    if current_bytes != change.original().map(str::as_bytes) {
        return Err(FsError::Stale { path });
    }
    let permissions = current.map(|(_, permissions)| permissions);
    write_atomic(&target, change.new_content().as_bytes(), permissions)
        .await
        .map_err(|e| io_error(&path, &target, &e))?;
    tracing::debug!(%path, created = change.creates(), "file written");
    Ok(change.to_change())
}

/// Distinguishes temporary files of concurrent writes in one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How many temporary names are tried before giving up.
const TEMP_ATTEMPTS: usize = 16;

/// Replaces `target` with `content` through a temporary file in its
/// directory (created if missing) and a rename, applying `permissions`
/// when given.
async fn write_atomic(
    target: &Path,
    content: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "not a file path",
        ));
    };
    tokio::fs::create_dir_all(dir).await?;
    let mut attempt = 0;
    let (temp, mut file) = loop {
        let temp = dir.join(format!(
            ".{}.crest-{}-{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await
        {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists && attempt < TEMP_ATTEMPTS => {
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    };
    let written = async {
        file.write_all(content).await?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions).await?;
        }
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temp, target).await
    }
    .await;
    if written.is_err() {
        // Best effort: the original error is the one worth reporting.
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

/// Resolves where a write to `path` lands (see
/// [`WorkspaceGuard::resolve_for_write`]) and denies it when the requested
/// or the symlink-resolved path is sensitive.
async fn resolve_writable(
    guard: &WorkspaceGuard,
    path: &WorkspacePath,
) -> Result<(WorkspacePath, PathBuf), FsError> {
    let (path, target) = guard.resolve_for_write(path.as_str()).await?;
    guard.check_readable(&path)?;
    if let Some(real) = relative(guard, &target)
        && real != path
    {
        guard.check_readable(&real).map_err(|e| match e {
            FsError::SensitivePath { pattern, .. } => FsError::SensitivePath {
                path: path.clone(),
                pattern,
            },
            e => e,
        })?;
    }
    Ok((path, target))
}

/// The workspace-relative form of `target`, already known to be inside.
fn relative(guard: &WorkspaceGuard, target: &Path) -> Option<WorkspacePath> {
    let rest = target.strip_prefix(guard.root().as_path()).ok()?;
    if rest.as_os_str().is_empty() {
        return Some(WorkspacePath::root());
    }
    WorkspacePath::parse(&rest.to_string_lossy()).ok()
}

/// Reads an existing text file, failing on directories and binaries.
async fn read_existing(path: &WorkspacePath, real: &Path) -> Result<String, FsError> {
    let meta = tokio::fs::metadata(real)
        .await
        .map_err(|e| io_error(path, real, &e))?;
    if meta.is_dir() {
        return Err(FsError::IsDirectory { path: path.clone() });
    }
    read_text(path, real).await
}

/// Unified diff from `old` (`None` for a new file, shown as `/dev/null`) to
/// `new`, with [`DIFF_CONTEXT`] lines of context; empty when nothing
/// changes.
pub(crate) fn unified_diff(path: &WorkspacePath, old: Option<&str>, new: &str) -> String {
    let old_name = match old {
        Some(_) => format!("a/{path}"),
        None => "/dev/null".to_owned(),
    };
    TextDiff::from_lines(old.unwrap_or(""), new)
        .unified_diff()
        .context_radius(DIFF_CONTEXT)
        .header(&old_name, &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use domain::workspace::PathError;

    use super::*;

    pub(super) struct Fixture {
        _dir: tempfile::TempDir,
        pub ws: PathBuf,
        pub guard: WorkspaceGuard,
    }

    pub(super) fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let guard = WorkspaceGuard::open(&ws).unwrap();
        Fixture {
            _dir: dir,
            ws,
            guard,
        }
    }

    pub(super) fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn edit(path: &str, old: &str, new: &str) -> EditFileRequest {
        EditFileRequest {
            path: p(path),
            old: old.into(),
            new: new.into(),
        }
    }

    fn write(path: &str, content: &str) -> WriteFileRequest {
        WriteFileRequest {
            path: p(path),
            content: content.into(),
        }
    }

    #[tokio::test]
    async fn write_of_new_file_diffs_against_empty() {
        let f = fixture();
        let change = prepare_write(&f.guard, write("src/new.rs", "a\n"))
            .await
            .unwrap();
        assert!(change.creates());
        assert_eq!(change.original(), None);
        assert_eq!(
            change.diff_unified(),
            "--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1 @@\n+a\n"
        );
        assert!(!f.ws.join("src").exists(), "nothing is written yet");
    }

    #[tokio::test]
    async fn write_over_existing_file_diffs_against_it() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "old\n").unwrap();
        let change = prepare_write(&f.guard, write("a.txt", "new\n"))
            .await
            .unwrap();
        assert_eq!(change.original(), Some("old\n"));
        assert_eq!(
            change.diff_unified(),
            "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.ws.join("a.txt")).unwrap(),
            "old\n"
        );
    }

    #[tokio::test]
    async fn write_is_denied_outside_the_workspace_and_on_sensitive_paths() {
        let f = fixture();
        std::fs::create_dir(f.ws.join(".git")).unwrap();
        std::fs::create_dir(f.ws.join("src")).unwrap();
        symlink(f.ws.parent().unwrap(), f.ws.join("up")).unwrap();
        symlink(f.ws.join(".git"), f.ws.join("hooks")).unwrap();
        assert!(matches!(
            prepare_write(&f.guard, write(".git/hooks/pre-commit", "x")).await,
            Err(FsError::SensitivePath { .. })
        ));
        assert_eq!(
            prepare_write(&f.guard, write("hooks/config", "x")).await,
            Err(FsError::SensitivePath {
                path: p("hooks/config"),
                pattern: ".git/".into()
            }),
            "a symlink into .git/ is denied too"
        );
        assert!(matches!(
            prepare_write(&f.guard, write(".env", "KEY=1")).await,
            Err(FsError::SensitivePath { .. })
        ));
        assert!(matches!(
            prepare_write(&f.guard, write("up/escape.txt", "x")).await,
            Err(FsError::Path(PathError::OutsideWorkspace { .. }))
        ));
        assert_eq!(
            prepare_write(&f.guard, write("src", "x")).await,
            Err(FsError::IsDirectory { path: p("src") })
        );
    }

    async fn applied(f: &Fixture, change: PendingChange) -> Result<FileChange, FsError> {
        apply(&f.guard, change).await
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn apply_writes_edits_and_creates_parents() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "one\ntwo\n").unwrap();
        let change = prepare_edit(&f.guard, edit("a.txt", "two", "2"))
            .await
            .unwrap();
        let diff = change.diff_unified().to_owned();
        assert_eq!(
            applied(&f, change).await,
            Ok(FileChange {
                path: p("a.txt"),
                diff_unified: diff,
                created: false
            })
        );
        assert_eq!(
            std::fs::read_to_string(f.ws.join("a.txt")).unwrap(),
            "one\n2\n"
        );

        let create = prepare_write(&f.guard, write("src/deep/new.rs", "fn main() {}\n"))
            .await
            .unwrap();
        assert!(applied(&f, create).await.unwrap().created);
        assert_eq!(
            std::fs::read_to_string(f.ws.join("src/deep/new.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert_eq!(
            entries(&f.ws),
            ["a.txt", "src"],
            "no temporary file is left"
        );
        assert_eq!(entries(&f.ws.join("src/deep")), ["new.rs"]);
    }

    #[tokio::test]
    async fn apply_detects_stale_changes() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "one\n").unwrap();
        let edit = prepare_edit(&f.guard, edit("a.txt", "one", "1"))
            .await
            .unwrap();
        std::fs::write(f.ws.join("a.txt"), "changed\n").unwrap();
        assert_eq!(
            applied(&f, edit).await,
            Err(FsError::Stale { path: p("a.txt") })
        );
        assert_eq!(
            std::fs::read_to_string(f.ws.join("a.txt")).unwrap(),
            "changed\n"
        );

        let create = prepare_write(&f.guard, write("new.txt", "a"))
            .await
            .unwrap();
        std::fs::write(f.ws.join("new.txt"), "b").unwrap();
        assert_eq!(
            applied(&f, create).await,
            Err(FsError::Stale { path: p("new.txt") })
        );

        let overwrite = prepare_write(&f.guard, write("new.txt", "c"))
            .await
            .unwrap();
        std::fs::remove_file(f.ws.join("new.txt")).unwrap();
        assert_eq!(
            applied(&f, overwrite).await,
            Err(FsError::Stale { path: p("new.txt") })
        );
        assert!(!f.ws.join("new.txt").exists());
    }

    #[tokio::test]
    async fn apply_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let f = fixture();
        std::fs::write(f.ws.join("run.sh"), "echo a\n").unwrap();
        std::fs::set_permissions(f.ws.join("run.sh"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        symlink("run.sh", f.ws.join("link.sh")).unwrap();
        let change = prepare_write(&f.guard, write("link.sh", "echo b\n"))
            .await
            .unwrap();
        applied(&f, change).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(f.ws.join("run.sh")).unwrap(),
            "echo b\n"
        );
        let mode = std::fs::metadata(f.ws.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o750);
        assert!(
            std::fs::symlink_metadata(f.ws.join("link.sh"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link is kept and its target rewritten"
        );
    }

    #[tokio::test]
    async fn apply_rechecks_the_guards() {
        let f = fixture();
        std::fs::create_dir(f.ws.join("dir")).unwrap();
        let change = prepare_write(&f.guard, write("dir/a.txt", "a"))
            .await
            .unwrap();
        std::fs::remove_dir(f.ws.join("dir")).unwrap();
        symlink(f.ws.parent().unwrap(), f.ws.join("dir")).unwrap();
        assert!(matches!(
            applied(&f, change).await,
            Err(FsError::Path(PathError::OutsideWorkspace { .. }))
        ));
        assert!(!f.ws.parent().unwrap().join("a.txt").exists());
    }

    #[test]
    fn diff_has_header_and_three_lines_of_context() {
        let old: String = (1..=10).map(|n| format!("{n}\n")).collect();
        let new = old.replace("5\n", "five\n");
        assert_eq!(
            unified_diff(&p("n.txt"), Some(&old), &new),
            "--- a/n.txt\n+++ b/n.txt\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n"
        );
        assert_eq!(unified_diff(&p("n.txt"), Some(&old), &old), "");
    }

    #[test]
    fn diff_of_new_file_is_against_dev_null() {
        assert_eq!(
            unified_diff(&p("new.txt"), None, "a\nb"),
            "--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n"
        );
    }

    #[tokio::test]
    async fn edit_replaces_the_unique_match_without_writing() {
        let f = fixture();
        std::fs::write(f.ws.join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let change = prepare_edit(&f.guard, edit("a.rs", "two", "three"))
            .await
            .unwrap();
        assert_eq!(change.path(), &p("a.rs"));
        assert_eq!(change.original(), Some("fn one() {}\nfn two() {}\n"));
        assert_eq!(change.new_content(), "fn one() {}\nfn three() {}\n");
        assert_eq!(
            change.diff_unified(),
            "--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n fn one() {}\n-fn two() {}\n+fn three() {}\n"
        );
        assert!(!change.creates());
        assert_eq!(
            std::fs::read_to_string(f.ws.join("a.rs")).unwrap(),
            "fn one() {}\nfn two() {}\n"
        );
    }

    #[tokio::test]
    async fn edit_requires_a_single_exact_match() {
        let f = fixture();
        std::fs::write(f.ws.join("a.rs"), "x x\n").unwrap();
        let prepare = |old: &'static str| prepare_edit(&f.guard, edit("a.rs", old, "y"));
        assert_eq!(prepare("x").await, Err(FsError::OldNotUnique { count: 2 }));
        assert_eq!(prepare("z").await, Err(FsError::OldNotFound));
        assert_eq!(prepare("").await, Err(FsError::EmptyOld));
    }

    #[tokio::test]
    async fn edit_fails_on_missing_directories_binaries_and_sensitive_paths() {
        let f = fixture();
        std::fs::create_dir_all(f.ws.join(".git")).unwrap();
        std::fs::write(f.ws.join(".git/config"), "[core]\n").unwrap();
        std::fs::write(f.ws.join("b.bin"), b"\0x").unwrap();
        assert_eq!(
            prepare_edit(&f.guard, edit("nope.rs", "a", "b")).await,
            Err(FsError::NotFound { path: p("nope.rs") })
        );
        assert!(matches!(
            prepare_edit(&f.guard, edit(".git/config", "core", "x")).await,
            Err(FsError::SensitivePath { .. })
        ));
        assert_eq!(
            prepare_edit(&f.guard, edit("b.bin", "x", "y")).await,
            Err(FsError::Binary { path: p("b.bin") })
        );
        std::fs::create_dir(f.ws.join("src")).unwrap();
        assert_eq!(
            prepare_edit(&f.guard, edit("src", "a", "b")).await,
            Err(FsError::IsDirectory { path: p("src") })
        );
    }
}
