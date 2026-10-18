//! Workspace guard: confines file system access to the canonical root.
//!
//! The domain resolves paths lexically ([`WorkspaceRoot::resolve`]); this
//! guard adds what only the file system knows: it canonicalizes the joined
//! path (or its nearest existing ancestor when it does not exist yet), which
//! resolves every symlink, and rejects any result outside the root with
//! [`PathError::OutsideWorkspace`].
//!
//! # Limitation: TOCTOU
//!
//! The check and the later open are separate system calls, so a process
//! that swaps a component for a symlink between them can still escape the
//! root. The guard protects against the model's requests, not against a
//! concurrent attacker with write access to the workspace; closing that race
//! would need `openat2(RESOLVE_BENEATH)`, which is not portable.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use application::ports::fs::FsError;
use domain::workspace::{PathError, SensitivePathRules, WorkspacePath, WorkspaceRoot};

/// Confines paths to one canonical workspace root.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root: WorkspaceRoot,
    rules: SensitivePathRules,
}

impl WorkspaceGuard {
    /// Opens the workspace at `path` with the default sensitive-path rules.
    ///
    /// The root is canonicalized once here (so a root that is itself a
    /// symlink is replaced by its target) and never changes afterwards.
    /// Fails when `path` does not exist or is not a directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FsError> {
        let path = path.as_ref();
        let canonical = std::fs::canonicalize(path).map_err(|e| io_error(path, &e))?;
        if !canonical.is_dir() {
            return Err(FsError::Io(format!(
                "workspace root is not a directory: {}",
                path.display()
            )));
        }
        let root = WorkspaceRoot::new(canonical)?;
        tracing::debug!(root = %root.as_path().display(), "workspace opened");
        Ok(Self {
            root,
            rules: SensitivePathRules::default(),
        })
    }

    /// Replaces the sensitive-path rules (configured from Lua).
    pub fn with_sensitive_rules(mut self, rules: SensitivePathRules) -> Self {
        self.rules = rules;
        self
    }

    /// The canonical workspace root.
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    /// The sensitive-path rules in effect.
    pub fn sensitive_rules(&self) -> &SensitivePathRules {
        &self.rules
    }

    /// Resolves `input` to an existing file or directory inside the root.
    ///
    /// Returns the lexical [`WorkspacePath`] (as the caller named it) and
    /// the canonical absolute path, with every symlink resolved. Fails with
    /// [`PathError::OutsideWorkspace`] when either escapes the root (also
    /// for a dangling symlink whose target lies outside) and with
    /// [`FsError::NotFound`] when the path does not exist.
    pub async fn resolve_existing(&self, input: &str) -> Result<(WorkspacePath, PathBuf), FsError> {
        let path = self.root.resolve(input)?;
        let joined = self.root.join(&path);
        let canonical = match tokio::fs::canonicalize(&joined).await {
            Ok(canonical) => canonical,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // A dangling symlink pointing outside must not look like a
                // plain missing file inside the workspace.
                return match resolve_lenient(&joined).await? {
                    Some(target) => {
                        self.ensure_inside(input, &target)?;
                        Err(FsError::NotFound { path })
                    }
                    None => Err(PathError::OutsideWorkspace {
                        path: input.to_owned(),
                    }
                    .into()),
                };
            }
            Err(e) => return Err(io_error(&joined, &e)),
        };
        self.ensure_inside(input, &canonical)?;
        Ok((path, canonical))
    }

    /// Resolves `input` to the place a write would land, which may not exist.
    ///
    /// The nearest existing ancestor is canonicalized and the missing tail
    /// appended, so a missing file in missing nested directories is checked
    /// against its first real ancestor. Dangling symlinks are followed to
    /// where they would create the file. Fails with
    /// [`PathError::OutsideWorkspace`] when the result escapes the root,
    /// including through an intermediate symlinked directory or a symlinked
    /// file pointing outside.
    pub async fn resolve_for_write(
        &self,
        input: &str,
    ) -> Result<(WorkspacePath, PathBuf), FsError> {
        let path = self.root.resolve(input)?;
        let joined = self.root.join(&path);
        let target =
            resolve_lenient(&joined)
                .await?
                .ok_or_else(|| PathError::OutsideWorkspace {
                    path: input.to_owned(),
                })?;
        self.ensure_inside(input, &target)?;
        Ok((path, target))
    }

    /// Fails with [`FsError::SensitivePath`] when `path` matches one of the
    /// sensitive-path rules (any component, so `.git/config` is denied by
    /// `.git/`).
    pub fn check_readable(&self, path: &WorkspacePath) -> Result<(), FsError> {
        match self.rules.matching_pattern(path) {
            Some(pattern) => Err(FsError::SensitivePath {
                path: path.clone(),
                pattern: pattern.to_owned(),
            }),
            None => Ok(()),
        }
    }

    /// [`Self::resolve_existing`] followed by [`Self::check_readable`] on
    /// both the requested path and the symlink-resolved one, so an innocent
    /// name linking to `.env` is denied too.
    pub async fn resolve_readable(&self, input: &str) -> Result<(WorkspacePath, PathBuf), FsError> {
        let (path, canonical) = self.resolve_existing(input).await?;
        self.check_readable(&path)?;
        if let Some(real) = self.relative(&canonical)
            && real != path
            && let Some(pattern) = self.rules.matching_pattern(&real)
        {
            return Err(FsError::SensitivePath {
                path,
                pattern: pattern.to_owned(),
            });
        }
        Ok((path, canonical))
    }

    /// The workspace-relative form of a canonical path inside the root.
    fn relative(&self, canonical: &Path) -> Option<WorkspacePath> {
        let rest = canonical.strip_prefix(self.root.as_path()).ok()?;
        if rest.as_os_str().is_empty() {
            return Some(WorkspacePath::root());
        }
        WorkspacePath::parse(&rest.to_string_lossy()).ok()
    }

    /// Fails with [`PathError::OutsideWorkspace`] unless `canonical` is the
    /// root or below it. [`Path::starts_with`] compares whole components,
    /// so `/ws-other` is not inside `/ws`.
    fn ensure_inside(&self, input: &str, canonical: &Path) -> Result<(), FsError> {
        if canonical.starts_with(self.root.as_path()) {
            Ok(())
        } else {
            tracing::warn!(input, resolved = %canonical.display(), "path escapes the workspace");
            Err(PathError::OutsideWorkspace {
                path: input.to_owned(),
            }
            .into())
        }
    }
}

/// How many dangling symlinks [`resolve_lenient`] follows, as Linux's
/// `MAXSYMLINKS`.
const MAX_SYMLINK_HOPS: usize = 40;

/// Canonicalizes `path` even when its last components do not exist: the
/// nearest existing ancestor is canonicalized and the missing names are
/// appended. A dangling symlink is replaced by its target and resolution
/// starts over.
///
/// Returns `None` when the result cannot be trusted to stay put: a missing
/// tail that contains `..` (which the kernel would resolve against the
/// symlink targets, not lexically) or too many dangling symlinks.
async fn resolve_lenient(path: &Path) -> Result<Option<PathBuf>, FsError> {
    let mut current = path.to_path_buf();
    // Missing names, innermost first.
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut hops = 0;
    loop {
        match tokio::fs::canonicalize(&current).await {
            Ok(canonical) => {
                return Ok(Some(tail.iter().rev().fold(canonical, |p, n| p.join(n))));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&current, &e)),
        }
        match tokio::fs::symlink_metadata(&current).await {
            Ok(meta) if meta.file_type().is_symlink() => {
                hops += 1;
                if hops > MAX_SYMLINK_HOPS {
                    return Ok(None);
                }
                let link = tokio::fs::read_link(&current)
                    .await
                    .map_err(|e| io_error(&current, &e))?;
                current = match current.parent() {
                    Some(parent) => parent.join(link),
                    None => link,
                };
            }
            Ok(_) => {
                return Err(FsError::Io(format!(
                    "{}: cannot be canonicalized",
                    current.display()
                )));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (current.file_name(), current.parent()) else {
                    // Ends in `..` or is `/`: refuse to guess.
                    return Ok(None);
                };
                tail.push(name.to_owned());
                current = parent.to_path_buf();
            }
            Err(e) => return Err(io_error(&current, &e)),
        }
    }
}

/// Renders an I/O error with the path it happened on.
fn io_error(path: &Path, error: &std::io::Error) -> FsError {
    FsError::Io(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A workspace `ws/` next to an `outside/` directory holding `secret`.
    struct Fixture {
        _dir: tempfile::TempDir,
        ws: PathBuf,
        outside: PathBuf,
        guard: WorkspaceGuard,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let ws = base.join("ws");
        let outside = base.join("outside");
        std::fs::create_dir(&ws).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        let guard = WorkspaceGuard::open(&ws).unwrap();
        Fixture {
            _dir: dir,
            ws,
            outside,
            guard,
        }
    }

    fn is_outside(result: Result<impl std::fmt::Debug, FsError>) -> bool {
        matches!(
            result,
            Err(FsError::Path(PathError::OutsideWorkspace { .. }))
        )
    }

    #[test]
    fn open_canonicalizes_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let guard = WorkspaceGuard::open(dir.path().join("./")).unwrap();
        assert_eq!(guard.root().as_path(), canonical);
    }

    #[test]
    fn open_resolves_a_symlinked_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let guard = WorkspaceGuard::open(&link).unwrap();
        assert_eq!(
            guard.root().as_path(),
            std::fs::canonicalize(&real).unwrap()
        );
    }

    #[test]
    fn open_rejects_missing_or_file_roots() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            WorkspaceGuard::open(dir.path().join("missing")),
            Err(FsError::Io(_))
        ));
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(WorkspaceGuard::open(&file), Err(FsError::Io(_))));
    }

    #[tokio::test]
    async fn existing_resolves_a_normal_file() {
        let f = fixture();
        std::fs::create_dir(f.ws.join("src")).unwrap();
        std::fs::write(f.ws.join("src/main.rs"), "fn main() {}").unwrap();

        let (path, real) = f.guard.resolve_existing("./src//main.rs").await.unwrap();
        assert_eq!(path.as_str(), "src/main.rs");
        assert_eq!(real, f.ws.join("src/main.rs"));

        let absolute = f.ws.join("src/main.rs");
        let (path, _) = f
            .guard
            .resolve_existing(absolute.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(path.as_str(), "src/main.rs");
    }

    #[tokio::test]
    async fn existing_rejects_lexical_escape() {
        let f = fixture();
        assert!(is_outside(
            f.guard.resolve_existing("../outside/secret").await
        ));
        assert!(is_outside(f.guard.resolve_existing("a/../../x").await));
        let secret = f.outside.join("secret");
        assert!(is_outside(
            f.guard.resolve_existing(secret.to_str().unwrap()).await
        ));
    }

    #[tokio::test]
    async fn existing_reports_missing_files() {
        let f = fixture();
        assert!(matches!(
            f.guard.resolve_existing("nope.txt").await,
            Err(FsError::NotFound { path }) if path.as_str() == "nope.txt"
        ));
    }

    #[tokio::test]
    async fn existing_rejects_symlink_to_outside_file() {
        let f = fixture();
        symlink(f.outside.join("secret"), f.ws.join("link")).unwrap();
        assert!(is_outside(f.guard.resolve_existing("link").await));
    }

    #[tokio::test]
    async fn existing_rejects_symlinked_directory_component() {
        let f = fixture();
        symlink(&f.outside, f.ws.join("out")).unwrap();
        assert!(is_outside(f.guard.resolve_existing("out/secret").await));
        assert!(is_outside(f.guard.resolve_existing("out").await));
    }

    #[tokio::test]
    async fn existing_allows_internal_symlink() {
        let f = fixture();
        std::fs::create_dir(f.ws.join("real")).unwrap();
        std::fs::write(f.ws.join("real/a.txt"), "a").unwrap();
        symlink("real", f.ws.join("alias")).unwrap();

        let (path, real) = f.guard.resolve_existing("alias/a.txt").await.unwrap();
        assert_eq!(path.as_str(), "alias/a.txt");
        assert_eq!(real, f.ws.join("real/a.txt"));
    }

    #[tokio::test]
    async fn existing_rejects_sibling_with_root_prefix() {
        let f = fixture();
        let sibling = f.ws.with_file_name("ws-other");
        std::fs::create_dir(&sibling).unwrap();
        std::fs::write(sibling.join("x"), "x").unwrap();
        symlink(sibling.join("x"), f.ws.join("x")).unwrap();
        assert!(is_outside(f.guard.resolve_existing("x").await));
    }

    #[tokio::test]
    async fn write_resolves_new_file_in_new_nested_dirs() {
        let f = fixture();
        let (path, target) = f.guard.resolve_for_write("a/b/c/new.txt").await.unwrap();
        assert_eq!(path.as_str(), "a/b/c/new.txt");
        assert_eq!(target, f.ws.join("a/b/c/new.txt"));
    }

    #[tokio::test]
    async fn write_resolves_existing_file() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "a").unwrap();
        let (_, target) = f.guard.resolve_for_write("a.txt").await.unwrap();
        assert_eq!(target, f.ws.join("a.txt"));
    }

    #[tokio::test]
    async fn write_rejects_lexical_escape() {
        let f = fixture();
        assert!(is_outside(
            f.guard.resolve_for_write("../outside/new").await
        ));
    }

    #[tokio::test]
    async fn write_rejects_symlinked_directory_component() {
        let f = fixture();
        symlink(&f.outside, f.ws.join("out")).unwrap();
        assert!(is_outside(f.guard.resolve_for_write("out/new.txt").await));
        assert!(is_outside(
            f.guard.resolve_for_write("out/x/y/new.txt").await
        ));
        assert!(is_outside(f.guard.resolve_for_write("out/secret").await));
    }

    #[tokio::test]
    async fn write_rejects_symlinked_file_pointing_outside() {
        let f = fixture();
        symlink(f.outside.join("secret"), f.ws.join("link")).unwrap();
        assert!(is_outside(f.guard.resolve_for_write("link").await));
    }

    #[tokio::test]
    async fn write_rejects_dangling_symlink_pointing_outside() {
        let f = fixture();
        symlink(f.outside.join("not-yet"), f.ws.join("dangling")).unwrap();
        assert!(is_outside(f.guard.resolve_for_write("dangling").await));
        symlink("../outside/missing/new", f.ws.join("relative")).unwrap();
        assert!(is_outside(f.guard.resolve_for_write("relative").await));
    }

    #[tokio::test]
    async fn write_follows_internal_dangling_symlink() {
        let f = fixture();
        symlink("real/new.txt", f.ws.join("alias")).unwrap();
        let (path, target) = f.guard.resolve_for_write("alias").await.unwrap();
        assert_eq!(path.as_str(), "alias");
        assert_eq!(target, f.ws.join("real/new.txt"));
    }

    #[tokio::test]
    async fn write_rejects_symlink_loops_and_dotdot_tails() {
        let f = fixture();
        symlink("loop-b", f.ws.join("loop-a")).unwrap();
        symlink("loop-a", f.ws.join("loop-b")).unwrap();
        assert!(f.guard.resolve_for_write("loop-a").await.is_err());
        symlink("missing/../../outside/x", f.ws.join("tricky")).unwrap();
        assert!(is_outside(f.guard.resolve_for_write("tricky").await));
    }

    #[tokio::test]
    async fn existing_rejects_dangling_symlink_pointing_outside() {
        let f = fixture();
        symlink(f.outside.join("not-yet"), f.ws.join("dangling")).unwrap();
        assert!(is_outside(f.guard.resolve_existing("dangling").await));
        symlink("real/missing", f.ws.join("inside")).unwrap();
        assert!(matches!(
            f.guard.resolve_existing("inside").await,
            Err(FsError::NotFound { .. })
        ));
    }

    fn sensitive(result: Result<impl std::fmt::Debug, FsError>) -> Option<String> {
        match result {
            Err(FsError::SensitivePath { pattern, .. }) => Some(pattern),
            _ => None,
        }
    }

    #[tokio::test]
    async fn readable_denies_default_sensitive_paths() {
        let f = fixture();
        std::fs::write(f.ws.join(".env"), "KEY=x").unwrap();
        std::fs::write(f.ws.join(".env.local"), "KEY=x").unwrap();
        std::fs::create_dir(f.ws.join(".git")).unwrap();
        std::fs::write(f.ws.join(".git/config"), "").unwrap();
        std::fs::write(f.ws.join("cert.pem"), "").unwrap();

        let guard = &f.guard;
        assert_eq!(
            sensitive(guard.resolve_readable(".env").await).as_deref(),
            Some(".env*")
        );
        assert_eq!(
            sensitive(guard.resolve_readable(".env.local").await).as_deref(),
            Some(".env*")
        );
        assert_eq!(
            sensitive(guard.resolve_readable(".git/config").await).as_deref(),
            Some(".git/")
        );
        assert_eq!(
            sensitive(guard.resolve_readable("cert.pem").await).as_deref(),
            Some("*.pem")
        );
    }

    #[tokio::test]
    async fn readable_denies_symlink_to_sensitive_file() {
        let f = fixture();
        std::fs::write(f.ws.join(".env"), "KEY=x").unwrap();
        symlink(".env", f.ws.join("innocent.txt")).unwrap();
        assert_eq!(
            sensitive(f.guard.resolve_readable("innocent.txt").await).as_deref(),
            Some(".env*")
        );
    }

    #[tokio::test]
    async fn readable_allows_normal_files_and_custom_rules() {
        let f = fixture();
        std::fs::write(f.ws.join(".env"), "KEY=x").unwrap();
        std::fs::write(f.ws.join("notes.md"), "n").unwrap();
        assert!(f.guard.resolve_readable("notes.md").await.is_ok());

        let guard = f
            .guard
            .clone()
            .with_sensitive_rules(SensitivePathRules::new(["*.md"]).unwrap());
        assert!(guard.resolve_readable(".env").await.is_ok());
        assert_eq!(
            sensitive(guard.resolve_readable("notes.md").await).as_deref(),
            Some("*.md")
        );
    }
}
