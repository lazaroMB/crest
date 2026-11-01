//! Browsing the disk to choose the project crest opens.
//!
//! Before a project is open there is no workspace to confine paths to, so
//! this port is separate from [`FileSystemPort`](super::fs::FileSystemPort):
//! it only lists directories and tells which ones look like projects. It
//! never reads the content of files nor runs anything.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

/// Most entries a listing returns; the rest are left out.
pub const MAX_DIR_ENTRIES: usize = 2000;

/// A directory inside a listed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Its name.
    pub name: String,
    /// Its absolute path.
    pub path: PathBuf,
    /// Whether the name starts with a `.`.
    pub hidden: bool,
    /// Whether it holds a `.git` or a `.crest`, so it looks like a project.
    pub is_project: bool,
}

/// The directories inside one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    /// The listed directory, canonical.
    pub path: PathBuf,
    /// Its parent, unless it is the root.
    pub parent: Option<PathBuf>,
    /// The directories inside it: projects first, then by name ignoring
    /// case; at most [`MAX_DIR_ENTRIES`].
    pub dirs: Vec<DirEntry>,
    /// Whether some entries were left out.
    pub truncated: bool,
}

/// Why a directory cannot be listed or opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowseError {
    /// Nothing at that path.
    #[error("{0} does not exist")]
    NotFound(String),
    /// The path is a file.
    #[error("{0} is not a directory")]
    NotADirectory(String),
    /// The directory exists but cannot be read (permissions, I/O).
    #[error("cannot read {path}: {reason}")]
    Unreadable {
        /// The path.
        path: String,
        /// The error of the system.
        reason: String,
    },
}

/// Lists directories of the local disk.
#[async_trait]
pub trait ProjectBrowserPort: Send + Sync {
    /// Where browsing starts: the current directory, else the home one.
    fn start_dir(&self) -> Option<PathBuf>;

    /// The directories inside `path`.
    async fn list_dirs(&self, path: &Path) -> Result<DirListing, BrowseError>;

    /// The directory `input` names (`~` expanded), canonical, after
    /// checking that it exists, is a directory and can be read.
    async fn resolve(&self, input: &str) -> Result<PathBuf, BrowseError>;
}

/// A workspace with saved sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentWorkspace {
    /// Its root, as stored.
    pub root: String,
    /// When one of its sessions was last saved, in milliseconds since the
    /// Unix epoch.
    pub updated_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn ProjectBrowserPort>>) {}
        assert_dyn(None);
    }
}
