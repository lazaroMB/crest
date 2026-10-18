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
    /// [`PathError::OutsideWorkspace`] when either escapes the root and with
    /// [`FsError::NotFound`] when the path does not exist.
    pub async fn resolve_existing(&self, input: &str) -> Result<(WorkspacePath, PathBuf), FsError> {
        let path = self.root.resolve(input)?;
        let joined = self.root.join(&path);
        let canonical = match tokio::fs::canonicalize(&joined).await {
            Ok(canonical) => canonical,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(FsError::NotFound { path }),
            Err(e) => return Err(io_error(&joined, &e)),
        };
        self.ensure_inside(input, &canonical)?;
        Ok((path, canonical))
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
}
