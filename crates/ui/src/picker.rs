//! File and text pickers: `<leader>ff` (`picker.files`) finds a file and
//! `<leader>fg` (`picker.grep`) searches the workspace contents.
//!
//! Both call the same ports and the same default requests as the agent's
//! `find_file` and `grep` tools ([`files_request`], [`grep_request`]), so
//! the user sees what the agent would see. Results follow the query as it
//! is typed: each keystroke restarts a short debounce, and replacing the
//! search task drops the previous search future, which cancels it (the
//! adapters kill their `rg`/`fzf` processes on drop).

use std::sync::Arc;
use std::time::Duration;

use application::keymap::Action;
use application::ports::search::{
    ContentSearchPort, FileList, FileSearchPort, FindFileRequest, GrepRequest, GrepResult,
    SearchError,
};
use domain::workspace::WorkspacePath;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Action that opens the file picker.
pub const FILES_ACTION: &str = "picker.files";
/// Action that opens the text picker.
pub const GREP_ACTION: &str = "picker.grep";

/// Wait after the last keystroke before searching.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

/// The request the file picker sends for `query`: the defaults of the
/// `find_file` tool (`{"query": …}`).
pub fn files_request(query: &str) -> FindFileRequest {
    FindFileRequest::new(query)
}

/// The request the text picker sends for `query`: the defaults of the
/// `grep` tool (`{"pattern": …}`), so the pattern is a regex.
pub fn grep_request(query: &str) -> GrepRequest {
    GrepRequest::new(query)
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
}

/// Rows of a `find_file` result, in its order.
pub fn items_from_files(list: &FileList) -> Vec<PickerItem> {
    list.paths
        .iter()
        .map(|path| PickerItem {
            path: path.clone(),
            line: None,
            text: None,
        })
        .collect()
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
        })
        .collect()
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
/// move, `Enter` opens, `Esc` closes.
pub struct PickerView {
    files: Arc<dyn FileSearchPort>,
    content: Arc<dyn ContentSearchPort>,
    mode: Option<PickerMode>,
    input: Entity<InputState>,
    items: Vec<PickerItem>,
    selected: usize,
    error: Option<String>,
    search: Option<Task<()>>,
    restore_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for PickerView {}

impl PickerView {
    /// A closed picker over the search ports.
    pub fn new(
        files: Arc<dyn FileSearchPort>,
        content: Arc<dyn ContentSearchPort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let changed = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_search(cx);
            }
        });
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        Self {
            files,
            content,
            mode: None,
            input,
            items: Vec::new(),
            selected: 0,
            error: None,
            search: None,
            restore_focus: None,
            _subscriptions: vec![changed, keys],
        }
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

    /// The error of the last search, if it failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether a search is waiting or running.
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// The query typed so far.
    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Runs `picker.files` or `picker.grep`. Returns whether the action
    /// belongs to the picker.
    pub fn handle_action(
        &mut self,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match PickerMode::for_action(action) {
            Some(mode) => {
                self.open(mode, window, cx);
                true
            }
            None => false,
        }
    }

    /// Opens the picker in `mode` with an empty query and focuses its input.
    pub fn open(&mut self, mode: PickerMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_none() {
            self.restore_focus = window.focused(cx);
        }
        self.mode = Some(mode);
        self.set_query("", window, cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Replaces the query and searches for it.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.schedule_search(cx);
    }

    /// Closes without a choice.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close(window, cx) {
            cx.emit(PickerEvent::Dismissed);
        }
    }

    /// Opens the highlighted row: emits [`PickerEvent::Open`] and closes.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        self.close(window, cx);
        cx.emit(PickerEvent::Open {
            path: item.path,
            line: item.line,
        });
    }

    /// Moves the highlight by `delta` rows, wrapping around.
    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        let len = len as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.mode.take().is_none() {
            return false;
        }
        self.search = None;
        self.items.clear();
        self.error = None;
        if let Some(handle) = self.restore_focus.take() {
            window.focus(&handle, cx);
        }
        cx.notify();
        true
    }

    /// Starts a search for the current query after the debounce. The task
    /// it replaces, and the search future it owns, are dropped.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.mode else {
            return;
        };
        let query = self.query(cx);
        if query.trim().is_empty() {
            self.search = None;
            self.items.clear();
            self.selected = 0;
            self.error = None;
            cx.notify();
            return;
        }
        let files = self.files.clone();
        let content = self.content.clone();
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let result = cx
                .background_spawn(async move { run_search(mode, &query, files, content).await })
                .await;
            this.update(cx, |this, cx| this.found(result, cx)).ok();
        }));
    }

    fn found(&mut self, result: Result<Vec<PickerItem>, SearchError>, cx: &mut Context<Self>) {
        self.search = None;
        match result {
            Ok(items) => {
                self.items = items;
                self.error = None;
            }
            Err(err) => {
                self.items.clear();
                self.error = Some(err.to_string());
            }
        }
        self.selected = 0;
        cx.notify();
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode.is_none() {
            return;
        }
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        if !focused {
            return;
        }
        let keystroke = &event.keystroke;
        let ctrl = keystroke.modifiers.control;
        match keystroke.key.as_str() {
            "escape" => self.dismiss(window, cx),
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
    files: Arc<dyn FileSearchPort>,
    content: Arc<dyn ContentSearchPort>,
) -> Result<Vec<PickerItem>, SearchError> {
    match mode {
        PickerMode::Files => files
            .find_file(files_request(query))
            .await
            .map(|list| items_from_files(&list)),
        PickerMode::Grep => content
            .grep(grep_request(query))
            .await
            .map(|result| items_from_grep(&result)),
    }
}

impl PickerView {
    fn render_item(
        &self,
        ix: usize,
        item: &PickerItem,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let location = match item.line {
            Some(line) => format!("{}:{line}", item.path),
            None => item.path.to_string(),
        };
        let mut row = h_flex()
            .id(ix)
            .gap_2()
            .px_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(SharedString::from(location))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ix;
                this.confirm(window, cx);
            }));
        if let Some(text) = &item.text {
            row = row.child(
                div()
                    .text_color(theme.muted_foreground)
                    .text_ellipsis()
                    .child(SharedString::from(text.trim().to_owned())),
            );
        }
        if ix == self.selected {
            row = row.bg(theme.list_active);
        }
        row
    }
}

impl Render for PickerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(mode) = self.mode else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let status = match (&self.error, self.items.len()) {
            (Some(error), _) => error.clone(),
            (None, 0) if self.search.is_some() => "searching…".to_owned(),
            (None, 0) => "no results".to_owned(),
            (None, n) => format!("{n} results"),
        };
        let (bg, border, muted) = (theme.popover, theme.border, theme.muted_foreground);
        let rows: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| self.render_item(ix, item, cx).into_any_element())
            .collect();
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .child(
                v_flex()
                    .w(px(720.))
                    .max_w_full()
                    .max_h(px(520.))
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
                            .child(div().flex_1().child(Input::new(&self.input))),
                    )
                    .child(
                        div()
                            .id("picker-results")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .child(div().text_color(muted).child(SharedString::from(status))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use application::ports::search::{ListFilesRequest, TreeNode, TreeRequest};
    use application::testing::fs::InMemoryFs;
    use domain::workspace::WorkspaceRoot;
    use futures::executor::block_on;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    fn fake_fs() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
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
        let grep: GrepRequest = serde_json::from_str(r#"{"pattern":"run"}"#).unwrap();
        assert_eq!(grep_request("run"), grep);
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
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input("lib");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert!(picker.items().is_empty());
            assert!(picker.is_searching());
        });
        settle(cx);
        picker.read_with(cx, |picker, _| assert_eq!(picker.items().len(), 1));
        // Clearing the query clears the results without searching.
        cx.simulate_keystrokes("backspace backspace backspace");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert!(picker.items().is_empty());
            assert!(!picker.is_searching());
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
        let expected = block_on(fs.grep(grep_request("run"))).unwrap();
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
        // `(` is a regex metacharacter, so the request fails like the tool's.
        picker.read_with(cx, |picker, _| {
            assert!(picker.items().is_empty());
            assert!(
                picker
                    .error()
                    .is_some_and(|e| e.contains("invalid pattern"))
            );
        });
        cx.simulate_keystrokes("backspace backspace");
        settle(cx);
        cx.simulate_keystrokes("ctrl-n enter");
        let expected = block_on(fs.grep(grep_request("run"))).unwrap();
        assert_eq!(
            *events.borrow(),
            [PickerEvent::Open {
                path: expected.hits[1].path.clone(),
                line: Some(expected.hits[1].line_number),
            }]
        );
    }
}
