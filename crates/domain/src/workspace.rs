//! Workspace paths: a canonical root and lexically normalized paths inside it.
//!
//! This module is pure: it never touches the file system. The caller (the
//! fs adapter) canonicalizes the root before building a [`WorkspaceRoot`] and
//! is responsible for resolving symlinks after joining a [`WorkspacePath`].
//! Only Unix path semantics are supported.

use std::path::{Component, Path, PathBuf};

/// Errors produced while building workspace roots and paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The workspace root is not an absolute path.
    #[error("workspace root must be an absolute path: {}", path.display())]
    NotAbsoluteRoot {
        /// The rejected root.
        path: PathBuf,
    },
    /// The path is malformed.
    #[error("invalid path: {reason}")]
    InvalidPath {
        /// Why the path was rejected.
        reason: InvalidPathReason,
    },
}

/// Why a path was rejected as [`PathError::InvalidPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPathReason {
    /// The path is the empty string.
    #[error("path is empty")]
    Empty,
    /// The path contains a NUL byte.
    #[error("path contains a NUL byte")]
    ContainsNul,
    /// The root contains `..` components, so it is not canonical.
    #[error("workspace root is not normalized")]
    RootNotNormalized,
}

impl From<InvalidPathReason> for PathError {
    fn from(reason: InvalidPathReason) -> Self {
        Self::InvalidPath { reason }
    }
}

/// The absolute root directory of a workspace.
///
/// It is expected to be already canonicalized by the caller; this type only
/// checks that it is absolute, has no `..` components and no NUL bytes, and
/// drops redundant `.` components and trailing slashes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceRoot(PathBuf);

impl WorkspaceRoot {
    /// Builds a root from an absolute, canonical path.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(InvalidPathReason::Empty.into());
        }
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(InvalidPathReason::ContainsNul.into());
        }
        if !path.is_absolute() {
            return Err(PathError::NotAbsoluteRoot { path });
        }
        // `components()` already drops `.` and trailing slashes; only `..`
        // remains to be rejected.
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(InvalidPathReason::RootNotNormalized.into());
        }
        Ok(Self(path.components().collect()))
    }

    /// The root as a path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> WorkspaceRoot {
        WorkspaceRoot::new("/ws").unwrap()
    }

    #[test]
    fn root_accepts_absolute_path() {
        assert_eq!(root().as_path(), Path::new("/ws"));
    }

    #[test]
    fn root_strips_trailing_slash_and_dot_components() {
        let r = WorkspaceRoot::new("/ws/./proj/").unwrap();
        assert_eq!(r.as_path(), Path::new("/ws/proj"));
    }

    #[test]
    fn root_accepts_filesystem_root() {
        assert_eq!(WorkspaceRoot::new("/").unwrap().as_path(), Path::new("/"));
    }

    #[test]
    fn root_accepts_unicode() {
        let r = WorkspaceRoot::new("/home/josé/проект").unwrap();
        assert_eq!(r.as_path(), Path::new("/home/josé/проект"));
    }

    #[test]
    fn root_rejects_relative_path() {
        assert_eq!(
            WorkspaceRoot::new("ws"),
            Err(PathError::NotAbsoluteRoot { path: "ws".into() })
        );
        assert_eq!(
            WorkspaceRoot::new("./ws"),
            Err(PathError::NotAbsoluteRoot {
                path: "./ws".into()
            })
        );
    }

    #[test]
    fn root_rejects_empty_path() {
        assert_eq!(WorkspaceRoot::new(""), Err(InvalidPathReason::Empty.into()));
    }

    #[test]
    fn root_rejects_parent_components() {
        assert_eq!(
            WorkspaceRoot::new("/ws/../etc"),
            Err(InvalidPathReason::RootNotNormalized.into())
        );
    }

    #[test]
    fn root_rejects_nul() {
        assert_eq!(
            WorkspaceRoot::new("/ws\0x"),
            Err(InvalidPathReason::ContainsNul.into())
        );
    }

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            PathError::from(InvalidPathReason::ContainsNul).to_string(),
            "invalid path: path contains a NUL byte"
        );
        assert_eq!(
            PathError::NotAbsoluteRoot { path: "ws".into() }.to_string(),
            "workspace root must be an absolute path: ws"
        );
    }
}
