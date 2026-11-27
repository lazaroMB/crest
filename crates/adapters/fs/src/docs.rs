//! [`DocFilesPort`] over the local disk: the files of chat documents.
//!
//! The person is the author of these files, so nothing here asks for an
//! approval, and the agent never gets this adapter (it has its own
//! [`FileSystemPort`](application_core::ports::fs::FileSystemPort)). What it
//! accepts is narrow: [`check_dir`] lets a folder through only as
//! `<root>/.crest/docs/<id>` of a root of the project or
//! `<data dir>/docs/<id>`, and [`check_name`] refuses separators and `..`.
//! Every write goes to a temporary file in the same folder and is renamed
//! over the target, so a crash leaves the old text.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use application_core::ports::docs::{DocBases, DocFilesError, DocFilesPort, check_dir, check_name};
use async_trait::async_trait;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// [`DocFilesPort`] that reads and writes under the folders of `bases`.
///
/// Cloning shares the bases, so [`set_bases`](Self::set_bases) on one handle
/// is seen by all.
#[derive(Debug, Clone, Default)]
pub struct LocalDocFiles {
    bases: Arc<RwLock<DocBases>>,
}

impl LocalDocFiles {
    /// An adapter that accepts the folders under `bases`.
    #[must_use]
    pub fn new(bases: DocBases) -> Self {
        Self {
            bases: Arc::new(RwLock::new(bases)),
        }
    }

    /// Replaces the accepted bases, as when the roots of a project change.
    pub fn set_bases(&self, bases: DocBases) {
        *self.bases.write().unwrap_or_else(PoisonError::into_inner) = bases;
    }

    /// The folder `dir` as a path, after the checks; `name` too when given.
    fn resolve(&self, dir: &str, name: Option<&str>) -> Result<PathBuf, DocFilesError> {
        check_dir(
            &self.bases.read().unwrap_or_else(PoisonError::into_inner),
            dir,
        )?;
        if let Some(name) = name {
            check_name(name)?;
        }
        Ok(PathBuf::from(dir.trim_end_matches('/')))
    }
}

fn io_error(path: &Path, err: &io::Error) -> DocFilesError {
    if err.kind() == io::ErrorKind::NotFound {
        DocFilesError::NotFound(path.display().to_string())
    } else {
        DocFilesError::Io(format!("{}: {err}", path.display()))
    }
}

/// Runs blocking disk work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DocFilesError> + Send + 'static,
) -> Result<T, DocFilesError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| DocFilesError::Io(format!("document task failed: {err}")))?
}

/// Writes `bytes` to `folder/name` through a temporary file in `folder`.
fn write_atomic(folder: &Path, name: &str, bytes: &[u8]) -> Result<(), DocFilesError> {
    std::fs::create_dir_all(folder).map_err(|e| io_error(folder, &e))?;
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = folder.join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    let target = folder.join(name);
    let result = std::fs::write(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, &target))
        .map_err(|e| io_error(&target, &e));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Copies the folder `from` into `to` (created), files and sub-folders.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn relocate_sync(from: &Path, to: &Path) -> Result<(), DocFilesError> {
    if from == to || !from.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Another filesystem (or a target that is in the way): copy, then remove.
    let existed = to.exists();
    if let Err(err) = copy_tree(from, to) {
        if !existed {
            let _ = std::fs::remove_dir_all(to);
        }
        return Err(io_error(to, &err));
    }
    std::fs::remove_dir_all(from).map_err(|e| io_error(from, &e))
}

#[async_trait]
impl DocFilesPort for LocalDocFiles {
    async fn read(&self, dir: &str, file: &str) -> Result<String, DocFilesError> {
        let path = self.resolve(dir, Some(file))?.join(file);
        blocking(move || std::fs::read_to_string(&path).map_err(|e| io_error(&path, &e))).await
    }

    async fn write(&self, dir: &str, file: &str, text: &str) -> Result<(), DocFilesError> {
        let folder = self.resolve(dir, Some(file))?;
        let (file, text) = (file.to_owned(), text.to_owned());
        blocking(move || write_atomic(&folder, &file, text.as_bytes())).await
    }

    async fn write_asset(&self, dir: &str, name: &str, bytes: &[u8]) -> Result<(), DocFilesError> {
        let folder = self.resolve(dir, Some(name))?.join("assets");
        let (name, bytes) = (name.to_owned(), bytes.to_vec());
        blocking(move || write_atomic(&folder, &name, &bytes)).await
    }

    async fn read_asset(
        &self,
        dir: &str,
        name: &str,
        max: usize,
    ) -> Result<Vec<u8>, DocFilesError> {
        let path = self.resolve(dir, Some(name))?.join("assets").join(name);
        blocking(move || {
            let file = std::fs::File::open(&path).map_err(|e| io_error(&path, &e))?;
            let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
            let mut bytes = Vec::new();
            file.take(limit)
                .read_to_end(&mut bytes)
                .map_err(|e| io_error(&path, &e))?;
            if bytes.len() > max {
                return Err(DocFilesError::Io(format!(
                    "{} is over the {max} byte limit",
                    path.display()
                )));
            }
            Ok(bytes)
        })
        .await
    }

    async fn relocate(&self, from: &str, to: &str) -> Result<(), DocFilesError> {
        let (from, to) = (self.resolve(from, None)?, self.resolve(to, None)?);
        blocking(move || relocate_sync(&from, &to)).await
    }

    async fn remove(&self, dir: &str, file: &str) -> Result<(), DocFilesError> {
        let path = self.resolve(dir, Some(file))?.join(file);
        blocking(move || match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_error(&path, &e)),
            _ => Ok(()),
        })
        .await
    }

    async fn exists(&self, dir: &str, file: &str) -> bool {
        let Ok(folder) = self.resolve(dir, Some(file)) else {
            return false;
        };
        let path = folder.join(file);
        blocking(move || Ok(path.is_file())).await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: String,
        data: String,
        files: LocalDocFiles,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&root).unwrap();
        let (root, data) = (
            root.to_string_lossy().into_owned(),
            data.to_string_lossy().into_owned(),
        );
        let files = LocalDocFiles::new(DocBases {
            roots: vec![root.clone()],
            data_dir: Some(data.clone()),
        });
        Fixture {
            _tmp: tmp,
            root,
            data,
            files,
        }
    }

    impl Fixture {
        fn dir(&self) -> String {
            format!("{}/.crest/docs/s1", self.root)
        }
    }

    #[tokio::test]
    async fn text_and_assets_round_trip() {
        let f = fixture();
        let dir = f.dir();
        assert!(!f.files.exists(&dir, "a.md").await);
        f.files.write(&dir, "a.md", "# hi\n").await.unwrap();
        assert_eq!(f.files.read(&dir, "a.md").await.unwrap(), "# hi\n");
        assert!(f.files.exists(&dir, "a.md").await);
        f.files
            .write_asset(&dir, "p.png", &[1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            f.files.read_asset(&dir, "p.png", 3).await.unwrap(),
            [1, 2, 3]
        );
        assert!(Path::new(&dir).join("assets/p.png").is_file());
        assert!(matches!(
            f.files.read_asset(&dir, "p.png", 2).await,
            Err(DocFilesError::Io(_))
        ));
        assert!(matches!(
            f.files.read(&dir, "missing.md").await,
            Err(DocFilesError::NotFound(_))
        ));
        f.files.remove(&dir, "a.md").await.unwrap();
        f.files.remove(&dir, "a.md").await.unwrap();
        assert!(!f.files.exists(&dir, "a.md").await);
    }

    #[tokio::test]
    async fn the_data_dir_is_a_base_too() {
        let f = fixture();
        let dir = format!("{}/docs/s9", f.data);
        f.files.write(&dir, "n.md", "x").await.unwrap();
        assert_eq!(f.files.read(&dir, "n.md").await.unwrap(), "x");
    }

    #[tokio::test]
    async fn a_write_replaces_the_text_and_leaves_no_temp_file() {
        let f = fixture();
        let dir = f.dir();
        f.files.write(&dir, "a.md", "old").await.unwrap();
        f.files.write(&dir, "a.md", "new").await.unwrap();
        assert_eq!(f.files.read(&dir, "a.md").await.unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.md"]);
    }

    #[tokio::test]
    async fn folders_outside_the_bases_and_bad_names_are_refused() {
        let f = fixture();
        let dir = f.dir();
        for outside in [
            format!("{}/docs/s1", f.root),
            format!("{}/.crest/docs/s1/assets", f.root),
            format!("{}/.crest/docs/../x", f.root),
            "/tmp/.crest/docs/s1".to_owned(),
        ] {
            assert!(
                matches!(
                    f.files.write(&outside, "a.md", "x").await,
                    Err(DocFilesError::OutsideBase(_))
                ),
                "{outside}"
            );
            assert!(!Path::new(&outside).exists() || outside.starts_with(&f.root));
        }
        for name in ["", "../a.md", "a/b.md", "a\\b.md", ".."] {
            assert!(
                matches!(
                    f.files.write(&dir, name, "x").await,
                    Err(DocFilesError::BadName(_))
                ),
                "{name:?}"
            );
            assert!(matches!(
                f.files.write_asset(&dir, name, &[1]).await,
                Err(DocFilesError::BadName(_))
            ));
        }
        assert!(!Path::new(&dir).exists());
        f.files.set_bases(DocBases::default());
        assert!(matches!(
            f.files.write(&dir, "a.md", "x").await,
            Err(DocFilesError::OutsideBase(_))
        ));
    }

    #[tokio::test]
    async fn relocate_moves_the_text_and_the_assets() {
        let f = fixture();
        let from = f.dir();
        let to = format!("{}/docs/s1", f.data);
        f.files.write(&from, "a.md", "text").await.unwrap();
        f.files.write_asset(&from, "p.png", &[7]).await.unwrap();
        f.files.relocate(&from, &to).await.unwrap();
        assert!(!Path::new(&from).exists());
        assert_eq!(f.files.read(&to, "a.md").await.unwrap(), "text");
        assert_eq!(f.files.read_asset(&to, "p.png", 10).await.unwrap(), [7]);
        // Nothing to move is not an error.
        f.files.relocate(&from, &to).await.unwrap();
        assert_eq!(f.files.read(&to, "a.md").await.unwrap(), "text");
    }

    #[tokio::test]
    async fn relocate_into_an_impossible_target_keeps_the_source() {
        let f = fixture();
        let from = f.dir();
        f.files.write(&from, "a.md", "text").await.unwrap();
        f.files.write_asset(&from, "p.png", &[7]).await.unwrap();
        // `<data>` is a regular file, so nothing can be created below it.
        std::fs::write(&f.data, "not a folder").unwrap();
        let to = format!("{}/docs/s1", f.data);
        assert!(f.files.relocate(&from, &to).await.is_err());
        assert_eq!(f.files.read(&from, "a.md").await.unwrap(), "text");
        assert_eq!(f.files.read_asset(&from, "p.png", 10).await.unwrap(), [7]);
    }
}
