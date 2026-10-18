//! In-memory fakes of the file ports: [`InMemoryFs`] implements
//! [`FileSystemPort`] over a map of paths to bytes, so use cases can be
//! tested without touching the disk.
//!
//! The fake applies the same guards the real adapter does (sensitive paths,
//! binary files, the read size limit) and lets tests simulate what only a
//! real file system produces: [`InMemoryFs::with_escaping_link`] marks a
//! path as a symlink that resolves outside the workspace, and
//! [`InMemoryFs::set_file`] changes a file behind the use case's back.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::workspace::{PathError, SensitivePathRules, WorkspacePath, WorkspaceRoot};

use crate::ports::fs::{
    EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, MAX_READ_BYTES,
    PendingChange, ReadFileRequest, WriteFileRequest,
};

/// A workspace held in memory. Directories are implied by the files below
/// them, as in a file listing, so empty directories do not exist.
#[derive(Debug)]
pub struct InMemoryFs {
    root: WorkspaceRoot,
    rules: SensitivePathRules,
    max_read_bytes: u64,
    escaping: BTreeSet<WorkspacePath>,
    files: Mutex<BTreeMap<WorkspacePath, Vec<u8>>>,
}

impl InMemoryFs {
    /// An empty workspace at `root` with the default sensitive-path rules.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            rules: SensitivePathRules::default(),
            max_read_bytes: MAX_READ_BYTES,
            escaping: BTreeSet::new(),
            files: Mutex::new(BTreeMap::new()),
        }
    }

    /// Adds a text file.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn with_file(self, path: &str, content: impl Into<String>) -> Self {
        self.with_bytes(path, content.into().into_bytes())
    }

    /// Adds a file with raw content, e.g. to simulate a binary file.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn with_bytes(self, path: &str, content: impl Into<Vec<u8>>) -> Self {
        self.lock().insert(parse(path), content.into());
        self
    }

    /// Replaces the sensitive-path rules (the defaults deny `.env*`,
    /// `*.pem`, `id_rsa*` and `.git/`).
    pub fn with_sensitive_rules(mut self, rules: SensitivePathRules) -> Self {
        self.rules = rules;
        self
    }

    /// Lowers the size limit of reads without range, so tests need not
    /// build files of [`MAX_READ_BYTES`].
    pub fn with_max_read_bytes(mut self, limit: u64) -> Self {
        self.max_read_bytes = limit;
        self
    }

    /// Marks `path` as a symlink that resolves outside the workspace: every
    /// access to it or below it fails with [`PathError::OutsideWorkspace`],
    /// as the real adapter does after canonicalizing.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn with_escaping_link(mut self, path: &str) -> Self {
        self.escaping.insert(parse(path));
        self
    }

    /// Creates or overwrites a text file, e.g. to make a prepared change
    /// stale.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn set_file(&self, path: &str, content: impl Into<String>) {
        self.lock().insert(parse(path), content.into().into_bytes());
    }

    /// Removes a file, returning whether it existed.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn remove_file(&self, path: &str) -> bool {
        self.lock().remove(&parse(path)).is_some()
    }

    /// The content of a file, decoded lossily, or `None` if it is missing.
    ///
    /// # Panics
    ///
    /// When `path` is not a valid relative workspace path.
    pub fn file(&self, path: &str) -> Option<String> {
        self.lock()
            .get(&parse(path))
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    /// Every file with its content decoded lossily, sorted by path, for
    /// asserting on the whole workspace at once.
    pub fn snapshot(&self) -> BTreeMap<WorkspacePath, String> {
        self.lock()
            .iter()
            .map(|(path, bytes)| (path.clone(), String::from_utf8_lossy(bytes).into_owned()))
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<WorkspacePath, Vec<u8>>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fails when `path` is at or below an escaping link.
    fn check_inside(&self, path: &WorkspacePath) -> Result<(), PathError> {
        if self.escaping.iter().any(|link| is_within(path, link)) {
            return Err(PathError::OutsideWorkspace {
                path: path.to_string(),
            });
        }
        Ok(())
    }

    /// The guards of every read and write: inside the workspace and not
    /// sensitive.
    fn check_access(&self, path: &WorkspacePath) -> Result<(), FsError> {
        self.check_inside(path)?;
        if let Some(pattern) = self.rules.matching_pattern(path) {
            return Err(FsError::SensitivePath {
                path: path.clone(),
                pattern: pattern.to_owned(),
            });
        }
        Ok(())
    }
}

fn parse(path: &str) -> WorkspacePath {
    match WorkspacePath::parse(path) {
        Ok(path) => path,
        Err(err) => panic!("invalid path {path:?} for InMemoryFs: {err}"),
    }
}

/// Whether `path` is `base` or lies below it.
fn is_within(path: &WorkspacePath, base: &WorkspacePath) -> bool {
    base.is_root() || {
        let mut components = path.components();
        base.components().all(|c| components.next() == Some(c))
    }
}

/// Whether some file lies strictly below `path`.
fn is_dir(files: &BTreeMap<WorkspacePath, Vec<u8>>, path: &WorkspacePath) -> bool {
    path.is_root()
        || files
            .keys()
            .any(|file| file != path && is_within(file, path))
}

/// The file content as text, or `None` when it looks binary (NUL bytes or
/// invalid UTF-8).
fn as_text(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

#[async_trait]
impl FileSystemPort for InMemoryFs {
    fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent, FsError> {
        let path = &request.path;
        self.check_access(path)?;
        let files = self.lock();
        let Some(bytes) = files.get(path) else {
            return Err(if is_dir(&files, path) {
                FsError::IsDirectory { path: path.clone() }
            } else {
                FsError::NotFound { path: path.clone() }
            });
        };
        let size = bytes.len() as u64;
        if size > self.max_read_bytes && !request.has_explicit_range() {
            return Err(FsError::TooLarge {
                size,
                limit: self.max_read_bytes,
            });
        }
        let text = as_text(bytes).ok_or_else(|| FsError::Binary { path: path.clone() })?;
        Ok(FileContent::from_text(text, &request))
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        self.check_inside(path)?;
        let files = self.lock();
        Ok(files.contains_key(path) || is_dir(&files, path))
    }

    async fn prepare_edit(&self, _request: EditFileRequest) -> Result<PendingChange, FsError> {
        Err(FsError::Io("prepare_edit is not supported yet".into()))
    }

    async fn prepare_write(&self, _request: WriteFileRequest) -> Result<PendingChange, FsError> {
        Err(FsError::Io("prepare_write is not supported yet".into()))
    }

    async fn apply(&self, _change: PendingChange) -> Result<FileChange, FsError> {
        Err(FsError::Io("apply is not supported yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn fs() -> InMemoryFs {
        InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
            .with_file("src/lib.rs", "one\ntwo\nthree\n")
            .with_file(".env", "KEY=secret\n")
            .with_bytes("logo.png", vec![0x89, b'P', 0, 1])
    }

    fn read(path: &str) -> ReadFileRequest {
        ReadFileRequest::new(p(path))
    }

    #[tokio::test]
    async fn reads_a_slice_of_a_text_file() {
        let fs = fs();
        assert_eq!(fs.root().as_path(), std::path::Path::new("/ws"));
        let request = ReadFileRequest {
            offset: Some(2),
            limit: Some(1),
            ..read("src/lib.rs")
        };
        let content = fs.read_file(request).await.unwrap();
        assert_eq!(content.lines, ["two"]);
        assert_eq!((content.start_line, content.total_lines), (2, 3));
        assert!(content.truncated);
    }

    #[tokio::test]
    async fn read_reports_missing_files_and_directories() {
        let fs = fs();
        assert_eq!(
            fs.read_file(read("src/main.rs")).await,
            Err(FsError::NotFound {
                path: p("src/main.rs")
            })
        );
        for dir in ["src", "."] {
            assert_eq!(
                fs.read_file(read(dir)).await,
                Err(FsError::IsDirectory { path: p(dir) })
            );
        }
    }

    #[tokio::test]
    async fn read_rejects_binary_and_sensitive_files() {
        let fs = fs().with_bytes("bad.txt", vec![0xff, 0xfe]);
        for path in ["logo.png", "bad.txt"] {
            assert_eq!(
                fs.read_file(read(path)).await,
                Err(FsError::Binary { path: p(path) })
            );
        }
        assert_eq!(
            fs.read_file(read(".env")).await,
            Err(FsError::SensitivePath {
                path: p(".env"),
                pattern: ".env*".into()
            })
        );
        let relaxed = fs.with_sensitive_rules(SensitivePathRules::new(["*.key"]).unwrap());
        assert!(relaxed.read_file(read(".env")).await.is_ok());
    }

    #[tokio::test]
    async fn large_files_need_an_explicit_range() {
        let fs = fs().with_max_read_bytes(5);
        assert_eq!(
            fs.read_file(read("src/lib.rs")).await,
            Err(FsError::TooLarge { size: 14, limit: 5 })
        );
        let ranged = ReadFileRequest {
            limit: Some(1),
            ..read("src/lib.rs")
        };
        assert_eq!(fs.read_file(ranged).await.unwrap().lines, ["one"]);
    }

    #[tokio::test]
    async fn escaping_links_are_outside_the_workspace() {
        let fs = fs()
            .with_file("vendor/out/a.rs", "x")
            .with_escaping_link("vendor/out");
        let outside = PathError::OutsideWorkspace {
            path: "vendor/out/a.rs".into(),
        };
        assert_eq!(
            fs.read_file(read("vendor/out/a.rs")).await,
            Err(outside.clone().into())
        );
        assert_eq!(fs.exists(&p("vendor/out/a.rs")).await, Err(outside.into()));
        assert_eq!(fs.exists(&p("vendor")).await, Ok(true));
    }

    #[tokio::test]
    async fn exists_sees_files_and_implied_directories() {
        let fs = fs();
        for (path, expected) in [
            ("src/lib.rs", true),
            ("src", true),
            (".", true),
            ("src/li", false),
            ("missing", false),
        ] {
            assert_eq!(fs.exists(&p(path)).await, Ok(expected), "{path}");
        }
    }

    #[test]
    fn snapshot_and_mutators_expose_the_tree() {
        let fs = fs();
        fs.set_file("README.md", "hi");
        assert!(fs.remove_file(".env"));
        assert!(!fs.remove_file(".env"));
        assert_eq!(fs.file("README.md").as_deref(), Some("hi"));
        assert_eq!(fs.file(".env"), None);
        let paths: Vec<_> = fs.snapshot().into_keys().map(|k| k.to_string()).collect();
        assert_eq!(paths, ["README.md", "logo.png", "src/lib.rs"]);
    }
}
