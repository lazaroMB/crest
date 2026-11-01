//! What the picker asks the search ports and the rows it lists.

use std::ops::Range;

use application_core::keymap::Action;
use application_core::ports::search::{
    ColumnRange, FileList, FindFileRequest, GrepRequest, GrepResult,
};
use domain::workspace::WorkspacePath;

use super::preview::valid_range;
use super::{FILES_ACTION, GREP_ACTION};

/// The request the file picker sends for `query`: the defaults of the
/// `find_file` tool (`{"query": …}`).
pub fn files_request(query: &str) -> FindFileRequest {
    FindFileRequest::new(query)
}

/// How the text picker matches its query. The default searches for the
/// literal text, ignoring case, anywhere in a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GrepOptions {
    /// Upper and lower case must match (`Alt+C`).
    pub match_case: bool,
    /// The query is a regular expression (`Alt+R`), not literal text.
    pub regex: bool,
    /// Only whole words match (`Alt+W`).
    pub whole_word: bool,
}

/// One of the [`GrepOptions`] the user can flip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepToggle {
    /// [`GrepOptions::match_case`].
    MatchCase,
    /// [`GrepOptions::regex`].
    Regex,
    /// [`GrepOptions::whole_word`].
    WholeWord,
}

impl GrepToggle {
    /// Every toggle, in the order of the chips.
    pub const ALL: [Self; 3] = [Self::MatchCase, Self::Regex, Self::WholeWord];

    /// Chip label.
    pub fn label(self) -> &'static str {
        match self {
            Self::MatchCase => "Aa  Match case",
            Self::Regex => ".*  Regex",
            Self::WholeWord => "ab  Whole word",
        }
    }

    /// What it is called.
    pub fn name(self) -> &'static str {
        match self {
            Self::MatchCase => "Match case",
            Self::Regex => "Regex",
            Self::WholeWord => "Whole word",
        }
    }

    /// Keys that flip it.
    pub fn keys(self) -> &'static str {
        match self {
            Self::MatchCase => "Alt+C",
            Self::Regex => "Alt+R",
            Self::WholeWord => "Alt+W",
        }
    }

    /// Debug selector of its chip.
    pub fn selector(self) -> &'static str {
        match self {
            Self::MatchCase => "picker-case",
            Self::Regex => "picker-regex",
            Self::WholeWord => "picker-word",
        }
    }

    /// The toggle the letter of `Alt+<letter>` flips.
    pub(super) fn for_key(key: &str) -> Option<Self> {
        match key {
            "c" => Some(Self::MatchCase),
            "r" => Some(Self::Regex),
            "w" => Some(Self::WholeWord),
            _ => None,
        }
    }
}

impl GrepOptions {
    /// Whether `toggle` is on.
    pub fn get(self, toggle: GrepToggle) -> bool {
        match toggle {
            GrepToggle::MatchCase => self.match_case,
            GrepToggle::Regex => self.regex,
            GrepToggle::WholeWord => self.whole_word,
        }
    }

    /// Flips `toggle`.
    pub fn flip(&mut self, toggle: GrepToggle) {
        let flag = match toggle {
            GrepToggle::MatchCase => &mut self.match_case,
            GrepToggle::Regex => &mut self.regex,
            GrepToggle::WholeWord => &mut self.whole_word,
        };
        *flag = !*flag;
    }
}

/// The request the text picker sends for `query`: the `grep` tool's
/// defaults with the picker's options (`fixed`, `ignore_case`, `word`).
pub fn grep_request(query: &str, options: GrepOptions) -> GrepRequest {
    GrepRequest {
        fixed: !options.regex,
        ignore_case: !options.match_case,
        word: options.whole_word,
        ..GrepRequest::new(query)
    }
}

/// Which search a picker runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// `find_file`: paths ranked by fuzzy match.
    Files,
    /// `grep`: matching lines.
    Grep,
}

impl PickerMode {
    /// The mode an action opens, if it is a picker action.
    pub fn for_action(action: &Action) -> Option<Self> {
        match action {
            Action::Builtin(name) if name == FILES_ACTION => Some(Self::Files),
            Action::Builtin(name) if name == GREP_ACTION => Some(Self::Grep),
            _ => None,
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Files => "Find file",
            Self::Grep => "Search text",
        }
    }

    /// Placeholder of the query input.
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Files => "Search files…",
            Self::Grep => "Search text (ripgrep)…",
        }
    }
}

/// One row of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    /// File to open.
    pub path: WorkspacePath,
    /// 1-based line, for text matches.
    pub line: Option<u64>,
    /// The matching line, for text matches.
    pub text: Option<String>,
    /// Range of the match within `text`.
    pub column_range: Option<ColumnRange>,
    /// Lines before the matching one, for the preview.
    pub context_before: Vec<String>,
    /// Lines after the matching one, for the preview.
    pub context_after: Vec<String>,
    /// A file chosen before, listed first for an empty query.
    pub recent: bool,
}

impl PickerItem {
    /// A row for the file `path`.
    pub fn file(path: WorkspacePath) -> Self {
        Self {
            path,
            line: None,
            text: None,
            column_range: None,
            context_before: Vec::new(),
            context_after: Vec::new(),
            recent: false,
        }
    }
}

/// Rows of a `find_file` result, in its order.
pub fn items_from_files(list: &FileList) -> Vec<PickerItem> {
    list.paths.iter().cloned().map(PickerItem::file).collect()
}

/// Rows for an empty query: the `recent` files (most recent first, marked),
/// then the files of `list` that are not among them.
pub fn items_for_empty_query(recent: &[WorkspacePath], list: &FileList) -> Vec<PickerItem> {
    let recent_rows = recent.iter().cloned().map(|path| PickerItem {
        recent: true,
        ..PickerItem::file(path)
    });
    let rest = list
        .paths
        .iter()
        .filter(|path| !recent.contains(path))
        .cloned()
        .map(PickerItem::file);
    recent_rows.chain(rest).collect()
}

/// Rows of a `grep` result, in its order.
pub fn items_from_grep(result: &GrepResult) -> Vec<PickerItem> {
    result
        .hits
        .iter()
        .map(|hit| PickerItem {
            path: hit.path.clone(),
            line: Some(hit.line_number),
            text: Some(hit.line.clone()),
            column_range: Some(hit.column_range),
            context_before: hit.context_before.clone(),
            context_after: hit.context_after.clone(),
            recent: false,
        })
        .collect()
}

/// What a search found: its rows and how many matched in all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) items: Vec<PickerItem>,
    pub(super) total: usize,
    pub(super) truncated: bool,
}

/// The count under the results: `12 results`, or `100 of 2345
/// (truncated)` when the search returned only part of its matches.
pub fn count_text(shown: usize, total: usize, truncated: bool) -> String {
    if truncated || total > shown {
        format!("{shown} of {} (truncated)", total.max(shown))
    } else if shown == 1 {
        "1 result".to_owned()
    } else {
        format!("{shown} results")
    }
}

/// `text` cut from the left to at most `max` characters, with an ellipsis
/// in front, at a `/` when there is one so the end of the path stays whole:
/// `…/ui/src/main_view.rs`.
pub fn truncate_left(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let keep = max.saturating_sub(1);
    let tail: String = text.chars().skip(count - keep).collect();
    match tail.find('/') {
        Some(at) if at + 1 < tail.len() => format!("…{}", &tail[at..]),
        _ => format!("…{tail}"),
    }
}

/// Bytes of a matching line shown before its match at most; a longer start
/// is cut so the match stays in view.
const ROW_LEAD: usize = 40;

/// The text a result row shows for the matching line `text` and where the
/// match is in it: without the indentation, and cut in front (`…`) when the
/// match is far to the right.
pub fn row_text(text: &str, range: Option<ColumnRange>) -> (String, Option<Range<usize>>) {
    let range = range.and_then(|range| valid_range(text, range));
    let indent = text.len() - text.trim_start().len();
    let mut start = indent;
    if let Some(range) = &range
        && range.start > indent + ROW_LEAD
    {
        start = range.start - ROW_LEAD / 2;
        while !text.is_char_boundary(start) {
            start -= 1;
        }
    }
    let body = text[start..].trim_end();
    let prefix = if start > indent { "…" } else { "" };
    let shown = format!("{prefix}{body}");
    let range = range.and_then(|range| {
        let from = range.start.checked_sub(start)? + prefix.len();
        let to = (range.end - start).min(body.len()) + prefix.len();
        (from < to).then_some(from..to)
    });
    (shown, range)
}
