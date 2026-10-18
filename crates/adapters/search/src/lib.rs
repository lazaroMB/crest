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

use application::ports::search::{
    ContentSearchPort, DEFAULT_SEARCH_TIMEOUT, FileList, FileSearchPort, FindFileRequest,
    GrepRequest, GrepResult, ListFilesRequest, SearchError, TreeNode, TreeRequest,
};
use async_trait::async_trait;
use domain::workspace::{SensitivePathRules, WorkspaceRoot};

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
    root: WorkspaceRoot,
    rg: OsString,
    fzf: OsString,
    timeout: Duration,
    rules: SensitivePathRules,
    /// `fzf --version`, checked on the first `find_file` and shared by
    /// clones.
    fzf_status: Arc<OnceCell<ToolStatus>>,
}

impl RipgrepSearch {
    /// An adapter for `root` (already canonicalized) using `rg` and `fzf`
    /// from `PATH`, [`DEFAULT_SEARCH_TIMEOUT`] and the default sensitive
    /// path rules.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            rg: Tool::Ripgrep.program().into(),
            fzf: Tool::Fzf.program().into(),
            timeout: DEFAULT_SEARCH_TIMEOUT,
            rules: SensitivePathRules::default(),
            fzf_status: Arc::default(),
        }
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

    /// The workspace root.
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    /// Checks the configured `rg` and `fzf` (meant for startup; see
    /// [`ToolReport::problems`]).
    pub async fn check_tools(&self) -> ToolReport {
        ToolReport {
            rg: detect::detect(Tool::Ripgrep, &self.rg).await,
            fzf: detect::detect(Tool::Fzf, &self.fzf).await,
        }
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
