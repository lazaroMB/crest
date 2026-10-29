//! The project picker: the window's content while no project is open.
//!
//! A path field on top lists the directory it names
//! ([`split_typed_path`]): `~/Projects/` lists `~/Projects`, and
//! `~/Projects/cr` keeps its entries matching `cr` ([`filter_dirs`]).
//! Below it, the recent projects (while there is no filter) and the
//! directories, projects first.
//!
//! Keys: `↑`/`↓` choose; `Enter` opens the chosen row as the project (or
//! the typed path when nothing matches); `Ctrl+Enter` opens the directory
//! being listed; `Tab` or `→` enters the chosen directory; `←` with no
//! filter goes up to the parent. A click chooses a row and a double click
//! enters it (opens a recent one).
//!
//! All disk access goes through the [`ProjectBrowserPort`]; choosing emits
//! [`ProjectPickerEvent::Chosen`] and the owner opens the project. Opened
//! from a project (`/project`), `Esc` or **Back** emits
//! [`ProjectPickerEvent::Back`] to return to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use application::ports::projects::{
    BrowseError, DirEntry, DirListing, ProjectBrowserPort, RecentWorkspace,
};
use application::usecases::project_picker::{filter_dirs, split_typed_path};
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// What the picker asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPickerEvent {
    /// Open this directory as the project.
    Chosen(PathBuf),
    /// Go back to the project the picker was opened from.
    Back,
}

/// A recent project and whether it still exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRow {
    /// Its root.
    pub root: PathBuf,
    /// Last save, in milliseconds since the Unix epoch.
    pub updated_at: u64,
    /// Whether it can still be opened.
    pub exists: bool,
}

/// One selectable row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A recent project.
    Recent(RecentRow),
    /// A directory of the listed one.
    Dir(DirEntry),
}

impl Row {
    fn path(&self) -> &Path {
        match self {
            Self::Recent(recent) => &recent.root,
            Self::Dir(dir) => &dir.path,
        }
    }

    fn openable(&self) -> bool {
        !matches!(self, Self::Recent(RecentRow { exists: false, .. }))
    }
}

/// The picker view.
pub struct ProjectPickerView {
    browser: Arc<dyn ProjectBrowserPort>,
    input: Entity<InputState>,
    /// The directory text the listing is for, as typed.
    listed_for: Option<String>,
    listing: Option<DirListing>,
    recent: Vec<RecentRow>,
    selected: usize,
    /// The scroll of the rows, which follows the chosen one.
    scroll: ScrollHandle,
    error: Option<String>,
    notice: Option<String>,
    /// Whether there is a project to go back to.
    can_go_back: bool,
    listing_task: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProjectPickerEvent> for ProjectPickerView {}

impl ProjectPickerView {
    /// A picker listing the browser's start directory.
    pub fn new(
        browser: Arc<dyn ProjectBrowserPort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let start = browser
            .start_dir()
            .map(|dir| with_slash(&dir))
            .unwrap_or_else(|| "/".to_owned());
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Path of the project…");
            state.set_value(start, window, cx);
            state
        });
        let changed = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.select(0);
                this.error = None;
                this.refresh(cx);
            }
        });
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        let mut this = Self {
            browser,
            input,
            listed_for: None,
            listing: None,
            recent: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            error: None,
            notice: None,
            can_go_back: false,
            listing_task: None,
            _tasks: Vec::new(),
            _subscriptions: vec![changed, keys],
        };
        this.refresh(cx);
        this
    }

    /// Shows the recent projects, checking which ones still exist.
    pub fn set_recent(&mut self, recent: Vec<RecentWorkspace>, cx: &mut Context<Self>) {
        let browser = Arc::clone(&self.browser);
        self._tasks.push(cx.spawn(async move |this, cx| {
            let mut rows = Vec::with_capacity(recent.len());
            for workspace in recent {
                let exists = browser.resolve(&workspace.root).await.is_ok();
                rows.push(RecentRow {
                    root: PathBuf::from(workspace.root),
                    updated_at: workspace.updated_at,
                    exists,
                });
            }
            this.update(cx, |this, cx| {
                this.recent = rows;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Offers `Esc` and a **Back** button to return to the open project.
    pub fn set_can_go_back(&mut self, can_go_back: bool, cx: &mut Context<Self>) {
        self.can_go_back = can_go_back;
        cx.notify();
    }

    /// A message under the path field (why a project did not open).
    pub fn set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        self.notice = notice;
        cx.notify();
    }

    /// Lists `dir` (the path field becomes `dir/`).
    pub fn show_dir(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text(&with_slash(dir), window, cx);
    }

    /// The text of the path field.
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The listing shown, once read.
    pub fn listing(&self) -> Option<&DirListing> {
        self.listing.as_ref()
    }

    /// Why the typed directory cannot be listed, if it cannot.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The selectable rows, in order: recent projects (with no filter)
    /// and the matching directories.
    pub fn rows(&self, cx: &App) -> Vec<Row> {
        let text = self.text(cx);
        let typed = split_typed_path(&text);
        let mut rows: Vec<Row> = Vec::new();
        if typed.filter.is_empty() {
            rows.extend(self.recent.iter().cloned().map(Row::Recent));
        }
        if let Some(listing) = &self.listing {
            rows.extend(
                filter_dirs(&listing.dirs, typed.filter)
                    .into_iter()
                    .cloned()
                    .map(Row::Dir),
            );
        }
        rows
    }

    /// Index of the chosen row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The scroll of the rows, which follows the chosen one.
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Chooses row `ix` and scrolls it into view.
    fn select(&mut self, ix: usize) {
        self.selected = ix;
        crate::keyscroll::ensure_visible(&self.scroll, ix);
    }

    /// Gives the focus to the path field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.select(0);
        self.error = None;
        self.refresh(cx);
    }

    /// Lists the directory part of the path field unless it is already.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let dir_text = split_typed_path(&text).dir.to_owned();
        if self.listed_for.as_deref() == Some(dir_text.as_str()) {
            cx.notify();
            return;
        }
        self.listed_for = Some(dir_text.clone());
        let browser = Arc::clone(&self.browser);
        let start = browser.start_dir();
        self.listing_task = Some(cx.spawn(async move |this, cx| {
            let result = list(browser.as_ref(), &dir_text, start).await;
            this.update(cx, |this, cx| {
                if this.listed_for.as_deref() != Some(dir_text.as_str()) {
                    return;
                }
                match result {
                    Ok(listing) => {
                        this.listing = Some(listing);
                        this.error = None;
                    }
                    Err(err) => {
                        this.listing = None;
                        this.error = Some(err.to_string());
                    }
                }
                this.listing_task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let ctrl = keystroke.modifiers.control || keystroke.modifiers.platform;
        let rows = self.rows(cx);
        let text = self.text(cx);
        let no_filter = split_typed_path(&text).filter.is_empty();
        match keystroke.key.as_str() {
            "escape" if self.can_go_back => cx.emit(ProjectPickerEvent::Back),
            "enter" if ctrl => self.open_listed(cx),
            "enter" => self.open_selected(&rows, cx),
            "up" if !rows.is_empty() => {
                self.select((self.selected + rows.len() - 1) % rows.len());
            }
            "down" if !rows.is_empty() => self.select((self.selected + 1) % rows.len()),
            "tab" | "right" => {
                let Some(row) = rows.get(self.selected) else {
                    return;
                };
                if keystroke.key == "right" && !no_filter && !matches!(row, Row::Dir(_)) {
                    return;
                }
                let path = row.path().to_path_buf();
                if row.openable() {
                    self.show_dir(&path, window, cx);
                }
            }
            "left" if no_filter => {
                let Some(parent) = self.listing.as_ref().and_then(|l| l.parent.clone()) else {
                    return;
                };
                self.show_dir(&parent, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// `Enter`: the chosen row, or the typed path when nothing matches.
    fn open_selected(&mut self, rows: &[Row], cx: &mut Context<Self>) {
        match rows.get(self.selected.min(rows.len().saturating_sub(1))) {
            Some(row) if row.openable() => {
                cx.emit(ProjectPickerEvent::Chosen(row.path().to_path_buf()));
            }
            Some(_) => self.error = Some("that project no longer exists".into()),
            None => self.open_typed(cx),
        }
    }

    /// `Ctrl+Enter` or the button: the listed directory.
    fn open_listed(&mut self, cx: &mut Context<Self>) {
        match &self.listing {
            Some(listing) => cx.emit(ProjectPickerEvent::Chosen(listing.path.clone())),
            None => self.open_typed(cx),
        }
    }

    /// The path field as typed, once the browser accepts it.
    fn open_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let browser = Arc::clone(&self.browser);
        self._tasks.push(cx.spawn(async move |this, cx| {
            let result = browser.resolve(&text).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(path) => cx.emit(ProjectPickerEvent::Chosen(path)),
                    Err(err) => this.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn click_row(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = ix;
        let rows = self.rows(cx);
        if event.click_count() >= 2 {
            match rows.get(ix) {
                Some(row @ Row::Recent(_)) => self.open_selected(std::slice::from_ref(row), cx),
                Some(Row::Dir(dir)) => {
                    let path = dir.path.clone();
                    self.show_dir(&path, window, cx);
                }
                None => {}
            }
        }
        self.focus(window, cx);
        cx.notify();
    }

    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let (marker, label, detail, color) = match row {
            Row::Recent(recent) => (
                "★",
                recent.root.display().to_string(),
                if recent.exists {
                    "recent".to_owned()
                } else {
                    "recent · missing".to_owned()
                },
                if recent.exists {
                    theme.foreground
                } else {
                    theme.muted_foreground
                },
            ),
            Row::Dir(dir) => (
                if dir.is_project { "◆" } else { "▸" },
                format!("{}/", dir.name),
                if dir.is_project {
                    "project".to_owned()
                } else {
                    String::new()
                },
                if dir.is_project {
                    theme.primary
                } else {
                    theme.foreground
                },
            ),
        };
        let mut item = h_flex()
            .id(("project-row", ix))
            .debug_selector(move || format!("project-row-{ix}"))
            .gap_2()
            .px_2()
            .cursor_pointer()
            .child(div().text_color(theme.muted_foreground).child(marker))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(detail)),
            );
        if ix == self.selected {
            item = item.bg(theme.list_active).rounded_sm();
        }
        item.on_click(cx.listener(move |this, event, window, cx| {
            this.click_row(ix, event, window, cx);
        }))
        .into_any_element()
    }
}

/// `dir` as the path field shows it, ending in a separator: the one the
/// path already uses, else the platform's (`\` on Windows).
fn with_slash(dir: &Path) -> String {
    let text = dir.display().to_string();
    if text.ends_with(std::path::is_separator) {
        return text;
    }
    let separator = if text.contains('/') && !text.contains('\\') {
        '/'
    } else {
        std::path::MAIN_SEPARATOR
    };
    format!("{text}{separator}")
}

/// The listing for the directory part `dir_text` of the path field; an
/// empty one is the start directory.
async fn list(
    browser: &dyn ProjectBrowserPort,
    dir_text: &str,
    start: Option<PathBuf>,
) -> Result<DirListing, BrowseError> {
    let dir = if dir_text.is_empty() {
        start.ok_or_else(|| BrowseError::NotFound("the start directory".into()))?
    } else {
        browser.resolve(dir_text).await?
    };
    browser.list_dirs(&dir).await
}

impl Focusable for ProjectPickerView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for ProjectPickerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let items: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, cx))
            .collect();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let status = match (&self.error, &self.listing, self.listing_task.is_some()) {
            (Some(error), _, _) => error.clone(),
            (None, _, true) => "listing…".to_owned(),
            (None, Some(listing), false) if listing.truncated => format!(
                "{} directories (more not shown; type to filter)",
                listing.dirs.len()
            ),
            (None, Some(_), false) if rows.is_empty() => "no directories match".to_owned(),
            _ => String::new(),
        };
        let current = self
            .listing
            .as_ref()
            .map(|l| format!("Open {}", l.path.display()))
            .unwrap_or_else(|| "Open the typed path".to_owned());
        let open = div()
            .id("open-listed")
            .debug_selector(|| "open-listed".to_owned())
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.primary)
            .text_color(theme.primary)
            .cursor_pointer()
            .child(SharedString::from(current))
            .on_click(cx.listener(|this, _, _, cx| this.open_listed(cx)));
        div().size_full().flex().justify_center().pt(px(80.)).child(
            v_flex()
                .id("project-picker")
                .w(px(760.))
                .max_w_full()
                .max_h(px(620.))
                .gap_2()
                .p_3()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .child(
                    h_flex()
                        .justify_between()
                        .child(div().text_lg().child("Open a project"))
                        .children(self.can_go_back.then(|| {
                            div()
                                .id("project-back")
                                .debug_selector(|| "project-back".to_owned())
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .child("Back (Esc)")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(ProjectPickerEvent::Back)),
                                )
                        })),
                )
                .child(Input::new(&self.input))
                .children(
                    self.notice
                        .clone()
                        .map(|notice| div().text_color(theme.danger).child(notice)),
                )
                .child(crate::keyscroll::list("project-rows", &self.scroll, items).flex_1())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(muted)
                                .child(SharedString::from(status)),
                        )
                        .child(open),
                )
                .child(div().text_xs().text_color(muted).child(
                    "↑↓ choose · Enter open · Ctrl+Enter open this folder · Tab/→ enter · ← up",
                )),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use application::testing::projects::InMemoryDirs;
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    type Chosen = Rc<RefCell<Vec<PathBuf>>>;

    fn dirs() -> InMemoryDirs {
        InMemoryDirs::new("/home/u")
            .with_project("/home/u/crest")
            .with_dir("/home/u/notes/old")
            .with_dir("/home/u/.cache")
            .with_unreadable("/home/u/locked")
    }

    fn picker(
        browser: InMemoryDirs,
        cx: &mut TestAppContext,
    ) -> (Entity<ProjectPickerView>, &mut VisualTestContext, Chosen) {
        cx.update(gpui_component::init);
        let browser: Arc<dyn ProjectBrowserPort> = Arc::new(browser);
        let (view, cx) =
            cx.add_window_view(|window, cx| ProjectPickerView::new(browser, window, cx));
        let chosen = Chosen::default();
        let seen = chosen.clone();
        cx.update(|window, cx| {
            cx.subscribe(&view, move |_, event: &ProjectPickerEvent, _| {
                if let ProjectPickerEvent::Chosen(path) = event {
                    seen.borrow_mut().push(path.clone());
                }
            })
            .detach();
            view.update(cx, |view, cx| view.focus(window, cx));
        });
        cx.run_until_parked();
        (view, cx, chosen)
    }

    fn names(view: &Entity<ProjectPickerView>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            view.rows(cx)
                .into_iter()
                .map(|row| match row {
                    Row::Recent(recent) => format!("★{}", recent.root.display()),
                    Row::Dir(dir) => dir.name,
                })
                .collect()
        })
    }

    fn set(view: &Entity<ProjectPickerView>, text: &str, cx: &mut VisualTestContext) {
        view.update_in(cx, |view, window, cx| view.set_text(text, window, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn lists_the_start_directory_with_projects_first(cx: &mut TestAppContext) {
        let (view, cx, _) = picker(dirs(), cx);
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/home/u/"));
        assert_eq!(names(&view, cx), ["crest", "locked", "notes"]);
        // Hidden ones once the filter starts with a dot.
        set(&view, "/home/u/.c", cx);
        assert_eq!(names(&view, cx), [".cache"]);
    }

    #[gpui::test]
    fn keys_navigate_and_open(cx: &mut TestAppContext) {
        let (view, cx, chosen) = picker(dirs(), cx);
        // Down to `notes`, enter it, up again.
        cx.simulate_keystrokes("down down tab");
        assert_eq!(names(&view, cx), ["old"]);
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/home/u/notes/"));
        cx.simulate_keystrokes("left");
        assert_eq!(names(&view, cx), ["crest", "locked", "notes"]);
        // Enter opens the chosen row; Ctrl+Enter the listed directory.
        cx.simulate_keystrokes("enter");
        cx.simulate_keystrokes("ctrl-enter");
        assert_eq!(
            *chosen.borrow(),
            [PathBuf::from("/home/u/crest"), PathBuf::from("/home/u")]
        );
    }

    #[gpui::test]
    fn the_chosen_row_stays_in_view(cx: &mut TestAppContext) {
        let mut dirs = InMemoryDirs::new("/home/u");
        for n in 0..40 {
            dirs = dirs.with_dir(&format!("/home/u/dir_{n:02}"));
        }
        let (view, cx, _) = picker(dirs, cx);
        assert_eq!(names(&view, cx).len(), 40);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        for _ in 0..30 {
            cx.simulate_keystrokes("down");
        }
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected(), 30);
            let scroll = view.scroll_handle();
            assert!(scroll.offset().y < px(0.), "the rows scrolled");
            assert!(crate::keyscroll::is_visible(scroll, 30), "row 30 in view");
        });
        // A new filter starts again at the top.
        set(&view, "/home/u/dir_0", cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected(), 0);
            assert!(crate::keyscroll::is_visible(view.scroll_handle(), 0));
        });
    }

    #[gpui::test]
    fn typed_paths_filter_and_open(cx: &mut TestAppContext) {
        let (view, cx, chosen) = picker(dirs(), cx);
        set(&view, "~/no", cx);
        assert_eq!(names(&view, cx), ["notes"]);
        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/home/u/notes/"));
        // Nothing matches: Enter opens the typed path if it exists.
        set(&view, "~/notes/old", cx);
        assert_eq!(names(&view, cx), ["old"]);
        set(&view, "~/crest/", cx);
        assert!(names(&view, cx).is_empty());
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*chosen.borrow(), [PathBuf::from("/home/u/crest")]);
    }

    #[gpui::test]
    fn unreadable_and_missing_directories_say_why(cx: &mut TestAppContext) {
        let (view, cx, chosen) = picker(dirs(), cx);
        set(&view, "/home/u/locked/", cx);
        view.read_with(cx, |view, _| {
            assert!(view.error().unwrap().contains("permission denied"));
            assert!(view.listing().is_none());
        });
        set(&view, "/nowhere/", cx);
        view.read_with(cx, |view, _| {
            assert!(view.error().unwrap().contains("does not exist"));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(chosen.borrow().is_empty());
    }

    #[gpui::test]
    fn recent_projects_come_first_and_missing_ones_do_not_open(cx: &mut TestAppContext) {
        let (view, cx, chosen) = picker(dirs(), cx);
        view.update(cx, |view, cx| {
            view.set_recent(
                vec![
                    RecentWorkspace {
                        root: "/gone".into(),
                        updated_at: 9,
                    },
                    RecentWorkspace {
                        root: "/home/u/crest".into(),
                        updated_at: 5,
                    },
                ],
                cx,
            );
        });
        assert_eq!(
            names(&view, cx),
            ["★/gone", "★/home/u/crest", "crest", "locked", "notes"]
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(chosen.borrow().is_empty());
        view.read_with(cx, |view, _| {
            assert_eq!(view.error(), Some("that project no longer exists"));
        });
        cx.simulate_keystrokes("down enter");
        assert_eq!(*chosen.borrow(), [PathBuf::from("/home/u/crest")]);
        // A filter hides them.
        set(&view, "/home/u/cr", cx);
        assert_eq!(names(&view, cx), ["crest"]);
    }
}
