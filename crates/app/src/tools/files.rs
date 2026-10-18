//! The file tools of the model: search (`list_files`, `find_file`, `grep`,
//! `tree`), reading (`read_file`), changes (`edit_file`, `write_file`) and
//! commands (`run_command`), each a thin [`TypedTool`] over its port.
//!
//! Inputs are the port requests themselves, so their `schemars` schema
//! (doc comments and defaults included) is what the model sees. Outputs are
//! compact text cut to the registry's [`ResultBudget`]; port failures come
//! back as error outputs with a hint on how to recover, never as aborts.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::permission::ToolEffect;
use domain::workspace::PathError;
use futures::FutureExt;

use super::budget::ResultBudget;
use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::ports::fs::{FileContent, FileSystemPort, FsError, ReadFileRequest};
use crate::ports::search::{
    ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepHit, GrepRequest,
    ListFilesRequest, SearchError, TreeNode, TreeNodeKind, TreeRequest,
};

/// Tokens kept free below the registry budget for the notes the tools add
/// after a cut, so the registry's own truncation never cuts them again.
const NOTE_RESERVE_TOKENS: usize = 64;

/// The budget a tool cuts its own listing to.
fn inner_budget(budget: ResultBudget) -> ResultBudget {
    ResultBudget::new(budget.max_tokens.saturating_sub(NOTE_RESERVE_TOKENS))
}

/// The message the model gets for a path outside the workspace.
fn path_error_message(err: &PathError) -> String {
    match err {
        PathError::OutsideWorkspace { path } => format!(
            "PathOutsideWorkspace: `{path}` resolves outside the workspace; \
             use a path relative to the workspace root"
        ),
        other => other.to_string(),
    }
}

fn search_error_message(err: &SearchError) -> String {
    match err {
        SearchError::Path(err) => path_error_message(err),
        SearchError::InvalidPattern(detail) => format!(
            "invalid pattern: {detail}. Escape regex metacharacters or set `fixed` to true \
             for a literal search"
        ),
        other => other.to_string(),
    }
}

/// Renders a list of paths, cut to `budget`, with a note when the port
/// already cut it to the request limit.
fn render_file_list(list: &FileList, budget: ResultBudget, empty: &str, hint: &str) -> String {
    if list.paths.is_empty() {
        return empty.to_owned();
    }
    let paths: Vec<&str> = list.paths.iter().map(|p| p.as_str()).collect();
    let mut out = inner_budget(budget).truncate_list(&paths);
    if list.truncated {
        let _ = write!(
            out,
            "\n[showing {} of {} files; {hint}]",
            list.paths.len(),
            list.total
        );
    }
    out
}

const LIST_FILES_DESCRIPTION: &str = "\
Lists files in the workspace, one path per line, sorted by path. Hidden files are included; \
files ignored by .gitignore and the .git directory are not.
- Use it to see what exists, optionally narrowed with `glob` (e.g. `**/*.rs`, `src/**/mod.rs`).
- To find a file whose name you roughly know, prefer `find_file`; to see the layout of \
directories, prefer `tree`.
- Returns at most `limit` paths (default 500); narrow `glob` when the list is cut.
- Paths are relative to the workspace root.";

/// `list_files`: workspace files, optionally filtered by a glob.
pub struct ListFilesTool {
    search: Arc<dyn FileSearchPort>,
    budget: ResultBudget,
}

impl ListFilesTool {
    /// The tool over `search`, cutting results to `budget`.
    #[must_use]
    pub fn new(search: Arc<dyn FileSearchPort>, budget: ResultBudget) -> Self {
        Self { search, budget }
    }
}

#[async_trait]
impl TypedTool for ListFilesTool {
    type Input = ListFilesRequest;

    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        LIST_FILES_DESCRIPTION
    }

    fn effect(&self, _input: &ListFilesRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ListFilesRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input.glob.clone().unwrap_or_else(|| ".".to_owned());
        let search = Arc::clone(&self.search);
        let budget = self.budget;
        let run = async move {
            Ok(match search.list_files(input).await {
                Ok(list) => ToolOutput::text(render_file_list(
                    &list,
                    budget,
                    "No files found.",
                    "narrow the search with `glob`",
                )),
                Err(err) => ToolOutput::error(search_error_message(&err)),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

const FIND_FILE_DESCRIPTION: &str = "\
Finds files by fuzzy-matching `query` against their paths (fzf), best match first, one path \
per line.
- Use it when you know part of a file name or path, e.g. `run_turn` or `app/tools/reg`.
- To search file contents use `grep`; to list files by pattern use `list_files` with a glob.
- Returns at most `limit` paths (default 20). Paths are relative to the workspace root.";

/// `find_file`: fuzzy search of file paths.
pub struct FindFileTool {
    search: Arc<dyn FileSearchPort>,
    budget: ResultBudget,
}

impl FindFileTool {
    /// The tool over `search`, cutting results to `budget`.
    #[must_use]
    pub fn new(search: Arc<dyn FileSearchPort>, budget: ResultBudget) -> Self {
        Self { search, budget }
    }
}

#[async_trait]
impl TypedTool for FindFileTool {
    type Input = FindFileRequest;

    fn name(&self) -> &str {
        "find_file"
    }

    fn description(&self) -> &str {
        FIND_FILE_DESCRIPTION
    }

    fn effect(&self, _input: &FindFileRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: FindFileRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input.query.clone();
        let search = Arc::clone(&self.search);
        let budget = self.budget;
        let run = async move {
            Ok(match search.find_file(input).await {
                Ok(list) => ToolOutput::text(render_file_list(
                    &list,
                    budget,
                    "No files match the query.",
                    "make the query more specific",
                )),
                Err(err) => ToolOutput::error(search_error_message(&err)),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

const GREP_DESCRIPTION: &str = "\
Searches file contents with ripgrep. Each match is printed as `path:line: text`, surrounded by \
its context lines as `path-line- text`; groups of lines are separated by `--`.
- `pattern` is a regular expression in ripgrep (Rust regex) syntax; set `fixed` to true to \
search for a literal string, and `ignore_case` for a case-insensitive search.
- Narrow the search with `path` (a file or directory) and `glob` (e.g. `*.rs`).
- Files ignored by .gitignore are skipped. Paths are relative to the workspace root.
- Returns at most `limit` matches (default 100) with `context` lines around each (default 2; \
use 0 for just the matching lines). Refine the pattern when the result is cut.";

fn render_hit(hit: &GrepHit) -> String {
    let path = hit.path.as_str();
    let first = hit
        .line_number
        .saturating_sub(hit.context_before.len() as u64);
    let mut out = String::new();
    for (offset, line) in hit.context_before.iter().enumerate() {
        let _ = writeln!(out, "{path}-{}- {line}", first + offset as u64);
    }
    let _ = write!(out, "{path}:{}: {}", hit.line_number, hit.line);
    for (offset, line) in hit.context_after.iter().enumerate() {
        let _ = write!(
            out,
            "\n{path}-{}- {line}",
            hit.line_number + 1 + offset as u64
        );
    }
    out
}

/// `grep`: regex or literal search of file contents.
pub struct GrepTool {
    search: Arc<dyn ContentSearchPort>,
    budget: ResultBudget,
}

impl GrepTool {
    /// The tool over `search`, cutting results to `budget`.
    #[must_use]
    pub fn new(search: Arc<dyn ContentSearchPort>, budget: ResultBudget) -> Self {
        Self { search, budget }
    }
}

#[async_trait]
impl TypedTool for GrepTool {
    type Input = GrepRequest;

    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        GREP_DESCRIPTION
    }

    fn effect(&self, _input: &GrepRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: GrepRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = match &input.path {
            Some(path) => format!("{} in {path}", input.pattern),
            None => input.pattern.clone(),
        };
        let search = Arc::clone(&self.search);
        let budget = self.budget;
        let run = async move {
            let with_context = input.context > 0;
            let result = match search.grep(input).await {
                Ok(result) => result,
                Err(err) => return Ok(ToolOutput::error(search_error_message(&err))),
            };
            if result.hits.is_empty() {
                return Ok(ToolOutput::text("No matches found."));
            }
            let items: Vec<String> = result
                .hits
                .iter()
                .enumerate()
                .map(|(index, hit)| {
                    let rendered = render_hit(hit);
                    if with_context && index > 0 {
                        format!("--\n{rendered}")
                    } else {
                        rendered
                    }
                })
                .collect();
            let mut out = inner_budget(budget).truncate_list(&items);
            if result.truncated {
                let _ = write!(
                    out,
                    "\n[showing {} of at least {} matches; refine the pattern, `path` or `glob`]",
                    result.hits.len(),
                    result.total
                );
            }
            Ok(ToolOutput::text(out))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

const TREE_DESCRIPTION: &str = "\
Shows the directory tree under `path` (the workspace root when omitted), built from the \
file listing: directories first and marked with a trailing `/`, two spaces of indentation \
per level.
- Use it to get an overview of a project or a directory before reading files.
- Expands `depth` levels below `path` (default 3); deeper directories show how many files \
they contain instead. Empty and ignored directories are not shown.
- Paths are relative to the workspace root.";

fn render_tree(node: &TreeNode, level: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(level);
    let name = if level == 0 {
        node.path.as_str()
    } else {
        node.name.as_str()
    };
    let line = match node.kind {
        TreeNodeKind::File => format!("{indent}{name}"),
        TreeNodeKind::Dir if node.omitted_files > 0 => {
            format!("{indent}{name}/ ({} files)", node.omitted_files)
        }
        TreeNodeKind::Dir => format!("{indent}{name}/"),
    };
    lines.push(line);
    for child in &node.children {
        render_tree(child, level + 1, lines);
    }
}

/// `tree`: directory layout built from the file listing.
pub struct TreeTool {
    search: Arc<dyn FileSearchPort>,
    budget: ResultBudget,
}

impl TreeTool {
    /// The tool over `search`, cutting results to `budget`.
    #[must_use]
    pub fn new(search: Arc<dyn FileSearchPort>, budget: ResultBudget) -> Self {
        Self { search, budget }
    }
}

#[async_trait]
impl TypedTool for TreeTool {
    type Input = TreeRequest;

    fn name(&self) -> &str {
        "tree"
    }

    fn description(&self) -> &str {
        TREE_DESCRIPTION
    }

    fn effect(&self, _input: &TreeRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: TreeRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input
            .path
            .as_ref()
            .map_or_else(|| ".".to_owned(), ToString::to_string);
        let search = Arc::clone(&self.search);
        let budget = self.budget;
        let run = async move {
            Ok(match search.tree(input).await {
                Ok(tree) if tree.children.is_empty() && tree.omitted_files == 0 => {
                    ToolOutput::text(format!("No files under {}.", tree.path))
                }
                Ok(tree) => {
                    let mut lines = Vec::new();
                    render_tree(&tree, 0, &mut lines);
                    ToolOutput::text(inner_budget(budget).truncate_list(&lines))
                }
                Err(err) => ToolOutput::error(search_error_message(&err)),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// The message the model gets for a failed file operation.
fn fs_error_message(err: &FsError) -> String {
    match err {
        FsError::Path(err) => path_error_message(err),
        FsError::NotFound { path } => {
            format!("file not found: {path}. Check the path with `find_file` or `list_files`")
        }
        FsError::OldNotFound => "`old` was not found in the file; read the file again and copy \
             the exact text, including whitespace and indentation"
            .to_owned(),
        FsError::OldNotUnique { count } => format!(
            "`old` appears {count} times in the file; include more surrounding context so it \
             matches exactly once"
        ),
        other => other.to_string(),
    }
}

const READ_FILE_DESCRIPTION: &str = "\
Reads a text file of the workspace. Lines are returned numbered as `cat -n` does (line \
number, a tab, the line); the numbers are not part of the file.
- `path` is relative to the workspace root.
- Returns up to 2000 lines by default. For long files pass `offset` (1-based first line) and \
`limit` (number of lines) to read a range; a cut result ends with the offset to continue.
- Binary files are rejected, and files over 2 MB can only be read with an explicit range.
- Read a file before editing it, so `edit_file` gets the exact current text.";

/// `read_file`: a numbered slice of a text file.
pub struct ReadFileTool {
    fs: Arc<dyn FileSystemPort>,
    budget: ResultBudget,
}

impl ReadFileTool {
    /// The tool over `fs`, cutting results to `budget`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystemPort>, budget: ResultBudget) -> Self {
        Self { fs, budget }
    }
}

fn render_content(content: &FileContent, budget: ResultBudget) -> String {
    if content.lines.is_empty() {
        return if content.total_lines == 0 {
            format!("[{} is empty]", content.path)
        } else {
            format!(
                "[offset {} is past the end of {} ({} lines)]",
                content.start_line, content.path, content.total_lines
            )
        };
    }
    inner_budget(budget).truncate_text(content.render_numbered().trim_end_matches('\n'))
}

#[async_trait]
impl TypedTool for ReadFileTool {
    type Input = ReadFileRequest;

    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        READ_FILE_DESCRIPTION
    }

    fn effect(&self, _input: &ReadFileRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ReadFileRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input.path.to_string();
        let fs = Arc::clone(&self.fs);
        let budget = self.budget;
        let run = async move {
            Ok(match fs.read_file(input).await {
                Ok(content) => ToolOutput::text(render_content(&content, budget)),
                Err(err) => ToolOutput::error(fs_error_message(&err)),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use domain::workspace::WorkspaceRoot;
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::fs::InMemoryFs;
    use crate::tools::registry::{Tool, Typed};

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn workspace() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                .with_file("Cargo.toml", "[workspace]\n")
                .with_file("src/lib.rs", "pub mod tools;\n\nfn main() {}\n")
                .with_file("src/tools/mod.rs", "// tools\npub mod files;\n")
                .with_file("src/tools/files.rs", "fn grep() {}\nfn tree() {}\n")
                .with_file("docs/a/b/c.md", "# deep\n"),
        )
    }

    async fn call(tool: &dyn Tool, input: Value) -> (String, ToolOutput) {
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        let subject = prepared.subject.clone();
        (subject, prepared.run.await.unwrap())
    }

    fn budget() -> ResultBudget {
        ResultBudget::default()
    }

    #[tokio::test]
    async fn list_files_prints_one_path_per_line() {
        let tool = Typed(ListFilesTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "glob": "*.rs" })).await;
        assert_eq!(subject, "*.rs");
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "src/lib.rs\nsrc/tools/files.rs\nsrc/tools/mod.rs"
        );
        assert_eq!(tool.effect(&json!({})), ToolEffect::Read);
    }

    #[tokio::test]
    async fn list_files_notes_the_limit() {
        let tool = Typed(ListFilesTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "limit": 2 })).await;
        assert_eq!(subject, ".");
        assert_eq!(
            out.text_content(),
            "Cargo.toml\ndocs/a/b/c.md\n\
             [showing 2 of 5 files; narrow the search with `glob`]"
        );
    }

    #[tokio::test]
    async fn list_files_cuts_whole_paths_to_the_budget() {
        let tool = Typed(ListFilesTool::new(
            workspace(),
            ResultBudget::new(NOTE_RESERVE_TOKENS + 6),
        ));
        let (_, out) = call(&tool, json!({})).await;
        assert_eq!(
            out.text_content(),
            "Cargo.toml\ndocs/a/b/c.md\n3 resultados más; refina la búsqueda"
        );
    }

    #[tokio::test]
    async fn find_file_ranks_matches() {
        let tool = Typed(FindFileTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "query": "files" })).await;
        assert_eq!(subject, "files");
        assert_eq!(out.text_content(), "src/tools/files.rs");
        let (_, out) = call(&tool, json!({ "query": "nope" })).await;
        assert_eq!(out.text_content(), "No files match the query.");
    }

    #[tokio::test]
    async fn grep_prints_matches_with_context() {
        let tool = Typed(GrepTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "pattern": "pub mod", "context": 1 })).await;
        assert_eq!(subject, "pub mod");
        assert_eq!(
            out.text_content(),
            "src/lib.rs:1: pub mod tools;\n\
             src/lib.rs-2- \n\
             --\n\
             src/tools/mod.rs-1- // tools\n\
             src/tools/mod.rs:2: pub mod files;"
        );
    }

    #[tokio::test]
    async fn grep_notes_the_limit_and_scopes_the_subject() {
        let tool = Typed(GrepTool::new(workspace(), budget()));
        let input = json!({ "pattern": "fn", "path": "src/tools", "context": 0, "limit": 1 });
        let (subject, out) = call(&tool, input).await;
        assert_eq!(subject, "fn in src/tools");
        assert_eq!(
            out.text_content(),
            "src/tools/files.rs:1: fn grep() {}\n\
             [showing 1 of at least 2 matches; refine the pattern, `path` or `glob`]"
        );
    }

    #[tokio::test]
    async fn grep_reports_an_invalid_pattern() {
        let tool = Typed(GrepTool::new(workspace(), budget()));
        let (_, out) = call(&tool, json!({ "pattern": "fn(" })).await;
        assert!(out.is_error);
        assert!(
            out.text_content().starts_with("invalid pattern:"),
            "{}",
            out.text_content()
        );
        assert!(out.text_content().contains("`fixed`"));
    }

    #[tokio::test]
    async fn tree_indents_and_counts_cut_directories() {
        let tool = Typed(TreeTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "depth": 2 })).await;
        assert_eq!(subject, ".");
        assert_eq!(
            out.text_content(),
            "./\n  docs/\n    a/ (1 files)\n  src/\n    tools/ (2 files)\n    lib.rs\n  Cargo.toml"
        );
    }

    #[tokio::test]
    async fn tree_rejects_a_path_outside_the_workspace() {
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                .with_file("link/x", "")
                .with_escaping_link("link"),
        );
        let tool = Typed(TreeTool::new(fs, budget()));
        let (subject, out) = call(&tool, json!({ "path": "link" })).await;
        assert_eq!(subject, "link");
        assert!(out.is_error);
        assert!(
            out.text_content()
                .starts_with("PathOutsideWorkspace: `link`"),
            "{}",
            out.text_content()
        );
    }

    #[tokio::test]
    async fn read_file_numbers_lines_and_supports_ranges() {
        let tool = Typed(ReadFileTool::new(workspace(), budget()));
        let (subject, out) = call(&tool, json!({ "path": "src/lib.rs" })).await;
        assert_eq!(subject, "src/lib.rs");
        assert_eq!(
            out.text_content(),
            "     1\tpub mod tools;\n     2\t\n     3\tfn main() {}"
        );
        let input = json!({ "path": "src/lib.rs", "offset": 2, "limit": 1 });
        let (_, out) = call(&tool, input).await;
        assert_eq!(
            out.text_content(),
            "     2\t\n[showing lines 2-2 of 3; continue with offset=3]"
        );
        let (_, out) = call(&tool, json!({ "path": "src/lib.rs", "offset": 9 })).await;
        assert_eq!(
            out.text_content(),
            "[offset 9 is past the end of src/lib.rs (3 lines)]"
        );
    }

    #[tokio::test]
    async fn read_file_reports_missing_and_sensitive_files() {
        let fs = workspace();
        fs.set_file(".env", "SECRET=1\n");
        let tool = Typed(ReadFileTool::new(fs, budget()));
        let (_, out) = call(&tool, json!({ "path": "nope.rs" })).await;
        assert!(out.is_error);
        assert!(out.text_content().starts_with("file not found: nope.rs."));
        let (_, out) = call(&tool, json!({ "path": ".env" })).await;
        assert!(out.is_error);
        assert!(
            out.text_content().contains("denied"),
            "{}",
            out.text_content()
        );
    }

    #[tokio::test]
    async fn read_file_rejects_paths_outside_the_workspace_as_input() {
        let tool = Typed(ReadFileTool::new(workspace(), budget()));
        let err = tool
            .prepare(json!({ "path": "../etc/passwd" }), &ctx())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ToolError::InvalidInput { message, .. } if message.contains("outside the workspace")),
            "{err:?}"
        );
    }
}
