//! FileSystemPort adapter with workspace guards.
//!
//! [`guard::WorkspaceGuard`] confines every path to the canonical workspace
//! root, resolving symlinks so that none can escape it, and denies reads of
//! sensitive paths. [`LocalFs`] implements [`FileSystemPort`] on top of it.
//!
//! Layout: `read_file` lives in `read.rs` (task T2.D2) and the two-phase
//! `edit_file`/`write_file` (`prepare_edit`, `prepare_write`, `apply`) in
//! `write.rs` (task T2.D3). Until those land, the corresponding methods
//! fail with [`FsError::Io`] naming the pending task. Every one of them must
//! go through the guard: [`WorkspaceGuard::resolve_readable`] for reads and
//! [`WorkspaceGuard::resolve_for_write`] for writes, re-checked in `apply`.

pub mod guard;

use application::ports::fs::{
    EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, PendingChange,
    ReadFileRequest, WriteFileRequest,
};
use async_trait::async_trait;
use domain::workspace::{WorkspacePath, WorkspaceRoot};

pub use guard::WorkspaceGuard;

/// [`FileSystemPort`] over the local disk, confined by a [`WorkspaceGuard`].
#[derive(Debug, Clone)]
pub struct LocalFs {
    guard: WorkspaceGuard,
}

impl LocalFs {
    /// An adapter for the workspace confined by `guard`.
    pub fn new(guard: WorkspaceGuard) -> Self {
        Self { guard }
    }

    /// The guard every operation goes through.
    pub fn guard(&self) -> &WorkspaceGuard {
        &self.guard
    }
}

fn not_implemented(task: &str) -> FsError {
    FsError::Io(format!("not implemented yet: {task}"))
}

#[async_trait]
impl FileSystemPort for LocalFs {
    fn root(&self) -> &WorkspaceRoot {
        self.guard.root()
    }

    async fn read_file(&self, _request: ReadFileRequest) -> Result<FileContent, FsError> {
        Err(not_implemented("T2.D2"))
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        match self.guard.resolve_existing(path.as_str()).await {
            Ok(_) => Ok(true),
            Err(FsError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn prepare_edit(&self, _request: EditFileRequest) -> Result<PendingChange, FsError> {
        Err(not_implemented("T2.D3"))
    }

    async fn prepare_write(&self, _request: WriteFileRequest) -> Result<PendingChange, FsError> {
        Err(not_implemented("T2.D3"))
    }

    async fn apply(&self, _change: PendingChange) -> Result<FileChange, FsError> {
        Err(not_implemented("T2.D3"))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use domain::workspace::PathError;

    use super::*;

    #[tokio::test]
    async fn exists_reports_files_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "a").unwrap();
        symlink(dir.path(), ws.join("up")).unwrap();
        let fs = LocalFs::new(WorkspaceGuard::open(&ws).unwrap());
        let path = |p: &str| WorkspacePath::parse(p).unwrap();

        assert_eq!(fs.root().as_path(), std::fs::canonicalize(&ws).unwrap());
        assert_eq!(fs.exists(&path("a.txt")).await, Ok(true));
        assert_eq!(fs.exists(&WorkspacePath::root()).await, Ok(true));
        assert_eq!(fs.exists(&path("b.txt")).await, Ok(false));
        assert!(matches!(
            fs.exists(&path("up")).await,
            Err(FsError::Path(PathError::OutsideWorkspace { .. }))
        ));
    }

    #[test]
    fn local_fs_is_a_port() {
        fn assert_port<T: FileSystemPort + 'static>() {}
        assert_port::<LocalFs>();
    }
}
