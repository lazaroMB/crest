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
use domain::block::{BlockKind, DiffSpec};
use domain::permission::ToolEffect;
use domain::workspace::PathError;
use futures::FutureExt;

use super::budget::ResultBudget;
use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::ports::approval::ApprovalPreview;
use crate::ports::fs::{
    EditFileRequest, FileContent, FileSystemPort, FsError, PendingChange, ReadFileRequest,
    WriteFileRequest,
};
use crate::ports::search::{
    ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepHit, GrepRequest,
    ListFilesRequest, SearchError, TreeNode, TreeNodeKind, TreeRequest,
};
use crate::ports::shell::{CommandOutput, RunCommandRequest, ShellPort};

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

/// The diff shown for a prepared change, in the approval dialog and in the
/// timeline.
fn diff_spec(change: &PendingChange) -> DiffSpec {
    DiffSpec {
        path: change.path().to_string(),
        old_text: change.original().map(str::to_owned),
        new_text: change.new_content().to_owned(),
        unified: change.diff_unified().to_owned(),
        approval: None,
    }
}

/// A call that writes `change` once approved, previewed as its diff.
fn change_call(fs: Arc<dyn FileSystemPort>, change: PendingChange) -> PreparedCall {
    let spec = diff_spec(&change);
    let subject = change.path().to_string();
    let lines = change.new_content().lines().count();
    let run = async move {
        Ok(match fs.apply(change).await {
            Ok(applied) if applied.created => {
                ToolOutput::text(format!("Created {} ({lines} lines).", applied.path))
            }
            Ok(applied) => ToolOutput::text(format!("Updated {} ({lines} lines).", applied.path)),
            Err(err) => ToolOutput::error(fs_error_message(&err)),
        })
    };
    PreparedCall::new(subject, run.boxed())
        .with_preview(ApprovalPreview::Diff(spec.clone()))
        .with_block(BlockKind::Diff(spec))
}

fn prepare_error(err: &FsError) -> ToolError {
    ToolError::Execution(fs_error_message(err))
}

const EDIT_FILE_DESCRIPTION: &str = "\
Edits an existing file by replacing one exact occurrence of `old` with `new`.
- `old` must match the current file content exactly, including whitespace and indentation, \
and must appear exactly once: when it appears several times, include more surrounding lines \
to make it unique. Do not include the line numbers that `read_file` prints.
- Read the file first. To create a file or rewrite it entirely, use `write_file`.
- The user sees the diff and may need to approve it before it is written.
- `path` is relative to the workspace root.";

/// `edit_file`: exact, unique replacement in an existing file.
pub struct EditFileTool {
    fs: Arc<dyn FileSystemPort>,
}

impl EditFileTool {
    /// The tool over `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystemPort>) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl TypedTool for EditFileTool {
    type Input = EditFileRequest;

    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        EDIT_FILE_DESCRIPTION
    }

    fn effect(&self, _input: &EditFileRequest) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: EditFileRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let change = self
            .fs
            .prepare_edit(input)
            .await
            .map_err(|err| prepare_error(&err))?;
        Ok(change_call(Arc::clone(&self.fs), change))
    }
}

const WRITE_FILE_DESCRIPTION: &str = "\
Creates a file, or overwrites an existing one, with `content` as its full new content. \
Missing parent directories are created.
- Prefer `edit_file` for changes to an existing file: it is cheaper and safer than rewriting \
it. Read an existing file before overwriting it.
- The user sees the diff and may need to approve it before it is written.
- `path` is relative to the workspace root.";

/// `write_file`: create or overwrite a whole file.
pub struct WriteFileTool {
    fs: Arc<dyn FileSystemPort>,
}

impl WriteFileTool {
    /// The tool over `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystemPort>) -> Self {
        Self { fs }
    }
}

#[async_trait]
impl TypedTool for WriteFileTool {
    type Input = WriteFileRequest;

    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        WRITE_FILE_DESCRIPTION
    }

    fn effect(&self, _input: &WriteFileRequest) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: WriteFileRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let change = self
            .fs
            .prepare_write(input)
            .await
            .map_err(|err| prepare_error(&err))?;
        Ok(change_call(Arc::clone(&self.fs), change))
    }
}

const RUN_COMMAND_DESCRIPTION: &str = "\
Runs a program in the workspace root and returns its exit code, stdout and stderr.
- `command` is the program and `args` its arguments, one per element. No shell is involved: \
pipes, redirections, `&&`, globs and variables are not interpreted.
- Use it to build, test, format or inspect history (`git log`, `git blame`, `git diff`, \
`git status`, `git show` run without approval). Other commands may need the user's approval.
- Prefer `grep`, `find_file`, `list_files`, `tree` and `read_file` over `rg`, `find`, `ls` \
or `cat`, and `edit_file`/`write_file` over commands that modify files.
- The process is killed after `timeout_s` seconds (default 120). Long output is cut; a \
non-zero exit code is reported in the result, not as a failure of the tool.";

/// `run_command`: a program run without a shell in the workspace root.
pub struct RunCommandTool {
    shell: Arc<dyn ShellPort>,
    budget: ResultBudget,
}

impl RunCommandTool {
    /// The tool over `shell`, cutting results to `budget`.
    #[must_use]
    pub fn new(shell: Arc<dyn ShellPort>, budget: ResultBudget) -> Self {
        Self { shell, budget }
    }
}

fn render_command_output(
    request: &RunCommandRequest,
    output: &CommandOutput,
    budget: ResultBudget,
) -> String {
    let mut out = match output.status {
        _ if output.timed_out => format!(
            "timed out after {}s; the process was killed",
            request.timeout_s
        ),
        Some(code) => format!("exit code: {code}"),
        None => "killed by a signal".to_owned(),
    };
    // Each stream gets half of the budget, so stderr is never crowded out.
    let half = ResultBudget::new(inner_budget(budget).max_tokens / 2);
    for (name, text) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
        let text = text.trim_end_matches('\n');
        if !text.is_empty() {
            let _ = write!(out, "\n--- {name} ---\n{}", half.truncate_text(text));
        }
    }
    if output.stdout.trim().is_empty() && output.stderr.trim().is_empty() {
        out.push_str("\n(no output)");
    }
    if output.truncated {
        out.push_str("\n[output was cut to the size limit of the shell]");
    }
    out
}

#[async_trait]
impl TypedTool for RunCommandTool {
    type Input = RunCommandRequest;

    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        RUN_COMMAND_DESCRIPTION
    }

    /// As [`ShellPort::classify`] declares it: [`ToolEffect::Read`] for
    /// read-only git commands, [`ToolEffect::Execute`] otherwise.
    fn effect(&self, input: &RunCommandRequest) -> ToolEffect {
        self.shell.classify(input)
    }

    async fn prepare(
        &self,
        input: RunCommandRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input.display();
        let preview = ApprovalPreview::Command {
            command: input.command.clone(),
            args: input.args.clone(),
            cwd: ".".to_owned(),
        };
        let shell = Arc::clone(&self.shell);
        let budget = self.budget;
        let run = async move {
            Ok(match shell.run(input.clone()).await {
                Ok(output) => ToolOutput::text(render_command_output(&input, &output, budget)),
                Err(err) => ToolOutput::error(err.to_string()),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()).with_preview(preview))
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use domain::workspace::WorkspaceRoot;
    use serde_json::{Value, json};

    use super::*;
    use crate::ports::shell::ShellError;
    use crate::testing::fs::{InMemoryFs, ScriptedShell, unified_diff};
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

    #[tokio::test]
    async fn edit_file_previews_the_diff_and_applies_it_when_run() {
        let fs = workspace();
        let tool = Typed(EditFileTool::new(Arc::clone(&fs) as Arc<dyn FileSystemPort>));
        let input = json!({ "path": "src/lib.rs", "old": "fn main() {}", "new": "fn main() {\n    run();\n}" });
        assert_eq!(tool.effect(&input), ToolEffect::Write);
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "src/lib.rs");
        let old = "pub mod tools;\n\nfn main() {}\n";
        let new = "pub mod tools;\n\nfn main() {\n    run();\n}\n";
        let expected = DiffSpec {
            path: "src/lib.rs".into(),
            old_text: Some(old.into()),
            new_text: new.into(),
            unified: unified_diff(&"src/lib.rs".parse().unwrap(), Some(old), new),
            approval: None,
        };
        assert!(
            expected.unified.contains("+    run();"),
            "{}",
            expected.unified
        );
        assert_eq!(
            prepared.preview,
            Some(ApprovalPreview::Diff(expected.clone()))
        );
        assert_eq!(prepared.block, Some(BlockKind::Diff(expected)));
        assert_eq!(
            fs.file("src/lib.rs").as_deref(),
            Some(old),
            "prepare must not write"
        );
        let out = prepared.run.await.unwrap();
        assert_eq!(out, ToolOutput::text("Updated src/lib.rs (5 lines)."));
        assert_eq!(fs.file("src/lib.rs").as_deref(), Some(new));
    }

    #[tokio::test]
    async fn edit_file_requires_a_unique_old() {
        let tool = Typed(EditFileTool::new(workspace()));
        let input = json!({ "path": "src/tools/files.rs", "old": "fn ", "new": "pub fn " });
        let err = tool.prepare(input, &ctx()).await.unwrap_err();
        assert_eq!(
            ToolOutput::from(err).text_content(),
            "`old` appears 2 times in the file; include more surrounding context so it \
             matches exactly once"
        );
        let input = json!({ "path": "src/lib.rs", "old": "missing", "new": "" });
        let err = tool.prepare(input, &ctx()).await.unwrap_err();
        assert!(err.to_string().starts_with("`old` was not found"), "{err}");
    }

    #[tokio::test]
    async fn edit_file_fails_when_the_file_changed_before_running() {
        let fs = workspace();
        let tool = Typed(EditFileTool::new(Arc::clone(&fs) as Arc<dyn FileSystemPort>));
        let input = json!({ "path": "Cargo.toml", "old": "[workspace]", "new": "[package]" });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        fs.set_file("Cargo.toml", "[workspace]\nmembers = []\n");
        let out = prepared.run.await.unwrap();
        assert!(out.is_error);
        assert!(
            out.text_content().contains("changed since"),
            "{}",
            out.text_content()
        );
    }

    #[tokio::test]
    async fn write_file_creates_a_file_with_a_diff() {
        let fs = workspace();
        let tool = Typed(WriteFileTool::new(
            Arc::clone(&fs) as Arc<dyn FileSystemPort>
        ));
        let input = json!({ "path": "notes/todo.md", "content": "- a\n- b\n" });
        assert_eq!(tool.effect(&input), ToolEffect::Write);
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "notes/todo.md");
        let Some(BlockKind::Diff(spec)) = &prepared.block else {
            panic!("expected a diff block, got {:?}", prepared.block);
        };
        assert_eq!(spec.old_text, None);
        assert!(spec.unified.contains("+- a"), "{}", spec.unified);
        let out = prepared.run.await.unwrap();
        assert_eq!(out, ToolOutput::text("Created notes/todo.md (2 lines)."));
        assert_eq!(fs.file("notes/todo.md").as_deref(), Some("- a\n- b\n"));
    }

    #[tokio::test]
    async fn write_file_rejects_sensitive_paths_when_preparing() {
        let tool = Typed(WriteFileTool::new(workspace()));
        let input = json!({ "path": "key.pem", "content": "x" });
        let err = tool.prepare(input, &ctx()).await.unwrap_err();
        assert!(
            matches!(&err, ToolError::Execution(m) if m.contains("denied")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn run_command_previews_the_command_and_reports_its_output() {
        let shell = Arc::new(ScriptedShell::new().on(
            "cargo",
            ["test", "-p", "my crate"],
            CommandOutput {
                status: Some(101),
                stdout: "running 1 test\n".into(),
                stderr: "error: test failed\n".into(),
                ..CommandOutput::default()
            },
        ));
        let tool = Typed(RunCommandTool::new(
            Arc::clone(&shell) as Arc<dyn ShellPort>,
            budget(),
        ));
        let input = json!({ "command": "cargo", "args": ["test", "-p", "my crate"] });
        assert_eq!(tool.effect(&input), ToolEffect::Execute);
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "cargo test -p 'my crate'");
        assert_eq!(
            prepared.preview,
            Some(ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into(), "-p".into(), "my crate".into()],
                cwd: ".".into(),
            })
        );
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "exit code: 101\n--- stdout ---\nrunning 1 test\n--- stderr ---\nerror: test failed"
        );
        assert_eq!(shell.calls()[0].timeout_s, 120);
    }

    #[tokio::test]
    async fn run_command_reads_git_history_without_approval() {
        let tool = Typed(RunCommandTool::new(
            Arc::new(ScriptedShell::new()),
            budget(),
        ));
        let read = json!({ "command": "git", "args": ["log", "--oneline"] });
        assert_eq!(tool.effect(&read), ToolEffect::Read);
        let write = json!({ "command": "git", "args": ["commit", "-m", "x"] });
        assert_eq!(tool.effect(&write), ToolEffect::Execute);
    }

    #[tokio::test]
    async fn run_command_reports_timeouts_and_errors() {
        let shell = ScriptedShell::new()
            .on(
                "sleep",
                ["9"],
                CommandOutput {
                    timed_out: true,
                    ..CommandOutput::default()
                },
            )
            .on_error(
                "nope",
                Vec::<String>::new(),
                ShellError::NotFound {
                    command: "nope".into(),
                },
            );
        let tool = Typed(RunCommandTool::new(Arc::new(shell), budget()));
        let input = json!({ "command": "sleep", "args": ["9"], "timeout_s": 1 });
        let (_, out) = call(&tool, input).await;
        assert_eq!(
            out.text_content(),
            "timed out after 1s; the process was killed\n(no output)"
        );
        let (_, out) = call(&tool, json!({ "command": "nope" })).await;
        assert!(out.is_error);
        assert_eq!(out.text_content(), "command not found: nope");
    }

    #[tokio::test]
    async fn run_command_cuts_each_stream_to_half_the_budget() {
        let stdout: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let shell = ScriptedShell::new().with_default(CommandOutput {
            status: Some(0),
            stdout,
            stderr: "warning: x\n".into(),
            ..CommandOutput::default()
        });
        let tool = Typed(RunCommandTool::new(
            Arc::new(shell),
            ResultBudget::new(NOTE_RESERVE_TOKENS + 40),
        ));
        let (_, out) = call(&tool, json!({ "command": "make" })).await;
        let text = out.text_content();
        assert!(text.contains("líneas más omitidas"), "{text}");
        assert!(text.ends_with("--- stderr ---\nwarning: x"), "{text}");
    }
}
