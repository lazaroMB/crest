//! `edit_file` and `write_file`, in two phases around the user's approval.
//!
//! `prepare_*` reads the current content, computes the new one and its
//! unified diff (with `similar`) and returns a [`PendingChange`] without
//! touching the disk.
//!
//! # Sensitive paths
//!
//! Writes are denied by the same sensitive-path rules as reads (`.git/`,
//! `.env*`, keys…), on both the requested path and the one its symlinks
//! resolve to: rewriting `.git/hooks/*` or `.git/config` would run code
//! outside the approval flow, and a file the model may not read is not one
//! it should overwrite blindly. This matches the in-memory fake.

use std::path::Path;

use application::ports::fs::{EditFileRequest, FsError, PendingChange, replace_unique};
use domain::workspace::WorkspacePath;
use similar::TextDiff;

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
    use std::path::PathBuf;

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
