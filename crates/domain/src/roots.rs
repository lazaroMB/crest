//! The directories a session works in: an ordered list of workspace roots.
//!
//! A project session has the project's directories, a standalone chat has
//! none. This module is pure like [`crate::workspace`]; each root is checked
//! by [`WorkspaceRoot::resolve`], so every rule of one root (components
//! compared whole, so `/a/b` does not contain `/a/bc`; `..` that climbs out
//! is refused) holds for each of them. The fs adapter still canonicalizes
//! the joined path and checks it against the root it landed in, so a symlink
//! cannot lead out of any root.
//!
//! Resolution:
//!
//! - A relative path resolves against the FIRST root.
//! - An absolute path resolves against the first root that contains it
//!   (when roots nest, the earlier one wins).
//! - A path no root contains is [`PathError::OutsideWorkspace`]; with no
//!   roots at all, every path is [`PathError::NoRoots`].

use std::path::PathBuf;

use crate::workspace::{
    PathError, WorkspacePath, WorkspaceRoot, is_absolute_input, parse_any_path,
};

/// An ordered list of roots without repeats; empty (the default) for a
/// standalone chat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRoots(Vec<WorkspaceRoot>);

/// A path placed in one of the roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootedPath {
    index: usize,
    root: WorkspaceRoot,
    path: WorkspacePath,
}

impl WorkspacePath {
    /// Like [`WorkspacePath::parse`], but an absolute path is kept (lexically
    /// normalized, `/`-separated) for the roots to place. It is still
    /// unchecked: only a file port may act on it, after resolving it against
    /// its roots ([`WorkspaceRoots::resolve`]).
    pub fn parse_any(input: &str) -> Result<Self, PathError> {
        parse_any_path(input)
    }

    /// Whether this is an absolute path from [`Self::parse_any`], not yet
    /// placed in a root.
    pub fn is_absolute(&self) -> bool {
        is_absolute_input(self.as_str())
    }
}

impl WorkspaceRoots {
    /// The roots in order; a root listed twice keeps its first place.
    pub fn new(roots: impl IntoIterator<Item = WorkspaceRoot>) -> Self {
        let mut list: Vec<WorkspaceRoot> = Vec::new();
        for root in roots {
            if !list.contains(&root) {
                list.push(root);
            }
        }
        Self(list)
    }

    /// The first root: the working directory of commands and the base of
    /// relative paths. `None` for a standalone chat.
    pub fn first(&self) -> Option<&WorkspaceRoot> {
        self.0.first()
    }

    /// The roots, in order.
    pub fn iter(&self) -> impl Iterator<Item = &WorkspaceRoot> {
        self.0.iter()
    }

    /// How many roots there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there is no root.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Places `input` in a root (see the module docs for the order).
    ///
    /// # Errors
    ///
    /// [`PathError::NoRoots`] without roots, [`PathError::OutsideWorkspace`]
    /// when no root contains it, [`PathError::InvalidPath`] for an empty
    /// input or a NUL byte.
    pub fn resolve(&self, input: &str) -> Result<RootedPath, PathError> {
        let first = self.first().ok_or(PathError::NoRoots)?;
        if !WorkspacePath::parse_any(input)?.is_absolute() {
            return Ok(RootedPath::new(0, first, first.resolve(input)?));
        }
        for (index, root) in self.0.iter().enumerate() {
            match root.resolve(input) {
                Ok(path) => return Ok(RootedPath::new(index, root, path)),
                Err(PathError::OutsideWorkspace { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        Err(PathError::OutsideWorkspace {
            path: input.to_owned(),
        })
    }

    /// `path` relative to the root it is in: a relative path already is, an
    /// absolute one (the address of a file in a later root) is placed first.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`], for an absolute path.
    pub fn relative_of(&self, path: &WorkspacePath) -> Result<WorkspacePath, PathError> {
        if path.is_absolute() {
            return Ok(self.resolve(path.as_str())?.path().clone());
        }
        Ok(path.clone())
    }

    /// Places a canonical absolute path in the first root that contains it
    /// (used on the result of canonicalizing, to check where a symlink led).
    pub fn locate(&self, canonical: &std::path::Path) -> Option<RootedPath> {
        let text = canonical.to_string_lossy();
        self.resolve(&text).ok()
    }
}

impl RootedPath {
    fn new(index: usize, root: &WorkspaceRoot, path: WorkspacePath) -> Self {
        Self {
            index,
            root: root.clone(),
            path,
        }
    }

    /// Position of its root in the list.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The root it is in.
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    /// The path relative to [`Self::root`]; what the sensitive-path rules
    /// are checked against, so they apply per root.
    pub fn path(&self) -> &WorkspacePath {
        &self.path
    }

    /// The absolute path, lexical only: the caller canonicalizes it.
    pub fn join(&self) -> PathBuf {
        self.root.join(&self.path)
    }

    /// The path as the tools name it: relative for the first root, absolute
    /// for the others (a relative one would mean the first root again).
    pub fn address(&self) -> WorkspacePath {
        if self.index == 0 {
            return self.path.clone();
        }
        WorkspacePath::parse_any(&self.join().to_string_lossy())
            .unwrap_or_else(|_| self.path.clone())
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn roots(paths: &[&str]) -> WorkspaceRoots {
        WorkspaceRoots::new(
            paths
                .iter()
                .map(|p| WorkspaceRoot::new(*p).unwrap_or_else(|e| panic!("{e}"))),
        )
    }

    /// (roots, input, expected `index:path`, or the error)
    #[test]
    fn resolving_tries_each_root_in_order() {
        let three = ["/a/b", "/a/bc", "/c/d"];
        let outside = |input: &str| {
            Err(PathError::OutsideWorkspace {
                path: input.to_owned(),
            })
        };
        let cases: Vec<(&[&str], &str, Result<String, PathError>)> = vec![
            (&three, "src/x.rs", Ok("0:src/x.rs".into())),
            (&three, ".", Ok("0:.".into())),
            (&three, "/a/b/src/x.rs", Ok("0:src/x.rs".into())),
            (&three, "/a/bc/y", Ok("1:y".into())),
            (&three, "/c/d", Ok("2:.".into())),
            (&three, "/c/d/e/../f", Ok("2:f".into())),
            // A root is not a prefix of its neighbour: `/a/b` vs `/a/bc`.
            (&["/a/b"], "/a/bc/y", outside("/a/bc/y")),
            (&["/a/bc"], "/a/b/y", outside("/a/b/y")),
            (&three, "/a/bcd/y", outside("/a/bcd/y")),
            (&three, "/a", outside("/a")),
            (&three, "/etc/passwd", outside("/etc/passwd")),
            // `..` out of a root, by either spelling.
            (&three, "../x", outside("../x")),
            (&three, "/a/b/../x", outside("/a/b/../x")),
            (&three, "/a/b/../bc/y", Ok("1:y".into())),
            (&three, "/c/d/../../etc", outside("/c/d/../../etc")),
            (
                &three,
                "",
                Err(PathError::InvalidPath {
                    reason: crate::workspace::InvalidPathReason::Empty,
                }),
            ),
            (&[], "a.txt", Err(PathError::NoRoots)),
            (&[], "/a/b", Err(PathError::NoRoots)),
            // A repeated root changes nothing.
            (&["/a/b", "/a/b", "/c/d"], "/c/d/z", Ok("1:z".into())),
        ];
        for (list, input, expected) in cases {
            let got = roots(list)
                .resolve(input)
                .map(|r| format!("{}:{}", r.index(), r.path()));
            assert_eq!(got, expected, "{list:?} {input:?}");
        }
    }

    #[test]
    fn a_repeated_root_is_listed_once_and_nested_roots_prefer_the_earlier() {
        assert_eq!(roots(&["/a", "/b", "/a"]).len(), 2);
        let nested = roots(&["/a/b", "/a"]);
        assert_eq!(nested.resolve("/a/b/x").unwrap().index(), 0);
        assert_eq!(nested.resolve("/a/c/x").unwrap().index(), 1);
    }

    #[test]
    fn the_address_is_relative_for_the_first_root_and_absolute_for_the_rest() {
        let list = roots(&["/a/b", "/c/d"]);
        for (input, address) in [("/a/b/x.rs", "x.rs"), ("/c/d/s/y.rs", "/c/d/s/y.rs")] {
            let rooted = list.resolve(input).unwrap();
            assert_eq!(rooted.address().as_str(), address);
            // The address resolves back to the same place.
            assert_eq!(list.resolve(rooted.address().as_str()).unwrap(), rooted);
        }
    }

    #[test]
    fn parse_any_keeps_absolute_paths_and_parse_still_refuses_them() {
        assert_eq!(
            WorkspacePath::parse_any("/a/./b//c/..").unwrap().as_str(),
            "/a/b"
        );
        assert!(WorkspacePath::parse_any("/a").unwrap().is_absolute());
        assert!(!WorkspacePath::parse_any("a/b").unwrap().is_absolute());
        assert!(WorkspacePath::parse("/a").is_err());
    }
}
