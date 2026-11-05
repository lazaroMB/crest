//! FileSearchPort and ContentSearchPort adapter backed by rg and fzf.
//!
//! [`RipgrepSearch`] runs `rg` (and `fzf` for `find_file`) as external
//! processes, without a shell and with the workspace root as working
//! directory (see [`process`]).
//!
//! # Sensitive paths
//!
//! Paths denied by the configured [`SensitivePathRules`] (by default
//! `.env*`, `*.pem`, `id_rsa*` and `.git/`) never appear in any result:
//! not in `list_files`, `find_file` or `tree`, and `grep` never returns
//! their contents. Each rule is passed to `rg` as an exclusion glob, so the
//! files are not even read, and every path in the output is checked again
//! with [`SensitivePathRules::is_denied`].
//!
//! # Ignore files
//!
//! Listings honor `.gitignore`, `.ignore` and `.rgignore` whether or not
//! the workspace is a git repository (`rg --no-require-git`): a project
//! that is not (yet) under git still keeps `target/` or `node_modules/` out
//! of the model's context. Hidden files are listed; `.git` never is.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use application_core::ports::search::{
    ContentSearchPort, DEFAULT_SEARCH_TIMEOUT, FileList, FileSearchPort, FindFileRequest,
    GrepRequest, GrepResult, ListFilesRequest, SearchError, TreeNode, TreeRequest,
};
use async_trait::async_trait;
use domain::workspace::{PathError, SensitivePathRules, WorkspaceRoot};

use tokio::sync::OnceCell;

use crate::detect::{Tool, ToolReport, ToolStatus};

pub mod detect;
pub mod files;
pub mod find;
pub mod grep;
pub mod process;

#[cfg(test)]
mod test_support;

/// Search adapter backed by `rg` and `fzf`.
#[derive(Debug, Clone)]
pub struct RipgrepSearch {
    /// `None` for a chat without a directory, where every search fails with
    /// [`PathError::NoRoots`].
    root: Option<WorkspaceRoot>,
    rg: OsString,
    fzf: OsString,
    timeout: Duration,
    rules: SensitivePathRules,
    /// `fzf --version`, checked on the first `find_file` and shared by
    /// clones.
    fzf_status: Arc<OnceCell<ToolStatus>>,
}

/// An adapter over `root` with the default tools, timeout and rules.
fn open(root: Option<WorkspaceRoot>) -> RipgrepSearch {
    RipgrepSearch {
        root,
        rg: Tool::Ripgrep.program().into(),
        fzf: Tool::Fzf.program().into(),
        timeout: DEFAULT_SEARCH_TIMEOUT,
        rules: SensitivePathRules::default(),
        fzf_status: Arc::default(),
    }
}

/// The directory searches run in: the first root of the session. Searches
/// stay in it even when the session has more.
pub(crate) fn directory(search: &RipgrepSearch) -> Result<&WorkspaceRoot, PathError> {
    search.root.as_ref().ok_or(PathError::NoRoots)
}

impl RipgrepSearch {
    /// An adapter for `root` (already canonicalized) using `rg` and `fzf`
    /// from `PATH`, [`DEFAULT_SEARCH_TIMEOUT`] and the default sensitive
    /// path rules.
    pub fn new(root: WorkspaceRoot) -> Self {
        open(Some(root))
    }

    /// Uses `program` (a name looked up in `PATH`, or a path) as `rg`.
    #[must_use]
    pub fn with_rg_program(mut self, program: impl Into<OsString>) -> Self {
        self.rg = program.into();
        self
    }

    /// Uses `program` (a name looked up in `PATH`, or a path) as `fzf`.
    #[must_use]
    pub fn with_fzf_program(mut self, program: impl Into<OsString>) -> Self {
        self.fzf = program.into();
        self.fzf_status = Arc::default();
        self
    }

    /// Sets the timeout applied to each search.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Replaces the sensitive path rules filtered out of every result.
    #[must_use]
    pub fn with_sensitive_rules(mut self, rules: SensitivePathRules) -> Self {
        self.rules = rules;
        self
    }

    /// An adapter without a directory (a standalone chat): every search
    /// fails with [`PathError::NoRoots`], and nothing is searched in the
    /// process's own working directory.
    pub fn standalone() -> Self {
        open(None)
    }

    /// Checks the configured `rg` and `fzf` (meant for startup; see
    /// [`ToolReport::problems`]).
    pub async fn check_tools(&self) -> ToolReport {
        let (rg, fzf) = tokio::join!(
            detect::detect(Tool::Ripgrep, &self.rg),
            detect::detect(Tool::Fzf, &self.fzf)
        );
        ToolReport { rg, fzf }
    }
}

#[async_trait]
impl FileSearchPort for RipgrepSearch {
    async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError> {
        files::list_files(self, request).await
    }

    async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError> {
        find::find_file(self, request).await
    }

    async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
        files::tree(self, request).await
    }
}

#[async_trait]
impl ContentSearchPort for RipgrepSearch {
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, SearchError> {
        grep::grep(self, request).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_support::Workspace;

    /// No directory, no search: nothing is searched in crest's own directory.
    #[tokio::test]
    async fn a_standalone_search_finds_nothing_and_asks_for_a_directory() {
        let search = RipgrepSearch::standalone();
        let expected = SearchError::Path(PathError::NoRoots);
        assert_eq!(
            search.grep(GrepRequest::new("needle")).await,
            Err(expected.clone())
        );
        assert_eq!(
            search.list_files(ListFilesRequest::default()).await,
            Err(expected.clone())
        );
        assert_eq!(
            search.find_file(FindFileRequest::new("a")).await,
            Err(expected)
        );
    }

    #[tokio::test]
    async fn greps_through_the_port() {
        let ws = Workspace::new();
        ws.write("a.txt", "needle\n");
        let port: Arc<dyn ContentSearchPort> = Arc::new(ws.search());
        let result = port.grep(GrepRequest::new("needle")).await.unwrap();
        assert_eq!(result.hits.len(), 1);
    }

    #[tokio::test]
    async fn lists_and_finds_through_the_port() {
        let ws = Workspace::new();
        ws.write("src/main.rs", "").write(".env", "");
        let port: Arc<dyn FileSearchPort> = Arc::new(ws.search());
        let all = port.list_files(ListFilesRequest::default()).await.unwrap();
        assert_eq!(all.total, 1);
        let found = port.find_file(FindFileRequest::new("main")).await.unwrap();
        assert_eq!(found.paths, all.paths);
        let tree = port.tree(TreeRequest::default()).await.unwrap();
        assert_eq!(tree.children.len(), 1);
    }
}
