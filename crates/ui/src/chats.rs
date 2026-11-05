//! The chats sidebar: a new chat, then each project already opened and,
//! under it, its chats, newest first.
//!
//! The binary fills the list ([`crate::workbench::ProjectOpener::catalog`]).
//! Choosing a row emits [`ChatsEvent`]; the workbench opens that session.
//! A right click renames or deletes the chat; `Esc`, **Cancel** or a click
//! elsewhere closes that menu. The list scrolls with a scrollbar; a
//! project without chats says so. The file tree stays the other side
//! panel.
//!
//! The list is in the `focus.next` cycle. With the focus, `Up`/`Down` (or
//! `k`/`j`), `Home`/`End` move the selection, kept in view; `Enter` opens
//! the chat (or folds the project), `Left`/`Right` fold and unfold,
//! `F2` or `r` renames and `Delete` asks to delete, `Enter` confirming.
//! In the name field `Enter` saves (the field's own event, not a key
//! check), so does losing the focus or a click elsewhere after a change,
//! and `Esc` cancels. The new name shows at once, before it is saved.
//! The keymap's `chat.*` actions reach the window as [`ChatsCommand`].

use std::collections::{HashMap, HashSet};

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Global,
    IntoElement, KeyDownEvent, KeystrokeEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, Styled as _, Subscription, WeakEntity, WeakFocusHandle, Window, div,
};
use gpui_component::input::{InputEvent, InputState};

mod model;
mod rail;
mod rows;
mod search;

use model::{CurrentChat, listed_with, visible_chats, visible_projects};
pub use model::{MARK_BAND, STRIP_WIDTH, SidebarWidth};

/// Which session to open in a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOpen {
    /// The latest saved session, or a new one when the project has none.
    Latest,
    /// A new empty session in the directory, joining the project the
    /// directory has. The previous one stays saved.
    New,
    /// A new empty session in the project with this id, in its first
    /// directory.
    NewIn(String),
    /// A new empty session with no directory and no project.
    NewStandalone,
    /// The session with this id.
    Id(String),
}

/// One saved chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEntry {
    /// Session id, as printed.
    pub id: String,
    /// What the row shows.
    pub title: String,
}

/// A project and its chats, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChats {
    /// Project id, as printed (a directory without a project uses its root).
    pub id: String,
    /// Name shown on the row.
    pub name: String,
    /// Its first directory, as stored: where its chats open.
    pub root: String,
    /// How many directories it has.
    pub dirs: usize,
    /// Its chats, newest first.
    pub chats: Vec<ChatEntry>,
}

/// What the sidebar lists: the projects in their order, and the chats that
/// are in none, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Projects, in the order the sidebar shows them.
    pub projects: Vec<ProjectChats>,
    /// Chats with no project.
    pub chats: Vec<ChatEntry>,
}

/// What the panel asks the window to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatsEvent {
    /// An empty chat where the open one is: in its project, or in none.
    New,
    /// An empty chat in no project and with no directory.
    NewStandalone,
    /// An empty chat in the project `project`, in its first directory `root`.
    NewIn {
        /// Project id.
        project: String,
        /// Its first directory.
        root: String,
    },
    /// The chat `id` of the project `root`.
    Open {
        /// Workspace root of the chat.
        root: String,
        /// Session id.
        id: String,
    },
    /// Store `title` on the chat `id`.
    Rename {
        /// Session id.
        id: String,
        /// The name to keep.
        title: String,
    },
    /// Remove the chat `id` of the project `root`.
    Delete {
        /// Workspace root of the chat.
        root: String,
        /// Session id.
        id: String,
    },
    /// The sidebar changed width (a click on the mark, the cycle key):
    /// the window tells the session and moves the keyboard if needed.
    Sidebar(SidebarWidth),
    /// The menu was cancelled (`Esc`, **Cancel**): the prompt takes the
    /// focus back from the rename field.
    Dismissed,
}

/// What a key asks the window to do with the chats (the keymap's `chat.*`
/// and `panel.chats` actions, and `focus.next` into the list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatsCommand {
    /// `chat.new`: an empty chat in the open project.
    New,
    /// `chat.next`: the chat below the open one (older), wrapping.
    Next,
    /// `chat.prev`: the chat above the open one (newer), wrapping.
    Prev,
    /// `chat.rename`: the name field of the open chat.
    Rename,
    /// `chat.delete`: asks to delete the open chat.
    Delete,
    /// `panel.chats`: the next width (full, strip, hidden).
    TogglePanel,
    /// `focus.next` reached the list.
    Focus,
}

impl ChatsCommand {
    /// The command of the builtin action `name`, if it is a chats one.
    #[must_use]
    pub fn for_action(name: &str) -> Option<Self> {
        Some(match name {
            "chat.new" => Self::New,
            "chat.next" => Self::Next,
            "chat.prev" => Self::Prev,
            "chat.rename" => Self::Rename,
            "chat.delete" => Self::Delete,
            "panel.chats" => Self::TogglePanel,
            _ => return None,
        })
    }
}

/// A row of the list, as the keyboard selects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRow {
    /// The header of the project `id`.
    Project(String),
    /// The chat `id`, of the project `project` or of none.
    Chat {
        /// Project id; `None` for a chat in no project.
        project: Option<String>,
        /// Session id.
        id: String,
    },
}

/// What the right-click menu is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatActionKind {
    /// Rename and Delete.
    Menu,
    /// The name field is open.
    Rename,
    /// Delete still needs a second click.
    ConfirmDelete,
}

/// The chat whose menu is open.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatAction {
    id: String,
    root: String,
    title: String,
    kind: ChatActionKind,
}

/// The rename field, while it exists. The prompt leaves its keys alone
/// when this field has the focus.
struct ChatRename(WeakEntity<InputState>);

impl Global for ChatRename {}

/// The search field, while it exists (same rule as [`ChatRename`]).
struct ChatSearch(WeakEntity<InputState>);

impl Global for ChatSearch {}

fn focused_input(input: Option<&WeakEntity<InputState>>, window: &Window, cx: &App) -> bool {
    input
        .and_then(WeakEntity::upgrade)
        .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
}

/// Whether the rename field is the focused text input.
pub fn rename_has_focus(window: &Window, cx: &App) -> bool {
    let active = cx.try_global::<ChatRename>();
    focused_input(active.map(|active| &active.0), window, cx)
}

/// Whether a text field of the sidebar (rename or search) has the focus.
/// The prompt leaves the keys of both alone.
pub fn field_has_focus(window: &Window, cx: &App) -> bool {
    let search = cx.try_global::<ChatSearch>();
    rename_has_focus(window, cx) || focused_input(search.map(|active| &active.0), window, cx)
}

/// The focus handles of the chats lists (one per window). The prompt
/// leaves the list's keys alone ([`list_has_focus`]).
#[derive(Default)]
struct ChatLists(Vec<WeakFocusHandle>);

impl Global for ChatLists {}

/// Whether a chats list has the focus (not its rename field).
pub fn list_has_focus(window: &Window, cx: &App) -> bool {
    cx.try_global::<ChatLists>().is_some_and(|lists| {
        lists
            .0
            .iter()
            .filter_map(WeakFocusHandle::upgrade)
            .any(|handle| handle.is_focused(window))
    })
}

/// The sidebar of projects and chats.
pub struct ChatsPanel {
    /// The catalog as given; `listed` adds the open chat to it.
    catalog: Catalog,
    listed: Catalog,
    /// What the search leaves of `listed`: the rows the list shows.
    visible: Catalog,
    /// How wide the sidebar is.
    width: SidebarWidth,
    /// The pointer is on the hidden sidebar's mark or its revealed rows.
    pointer_on_rail: bool,
    /// The strip's project (its root) under the pointer.
    pointer_on_project: Option<String>,
    /// The search field, while it is open (full width only), and what it holds.
    search: Option<Entity<InputState>>,
    query: String,
    /// The search field's events; dropped with it.
    _search_events: Option<Subscription>,
    expanded: HashSet<String>,
    current_root: String,
    current_project: Option<String>,
    current_id: Option<String>,
    /// Chats with a turn running, by id: whether they wait on the user.
    activity: HashMap<String, bool>,
    action: Option<ChatAction>,
    rename_input: Option<Entity<InputState>>,
    /// The rename field's events (`Enter`, focus lost); dropped with it.
    _rename_events: Option<Subscription>,
    /// Titles saved from the rename field, by chat id, shown until the
    /// catalog has them (the save may wait for a running turn).
    pending_titles: HashMap<String, String>,
    /// The scroll of the list: one item per row ([`Self::rows`]).
    scroll: ScrollHandle,
    /// The row the keyboard is on.
    selected: Option<ChatRow>,
    focus_handle: FocusHandle,
    _keys: Subscription,
}

impl ChatsPanel {
    /// A panel for the session that is open. The catalog arrives with
    /// [`Self::set_projects`].
    pub fn new(current_root: String, current_id: Option<String>, cx: &mut Context<Self>) -> Self {
        let current = CurrentChat {
            root: &current_root,
            project: None,
            id: current_id.as_deref(),
        };
        let listed = listed_with(Catalog::default(), &current);
        let mut expanded = HashSet::new();
        if let Some(project) = listed.projects.first() {
            expanded.insert(project.id.clone());
        }
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.on_key(event, window, cx))
                .ok();
        });
        let focus_handle = cx.focus_handle();
        let lists = cx.default_global::<ChatLists>();
        lists.0.retain(|handle| handle.upgrade().is_some());
        lists.0.push(focus_handle.downgrade());
        Self {
            catalog: Catalog::default(),
            visible: listed.clone(),
            listed,
            width: SidebarWidth::default(),
            pointer_on_rail: false,
            pointer_on_project: None,
            search: None,
            query: String::new(),
            _search_events: None,
            expanded,
            current_root,
            current_project: None,
            current_id,
            activity: HashMap::new(),
            action: None,
            rename_input: None,
            _rename_events: None,
            pending_titles: HashMap::new(),
            scroll: ScrollHandle::new(),
            selected: None,
            focus_handle,
            _keys: keys,
        }
    }

    /// The rows as the list shows them: each project, then its chats when
    /// it is unfolded.
    #[must_use]
    pub fn rows(&self) -> Vec<ChatRow> {
        let mut rows = Vec::new();
        for project in &self.visible.projects {
            rows.push(ChatRow::Project(project.id.clone()));
            if self.is_open(&project.id) {
                rows.extend(project.chats.iter().map(|chat| ChatRow::Chat {
                    project: Some(project.id.clone()),
                    id: chat.id.clone(),
                }));
            }
        }
        rows.extend(self.visible.chats.iter().map(|chat| ChatRow::Chat {
            project: None,
            id: chat.id.clone(),
        }));
        rows
    }

    /// The row the keyboard is on, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&ChatRow> {
        self.selected.as_ref()
    }

    /// The index of the selected row in [`Self::rows`].
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.rows().iter().position(|row| row == selected)
    }

    /// The row of the open chat, if the list shows it.
    fn current_row(&self) -> Option<ChatRow> {
        let id = self.current_id.as_deref()?;
        self.rows()
            .into_iter()
            .find(|row| matches!(row, ChatRow::Chat { id: row_id, .. } if row_id == id))
    }

    /// Gives the list the keyboard, on the open chat when nothing is
    /// selected.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_index().is_none() {
            self.selected = self
                .current_row()
                .or_else(|| self.rows().into_iter().next());
        }
        self.reveal_selected();
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Moves the selection to row `ix` (clamped) and keeps it in view.
    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = self.rows();
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        self.selected = rows.into_iter().nth(ix.min(last));
        self.reveal_selected();
        cx.notify();
    }

    fn reveal_selected(&self) {
        if let Some(ix) = self.selected_index() {
            crate::keyscroll::ensure_visible(&self.scroll, ix);
        }
    }

    /// The chat `step` rows away from the open one in its project
    /// (positive: older), wrapping; `None` when it has no other chat.
    #[must_use]
    pub fn neighbor(&self, step: isize) -> Option<(String, String)> {
        let id = self.current_id.as_deref()?;
        let (root, chats) = self.group_of(id)?;
        let len = chats.len();
        let at = chats.iter().position(|chat| chat.id == id)?;
        if len < 2 {
            return None;
        }
        let len = isize::try_from(len).ok()?;
        let at = isize::try_from(at).ok()?;
        let next = usize::try_from((at + step).rem_euclid(len)).ok()?;
        Some((root.to_owned(), chats[next].id.clone()))
    }

    /// The chats the chat `id` is listed with (its project's, or the ones in
    /// no project) and the directory they open in (empty without a project).
    fn group_of(&self, id: &str) -> Option<(&str, &[ChatEntry])> {
        let listed = |chats: &[ChatEntry]| chats.iter().any(|chat| chat.id == id);
        let project = self
            .listed
            .projects
            .iter()
            .find(|project| listed(&project.chats));
        match project {
            Some(project) => Some((project.root.as_str(), project.chats.as_slice())),
            None => listed(&self.listed.chats).then_some(("", self.listed.chats.as_slice())),
        }
    }

    /// The title the list shows for the chat `id`, if listed.
    #[must_use]
    pub fn title_of(&self, id: &str) -> Option<String> {
        self.listed
            .projects
            .iter()
            .flat_map(|project| &project.chats)
            .chain(&self.listed.chats)
            .find(|chat| chat.id == id)
            .map(|chat| chat.title.clone())
    }

    /// Opens the name field of the open chat (`chat.rename`).
    pub fn rename_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row @ ChatRow::Chat { .. }) = self.current_row() {
            self.selected = Some(row.clone());
            self.act_on_row(&row, ChatActionKind::Rename, window, cx);
        }
    }

    /// Asks to delete the open chat (`chat.delete`); the list takes the
    /// focus so `Enter` confirms and `Esc` cancels.
    pub fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row @ ChatRow::Chat { .. }) = self.current_row() {
            self.selected = Some(row.clone());
            self.act_on_row(&row, ChatActionKind::ConfirmDelete, window, cx);
            window.focus(&self.focus_handle, cx);
        }
    }

    /// [`Self::act_on`] for a chat row (anything else does nothing).
    fn act_on_row(
        &mut self,
        row: &ChatRow,
        kind: ChatActionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let ChatRow::Chat { id, .. } = row {
            let root = self
                .group_of(id)
                .map_or_else(String::new, |(root, _)| root.to_owned());
            self.act_on(root, id.clone(), kind, window, cx);
        }
    }

    /// Opens the menu of the chat `id` in `kind`.
    fn act_on(
        &mut self,
        root: String,
        id: String,
        kind: ChatActionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.title_of(&id).unwrap_or_else(|| chat_label(None));
        self.drop_rename();
        self.action = Some(ChatAction {
            id,
            root,
            title,
            kind: ChatActionKind::Menu,
        });
        match kind {
            ChatActionKind::Rename => self.begin_rename(window, cx),
            ChatActionKind::ConfirmDelete => self.ask_delete(cx),
            ChatActionKind::Menu => {}
        }
        self.reveal_selected();
        cx.notify();
    }

    /// Keys while the list has the focus.
    fn on_list_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the rename field bubble up here: `r` or `Enter`
        // there are text and the field's own.
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        if self.width != SidebarWidth::Full {
            self.on_rail_key(event.keystroke.key.as_str(), cx);
            return;
        }
        let confirming = self
            .action
            .as_ref()
            .is_some_and(|action| action.kind == ChatActionKind::ConfirmDelete);
        let key = event.keystroke.key.as_str();
        let rows = self.rows();
        let at = self.selected_index();
        match key {
            "enter" | "y" if confirming => self.commit_delete(cx),
            "up" | "k" => self.select_index(at.map_or(0, |ix| ix.saturating_sub(1)), cx),
            "down" | "j" => self.select_index(at.map_or(0, |ix| ix + 1), cx),
            "home" => self.select_index(0, cx),
            "end" => self.select_index(rows.len().saturating_sub(1), cx),
            "enter" | "right" | "left" | "f2" | "r" | "delete" => {
                let Some(row) = self.selected.clone() else {
                    return;
                };
                self.row_key(key, row, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// What `key` does on `row`.
    fn row_key(&mut self, key: &str, row: ChatRow, window: &mut Window, cx: &mut Context<Self>) {
        match (key, row) {
            ("enter", ChatRow::Project(root)) => self.toggle(&root, cx),
            ("right", ChatRow::Project(root)) => {
                if !self.expanded.contains(&root) {
                    self.toggle(&root, cx);
                }
            }
            ("left", ChatRow::Project(root)) => {
                if self.expanded.contains(&root) {
                    self.toggle(&root, cx);
                }
            }
            (
                "left",
                ChatRow::Chat {
                    project: Some(project),
                    ..
                },
            ) => {
                self.selected = Some(ChatRow::Project(project));
                self.reveal_selected();
                cx.notify();
            }
            ("enter", ChatRow::Chat { id, .. }) => {
                self.clear_action(cx);
                if self.current_id.as_deref() == Some(id.as_str()) {
                    cx.emit(ChatsEvent::Dismissed);
                } else {
                    let root = self
                        .group_of(&id)
                        .map_or_else(String::new, |(root, _)| root.to_owned());
                    cx.emit(ChatsEvent::Open { root, id });
                }
            }
            ("f2" | "r", row @ ChatRow::Chat { .. }) => {
                self.act_on_row(&row, ChatActionKind::Rename, window, cx);
            }
            ("delete", row @ ChatRow::Chat { .. }) => {
                self.act_on_row(&row, ChatActionKind::ConfirmDelete, window, cx);
            }
            _ => {}
        }
    }

    /// Forgets the title being saved on the chat `id` (the save failed):
    /// the list shows the catalog's again.
    pub fn forget_title(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.pending_titles.remove(id).is_some() {
            self.rebuild();
            cx.notify();
        }
    }

    /// Marks the chat `id` as the open one, in the project `project` (none
    /// for a chat with no project) and the directory `root` (empty for a
    /// chat with none).
    pub fn set_current(
        &mut self,
        root: String,
        project: Option<String>,
        id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.current_root = root;
        self.current_project = project;
        self.current_id = id;
        self.rebuild();
        cx.notify();
    }

    /// Shows on the row of the chat `id` that a turn runs (`busy`) and
    /// whether it waits on the user (`waiting`: an approval or a form).
    pub fn set_activity(&mut self, id: &str, busy: bool, waiting: bool, cx: &mut Context<Self>) {
        if busy {
            self.activity.insert(id.to_owned(), waiting);
        } else {
            self.activity.remove(id);
        }
        cx.notify();
    }

    /// The scroll of the list, one item per row ([`Self::rows`]).
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Another chat of the group of `id` to open after deleting it, if one
    /// remains.
    #[must_use]
    pub fn next_chat(&self, id: &str) -> Option<String> {
        let (_, chats) = self.group_of(id)?;
        next_chat_id(chats, id).map(str::to_owned)
    }

    /// Replaces the list. The open project stays expanded, and the open
    /// chat stays visible even before it has been saved.
    pub fn set_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        // A saved title is the catalog's from now on.
        self.pending_titles.retain(|id, title| {
            !catalog
                .projects
                .iter()
                .flat_map(|project| &project.chats)
                .chain(&catalog.chats)
                .any(|chat| &chat.id == id && &chat.title == title)
        });
        self.catalog = catalog;
        self.rebuild();
        cx.notify();
    }

    fn rebuild(&mut self) {
        let current = CurrentChat {
            root: &self.current_root,
            project: self.current_project.as_deref(),
            id: self.current_id.as_deref(),
        };
        let mut listed = listed_with(self.catalog.clone(), &current);
        let titled = listed
            .projects
            .iter_mut()
            .flat_map(|project| &mut project.chats)
            .chain(&mut listed.chats);
        for chat in titled {
            if let Some(title) = self.pending_titles.get(&chat.id) {
                chat.title.clone_from(title);
            }
        }
        if let Some(project) = listed.projects.iter().find(|p| current.is_in(p)) {
            self.expanded.insert(project.id.clone());
        }
        self.listed = listed;
        self.refilter();
    }

    /// Lists again what the search leaves; the selection stays if its row does.
    fn refilter(&mut self) {
        self.visible = Catalog {
            projects: visible_projects(&self.listed.projects, &self.query),
            chats: visible_chats(&self.listed.chats, &self.query),
        };
        if self.selected_index().is_none() {
            self.selected = None;
        }
    }

    fn toggle(&mut self, project: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(project) {
            self.expanded.insert(project.to_owned());
        }
        cx.notify();
    }

    /// `Esc` closes the menu (and the rename field) before the field or
    /// the session take it. `Enter` in the field is not handled here: it
    /// arrives as [`InputEvent::PressEnter`] ([`Self::on_rename_event`]),
    /// which does not depend on a focus check made before the key reaches
    /// the field.
    fn on_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "escape" {
            return;
        }
        if self.action.is_some() {
            self.cancel_action(cx);
            cx.stop_propagation();
        } else if focused_input(
            self.search.as_ref().map(Entity::downgrade).as_ref(),
            window,
            cx,
        ) {
            self.close_search(cx);
            cx.emit(ChatsEvent::Dismissed);
            cx.stop_propagation();
        }
    }

    /// The rename field's `Enter` saves the name; losing the focus saves
    /// it too when it changed (an unchanged name leaves the field open).
    fn on_rename_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if self.rename_input.as_ref() != Some(input) {
            return;
        }
        let value = input.read(cx).value().to_string();
        match event {
            InputEvent::PressEnter { .. } => self.commit_rename(value, cx),
            InputEvent::Blur if self.rename_changed(&value) => self.commit_rename(value, cx),
            _ => {}
        }
    }

    /// Whether `value` names the chat differently from its title.
    fn rename_changed(&self, value: &str) -> bool {
        let original = self.action.as_ref().map(|action| action.title.as_str());
        clean_title(value).is_some_and(|title| Some(title.as_str()) != original)
    }

    /// A click outside the menu: the rename field saves a changed name,
    /// anything else just closes.
    fn click_outside(&mut self, cx: &mut Context<Self>) {
        let value = self
            .rename_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string());
        match value {
            Some(value) if self.rename_changed(&value) => self.commit_rename(value, cx),
            _ => self.clear_action(cx),
        }
    }

    /// Drops the rename field and its subscription (no `Blur` follows).
    fn drop_rename(&mut self) {
        self.rename_input = None;
        self._rename_events = None;
    }

    fn open_menu(
        &mut self,
        id: String,
        root: String,
        title: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_menu = self
            .action
            .as_ref()
            .is_some_and(|action| action.id == id && matches!(action.kind, ChatActionKind::Menu));
        if same_menu {
            self.clear_action(cx);
            return;
        }
        self.drop_rename();
        self.action = Some(ChatAction {
            id,
            root,
            title,
            kind: ChatActionKind::Menu,
        });
        cx.notify();
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.action.as_ref().map(|action| action.title.clone()) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Chat name");
            state.set_value(title.clone(), window, cx);
            state
        });
        let handle = input.read(cx).focus_handle(cx);
        // The click that opened the field finishes after this handler, so
        // the focus is set again once that click is done.
        window.focus(&handle, cx);
        window.defer(cx, {
            let handle = handle.clone();
            move |window, cx| window.focus(&handle, cx)
        });
        cx.set_global(ChatRename(input.downgrade()));
        self._rename_events = Some(cx.subscribe(&input, |this, input, event, cx| {
            this.on_rename_event(&input, event, cx);
        }));
        self.rename_input = Some(input);
        if let Some(action) = &mut self.action {
            action.kind = ChatActionKind::Rename;
        }
        cx.notify();
    }

    fn ask_delete(&mut self, cx: &mut Context<Self>) {
        self.drop_rename();
        if let Some(action) = &mut self.action {
            action.kind = ChatActionKind::ConfirmDelete;
        }
        cx.notify();
    }

    fn commit_rename(&mut self, raw: String, cx: &mut Context<Self>) {
        let Some(action) = self.action.clone() else {
            return;
        };
        self.clear_action(cx);
        let title = clean_title(&raw).filter(|title| *title != action.title);
        let Some(title) = title else {
            // Nothing to save: the prompt takes the focus back.
            cx.emit(ChatsEvent::Dismissed);
            return;
        };
        // Shown at once: the save may wait for a turn of that chat.
        self.pending_titles.insert(action.id.clone(), title.clone());
        self.rebuild();
        cx.emit(ChatsEvent::Rename {
            id: action.id,
            title,
        });
    }

    fn commit_delete(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.action.clone() else {
            return;
        };
        self.clear_action(cx);
        cx.emit(ChatsEvent::Delete {
            root: action.root,
            id: action.id,
        });
    }

    /// Whether the right-click menu (or its rename field or delete
    /// question) is open.
    pub fn menu_open(&self) -> bool {
        self.action.is_some()
    }

    /// Closes the menu without doing anything and gives the focus back.
    fn cancel_action(&mut self, cx: &mut Context<Self>) {
        self.clear_action(cx);
        cx.emit(ChatsEvent::Dismissed);
    }

    fn clear_action(&mut self, cx: &mut Context<Self>) {
        self.action = None;
        self.drop_rename();
        cx.notify();
    }
}

impl EventEmitter<ChatsEvent> for ChatsPanel {}

impl Focusable for ChatsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.width {
            SidebarWidth::Full => rail::full(self, window, cx),
            SidebarWidth::Strip => rail::strip(self, cx),
            SidebarWidth::Hidden => rail::hidden(self, window, cx),
        }
    }
}

fn label(text: &str) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(SharedString::from(text.to_owned()))
}

/// The directory name of `root`. Both slash styles count, so a Windows
/// path keeps its last folder on macOS too.
#[must_use]
pub fn project_name(root: &str) -> String {
    root.replace('\\', "/")
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(root)
        .to_owned()
}

/// The name to store, trimmed. Empty input is cancelled. Longer names
/// keep their first 120 characters.
#[must_use]
pub fn clean_title(raw: &str) -> Option<String> {
    let title = raw.trim();
    if title.is_empty() {
        return None;
    }
    let title: String = title.chars().take(120).collect();
    Some(title)
}

/// The first chat that is not `id`, to open after a delete.
#[must_use]
pub fn next_chat_id<'a>(chats: &'a [ChatEntry], id: &str) -> Option<&'a str> {
    chats
        .iter()
        .find(|chat| chat.id != id)
        .map(|chat| chat.id.as_str())
}

/// The row text of a chat: its title, or "New chat" when it has none.
#[must_use]
pub fn chat_label(title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("New chat")
        .to_owned()
}

/// The two roots name the same directory. Slashes are ignored; on Windows
/// the comparison is case-insensitive.
pub fn same_workspace(left: &str, right: &str) -> bool {
    let normalize = |root: &str| root.replace('\\', "/").trim_end_matches('/').to_owned();
    let (left, right) = (normalize(left), normalize(right));
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

#[cfg(test)]
mod grouped_tests;

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, px};

    use super::grouped_tests::click;
    use super::*;

    fn panel<'a>(
        projects: Vec<ProjectChats>,
        current_id: Option<&str>,
        cx: &'a mut TestAppContext,
    ) -> (Entity<ChatsPanel>, &'a mut VisualTestContext) {
        cx.update(gpui_component::init);
        let current_id = current_id.map(str::to_owned);
        let (view, cx) = cx.add_window_view(move |_, cx| {
            let mut panel = ChatsPanel::new("/work/crest".into(), current_id, cx);
            panel.set_catalog(catalog(projects), cx);
            panel.set_width(SidebarWidth::Full, cx);
            panel
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn a_project_without_chats_says_so(cx: &mut TestAppContext) {
        let (_view, cx) = panel(Vec::new(), None, cx);
        assert!(cx.debug_bounds("chats-empty").is_some());
    }

    #[gpui::test]
    fn a_long_list_scrolls(cx: &mut TestAppContext) {
        let ids: Vec<String> = (0..80).map(|n| format!("chat-{n}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (view, cx) = panel(vec![project("/work/crest", &ids)], Some("chat-0"), cx);
        assert!(cx.debug_bounds("chats-empty").is_none());
        view.read_with(cx, |view, _| {
            assert!(
                view.scroll_handle().max_offset().y > px(0.),
                "the rows overflow the panel and the list tracks it"
            );
        });
    }

    /// A panel on the chat `a` of `/work/crest` (also listing `b`), with
    /// its events recorded.
    fn renaming(
        cx: &mut TestAppContext,
    ) -> (
        Entity<ChatsPanel>,
        std::rc::Rc<std::cell::RefCell<Vec<ChatsEvent>>>,
        &mut VisualTestContext,
    ) {
        let (view, cx) = panel(vec![project("/work/crest", &["a", "b"])], Some("a"), cx);
        cx.update(|window, _| window.activate_window());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = std::rc::Rc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ChatsEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        view.update_in(cx, |panel, window, cx| panel.rename_current(window, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(cx.update(|window, cx| rename_has_focus(window, cx)));
        (view, events, cx)
    }

    fn field(view: &Entity<ChatsPanel>, cx: &mut VisualTestContext) -> Entity<InputState> {
        view.read_with(cx, |panel, _| panel.rename_input.clone())
            .expect("the rename field is open")
    }

    fn type_name(view: &Entity<ChatsPanel>, name: &str, cx: &mut VisualTestContext) {
        let input = field(view, cx);
        input.update_in(cx, |state, window, cx| state.set_value(name, window, cx));
    }

    fn renamed(events: &std::cell::RefCell<Vec<ChatsEvent>>) -> Vec<(String, String)> {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                ChatsEvent::Rename { id, title } => Some((id.clone(), title.clone())),
                _ => None,
            })
            .collect()
    }

    fn listed_title(view: &Entity<ChatsPanel>, id: &str, cx: &mut VisualTestContext) -> String {
        view.read_with(cx, |panel, _| panel.title_of(id))
            .expect("the chat is listed")
    }

    #[gpui::test]
    fn the_fields_enter_event_saves_the_name(cx: &mut TestAppContext) {
        let (view, events, cx) = renaming(cx);
        type_name(&view, "  Nuevo nombre ", cx);
        // The field's own event, whatever the key routing before it.
        field(&view, cx).update(cx, |_, cx| {
            cx.emit(InputEvent::PressEnter {
                secondary: false,
                shift: false,
            });
        });
        cx.run_until_parked();
        assert_eq!(renamed(&events), [("a".into(), "Nuevo nombre".into())]);
        assert!(!view.read_with(cx, |panel, _| panel.menu_open()));
        assert_eq!(
            listed_title(&view, "a", cx),
            "Nuevo nombre",
            "shown at once"
        );
        // The catalog catches up; a stale one does not undo it.
        view.update(cx, |panel, cx| {
            panel.set_catalog(catalog(vec![project("/work/crest", &["a", "b"])]), cx);
        });
        assert_eq!(listed_title(&view, "a", cx), "Nuevo nombre");
        let mut saved = project("/work/crest", &["a", "b"]);
        saved.chats[0].title = "Nuevo nombre".into();
        view.update(cx, |panel, cx| panel.set_catalog(catalog(vec![saved]), cx));
        view.read_with(cx, |panel, _| assert!(panel.pending_titles.is_empty()));
        // A failed save shows the catalog's title again.
        view.update(cx, |panel, cx| {
            panel.pending_titles.insert("b".into(), "Otro".into());
            panel.forget_title("b", cx);
        });
        assert_eq!(listed_title(&view, "b", cx), "b");
    }

    #[gpui::test]
    fn the_enter_key_saves_the_name_and_typed_letters_stay_text(cx: &mut TestAppContext) {
        let (view, events, cx) = renaming(cx);
        let input = field(&view, cx);
        type_name(&view, "", cx);
        // `r` and `j` are list keys; in the field they are text.
        cx.simulate_keystrokes("r j");
        cx.run_until_parked();
        assert_eq!(field(&view, cx), input, "the field was not reopened");
        assert_eq!(
            input.read_with(cx, |state, _| state.value().to_string()),
            "rj"
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(renamed(&events), [("a".into(), "rj".into())]);
        assert!(!view.read_with(cx, |panel, _| panel.menu_open()));
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, ChatsEvent::Open { .. })),
            "Enter did not reach the list"
        );
    }

    #[gpui::test]
    fn losing_the_focus_saves_a_changed_name(cx: &mut TestAppContext) {
        let (view, events, cx) = renaming(cx);
        // Unchanged, the field stays open.
        view.update_in(cx, |panel, window, cx| {
            window.focus(&panel.focus_handle, cx)
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(renamed(&events).is_empty());
        assert!(view.read_with(cx, |panel, _| panel.menu_open()));
        let input = field(&view, cx);
        input.update_in(cx, |state, window, cx| state.focus(window, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        type_name(&view, "Por foco", cx);
        view.update_in(cx, |panel, window, cx| {
            window.focus(&panel.focus_handle, cx)
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert_eq!(renamed(&events), [("a".into(), "Por foco".into())]);
        assert!(!view.read_with(cx, |panel, _| panel.menu_open()));
        assert_eq!(listed_title(&view, "a", cx), "Por foco");
    }

    #[gpui::test]
    fn esc_in_the_field_cancels(cx: &mut TestAppContext) {
        let (view, events, cx) = renaming(cx);
        type_name(&view, "No se guarda", cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!view.read_with(cx, |panel, _| panel.menu_open()));
        assert!(renamed(&events).is_empty());
        assert!(matches!(
            events.borrow().last(),
            Some(ChatsEvent::Dismissed)
        ));
        assert_eq!(listed_title(&view, "a", cx), "a");
    }

    #[gpui::test]
    fn the_rename_key_of_the_list_opens_a_working_field(cx: &mut TestAppContext) {
        let (view, cx) = panel(vec![project("/work/crest", &["a", "b"])], Some("a"), cx);
        cx.update(|window, _| window.activate_window());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = std::rc::Rc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ChatsEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        view.update_in(cx, |panel, window, cx| panel.focus(window, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_keystrokes("f2");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(cx.update(|window, cx| rename_has_focus(window, cx)));
        type_name(&view, "", cx);
        cx.simulate_input("Por tecla");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(renamed(&events), [("a".into(), "Por tecla".into())]);
    }

    #[gpui::test]
    fn the_search_filters_the_rows_and_escape_closes_it(cx: &mut TestAppContext) {
        let projects = vec![
            project("/work/crest", &["alpha", "beta"]),
            project("/other", &["gamma"]),
        ];
        let (view, cx) = panel(projects, Some("alpha"), cx);
        cx.update(|window, _| window.activate_window());
        let rows = |cx: &mut VisualTestContext| view.read_with(cx, |panel, _| panel.rows());
        assert_eq!(rows(cx).len(), 4, "crest unfolded, other folded");
        view.update_in(cx, |panel, window, cx| panel.open_search(window, cx));
        cx.run_until_parked();
        assert!(cx.update(|window, cx| field_has_focus(window, cx)));
        cx.simulate_input("gam");
        cx.run_until_parked();
        assert_eq!(
            rows(cx),
            [
                ChatRow::Project("/other".into()),
                ChatRow::Chat {
                    project: Some("/other".into()),
                    id: "gamma".into()
                }
            ],
            "a search unfolds what matches and hides the rest"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(rows(cx).len(), 4, "closing the search lists everything");
        assert!(!cx.update(|window, cx| field_has_focus(window, cx)));
    }

    #[gpui::test]
    fn the_strip_and_the_hidden_rows_reach_the_same_actions(cx: &mut TestAppContext) {
        let (view, cx) = panel(vec![project("/work/crest", &["a"])], Some("a"), cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = std::rc::Rc::clone(&events);
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ChatsEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        view.update(cx, |panel, cx| panel.set_width(SidebarWidth::Strip, cx));
        click("sidebar-initial-0", cx);
        click("sidebar-new", cx);
        // The mark of the strip opens the full sidebar, and the one of the
        // full sidebar hides it (not the strip).
        click("sidebar-mark", cx);
        click("sidebar-mark", cx);
        assert_eq!(
            *events.borrow(),
            [
                ChatsEvent::Sidebar(SidebarWidth::Strip),
                ChatsEvent::Open {
                    root: "/work/crest".into(),
                    id: "a".into()
                },
                ChatsEvent::NewStandalone,
                ChatsEvent::Sidebar(SidebarWidth::Full),
                ChatsEvent::Sidebar(SidebarWidth::Hidden),
            ]
        );
    }

    fn catalog(projects: Vec<ProjectChats>) -> Catalog {
        Catalog {
            projects,
            chats: Vec::new(),
        }
    }

    fn project(root: &str, chats: &[&str]) -> ProjectChats {
        ProjectChats {
            id: root.into(),
            name: project_name(root),
            root: root.into(),
            dirs: 1,
            chats: chats
                .iter()
                .map(|id| ChatEntry {
                    id: (*id).into(),
                    title: (*id).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn names_come_from_the_directory_and_the_title() {
        assert_eq!(project_name(r"C:\work\crest"), "crest");
        assert_eq!(project_name("crest"), "crest");
        assert_eq!(chat_label(None), "New chat");
        assert_eq!(chat_label(Some("  ")), "New chat");
        assert_eq!(chat_label(Some(" hola ")), "hola");
        assert_eq!(clean_title("  "), None);
        assert_eq!(clean_title("  hola  ").as_deref(), Some("hola"));
        assert_eq!(clean_title(&"a".repeat(200)).unwrap().chars().count(), 120);
        let chats = vec![
            ChatEntry {
                id: "new".into(),
                title: "New chat".into(),
            },
            ChatEntry {
                id: "old".into(),
                title: "old".into(),
            },
        ];
        assert_eq!(next_chat_id(&chats, "new"), Some("old"));
        assert_eq!(next_chat_id(&chats, "missing"), Some("new"));
        assert!(next_chat_id(&chats[..1], "new").is_none());
    }
}
