//! Where the Spec and Plan nodes write the folder of each Spec.
//!
//! The nodes do not go through the agent's tools, so writing the folder
//! never waits for an interactive approval: [`FsSpecSink`] writes straight
//! through [`FileSystemPort`], which still confines every path to the
//! workspace and its sensitive-path rules.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::workspace::WorkspacePath;

use crate::ports::fs::{FileSystemPort, WriteFileRequest};
use crate::ports::search::{FileSearchPort, ListFilesRequest};

/// Folder of the Specs inside the workspace.
pub const SPECS_DIR: &str = ".crest/specs";

/// Most paths read when listing [`SPECS_DIR`].
const LIST_LIMIT: usize = 10_000;

/// Why the folder of a Spec could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("spec folder: {0}")]
pub struct SpecSinkError(pub String);

/// The `.crest/specs/` folder.
#[async_trait]
pub trait SpecSink: Send + Sync {
    /// Names of the folders already in [`SPECS_DIR`], e.g.
    /// `0007-session-cache`.
    async fn folders(&self) -> Result<Vec<String>, SpecSinkError>;

    /// Writes `content` to `<SPECS_DIR>/<folder>/<file>`, replacing it.
    /// `file` may hold subfolders (`evidence/chart-1.csv`).
    async fn write(&self, folder: &str, file: &str, content: &str) -> Result<(), SpecSinkError>;
}

/// A [`SpecSink`] held in memory, for tests and for flows without a
/// workspace.
#[derive(Debug, Default)]
pub struct MemorySpecSink {
    files: Mutex<BTreeMap<String, String>>,
}

impl MemorySpecSink {
    /// An empty `.crest/specs/`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an existing folder with an empty `spec.md`.
    #[must_use]
    pub fn with_folder(self, folder: &str) -> Self {
        if let Ok(mut files) = self.files.lock() {
            files.insert(format!("{folder}/spec.md"), String::new());
        }
        self
    }

    /// Content of `<folder>/<file>`, if written.
    #[must_use]
    pub fn file(&self, folder: &str, file: &str) -> Option<String> {
        self.files
            .lock()
            .ok()
            .and_then(|f| f.get(&format!("{folder}/{file}")).cloned())
    }

    /// Every file as `<folder>/<file>`, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        self.files
            .lock()
            .map(|f| f.keys().cloned().collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl SpecSink for MemorySpecSink {
    async fn folders(&self) -> Result<Vec<String>, SpecSinkError> {
        let files = self
            .files
            .lock()
            .map_err(|_| SpecSinkError("poisoned lock".into()))?;
        let mut folders: Vec<String> = files
            .keys()
            .filter_map(|k| k.split('/').next())
            .map(str::to_owned)
            .collect();
        folders.dedup();
        Ok(folders)
    }

    async fn write(&self, folder: &str, file: &str, content: &str) -> Result<(), SpecSinkError> {
        self.files
            .lock()
            .map_err(|_| SpecSinkError("poisoned lock".into()))?
            .insert(format!("{folder}/{file}"), content.to_owned());
        Ok(())
    }
}

/// A [`SpecSink`] over the workspace: lists with [`FileSearchPort`] and
/// writes with [`FileSystemPort`] without asking for approval.
///
/// The listing honors ignore files, so a `.crest/` in `.gitignore` hides
/// the existing folders from the numbering.
pub struct FsSpecSink {
    fs: Arc<dyn FileSystemPort>,
    search: Arc<dyn FileSearchPort>,
}

impl std::fmt::Debug for FsSpecSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsSpecSink").finish_non_exhaustive()
    }
}

impl FsSpecSink {
    /// A sink writing through `fs` and listing through `search`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystemPort>, search: Arc<dyn FileSearchPort>) -> Self {
        Self { fs, search }
    }
}

#[async_trait]
impl SpecSink for FsSpecSink {
    async fn folders(&self) -> Result<Vec<String>, SpecSinkError> {
        let list = self
            .search
            .list_files(ListFilesRequest {
                glob: Some(format!("{SPECS_DIR}/**")),
                limit: LIST_LIMIT,
            })
            .await
            .map_err(|e| SpecSinkError(e.to_string()))?;
        let depth = SPECS_DIR.split('/').count();
        let mut folders: Vec<String> = list
            .paths
            .iter()
            .filter_map(|p| {
                let parts: Vec<&str> = p.components().collect();
                let inside = parts.len() > depth + 1 && parts.join("/").starts_with(SPECS_DIR);
                inside.then(|| parts[depth].to_owned())
            })
            .collect();
        folders.sort();
        folders.dedup();
        Ok(folders)
    }

    async fn write(&self, folder: &str, file: &str, content: &str) -> Result<(), SpecSinkError> {
        let bad = |c: &str| c.is_empty() || c == "." || c == "..";
        if folder.contains('/') || bad(folder) || file.split('/').any(bad) {
            return Err(SpecSinkError(format!(
                "{folder}/{file} is not a file inside a spec folder"
            )));
        }
        let path = WorkspacePath::parse(&format!("{SPECS_DIR}/{folder}/{file}"))
            .map_err(|e| SpecSinkError(e.to_string()))?;
        let change = self
            .fs
            .prepare_write(WriteFileRequest {
                path,
                content: content.to_owned(),
            })
            .await
            .map_err(|e| SpecSinkError(e.to_string()))?;
        self.fs
            .apply(change)
            .await
            .map_err(|e| SpecSinkError(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fs::InMemoryFs;
    use domain::workspace::WorkspaceRoot;

    #[tokio::test]
    async fn the_fs_sink_lists_folders_and_writes_files() {
        let dir = std::env::temp_dir();
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new(dir).unwrap())
                .with_file(".crest/specs/0002-a/spec.md", "x")
                .with_file(".crest/specs/0005-b/evidence/c.csv", "x")
                .with_file("src/lib.rs", ""),
        );
        let sink = FsSpecSink::new(fs.clone(), fs.clone());
        assert_eq!(sink.folders().await.unwrap(), ["0002-a", "0005-b"]);
        sink.write("0006-c", "evidence/d.csv", "a,b\n")
            .await
            .unwrap();
        assert_eq!(
            fs.file(".crest/specs/0006-c/evidence/d.csv").as_deref(),
            Some("a,b\n")
        );
        assert!(sink.write("..", "x", "").await.is_err());
        assert!(sink.write("a", "../x", "").await.is_err());
    }
}
