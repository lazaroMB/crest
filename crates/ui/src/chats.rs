//! The chats sidebar: a new chat, then each project already opened and,
//! under it, its chats, newest first.
//!
//! The binary fills the list ([`crate::workbench::ProjectOpener::catalog`]).
//! Choosing a row emits [`ChatsEvent`]; the workbench opens that session.
//! A right click renames or deletes the chat. The file tree stays the
//! other side panel.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, Global,
    InteractiveElement as _, IntoElement, KeystrokeEvent, MouseButton, MouseDownEvent,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Which session to open in a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOpen {
    /// The latest saved session, or a new one when the project has none.
    Latest,
    /// A new empty session. The previous one stays saved.
    New,
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
    /// Directory name shown on the row.
    pub name: String,
    /// Workspace root, as stored.
    pub root: String,
    /// Its chats, newest first.
    pub chats: Vec<ChatEntry>,
}

/// What the panel asks the window to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatsEvent {
    /// An empty session in the project that is open.
    New,
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

/// Width of the panel, beside the session.
const PANEL_WIDTH: f32 = 240.0;

/// The rename field, while it exists. The prompt leaves its keys alone
/// when this field has the focus.
struct ChatRename(WeakEntity<InputState>);

impl Global for ChatRename {}

/// Whether the rename field is the focused text input.
pub fn rename_has_focus(window: &Window, cx: &App) -> bool {
    cx.try_global::<ChatRename>()
        .and_then(|active| active.0.upgrade())
        .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
}

/// The sidebar of projects and chats.
pub struct ChatsPanel {
    /// The catalog as given; `projects` adds the open chat to it.
    catalog: Vec<ProjectChats>,
    projects: Vec<ProjectChats>,
    expanded: HashSet<String>,
    current_root: String,
    current_id: Option<String>,
    /// Chats with a turn running, by id: whether they wait on the user.
    activity: HashMap<String, bool>,
    action: Option<ChatAction>,
    rename_input: Option<Entity<InputState>>,
    _keys: Subscription,
}

impl ChatsPanel {
    /// A panel for the session that is open. The catalog arrives with
    /// [`Self::set_projects`].
    pub fn new(current_root: String, current_id: Option<String>, cx: &mut Context<Self>) -> Self {
        let projects = catalog_rows(Vec::new(), &current_root, current_id.as_deref());
        let mut expanded = HashSet::new();
        if let Some(project) = projects.first() {
            expanded.insert(project.root.clone());
        }
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.on_key(event, window, cx))
                .ok();
        });
        Self {
            catalog: Vec::new(),
            projects,
            expanded,
            current_root,
            current_id,
            activity: HashMap::new(),
            action: None,
            rename_input: None,
            _keys: keys,
        }
    }

    /// Marks the chat `id` of `root` as the open one.
    pub fn set_current(&mut self, root: String, id: Option<String>, cx: &mut Context<Self>) {
        self.current_root = root;
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

    /// Another chat of `root` to open after deleting `id`, if one remains.
    #[must_use]
    pub fn next_chat(&self, root: &str, id: &str) -> Option<String> {
        let project = self
            .projects
            .iter()
            .find(|project| same_workspace(&project.root, root))?;
        next_chat_id(&project.chats, id).map(str::to_owned)
    }

    /// Replaces the list. The open project stays expanded, and the open
    /// chat stays visible even before it has been saved.
    pub fn set_projects(&mut self, projects: Vec<ProjectChats>, cx: &mut Context<Self>) {
        self.catalog = projects;
        self.rebuild();
        cx.notify();
    }

    fn rebuild(&mut self) {
        let projects = catalog_rows(
            self.catalog.clone(),
            &self.current_root,
            self.current_id.as_deref(),
        );
        if let Some(project) = projects
            .iter()
            .find(|project| same_workspace(&project.root, &self.current_root))
        {
            self.expanded.insert(project.root.clone());
        }
        self.projects = projects;
    }

    fn toggle(&mut self, root: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(root) {
            self.expanded.insert(root.to_owned());
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.action.is_some() {
            self.clear_action(cx);
            return;
        }
        let Some(input) = &self.rename_input else {
            return;
        };
        if !input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        if event.keystroke.key == "enter" {
            let title = input.read(cx).value().to_string();
            self.commit_rename(title, cx);
            cx.stop_propagation();
        }
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
        self.rename_input = None;
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
        self.rename_input = Some(input);
        if let Some(action) = &mut self.action {
            action.kind = ChatActionKind::Rename;
        }
        cx.notify();
    }

    fn ask_delete(&mut self, cx: &mut Context<Self>) {
        self.rename_input = None;
        if let Some(action) = &mut self.action {
            action.kind = ChatActionKind::ConfirmDelete;
        }
        cx.notify();
    }

    fn commit_rename(&mut self, raw: String, cx: &mut Context<Self>) {
        let Some(action) = self.action.clone() else {
            return;
        };
        let Some(title) = clean_title(&raw) else {
            self.clear_action(cx);
            return;
        };
        self.clear_action(cx);
        if title == action.title {
            return;
        }
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

    fn clear_action(&mut self, cx: &mut Context<Self>) {
        self.action = None;
        self.rename_input = None;
        cx.notify();
    }

    fn projects(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.projects
            .iter()
            .enumerate()
            .map(|(index, project)| self.project_block(index, project, cx))
            .collect()
    }

    fn project_block(
        &self,
        index: usize,
        project: &ProjectChats,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, active) = (theme.muted_foreground, theme.list_active);
        let expanded = self.expanded.contains(&project.root);
        let marker = if expanded { "▾" } else { "▸" };
        let root = project.root.clone();
        let mut block = v_flex().child(
            h_flex()
                .id(SharedString::from(format!("chats-project-{index}")))
                .w_full()
                .gap_1()
                .px_2()
                .py_1()
                .cursor_pointer()
                .hover(move |style| style.bg(active).rounded_sm())
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(&root, cx)))
                .child(div().w(px(12.)).text_color(muted).child(marker))
                .child(label(&project.name)),
        );
        if expanded {
            for (row, chat) in project.chats.iter().enumerate() {
                let selected = self.current_id.as_deref() == Some(chat.id.as_str());
                // A turn running out of sight: waiting on the user, or busy.
                let badge = match self.activity.get(&chat.id) {
                    Some(true) => Some("⏳"),
                    Some(false) => Some("●"),
                    None => None,
                };
                let (root, id, title) = (project.root.clone(), chat.id.clone(), chat.title.clone());
                let (menu_root, menu_id, menu_title) = (root.clone(), id.clone(), title.clone());
                let menu_for_chat = self.action.as_ref().is_some_and(|action| action.id == id);
                let mut item = h_flex()
                    .id(SharedString::from(format!("chats-session-{index}-{row}")))
                    .w_full()
                    .pl(px(22.))
                    .pr_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(move |style| style.bg(active).rounded_sm())
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_menu(
                                menu_id.clone(),
                                menu_root.clone(),
                                menu_title.clone(),
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.clear_action(cx);
                        if this.current_id.as_deref() == Some(id.as_str()) {
                            return;
                        }
                        cx.emit(ChatsEvent::Open {
                            root: root.clone(),
                            id: id.clone(),
                        });
                    }))
                    .child(label(&chat.title))
                    .children(badge.map(|badge| {
                        div()
                            .debug_selector(|| "chats-activity".to_owned())
                            .text_xs()
                            .text_color(muted)
                            .child(badge)
                    }));
                if selected {
                    item = item.bg(active).rounded_sm();
                }
                block = block.child(item);
                if menu_for_chat {
                    block = block.child(self.menu(index, row, cx));
                }
            }
        }
        block.into_any_element()
    }

    fn menu(&self, index: usize, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(action) = &self.action else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (popover, danger, border, muted, active) = (
            theme.popover,
            theme.danger,
            theme.border,
            theme.muted_foreground,
            theme.list_active,
        );
        let mut menu = v_flex()
            .mx_2()
            .mb_1()
            .py_1()
            .border_1()
            .border_color(border)
            .rounded_md()
            .bg(popover)
            .text_sm();
        match action.kind {
            ChatActionKind::Menu => {
                menu = menu
                    .child(menu_row(
                        format!("chats-rename-{index}-{row}"),
                        "Rename",
                        false,
                        active,
                        danger,
                        cx.listener(|this, _, window, cx| this.begin_rename(window, cx)),
                    ))
                    .child(menu_row(
                        format!("chats-delete-{index}-{row}"),
                        "Delete",
                        true,
                        active,
                        danger,
                        cx.listener(|this, _, _, cx| this.ask_delete(cx)),
                    ));
            }
            ChatActionKind::Rename => {
                if let Some(input) = &self.rename_input {
                    menu = menu
                        .child(div().px_2().py_1().child(Input::new(input)))
                        .child(
                            div()
                                .px_2()
                                .text_xs()
                                .text_color(muted)
                                .child("Enter saves · Esc cancels"),
                        );
                }
            }
            ChatActionKind::ConfirmDelete => {
                menu = menu
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_color(danger)
                            .child("Delete this chat?"),
                    )
                    .child(menu_row(
                        format!("chats-confirm-delete-{index}-{row}"),
                        "Delete",
                        true,
                        active,
                        danger,
                        cx.listener(|this, _, _, cx| this.commit_delete(cx)),
                    ))
                    .child(menu_row(
                        format!("chats-cancel-delete-{index}-{row}"),
                        "Cancel",
                        false,
                        active,
                        danger,
                        cx.listener(|this, _, _, cx| this.clear_action(cx)),
                    ));
            }
        }
        menu.into_any_element()
    }
}

fn menu_row(
    id: String,
    text: &'static str,
    danger: bool,
    active: gpui::Hsla,
    danger_color: gpui::Hsla,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let mut row = h_flex()
        .id(SharedString::from(id))
        .w_full()
        .px_2()
        .py_1()
        .cursor_pointer()
        .hover(move |style| style.bg(active).rounded_sm())
        .on_click(click)
        .child(text);
    if danger {
        row = row.text_color(danger_color);
    }
    row.into_any_element()
}

impl EventEmitter<ChatsEvent> for ChatsPanel {}

impl Render for ChatsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (sidebar, foreground, muted, border, active) = (
            theme.sidebar,
            theme.sidebar_foreground,
            theme.muted_foreground,
            theme.border,
            theme.list_active,
        );
        v_flex()
            .id("chats-panel")
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(border)
            .bg(sidebar)
            .text_color(foreground)
            .child(div().px_2().py_1().text_color(muted).child("Chats"))
            .child(
                h_flex()
                    .id("chats-new")
                    .w_full()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(move |style| style.bg(active).rounded_sm())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatsEvent::New)))
                    .child(label("New chat")),
            )
            .child(
                div()
                    .id("chats-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(self.projects(cx)),
            )
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

/// `projects` plus the open project and the open chat, when the catalog
/// does not list them yet.
#[must_use]
pub fn catalog_rows(
    mut projects: Vec<ProjectChats>,
    current_root: &str,
    current_id: Option<&str>,
) -> Vec<ProjectChats> {
    if current_root.is_empty() {
        return projects;
    }
    if !projects
        .iter()
        .any(|project| same_workspace(&project.root, current_root))
    {
        projects.insert(
            0,
            ProjectChats {
                name: project_name(current_root),
                root: current_root.to_owned(),
                chats: Vec::new(),
            },
        );
    }
    if let Some(id) = current_id.filter(|id| !id.is_empty())
        && let Some(project) = projects
            .iter_mut()
            .find(|project| same_workspace(&project.root, current_root))
        && !project.chats.iter().any(|chat| chat.id == id)
    {
        project.chats.insert(
            0,
            ChatEntry {
                id: id.to_owned(),
                title: "New chat".into(),
            },
        );
    }
    projects
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
mod tests {
    use super::*;

    fn project(root: &str, chats: &[&str]) -> ProjectChats {
        ProjectChats {
            name: project_name(root),
            root: root.into(),
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

    #[test]
    fn the_open_project_and_chat_stay_on_the_list() {
        let rows = catalog_rows(Vec::new(), r"C:\work\crest", Some("abc"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "crest");
        assert_eq!(rows[0].chats[0].id, "abc");
        assert_eq!(rows[0].chats[0].title, "New chat");

        let listed = catalog_rows(
            vec![
                project("C:/work/crest", &["abc"]),
                project("/other", &["z"]),
            ],
            r"C:\work\crest",
            Some("abc"),
        );
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].chats.len(), 1, "the saved chat is not repeated");

        let added = catalog_rows(vec![project("/other", &["z"])], "/work/crest", Some("new"));
        assert_eq!(added[0].root, "/work/crest");
        assert_eq!(added[0].chats[0].id, "new");
        assert!(catalog_rows(vec![project("/other", &[])], "", None).len() == 1);
    }
}
