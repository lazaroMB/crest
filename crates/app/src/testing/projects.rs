//! [`ProjectBrowserPort`] over a fixed set of directories.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::ports::projects::{BrowseError, DirEntry, DirListing, ProjectBrowserPort};

/// Directories in memory: each one with its subdirectories, which may be
/// marked as projects or as unreadable. `~` is `home`.
#[derive(Debug, Clone)]
pub struct InMemoryDirs {
    home: PathBuf,
    start: PathBuf,
    dirs: BTreeSet<PathBuf>,
    projects: BTreeSet<PathBuf>,
    unreadable: BTreeSet<PathBuf>,
}

impl InMemoryDirs {
    /// Only `/` and `home`; browsing starts at `home`.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let mut fake = Self {
            start: home.clone(),
            home: home.clone(),
            dirs: BTreeSet::new(),
            projects: BTreeSet::new(),
            unreadable: BTreeSet::new(),
        };
        fake.add(&home);
        fake
    }

    fn add(&mut self, path: &Path) {
        for ancestor in path.ancestors() {
            self.dirs.insert(ancestor.to_path_buf());
        }
    }

    /// Adds the directory `path` (and its parents).
    #[must_use]
    pub fn with_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.add(path.as_ref());
        self
    }

    /// Adds the directory `path` holding a `.git`.
    #[must_use]
    pub fn with_project(mut self, path: impl AsRef<Path>) -> Self {
        self.add(path.as_ref());
        self.projects.insert(path.as_ref().to_path_buf());
        self
    }

    /// Adds the directory `path`, which cannot be listed.
    #[must_use]
    pub fn with_unreadable(mut self, path: impl AsRef<Path>) -> Self {
        self.add(path.as_ref());
        self.unreadable.insert(path.as_ref().to_path_buf());
        self
    }

    /// Browsing starts at `path` instead of `home`.
    #[must_use]
    pub fn starting_at(mut self, path: impl Into<PathBuf>) -> Self {
        self.start = path.into();
        self
    }

    fn expand(&self, input: &str) -> PathBuf {
        match input.strip_prefix('~') {
            Some(rest) => self.home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(input),
        }
    }

    fn check(&self, path: &Path) -> Result<(), BrowseError> {
        let shown = path.display().to_string();
        if !self.dirs.contains(path) {
            return Err(BrowseError::NotFound(shown));
        }
        if self.unreadable.contains(path) {
            return Err(BrowseError::Unreadable {
                path: shown,
                reason: "permission denied".into(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProjectBrowserPort for InMemoryDirs {
    fn start_dir(&self) -> Option<PathBuf> {
        Some(self.start.clone())
    }

    async fn list_dirs(&self, path: &Path) -> Result<DirListing, BrowseError> {
        self.check(path)?;
        let mut dirs: BTreeMap<String, DirEntry> = BTreeMap::new();
        for dir in &self.dirs {
            if dir.parent() == Some(path) {
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                dirs.insert(
                    name.to_lowercase(),
                    DirEntry {
                        hidden: name.starts_with('.'),
                        is_project: self.projects.contains(dir),
                        path: dir.clone(),
                        name,
                    },
                );
            }
        }
        let mut dirs: Vec<DirEntry> = dirs.into_values().collect();
        dirs.sort_by(|a, b| b.is_project.cmp(&a.is_project));
        Ok(DirListing {
            path: path.to_path_buf(),
            parent: path.parent().map(Path::to_path_buf),
            dirs,
            truncated: false,
        })
    }

    async fn resolve(&self, input: &str) -> Result<PathBuf, BrowseError> {
        let path = self.expand(input.trim());
        let path = if path.as_os_str().len() > 1 {
            PathBuf::from(path.to_string_lossy().trim_end_matches('/'))
        } else {
            path
        };
        self.check(&path)?;
        Ok(path)
    }
}
