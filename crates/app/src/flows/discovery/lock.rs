//! The lock of an open discovery's folder: while a discovery is open the
//! tools cannot edit or write anything under `.crest/specs/<id>/`; the
//! engine writes there through the plain file port, as the spec sink does.
//!
//! [`DiscoveryLock`] is the shared set of locked folders; [`LockedFs`]
//! wraps a [`FileSystemPort`] and denies `prepare_edit`, `prepare_write`
//! and `apply` for paths under a locked folder with
//! [`FsError::SensitivePath`], the same error a sensitive path gives.

use std::collections::BTreeSet;
use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use domain::workspace::{WorkspacePath, WorkspaceRoot};

use crate::flows::research::nodes::spec::sink::SPECS_DIR;
use crate::ports::discovery::DiscoveryId;
use crate::ports::fs::{
    DirEntry, EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, PendingChange,
    ReadFileRequest, WriteFileRequest,
};

/// The folders locked right now, workspace-relative (`.crest/specs/0007-x`).
#[derive(Debug, Clone, Default)]
pub struct DiscoveryLock {
    folders: Arc<RwLock<BTreeSet<String>>>,
}

impl DiscoveryLock {
    /// Nothing locked.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The folder of `id`.
    #[must_use]
    pub fn folder_of(id: &DiscoveryId) -> String {
        format!("{SPECS_DIR}/{id}")
    }

    /// Locks the folder of `id`.
    pub fn lock(&self, id: &DiscoveryId) {
        self.folders
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(Self::folder_of(id));
    }

    /// Unlocks the folder of `id`.
    pub fn unlock(&self, id: &DiscoveryId) {
        self.folders
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&Self::folder_of(id));
    }

    /// The locked folder `path` is under, if any.
    #[must_use]
    pub fn locked_by(&self, path: &WorkspacePath) -> Option<String> {
        let folders = self.folders.read().unwrap_or_else(PoisonError::into_inner);
        let path = path.as_str();
        folders
            .iter()
            .find(|f| path == f.as_str() || path.starts_with(&format!("{f}/")))
            .cloned()
    }

    /// The locked folders, sorted.
    #[must_use]
    pub fn folders(&self) -> Vec<String> {
        self.folders
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

/// A [`FileSystemPort`] that refuses to change files under a locked
/// discovery folder.
pub struct LockedFs {
    inner: Arc<dyn FileSystemPort>,
    lock: DiscoveryLock,
}

impl std::fmt::Debug for LockedFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockedFs")
            .field("locked", &self.lock.folders())
            .finish_non_exhaustive()
    }
}

impl LockedFs {
    /// `inner` guarded by `lock`.
    #[must_use]
    pub fn new(inner: Arc<dyn FileSystemPort>, lock: DiscoveryLock) -> Self {
        Self { inner, lock }
    }

    fn check(&self, path: &WorkspacePath) -> Result<(), FsError> {
        match self.lock.locked_by(path) {
            Some(folder) => Err(FsError::SensitivePath {
                path: path.clone(),
                pattern: format!("{folder}/ (open discovery; `/research` writes it)"),
            }),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl FileSystemPort for LockedFs {
    fn root(&self) -> &WorkspaceRoot {
        self.inner.root()
    }

    async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent, FsError> {
        self.inner.read_file(request).await
    }

    async fn read_bytes(&self, path: &WorkspacePath, max: u64) -> Result<Vec<u8>, FsError> {
        self.inner.read_bytes(path, max).await
    }

    async fn read_head(&self, path: &WorkspacePath, max: u64) -> Result<(Vec<u8>, u64), FsError> {
        self.inner.read_head(path, max).await
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        self.inner.exists(path).await
    }

    async fn list_dir(&self, path: &WorkspacePath) -> Result<Vec<DirEntry>, FsError> {
        self.inner.list_dir(path).await
    }

    async fn prepare_edit(&self, request: EditFileRequest) -> Result<PendingChange, FsError> {
        self.check(&request.path)?;
        self.inner.prepare_edit(request).await
    }

    async fn prepare_write(&self, request: WriteFileRequest) -> Result<PendingChange, FsError> {
        self.check(&request.path)?;
        self.inner.prepare_write(request).await
    }

    async fn apply(&self, change: PendingChange) -> Result<FileChange, FsError> {
        self.check(change.path())?;
        self.inner.apply(change).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fs::InMemoryFs;

    #[tokio::test]
    async fn locked_folders_refuse_writes_until_unlocked() {
        let dir = std::env::temp_dir();
        let root = WorkspaceRoot::new(dir).unwrap();
        let inner: Arc<dyn FileSystemPort> = Arc::new(InMemoryFs::new(root));
        let lock = DiscoveryLock::new();
        let fs = LockedFs::new(inner, lock.clone());
        let id = DiscoveryId::new("0007-x");
        lock.lock(&id);
        let inside = WorkspacePath::parse(".crest/specs/0007-x/spec.md").unwrap();
        let request = || WriteFileRequest {
            path: inside.clone(),
            content: "x".into(),
        };
        let err = fs.prepare_write(request()).await.unwrap_err();
        assert!(
            matches!(err, FsError::SensitivePath { ref pattern, .. } if pattern.contains("open discovery")),
            "{err}"
        );
        let sibling = WorkspacePath::parse(".crest/specs/0007-xy/spec.md").unwrap();
        assert!(
            fs.prepare_write(WriteFileRequest {
                path: sibling,
                content: "x".into(),
            })
            .await
            .is_ok()
        );
        assert_eq!(
            lock.locked_by(&inside).as_deref(),
            Some(".crest/specs/0007-x")
        );
        lock.unlock(&id);
        assert!(fs.prepare_write(request()).await.is_ok());
        assert!(lock.folders().is_empty());
    }
}
