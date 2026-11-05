//! Projects: a name and one or more directories that chats can belong to.
//!
//! The rules live here, pure, so every store applies the same ones: a
//! project needs a name and at least one directory, directories are kept
//! without a trailing slash and never repeat, and the last directory cannot
//! be removed.

use crate::conversation::ProjectId;

/// Why a change to a project is refused. The messages are meant for the
/// person using the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    /// The name is empty or only spaces.
    #[error("a project needs a name")]
    NameMissing,
    /// A project was created with no directory.
    #[error("a project needs at least one directory")]
    NoDirectory,
    /// The directory to add is empty or only spaces.
    #[error("a directory path cannot be empty")]
    EmptyDirectory,
    /// The directory to remove is the only one left.
    #[error("a project keeps at least one directory")]
    LastDirectory,
}

/// A named set of directories; chats in it can use every one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Project id.
    pub id: ProjectId,
    /// Display name; not unique.
    pub name: String,
    /// Whether it is listed first.
    pub pinned: bool,
    /// Its directories in the order they were added; at least one, no
    /// repeats, none with a trailing slash.
    pub roots: Vec<String>,
}

/// `path` without surrounding spaces or trailing slashes (`/` stays `/`);
/// `None` when nothing is left of it.
#[must_use]
pub fn normalize_root(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let stripped = path.trim_end_matches('/');
    Some(if stripped.is_empty() { "/" } else { stripped }.to_owned())
}

/// The name a project gets from its directory: the last path component
/// (the whole path for `/`).
#[must_use]
pub fn name_from_root(root: &str) -> String {
    let root = normalize_root(root).unwrap_or_default();
    match root.rsplit(['/', '\\']).next() {
        Some(last) if !last.is_empty() => last.to_owned(),
        _ => root,
    }
}

fn clean_name(name: &str) -> Result<String, ProjectError> {
    match name.trim() {
        "" => Err(ProjectError::NameMissing),
        name => Ok(name.to_owned()),
    }
}

impl Project {
    /// A new project with a fresh id, not pinned. Directories that are
    /// empty or repeat an earlier one are dropped.
    ///
    /// # Errors
    /// [`ProjectError::NameMissing`] without a name, or
    /// [`ProjectError::NoDirectory`] when no directory is left.
    pub fn new(name: &str, roots: &[String]) -> Result<Self, ProjectError> {
        let name = clean_name(name)?;
        let mut clean: Vec<String> = Vec::new();
        for root in roots.iter().filter_map(|root| normalize_root(root)) {
            if !clean.contains(&root) {
                clean.push(root);
            }
        }
        if clean.is_empty() {
            return Err(ProjectError::NoDirectory);
        }
        Ok(Self {
            id: ProjectId::new(),
            name,
            pinned: false,
            roots: clean,
        })
    }

    /// Renames the project.
    ///
    /// # Errors
    /// [`ProjectError::NameMissing`] for an empty name.
    pub fn rename(&mut self, name: &str) -> Result<(), ProjectError> {
        self.name = clean_name(name)?;
        Ok(())
    }

    /// Adds a directory after the others. Returns the normalized root when
    /// it was added, `None` when the project already had it.
    ///
    /// # Errors
    /// [`ProjectError::EmptyDirectory`] for an empty path.
    pub fn add_root(&mut self, path: &str) -> Result<Option<String>, ProjectError> {
        let root = normalize_root(path).ok_or(ProjectError::EmptyDirectory)?;
        if self.roots.contains(&root) {
            return Ok(None);
        }
        self.roots.push(root.clone());
        Ok(Some(root))
    }

    /// Removes a directory. Returns the normalized root when it was
    /// removed, `None` when the project did not have it.
    ///
    /// # Errors
    /// [`ProjectError::LastDirectory`] when it is the only directory.
    pub fn remove_root(&mut self, path: &str) -> Result<Option<String>, ProjectError> {
        let Some(root) = normalize_root(path).filter(|root| self.roots.contains(root)) else {
            return Ok(None);
        };
        if self.roots.len() == 1 {
            return Err(ProjectError::LastDirectory);
        }
        self.roots.retain(|r| *r != root);
        Ok(Some(root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn roots_are_normalized() {
        for (input, expected) in [
            ("/a/b", Some("/a/b")),
            ("/a/b/", Some("/a/b")),
            ("  /a/b//  ", Some("/a/b")),
            ("/", Some("/")),
            ("///", Some("/")),
            ("", None),
            ("   ", None),
        ] {
            assert_eq!(normalize_root(input).as_deref(), expected, "{input:?}");
        }
    }

    #[test]
    fn names_come_from_the_last_component() {
        for (root, name) in [
            ("/home/me/crest", "crest"),
            ("/home/me/crest/", "crest"),
            ("/", "/"),
            ("relative", "relative"),
            ("C:\\work\\app", "app"),
        ] {
            assert_eq!(name_from_root(root), name, "{root:?}");
        }
    }

    #[test]
    fn a_project_needs_a_name_and_a_directory() {
        for (name, dirs, error) in [
            ("", &["/a"][..], ProjectError::NameMissing),
            ("  ", &["/a"], ProjectError::NameMissing),
            ("x", &[], ProjectError::NoDirectory),
            ("x", &["", "  "], ProjectError::NoDirectory),
        ] {
            assert_eq!(Project::new(name, &roots(dirs)).unwrap_err(), error);
        }
        let project = Project::new(" app ", &roots(&["/a/", "/b", "/a", ""])).unwrap();
        assert_eq!((project.name.as_str(), project.pinned), ("app", false));
        assert_eq!(project.roots, ["/a", "/b"]);
    }

    #[test]
    fn roots_are_added_and_removed_by_the_rules() {
        let mut project = Project::new("p", &roots(&["/a"])).unwrap();
        assert_eq!(project.add_root("/b/").unwrap().as_deref(), Some("/b"));
        assert_eq!(project.add_root("/b").unwrap(), None);
        assert_eq!(project.add_root(" "), Err(ProjectError::EmptyDirectory));
        assert_eq!(project.roots, ["/a", "/b"]);
        assert_eq!(project.remove_root("/zzz").unwrap(), None);
        assert_eq!(project.remove_root("/a/").unwrap().as_deref(), Some("/a"));
        assert_eq!(project.remove_root("/b"), Err(ProjectError::LastDirectory));
        assert_eq!(project.remove_root("/zzz").unwrap(), None);
        assert_eq!(project.rename(" "), Err(ProjectError::NameMissing));
        project.rename(" q ").unwrap();
        assert_eq!(project.name, "q");
    }
}
