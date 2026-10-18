//! File and content search ports, implemented with `rg` and `fzf`.
//!
//! Requests double as the JSON input of the model's search tools: their
//! `JsonSchema` (doc comments included) becomes the tool schema, and missing
//! fields take the documented defaults through `#[serde(default)]`.
//!
//! Cancellation is done by dropping the returned future. Adapters that spawn
//! processes must kill them when the future is dropped (for example with
//! `kill_on_drop(true)`), and must enforce [`DEFAULT_SEARCH_TIMEOUT`] (or a
//! configured value) on every search, failing with [`SearchError::Timeout`].

use std::time::Duration;

use async_trait::async_trait;
use domain::workspace::{PathError, WorkspacePath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Timeout that adapters apply to each search unless configured otherwise.
pub const DEFAULT_SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Default of [`ListFilesRequest::limit`].
pub const DEFAULT_LIST_LIMIT: usize = 500;
/// Default of [`FindFileRequest::limit`].
pub const DEFAULT_FIND_LIMIT: usize = 20;
/// Default of [`TreeRequest::depth`].
pub const DEFAULT_TREE_DEPTH: usize = 3;

fn default_list_limit() -> usize {
    DEFAULT_LIST_LIMIT
}

fn default_find_limit() -> usize {
    DEFAULT_FIND_LIMIT
}

fn default_tree_depth() -> usize {
    DEFAULT_TREE_DEPTH
}

/// Errors of [`FileSearchPort`] and [`ContentSearchPort`].
///
/// I/O failures are carried as [`SearchError::Io`] with the rendered message,
/// so the error stays `Clone + PartialEq` (`std::io::Error` is neither).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    /// An external tool (`rg`, `fzf`) is not installed or not runnable.
    #[error("required tool `{tool}` is missing: {hint}")]
    ToolMissing {
        /// Name of the missing executable.
        tool: String,
        /// How to install it, shown to the user.
        hint: String,
    },
    /// The search did not finish within its timeout.
    #[error("search timed out after {after:?}")]
    Timeout {
        /// The timeout that elapsed.
        after: Duration,
    },
    /// The search process was terminated from outside (e.g. by a signal)
    /// before producing a result. Dropping the future does not produce this
    /// error: a dropped future never resolves.
    #[error("search was cancelled")]
    Cancelled,
    /// The pattern or glob was rejected by the search tool.
    #[error("invalid pattern: {0}")]
    InvalidPattern(String),
    /// A path in the request or in the tool output is not valid inside the
    /// workspace.
    #[error(transparent)]
    Path(#[from] PathError),
    /// Spawning the tool or reading its output failed.
    #[error("search I/O error: {0}")]
    Io(String),
    /// The tool output could not be parsed.
    #[error("could not parse search output: {0}")]
    Parse(String),
}

/// Input of the `list_files` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListFilesRequest {
    /// Optional glob that listed paths must match, e.g. `**/*.rs`.
    #[serde(default)]
    pub glob: Option<String>,
    /// Maximum number of paths to return (default 500).
    #[serde(default = "default_list_limit")]
    pub limit: usize,
}

impl Default for ListFilesRequest {
    fn default() -> Self {
        Self {
            glob: None,
            limit: DEFAULT_LIST_LIMIT,
        }
    }
}

/// Input of the `find_file` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindFileRequest {
    /// Fuzzy query matched against file paths.
    pub query: String,
    /// Maximum number of paths to return (default 20).
    #[serde(default = "default_find_limit")]
    pub limit: usize,
}

impl FindFileRequest {
    /// A request for `query` with the default limit.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: DEFAULT_FIND_LIMIT,
        }
    }
}

/// Input of the `tree` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TreeRequest {
    /// Directory to start from; the workspace root when omitted.
    #[serde(default)]
    pub path: Option<WorkspacePath>,
    /// How many directory levels to expand below `path` (default 3).
    #[serde(default = "default_tree_depth")]
    pub depth: usize,
}

impl Default for TreeRequest {
    fn default() -> Self {
        Self {
            path: None,
            depth: DEFAULT_TREE_DEPTH,
        }
    }
}

/// Paths found by `list_files` or `find_file`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileList {
    /// The returned paths: sorted for `list_files`, best match first for
    /// `find_file`.
    pub paths: Vec<WorkspacePath>,
    /// Whether `total` exceeds the number of returned paths.
    pub truncated: bool,
    /// How many paths matched before applying the limit.
    pub total: usize,
}

impl FileList {
    /// Keeps the first `limit` of `all` matching paths, recording the total.
    pub fn from_matches(all: Vec<WorkspacePath>, limit: usize) -> Self {
        let total = all.len();
        let mut paths = all;
        paths.truncate(limit);
        Self {
            truncated: total > paths.len(),
            paths,
            total,
        }
    }
}

/// Whether a [`TreeNode`] is a file or a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeNodeKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
}

/// A node of the tree returned by the `tree` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    /// Last component of the path (`.` for the workspace root).
    pub name: String,
    /// Full path of the node inside the workspace.
    pub path: WorkspacePath,
    /// File or directory.
    pub kind: TreeNodeKind,
    /// Children sorted with directories first, then by name. Empty for
    /// files and for directories cut by the depth limit.
    pub children: Vec<TreeNode>,
    /// Number of files below this directory that were not expanded because
    /// of the depth limit (0 for files and fully expanded directories).
    pub omitted_files: usize,
}

/// Finds files by name. Implemented with `rg --files` and `fzf --filter`;
/// also used by the UI file picker so the user sees the agent's results.
///
/// All paths are relative to the workspace root, and `.git` is excluded.
#[async_trait]
pub trait FileSearchPort: Send + Sync {
    /// Lists workspace files (including hidden ones, honoring ignore files),
    /// optionally filtered by `request.glob`, sorted by path and cut to
    /// `request.limit`.
    async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError>;

    /// Fuzzy-finds files whose path matches `request.query`, best match
    /// first, cut to `request.limit`. When `fzf` is missing, adapters fall
    /// back to case-insensitive substring matching instead of failing.
    async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError>;

    /// Returns the directory tree under `request.path` (the root when
    /// `None`) expanded `request.depth` levels, built from the file listing.
    async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> WorkspacePath {
        WorkspacePath::parse(s).unwrap()
    }

    #[test]
    fn requests_take_documented_defaults() {
        let list: ListFilesRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(list, ListFilesRequest::default());
        assert_eq!(list.limit, 500);

        let find: FindFileRequest = serde_json::from_str(r#"{"query":"main"}"#).unwrap();
        assert_eq!(find, FindFileRequest::new("main"));
        assert_eq!(find.limit, 20);

        let tree: TreeRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(tree, TreeRequest::default());
        assert_eq!(tree.depth, 3);
    }

    #[test]
    fn tree_request_rejects_paths_outside_workspace() {
        let result = serde_json::from_str::<TreeRequest>(r#"{"path":"../etc"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn file_list_records_truncation() {
        let list = FileList::from_matches(vec![p("a"), p("b"), p("c")], 2);
        assert_eq!(list.paths, vec![p("a"), p("b")]);
        assert!(list.truncated);
        assert_eq!(list.total, 3);

        let all = FileList::from_matches(vec![p("a")], 2);
        assert!(!all.truncated);
    }
}
