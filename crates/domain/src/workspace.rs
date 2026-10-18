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
    /// The path resolves outside of the workspace root. Reported to the model
    /// as `PathOutsideWorkspace`.
    #[error("path is outside the workspace: {path}")]
    OutsideWorkspace {
        /// The path exactly as it was received.
        path: String,
    },
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

    /// Resolves user or model input to a path inside this workspace.
    ///
    /// Relative input is taken relative to the root; absolute input must
    /// lie under the root. Normalization is purely lexical: `.` and empty
    /// components are dropped and `..` removes the previous component, so
    /// `a/b/../c` becomes `a/c`. A `..` that would climb above the root, or
    /// an absolute path that does not start with the root (compared by
    /// whole components, so `/ws-other` is not inside `/ws`), yields
    /// [`PathError::OutsideWorkspace`]. `"."` (and the root itself) resolve
    /// to [`WorkspacePath::root`]; the empty string and NUL bytes are
    /// rejected as [`PathError::InvalidPath`].
    ///
    /// Symlinks are not followed: the fs adapter must canonicalize the joined
    /// path and check again that it stays under the root.
    pub fn resolve(&self, input: &str) -> Result<WorkspacePath, PathError> {
        check_input(input)?;
        let outside = || PathError::OutsideWorkspace {
            path: input.to_owned(),
        };
        if !input.starts_with('/') {
            let segments = normalize(input, false).ok_or_else(outside)?;
            return Ok(WorkspacePath::from_segments(&segments));
        }
        let segments = normalize(input, true).ok_or_else(outside)?;
        let mut rest = segments.as_slice();
        for component in self.0.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            match rest.split_first() {
                Some((first, tail)) if Path::new(first).as_os_str() == name => rest = tail,
                _ => return Err(outside()),
            }
        }
        Ok(WorkspacePath::from_segments(rest))
    }
}

/// A lexically normalized path relative to a [`WorkspaceRoot`].
///
/// It never contains `.`, `..`, empty components, a leading or trailing `/`
/// or NUL bytes. The root itself is represented as `"."`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    const ROOT: &'static str = ".";

    /// The workspace root itself.
    pub fn root() -> Self {
        Self(Self::ROOT.to_owned())
    }

    /// The path as `/`-separated components, or `"."` for the root.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_segments(segments: &[&str]) -> Self {
        if segments.is_empty() {
            Self::root()
        } else {
            Self(segments.join("/"))
        }
    }
}

fn check_input(input: &str) -> Result<(), PathError> {
    if input.is_empty() {
        return Err(InvalidPathReason::Empty.into());
    }
    if input.contains('\0') {
        return Err(InvalidPathReason::ContainsNul.into());
    }
    Ok(())
}

/// Lexically normalizes `input` into its components.
///
/// Returns `None` when a `..` climbs above the start. For absolute paths a
/// `..` at the file system root stays there, as the kernel does.
fn normalize(input: &str, absolute: bool) -> Option<Vec<&str>> {
    let mut stack = Vec::new();
    for segment in input.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() && !absolute {
                    return None;
                }
            }
            name => stack.push(name),
        }
    }
    Some(stack)
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

    fn resolve(input: &str) -> Result<String, PathError> {
        root().resolve(input).map(|p| p.as_str().to_owned())
    }

    fn outside(input: &str) -> Result<String, PathError> {
        Err(PathError::OutsideWorkspace {
            path: input.to_owned(),
        })
    }

    #[test]
    fn resolve_keeps_simple_relative_path() {
        assert_eq!(resolve("src/main.rs"), Ok("src/main.rs".into()));
    }

    #[test]
    fn resolve_drops_dots_empty_components_and_trailing_slash() {
        assert_eq!(resolve("./a//b/"), Ok("a/b".into()));
        assert_eq!(resolve("a/./b/."), Ok("a/b".into()));
    }

    #[test]
    fn resolve_allows_parent_that_stays_inside() {
        assert_eq!(resolve("a/b/../c"), Ok("a/c".into()));
        assert_eq!(resolve("a/.."), Ok(".".into()));
    }

    #[test]
    fn resolve_rejects_parent_that_escapes() {
        assert_eq!(resolve("../x"), outside("../x"));
        assert_eq!(resolve("a/../../x"), outside("a/../../x"));
        assert_eq!(resolve(".."), outside(".."));
    }

    #[test]
    fn resolve_rejects_escape_even_if_it_comes_back() {
        // `../ws/a` would lexically land inside again, but climbing above
        // the root is never allowed for relative input.
        assert_eq!(resolve("../ws/a"), outside("../ws/a"));
    }

    #[test]
    fn resolve_treats_dot_as_root() {
        assert_eq!(resolve("."), Ok(".".into()));
        assert_eq!(resolve("./"), Ok(".".into()));
    }

    #[test]
    fn resolve_accepts_root_itself_as_absolute() {
        assert_eq!(resolve("/ws"), Ok(".".into()));
        assert_eq!(resolve("/ws/"), Ok(".".into()));
        assert!(root().resolve("/ws").unwrap() == WorkspacePath::root());
    }

    #[test]
    fn resolve_accepts_absolute_path_inside() {
        assert_eq!(resolve("/ws/src/lib.rs"), Ok("src/lib.rs".into()));
        assert_eq!(resolve("//ws/./src//lib.rs"), Ok("src/lib.rs".into()));
        assert_eq!(resolve("/other/../ws/a"), Ok("a".into()));
    }

    #[test]
    fn resolve_rejects_absolute_path_outside() {
        assert_eq!(resolve("/etc/passwd"), outside("/etc/passwd"));
        assert_eq!(resolve("/"), outside("/"));
        assert_eq!(resolve("/ws/../etc"), outside("/ws/../etc"));
    }

    #[test]
    fn resolve_rejects_misleading_prefix() {
        assert_eq!(resolve("/ws-other/a"), outside("/ws-other/a"));
        assert_eq!(resolve("/wsx"), outside("/wsx"));
    }

    #[test]
    fn resolve_with_nested_root() {
        let r = WorkspaceRoot::new("/home/me/proj").unwrap();
        assert_eq!(r.resolve("/home/me/proj/a").unwrap().as_str(), "a");
        assert!(r.resolve("/home/me").is_err());
        assert!(r.resolve("/home/me/project/a").is_err());
    }

    #[test]
    fn resolve_with_filesystem_root_accepts_any_absolute_path() {
        let r = WorkspaceRoot::new("/").unwrap();
        assert_eq!(r.resolve("/etc/passwd").unwrap().as_str(), "etc/passwd");
        assert_eq!(r.resolve("/..").unwrap(), WorkspacePath::root());
        assert!(r.resolve("..").is_err());
    }

    #[test]
    fn resolve_keeps_unicode_untouched() {
        assert_eq!(
            resolve("docs/ñandú/日本.md"),
            Ok("docs/ñandú/日本.md".into())
        );
        let r = WorkspaceRoot::new("/home/josé").unwrap();
        assert_eq!(r.resolve("/home/josé/ü").unwrap().as_str(), "ü");
        assert!(r.resolve("/home/jose/ü").is_err());
    }

    #[test]
    fn resolve_treats_dotted_names_as_normal() {
        assert_eq!(resolve("...").unwrap(), "...");
        assert_eq!(resolve("a/..b/.c"), Ok("a/..b/.c".into()));
    }

    #[test]
    fn resolve_rejects_empty_input() {
        assert_eq!(resolve(""), Err(InvalidPathReason::Empty.into()));
    }

    #[test]
    fn resolve_rejects_nul() {
        assert_eq!(resolve("a\0b"), Err(InvalidPathReason::ContainsNul.into()));
        assert_eq!(
            resolve("/ws/a\0"),
            Err(InvalidPathReason::ContainsNul.into())
        );
    }

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            outside("/etc/passwd").unwrap_err().to_string(),
            "path is outside the workspace: /etc/passwd"
        );
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
