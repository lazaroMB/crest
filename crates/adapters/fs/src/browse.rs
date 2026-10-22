//! [`ProjectBrowserPort`] over the local disk: lists the directories of a
//! directory to choose the project crest opens.
//!
//! Only directory entries are returned (symlinks to directories are
//! followed); a directory counts as a project when it holds a `.git` or a
//! `.crest`. Nothing is read or run: only names and metadata.

use std::path::{Path, PathBuf};

use application::ports::projects::{
    BrowseError, DirEntry, DirListing, MAX_DIR_ENTRIES, ProjectBrowserPort,
};
use async_trait::async_trait;

/// Names whose presence makes a directory a project.
const PROJECT_MARKERS: [&str; 2] = [".git", ".crest"];

/// [`ProjectBrowserPort`] over the local disk.
#[derive(Debug, Clone, Default)]
pub struct LocalProjectBrowser {
    home: Option<PathBuf>,
}

impl LocalProjectBrowser {
    /// A browser whose `~` is the user's home (`$HOME`, `%USERPROFILE%`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            home: dirs::home_dir(),
        }
    }

    /// A browser whose `~` is `home` (tests).
    #[must_use]
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }

    /// `input` with a leading `~` replaced by the home directory.
    fn expand(&self, input: &str) -> PathBuf {
        match (input.strip_prefix('~'), &self.home) {
            (Some(rest), Some(home))
                if rest.is_empty() || rest.starts_with(std::path::is_separator) =>
            {
                home.join(rest.trim_start_matches(std::path::is_separator))
            }
            _ => PathBuf::from(input),
        }
    }
}

fn unreadable(path: &Path, err: &std::io::Error) -> BrowseError {
    BrowseError::Unreadable {
        path: path.display().to_string(),
        reason: err.to_string(),
    }
}

/// The canonical directory at `path`, or why it is not one.
async fn directory(path: &Path) -> Result<PathBuf, BrowseError> {
    let shown = || path.display().to_string();
    let canonical = match crate::guard::canonicalize(path).await {
        Ok(canonical) => canonical,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(BrowseError::NotFound(shown()));
        }
        Err(err) => return Err(unreadable(path, &err)),
    };
    let meta = tokio::fs::metadata(&canonical)
        .await
        .map_err(|err| unreadable(path, &err))?;
    if meta.is_dir() {
        Ok(canonical)
    } else {
        Err(BrowseError::NotADirectory(shown()))
    }
}

async fn is_project(dir: &Path) -> bool {
    for marker in PROJECT_MARKERS {
        if tokio::fs::symlink_metadata(dir.join(marker)).await.is_ok() {
            return true;
        }
    }
    false
}

#[async_trait]
impl ProjectBrowserPort for LocalProjectBrowser {
    fn start_dir(&self) -> Option<PathBuf> {
        std::env::current_dir().ok().or_else(|| self.home.clone())
    }

    async fn list_dirs(&self, path: &Path) -> Result<DirListing, BrowseError> {
        let path = directory(path).await?;
        let mut entries = tokio::fs::read_dir(&path)
            .await
            .map_err(|err| unreadable(&path, &err))?;
        let mut dirs = Vec::new();
        let mut truncated = false;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| unreadable(&path, &err))?
        {
            // `metadata` follows symlinks: a link to a directory counts.
            let entry_path = entry.path();
            match tokio::fs::metadata(&entry_path).await {
                Ok(meta) if meta.is_dir() => {}
                _ => continue,
            }
            if dirs.len() == MAX_DIR_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            dirs.push(DirEntry {
                hidden: name.starts_with('.'),
                is_project: is_project(&entry_path).await,
                path: entry_path,
                name,
            });
        }
        dirs.sort_by(|a, b| {
            b.is_project
                .cmp(&a.is_project)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(DirListing {
            parent: path.parent().map(Path::to_path_buf),
            path,
            dirs,
            truncated,
        })
    }

    async fn resolve(&self, input: &str) -> Result<PathBuf, BrowseError> {
        let path = directory(&self.expand(input.trim())).await?;
        // Opening it is enough to know it can be listed.
        tokio::fs::read_dir(&path)
            .await
            .map(drop)
            .map_err(|err| unreadable(&path, &err))?;
        Ok(path)
    }
}

// The fixtures build symlinks and permission modes, which are Unix-only.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for sub in ["notes", "crest/.git", "Beta/.crest", ".cache", "locked"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("notes"), root.join("linked")).unwrap();
        dir
    }

    #[tokio::test]
    async fn lists_only_directories_with_projects_first() {
        let dir = tree();
        let browser = LocalProjectBrowser::with_home(dir.path());
        let listing = browser.list_dirs(dir.path()).await.unwrap();
        let names: Vec<(&str, bool, bool)> = listing
            .dirs
            .iter()
            .map(|d| (d.name.as_str(), d.is_project, d.hidden))
            .collect();
        assert_eq!(
            names,
            [
                ("Beta", true, false),
                ("crest", true, false),
                (".cache", false, true),
                ("linked", false, false),
                ("locked", false, false),
                ("notes", false, false),
            ]
        );
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(listing.path, root);
        assert_eq!(listing.parent.as_deref(), root.parent());
        assert!(!listing.truncated);
    }

    #[tokio::test]
    async fn bad_paths_say_why() {
        let dir = tree();
        let browser = LocalProjectBrowser::with_home(dir.path());
        assert!(matches!(
            browser.list_dirs(&dir.path().join("nope")).await,
            Err(BrowseError::NotFound(_))
        ));
        assert!(matches!(
            browser.list_dirs(&dir.path().join("file.txt")).await,
            Err(BrowseError::NotADirectory(_))
        ));
        let locked = dir.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads everything: only check when permissions apply.
        if std::fs::read_dir(&locked).is_err() {
            assert!(matches!(
                browser.list_dirs(&locked).await,
                Err(BrowseError::Unreadable { .. })
            ));
            assert!(matches!(
                browser.resolve(locked.to_str().unwrap()).await,
                Err(BrowseError::Unreadable { .. })
            ));
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    async fn resolve_expands_the_home_and_canonicalizes() {
        let dir = tree();
        let browser = LocalProjectBrowser::with_home(dir.path());
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(browser.resolve("~").await.unwrap(), root);
        assert_eq!(
            browser.resolve(" ~/crest/ ").await.unwrap(),
            root.join("crest")
        );
        assert_eq!(
            browser.resolve("~/linked").await.unwrap(),
            root.join("notes"),
            "symlinks resolve to their target"
        );
        assert!(matches!(
            browser.resolve("~/file.txt").await,
            Err(BrowseError::NotADirectory(_))
        ));
        assert!(matches!(
            browser.resolve("~nobody").await,
            Err(BrowseError::NotFound(_))
        ));
    }
}
