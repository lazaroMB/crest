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

    /// Whether `path` exists (as a file or directory) inside the workspace.
    /// Paths that resolve outside the root fail instead of returning false.
    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError>;
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
}
