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

use std::path::Path;

use application::ports::fs::FsError;
use domain::workspace::{SensitivePathRules, WorkspaceRoot};

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
}

/// Renders an I/O error with the path it happened on.
fn io_error(path: &Path, error: &std::io::Error) -> FsError {
    FsError::Io(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
