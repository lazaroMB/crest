//! File and text pickers: `<leader>ff` or `/picker.files` (`picker.files`)
//! finds a file and `<leader>fg` or `/picker.grep` (`picker.grep`)
//! searches the workspace contents.
//!
//! Both call the same ports as the agent's `find_file` and `grep` tools
//! ([`files_request`], [`grep_request`]), so the user sees what the agent
//! would see. Results follow the query as it is typed: each keystroke
//! restarts a short debounce, and replacing the search task drops the
//! previous search future, which cancels it (the adapters kill their
//! `rg`/`fzf` processes on drop). While a search runs the input shows a
//! spinner and the previous results stay, dimmed.
//!
//! With an empty query the file picker lists the files chosen last, then
//! the first files of the workspace. The text picker searches for the
//! literal text by default; `Alt+C` (match case), `Alt+R` (regex) and
//! `Alt+W` (whole word), or their chips, change how ([`GrepOptions`]).
//!
//! On a wide enough window the popup has two columns: the results and a
//! [`preview`] of the highlighted one, read through the
//! [`FileSystemPort`] around the matching line. `Alt+P` hides or shows it.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use application::keymap::{Action, KeyCode};
use application::ports::fs::FileSystemPort;
use application::ports::search::{
    ColumnRange, ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepRequest,
    GrepResult, ListFilesRequest, SearchError,
};
use domain::workspace::WorkspacePath;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, KeystrokeEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, h_flex, v_flex};

pub mod preview;

use preview::{
    LineStyles, PREVIEW_DEBOUNCE, Preview, overlay, preview_request, syntax_styles, valid_range,
};

/// Action that opens the file picker.
pub const FILES_ACTION: &str = "picker.files";
/// Action that opens the text picker.
pub const GREP_ACTION: &str = "picker.grep";

/// Wait after the last keystroke before searching.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

/// Files chosen in the file picker it remembers, to list first.
pub const RECENT_LIMIT: usize = 10;
/// Workspace files the file picker lists for an empty query.
pub const EMPTY_QUERY_LIMIT: usize = 50;

/// Widest the popup gets.
const POPUP_WIDTH: f32 = 1100.;
/// Tallest the popup gets.
const POPUP_HEIGHT: f32 = 560.;
/// Narrower than this, the popup has one column and no preview.
pub const TWO_COLUMNS_MIN_WIDTH: f32 = 860.;
/// Share of the popup the results take next to the preview.
const RESULTS_SHARE: f32 = 0.45;
/// Rough width of a character of the results, to cut long paths.
const CHAR_WIDTH: f32 = 7.5;

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
    fn for_key(key: &str) -> Option<Self> {
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

    fn title(self) -> &'static str {
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
struct Found {
    items: Vec<PickerItem>,
    total: usize,
    truncated: bool,
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

/// What the picker asks the rest of the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEvent {
    /// Open `path`, at `line` for a text match.
    Open {
        /// File chosen.
        path: WorkspacePath,
        /// 1-based line, for text matches.
        line: Option<u64>,
    },
    /// The picker closed without a choice.
    Dismissed,
}

/// The picker popup. Render it above the rest of the window (as the last
/// child of a `relative` container); it is empty while closed.
///
/// Keys while open: type to search, `Up`/`Down` (or `Ctrl+P`/`Ctrl+N`)
/// move, `Enter` inserts the reference, `Esc` closes (wherever the focus
/// went), `Alt+P` shows or hides the preview and, in the text picker,
/// `Alt+C`/`Alt+R`/`Alt+W` flip match case, regex and whole word. The
/// shortcut that opened it closes it too, as do its `✕` and a click
/// outside the popup.
pub struct PickerView {
    files: Arc<dyn FileSearchPort>,
    content: Arc<dyn ContentSearchPort>,
    fs: Option<Arc<dyn FileSystemPort>>,
    mode: Option<PickerMode>,
    input: Entity<InputState>,
    items: Vec<PickerItem>,
    total: usize,
    truncated: bool,
    selected: usize,
    scroll: ScrollHandle,
    error: Option<String>,
    search: Option<Task<()>>,
    options: GrepOptions,
    recent: Vec<WorkspacePath>,
    preview_on: bool,
    preview: Option<Preview>,
    preview_styles: Vec<LineStyles>,
    preview_task: Option<Task<()>>,
    restore_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for PickerView {}

impl PickerView {
    /// A closed picker over the search ports, without a file system: the
    /// text picker previews the context of its matches only until
    /// [`Self::set_file_system`].
    pub fn new(
        files: Arc<dyn FileSearchPort>,
        content: Arc<dyn ContentSearchPort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let changed = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_search(window, cx);
                }
            },
        );
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        Self {
            files,
            content,
            fs: None,
            mode: None,
            input,
            items: Vec::new(),
            total: 0,
            truncated: false,
            selected: 0,
            scroll: ScrollHandle::new(),
            error: None,
            search: None,
            options: GrepOptions::default(),
            recent: Vec::new(),
            preview_on: true,
            preview: None,
            preview_styles: Vec::new(),
            preview_task: None,
            restore_focus: None,
            _subscriptions: vec![changed, keys],
        }
    }

    /// Reads the previewed files through `fs`.
    pub fn set_file_system(&mut self, fs: Option<Arc<dyn FileSystemPort>>) {
        self.fs = fs;
    }

    /// The open mode, or `None` when closed.
    pub fn mode(&self) -> Option<PickerMode> {
        self.mode
    }

    /// Current results.
    pub fn items(&self) -> &[PickerItem] {
        &self.items
    }

    /// Index of the highlighted row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The scroll of the results, which follows the highlighted row.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// The error of the last search, if it failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether a search is waiting or running.
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// How the text picker matches.
    pub fn options(&self) -> GrepOptions {
        self.options
    }

    /// Whether the preview is on (`Alt+P`).
    pub fn preview_enabled(&self) -> bool {
        self.preview_on
    }

    /// What the preview shows, if anything.
    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }

    /// The files chosen last in the file picker, most recent first.
    pub fn recent(&self) -> &[WorkspacePath] {
        &self.recent
    }

    /// The query typed so far.
    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Whether `Alt+<code>` is a key of the open picker, which the
    /// session's keymap must leave to it.
    pub fn owns_alt_key(code: &KeyCode) -> bool {
        matches!(code, KeyCode::Char('p' | 'c' | 'r' | 'w'))
    }

    /// Runs `picker.files` or `picker.grep`: opens the picker in that
    /// mode, switches to it, or closes the picker already open in it.
    /// Returns whether the action belongs to the picker.
    pub fn handle_action(
        &mut self,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match PickerMode::for_action(action) {
            Some(mode) if self.mode == Some(mode) => {
                self.dismiss(window, cx);
                true
            }
            Some(mode) => {
                self.open(mode, window, cx);
                true
            }
            None => false,
        }
    }

    /// Whether the picker is shown.
    pub fn is_open(&self) -> bool {
        self.mode.is_some()
    }

    /// Opens the picker in `mode` with an empty query and focuses its input.
    pub fn open(&mut self, mode: PickerMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_none() {
            self.restore_focus = window.focused(cx);
        }
        self.mode = Some(mode);
        self.items.clear();
        self.preview = None;
        self.preview_task = None;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(mode.placeholder(), window, cx);
        });
        self.set_query("", window, cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Replaces the query and searches for it.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.schedule_search(window, cx);
    }

    /// Flips one of the text picker's options and searches again.
    pub fn toggle(&mut self, toggle: GrepToggle, window: &mut Window, cx: &mut Context<Self>) {
        self.options.flip(toggle);
        if self.mode == Some(PickerMode::Grep) {
            self.schedule_search(window, cx);
        }
        cx.notify();
    }

    /// Shows or hides the preview.
    pub fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_on = !self.preview_on;
        if self.preview_on {
            self.schedule_preview(cx);
        } else {
            self.preview_task = None;
        }
        cx.notify();
    }

    /// Closes without a choice.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close(window, cx) {
            cx.emit(PickerEvent::Dismissed);
        }
    }

    /// Opens the highlighted row: emits [`PickerEvent::Open`] and closes.
    /// A file chosen in the file picker is remembered as recent.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        if self.mode == Some(PickerMode::Files) {
            self.recent.retain(|path| *path != item.path);
            self.recent.insert(0, item.path.clone());
            self.recent.truncate(RECENT_LIMIT);
        }
        self.close(window, cx);
        cx.emit(PickerEvent::Open {
            path: item.path,
            line: item.line,
        });
    }

    /// Moves the highlight by `delta` rows, wrapping around, and keeps it
    /// in view.
    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        let len = len as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        crate::keyscroll::ensure_visible(&self.scroll, self.selected);
        self.schedule_preview(cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mode.take().is_none() {
            return false;
        }
        self.search = None;
        self.items.clear();
        self.error = None;
        self.preview = None;
        self.preview_styles.clear();
        self.preview_task = None;
        self.set_loading(false, window, cx);
        if let Some(handle) = self.restore_focus.take() {
            window.focus(&handle, cx);
        }
        cx.notify();
        true
    }

    fn set_loading(&self, loading: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_loading(loading, window, cx));
    }

    /// Starts a search for the current query, after the debounce unless
    /// the query is empty. The task it replaces, and the search future it
    /// owns, are dropped. An empty query lists files in the file picker
    /// and clears the text picker.
    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.mode else {
            return;
        };
        let query = self.query(cx);
        let empty = query.trim().is_empty();
        if empty && mode == PickerMode::Grep {
            self.search = None;
            self.set_found(
                Ok(Found {
                    items: Vec::new(),
                    total: 0,
                    truncated: false,
                }),
                cx,
            );
            self.set_loading(false, window, cx);
            return;
        }
        let files = self.files.clone();
        let content = self.content.clone();
        let options = self.options;
        let recent = self.recent.clone();
        self.search = Some(cx.spawn_in(window, async move |this, cx| {
            if !empty {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }
            let result = cx
                .background_spawn(async move {
                    run_search(mode, &query, options, &recent, files, content).await
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.search = None;
                this.set_loading(false, window, cx);
                this.set_found(result, cx);
            })
            .ok();
        }));
        self.set_loading(true, window, cx);
        cx.notify();
    }

    fn set_found(&mut self, result: Result<Found, SearchError>, cx: &mut Context<Self>) {
        match result {
            Ok(found) => {
                self.items = found.items;
                self.total = found.total;
                self.truncated = found.truncated;
                self.error = None;
            }
            Err(err) => {
                self.items.clear();
                self.total = 0;
                self.truncated = false;
                self.error = Some(err.to_string());
            }
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        self.schedule_preview(cx);
        cx.notify();
    }

    /// Previews the highlighted row: at once from what the row knows, then
    /// from its file after [`PREVIEW_DEBOUNCE`].
    fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        if !self.preview_on || self.mode.is_none() {
            self.preview_task = None;
            return;
        }
        let Some(item) = self.items.get(self.selected).cloned() else {
            self.preview = None;
            self.preview_styles.clear();
            self.preview_task = None;
            return;
        };
        if self
            .preview
            .as_ref()
            .is_some_and(|preview| preview.is_for(&item))
        {
            return;
        }
        let Some(fs) = self.fs.clone() else {
            self.preview_task = None;
            let preview = if item.text.is_some() {
                Preview {
                    loading: false,
                    ..Preview::of_context(&item)
                }
            } else {
                Preview::failed(&item, "no preview without a file system".into())
            };
            self.set_preview(preview, cx);
            return;
        };
        self.set_preview(Preview::of_context(&item), cx);
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let request = preview_request(&item);
            let read = cx
                .background_spawn(async move { fs.read_file(request).await })
                .await;
            this.update(cx, |this, cx| {
                this.preview_task = None;
                let preview = match read {
                    Ok(content) => Preview::read(&item, content),
                    Err(err) => Preview::failed(&item, err.to_string()),
                };
                this.set_preview(preview, cx);
            })
            .ok();
        }));
    }

    fn set_preview(&mut self, preview: Preview, cx: &mut Context<Self>) {
        let theme = cx.theme().highlight_theme.clone();
        self.preview_styles = syntax_styles(&preview.path, &preview.lines, &theme);
        self.preview = Some(preview);
        cx.notify();
    }

    /// The line under the results: the error, the search state or the
    /// count.
    pub fn status_text(&self, cx: &App) -> String {
        let empty_query = self.query(cx).trim().is_empty();
        let shown = self.items.len();
        if let Some(error) = &self.error {
            return error.clone();
        }
        match self.mode {
            Some(PickerMode::Grep) if empty_query => {
                return "Type to search the text of the workspace".to_owned();
            }
            _ => {}
        }
        if self.search.is_some() {
            return if shown == 0 {
                "searching…".to_owned()
            } else {
                "searching… (showing the previous results)".to_owned()
            };
        }
        if shown == 0 {
            return "no results".to_owned();
        }
        if self.mode == Some(PickerMode::Files) && empty_query {
            return if self.recent.is_empty() {
                "workspace files; type to search".to_owned()
            } else {
                "recent files, then workspace files; type to search".to_owned()
            };
        }
        count_text(shown, self.total, self.truncated)
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.mode else {
            return;
        };
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        // `Esc` closes the picker wherever the focus went (a click in the
        // list, elsewhere in the window).
        if keystroke.key == "escape" && m == gpui::Modifiers::none() {
            self.dismiss(window, cx);
            cx.stop_propagation();
            return;
        }
        // The `Alt+…` toggles work wherever the focus went too.
        if m.alt && !m.control && !m.platform && !m.shift && !m.function {
            let key = keystroke.key.as_str();
            if key == "p" {
                self.toggle_preview(cx);
                cx.stop_propagation();
                return;
            }
            if mode == PickerMode::Grep
                && let Some(toggle) = GrepToggle::for_key(key)
            {
                self.toggle(toggle, window, cx);
                cx.stop_propagation();
                return;
            }
        }
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        if !focused {
            return;
        }
        let ctrl = m.control;
        match keystroke.key.as_str() {
            "enter" => self.confirm(window, cx),
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "p" if ctrl => self.move_selection(-1, cx),
            "n" if ctrl => self.move_selection(1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

async fn run_search(
    mode: PickerMode,
    query: &str,
    options: GrepOptions,
    recent: &[WorkspacePath],
    files: Arc<dyn FileSearchPort>,
    content: Arc<dyn ContentSearchPort>,
) -> Result<Found, SearchError> {
    match mode {
        PickerMode::Files if query.trim().is_empty() => {
            let list = files
                .list_files(ListFilesRequest {
                    glob: None,
                    limit: EMPTY_QUERY_LIMIT,
                })
                .await?;
            let items = items_for_empty_query(recent, &list);
            Ok(Found {
                total: list.total.max(items.len()),
                truncated: list.truncated,
                items,
            })
        }
        PickerMode::Files => files
            .find_file(files_request(query))
            .await
            .map(|list| Found {
                items: items_from_files(&list),
                total: list.total,
                truncated: list.truncated,
            }),
        PickerMode::Grep => content
            .grep(grep_request(query, options))
            .await
            .map(|result| Found {
                items: items_from_grep(&result),
                total: result.total,
                truncated: result.truncated,
            }),
    }
}

/// The keys the footer lists; `Alt+P` only when the window is `wide`
/// enough for the preview.
pub fn footer_keys(mode: PickerMode, wide: bool) -> String {
    let mut keys = vec!["↑↓ navigate", "Enter insert", "Esc close"];
    if wide {
        keys.push("Alt+P preview");
    }
    if mode == PickerMode::Grep {
        keys.extend(["Alt+C case", "Alt+R regex", "Alt+W word"]);
    }
    keys.join(" · ")
}

impl PickerView {
    fn render_item(
        &self,
        ix: usize,
        item: &PickerItem,
        path_chars: usize,
        stale: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let location = match item.line {
            Some(line) => format!("{}:{line}", item.path),
            None => item.path.to_string(),
        };
        // A match line shares the row with its location.
        let budget = if item.text.is_some() {
            path_chars * 2 / 5
        } else {
            path_chars
        };
        let mark = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.35)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let mut row = h_flex()
            .id(ix)
            .gap_2()
            .px_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(truncate_left(&location, budget.max(12)))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ix;
                this.confirm(window, cx);
            }));
        if item.recent {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("recent"),
            );
        }
        if let Some(text) = &item.text {
            let (shown, range) = row_text(text, item.column_range);
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_color(theme.muted_foreground)
                    .text_ellipsis()
                    .child(
                        StyledText::new(shown).with_highlights(range.map(|range| (range, mark))),
                    ),
            );
        }
        if ix == self.selected {
            row = row.bg(theme.list_active);
        }
        if stale {
            row = row.opacity(0.5);
        }
        row
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = self.options;
        h_flex()
            .gap_1()
            .children(GrepToggle::ALL.map(|toggle| {
                let on = options.get(toggle);
                div().debug_selector(|| toggle.selector().to_owned()).child(
                    Button::new(toggle.selector())
                        .ghost()
                        .xsmall()
                        .label(toggle.label())
                        .selected(on)
                        .toggled(on)
                        .tab_stop(false)
                        .tooltip(format!("{} ({})", toggle.name(), toggle.keys()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle(toggle, window, cx);
                            this.input.update(cx, |input, cx| input.focus(window, cx));
                        })),
                )
            }))
            .into_any_element()
    }

    fn render_preview(&self, path_chars: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let pane = v_flex()
            .id("picker-preview")
            .debug_selector(|| "picker-preview".to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .border_l_1()
            .border_color(theme.border)
            .pl_2()
            .gap_1();
        let Some(preview) = &self.preview else {
            return pane
                .items_center()
                .justify_center()
                .text_color(muted)
                .child("Nothing to preview")
                .into_any_element();
        };
        let mut title = preview.path.to_string();
        if let Some(line) = preview.line {
            title.push_str(&format!(":{line}"));
        }
        let note = match (&preview.error, preview.loading) {
            (Some(error), _) => Some(error.clone()),
            (None, true) => Some("reading…".to_owned()),
            (None, false) => None,
        };
        let header = h_flex()
            .gap_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(SharedString::from(truncate_left(
                &title,
                path_chars.max(12),
            )))
            .children(note.map(|note| {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(note))
            }));
        let mark = HighlightStyle {
            background_color: Some(theme.warning.opacity(0.35)),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let match_ix = preview.match_index();
        let last = preview.start_line + preview.lines.len();
        let gutter = px(8. * last.to_string().len() as f32 + 4.);
        let rows = preview.lines.iter().enumerate().map(|(ix, line)| {
            let styles = self.preview_styles.get(ix).cloned().unwrap_or_default();
            let is_match = match_ix == Some(ix);
            let styles = match preview
                .column_range
                .and_then(|range| valid_range(line, range))
            {
                Some(range) if is_match => overlay(&styles, range, mark),
                _ => styles,
            };
            // An empty line still takes a row.
            let text = if line.is_empty() {
                StyledText::new(" ")
            } else {
                StyledText::new(line.clone()).with_highlights(styles)
            };
            h_flex()
                .gap_2()
                .whitespace_nowrap()
                .when(is_match, |row| row.bg(theme.list_active))
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .text_right()
                        .text_color(muted)
                        .child(SharedString::from((preview.start_line + ix).to_string())),
                )
                .child(div().child(text))
        });
        pane.child(header)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .children(rows),
            )
            .into_any_element()
    }
}

impl Render for PickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(mode) = self.mode else {
            return div().into_any_element();
        };
        let viewport = window.viewport_size();
        let width = POPUP_WIDTH.min(f32::from(viewport.width) - 32.).max(240.);
        let height = POPUP_HEIGHT
            .min(f32::from(viewport.height) - 120.)
            .max(200.);
        let wide = width >= TWO_COLUMNS_MIN_WIDTH;
        let two_columns = self.preview_on && wide;
        let results_width = if two_columns {
            width * RESULTS_SHARE
        } else {
            width
        };
        let path_chars = (results_width / CHAR_WIDTH) as usize;
        let preview_chars = ((width - results_width) / CHAR_WIDTH) as usize;
        let status = self.status_text(cx);
        let stale = self.search.is_some();
        let theme = cx.theme();
        let (bg, border, muted) = (theme.popover, theme.border, theme.muted_foreground);
        let rows: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                self.render_item(ix, item, path_chars, stale, cx)
                    .into_any_element()
            })
            .collect();
        let results = crate::keyscroll::list("picker-results", &self.scroll, rows)
            .when(two_columns, |div| div.w(px(results_width)).flex_none())
            .when(!two_columns, |div| div.flex_1())
            .min_w_0()
            .h_full();
        let preview = two_columns.then(|| self.render_preview(preview_chars, cx));
        let chips = (mode == PickerMode::Grep).then(|| self.render_chips(cx));
        // The backdrop: a click outside the popup closes it.
        div()
            .id("picker-backdrop")
            .debug_selector(|| "picker-backdrop".to_owned())
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.dismiss(window, cx)),
            )
            .child(
                v_flex()
                    .id("picker")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w(px(width))
                    .h(px(height))
                    .gap_1()
                    .p_2()
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_color(muted).child(mode.title()))
                            .child(div().flex_1().child(Input::new(&self.input)))
                            .child(div().text_xs().text_color(muted).child("Esc"))
                            .child(
                                div().debug_selector(|| "picker-close".to_owned()).child(
                                    Button::new("picker-close")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Close (Esc)")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.dismiss(window, cx);
                                        })),
                                ),
                            ),
                    )
                    .children(chips)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_start()
                            .gap_2()
                            .child(results)
                            .children(preview),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                div()
                                    .debug_selector(|| "picker-status".to_owned())
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(SharedString::from(status)),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "picker-keys".to_owned())
                                    .flex_none()
                                    .child(SharedString::from(footer_keys(mode, wide))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use application::ports::search::{ListFilesRequest, TreeNode, TreeRequest};
    use application::testing::fs::{InMemoryFs, test_root};

    use futures::executor::block_on;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    fn fake_fs() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(test_root())
                .with_file("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n")
                .with_file("src/main.rs", "fn main() {\n    run();\n}\n")
                .with_file("src/lib.rs", "pub fn run() {}\n")
                .with_file("docs/main.md", "# main\nrun it\n")
                .with_file("crates/ui/src/main_view.rs", "// run\n"),
        )
    }

    fn picker(
        fs: Arc<InMemoryFs>,
        cx: &mut TestAppContext,
    ) -> (Entity<PickerView>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let (picker, cx) =
            cx.add_window_view(|window, cx| PickerView::new(fs.clone(), fs.clone(), window, cx));
        (picker, cx)
    }

    fn open(picker: &Entity<PickerView>, action: &str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| {
                assert!(picker.handle_action(&Action::builtin(action), window, cx));
            });
        });
    }

    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(SEARCH_DEBOUNCE);
        cx.run_until_parked();
    }

    fn events(
        picker: &Entity<PickerView>,
        cx: &mut VisualTestContext,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<PickerEvent>>> {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(picker, move |_, event: &PickerEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
            .detach();
        });
        events
    }

    #[test]
    fn actions_pick_the_mode() {
        assert_eq!(
            PickerMode::for_action(&Action::builtin("picker.files")),
            Some(PickerMode::Files)
        );
        assert_eq!(
            PickerMode::for_action(&Action::builtin("picker.grep")),
            Some(PickerMode::Grep)
        );
        assert_eq!(PickerMode::for_action(&Action::builtin("block.next")), None);
    }

    #[test]
    fn requests_are_the_tool_defaults() {
        // What the model gets when it calls the tool with only the query.
        let find: FindFileRequest = serde_json::from_str(r#"{"query":"main"}"#).unwrap();
        assert_eq!(files_request("main"), find);
        let grep: GrepRequest =
            serde_json::from_str(r#"{"pattern":"run","fixed":true,"ignore_case":true}"#).unwrap();
        assert_eq!(grep_request("run", GrepOptions::default()), grep);
        let all = GrepOptions {
            match_case: true,
            regex: true,
            whole_word: true,
        };
        let grep: GrepRequest = serde_json::from_str(r#"{"pattern":"run","word":true}"#).unwrap();
        assert_eq!(grep_request("run", all), grep);
    }

    #[gpui::test]
    fn file_results_match_the_port(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("main");
        settle(cx);
        let expected = block_on(fs.find_file(files_request("main"))).unwrap();
        assert_eq!(expected.paths.len(), 3);
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.query(cx), "main");
            let paths: Vec<_> = picker
                .items()
                .iter()
                .map(|item| item.path.clone())
                .collect();
            assert_eq!(paths, expected.paths);
            assert!(!picker.is_searching());
        });
    }

    #[gpui::test]
    fn enter_opens_the_highlighted_file(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        let events = events(&picker, cx);
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("main");
        settle(cx);
        let expected = block_on(fs.find_file(files_request("main"))).unwrap();
        cx.simulate_keystrokes("down down up down enter");
        assert_eq!(
            *events.borrow(),
            [PickerEvent::Open {
                path: expected.paths[2].clone(),
                line: None,
            }]
        );
        picker.read_with(cx, |picker, _| assert_eq!(picker.mode(), None));
    }

    #[gpui::test]
    fn escape_dismisses_and_gives_focus_back(cx: &mut TestAppContext) {
        let (picker, cx) = picker(fake_fs(), cx);
        let events = events(&picker, cx);
        let before = cx.update(|window, cx| {
            let handle = cx.focus_handle();
            window.focus(&handle, cx);
            handle
        });
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("run");
        cx.simulate_keystrokes("escape");
        assert_eq!(*events.borrow(), [PickerEvent::Dismissed]);
        cx.update(|window, _| assert!(before.is_focused(window)));
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.mode(), None);
            assert!(!picker.is_searching(), "the pending search was dropped");
        });
    }

    /// A port whose `find_file` never finishes for `slow` and counts the
    /// searches that were dropped before finishing.
    struct Slow {
        inner: Arc<InMemoryFs>,
        dropped: Arc<AtomicUsize>,
    }

    struct CountDrop(Arc<AtomicUsize>);

    impl Drop for CountDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl FileSearchPort for Slow {
        async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError> {
            self.inner.list_files(request).await
        }
        async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError> {
            if request.query == "slow" {
                let _guard = CountDrop(self.dropped.clone());
                futures::future::pending::<()>().await;
            }
            self.inner.find_file(request).await
        }
        async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
            self.inner.tree(request).await
        }
    }

    #[gpui::test]
    fn typing_cancels_the_search_in_flight(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let dropped = Arc::new(AtomicUsize::new(0));
        let slow = Arc::new(Slow {
            inner: fs.clone(),
            dropped: dropped.clone(),
        });
        cx.update(gpui_component::init);
        let (picker, cx) =
            cx.add_window_view(|window, cx| PickerView::new(slow.clone(), fs.clone(), window, cx));
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("slow");
        settle(cx);
        picker.read_with(cx, |picker, _| assert!(picker.is_searching()));
        assert_eq!(dropped.load(Ordering::SeqCst), 0, "still running");

        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| picker.set_query("lib", window, cx));
        });
        settle(cx);
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            1,
            "the slow search was dropped"
        );
        picker.read_with(cx, |picker, _| {
            let paths: Vec<_> = picker.items().iter().map(|i| i.path.to_string()).collect();
            assert_eq!(paths, ["src/lib.rs"]);
        });
    }

    #[gpui::test]
    fn search_waits_for_the_debounce(cx: &mut TestAppContext) {
        let (picker, cx) = picker(fake_fs(), cx);
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("pub");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert!(picker.items().is_empty());
            assert!(picker.is_searching());
        });
        settle(cx);
        picker.read_with(cx, |picker, _| assert_eq!(picker.items().len(), 1));
        // The old results stay, marked stale, while the next search waits.
        cx.simulate_input("x");
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert_eq!(picker.items().len(), 1);
            assert!(picker.is_searching());
            assert_eq!(
                picker.status_text(cx),
                "searching… (showing the previous results)"
            );
        });
        // Clearing the query clears the results without searching.
        cx.simulate_keystrokes("backspace backspace backspace backspace");
        cx.run_until_parked();
        picker.read_with(cx, |picker, cx| {
            assert!(picker.items().is_empty());
            assert!(!picker.is_searching());
            assert_eq!(
                picker.status_text(cx),
                "Type to search the text of the workspace"
            );
        });
    }

    #[gpui::test]
    fn draws_results(cx: &mut TestAppContext) {
        let (picker, cx) = picker(fake_fs(), cx);
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("main");
        settle(cx);
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1000.), px(700.)),
            |_, _| div().relative().size_full().child(picker.clone()),
        );
    }

    #[gpui::test]
    fn grep_results_match_the_port(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("run");
        settle(cx);
        let expected = block_on(fs.grep(grep_request("run", GrepOptions::default()))).unwrap();
        assert_eq!(expected.hits.len(), 4);
        picker.read_with(cx, |picker, _| {
            let rows: Vec<_> = picker
                .items()
                .iter()
                .map(|item| (item.path.clone(), item.line, item.text.clone()))
                .collect();
            let hits: Vec<_> = expected
                .hits
                .iter()
                .map(|hit| {
                    (
                        hit.path.clone(),
                        Some(hit.line_number),
                        Some(hit.line.clone()),
                    )
                })
                .collect();
            assert_eq!(rows, hits);
        });
    }

    #[gpui::test]
    fn enter_on_a_match_opens_its_line(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        let events = events(&picker, cx);
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("run()");
        settle(cx);
        // Literal by default: `run()` is the text, not a regex.
        picker.read_with(cx, |picker, _| {
            let mut rows: Vec<_> = picker.items().iter().map(|i| i.path.to_string()).collect();
            rows.sort_unstable();
            assert_eq!(rows, ["src/lib.rs", "src/main.rs"]);
        });
        // As a regex `(` is a metacharacter, so the request fails like
        // the tool's.
        cx.simulate_keystrokes("alt-r");
        settle(cx);
        picker.read_with(cx, |picker, _| {
            assert!(picker.options().regex);
            assert!(picker.items().is_empty());
            assert!(
                picker
                    .error()
                    .is_some_and(|e| e.contains("invalid pattern"))
            );
        });
        cx.simulate_keystrokes("alt-r backspace backspace");
        settle(cx);
        cx.simulate_keystrokes("ctrl-n enter");
        let expected = block_on(fs.grep(grep_request("run", GrepOptions::default()))).unwrap();
        assert_eq!(
            *events.borrow(),
            [PickerEvent::Open {
                path: expected.hits[1].path.clone(),
                line: Some(expected.hits[1].line_number),
            }]
        );
    }

    #[test]
    fn counts_paths_and_rows_read_well() {
        assert_eq!(count_text(1, 1, false), "1 result");
        assert_eq!(count_text(4, 4, false), "4 results");
        assert_eq!(count_text(100, 2345, true), "100 of 2345 (truncated)");
        assert_eq!(truncate_left("src/main.rs", 20), "src/main.rs");
        assert_eq!(
            truncate_left("crates/ui/src/picker/preview.rs", 22),
            "…/picker/preview.rs"
        );
        assert_eq!(truncate_left("abcdefghij", 5), "…ghij");
        // The indentation goes and the match keeps its place.
        let range = |start, end| Some(ColumnRange { start, end });
        assert_eq!(
            row_text("    run();", range(4, 7)),
            ("run();".to_owned(), Some(0..3))
        );
        // A match far to the right stays in view.
        let long = format!("{}needle end", "x".repeat(80));
        let (shown, at) = row_text(&long, range(80, 86));
        assert!(shown.starts_with('…'), "{shown}");
        let at = at.unwrap();
        assert_eq!(&shown[at], "needle");
        assert_eq!(
            footer_keys(PickerMode::Files, true),
            "↑↓ navigate · Enter insert · Esc close · Alt+P preview"
        );
        assert!(
            footer_keys(PickerMode::Grep, false).ends_with("Alt+C case · Alt+R regex · Alt+W word")
        );
    }

    #[gpui::test]
    fn an_empty_query_lists_recent_files_first(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        open(&picker, FILES_ACTION, cx);
        settle(cx);
        let listed = block_on(fs.list_files(ListFilesRequest::default())).unwrap();
        picker.read_with(cx, |picker, cx| {
            let paths: Vec<_> = picker.items().iter().map(|i| i.path.clone()).collect();
            assert_eq!(paths, listed.paths);
            assert_eq!(picker.status_text(cx), "workspace files; type to search");
        });
        cx.simulate_input("lib");
        settle(cx);
        cx.simulate_keystrokes("enter");
        open(&picker, FILES_ACTION, cx);
        settle(cx);
        picker.read_with(cx, |picker, cx| {
            let first = &picker.items()[0];
            assert_eq!((first.path.as_str(), first.recent), ("src/lib.rs", true));
            assert_eq!(picker.items().len(), listed.paths.len(), "listed once");
            assert!(picker.status_text(cx).starts_with("recent files"));
        });
    }

    #[gpui::test]
    fn the_preview_reads_around_the_match(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        cx.update(|_, cx| {
            picker.update(cx, |picker, _| picker.set_file_system(Some(fs.clone())));
        });
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("run();");
        settle(cx);
        // At once from the context grep returned, then from the file.
        picker.read_with(cx, |picker, _| {
            let preview = picker.preview().expect("previewed");
            assert!(preview.loading);
            assert_eq!(preview.match_index(), Some(1));
        });
        cx.executor().advance_clock(preview::PREVIEW_DEBOUNCE);
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            let preview = picker.preview().expect("previewed");
            assert!(!preview.loading);
            assert_eq!(preview.lines, ["fn main() {", "    run();", "}"]);
            assert_eq!(preview.start_line, 1);
            assert_eq!(preview.match_index(), Some(1));
            assert_eq!(
                preview.column_range,
                Some(ColumnRange { start: 4, end: 10 })
            );
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(1200.), px(800.)),
            |_, _| div().relative().size_full().child(picker.clone()),
        );
        // `Alt+P` hides it; moving no longer reads files.
        cx.simulate_keystrokes("alt-p");
        picker.read_with(cx, |picker, _| assert!(!picker.preview_enabled()));
    }

    #[gpui::test]
    fn the_chips_flip_the_options(cx: &mut TestAppContext) {
        let (picker, cx) = picker(fake_fs(), cx);
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("RUN");
        settle(cx);
        picker.read_with(cx, |picker, _| assert_eq!(picker.items().len(), 4));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let case = cx.debug_bounds("picker-case").expect("chip drawn");
        cx.simulate_click(case.center(), gpui::Modifiers::none());
        settle(cx);
        picker.read_with(cx, |picker, _| {
            assert!(picker.options().match_case);
            assert!(picker.items().is_empty(), "no upper-case RUN");
        });
        cx.update(|window, cx| {
            let input = picker.read(cx).input.focus_handle(cx);
            assert!(input.is_focused(window), "the input keeps the focus");
        });
    }

    #[gpui::test]
    fn the_highlighted_row_stays_in_view(cx: &mut TestAppContext) {
        let mut fs = InMemoryFs::new(test_root());
        for n in 0..40 {
            fs = fs.with_file(&format!("src/file_{n:02}.rs"), "\n");
        }
        let (picker, cx) = picker(Arc::new(fs), cx);
        open(&picker, FILES_ACTION, cx);
        settle(cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        for _ in 0..30 {
            cx.simulate_keystrokes("down");
        }
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.selected(), 30);
            let scroll = picker.scroll_handle();
            assert!(scroll.offset().y < px(0.), "the list scrolled");
            assert!(crate::keyscroll::is_visible(scroll, 30), "row 30 in view");
        });
    }

    // -- parity with the agent's tools --------------------------------------

    /// Output of the registered file tool `name` for `input`, as the model
    /// would get it.
    fn tool_output(fs: &Arc<InMemoryFs>, name: &str, input: serde_json::Value) -> String {
        use application::testing::fs::ScriptedShell;
        use application::tools::files::{FileToolPorts, register_file_tools};
        use application::tools::registry::{ToolContext, ToolRegistry};
        use domain::conversation::{SessionId, ToolUseId, TurnId};

        let mut registry = ToolRegistry::new();
        register_file_tools(
            &mut registry,
            FileToolPorts {
                file_search: fs.clone(),
                content_search: fs.clone(),
                fs: fs.clone(),
                shell: Arc::new(ScriptedShell::new()),
                rewriter: None,
            },
        )
        .unwrap();
        let tool = registry.get(name).unwrap();
        let ctx = ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"));
        block_on(async {
            let prepared = tool.prepare(input, &ctx).await.unwrap();
            prepared.run.await.unwrap()
        })
        .text_content()
    }

    #[gpui::test]
    fn file_picker_lists_what_find_file_returns(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("main");
        settle(cx);
        let output = tool_output(&fs, "find_file", serde_json::json!({"query": "main"}));
        let tool_paths: Vec<&str> = output.lines().collect();
        picker.read_with(cx, |picker, _| {
            let paths: Vec<String> = picker.items().iter().map(|i| i.path.to_string()).collect();
            assert_eq!(paths, tool_paths);
        });
    }

    #[gpui::test]
    fn text_picker_lists_what_grep_returns(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (picker, cx) = picker(fs.clone(), cx);
        open(&picker, GREP_ACTION, cx);
        cx.simulate_input("run");
        settle(cx);
        let output = tool_output(
            &fs,
            "grep",
            serde_json::json!({"pattern": "run", "fixed": true, "ignore_case": true}),
        );
        // Match lines are `path:line: text`; context lines use `-` and
        // groups are separated by `--`.
        let tool_matches: Vec<&str> = output
            .lines()
            .filter(|line| {
                line.split_once(':').is_some_and(|(_, rest)| {
                    rest.split_once(": ")
                        .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                })
            })
            .collect();
        assert_eq!(tool_matches.len(), 4);
        picker.read_with(cx, |picker, _| {
            let rows: Vec<String> = picker
                .items()
                .iter()
                .map(|i| {
                    let (line, text) = (
                        i.line.unwrap_or_default(),
                        i.text.clone().unwrap_or_default(),
                    );
                    format!("{}:{line}: {text}", i.path)
                })
                .collect();
            assert_eq!(rows, tool_matches);
        });
    }
}
