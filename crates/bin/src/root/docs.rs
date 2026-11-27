//! The documents of chats: the files port with the folders of every
//! project, and the service the tools and the window share.
//!
//! A document's folder is `<first root>/.crest/docs/<chat>` or
//! `<data dir>/docs/<chat>`. The roots change when a project gains or loses
//! a directory or a chat moves, so [`ProjectDocFiles`] reads them from the
//! store before each operation instead of keeping a list that goes stale.

use std::path::Path;
use std::sync::Arc;

use adapter_fs::docs::LocalDocFiles;
use application::ports::docs::{DocBases, DocFilesError, DocFilesPort};
use application::ports::store::SessionStorePort;
use application::usecases::docs::DocService;
use async_trait::async_trait;

use super::now_ms;

/// [`LocalDocFiles`] whose accepted project folders follow the store.
struct ProjectDocFiles {
    inner: LocalDocFiles,
    store: Arc<dyn SessionStorePort>,
    /// Directories that count besides the projects' (the one opened).
    extra_roots: Vec<String>,
    data_dir: Option<String>,
}

impl ProjectDocFiles {
    /// Accepts the folders of every project's directories now.
    async fn refresh(&self) {
        let mut roots = self.extra_roots.clone();
        if let Ok(projects) = self.store.projects().await {
            roots.extend(projects.into_iter().flat_map(|p| p.roots));
        }
        self.inner.set_bases(DocBases {
            roots,
            data_dir: self.data_dir.clone(),
        });
    }
}

#[async_trait]
impl DocFilesPort for ProjectDocFiles {
    async fn read(&self, dir: &str, file: &str) -> Result<String, DocFilesError> {
        self.refresh().await;
        self.inner.read(dir, file).await
    }

    async fn write(&self, dir: &str, file: &str, text: &str) -> Result<(), DocFilesError> {
        self.refresh().await;
        self.inner.write(dir, file, text).await
    }

    async fn write_asset(&self, dir: &str, name: &str, bytes: &[u8]) -> Result<(), DocFilesError> {
        self.refresh().await;
        self.inner.write_asset(dir, name, bytes).await
    }

    async fn read_asset(
        &self,
        dir: &str,
        name: &str,
        max: usize,
    ) -> Result<Vec<u8>, DocFilesError> {
        self.refresh().await;
        self.inner.read_asset(dir, name, max).await
    }

    async fn relocate(&self, from: &str, to: &str) -> Result<(), DocFilesError> {
        self.refresh().await;
        self.inner.relocate(from, to).await
    }

    async fn remove(&self, dir: &str, file: &str) -> Result<(), DocFilesError> {
        self.refresh().await;
        self.inner.remove(dir, file).await
    }

    async fn exists(&self, dir: &str, file: &str) -> bool {
        self.refresh().await;
        self.inner.exists(dir, file).await
    }
}

/// The folder crest keeps its data in: the one that holds the session
/// database `db_path`, else the platform's data directory.
pub(crate) fn data_dir(db_path: Option<&Path>) -> Option<String> {
    let dir = match db_path {
        Some(path) => path.parent().map(Path::to_path_buf),
        None => platform::dirs::data_dir(),
    };
    dir.map(|dir| dir.display().to_string())
}

/// The documents service over `store`. `opened` is the directory the
/// window opened, which also holds documents when no project lists it.
pub(crate) fn doc_service(
    store: Arc<dyn SessionStorePort>,
    db_path: Option<&Path>,
    opened: Option<&Path>,
) -> Arc<DocService> {
    let data_dir = data_dir(db_path);
    let files = ProjectDocFiles {
        inner: LocalDocFiles::new(DocBases::default()),
        store: Arc::clone(&store),
        extra_roots: opened
            .map(|dir| dir.display().to_string())
            .into_iter()
            .collect(),
        data_dir: data_dir.clone(),
    };
    Arc::new(DocService::new(store, Arc::new(files), data_dir, now_ms))
}
