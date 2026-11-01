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
//! [`FileSystemPort`] around the matching line. Pictures and 3D models
//! are previewed as stills (the same files `/open` shows as such); other
//! binary files get a card that says so and what to do. `Alt+P` hides or
//! shows the preview.

use std::sync::Arc;
use std::time::Duration;

use application::keymap::{Action, KeyCode};
use application::ports::fs::FileSystemPort;
use application::ports::search::{ContentSearchPort, FileSearchPort, SearchError};
use application::usecases::open_file::{FileKind, file_kind};
use domain::block::BlockId;
use domain::workspace::WorkspacePath;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    KeystrokeEvent, ScrollStrategy, Subscription, Task, UniformListScrollHandle, Window,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{InputEvent, InputState};
use uuid::Uuid;

mod items;
pub mod preview;
mod render;
mod search;

use items::Found;
pub use items::{
    GrepOptions, GrepToggle, PickerItem, PickerMode, count_text, files_request, grep_request,
    items_for_empty_query, items_from_files, items_from_grep, row_text, truncate_left,
};
pub use render::footer_keys;
use search::{read_image, read_model, read_text, run_search};

use preview::{LineStyles, PREVIEW_DEBOUNCE, Preview, syntax_styles};

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
/// Height of the 3D still in the preview.
const MODEL_PREVIEW_HEIGHT: f32 = 320.0;
/// The one image view and the one model view of the preview: `block_state`
/// never drops an entity, so every previewed file reuses these ids (the
/// views swap their content when it changes).
const PREVIEW_IMAGE: BlockId = BlockId(Uuid::from_u128(0x7069_636b_6572_2d70_7265_7669_6577_0001));
const PREVIEW_MODEL: BlockId = BlockId(Uuid::from_u128(0x7069_636b_6572_2d70_7265_7669_6577_0002));

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
    scroll: UniformListScrollHandle,
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
            scroll: UniformListScrollHandle::new(),
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
    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
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

    /// The text selected in the query field, if the picker is open and
    /// there is some.
    pub fn selected_text(&self, cx: &App) -> Option<String> {
        if !self.is_open() {
            return None;
        }
        let input = self.input.read(cx);
        let text = input.value().to_string();
        text.get(input.selected_range())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
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
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
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
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
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
        let kind = file_kind(&item.path);
        if kind == FileKind::Step {
            // Tessellating a part needs the formats adapter; /open has it.
            self.preview_task = None;
            self.set_preview(
                Preview::unsupported(&item, "STEP part (shown by /open)"),
                cx,
            );
            return;
        }
        self.set_preview(Preview::of_context(&item), cx);
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let preview = match kind {
                FileKind::Image => read_image(&fs, &item, cx).await,
                FileKind::Model => read_model(&fs, &item, cx).await,
                FileKind::Text | FileKind::Step => read_text(&fs, &item, cx).await,
            };
            this.update(cx, |this, cx| {
                this.preview_task = None;
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

#[cfg(test)]
mod tests;
