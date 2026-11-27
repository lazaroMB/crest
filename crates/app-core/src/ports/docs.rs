//! The files of chat documents (R03, spec 0003 section 2).
//!
//! A document is a markdown file in `<dir>`, with its images in
//! `<dir>/assets/`. The person is the author, so writes need no approval:
//! this port is separate from [`FileSystemPort`](super::fs::FileSystemPort)
//! and the agent never receives it. It only reaches the two kinds of folder
//! docs may live in, which [`check_dir`] enforces for every implementation.

use async_trait::async_trait;

/// Why a document file operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocFilesError {
    /// The folder is not under a document base.
    #[error("{0} is outside the document folders")]
    OutsideBase(String),
    /// The file name has a separator, `..` or is empty.
    #[error("{0:?} is not a valid file name")]
    BadName(String),
    /// The file or folder does not exist.
    #[error("{0} not found")]
    NotFound(String),
    /// The disk failed, or a limit was exceeded.
    #[error("{0}")]
    Io(String),
}

/// Where the files of docs may live: the roots of the chat's project (each
/// gives `<root>/.crest/docs/`) and crest's data dir (`<data_dir>/docs/`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocBases {
    /// Roots of the project; a doc dir is `<root>/.crest/docs/<id>`.
    pub roots: Vec<String>,
    /// Data dir of crest; a doc dir is `<data_dir>/docs/<id>`.
    pub data_dir: Option<String>,
}

/// Reads and writes the files of documents.
#[async_trait]
pub trait DocFilesPort: Send + Sync {
    /// The text of `<dir>/<file>`.
    async fn read(&self, dir: &str, file: &str) -> Result<String, DocFilesError>;

    /// Writes `<dir>/<file>`, creating the folder; a temporary file in the
    /// folder is renamed over it so a crash leaves the old text.
    async fn write(&self, dir: &str, file: &str, text: &str) -> Result<(), DocFilesError>;

    /// Writes `<dir>/assets/<name>` with the same atomic rule.
    async fn write_asset(&self, dir: &str, name: &str, bytes: &[u8]) -> Result<(), DocFilesError>;

    /// The bytes of `<dir>/assets/<name>`; [`DocFilesError::Io`] when larger
    /// than `max`.
    async fn read_asset(&self, dir: &str, name: &str, max: usize)
    -> Result<Vec<u8>, DocFilesError>;

    /// Moves the folder `from` to `to` (a rename; across filesystems a copy
    /// and then a removal). On failure the old folder and its files stay. A
    /// missing `from` is `Ok`: there is nothing to move.
    async fn relocate(&self, from: &str, to: &str) -> Result<(), DocFilesError>;

    /// Deletes `<dir>/<file>`; a missing file is `Ok`.
    async fn remove(&self, dir: &str, file: &str) -> Result<(), DocFilesError>;

    /// Whether `<dir>/<file>` exists.
    async fn exists(&self, dir: &str, file: &str) -> bool;
}

fn has_dotdot(path: &str) -> bool {
    path.split(['/', '\\']).any(|seg| seg == "..")
}

/// Accepts `dir` only when it is `<base>/docs/<id>` for a base of `bases`:
/// `<root>/.crest` for each root, or the data dir. Trailing slashes are
/// ignored; a `..` segment anywhere, a deeper path or a sibling is refused.
///
/// # Errors
/// [`DocFilesError::OutsideBase`] otherwise.
pub fn check_dir(bases: &DocBases, dir: &str) -> Result<(), DocFilesError> {
    let refuse = || DocFilesError::OutsideBase(dir.to_owned());
    let dir_trimmed = dir.trim_end_matches('/');
    if dir_trimmed.is_empty() || has_dotdot(dir) {
        return Err(refuse());
    }
    let under = |base: &str| -> bool {
        let base = base.trim_end_matches('/');
        if base.is_empty() {
            return false;
        }
        let prefix = format!("{base}/docs/");
        dir_trimmed
            .strip_prefix(&prefix)
            .is_some_and(|id| !id.is_empty() && id != "." && !id.contains(['/', '\\']))
    };
    let rooted = bases
        .roots
        .iter()
        .any(|root| under(&format!("{}/.crest", root.trim_end_matches('/'))));
    let data = bases.data_dir.as_deref().is_some_and(under);
    if rooted || data {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// Accepts a file name: no separator, no `..`, not empty.
///
/// # Errors
/// [`DocFilesError::BadName`] otherwise.
pub fn check_name(name: &str) -> Result<(), DocFilesError> {
    if domain::doc::is_safe_name(name) {
        Ok(())
    } else {
        Err(DocFilesError::BadName(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases() -> DocBases {
        DocBases {
            roots: vec!["/work/api".into(), "/work/web/".into()],
            data_dir: Some("/data/crest".into()),
        }
    }

    #[test]
    fn accepts_the_docs_folder_of_each_root_and_of_the_data_dir() {
        for dir in [
            "/work/api/.crest/docs/abc",
            "/work/web/.crest/docs/abc",
            "/work/web/.crest/docs/abc/",
            "/data/crest/docs/abc",
        ] {
            assert_eq!(check_dir(&bases(), dir), Ok(()), "{dir}");
        }
    }

    #[test]
    fn refuses_everything_else() {
        for dir in [
            "",
            "/",
            "/work/api/.crest/docs",
            "/work/api/.crest/docs/",
            "/work/api/.crest/docs/abc/assets",
            "/work/api/.crest/docs/../x",
            "/work/api/.crest/docs/abc/..",
            "/work/api/.crest/other/abc",
            "/work/api/.crest",
            "/work/api/docs/abc",
            "/work/api/.crest/docs2/abc",
            "/work/api",
            "/other/.crest/docs/abc",
            "/data/crest/docs/abc/x",
            "/data/docs/abc",
            "/data/crest/docs/..",
            "/work/api/.crest/docs/.",
        ] {
            assert_eq!(
                check_dir(&bases(), dir),
                Err(DocFilesError::OutsideBase(dir.to_owned())),
                "{dir}"
            );
        }
    }

    #[test]
    fn a_root_that_is_not_in_the_list_is_refused() {
        let none = DocBases::default();
        assert!(check_dir(&none, "/work/api/.crest/docs/abc").is_err());
        assert!(check_dir(&none, "/data/crest/docs/abc").is_err());
    }

    #[test]
    fn names_with_separators_or_dots_are_refused() {
        for name in ["", "a/b.md", "a\\b.md", "..", "../a.md", "."] {
            assert_eq!(
                check_name(name),
                Err(DocFilesError::BadName(name.to_owned())),
                "{name:?}"
            );
        }
        assert_eq!(check_name("release-notes.md"), Ok(()));
        assert_eq!(check_name("chart 1.png"), Ok(()));
    }
}
