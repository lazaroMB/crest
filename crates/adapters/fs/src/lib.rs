//! FileSystemPort adapter with workspace guards.
//!
//! [`guard::WorkspaceGuard`] confines every path to the canonical workspace
//! root, resolving symlinks so that none can escape it, and denies reads
//! and writes of sensitive paths. [`LocalFs`] implements [`FileSystemPort`]
//! on top of it.
//!
//! Layout: `read_file` lives in `read.rs` (task T2.D2) and the two-phase
//! `edit_file`/`write_file` (`prepare_edit`, `prepare_write`, `apply`) in
//! `write.rs` (task T2.D3). Every one of them goes through the guard:
//! [`WorkspaceGuard::resolve_readable`] for reads and
//! [`WorkspaceGuard::resolve_for_write`] for writes, re-checked in `apply`.
//!
//! [`LocalProjectBrowser`] (`browse.rs`) lists directories anywhere on the
//! disk to choose the project to open, before there is a workspace.

pub mod browse;
pub mod guard;
mod read;
mod write;

use application::ports::fs::{
    DirEntry, EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, PendingChange,
    ReadFileRequest, WriteFileRequest,
};
use async_trait::async_trait;
use domain::workspace::{WorkspacePath, WorkspaceRoot};

pub use browse::LocalProjectBrowser;
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

#[async_trait]
impl FileSystemPort for LocalFs {
    fn root(&self) -> &WorkspaceRoot {
        self.guard.root()
    }

    async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent, FsError> {
        read::read_file(&self.guard, &request).await
    }

    async fn read_bytes(&self, path: &WorkspacePath, max: u64) -> Result<Vec<u8>, FsError> {
        read::read_bytes(&self.guard, path, max).await
    }

    async fn read_head(&self, path: &WorkspacePath, max: u64) -> Result<(Vec<u8>, u64), FsError> {
        read::read_head(&self.guard, path, max).await
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        match self.guard.resolve_existing(path.as_str()).await {
            Ok(_) => Ok(true),
            Err(FsError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn list_dir(&self, path: &WorkspacePath) -> Result<Vec<DirEntry>, FsError> {
        read::list_dir(&self.guard, path).await
    }

    async fn prepare_edit(&self, request: EditFileRequest) -> Result<PendingChange, FsError> {
        write::prepare_edit(&self.guard, request).await
    }

    async fn prepare_write(&self, request: WriteFileRequest) -> Result<PendingChange, FsError> {
        write::prepare_write(&self.guard, request).await
    }

    async fn apply(&self, change: PendingChange) -> Result<FileChange, FsError> {
        write::apply(&self.guard, change).await
    }
}

// The fixtures build symlinks and permission modes, which are Unix-only.
#[cfg(test)]
#[cfg(unix)]
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

    #[tokio::test]
    async fn writes_through_the_port_are_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let fs = LocalFs::new(WorkspaceGuard::open(dir.path()).unwrap());
        let path = WorkspacePath::parse("src/a.txt").unwrap();
        let write = WriteFileRequest {
            path: path.clone(),
            content: "one\ntwo\n".into(),
        };
        fs.apply(fs.prepare_write(write).await.unwrap())
            .await
            .unwrap();
        let edit = EditFileRequest {
            path: path.clone(),
            old: "two".into(),
            new: "2".into(),
        };
        fs.apply(fs.prepare_edit(edit).await.unwrap())
            .await
            .unwrap();
        let content = fs.read_file(ReadFileRequest::new(path)).await.unwrap();
        assert_eq!(content.render_numbered(), "     1\tone\n     2\t2\n");
    }

    #[tokio::test]
    async fn list_dir_sorts_entries_and_hides_sensitive_ones() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("target/criterion/fib")).unwrap();
        std::fs::write(ws.join("target/criterion/b.json"), "{}").unwrap();
        std::fs::write(ws.join(".env"), "SECRET=1").unwrap();
        symlink(ws.join("target"), ws.join("link")).unwrap();
        let fs = LocalFs::new(WorkspaceGuard::open(&ws).unwrap());
        let path = |p: &str| WorkspacePath::parse(p).unwrap();

        let root = fs.list_dir(&WorkspacePath::root()).await.unwrap();
        let names: Vec<_> = root.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("link", true), ("target", true)]);
        let inner = fs.list_dir(&path("target/criterion")).await.unwrap();
        let names: Vec<_> = inner.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("b.json", false), ("fib", true)]);
        assert!(matches!(
            fs.list_dir(&path("nope")).await,
            Err(FsError::NotFound { .. })
        ));
        assert!(matches!(
            fs.list_dir(&path("target/criterion/b.json")).await,
            Err(FsError::Io(_))
        ));
    }

    #[test]
    fn local_fs_is_a_port() {
        fn assert_port<T: FileSystemPort + 'static>() {}
        assert_port::<LocalFs>();
    }
}
