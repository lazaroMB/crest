//! In-memory fakes of the file ports: [`InMemoryFs`] implements
//! [`FileSystemPort`], [`FileSearchPort`] and [`ContentSearchPort`] over a
//! map of paths to bytes, so use cases can be tested with a single fake
//! without touching the disk, `rg` or `fzf`; [`ScriptedShell`] fakes
//! [`ShellPort`] with programmed replies.
//!
//! The fake applies the same guards the real adapter does (sensitive paths,
//! binary files, the read size limit) and lets tests simulate what only a
//! real file system produces: [`InMemoryFs::with_escaping_link`] marks a
//! path as a symlink that resolves outside the workspace, and
//! [`InMemoryFs::set_file`] changes a file behind the use case's back.
//!
//! Diffs of prepared changes come from [`unified_diff`], a simple stand-in
//! for the adapter's `similar`-based diff.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::workspace::{PathError, SensitivePathRules, WorkspacePath, WorkspaceRoot};

use crate::ports::fs::{
    EditFileRequest, FileChange, FileContent, FileSystemPort, FsError, MAX_READ_BYTES,
    PendingChange, ReadFileRequest, WriteFileRequest, replace_unique,
};
use crate::ports::search::{
    ColumnRange, ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepHit,
    GrepRequest, GrepResult, ListFilesRequest, SearchError, TreeNode, TreeRequest,
};
use crate::ports::shell::{CommandOutput, RunCommandRequest, ShellError, ShellPort, ShellSpec};

/// The root of in-memory test workspaces: `/ws`, or `C:\ws` on Windows,
/// where `/ws` is not absolute.
///
/// # Panics
///
/// Never: the path is absolute on its platform.
#[must_use]
pub fn test_root() -> WorkspaceRoot {
    let path = if cfg!(windows) { r"C:\ws" } else { "/ws" };
    WorkspaceRoot::new(path).unwrap_or_else(|err| panic!("{err}"))
}

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

    async fn read_bytes(&self, path: &WorkspacePath, max: u64) -> Result<Vec<u8>, FsError> {
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
        if size > max {
            return Err(FsError::TooLarge { size, limit: max });
        }
        Ok(bytes.clone())
    }

    async fn exists(&self, path: &WorkspacePath) -> Result<bool, FsError> {
        self.check_inside(path)?;
        let files = self.lock();
        Ok(files.contains_key(path) || is_dir(&files, path))
    }

    async fn prepare_edit(&self, request: EditFileRequest) -> Result<PendingChange, FsError> {
        let path = request.path;
        self.check_access(&path)?;
        let files = self.lock();
        check_writable(&files, &path)?;
        let original = match files.get(&path) {
            Some(bytes) => as_text(bytes).ok_or_else(|| FsError::Binary { path: path.clone() })?,
            None => return Err(FsError::NotFound { path }),
        };
        let new_content = replace_unique(original, &request.old, &request.new)?;
        let diff = unified_diff(&path, Some(original), &new_content);
        Ok(PendingChange::new(
            path,
            Some(original.to_owned()),
            new_content,
            diff,
        ))
    }

    async fn prepare_write(&self, request: WriteFileRequest) -> Result<PendingChange, FsError> {
        let path = request.path;
        self.check_access(&path)?;
        let files = self.lock();
        check_writable(&files, &path)?;
        let original = match files.get(&path) {
            Some(bytes) => Some(
                as_text(bytes)
                    .ok_or_else(|| FsError::Binary { path: path.clone() })?
                    .to_owned(),
            ),
            None => None,
        };
        let diff = unified_diff(&path, original.as_deref(), &request.content);
        Ok(PendingChange::new(path, original, request.content, diff))
    }

    async fn apply(&self, change: PendingChange) -> Result<FileChange, FsError> {
        let path = change.path();
        self.check_access(path)?;
        let mut files = self.lock();
        check_writable(&files, path)?;
        let current = files.get(path).map(Vec::as_slice);
        if current != change.original().map(str::as_bytes) {
            return Err(FsError::Stale { path: path.clone() });
        }
        files.insert(path.clone(), change.new_content().as_bytes().to_vec());
        Ok(change.to_change())
    }
}

/// Fails when `path` cannot hold a file: it is a directory, or one of its
/// ancestors is a file.
fn check_writable(
    files: &BTreeMap<WorkspacePath, Vec<u8>>,
    path: &WorkspacePath,
) -> Result<(), FsError> {
    if is_dir(files, path) {
        return Err(FsError::IsDirectory { path: path.clone() });
    }
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if files.contains_key(&dir) {
            return Err(FsError::Io(format!("not a directory: {dir}")));
        }
        ancestor = dir.parent();
    }
    Ok(())
}

/// Lines of unchanged context around the changed block of [`unified_diff`].
const DIFF_CONTEXT: usize = 3;

/// A minimal unified diff from `old` (`None` for a new file) to `new`.
///
/// It trims the common leading and trailing lines and reports everything
/// between them as one hunk (all removals, then all additions) with up to
/// [`DIFF_CONTEXT`] lines of context. That is stable and readable, but not
/// minimal like the real adapter's, which computes it with `similar`; tests
/// should assert on the fake's diff only through this format. Identical
/// contents give an empty diff.
pub fn unified_diff(path: &WorkspacePath, old: Option<&str>, new: &str) -> String {
    let old_lines: Vec<&str> = old.map_or_else(Vec::new, |old| old.split_inclusive('\n').collect());
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    if old.is_some() && old_lines == new_lines {
        return String::new();
    }
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = prefix.saturating_sub(DIFF_CONTEXT);
    let old_end = (old_lines.len() - suffix + DIFF_CONTEXT).min(old_lines.len());
    let new_end = (new_lines.len() - suffix + DIFF_CONTEXT).min(new_lines.len());

    let mut out = match old {
        Some(_) => format!("--- a/{path}\n+++ b/{path}\n"),
        None => format!("--- /dev/null\n+++ b/{path}\n"),
    };
    out.push_str(&format!(
        "@@ -{} +{} @@\n",
        hunk_range(start, old_end - start),
        hunk_range(start, new_end - start)
    ));
    let mut push = |sign: char, line: &str| {
        out.push(sign);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    };
    for line in &old_lines[start..prefix] {
        push(' ', line);
    }
    for line in &old_lines[prefix..old_lines.len() - suffix] {
        push('-', line);
    }
    for line in &new_lines[prefix..new_lines.len() - suffix] {
        push('+', line);
    }
    for line in &new_lines[new_lines.len() - suffix..new_end] {
        push(' ', line);
    }
    out
}

/// The `start,len` of a hunk side, 1-based; an empty side names the line
/// before it, as `diff -u` does.
fn hunk_range(start: usize, len: usize) -> String {
    let first = if len == 0 { start } else { start + 1 };
    format!("{first},{len}")
}

impl InMemoryFs {
    /// The files `rg --files --hidden --glob '!.git'` would list: all but
    /// those inside `.git` or behind an escaping link, sorted by path.
    fn listed(&self) -> Vec<WorkspacePath> {
        self.lock()
            .keys()
            .filter(|path| !path.components().any(|c| c == ".git"))
            .filter(|path| self.check_inside(path).is_ok())
            .cloned()
            .collect()
    }
}

/// Matches `path` against a glob as `rg --glob` does for the simple cases:
/// a glob without `/` matches the file name at any depth, one with `/`
/// matches the whole path. `*` and `?` stay within a component, `**`
/// crosses them and `**/` also matches nothing. Character classes, braces
/// and negation are not supported (they match literally).
pub fn glob_matches(glob: &str, path: &WorkspacePath) -> bool {
    let pattern: Vec<char> = glob.chars().collect();
    let subject = if glob.contains('/') {
        path.as_str()
    } else {
        path.file_name().unwrap_or_default()
    };
    let subject: Vec<char> = subject.chars().collect();
    glob_rec(&pattern, &subject)
}

fn glob_rec(pattern: &[char], subject: &[char]) -> bool {
    match pattern {
        [] => subject.is_empty(),
        ['*', '*', rest @ ..] => {
            let after_slash = rest.strip_prefix(&['/']);
            after_slash.is_some_and(|r| glob_rec(r, subject))
                || (0..=subject.len()).any(|i| glob_rec(rest, &subject[i..]))
        }
        ['*', rest @ ..] => {
            let component_end = subject
                .iter()
                .position(|c| *c == '/')
                .unwrap_or(subject.len());
            (0..=component_end).any(|i| glob_rec(rest, &subject[i..]))
        }
        ['?', rest @ ..] => match subject {
            [c, tail @ ..] if *c != '/' => glob_rec(rest, tail),
            _ => false,
        },
        [p, rest @ ..] => match subject {
            [c, tail @ ..] if c == p => glob_rec(rest, tail),
            _ => false,
        },
    }
}

#[async_trait]
impl FileSearchPort for InMemoryFs {
    async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError> {
        let matching = self
            .listed()
            .into_iter()
            .filter(|path| request.glob.as_ref().is_none_or(|g| glob_matches(g, path)))
            .collect();
        Ok(FileList::from_matches(matching, request.limit))
    }

    /// Matches `request.query` as a case-insensitive substring (the
    /// adapter's fallback without `fzf`), ranking paths whose file name
    /// contains it first, then shorter paths, then by path.
    async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError> {
        let query = request.query.to_lowercase();
        let contains = |s: &str| s.to_lowercase().contains(&query);
        let mut matching: Vec<WorkspacePath> = self
            .listed()
            .into_iter()
            .filter(|path| contains(path.as_str()))
            .collect();
        matching.sort_by_cached_key(|path| {
            let in_name = path.file_name().is_some_and(contains);
            (!in_name, path.as_str().len(), path.clone())
        });
        Ok(FileList::from_matches(matching, request.limit))
    }

    async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
        let base = request.path.unwrap_or_else(WorkspacePath::root);
        self.check_inside(&base)?;
        Ok(TreeNode::from_files(&base, &self.listed(), request.depth))
    }
}

/// Regex metacharacters the fake grep cannot interpret.
const REGEX_META: &str = ".^$*+?()[]{}|";

/// The literal text a grep pattern stands for. The fake has no regex
/// engine: a non-`fixed` pattern may only escape punctuation (`\.`), and
/// any other metacharacter fails with [`SearchError::InvalidPattern`], so a
/// test never silently gets literal matching where it expected a regex.
fn literal_pattern(request: &GrepRequest) -> Result<String, SearchError> {
    if request.fixed {
        return Ok(request.pattern.clone());
    }
    let unsupported = |what: String| {
        SearchError::InvalidPattern(format!(
            "{what} in `{}`: the fake grep only matches literals",
            request.pattern
        ))
    };
    let mut literal = String::new();
    let mut chars = request.pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(escaped) if escaped.is_ascii_punctuation() => literal.push(escaped),
                Some(escaped) => return Err(unsupported(format!("escape `\\{escaped}`"))),
                None => return Err(unsupported("trailing `\\`".into())),
            },
            c if REGEX_META.contains(c) => return Err(unsupported(format!("`{c}`"))),
            c => literal.push(c),
        }
    }
    Ok(literal)
}

#[async_trait]
impl ContentSearchPort for InMemoryFs {
    /// Searches the listed text files (binary ones are skipped, as `rg`
    /// does) for a literal (see [`literal_pattern`]); `ignore_case` folds
    /// ASCII letters only. Each hit carries its own neighbouring lines as
    /// context, even when they are matches themselves.
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, SearchError> {
        let needle = literal_pattern(&request)?;
        let fold = |s: &str| {
            if request.ignore_case {
                s.to_ascii_lowercase()
            } else {
                s.to_owned()
            }
        };
        let needle = fold(&needle);
        let base = request.path.clone().unwrap_or_else(WorkspacePath::root);
        self.check_inside(&base)?;
        let context = usize::try_from(request.context).unwrap_or(usize::MAX);
        let paths: Vec<WorkspacePath> = self
            .listed()
            .into_iter()
            .filter(|path| is_within(path, &base))
            .filter(|path| request.glob.as_ref().is_none_or(|g| glob_matches(g, path)))
            .collect();

        let files = self.lock();
        let mut result = GrepResult::default();
        for path in paths {
            let Some(text) = files.get(&path).and_then(|bytes| as_text(bytes)) else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                let Some(start) = fold(line).find(&needle) else {
                    continue;
                };
                result.total += 1;
                if result.hits.len() == request.limit {
                    result.truncated = true;
                    continue;
                }
                let after_end = index
                    .saturating_add(context)
                    .saturating_add(1)
                    .min(lines.len());
                result.hits.push(GrepHit {
                    path: path.clone(),
                    line_number: index as u64 + 1,
                    column_range: ColumnRange {
                        start,
                        end: start + needle.len(),
                    },
                    line: (*line).to_owned(),
                    context_before: to_owned(&lines[index.saturating_sub(context)..index]),
                    context_after: to_owned(&lines[index + 1..after_end]),
                });
            }
        }
        Ok(result)
    }
}

fn to_owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// What [`ScriptedShell`] does for a command.
#[derive(Debug, Clone)]
enum Reply {
    Return(Result<CommandOutput, ShellError>),
    Hang,
}

/// A [`ShellPort`] that answers from a script instead of spawning
/// processes.
///
/// Replies are keyed by the exact program and arguments; the timeout is
/// ignored. Unscripted commands get the default reply, which is
/// [`ShellError::NotFound`] unless configured. A hanging reply never
/// resolves, to test timeouts and cancellation: dropping its future counts
/// as one [`ScriptedShell::cancelled`].
#[derive(Debug, Default)]
pub struct ScriptedShell {
    script: BTreeMap<(String, Vec<String>), Reply>,
    default: Option<Reply>,
    calls: Mutex<Vec<RunCommandRequest>>,
    cancelled: Arc<AtomicUsize>,
}

impl ScriptedShell {
    /// A shell with an empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers `command args` with `output`, replacing an earlier reply.
    pub fn on<I, S>(self, command: &str, args: I, output: CommandOutput) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reply(command, args, Reply::Return(Ok(output)))
    }

    /// Answers `command args` with `error`.
    pub fn on_error<I, S>(self, command: &str, args: I, error: ShellError) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reply(command, args, Reply::Return(Err(error)))
    }

    /// Makes `command args` hang forever.
    pub fn on_hang<I, S>(self, command: &str, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.reply(command, args, Reply::Hang)
    }

    /// Answers unscripted commands with `output`.
    pub fn with_default(mut self, output: CommandOutput) -> Self {
        self.default = Some(Reply::Return(Ok(output)));
        self
    }

    /// Answers unscripted commands with `error`.
    pub fn with_default_error(mut self, error: ShellError) -> Self {
        self.default = Some(Reply::Return(Err(error)));
        self
    }

    /// Makes unscripted commands hang forever.
    pub fn hang_by_default(mut self) -> Self {
        self.default = Some(Reply::Hang);
        self
    }

    /// Every request received, in order, including hanging ones.
    pub fn calls(&self) -> Vec<RunCommandRequest> {
        self.calls_lock().clone()
    }

    /// How many hanging runs were dropped before resolving.
    pub fn cancelled(&self) -> usize {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn reply<I, S>(mut self, command: &str, args: I, reply: Reply) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args = args.into_iter().map(Into::into).collect();
        self.script.insert((command.to_owned(), args), reply);
        self
    }

    fn calls_lock(&self) -> MutexGuard<'_, Vec<RunCommandRequest>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Counts a cancellation when a hanging run is dropped.
struct CancelGuard(Arc<AtomicUsize>);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl ShellPort for ScriptedShell {
    /// `sh -c` on every platform, so scripts are the same everywhere.
    fn line_shell(&self) -> ShellSpec {
        ShellSpec::posix()
    }

    async fn run(&self, request: RunCommandRequest) -> Result<CommandOutput, ShellError> {
        let key = (request.command.clone(), request.args.clone());
        let reply = self.script.get(&key).or(self.default.as_ref()).cloned();
        let command = request.command.clone();
        self.calls_lock().push(request);
        match reply {
            Some(Reply::Return(result)) => result,
            None => Err(ShellError::NotFound { command }),
            Some(Reply::Hang) => {
                let _guard = CancelGuard(Arc::clone(&self.cancelled));
                std::future::pending().await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn fs() -> InMemoryFs {
        InMemoryFs::new(test_root())
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
        assert_eq!(fs.root(), &test_root());
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

    fn edit(path: &str, old: &str, new: &str) -> EditFileRequest {
        EditFileRequest {
            path: p(path),
            old: old.into(),
            new: new.into(),
        }
    }

    fn write(path: &str, content: &str) -> WriteFileRequest {
        WriteFileRequest {
            path: p(path),
            content: content.into(),
        }
    }

    #[tokio::test]
    async fn edit_is_written_only_when_applied() {
        let fs = fs();
        let pending = fs
            .prepare_edit(edit("src/lib.rs", "two", "2"))
            .await
            .unwrap();
        assert_eq!(pending.new_content(), "one\n2\nthree\n");
        assert_eq!(
            pending.diff_unified(),
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"
        );
        assert_eq!(fs.file("src/lib.rs").as_deref(), Some("one\ntwo\nthree\n"));

        let change = fs.apply(pending).await.unwrap();
        assert!(!change.created);
        assert_eq!(fs.file("src/lib.rs").as_deref(), Some("one\n2\nthree\n"));
    }

    #[tokio::test]
    async fn edit_rejects_missing_ambiguous_and_guarded_files() {
        let fs = fs().with_file("dup.txt", "x x");
        let cases = [
            (
                edit("nope.rs", "a", "b"),
                FsError::NotFound { path: p("nope.rs") },
            ),
            (
                edit("dup.txt", "x", "y"),
                FsError::OldNotUnique { count: 2 },
            ),
            (edit("dup.txt", "z", "y"), FsError::OldNotFound),
            (edit("dup.txt", "", "y"), FsError::EmptyOld),
            (
                edit("logo.png", "P", "Q"),
                FsError::Binary {
                    path: p("logo.png"),
                },
            ),
            (
                edit("src", "a", "b"),
                FsError::IsDirectory { path: p("src") },
            ),
            (
                edit(".env", "KEY", "K"),
                FsError::SensitivePath {
                    path: p(".env"),
                    pattern: ".env*".into(),
                },
            ),
        ];
        for (request, expected) in cases {
            assert_eq!(fs.prepare_edit(request).await, Err(expected));
        }
        let outside = fs.with_escaping_link("link");
        assert_eq!(
            outside.prepare_edit(edit("link/a", "a", "b")).await,
            Err(PathError::OutsideWorkspace {
                path: "link/a".into()
            }
            .into())
        );
    }

    #[tokio::test]
    async fn write_creates_files_with_a_creation_diff() {
        let fs = fs();
        let pending = fs
            .prepare_write(write("docs/new.md", "hi\nthere"))
            .await
            .unwrap();
        assert!(pending.creates());
        assert_eq!(
            pending.diff_unified(),
            "--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1,2 @@\n+hi\n+there\n\\ No newline at end of file\n"
        );
        let change = fs.apply(pending).await.unwrap();
        assert!(change.created);
        assert_eq!(fs.file("docs/new.md").as_deref(), Some("hi\nthere"));
    }

    #[tokio::test]
    async fn write_overwrites_with_a_diff_against_the_current_content() {
        let fs = fs();
        let pending = fs
            .prepare_write(write("src/lib.rs", "one\n"))
            .await
            .unwrap();
        assert_eq!(pending.original(), Some("one\ntwo\nthree\n"));
        assert_eq!(
            pending.diff_unified(),
            "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,1 @@\n one\n-two\n-three\n"
        );
        let same = fs
            .prepare_write(write("src/lib.rs", "one\ntwo\nthree\n"))
            .await;
        assert_eq!(same.unwrap().diff_unified(), "");
    }

    #[tokio::test]
    async fn write_rejects_guarded_paths() {
        let fs = fs();
        let cases = [
            (write("src", "x"), FsError::IsDirectory { path: p("src") }),
            (
                write("logo.png", "x"),
                FsError::Binary {
                    path: p("logo.png"),
                },
            ),
            (
                write("src/lib.rs/x", "x"),
                FsError::Io("not a directory: src/lib.rs".into()),
            ),
            (
                write("keys/id_rsa", "x"),
                FsError::SensitivePath {
                    path: p("keys/id_rsa"),
                    pattern: "id_rsa*".into(),
                },
            ),
        ];
        for (request, expected) in cases {
            assert_eq!(fs.prepare_write(request).await, Err(expected));
        }
    }

    #[tokio::test]
    async fn apply_detects_stale_changes() {
        let fs = fs();
        let edit = fs
            .prepare_edit(edit("src/lib.rs", "one", "1"))
            .await
            .unwrap();
        fs.set_file("src/lib.rs", "changed\n");
        assert_eq!(
            fs.apply(edit).await,
            Err(FsError::Stale {
                path: p("src/lib.rs")
            })
        );
        assert_eq!(fs.file("src/lib.rs").as_deref(), Some("changed\n"));

        let create = fs.prepare_write(write("new.txt", "a")).await.unwrap();
        fs.set_file("new.txt", "b");
        assert_eq!(
            fs.apply(create).await,
            Err(FsError::Stale { path: p("new.txt") })
        );

        let overwrite = fs.prepare_write(write("new.txt", "c")).await.unwrap();
        fs.remove_file("new.txt");
        assert_eq!(
            fs.apply(overwrite).await,
            Err(FsError::Stale { path: p("new.txt") })
        );
    }

    #[test]
    fn diff_keeps_only_nearby_context() {
        let old: String = (1..=10).map(|n| format!("{n}\n")).collect();
        let new = old.replace("5\n", "five\n");
        let diff = unified_diff(&p("n.txt"), Some(&old), &new);
        assert_eq!(
            diff,
            "--- a/n.txt\n+++ b/n.txt\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n"
        );
    }

    fn search_fs() -> InMemoryFs {
        InMemoryFs::new(test_root())
            .with_file("Cargo.toml", "")
            .with_file("src/main.rs", "")
            .with_file("src/ports/fs.rs", "")
            .with_file("crates/main/lib.rs", "")
            .with_file(".github/ci.yml", "")
            .with_file(".git/HEAD", "")
            .with_file("out/x.rs", "")
            .with_escaping_link("out")
    }

    fn names(list: &FileList) -> Vec<&str> {
        list.paths.iter().map(WorkspacePath::as_str).collect()
    }

    #[tokio::test]
    async fn list_files_skips_git_and_filters_by_glob() {
        let fs = search_fs();
        let all = fs.list_files(ListFilesRequest::default()).await.unwrap();
        assert_eq!(
            names(&all),
            [
                ".github/ci.yml",
                "Cargo.toml",
                "crates/main/lib.rs",
                "src/main.rs",
                "src/ports/fs.rs"
            ]
        );
        let glob = |g: &str, limit| ListFilesRequest {
            glob: Some(g.into()),
            limit,
        };
        let rs = fs.list_files(glob("*.rs", 2)).await.unwrap();
        assert_eq!(names(&rs), ["crates/main/lib.rs", "src/main.rs"]);
        assert!(rs.truncated);
        assert_eq!(rs.total, 3);
        let nested = fs.list_files(glob("src/**/*.rs", 10)).await.unwrap();
        assert_eq!(names(&nested), ["src/main.rs", "src/ports/fs.rs"]);
        let top = fs.list_files(glob("src/*.rs", 10)).await.unwrap();
        assert_eq!(names(&top), ["src/main.rs"]);
    }

    #[test]
    fn glob_supports_simple_wildcards() {
        for (glob, path, expected) in [
            ("*.rs", "a/b/c.rs", true),
            ("?.rs", "a/b.rs", true),
            ("?.rs", "ab.rs", false),
            ("**/*.rs", "c.rs", true),
            ("**/*.rs", "a/b/c.rs", true),
            ("a/*", "a/b/c", false),
            ("a/**", "a/b/c", true),
            ("Cargo.toml", "x/Cargo.toml", true),
            ("x/Cargo.toml", "Cargo.toml", false),
        ] {
            assert_eq!(glob_matches(glob, &p(path)), expected, "{glob} vs {path}");
        }
    }

    #[tokio::test]
    async fn find_file_ranks_name_matches_first() {
        let fs = search_fs();
        let found = fs.find_file(FindFileRequest::new("MAIN")).await.unwrap();
        assert_eq!(names(&found), ["src/main.rs", "crates/main/lib.rs"]);
        let limited = FindFileRequest {
            limit: 1,
            ..FindFileRequest::new("")
        };
        let first = fs.find_file(limited).await.unwrap();
        assert_eq!((names(&first), first.total), (vec!["Cargo.toml"], 5));
    }

    #[tokio::test]
    async fn tree_is_built_from_the_listing() {
        let fs = search_fs();
        let tree = fs.tree(TreeRequest::default()).await.unwrap();
        let top: Vec<_> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(top, [".github", "crates", "src", "Cargo.toml"]);

        let src = TreeRequest {
            path: Some(p("src")),
            depth: 1,
        };
        let src = fs.tree(src).await.unwrap();
        assert_eq!(src.children[0].name, "ports");
        assert_eq!(src.children[0].omitted_files, 1);

        let outside = TreeRequest {
            path: Some(p("out")),
            depth: 1,
        };
        assert_eq!(
            fs.tree(outside).await,
            Err(SearchError::Path(PathError::OutsideWorkspace {
                path: "out".into()
            }))
        );
    }

    fn grep_fs() -> InMemoryFs {
        InMemoryFs::new(test_root())
            .with_file("src/a.rs", "fn one() {}\nlet x = 1;\nfn two() {}\nend\n")
            .with_file("src/b.txt", "FN upper\n")
            .with_file("docs/c.md", "fn in docs\n")
            .with_bytes("src/bin.rs", b"fn \0 binary".to_vec())
            .with_file(".git/config", "fn hidden\n")
    }

    fn grep_at(pattern: &str, context: u32) -> GrepRequest {
        GrepRequest {
            context,
            ..GrepRequest::new(pattern)
        }
    }

    fn hits(result: &GrepResult) -> Vec<(&str, u64)> {
        result
            .hits
            .iter()
            .map(|h| (h.path.as_str(), h.line_number))
            .collect()
    }

    #[tokio::test]
    async fn grep_returns_hits_with_context() {
        let fs = grep_fs();
        let result = fs.grep(grep_at("fn", 1)).await.unwrap();
        assert_eq!(
            hits(&result),
            [("docs/c.md", 1), ("src/a.rs", 1), ("src/a.rs", 3)]
        );
        assert_eq!((result.total, result.truncated), (3, false));
        let two = &result.hits[2];
        assert_eq!(two.column_range, ColumnRange { start: 0, end: 2 });
        assert_eq!(two.line, "fn two() {}");
        assert_eq!(two.context_before, ["let x = 1;"]);
        assert_eq!(two.context_after, ["end"]);
        assert!(result.hits[1].context_before.is_empty());
    }

    #[tokio::test]
    async fn grep_honors_case_path_glob_and_limit() {
        let fs = grep_fs();
        let upper = GrepRequest {
            ignore_case: true,
            path: Some(p("src")),
            ..GrepRequest::new("fn")
        };
        let result = fs.grep(upper).await.unwrap();
        assert_eq!(
            hits(&result),
            [("src/a.rs", 1), ("src/a.rs", 3), ("src/b.txt", 1)]
        );

        let globbed = GrepRequest {
            glob: Some("*.md".into()),
            ..GrepRequest::new("fn")
        };
        assert_eq!(hits(&fs.grep(globbed).await.unwrap()), [("docs/c.md", 1)]);

        let limited = GrepRequest {
            limit: 1,
            ..GrepRequest::new("fn")
        };
        let result = fs.grep(limited).await.unwrap();
        assert_eq!(
            (result.hits.len(), result.total, result.truncated),
            (1, 3, true)
        );

        let none = fs.grep(GrepRequest::new("absent")).await.unwrap();
        assert_eq!(none, GrepResult::default());
    }

    #[tokio::test]
    async fn grep_matches_literals_only() {
        let fs = grep_fs();
        let escaped = fs.grep(GrepRequest::new(r"one\(\)")).await.unwrap();
        assert_eq!(hits(&escaped), [("src/a.rs", 1)]);
        assert_eq!(
            escaped.hits[0].column_range,
            ColumnRange { start: 3, end: 8 }
        );
        let fixed = GrepRequest {
            fixed: true,
            ..GrepRequest::new("() {")
        };
        assert_eq!(fs.grep(fixed).await.unwrap().total, 2);
        for pattern in ["fn (", r"\d", "a.b"] {
            assert!(
                matches!(
                    fs.grep(GrepRequest::new(pattern)).await,
                    Err(SearchError::InvalidPattern(_))
                ),
                "{pattern}"
            );
        }
    }

    #[tokio::test]
    async fn grep_rejects_paths_outside_the_workspace() {
        let fs = grep_fs().with_escaping_link("docs");
        let request = GrepRequest {
            path: Some(p("docs")),
            ..GrepRequest::new("fn")
        };
        assert_eq!(
            fs.grep(request).await,
            Err(SearchError::Path(PathError::OutsideWorkspace {
                path: "docs".into()
            }))
        );
    }
}

#[cfg(test)]
mod shell_tests {
    use std::time::Duration;

    use domain::permission::ToolEffect;

    use super::*;

    fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: Some(0),
            stdout: stdout.into(),
            ..CommandOutput::default()
        }
    }

    fn run(command: &str, args: &[&str]) -> RunCommandRequest {
        RunCommandRequest::new(command, args.iter().copied())
    }

    #[tokio::test]
    async fn replies_by_exact_command_and_records_calls() {
        let shell = ScriptedShell::new()
            .on("cargo", ["test"], ok("old"))
            .on("cargo", ["test"], ok("passed"))
            .on_error("cargo", ["fmt"], ShellError::Spawn("denied".into()));

        assert_eq!(shell.run(run("cargo", &["test"])).await, Ok(ok("passed")));
        assert_eq!(
            shell.run(run("cargo", &["fmt"])).await,
            Err(ShellError::Spawn("denied".into()))
        );
        assert_eq!(
            shell.run(run("cargo", &["test", "-q"])).await,
            Err(ShellError::NotFound {
                command: "cargo".into()
            })
        );
        let calls: Vec<_> = shell
            .calls()
            .iter()
            .map(RunCommandRequest::display)
            .collect();
        assert_eq!(calls, ["cargo test", "cargo fmt", "cargo test -q"]);
    }

    #[tokio::test]
    async fn default_reply_is_configurable() {
        let shell = ScriptedShell::new().with_default(ok("any"));
        assert_eq!(shell.run(run("ls", &[])).await, Ok(ok("any")));
        let failing = ScriptedShell::new().with_default_error(ShellError::Io("boom".into()));
        assert_eq!(
            failing.run(run("ls", &[])).await,
            Err(ShellError::Io("boom".into()))
        );
    }

    #[tokio::test]
    async fn hanging_runs_count_cancellation_on_drop() {
        let shell = ScriptedShell::new()
            .on("echo", ["hi"], ok("hi"))
            .hang_by_default()
            .on_hang("sleep", ["9"]);
        for request in [run("sleep", &["9"]), run("other", &[])] {
            let timed = tokio::time::timeout(Duration::from_millis(10), shell.run(request)).await;
            assert!(timed.is_err());
        }
        assert_eq!(shell.cancelled(), 2);
        assert_eq!(shell.calls().len(), 2);
        assert_eq!(shell.run(run("echo", &["hi"])).await, Ok(ok("hi")));
        assert_eq!(shell.cancelled(), 2);
    }

    #[test]
    fn classification_uses_the_port_default() {
        let shell = ScriptedShell::new();
        assert_eq!(shell.classify(&run("git", &["status"])), ToolEffect::Read);
        assert_eq!(shell.classify(&run("rm", &["x"])), ToolEffect::Execute);
    }
}
