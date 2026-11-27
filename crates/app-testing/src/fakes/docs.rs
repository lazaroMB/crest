//! Fake of [`DocFilesPort`]: the files of documents in memory.
//!
//! It validates exactly like the disk adapter (the shared
//! [`check_dir`] and [`check_name`]) so a test of a use case, a tool or the
//! UI sees the same refusals.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;

use crate::ports::docs::{DocBases, DocFilesError, DocFilesPort, check_dir, check_name};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn norm(dir: &str) -> String {
    dir.trim_end_matches('/').to_owned()
}

/// [`DocFilesPort`] over two maps: texts keyed by `(dir, file)` and assets
/// keyed by `(dir, name)`.
#[derive(Debug, Default)]
pub struct InMemoryDocFiles {
    files: Mutex<HashMap<(String, String), String>>,
    assets: Mutex<HashMap<(String, String), Vec<u8>>>,
    bases: Mutex<DocBases>,
    fail_relocate: AtomicBool,
    fail_write: AtomicBool,
}

impl InMemoryDocFiles {
    /// A store that accepts folders under `bases`.
    #[must_use]
    pub fn new(bases: DocBases) -> Self {
        Self {
            bases: Mutex::new(bases),
            ..Self::default()
        }
    }

    /// Replaces the accepted bases, as when a project's roots change.
    pub fn set_bases(&self, bases: DocBases) {
        *lock(&self.bases) = bases;
    }

    /// Makes `relocate` fail (leaving everything in place) while `on`.
    pub fn fail_relocate(&self, on: bool) {
        self.fail_relocate.store(on, Ordering::SeqCst);
    }

    /// Makes `write`, `write_asset` and `remove` fail while `on`.
    pub fn fail_write(&self, on: bool) {
        self.fail_write.store(on, Ordering::SeqCst);
    }

    /// Every text, as `(dir, file, text)`, sorted, for assertions.
    #[must_use]
    pub fn files(&self) -> Vec<(String, String, String)> {
        let mut all: Vec<_> = lock(&self.files)
            .iter()
            .map(|((d, f), t)| (d.clone(), f.clone(), t.clone()))
            .collect();
        all.sort();
        all
    }

    /// Every asset, as `(dir, name, bytes)`, sorted, for assertions.
    #[must_use]
    pub fn assets(&self) -> Vec<(String, String, Vec<u8>)> {
        let mut all: Vec<_> = lock(&self.assets)
            .iter()
            .map(|((d, n), b)| (d.clone(), n.clone(), b.clone()))
            .collect();
        all.sort();
        all
    }

    fn check(&self, dir: &str, name: Option<&str>) -> Result<String, DocFilesError> {
        check_dir(&lock(&self.bases), dir)?;
        if let Some(name) = name {
            check_name(name)?;
        }
        Ok(norm(dir))
    }

    fn check_write(&self) -> Result<(), DocFilesError> {
        if self.fail_write.load(Ordering::SeqCst) {
            Err(DocFilesError::Io("disk full".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl DocFilesPort for InMemoryDocFiles {
    async fn read(&self, dir: &str, file: &str) -> Result<String, DocFilesError> {
        let dir = self.check(dir, Some(file))?;
        lock(&self.files)
            .get(&(dir.clone(), file.to_owned()))
            .cloned()
            .ok_or_else(|| DocFilesError::NotFound(format!("{dir}/{file}")))
    }

    async fn write(&self, dir: &str, file: &str, text: &str) -> Result<(), DocFilesError> {
        let dir = self.check(dir, Some(file))?;
        self.check_write()?;
        lock(&self.files).insert((dir, file.to_owned()), text.to_owned());
        Ok(())
    }

    async fn write_asset(&self, dir: &str, name: &str, bytes: &[u8]) -> Result<(), DocFilesError> {
        let dir = self.check(dir, Some(name))?;
        self.check_write()?;
        lock(&self.assets).insert((dir, name.to_owned()), bytes.to_vec());
        Ok(())
    }

    async fn read_asset(
        &self,
        dir: &str,
        name: &str,
        max: usize,
    ) -> Result<Vec<u8>, DocFilesError> {
        let dir = self.check(dir, Some(name))?;
        let bytes = lock(&self.assets)
            .get(&(dir.clone(), name.to_owned()))
            .cloned()
            .ok_or_else(|| DocFilesError::NotFound(format!("{dir}/assets/{name}")))?;
        if bytes.len() > max {
            return Err(DocFilesError::Io(format!(
                "{dir}/assets/{name} is {} bytes, over the {max} limit",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    async fn relocate(&self, from: &str, to: &str) -> Result<(), DocFilesError> {
        let (from, to) = (self.check(from, None)?, self.check(to, None)?);
        if self.fail_relocate.load(Ordering::SeqCst) {
            return Err(DocFilesError::Io("cannot move the folder".into()));
        }
        if from == to {
            return Ok(());
        }
        let mut files = lock(&self.files);
        let mut assets = lock(&self.assets);
        let moved: Vec<_> = files.keys().filter(|(d, _)| *d == from).cloned().collect();
        let moved_assets: Vec<_> = assets.keys().filter(|(d, _)| *d == from).cloned().collect();
        for key in moved {
            if let Some(text) = files.remove(&key) {
                files.insert((to.clone(), key.1), text);
            }
        }
        for key in moved_assets {
            if let Some(bytes) = assets.remove(&key) {
                assets.insert((to.clone(), key.1), bytes);
            }
        }
        Ok(())
    }

    async fn remove(&self, dir: &str, file: &str) -> Result<(), DocFilesError> {
        let dir = self.check(dir, Some(file))?;
        self.check_write()?;
        lock(&self.files).remove(&(dir, file.to_owned()));
        Ok(())
    }

    async fn exists(&self, dir: &str, file: &str) -> bool {
        let Ok(dir) = self.check(dir, Some(file)) else {
            return false;
        };
        lock(&self.files).contains_key(&(dir, file.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "/ws/.crest/docs/s1";

    fn files() -> InMemoryDocFiles {
        InMemoryDocFiles::new(DocBases {
            roots: vec!["/ws".into()],
            data_dir: Some("/data".into()),
        })
    }

    #[tokio::test]
    async fn round_trip_and_validation() {
        let f = files();
        f.write(DIR, "a.md", "hi").await.unwrap();
        assert_eq!(f.read(DIR, "a.md").await.unwrap(), "hi");
        assert!(f.exists(DIR, "a.md").await);
        assert!(!f.exists(DIR, "b.md").await);
        assert!(matches!(
            f.write("/etc/docs/x", "a.md", "x").await,
            Err(DocFilesError::OutsideBase(_))
        ));
        assert!(matches!(
            f.write(DIR, "../a.md", "x").await,
            Err(DocFilesError::BadName(_))
        ));
        f.remove(DIR, "a.md").await.unwrap();
        assert!(matches!(
            f.read(DIR, "a.md").await,
            Err(DocFilesError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn relocate_moves_and_failure_leaves_the_source() {
        let f = files();
        f.write(DIR, "a.md", "hi").await.unwrap();
        f.write_asset(DIR, "p.png", &[1, 2, 3]).await.unwrap();
        f.fail_relocate(true);
        assert!(f.relocate(DIR, "/data/docs/s1").await.is_err());
        assert!(f.exists(DIR, "a.md").await);
        f.fail_relocate(false);
        f.relocate(DIR, "/data/docs/s1").await.unwrap();
        assert!(!f.exists(DIR, "a.md").await);
        assert_eq!(f.read("/data/docs/s1", "a.md").await.unwrap(), "hi");
        assert_eq!(
            f.read_asset("/data/docs/s1", "p.png", 10).await.unwrap(),
            [1, 2, 3]
        );
        assert!(f.read_asset("/data/docs/s1", "p.png", 2).await.is_err());
        f.relocate("/ws/.crest/docs/nope", "/data/docs/n")
            .await
            .unwrap();
        f.fail_write(true);
        assert!(f.write("/data/docs/s1", "b.md", "x").await.is_err());
    }
}
