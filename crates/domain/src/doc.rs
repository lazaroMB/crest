//! Chat documents: markdown files that belong to one chat, and the pure
//! rules that build and edit them (R03, spec 0003 section 1).
//!
//! The text of a doc lives in its file; [`Doc`] carries only where it is.
//! On disk an image link is `assets/<name>`, a relative path every markdown
//! reader resolves.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::block::{Align, BlockKind, Cell, ChartSpec, ImageData, SourceRef, TableSpec};
use crate::conversation::SessionId;

/// Identifies a [`Doc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocId(pub Uuid);

impl DocId {
    /// A fresh random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parses the text form, trimming blanks.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Uuid::parse_str(text.trim()).ok().map(Self)
    }
}

impl Default for DocId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for DocId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// One markdown document of one chat. The markdown itself is in the file
/// `<dir>/<file_name>`; images sit in `<dir>/assets/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    /// Doc id.
    pub id: DocId,
    /// The chat it belongs to.
    pub session_id: SessionId,
    /// Title shown in the panel and the sidebar.
    pub title: String,
    /// File name with its extension (`release-notes.md`), unique in `dir`.
    pub file_name: String,
    /// Folder of the file, as text (`<root>/.crest/docs/<session id>` or
    /// `<data dir>/docs/<session id>`); the domain does no I/O on it.
    pub dir: String,
    /// Creation time, milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Last change, milliseconds since the Unix epoch.
    pub updated_at: u64,
}

impl Doc {
    /// The path of the markdown file, `dir` and `file_name` joined with `/`.
    #[must_use]
    pub fn path(&self) -> String {
        format!("{}/{}", self.dir.trim_end_matches('/'), self.file_name)
    }
}

// ---------------------------------------------------------------------------
// Appending and finding text
// ---------------------------------------------------------------------------

/// `markdown` with `addition` appended as its own section, a blank line
/// between them. An empty doc becomes the addition; the result ends with a
/// newline.
#[must_use]
pub fn append_markdown(markdown: &str, addition: &str) -> String {
    let body = markdown.trim_end();
    let addition = addition.trim();
    if body.is_empty() {
        format!("{addition}\n")
    } else {
        format!("{body}\n\n{addition}\n")
    }
}

/// One step of the loose pattern [`find_span`] builds from a passage.
enum Tok {
    /// Any run (possibly empty) of `*`, `_` and backtick.
    Marks,
    /// This character.
    Lit(char),
    /// One or more whitespace characters.
    Ws,
    /// An optional line prefix: `- `, `* `, `1. `, `# `..`### `, `> `.
    Prefix,
}

/// Where `text` sits in `markdown`, as byte offsets `[start, end)`.
///
/// An exact substring wins. Otherwise the words of `text` are matched in
/// order, allowing marks (`*`, `_`, backtick) between the characters of a
/// word, and between words whitespace with marks and an optional line
/// prefix around it: text selected on the rendered page has lost those.
/// The loose match includes the marks that touch the passage. Empty `text`
/// finds nothing (the prototype matched at offset 0).
#[must_use]
pub fn find_span(markdown: &str, text: &str) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }
    if let Some(start) = markdown.find(text) {
        return Some((start, start + text.len()));
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let mut pattern = vec![Tok::Marks];
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            pattern.extend([Tok::Marks, Tok::Ws, Tok::Prefix, Tok::Marks]);
        }
        for (at, c) in word.chars().enumerate() {
            if at > 0 {
                pattern.push(Tok::Marks);
            }
            pattern.push(Tok::Lit(c));
        }
    }
    pattern.push(Tok::Marks);
    markdown
        .char_indices()
        .map(|(i, _)| i)
        .find_map(|start| loose_match(&pattern, markdown, start).map(|end| (start, end)))
}

/// Matches `pattern` at byte `pos` of `s`, trying the greedy choice first
/// and backing off like a regex would; returns the end of the match.
fn loose_match(pattern: &[Tok], s: &str, pos: usize) -> Option<usize> {
    let Some((tok, rest)) = pattern.split_first() else {
        return Some(pos);
    };
    let tail = &s[pos..];
    match tok {
        Tok::Lit(c) => {
            let ch = tail.chars().next()?;
            if ch == *c {
                loose_match(rest, s, pos + ch.len_utf8())
            } else {
                None
            }
        }
        Tok::Marks => {
            let run = tail
                .bytes()
                .take_while(|b| matches!(b, b'*' | b'_' | b'`'))
                .count();
            (0..=run).rev().find_map(|n| loose_match(rest, s, pos + n))
        }
        Tok::Ws => {
            let run = whitespace_len(tail);
            if run == 0 {
                None
            } else {
                loose_match(rest, s, pos + run)
            }
        }
        Tok::Prefix => prefix_ends(tail)
            .into_iter()
            .find_map(|n| loose_match(rest, s, pos + n)),
    }
}

/// Bytes of the whitespace run at the start of `s`.
fn whitespace_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_whitespace())
        .map(char::len_utf8)
        .sum()
}

/// The ways an optional line prefix can match at the start of `tail`, most
/// greedy first, ending with "no prefix".
fn prefix_ends(tail: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let bytes = tail.as_bytes();
    let ws_after = |at: usize| whitespace_len(&tail[at..]);
    match bytes.first() {
        Some(b'-' | b'*') if ws_after(1) > 0 => ends.push(1 + ws_after(1)),
        Some(b'#') => {
            let hashes = bytes.iter().take_while(|b| **b == b'#').count();
            if hashes <= 3 && ws_after(hashes) > 0 {
                ends.push(hashes + ws_after(hashes));
            }
        }
        Some(b'>') => {
            let space = tail[1..].chars().next().filter(|c| c.is_whitespace());
            if let Some(c) = space {
                ends.push(1 + c.len_utf8());
            }
            ends.push(1);
        }
        Some(b'0'..=b'9') => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if bytes.get(digits) == Some(&b'.') && ws_after(digits + 1) > 0 {
                ends.push(digits + 1 + ws_after(digits + 1));
            }
        }
        _ => {}
    }
    ends.push(0);
    ends
}

/// `markdown` with the passage `from` (found as [`find_span`] finds it)
/// replaced by `to`; unchanged when the passage is not there.
#[must_use]
pub fn replace_span(markdown: &str, from: &str, to: &str) -> String {
    match find_span(markdown, from) {
        Some((start, end)) => format!("{}{}{}", &markdown[..start], to, &markdown[end..]),
        None => markdown.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Toolbar formats
// ---------------------------------------------------------------------------

/// A format the toolbar applies to the selection of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `**x**`.
    Bold,
    /// `*x*`.
    Italic,
    /// `` `x` ``, or a fence when the selection spans lines.
    Code,
    /// `## ` on every selected line.
    Heading,
    /// `- ` on every selected line.
    List,
    /// `> ` on every selected line.
    Quote,
    /// `[x](url)`.
    Link,
    /// `![alt](assets/name)` on its own line.
    Image,
    /// A 3 x 2 table template on its own block.
    Table,
}

/// What a [`Format`] needs besides the selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatExtra {
    /// Address of a link.
    pub url: Option<String>,
    /// Alternative text of an image; the name when absent.
    pub alt: Option<String>,
    /// File name of an image in `assets/`.
    pub name: Option<String>,
}

/// The text after a format, and where the selection sits in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    /// The new text.
    pub markdown: String,
    /// Start of the selection, a byte offset.
    pub start: usize,
    /// End of the selection, a byte offset.
    pub end: usize,
}

/// Rounds `at` down to a char boundary of `s` no greater than its length.
fn clamp_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Applies `format` to the selection `[start, end)` (byte offsets, in any
/// order, clamped to the text) of `markdown`.
///
/// Toggles: a span that already has the marks loses them; a line format
/// toggles its prefix on every selected line; `**x**` counts as bold and not
/// as italic. Returns `None` for a link without an address and for an image
/// without a name.
#[must_use]
pub fn format_selection(
    markdown: &str,
    start: usize,
    end: usize,
    format: Format,
    extra: &FormatExtra,
) -> Option<Formatted> {
    let from = clamp_boundary(markdown, start.min(end));
    let to = clamp_boundary(markdown, start.max(end));
    match format {
        Format::Bold => Some(wrap(markdown, from, to, "**", "**")),
        Format::Italic => Some(wrap(markdown, from, to, "*", "*")),
        Format::Code => Some(if markdown[from..to].contains('\n') {
            wrap(markdown, from, to, "```\n", "\n```")
        } else {
            wrap(markdown, from, to, "`", "`")
        }),
        Format::Heading => Some(prefix_lines(markdown, from, to, "## ")),
        Format::List => Some(prefix_lines(markdown, from, to, "- ")),
        Format::Quote => Some(prefix_lines(markdown, from, to, "> ")),
        Format::Link => {
            let url = extra
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())?;
            Some(insert_link(markdown, from, to, url))
        }
        Format::Image => {
            let name = extra.name.as_deref().filter(|n| !n.is_empty())?;
            let alt = extra
                .alt
                .as_deref()
                .filter(|a| !a.is_empty())
                .unwrap_or(name);
            Some(insert_block(
                markdown,
                from,
                to,
                &format!("![{alt}](assets/{name})"),
            ))
        }
        Format::Table => Some(insert_table(markdown, from, to)),
    }
}

/// The table [`Format::Table`] inserts: header, separator, two empty rows.
const TABLE_TEMPLATE: &str =
    "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |";

/// The text of the first header cell inside [`TABLE_TEMPLATE`].
const TABLE_FIRST_CELL: &str = "Column 1";

/// Inserts [`TABLE_TEMPLATE`] as its own block and selects its first header
/// cell.
fn insert_table(md: &str, start: usize, end: usize) -> Formatted {
    let block = insert_block(md, start, end, TABLE_TEMPLATE);
    let cell = block.start + TABLE_TEMPLATE.find(TABLE_FIRST_CELL).unwrap_or(0);
    Formatted {
        start: cell,
        end: cell + TABLE_FIRST_CELL.len(),
        ..block
    }
}

/// Wraps the selection in `before` and `after`, or removes that wrap when
/// it is already there (around the selection or inside it). An empty
/// selection leaves the caret between the marks.
fn wrap(md: &str, start: usize, end: usize, before: &str, after: &str) -> Formatted {
    let selected = &md[start..end];
    let bytes = md.as_bytes();
    let around =
        start >= before.len() && md[..start].ends_with(before) && md[end..].starts_with(after);
    // A single star each side of the selection is the inner pair of **bold**.
    let bold_pair = before == "*"
        && start >= 2
        && bytes[start - 2] == b'*'
        && bytes.get(end + 1) == Some(&b'*');
    let inside = selected.len() >= before.len() + after.len()
        && selected.starts_with(before)
        && selected.ends_with(after)
        && !(before == "*" && selected.starts_with("**") && selected.ends_with("**"));
    if around && !bold_pair {
        let at = start - before.len();
        return Formatted {
            markdown: format!("{}{}{}", &md[..at], selected, &md[end + after.len()..]),
            start: at,
            end: at + selected.len(),
        };
    }
    if inside {
        let stripped = &selected[before.len()..selected.len() - after.len()];
        return Formatted {
            markdown: format!("{}{}{}", &md[..start], stripped, &md[end..]),
            start,
            end: start + stripped.len(),
        };
    }
    let at = start + before.len();
    Formatted {
        markdown: format!("{}{before}{selected}{after}{}", &md[..start], &md[end..]),
        start: at,
        end: at + selected.len(),
    }
}

/// Toggles `prefix` on each line the selection touches. An empty line gains
/// the prefix so the caret can start typing after it.
fn prefix_lines(md: &str, start: usize, end: usize, prefix: &str) -> Formatted {
    let line_start = md[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = md[end..].find('\n').map_or(md.len(), |i| end + i);
    let lines: Vec<&str> = md[line_start..line_end].split('\n').collect();
    let on = lines.iter().any(|l| !l.trim().is_empty())
        && lines
            .iter()
            .all(|l| l.trim().is_empty() || l.starts_with(prefix));
    let next = lines
        .iter()
        .map(|line| {
            if on {
                line.strip_prefix(prefix).unwrap_or(line).to_owned()
            } else if line.trim().is_empty() {
                (*line).to_owned()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body = if next.is_empty() {
        prefix.to_owned()
    } else {
        next
    };
    Formatted {
        markdown: format!("{}{}{}", &md[..line_start], body, &md[line_end..]),
        start: line_start,
        end: line_start + body.len(),
    }
}

/// Links the selection, or unwraps it when it already is `[label](address)`.
fn insert_link(md: &str, start: usize, end: usize, url: &str) -> Formatted {
    let selected = &md[start..end];
    if let Some(label) = link_label(selected) {
        return Formatted {
            markdown: format!("{}{}{}", &md[..start], label, &md[end..]),
            start,
            end: start + label.len(),
        };
    }
    let label = if selected.is_empty() {
        "link"
    } else {
        selected
    };
    Formatted {
        markdown: format!("{}[{label}]({url}){}", &md[..start], &md[end..]),
        start: start + 1,
        end: start + 1 + label.len(),
    }
}

/// The label of `text` when all of it is one `[label](address)`.
fn link_label(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('[')?.strip_suffix(')')?;
    let at = inner.rfind("](")?;
    if inner[at + 2..].contains(')') {
        return None;
    }
    Some(&inner[..at])
}

/// Replaces the selection with `block`, on a line of its own.
fn insert_block(md: &str, start: usize, end: usize, block: &str) -> Formatted {
    let head = &md[..start];
    let lead = if start == 0 || head.ends_with("\n\n") {
        ""
    } else if head.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let tail = if end < md.len() && !md[end..].starts_with('\n') {
        "\n\n"
    } else {
        ""
    };
    let at = start + lead.len();
    Formatted {
        markdown: format!("{head}{lead}{block}{tail}{}", &md[end..]),
        start: at,
        end: at + block.len(),
    }
}

// ---------------------------------------------------------------------------
// Names and places
// ---------------------------------------------------------------------------

/// The stem of the file for `title`: lower case, runs of anything but
/// `a-z0-9` as one hyphen, no hyphen at the ends, `doc` when nothing is left.
#[must_use]
pub fn file_stem(title: &str) -> String {
    let mut stem = String::new();
    let mut pending = false;
    for c in title.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending && !stem.is_empty() {
                stem.push('-');
            }
            pending = false;
            stem.push(c);
        } else {
            pending = true;
        }
    }
    if stem.is_empty() {
        "doc".to_owned()
    } else {
        stem
    }
}

/// The file name of a doc titled `title`: `Release Notes!` is
/// `release-notes.md`.
#[must_use]
pub fn file_name(title: &str) -> String {
    format!("{}.md", file_stem(title))
}

/// [`file_name`] that is not in `taken`: `x.md`, then `x-2.md`, `x-3.md`...
/// Names compare ignoring ASCII case, as a case-insensitive disk would.
#[must_use]
pub fn unique_file_name(title: &str, taken: &[String]) -> String {
    let stem = file_stem(title);
    let free = |name: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    let first = format!("{stem}.md");
    if free(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{stem}-{n}.md"))
        .find(|name| free(name))
        .unwrap_or(first)
}

/// Whether `name` is safe as one file name inside a doc folder: not empty,
/// no separator, not `.` or `..`, no NUL or other control character.
#[must_use]
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.split(['/', '\\']).any(|part| part == "..")
        && !name.chars().any(char::is_control)
}

/// The title of a new doc: `<chat title> · notes`, or `Untitled doc` when
/// the chat has no title (or still has the placeholder `New chat`).
#[must_use]
pub fn default_title(chat_title: Option<&str>) -> String {
    match chat_title.map(str::trim) {
        Some(title) if !title.is_empty() && title != "New chat" => format!("{title} · notes"),
        _ => "Untitled doc".to_owned(),
    }
}

/// The folder of a chat's docs: `<root>/.crest/docs/<session>` for a chat
/// whose project has a first root, `<data dir>/docs/<session>` otherwise.
/// Trailing slashes of the bases are dropped.
#[must_use]
pub fn doc_dir(first_root: Option<&str>, data_dir: &str, session: SessionId) -> String {
    match first_root.map(|r| r.trim_end_matches('/')) {
        Some(root) if !root.is_empty() => format!("{root}/.crest/docs/{session}"),
        _ => format!("{}/docs/{session}", data_dir.trim_end_matches('/')),
    }
}

// ---------------------------------------------------------------------------
// Blocks as markdown
// ---------------------------------------------------------------------------

/// A block as markdown for a doc, or `None` for a kind that does not join a
/// doc.
///
/// `Some` for [`BlockKind::Text`] (the markdown as is; this is also how the
/// person's own message is represented: `prompt_block` in app-core stores it
/// as a `Text` block whose lines are already quoted with `> `, so it needs
/// no variant of its own), [`BlockKind::Table`], [`BlockKind::Chart`],
/// [`BlockKind::Computed`] and [`BlockKind::Diff`], and for the content
/// kinds [`BlockKind::Mermaid`] (a `mermaid` fence), [`BlockKind::Markdown`]
/// (the source as is), [`BlockKind::Shell`] (an `sh` fence with `$ command`,
/// the output and `# exit N` when it failed), [`BlockKind::Error`] (a
/// blockquote), [`BlockKind::Sources`] (a bullet list) and
/// [`BlockKind::Image`] (`![alt](path)`). A chart is
/// `![title](assets/<image>)` when `image` is given and its title and table
/// otherwise; an image block uses `assets/<image>` when `image` is given and
/// its own path otherwise (inline bytes have none, so `None`); `image` is
/// ignored by the other kinds. The conversation chrome (thinking, tool calls,
/// stage headers, forms, plugin blocks) is not document content. The rich
/// kinds without a case here fall back to their copy forms in the UI.
#[must_use]
pub fn block_markdown(kind: &BlockKind, image: Option<&str>) -> Option<String> {
    match kind {
        BlockKind::Text { markdown } => Some(markdown.clone()),
        BlockKind::Table(spec) => {
            let (columns, rows, numeric) = table_parts(spec);
            Some(table_markdown(&columns, &rows, &numeric))
        }
        BlockKind::Chart(spec) => Some(chart_markdown(spec, image)),
        BlockKind::Computed { envelope, .. } => Some(computed_markdown(envelope)),
        BlockKind::Diff(spec) => Some(format!(
            "`{}`\n\n{}",
            spec.path,
            fenced(spec.unified.trim_end_matches('\n'), "diff")
        )),
        BlockKind::Mermaid(spec) => Some(fenced(spec.source.trim_end_matches('\n'), "mermaid")),
        BlockKind::Markdown { source, .. } => Some(source.clone()),
        BlockKind::Shell {
            command,
            exit_code,
            stdout,
            stderr,
            ..
        } => Some(shell_markdown(command, *exit_code, stdout, stderr)),
        BlockKind::Error { message, .. } => Some(quoted(message)),
        BlockKind::Sources { sources } if !sources.is_empty() => Some(
            sources
                .iter()
                .map(source_item)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        BlockKind::Image {
            data_or_path,
            source,
            ..
        } => image_markdown(data_or_path, source.as_ref(), image),
        _ => None,
    }
}

/// A shell run as an `sh` fence: `$ command`, stdout, stderr and a trailing
/// `# exit N` (or `# killed`) when it did not succeed.
fn shell_markdown(command: &str, exit_code: Option<i32>, stdout: &str, stderr: &str) -> String {
    let mut lines = vec![format!("$ {command}")];
    for out in [stdout, stderr] {
        let out = out.trim_end_matches('\n');
        if !out.is_empty() {
            lines.push(out.to_owned());
        }
    }
    match exit_code {
        Some(0) => {}
        Some(code) => lines.push(format!("# exit {code}")),
        None => lines.push("# killed".to_owned()),
    }
    fenced(&lines.join("\n"), "sh")
}

/// `text` as a blockquote, one `> ` per line.
fn quoted(text: &str) -> String {
    text.trim_end_matches('\n')
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A source as a bullet: `[title](link)` when it has both, else its
/// reference line.
fn source_item(source: &SourceRef) -> String {
    let title = match source {
        SourceRef::Url { title, .. } => title.clone().filter(|t| !t.trim().is_empty()),
        SourceRef::Docs { title, .. }
        | SourceRef::Article { title, .. }
        | SourceRef::Paper { title, .. }
        | SourceRef::Standard { title, .. }
        | SourceRef::Book { title, .. } => Some(title.clone()),
        _ => None,
    };
    match (title, source.link()) {
        (Some(title), Some(link)) => {
            format!("- [{}]({link})", title.replace(['[', ']'], ""))
        }
        _ => format!("- {}", source.reference()),
    }
}

fn image_markdown(
    data: &ImageData,
    source: Option<&SourceRef>,
    image: Option<&str>,
) -> Option<String> {
    let target = match (image, data) {
        (Some(name), _) => format!("assets/{name}"),
        (None, ImageData::Path { path }) if !path.trim().is_empty() => path.clone(),
        _ => return None,
    };
    let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_owned();
    let alt = match source {
        Some(SourceRef::Url { title: Some(t), .. }) if !t.trim().is_empty() => t.clone(),
        Some(SourceRef::File { path, .. }) => name(path),
        _ => name(&target),
    };
    Some(format!("![{}]({target})", alt.replace(['[', ']'], "")))
}

/// Whether the domain turns `kind` into markdown for a doc (the UI also
/// offers "Add to doc" for rich kinds through their copy forms, see
/// `ui::blocks::doc_markdown`, so this is not the whole answer there).
#[must_use]
pub fn can_add_to_doc(kind: &BlockKind) -> bool {
    block_markdown(kind, None).is_some()
}

/// A fenced block long enough that `body` cannot close it.
#[must_use]
pub fn fenced(body: &str, info: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{info}\n{body}\n{fence}")
}

fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Bool(b) => b.to_string(),
        Cell::Number(n) => n.to_string(),
        Cell::Text(t) => t.clone(),
    }
}

/// Headers, cell text and the right-aligned flags of a table: a column is
/// numeric when declared right-aligned, or when it has numbers and nothing
/// but numbers and empty cells.
fn table_parts(spec: &TableSpec) -> (Vec<String>, Vec<Vec<String>>, Vec<bool>) {
    let columns = spec.columns.iter().map(|c| c.name.clone()).collect();
    let rows = spec
        .rows
        .iter()
        .map(|row| row.iter().map(cell_text).collect())
        .collect();
    let numeric = spec
        .columns
        .iter()
        .enumerate()
        .map(|(i, column)| match column.align {
            Some(Align::Right) => true,
            Some(_) => false,
            None => {
                let cells = || spec.rows.iter().filter_map(|row| row.get(i));
                cells().any(|c| matches!(c, Cell::Number(_)))
                    && cells().all(|c| matches!(c, Cell::Number(_) | Cell::Null))
            }
        })
        .collect();
    (columns, rows, numeric)
}

fn chart_markdown(spec: &ChartSpec, image: Option<&str>) -> String {
    let title = if spec.title.trim().is_empty() {
        "Chart"
    } else {
        spec.title.as_str()
    };
    if let Some(image) = image {
        return format!("![{title}](assets/{image})");
    }
    let (columns, rows, numeric) = table_parts(&spec.to_table());
    format!(
        "**{title}**\n\n{}",
        table_markdown(&columns, &rows, &numeric)
    )
}

/// The method, where it came from, why, and the headline numbers.
fn computed_markdown(envelope: &crate::computed::Computed) -> String {
    let provenance = [
        Some(envelope.kind.as_str()),
        envelope.provenance.requested_by.as_deref(),
        envelope.provenance.activity_started.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut out = format!("**{}** · {provenance}", envelope.method.name);
    if let Some(why) = envelope.method.why.as_deref().filter(|w| !w.is_empty()) {
        out.push_str(&format!("\n\n{why}"));
    }
    if let Some(error) = envelope.error.as_deref().filter(|e| !e.is_empty()) {
        out.push_str(&format!("\n\nError: {error}"));
    }
    if !envelope.values.is_empty() {
        let values: Vec<String> = envelope
            .values
            .iter()
            .map(|(key, value)| {
                let mut shown = value.display.clone().unwrap_or_else(|| value.v.to_string());
                if let Some(unit) = value.unit.as_deref().filter(|u| !u.is_empty()) {
                    shown.push(' ');
                    shown.push_str(unit);
                }
                match value.label.as_deref().filter(|l| !l.is_empty()) {
                    Some(label) => format!("- {key}: **{shown}** ({label})"),
                    None => format!("- {key}: **{shown}**"),
                }
            })
            .collect();
        out.push_str("\n\n");
        out.push_str(&values.join("\n"));
    }
    out
}

fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// A markdown table. `|` in a cell is escaped; a column flagged in `numeric`
/// is right-aligned.
#[must_use]
pub fn table_markdown(columns: &[String], rows: &[Vec<String>], numeric: &[bool]) -> String {
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    let mut lines = vec![
        line(columns.iter().map(|c| escape_cell(c)).collect()),
        line(
            (0..columns.len())
                .map(|i| {
                    if numeric.get(i).copied().unwrap_or(false) {
                        "---:".to_owned()
                    } else {
                        "---".to_owned()
                    }
                })
                .collect(),
        ),
    ];
    lines.extend(
        rows.iter()
            .map(|row| line(row.iter().map(|c| escape_cell(c)).collect())),
    );
    lines.join("\n")
}

/// CSV of a table; a cell with a comma, a quote or a line break is quoted.
#[must_use]
pub fn table_csv(columns: &[String], rows: &[Vec<String>]) -> String {
    let quote = |cell: &String| {
        if cell.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", cell.replace('"', "\"\""))
        } else {
            cell.clone()
        }
    };
    std::iter::once(columns)
        .chain(rows.iter().map(Vec::as_slice))
        .map(|row| row.iter().map(quote).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses CSV (or TSV with `sep = '\t'`): quoted fields may hold the
/// separator, doubled quotes and line breaks. Blank lines are skipped.
#[must_use]
pub fn parse_delimited(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' if quoted => quoted = false,
            '"' if cell.is_empty() => quoted = true,
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                if row.iter().any(|c| !c.is_empty()) || row.len() > 1 {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// A markdown table from CSV (`sep = ','`) or TSV (`sep = '\t'`) text, the
/// first line being the header; rows are padded or cut to its width. `None`
/// when there is no header or it has a single column (not a table).
#[must_use]
pub fn delimited_to_markdown_table(text: &str, sep: char) -> Option<String> {
    let mut rows = parse_delimited(text, sep).into_iter();
    let columns = rows.next()?;
    if columns.len() < 2 {
        return None;
    }
    let rows: Vec<Vec<String>> = rows
        .map(|mut row| {
            row.resize(columns.len(), String::new());
            row.iter().map(|c| c.replace('\n', " ")).collect()
        })
        .collect();
    let numeric: Vec<bool> = (0..columns.len())
        .map(|i| {
            let cells: Vec<&String> = rows
                .iter()
                .map(|r| &r[i])
                .filter(|c| !c.is_empty())
                .collect();
            !cells.is_empty() && cells.iter().all(|c| c.parse::<f64>().is_ok())
        })
        .collect();
    Some(table_markdown(&columns, &rows, &numeric))
}

fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// `markdown` with every `](assets/<name>)` whose `name` is in `assets`
/// turned into a `data:` URL, so the result is one self-contained file. A
/// name that is not in `assets` stays.
#[must_use]
pub fn inline_assets(markdown: &str, assets: &[(String, Vec<u8>)]) -> String {
    const OPEN: &str = "](assets/";
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(at) = rest.find(OPEN) {
        let name_start = at + OPEN.len();
        let Some(close) = rest[name_start..].find(')') else {
            break;
        };
        let name = &rest[name_start..name_start + close];
        out.push_str(&rest[..at]);
        match assets.iter().find(|(n, _)| n == name) {
            Some((_, bytes)) if !name.is_empty() => {
                out.push_str(&format!(
                    "](data:{};base64,{})",
                    mime_of(name),
                    STANDARD.encode(bytes)
                ));
            }
            _ => out.push_str(&rest[at..=name_start + close]),
        }
        rest = &rest[name_start + close + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Top-level blocks and diagrams
// ---------------------------------------------------------------------------

/// The byte range of one top-level block of a doc, `end` exclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// First byte of the block.
    pub start: usize,
    /// One past the last byte of the block (no trailing newline).
    pub end: usize,
}

/// A line of the text with its byte range, without the line break.
struct Line<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn lines_of(markdown: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for raw in markdown.split_inclusive('\n') {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        let text = text.strip_suffix('\r').unwrap_or(text);
        lines.push(Line {
            start,
            end: start + text.len(),
            text,
        });
        start += raw.len();
    }
    lines
}

/// The marker (`` ``` `` or `~~~`, three or more) and the info string when
/// `line` opens a fence.
fn fence_open(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start();
    let c = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|x| *x == c).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim();
    if c == '`' && info.contains('`') {
        return None;
    }
    Some((c, len, info))
}

/// Whether `line` closes a fence opened with `len` times `c`.
fn fence_close(line: &str, c: char, len: usize) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= len && trimmed.chars().all(|x| x == c)
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes)
        && trimmed[hashes..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Paragraph,
    List,
    Quote,
    Table,
}

fn is_list_item(line: &str) -> bool {
    let t = line.trim_start();
    let after_marker = if let Some(rest) = t.strip_prefix(['-', '*', '+']) {
        Some(rest)
    } else {
        let digits = t.chars().take_while(char::is_ascii_digit).count();
        (digits > 0 && digits <= 9)
            .then(|| t[digits..].strip_prefix(['.', ')']))
            .flatten()
    };
    after_marker.is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn classify(line: &str) -> Class {
    let t = line.trim_start();
    if t.starts_with('>') {
        Class::Quote
    } else if t.starts_with('|') {
        Class::Table
    } else if is_list_item(line) {
        Class::List
    } else {
        Class::Paragraph
    }
}

/// The top-level blocks of `markdown`, in order.
///
/// A fenced block (`` ``` `` or `~~~`, three or more, closed by the same
/// character at least as long; unterminated runs to the end) is one block. A
/// heading line is a block. Consecutive list items (and indented lines under
/// them), quote lines or table lines form one block; consecutive plain lines
/// form a paragraph; a blank line ends a block. Spans leave out the blank
/// lines between blocks.
#[must_use]
pub fn split_blocks(markdown: &str) -> Vec<Span> {
    let lines = lines_of(markdown);
    let mut spans = Vec::new();
    let mut open: Option<(usize, usize, Class)> = None; // start, end, class
    let mut i = 0;
    let flush = |open: &mut Option<(usize, usize, Class)>, spans: &mut Vec<Span>| {
        if let Some((start, end, _)) = open.take() {
            spans.push(Span { start, end });
        }
    };
    while i < lines.len() {
        let line = &lines[i];
        if line.text.trim().is_empty() {
            flush(&mut open, &mut spans);
        } else if let Some((c, len, _)) = fence_open(line.text) {
            flush(&mut open, &mut spans);
            let mut last = i;
            for (j, next) in lines.iter().enumerate().skip(i + 1) {
                last = j;
                if fence_close(next.text, c, len) {
                    break;
                }
            }
            spans.push(Span {
                start: line.start,
                end: lines[last].end,
            });
            i = last;
        } else if is_heading(line.text) {
            flush(&mut open, &mut spans);
            spans.push(Span {
                start: line.start,
                end: line.end,
            });
        } else {
            let class = classify(line.text);
            let indented = line.text.starts_with([' ', '\t']);
            match open {
                Some((_, ref mut end, kind))
                    if kind == class || (kind == Class::List && indented) =>
                {
                    *end = line.end;
                }
                _ => {
                    flush(&mut open, &mut spans);
                    open = Some((line.start, line.end, class));
                }
            }
        }
        i += 1;
    }
    flush(&mut open, &mut spans);
    spans
}

/// The fenced blocks whose info string starts with `mermaid` (any case),
/// the whole fence included, in order.
#[must_use]
pub fn diagram_spans(markdown: &str) -> Vec<Span> {
    split_blocks(markdown)
        .into_iter()
        .filter(|span| {
            let first = markdown[span.start..span.end].lines().next().unwrap_or("");
            fence_open(first).is_some_and(|(_, _, info)| {
                info.get(..7)
                    .is_some_and(|head| head.eq_ignore_ascii_case("mermaid"))
            })
        })
        .collect()
}

/// The pieces of a fenced block: its opening line, body and closing line
/// (`None` when the fence is not terminated), as byte ranges of `markdown`.
fn fence_parts<'a>(
    markdown: &'a str,
    span: &Span,
) -> Option<(Line<'a>, (usize, usize), Option<Line<'a>>)> {
    let text = markdown.get(span.start..span.end)?;
    let mut lines = lines_of(text);
    if lines.is_empty() {
        return None;
    }
    let shift = |l: Line<'_>| Line {
        start: l.start + span.start,
        end: l.end + span.start,
        text: &markdown[l.start + span.start..l.end + span.start],
    };
    let first = shift(lines.remove(0));
    let (c, len, _) = fence_open(first.text)?;
    let closer = match lines.last() {
        Some(last) if fence_close(last.text, c, len) => lines.pop().map(shift),
        _ => None,
    };
    let body = match (lines.first(), lines.last()) {
        (Some(a), Some(b)) => (a.start + span.start, b.end + span.start),
        _ => (first.end, first.end),
    };
    Some((first, body, closer))
}

/// The body of the fenced block at `span`, without the fence lines,
/// trimmed. Empty when `span` is not a fence.
#[must_use]
pub fn diagram_source(markdown: &str, span: &Span) -> String {
    fence_parts(markdown, span).map_or_else(String::new, |(_, (a, b), _)| {
        markdown[a..b].trim().to_owned()
    })
}

/// `markdown` with the body of its `index`-th diagram replaced by
/// `source.trim()`. An empty source removes the whole block and one blank
/// line with it. `None` when there is no such diagram.
#[must_use]
pub fn replace_diagram(markdown: &str, index: usize, source: &str) -> Option<String> {
    let spans = diagram_spans(markdown);
    let span = spans.get(index)?;
    let body = source.trim();
    if body.is_empty() {
        let before = markdown[..span.start].trim_end_matches(['\n', '\r']);
        let after = markdown[span.end..].trim_start_matches(['\n', '\r']);
        return Some(match (before.is_empty(), after.is_empty()) {
            (true, _) => after.to_owned(),
            (false, true) => format!("{before}\n"),
            (false, false) => format!("{before}\n\n{after}"),
        });
    }
    let (opener, _, closer) = fence_parts(markdown, span)?;
    let closer_text = match closer {
        Some(line) => line.text.to_owned(),
        None => {
            let (c, len, _) = fence_open(opener.text)?;
            c.to_string().repeat(len)
        }
    };
    Some(format!(
        "{}{}\n{body}\n{closer_text}{}",
        &markdown[..opener.start],
        opener.text,
        &markdown[span.end..]
    ))
}

/// `markdown` with a `mermaid` fence holding `source.trim()` as its own
/// block after top-level block `block_index`. `None`, or an index past the
/// last block, puts it at the end; an empty doc becomes the fence. A blank
/// `source` leaves the text unchanged.
#[must_use]
pub fn insert_diagram_after(markdown: &str, block_index: Option<usize>, source: &str) -> String {
    let body = source.trim();
    if body.is_empty() {
        return markdown.to_owned();
    }
    let fence = fenced(body, "mermaid");
    let spans = split_blocks(markdown);
    match block_index.filter(|i| *i < spans.len().saturating_sub(1)) {
        Some(i) => {
            let before = &markdown[..spans[i].end];
            let after = markdown[spans[i].end..].trim_start_matches(['\n', '\r']);
            format!("{before}\n\n{fence}\n\n{after}")
        }
        None => append_markdown(markdown, &fence),
    }
}

/// The index of the top-level block that holds byte `offset`, or else of the
/// last block that starts before it. `None` when the doc has no block or
/// `offset` is before the first one.
#[must_use]
pub fn block_at(markdown: &str, offset: usize) -> Option<usize> {
    split_blocks(markdown)
        .iter()
        .rposition(|span| span.start <= offset)
}

/// The starting points of the diagram editor: name and Mermaid source.
pub const DIAGRAM_TEMPLATES: &[(&str, &str)] = &[
    (
        "Flowchart",
        "flowchart LR\n  start[Idea] --> draft[Draft]\n  draft --> review{Review}\n  review -->|ok| publish[Publish]\n  review -->|changes| draft",
    ),
    (
        "Sequence",
        "sequenceDiagram\n  participant P as Person\n  participant A as Agent\n  P->>A: Write the summary\n  A-->>P: Draft ready",
    ),
    (
        "Pie",
        "pie title Time spent\n  \"Research\" : 40\n  \"Writing\" : 35\n  \"Review\" : 25",
    ),
    (
        "Timeline",
        "timeline\n  title Release\n  Week 1 : Research\n  Week 2 : Draft : Review\n  Week 3 : Publish",
    ),
    (
        "Use case",
        "usecase-beta\ndirection LR\nactor Customer(\"Customer\")\nsystemBoundary \"Order system\"\n  Checkout(\"Place order\")\nend\nCustomer --> Checkout",
    ),
    (
        "Bar chart",
        "xychart-beta\n  title \"Files changed by crate\"\n  x-axis [ui, app, adapters, domain, docs]\n  y-axis \"Lines\" 0 --> 2500\n  bar [2100, 1500, 760, 410, 230]",
    ),
];

// ---------------------------------------------------------------------------
// The /doc command
// ---------------------------------------------------------------------------

/// A format a doc can be exported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// One `.md` file with the images inlined.
    Markdown,
    /// A web page with the print stylesheet.
    Html,
    /// The web page opened in the browser with its print dialog.
    Pdf,
    /// The markdown on the clipboard.
    Copy,
}

impl ExportFormat {
    /// The format named by `word` (`md`, `markdown`, `html`, `pdf`, `copy`; any
    /// case).
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "html" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            "copy" => Some(Self::Copy),
            _ => None,
        }
    }

    /// The name shown in menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Html => "Web page",
            Self::Pdf => "PDF",
            Self::Copy => "Copy",
        }
    }

    /// The file extension, `None` for the clipboard.
    #[must_use]
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Markdown => Some("md"),
            Self::Html => Some("html"),
            Self::Pdf => Some("html"),
            Self::Copy => None,
        }
    }
}

/// What `/doc ...` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocCommand {
    /// `/doc`: this chat's doc.
    Open,
    /// `/doc new [title]`.
    New(Option<String>),
    /// `/doc find <text>`.
    Find(String),
    /// `/doc open <text>` and a bare `/doc <text>`.
    OpenTitle(String),
    /// `/doc use [title]`.
    Use(Option<String>),
    /// `/doc add`.
    Add,
    /// `/doc split`.
    Split,
    /// `/doc only`.
    Only,
    /// `/doc close`.
    Close,
    /// `/doc source`.
    Source,
    /// `/doc export [md|html|pdf|copy]`; `None` for no word or an unknown one,
    /// which the handler reports as a usage error.
    Export(Option<ExportFormat>),
    /// The command is malformed; the text says how to write it.
    Usage(&'static str),
}

impl DocCommand {
    /// Parses what follows `/doc`.
    #[must_use]
    pub fn parse(args: &str) -> Self {
        let args = args.trim();
        let (sub, rest) = args
            .split_once(char::is_whitespace)
            .map_or((args, ""), |(s, r)| (s, r.trim()));
        let rest_opt = || (!rest.is_empty()).then(|| rest.to_owned());
        match sub.to_ascii_lowercase().as_str() {
            "" => Self::Open,
            "new" => Self::New(rest_opt()),
            "find" => rest_opt().map_or(Self::Usage("usage: /doc find <text>"), Self::Find),
            "open" => rest_opt().map_or(Self::Usage("usage: /doc open <text>"), Self::OpenTitle),
            "use" => Self::Use(rest_opt()),
            "add" => Self::Add,
            "split" => Self::Split,
            "only" => Self::Only,
            "close" => Self::Close,
            "source" => Self::Source,
            "export" => Self::Export(ExportFormat::parse(rest)),
            _ => Self::OpenTitle(args.to_owned()),
        }
    }
}

/// The docs whose title contains `query` (any case), most recently changed
/// first; an empty query matches all.
#[must_use]
pub fn match_docs<'a>(docs: &'a [Doc], query: &str) -> Vec<&'a Doc> {
    let query = query.trim().to_lowercase();
    let mut hits: Vec<&Doc> = docs
        .iter()
        .filter(|d| query.is_empty() || d.title.to_lowercase().contains(&query))
        .collect();
    hits.sort_by_key(|d| std::cmp::Reverse(d.updated_at));
    hits
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::too_many_lines)]
mod tests {
    use super::*;
    use crate::block::{ChartKind, Column, DiffSpec, MermaidSpec, Series, ToolStatus};
    use crate::computed::{CalcSheetBody, ComputedBody, ComputedValue, Engine};

    fn extra() -> FormatExtra {
        FormatExtra::default()
    }

    fn fmt(md: &str, a: usize, b: usize, f: Format) -> Formatted {
        format_selection(md, a, b, f, &extra()).unwrap()
    }

    fn sel(f: &Formatted) -> &str {
        &f.markdown[f.start..f.end]
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    // --- append_markdown ---

    #[test]
    fn append_to_an_empty_doc_is_the_addition() {
        assert_eq!(append_markdown("", "  hello  "), "hello\n");
        assert_eq!(append_markdown("\n\n", "x"), "x\n");
    }

    #[test]
    fn append_to_a_doc_leaves_one_blank_line() {
        assert_eq!(append_markdown("one\n", "two"), "one\n\ntwo\n");
        assert_eq!(append_markdown("one\n\n\n", "\ntwo\n\n"), "one\n\ntwo\n");
        assert_eq!(append_markdown("one", "two"), "one\n\ntwo\n");
    }

    // --- find_span / replace_span ---

    #[test]
    fn find_span_prefers_the_exact_text() {
        let md = "alpha beta gamma";
        assert_eq!(find_span(md, "beta"), Some((6, 10)));
        assert_eq!(find_span(md, ""), None);
        assert_eq!(find_span(md, "   "), None);
    }

    #[test]
    fn find_span_sees_through_marks() {
        let md = "Say **bold move** and `code bit` now";
        let (a, b) = find_span(md, "bold move and code bit").unwrap();
        assert_eq!(&md[a..b], "**bold move** and `code bit`");
        let (a, b) = find_span("a **bo**ld word", "bold word").unwrap();
        assert_eq!(&"a **bo**ld word"[a..b], "**bo**ld word");
    }

    #[test]
    fn find_span_sees_through_line_breaks_and_prefixes() {
        let md = "intro\n\n- first item\n- second item\n\n# Title\ntext";
        let (a, b) = find_span(md, "first item second item").unwrap();
        assert_eq!(&md[a..b], "first item\n- second item");
        let (a, b) = find_span(md, "second item Title").unwrap();
        assert_eq!(&md[a..b], "second item\n\n# Title");
        let md = "1. one\n2. two";
        let (a, b) = find_span(md, "one two").unwrap();
        assert_eq!(&md[a..b], "one\n2. two");
        let md = "> quoted\n> lines";
        let (a, b) = find_span(md, "quoted lines").unwrap();
        assert_eq!(&md[a..b], "quoted\n> lines");
    }

    #[test]
    fn find_span_misses_an_absent_passage() {
        assert_eq!(find_span("alpha beta", "gamma"), None);
        assert_eq!(find_span("alpha beta", "beta alpha"), None);
        assert_eq!(find_span("", "x"), None);
    }

    #[test]
    fn find_span_handles_non_ascii() {
        let md = "El **niño** corre";
        let (a, b) = find_span(md, "niño corre").unwrap();
        assert_eq!(&md[a..b], "**niño** corre");
    }

    #[test]
    fn replace_span_replaces_or_leaves_the_text() {
        assert_eq!(replace_span("a b c", "b", "X"), "a X c");
        assert_eq!(replace_span("a **b** c", "b c", "X"), "a X");
        assert_eq!(replace_span("a b c", "zzz", "X"), "a b c");
    }

    // --- format_selection ---

    #[test]
    fn bold_wraps_and_toggles() {
        let f = fmt("a word b", 2, 6, Format::Bold);
        assert_eq!(f.markdown, "a **word** b");
        assert_eq!(sel(&f), "word");
        // Selection is the word: marks around it come off.
        let g = fmt(&f.markdown, f.start, f.end, Format::Bold);
        assert_eq!(g.markdown, "a word b");
        assert_eq!(sel(&g), "word");
        // Selection includes the marks.
        let h = fmt("a **word** b", 2, 10, Format::Bold);
        assert_eq!(h.markdown, "a word b");
        assert_eq!(sel(&h), "word");
    }

    #[test]
    fn empty_selection_leaves_the_caret_between_marks() {
        let f = fmt("ab", 1, 1, Format::Bold);
        assert_eq!(f.markdown, "a****b");
        assert_eq!((f.start, f.end), (3, 3));
        let g = fmt("ab", 1, 1, Format::Code);
        assert_eq!(g.markdown, "a``b");
        assert_eq!((g.start, g.end), (2, 2));
    }

    #[test]
    fn italic_wraps_and_toggles() {
        let f = fmt("a word", 2, 6, Format::Italic);
        assert_eq!(f.markdown, "a *word*");
        let g = fmt(&f.markdown, f.start, f.end, Format::Italic);
        assert_eq!(g.markdown, "a word");
    }

    #[test]
    fn double_stars_are_bold_not_italic() {
        // Selecting x inside **x**: italic adds a pair instead of removing one.
        let f = fmt("**x**", 2, 3, Format::Italic);
        assert_eq!(f.markdown, "***x***");
        // Selecting the whole **x** with italic also wraps.
        let g = fmt("**x**", 0, 5, Format::Italic);
        assert_eq!(g.markdown, "***x***");
        // And bold on it still removes.
        let h = fmt("**x**", 2, 3, Format::Bold);
        assert_eq!(h.markdown, "x");
    }

    #[test]
    fn code_uses_a_fence_for_several_lines() {
        let f = fmt("x\ny", 0, 3, Format::Code);
        assert_eq!(f.markdown, "```\nx\ny\n```");
        assert_eq!(sel(&f), "x\ny");
        let g = fmt(&f.markdown, f.start, f.end, Format::Code);
        assert_eq!(g.markdown, "x\ny");
        let h = fmt("a b", 0, 1, Format::Code);
        assert_eq!(h.markdown, "`a` b");
        let i = fmt(&h.markdown, h.start, h.end, Format::Code);
        assert_eq!(i.markdown, "a b");
    }

    #[test]
    fn heading_list_and_quote_toggle_on_every_line() {
        for (format, prefix) in [
            (Format::Heading, "## "),
            (Format::List, "- "),
            (Format::Quote, "> "),
        ] {
            let md = "one\ntwo\n\nthree";
            let f = fmt(md, 1, 6, format);
            assert_eq!(f.markdown, format!("{prefix}one\n{prefix}two\n\nthree"));
            assert_eq!(f.start, 0);
            assert_eq!(sel(&f), format!("{prefix}one\n{prefix}two"));
            let g = fmt(&f.markdown, f.start, f.end, format);
            assert_eq!(g.markdown, md);
        }
    }

    #[test]
    fn a_mixed_selection_prefixes_the_lines_that_lack_it() {
        let f = fmt("- a\nb", 0, 5, Format::List);
        assert_eq!(f.markdown, "- - a\n- b");
    }

    #[test]
    fn line_formats_on_an_empty_line_add_the_prefix() {
        let f = fmt("a\n\nb", 2, 2, Format::Heading);
        assert_eq!(f.markdown, "a\n## \nb");
        assert_eq!((f.start, f.end), (2, 5));
    }

    #[test]
    fn line_format_in_the_middle_of_a_line_takes_the_whole_line() {
        let f = fmt("first\nsecond line\nthird", 9, 9, Format::Quote);
        assert_eq!(f.markdown, "first\n> second line\nthird");
    }

    #[test]
    fn link_needs_an_address() {
        assert_eq!(format_selection("x", 0, 1, Format::Link, &extra()), None);
        let blank = FormatExtra {
            url: Some("  ".into()),
            ..FormatExtra::default()
        };
        assert_eq!(format_selection("x", 0, 1, Format::Link, &blank), None);
    }

    #[test]
    fn link_wraps_and_unwraps() {
        let e = FormatExtra {
            url: Some("https://a.b".into()),
            ..FormatExtra::default()
        };
        let f = format_selection("see docs now", 4, 8, Format::Link, &e).unwrap();
        assert_eq!(f.markdown, "see [docs](https://a.b) now");
        assert_eq!(sel(&f), "docs");
        let g = format_selection(&f.markdown, 4, 23, Format::Link, &e).unwrap();
        assert_eq!(g.markdown, "see docs now");
        assert_eq!(sel(&g), "docs");
        // An empty selection inserts a placeholder label, selected.
        let h = format_selection("ab", 1, 1, Format::Link, &e).unwrap();
        assert_eq!(h.markdown, "a[link](https://a.b)b");
        assert_eq!(sel(&h), "link");
    }

    #[test]
    fn image_goes_on_a_line_of_its_own() {
        let e = FormatExtra {
            name: Some("pic.png".into()),
            alt: Some("A pic".into()),
            ..FormatExtra::default()
        };
        let f = format_selection("before after", 7, 7, Format::Image, &e).unwrap();
        assert_eq!(f.markdown, "before \n\n![A pic](assets/pic.png)\n\nafter");
        assert_eq!(sel(&f), "![A pic](assets/pic.png)");
        // At the start, on an empty doc, after a line break.
        let g = format_selection("", 0, 0, Format::Image, &e).unwrap();
        assert_eq!(g.markdown, "![A pic](assets/pic.png)");
        let h = format_selection("a\n", 2, 2, Format::Image, &e).unwrap();
        assert_eq!(h.markdown, "a\n\n![A pic](assets/pic.png)");
        let i = format_selection("a\n\n", 3, 3, Format::Image, &e).unwrap();
        assert_eq!(i.markdown, "a\n\n![A pic](assets/pic.png)");
        // The name is the alt text when there is none; no name, no image.
        let n = FormatExtra {
            name: Some("p.png".into()),
            ..FormatExtra::default()
        };
        let j = format_selection("", 0, 0, Format::Image, &n).unwrap();
        assert_eq!(j.markdown, "![p.png](assets/p.png)");
        assert_eq!(format_selection("", 0, 0, Format::Image, &extra()), None);
    }

    #[test]
    fn table_inserts_a_template_block_and_selects_the_first_header_cell() {
        let t = "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |";
        let e = extra();
        let f = format_selection("", 0, 0, Format::Table, &e).unwrap();
        assert_eq!(f.markdown, t);
        assert_eq!(sel(&f), "Column 1");
        let g = format_selection("before after", 7, 7, Format::Table, &e).unwrap();
        assert_eq!(g.markdown, format!("before \n\n{t}\n\nafter"));
        assert_eq!(sel(&g), "Column 1");
        assert_eq!(g.start, "before \n\n| ".len());
        let h = format_selection("end", 3, 3, Format::Table, &e).unwrap();
        assert_eq!(h.markdown, format!("end\n\n{t}"));
        assert_eq!(sel(&h), "Column 1");
        let i = format_selection("a\n", 2, 2, Format::Table, &e).unwrap();
        assert_eq!(i.markdown, format!("a\n\n{t}"));
    }

    #[test]
    fn offsets_are_clamped_swapped_and_snapped() {
        let f = fmt("héllo", 6, 1, Format::Bold);
        // start 1 is inside no char (h is 1 byte); end 6 is len.
        assert_eq!(f.markdown, "h**éllo**");
        // 2 is inside the two-byte é: snaps down to 1.
        let g = fmt("héllo", 2, 99, Format::Bold);
        assert_eq!(g.markdown, "h**éllo**");
        let h = fmt("ab", 50, 60, Format::Bold);
        assert_eq!(h.markdown, "ab****");
    }

    // --- names ---

    #[test]
    fn file_names_come_from_titles() {
        assert_eq!(file_name("Release Notes!"), "release-notes.md");
        assert_eq!(file_stem("  --Hello,   World--  "), "hello-world");
        assert_eq!(file_name(""), "doc.md");
        assert_eq!(file_name("!!!"), "doc.md");
        assert_eq!(file_stem("v2.0 Plan"), "v2-0-plan");
    }

    #[test]
    fn same_titles_get_numbered_names() {
        let mut taken: Vec<String> = Vec::new();
        for expected in ["notes.md", "notes-2.md", "notes-3.md"] {
            let name = unique_file_name("Notes", &taken);
            assert_eq!(name, expected);
            taken.push(name);
        }
        assert_eq!(unique_file_name("", &[]), "doc.md");
        assert_eq!(unique_file_name("", &strings(&["doc.md"])), "doc-2.md");
        assert_eq!(unique_file_name("a", &strings(&["A.md"])), "a-2.md");
        assert_eq!(unique_file_name("b", &strings(&["a.md"])), "b.md");
    }

    #[test]
    fn unsafe_names_are_refused() {
        for bad in ["", ".", "..", "a/b", "a\\b", "../x", "a\0b", "a\nb", "/"] {
            assert!(!is_safe_name(bad), "{bad:?}");
        }
        for good in ["notes.md", "a b.md", "x-2.md", "a..b", "pic.png"] {
            assert!(is_safe_name(good), "{good:?}");
        }
    }

    #[test]
    fn default_titles() {
        assert_eq!(default_title(Some("Plan")), "Plan · notes");
        assert_eq!(default_title(Some("  Plan ")), "Plan · notes");
        assert_eq!(default_title(Some("New chat")), "Untitled doc");
        assert_eq!(default_title(Some("  ")), "Untitled doc");
        assert_eq!(default_title(None), "Untitled doc");
    }

    #[test]
    fn doc_dir_for_project_and_standalone_chats() {
        let s = SessionId::new();
        assert_eq!(
            doc_dir(Some("/work/app"), "/data", s),
            format!("/work/app/.crest/docs/{s}")
        );
        assert_eq!(
            doc_dir(Some("/work/app///"), "/data/", s),
            format!("/work/app/.crest/docs/{s}")
        );
        assert_eq!(doc_dir(None, "/data/", s), format!("/data/docs/{s}"));
        assert_eq!(doc_dir(Some(""), "/data", s), format!("/data/docs/{s}"));
        assert_eq!(doc_dir(Some("/"), "/data", s), format!("/data/docs/{s}"));
    }

    // --- block_markdown ---

    fn table() -> TableSpec {
        TableSpec {
            columns: vec![
                Column::new("Name"),
                Column::new("Qty"),
                Column {
                    name: "Price".into(),
                    align: Some(Align::Right),
                },
            ],
            rows: vec![
                vec![
                    Cell::Text("a|b".into()),
                    Cell::Number(3.0),
                    Cell::Text("1.5".into()),
                ],
                vec![Cell::Bool(true), Cell::Null, Cell::Null],
            ],
        }
    }

    fn chart() -> ChartSpec {
        ChartSpec {
            title: "Sales".into(),
            kind: ChartKind::Bar,
            labels: vec!["Q1".into(), "Q2".into()],
            series: vec![
                Series {
                    name: "EU".into(),
                    values: vec![1.0, 2.5],
                },
                Series {
                    name: "US".into(),
                    values: vec![3.0, 4.0],
                },
            ],
        }
    }

    fn computed_kind() -> BlockKind {
        let mut envelope = crate::computed::Computed::new(
            "c1",
            "stats.ttest_welch",
            "Welch t-test",
            Engine {
                name: "crest-stats".into(),
                version: "1".into(),
                backend: None,
                platform: None,
            },
        );
        envelope.method.why = Some("unequal variances".into());
        envelope.provenance.requested_by = Some("model".into());
        envelope.values.insert(
            "p".into(),
            ComputedValue {
                display: Some("0.01".into()),
                ..ComputedValue::new(0.0123)
            },
        );
        envelope.values.insert(
            "t".into(),
            ComputedValue {
                unit: Some("sd".into()),
                label: Some("large".into()),
                ..ComputedValue::new(-3.5)
            },
        );
        BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(ComputedBody::CalcSheet(CalcSheetBody { lines: Vec::new() })),
        }
    }

    fn diff_kind() -> BlockKind {
        BlockKind::Diff(DiffSpec {
            path: "src/a.rs".into(),
            old_text: None,
            new_text: "x".into(),
            unified: "-a\n+b\n".into(),
            approval: None,
        })
    }

    #[test]
    fn block_markdown_is_some_for_the_six_kinds() {
        let text = BlockKind::Text {
            markdown: "# Hi".into(),
        };
        assert_eq!(block_markdown(&text, None).as_deref(), Some("# Hi"));
        // The person's own message is a Text block of quoted lines.
        let user = BlockKind::Text {
            markdown: "> one\n> two\n".into(),
        };
        assert_eq!(
            block_markdown(&user, None).as_deref(),
            Some("> one\n> two\n")
        );
        assert!(block_markdown(&BlockKind::Table(table()), None).is_some());
        assert!(block_markdown(&BlockKind::Chart(chart()), None).is_some());
        assert!(block_markdown(&computed_kind(), None).is_some());
        assert!(block_markdown(&diff_kind(), None).is_some());
        for kind in [&text, &user, &computed_kind(), &diff_kind()] {
            assert!(can_add_to_doc(kind));
        }
        assert!(can_add_to_doc(&BlockKind::Table(table())));
        assert!(can_add_to_doc(&BlockKind::Chart(chart())));
    }

    fn shell(exit_code: Option<i32>, stdout: &str, stderr: &str) -> BlockKind {
        BlockKind::Shell {
            command: "ls -l".into(),
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
            truncated: false,
        }
    }

    #[test]
    fn mermaid_is_a_mermaid_fence_the_source_cannot_close() {
        let kind = |source: &str| {
            BlockKind::Mermaid(MermaidSpec {
                source: source.into(),
                title: None,
            })
        };
        assert_eq!(
            block_markdown(&kind("flowchart LR\n a-->b\n"), None).as_deref(),
            Some("```mermaid\nflowchart LR\n a-->b\n```")
        );
        let md = block_markdown(&kind("a[\"```\"]"), None).unwrap();
        assert!(
            md.starts_with("````mermaid\n") && md.ends_with("\n````"),
            "{md}"
        );
        assert!(can_add_to_doc(&kind("x")));
    }

    #[test]
    fn markdown_block_goes_in_as_is() {
        let kind = BlockKind::Markdown {
            title: Some("T".into()),
            source: "# T\n\ntext".into(),
        };
        assert_eq!(block_markdown(&kind, None).as_deref(), Some("# T\n\ntext"));
    }

    #[test]
    fn shell_is_a_console_fence_with_the_exit_code_when_it_failed() {
        assert_eq!(
            block_markdown(&shell(Some(0), "a\nb\n", ""), None).as_deref(),
            Some("```sh\n$ ls -l\na\nb\n```")
        );
        assert_eq!(
            block_markdown(&shell(Some(2), "", "no such file\n"), None).as_deref(),
            Some("```sh\n$ ls -l\nno such file\n# exit 2\n```")
        );
        assert_eq!(
            block_markdown(&shell(None, "", ""), None).as_deref(),
            Some("```sh\n$ ls -l\n# killed\n```")
        );
        let md = block_markdown(&shell(Some(0), "```\nx\n```", ""), None).unwrap();
        assert!(md.starts_with("````sh\n") && md.ends_with("\n````"), "{md}");
    }

    #[test]
    fn error_is_a_blockquote() {
        let kind = BlockKind::Error {
            message: "boom\n\nsecond".into(),
            hint: Some("retry".into()),
        };
        assert_eq!(
            block_markdown(&kind, None).as_deref(),
            Some("> boom\n>\n> second")
        );
    }

    #[test]
    fn sources_are_a_bullet_list() {
        let kind = BlockKind::Sources {
            sources: vec![
                SourceRef::Url {
                    url: "https://a.dev".into(),
                    title: Some("A [site]".into()),
                },
                SourceRef::File {
                    path: "src/lib.rs".into(),
                    line_start: Some(3),
                    line_end: Some(9),
                },
                SourceRef::Url {
                    url: "https://b.dev".into(),
                    title: None,
                },
            ],
        };
        assert_eq!(
            block_markdown(&kind, None).as_deref(),
            Some("- [A site](https://a.dev)\n- src/lib.rs:3-9\n- https://b.dev")
        );
        assert_eq!(
            block_markdown(&BlockKind::Sources { sources: vec![] }, None),
            None
        );
    }

    #[test]
    fn image_uses_the_given_asset_or_its_path() {
        let kind = BlockKind::Image {
            media_type: "image/png".into(),
            data_or_path: ImageData::Path {
                path: "/tmp/shots/a.png".into(),
            },
            source: None,
        };
        assert_eq!(
            block_markdown(&kind, None).as_deref(),
            Some("![a.png](/tmp/shots/a.png)")
        );
        assert_eq!(
            block_markdown(&kind, Some("img-1.png")).as_deref(),
            Some("![img-1.png](assets/img-1.png)")
        );
        let inline = BlockKind::Image {
            media_type: "image/png".into(),
            data_or_path: ImageData::Base64 {
                data: "AA==".into(),
            },
            source: None,
        };
        assert_eq!(block_markdown(&inline, None), None);
        assert_eq!(
            block_markdown(&inline, Some("i.png")).as_deref(),
            Some("![i.png](assets/i.png)")
        );
    }

    #[test]
    fn delimited_text_becomes_a_markdown_table() {
        let csv = "name,n\n\"a, b\",1\n\"say \"\"hi\"\"\",2.5\nshort\n";
        assert_eq!(
            delimited_to_markdown_table(csv, ',').as_deref(),
            Some("| name | n |\n| --- | ---: |\n| a, b | 1 |\n| say \"hi\" | 2.5 |\n| short |  |")
        );
        assert_eq!(
            delimited_to_markdown_table("a\tb\n1\t2", '\t').as_deref(),
            Some("| a | b |\n| ---: | ---: |\n| 1 | 2 |")
        );
        assert_eq!(delimited_to_markdown_table("one\ntwo", ','), None);
        assert_eq!(delimited_to_markdown_table("", ','), None);
        let round = table_csv(&strings(&["a", "b"]), &[strings(&["x,\ny", "\"q\""])]);
        assert_eq!(
            parse_delimited(&round, ','),
            vec![strings(&["a", "b"]), strings(&["x,\ny", "\"q\""])]
        );
    }

    #[test]
    fn block_markdown_is_none_for_other_kinds() {
        let others = [
            BlockKind::Thinking {
                text: "hmm".into(),
                collapsed: true,
            },
            BlockKind::StageHeader {
                stage: "plan".into(),
                index: 1,
                total: 2,
            },
            BlockKind::ToolCall {
                tool_use_id: "t".into(),
                name: "read".into(),
                summary: "s".into(),
                input: serde_json::Value::Null,
                status: ToolStatus::Succeeded,
                output: None,
            },
            BlockKind::Form(crate::form::FormSpec {
                title: "t".into(),
                description: None,
                fields: Vec::new(),
                submit_label: None,
                state: Default::default(),
                answer: None,
                reply: None,
            }),
            BlockKind::Custom {
                plugin: "p".into(),
                name: "n".into(),
                tree: crate::block::CustomNode::Text { text: "x".into() },
            },
        ];
        for kind in &others {
            assert_eq!(block_markdown(kind, None), None, "{kind:?}");
            assert!(!can_add_to_doc(kind));
        }
    }

    #[test]
    fn a_table_block_escapes_pipes_and_right_aligns_numbers() {
        let md = block_markdown(&BlockKind::Table(table()), None).unwrap();
        assert_eq!(
            md,
            "| Name | Qty | Price |\n| --- | ---: | ---: |\n| a\\|b | 3 | 1.5 |\n| true |  |  |"
        );
    }

    #[test]
    fn a_chart_block_is_an_image_or_a_table() {
        let kind = BlockKind::Chart(chart());
        assert_eq!(
            block_markdown(&kind, Some("sales.png")).as_deref(),
            Some("![Sales](assets/sales.png)")
        );
        assert_eq!(
            block_markdown(&kind, None).as_deref(),
            Some(
                "**Sales**\n\n|  | EU | US |\n| --- | ---: | ---: |\n| Q1 | 1 | 3 |\n| Q2 | 2.5 | 4 |"
            )
        );
        let mut untitled = chart();
        untitled.title = " ".into();
        assert_eq!(
            block_markdown(&BlockKind::Chart(untitled), Some("c.png")).as_deref(),
            Some("![Chart](assets/c.png)")
        );
    }

    #[test]
    fn a_diff_block_is_a_diff_fence() {
        assert_eq!(
            block_markdown(&diff_kind(), None).as_deref(),
            Some("`src/a.rs`\n\n```diff\n-a\n+b\n```")
        );
        let tricky = BlockKind::Diff(DiffSpec {
            path: "a.md".into(),
            old_text: None,
            new_text: String::new(),
            unified: "+```\n".into(),
            approval: None,
        });
        assert_eq!(
            block_markdown(&tricky, None).as_deref(),
            Some("`a.md`\n\n````diff\n+```\n````")
        );
    }

    #[test]
    fn a_computed_block_lists_method_and_values() {
        let md = block_markdown(&computed_kind(), None).unwrap();
        assert_eq!(
            md,
            "**Welch t-test** · stats.ttest_welch · model\n\nunequal variances\n\n- p: **0.01**\n- t: **-3.5 sd** (large)"
        );
    }

    // --- tables and assets ---

    #[test]
    fn table_markdown_escapes_and_aligns() {
        let md = table_markdown(
            &strings(&["a|b", "n"]),
            &[strings(&["x|y", "1"])],
            &[false, true],
        );
        assert_eq!(md, "| a\\|b | n |\n| --- | ---: |\n| x\\|y | 1 |");
        // A short numeric list means left.
        let md = table_markdown(&strings(&["a", "b"]), &[], &[true]);
        assert_eq!(md, "| a | b |\n| ---: | --- |");
    }

    #[test]
    fn table_csv_quotes_when_needed() {
        let csv = table_csv(
            &strings(&["a", "b,c"]),
            &[strings(&["say \"hi\"", "two\nlines"]), strings(&["x", ""])],
        );
        assert_eq!(csv, "a,\"b,c\"\n\"say \"\"hi\"\"\",\"two\nlines\"\nx,");
    }

    #[test]
    fn inline_assets_replaces_known_names_and_keeps_unknown() {
        let assets = vec![
            ("a.png".to_owned(), vec![1u8, 2, 3]),
            ("b.svg".to_owned(), b"<svg/>".to_vec()),
            ("c.JPG".to_owned(), vec![9u8]),
            ("d.bin".to_owned(), vec![0u8]),
        ];
        let md = "![a](assets/a.png) ![b](assets/b.svg) ![c](assets/c.JPG) ![d](assets/d.bin) ![x](assets/x.png) ![r](http://r/a.png) ](assets/a.png";
        let out = inline_assets(md, &assets);
        assert!(out.contains("![a](data:image/png;base64,AQID)"), "{out}");
        assert!(out.contains("![b](data:image/svg+xml;base64,PHN2Zy8+)"));
        assert!(out.contains("![c](data:image/jpeg;base64,CQ==)"));
        assert!(out.contains("![d](data:application/octet-stream;base64,AA==)"));
        assert!(out.contains("![x](assets/x.png)"));
        assert!(out.contains("![r](http://r/a.png)"));
        assert!(out.ends_with("](assets/a.png"));
        assert_eq!(inline_assets("no images", &assets), "no images");
        assert_eq!(
            inline_assets("![a](assets/a.png)", &[]),
            "![a](assets/a.png)"
        );
    }

    // --- blocks and diagrams ---

    const DOC: &str = "# Title\n\nIntro line\nsecond line\n\n```mermaid\nflowchart LR\n  a --> b\n```\n\n- one\n- two\n  more\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n> quote\n> more\n\n## Next\n\n~~~Mermaid\npie\n  \"a\" : 1\n~~~\n\n```rust\nfn x() {}\n\n\nfn y() {}\n```\n\n````\nunterminated ```\n\nstill code\n";

    fn texts(md: &str) -> Vec<&str> {
        split_blocks(md)
            .iter()
            .map(|s| &md[s.start..s.end])
            .collect()
    }

    #[test]
    fn split_blocks_finds_every_kind_of_block() {
        let blocks = texts(DOC);
        assert_eq!(
            blocks,
            vec![
                "# Title",
                "Intro line\nsecond line",
                "```mermaid\nflowchart LR\n  a --> b\n```",
                "- one\n- two\n  more",
                "| a | b |\n| - | - |\n| 1 | 2 |",
                "> quote\n> more",
                "## Next",
                "~~~Mermaid\npie\n  \"a\" : 1\n~~~",
                "```rust\nfn x() {}\n\n\nfn y() {}\n```",
                "````\nunterminated ```\n\nstill code",
            ]
        );
    }

    #[test]
    fn split_blocks_edge_cases() {
        assert!(split_blocks("").is_empty());
        assert!(split_blocks("\n \n\n").is_empty());
        assert_eq!(texts("a\r\nb\r\n\r\nc"), vec!["a\r\nb", "c"]);
        // Fences and headings interrupt a paragraph; so does a class change.
        assert_eq!(
            texts("text\n```\ncode\n```\nmore\n# H\nafter\n- a\nb"),
            vec!["text", "```\ncode\n```", "more", "# H", "after", "- a", "b"]
        );
        // A shorter fence line does not close a longer one.
        assert_eq!(
            texts("````\n```\nx\n````\ny"),
            vec!["````\n```\nx\n````", "y"]
        );
        // A `#` without a space is text.
        assert_eq!(texts("#tag\n#### four"), vec!["#tag", "#### four"]);
    }

    #[test]
    fn diagram_spans_are_the_mermaid_fences() {
        let spans = diagram_spans(DOC);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            &DOC[spans[0].start..spans[0].end],
            "```mermaid\nflowchart LR\n  a --> b\n```"
        );
        assert_eq!(
            &DOC[spans[1].start..spans[1].end],
            "~~~Mermaid\npie\n  \"a\" : 1\n~~~"
        );
        assert_eq!(diagram_source(DOC, &spans[0]), "flowchart LR\n  a --> b");
        assert_eq!(diagram_source(DOC, &spans[1]), "pie\n  \"a\" : 1");
        assert!(diagram_spans("```rust\nx\n```").is_empty());
        assert!(diagram_spans("```merm\nx\n```").is_empty());
    }

    #[test]
    fn diagram_source_of_odd_fences() {
        let md = "```mermaid\n```";
        let spans = diagram_spans(md);
        assert_eq!(diagram_source(md, &spans[0]), "");
        let md = "```mermaid\nflowchart\n  a-->b";
        let spans = diagram_spans(md);
        assert_eq!(spans.len(), 1);
        assert_eq!(diagram_source(md, &spans[0]), "flowchart\n  a-->b");
        assert_eq!(diagram_source("text", &Span { start: 0, end: 4 }), "");
        assert_eq!(diagram_source("text", &Span { start: 0, end: 99 }), "");
    }

    #[test]
    fn replace_diagram_in_place() {
        let out = replace_diagram(DOC, 1, "  pie\n  \"b\" : 2  \n").unwrap();
        assert!(out.contains("~~~Mermaid\npie\n  \"b\" : 2\n~~~"), "{out}");
        assert!(out.contains("flowchart LR\n  a --> b"));
        assert_eq!(diagram_spans(&out).len(), 2);
        let out = replace_diagram(DOC, 0, "graph TD\n x-->y").unwrap();
        assert!(
            out.contains("```mermaid\ngraph TD\n x-->y\n```\n\n- one"),
            "{out}"
        );
        // Unterminated fence gets a closing line.
        let out = replace_diagram("a\n\n```mermaid\nold", 0, "new").unwrap();
        assert_eq!(out, "a\n\n```mermaid\nnew\n```");
    }

    #[test]
    fn replace_diagram_with_empty_source_removes_the_block() {
        let md = "A\n\n```mermaid\nx\n```\n\nB\n";
        assert_eq!(replace_diagram(md, 0, " \n").unwrap(), "A\n\nB\n");
        assert_eq!(
            replace_diagram("```mermaid\nx\n```\n\nB", 0, "").unwrap(),
            "B"
        );
        assert_eq!(
            replace_diagram("A\n\n```mermaid\nx\n```\n", 0, "").unwrap(),
            "A\n"
        );
        assert_eq!(replace_diagram("```mermaid\nx\n```\n", 0, "").unwrap(), "");
        let out = replace_diagram(DOC, 0, "").unwrap();
        assert!(!out.contains("flowchart"));
        assert!(out.contains("Intro line\nsecond line\n\n- one"));
    }

    #[test]
    fn replace_diagram_out_of_range_is_none() {
        assert_eq!(replace_diagram(DOC, 2, "x"), None);
        assert_eq!(replace_diagram("no diagram", 0, "x"), None);
        assert_eq!(replace_diagram("", 0, ""), None);
    }

    #[test]
    fn insert_diagram_after_a_block() {
        let md = "one\n\ntwo\n\nthree\n";
        let fence = "```mermaid\npie\n```";
        assert_eq!(
            insert_diagram_after(md, Some(0), " pie \n"),
            format!("one\n\n{fence}\n\ntwo\n\nthree\n")
        );
        assert_eq!(
            insert_diagram_after(md, Some(1), "pie"),
            format!("one\n\ntwo\n\n{fence}\n\nthree\n")
        );
        // The last block, None and past the end all append.
        let end = format!("one\n\ntwo\n\nthree\n\n{fence}\n");
        assert_eq!(insert_diagram_after(md, Some(2), "pie"), end);
        assert_eq!(insert_diagram_after(md, None, "pie"), end);
        assert_eq!(insert_diagram_after(md, Some(usize::MAX), "pie"), end);
        assert_eq!(insert_diagram_after(md, Some(999), "pie"), end);
    }

    #[test]
    fn insert_diagram_into_an_empty_doc_or_with_no_source() {
        assert_eq!(
            insert_diagram_after("", None, "pie"),
            "```mermaid\npie\n```\n"
        );
        assert_eq!(
            insert_diagram_after("\n\n", Some(0), "pie"),
            "```mermaid\npie\n```\n"
        );
        assert_eq!(insert_diagram_after("a\n", Some(0), "  \n "), "a\n");
        assert_eq!(insert_diagram_after("", None, ""), "");
    }

    #[test]
    fn inserted_diagram_is_its_own_block() {
        let out = insert_diagram_after("# H\ntext\n\nlast", Some(0), "flowchart LR\n  a-->b");
        let spans = diagram_spans(&out);
        assert_eq!(spans.len(), 1);
        assert_eq!(diagram_source(&out, &spans[0]), "flowchart LR\n  a-->b");
        assert_eq!(texts(&out).len(), 4);
        // A source with a fence inside gets a longer one.
        let out = insert_diagram_after("", None, "a ``` b");
        assert_eq!(out, "````mermaid\na ``` b\n````\n");
    }

    #[test]
    fn block_at_finds_the_block() {
        let md = "# T\n\npara one\nmore\n\n```mermaid\nx\n```\n\nend";
        // Offsets: "# T" 0..3, para 5..19, fence 21..?, "end" last.
        assert_eq!(block_at(md, 0), Some(0));
        assert_eq!(block_at(md, 2), Some(0));
        assert_eq!(block_at(md, 4), Some(0)); // blank line: last block that starts before
        assert_eq!(block_at(md, 5), Some(1));
        assert_eq!(block_at(md, 12), Some(1));
        assert_eq!(block_at(md, md.find("x\n```").unwrap()), Some(2));
        assert_eq!(block_at(md, md.len()), Some(3));
        assert_eq!(block_at(md, usize::MAX), Some(3));
        assert_eq!(block_at("", 0), None);
        assert_eq!(block_at("\n\ntext", 0), None);
    }

    #[test]
    fn templates_cover_six_diagrams() {
        let names: Vec<&str> = DIAGRAM_TEMPLATES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "Flowchart",
                "Sequence",
                "Pie",
                "Timeline",
                "Use case",
                "Bar chart"
            ]
        );
        for (_, source) in DIAGRAM_TEMPLATES {
            assert!(!source.trim().is_empty());
        }
    }

    // --- commands ---

    #[test]
    fn doc_command_parses_every_form() {
        use DocCommand as C;
        assert_eq!(C::parse(""), C::Open);
        assert_eq!(C::parse("   "), C::Open);
        assert_eq!(C::parse("new"), C::New(None));
        assert_eq!(
            C::parse("new Release notes "),
            C::New(Some("Release notes".into()))
        );
        assert_eq!(C::parse("find rel"), C::Find("rel".into()));
        assert_eq!(C::parse("find"), C::Usage("usage: /doc find <text>"));
        assert_eq!(C::parse("find   "), C::Usage("usage: /doc find <text>"));
        assert_eq!(C::parse("open rel notes"), C::OpenTitle("rel notes".into()));
        assert_eq!(C::parse("open"), C::Usage("usage: /doc open <text>"));
        assert_eq!(C::parse("use"), C::Use(None));
        assert_eq!(C::parse("use plan"), C::Use(Some("plan".into())));
        assert_eq!(C::parse("add"), C::Add);
        assert_eq!(C::parse("split"), C::Split);
        assert_eq!(C::parse("only"), C::Only);
        assert_eq!(C::parse("close"), C::Close);
        assert_eq!(C::parse("source"), C::Source);
        assert_eq!(C::parse("SPLIT"), C::Split);
        assert_eq!(
            C::parse("export md"),
            C::Export(Some(ExportFormat::Markdown))
        );
        assert_eq!(
            C::parse("export markdown"),
            C::Export(Some(ExportFormat::Markdown))
        );
        assert_eq!(C::parse("export html"), C::Export(Some(ExportFormat::Html)));
        assert_eq!(C::parse("export copy"), C::Export(Some(ExportFormat::Copy)));
        assert_eq!(C::parse("export pdf"), C::Export(Some(ExportFormat::Pdf)));
        assert_eq!(C::parse("export zip"), C::Export(None));
        assert_eq!(C::parse("export"), C::Export(None));
        assert_eq!(
            C::parse("release notes"),
            C::OpenTitle("release notes".into())
        );
        assert_eq!(C::parse("  plan  "), C::OpenTitle("plan".into()));
    }

    #[test]
    fn export_formats() {
        assert_eq!(ExportFormat::parse("HTML"), Some(ExportFormat::Html));
        assert_eq!(ExportFormat::parse("PDF"), Some(ExportFormat::Pdf));
        assert_eq!(ExportFormat::parse("zip"), None);
        assert_eq!(ExportFormat::Pdf.extension(), Some("html"));
        assert_eq!(ExportFormat::Pdf.label(), "PDF");
        assert_eq!(ExportFormat::Markdown.extension(), Some("md"));
        assert_eq!(ExportFormat::Html.extension(), Some("html"));
        assert_eq!(ExportFormat::Copy.extension(), None);
        assert_eq!(ExportFormat::Markdown.label(), "Markdown");
        assert_eq!(ExportFormat::Html.label(), "Web page");
        assert_eq!(ExportFormat::Copy.label(), "Copy");
    }

    fn doc(title: &str, updated_at: u64) -> Doc {
        Doc {
            id: DocId::new(),
            session_id: SessionId::new(),
            title: title.into(),
            file_name: file_name(title),
            dir: "/d".into(),
            created_at: 0,
            updated_at,
        }
    }

    #[test]
    fn match_docs_orders_newest_first_ignoring_case() {
        let docs = vec![
            doc("Release Notes", 10),
            doc("Plan", 30),
            doc("release plan", 20),
        ];
        let titles = |q: &str| -> Vec<String> {
            match_docs(&docs, q)
                .iter()
                .map(|d| d.title.clone())
                .collect()
        };
        assert_eq!(titles("REL"), ["release plan", "Release Notes"]);
        assert_eq!(titles("plan"), ["Plan", "release plan"]);
        assert_eq!(titles(""), ["Plan", "release plan", "Release Notes"]);
        assert_eq!(titles("  "), ["Plan", "release plan", "Release Notes"]);
        assert!(titles("zzz").is_empty());
    }

    #[test]
    fn doc_path_joins_dir_and_file() {
        let mut d = doc("A", 1);
        d.dir = "/x/".into();
        assert_eq!(d.path(), "/x/a.md");
    }
}
