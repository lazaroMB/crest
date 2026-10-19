//! File system port: reading and writing files inside the workspace.
//!
//! Every path is a [`WorkspacePath`], already lexically confined to the
//! workspace. Adapters must still join it to [`FileSystemPort::root`],
//! canonicalize the result (the parent directory when the file does not
//! exist yet) and fail with [`PathError::OutsideWorkspace`] when it escapes
//! the root, which also blocks symlinks pointing outside. Paths matched by
//! the configured [`SensitivePathRules`](domain::workspace::SensitivePathRules)
//! fail with [`FsError::SensitivePath`].
//!
//! Cancellation is done by dropping the returned future.

use async_trait::async_trait;
use domain::workspace::{PathError, WorkspacePath, WorkspaceRoot};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Lines returned by `read_file` when the request has no `limit`.
pub const DEFAULT_READ_LIMIT: usize = 2000;
/// Files larger than this (in bytes) are only read with an explicit range.
pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Errors of [`FileSystemPort`].
///
/// I/O failures are carried as [`FsError::Io`] with the rendered message,
/// so the error stays `Clone + PartialEq` (`std::io::Error` is neither).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    /// The path is invalid or escapes the workspace.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The file does not exist.
    #[error("file not found: {path}")]
    NotFound {
        /// The requested path.
        path: WorkspacePath,
    },
    /// The path is a directory where a file was expected.
    #[error("path is a directory: {path}")]
    IsDirectory {
        /// The requested path.
        path: WorkspacePath,
    },
    /// The file looks binary (it contains NUL bytes or is not UTF-8).
    #[error("file is binary: {path}")]
    Binary {
        /// The requested path.
        path: WorkspacePath,
    },
    /// The file exceeds [`MAX_READ_BYTES`] and no explicit range was given.
    #[error("file is too large ({size} bytes, limit {limit}); read it with offset and limit")]
    TooLarge {
        /// Size of the file in bytes.
        size: u64,
        /// The size limit in bytes.
        limit: u64,
    },
    /// The path matches a sensitive-path pattern and may not be accessed.
    #[error("access to {path} is denied by the sensitive path pattern `{pattern}`")]
    SensitivePath {
        /// The requested path.
        path: WorkspacePath,
        /// The pattern that denied it.
        pattern: String,
    },
    /// `old` of an edit is empty, so it cannot locate a single place.
    #[error("the text to replace is empty")]
    EmptyOld,
    /// `old` of an edit does not occur in the file.
    #[error("the text to replace was not found in the file")]
    OldNotFound,
    /// `old` of an edit occurs more than once; it must be unique.
    #[error("the text to replace occurs {count} times; include more context to make it unique")]
    OldNotUnique {
        /// How many (non-overlapping) occurrences were found.
        count: usize,
    },
    /// The file changed between preparing a change and applying it.
    #[error("{path} changed since the edit was prepared; prepare it again")]
    Stale {
        /// The file that changed.
        path: WorkspacePath,
    },
    /// Any other I/O failure, as its rendered message.
    #[error("file system error: {0}")]
    Io(String),
}

/// Input of the `read_file` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileRequest {
    /// File to read.
    pub path: WorkspacePath,
    /// 1-based number of the first line to return (default 1).
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return (default 2000).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl ReadFileRequest {
    /// A request for the start of `path` with the default limit.
    pub fn new(path: WorkspacePath) -> Self {
        Self {
            path,
            offset: None,
            limit: None,
        }
    }

    /// The 1-based first line to return; `0` is treated as `1`.
    pub fn start_line(&self) -> usize {
        self.offset.unwrap_or(1).max(1)
    }

    /// The maximum number of lines to return.
    pub fn line_limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_READ_LIMIT)
    }

    /// Whether the caller asked for a range, which allows reading files
    /// larger than [`MAX_READ_BYTES`].
    pub fn has_explicit_range(&self) -> bool {
        self.offset.is_some() || self.limit.is_some()
    }
}

/// A slice of a text file returned by `read_file`.
///
/// Lines are stored without numbers or terminators, so the UI can render
/// them freely; [`FileContent::render_numbered`] produces the text sent to
/// the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    /// The file that was read.
    pub path: WorkspacePath,
    /// The returned lines, without line terminators.
    pub lines: Vec<String>,
    /// 1-based number of the first returned line.
    pub start_line: usize,
    /// Number of lines in the whole file.
    pub total_lines: usize,
    /// Whether the file has lines after the returned ones.
    pub truncated: bool,
}

impl FileContent {
    /// Slices `text` (the whole file) as `request` asks. Lines are split
    /// as by [`str::lines`], so `\r\n` terminators are removed too.
    pub fn from_text(text: &str, request: &ReadFileRequest) -> Self {
        let start_line = request.start_line();
        let mut total_lines = 0;
        let mut lines = Vec::new();
        for (index, line) in text.lines().enumerate() {
            total_lines += 1;
            if index + 1 >= start_line && lines.len() < request.line_limit() {
                lines.push(line.to_owned());
            }
        }
        let last_shown = start_line.saturating_sub(1) + lines.len();
        Self {
            path: request.path.clone(),
            truncated: last_shown < total_lines,
            lines,
            start_line,
            total_lines,
        }
    }

    /// Number of the line after the last returned one, to continue reading.
    pub fn next_offset(&self) -> usize {
        self.start_line + self.lines.len()
    }

    /// Renders the lines for the model as `cat -n` does (right-aligned line
    /// number, a tab, the line), followed by a hint to continue when
    /// truncated.
    pub fn render_numbered(&self) -> String {
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            out.push_str(&format!("{:>6}\t{line}\n", self.start_line + index));
        }
        if self.truncated {
            out.push_str(&format!(
                "[showing lines {}-{} of {}; continue with offset={}]\n",
                self.start_line,
                self.next_offset() - 1,
                self.total_lines,
                self.next_offset(),
            ));
        }
        out
    }
}

/// Input of the `edit_file` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditFileRequest {
    /// File to edit; it must exist.
    pub path: WorkspacePath,
    /// Exact text to replace; it must occur exactly once in the file.
    pub old: String,
    /// Replacement text.
    pub new: String,
}

/// Input of the `write_file` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileRequest {
    /// File to create or overwrite; missing parent directories are created.
    pub path: WorkspacePath,
    /// Full new content of the file.
    pub content: String,
}

/// Replaces the single occurrence of `old` in `text` with `new`.
///
/// This is the exact-match rule of `edit_file`: empty `old` fails with
/// [`FsError::EmptyOld`], no occurrence with [`FsError::OldNotFound`] and
/// several (counted without overlap) with [`FsError::OldNotUnique`].
pub fn replace_unique(text: &str, old: &str, new: &str) -> Result<String, FsError> {
    if old.is_empty() {
        return Err(FsError::EmptyOld);
    }
    match text.matches(old).count() {
        0 => Err(FsError::OldNotFound),
        1 => Ok(text.replacen(old, new, 1)),
        count => Err(FsError::OldNotUnique { count }),
    }
}

/// The outcome of a change, shown in the approval dialog and the timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// The changed file.
    pub path: WorkspacePath,
    /// Unified diff from the old content (empty for new files) to the new.
    pub diff_unified: String,
    /// Whether the file did not exist before.
    pub created: bool,
}

/// A change computed but not yet written, produced by
/// [`FileSystemPort::prepare_edit`] or [`FileSystemPort::prepare_write`].
///
/// Edits happen in two phases so that approval sits between them: the use
/// case prepares the change, shows [`PendingChange::diff_unified`] to the
/// user and only then calls [`FileSystemPort::apply`]. The original content
/// is kept so `apply` can detect that the file changed meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    path: WorkspacePath,
    original: Option<String>,
    new_content: String,
    diff_unified: String,
}

impl PendingChange {
    /// A change of `path` from `original` (`None` when the file does not
    /// exist) to `new_content`, with its precomputed unified diff.
    pub fn new(
        path: WorkspacePath,
        original: Option<String>,
        new_content: String,
        diff_unified: String,
    ) -> Self {
        Self {
            path,
            original,
            new_content,
            diff_unified,
        }
    }

    /// The file to change.
    pub fn path(&self) -> &WorkspacePath {
        &self.path
    }

    /// The content seen when preparing, or `None` if the file did not exist.
    pub fn original(&self) -> Option<&str> {
        self.original.as_deref()
    }

    /// The content to write.
    pub fn new_content(&self) -> &str {
        &self.new_content
    }

    /// Unified diff from [`Self::original`] to [`Self::new_content`].
    pub fn diff_unified(&self) -> &str {
        &self.diff_unified
    }

    /// Whether applying it creates the file.
    pub fn creates(&self) -> bool {
        self.original.is_none()
    }

    /// The [`FileChange`] reported once this change is applied.
    pub fn to_change(&self) -> FileChange {
        FileChange {
            path: self.path.clone(),
            diff_unified: self.diff_unified.clone(),
            created: self.creates(),
        }
    }
}

/// Reads and writes files inside one workspace, enforcing its guards.
#[async_trait]
pub trait FileSystemPort: Send + Sync {
    /// The canonical root every path is relative to; fixed for the session.
    fn root(&self) -> &WorkspaceRoot;

    /// Reads a text file and returns the slice selected by `request` (see
    /// [`FileContent::from_text`]). Fails with [`FsError::Binary`] for
    /// binary files, with [`FsError::TooLarge`] for files over
    /// [`MAX_READ_BYTES`] unless the request has an explicit range, and
    /// with [`FsError::SensitivePath`] for denied paths.
    async fn read_file(&self, request: ReadFileRequest) -> Result<FileContent, FsError>;

    /// Reads a whole file as bytes, text or not (a 3D model). Fails with
    /// [`FsError::TooLarge`] for files over `max` bytes and with
    /// [`FsError::SensitivePath`] for denied paths.
    async fn read_bytes(&self, path: &WorkspacePath, max: u64) -> Result<Vec<u8>, FsError>;

    /// Whether `path` exists (as a file or directory) inside the workspace.
    /// Paths that resolve outside the root fail instead of returning false.
    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError>;

    /// Computes the edit described by `request` (see [`replace_unique`])
    /// without writing it. Fails with [`FsError::NotFound`] when the file
    /// does not exist and with [`FsError::SensitivePath`] for denied paths.
    async fn prepare_edit(&self, request: EditFileRequest) -> Result<PendingChange, FsError>;

    /// Computes the creation or overwrite described by `request` without
    /// writing it; the diff is against the current content when the file
    /// exists. Fails with [`FsError::SensitivePath`] for denied paths.
    async fn prepare_write(&self, request: WriteFileRequest) -> Result<PendingChange, FsError>;

    /// Writes a prepared change, after re-checking the workspace guards.
    /// Fails with [`FsError::Stale`] when the file no longer matches
    /// [`PendingChange::original`] (or now exists for a creation), so an
    /// approval never applies to content the user did not see.
    async fn apply(&self, change: PendingChange) -> Result<FileChange, FsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(offset: Option<usize>, limit: Option<usize>) -> ReadFileRequest {
        ReadFileRequest {
            path: WorkspacePath::parse("a.txt").unwrap(),
            offset,
            limit,
        }
    }

    #[test]
    fn read_request_defaults_to_whole_file_start() {
        let req: ReadFileRequest = serde_json::from_str(r#"{"path":"a.txt"}"#).unwrap();
        assert_eq!(req, request(None, None));
        assert_eq!((req.start_line(), req.line_limit()), (1, 2000));
        assert!(!req.has_explicit_range());
        assert!(request(Some(0), None).has_explicit_range());
        assert_eq!(request(Some(0), None).start_line(), 1);
    }

    #[test]
    fn content_slices_requested_lines() {
        let text = "one\r\ntwo\nthree\nfour\n";
        let content = FileContent::from_text(text, &request(Some(2), Some(2)));
        assert_eq!(content.lines, ["two", "three"]);
        assert_eq!((content.start_line, content.total_lines), (2, 4));
        assert!(content.truncated);
        assert_eq!(content.next_offset(), 4);

        let tail = FileContent::from_text(text, &request(Some(3), None));
        assert!(!tail.truncated);
        let past_end = FileContent::from_text(text, &request(Some(9), None));
        assert!(past_end.lines.is_empty() && !past_end.truncated);
    }

    #[test]
    fn renders_numbered_lines_with_continuation_hint() {
        let content = FileContent::from_text("a\nb\nc\n", &request(Some(2), Some(1)));
        assert_eq!(
            content.render_numbered(),
            "     2\tb\n[showing lines 2-2 of 3; continue with offset=3]\n"
        );
    }

    #[test]
    fn replace_unique_requires_a_single_match() {
        assert_eq!(replace_unique("a b c", "b", "x"), Ok("a x c".to_owned()));
        assert_eq!(replace_unique("a b c", "", "x"), Err(FsError::EmptyOld));
        assert_eq!(replace_unique("a b c", "z", "x"), Err(FsError::OldNotFound));
        assert_eq!(
            replace_unique("aaa", "aa", "x"),
            Ok("xa".to_owned()),
            "occurrences are counted without overlap"
        );
        assert_eq!(
            replace_unique("b b", "b", "x"),
            Err(FsError::OldNotUnique { count: 2 })
        );
    }

    #[test]
    fn pending_change_reports_creation() {
        let path = WorkspacePath::parse("new.txt").unwrap();
        let pending = PendingChange::new(path.clone(), None, "hi\n".into(), "+hi\n".into());
        assert!(pending.creates());
        assert_eq!(
            pending.to_change(),
            FileChange {
                path,
                diff_unified: "+hi\n".into(),
                created: true
            }
        );
    }

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn FileSystemPort>>) {}
        assert_dyn(None);
    }
}
