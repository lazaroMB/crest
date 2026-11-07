//! The menus of chats and projects: what a right click or the `...` button
//! of a row offers, in the order and wording of the mock.
//!
//! This file is the menu's model and its keyboard; [`super::menu_view`]
//! draws it. A menu has a [`Subject`] (the chat or project it is about) and
//! a [`Page`] (the main list, "Move to project", the directories, the
//! delete question, or the name field of a rename). Every row is an
//! [`Item`]: it goes to another page, closes the menu, or asks the window
//! for a change ([`ChatsEvent`]). The rows are data, so a test can read
//! them without drawing anything.

use gpui::{Context, Window};

use super::glyph::Glyph;
use super::subject::Subject;
use super::{ChatRow, ChatsEvent, ChatsPanel};

/// A change to a chat that the window runs in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatOp {
    /// "Pin to top" (`true`) or "Unpin".
    Pin(bool),
    /// "Move to project" with a project id, "Remove from project" with
    /// none.
    Move(Option<String>),
    /// "New project...": a project named after the directory, with the
    /// chat in it.
    Promote(String),
    /// "Archive" (`true`) or restore (`false`).
    Archive(bool),
    /// "Branch chat".
    Branch,
}

/// A change to a project that the window runs in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectOp {
    /// "Rename".
    Rename(String),
    /// "Pin to top" (`true`) or "Unpin".
    Pin(bool),
    /// "Add directory...".
    AddDirectory(String),
    /// "Remove directory": one of its directories.
    RemoveDirectory(String),
    /// "Archive" (`true`) or restore (`false`).
    Archive(bool),
    /// "Delete": its chats become standalone chats.
    Delete,
}

/// What a menu row asks the window to choose a directory for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickFor {
    /// "New project...": the chat `chat` moves into a project of it.
    NewProject {
        /// Session id.
        chat: String,
    },
    /// "Add directory...": to the project `project`.
    AddDirectory {
        /// Project id.
        project: String,
    },
}

/// The page a menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    /// The list of rows.
    Main,
    /// The projects to move the chat to.
    Move,
    /// The directories of the project, to remove one.
    Directories,
    /// "Delete this chat?"
    Confirm,
    /// The name field, in the row itself; no popup.
    Rename,
}

/// How a row looks and whether the keyboard can stop on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    Plain,
    Danger,
    /// A question in the colour of danger; not a row.
    Warning,
    /// A small title over the rows below it.
    Heading,
    /// A line of text that is not a row (a directory, an explanation).
    Note,
    /// A row that does nothing now; the label says why.
    Disabled,
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Then {
    /// Another page of the same menu.
    Go(Page),
    /// Close the menu and tell the window.
    Emit(ChatsEvent),
    /// Close the menu.
    Close,
    /// Nothing: a heading or a note.
    Nothing,
}

/// One row of a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    /// Names the row in its selector (`chats-<key>-<place>`).
    pub key: &'static str,
    pub label: String,
    pub look: Look,
    pub mark: Option<Glyph>,
    pub then: Then,
}

impl Item {
    fn new(key: &'static str, label: impl Into<String>, then: Then) -> Self {
        Self {
            key,
            label: label.into(),
            look: Look::Plain,
            mark: None,
            then,
        }
    }

    fn with(mut self, look: Look, mark: Option<Glyph>) -> Self {
        self.look = look;
        self.mark = mark;
        self
    }

    fn text(key: &'static str, look: Look, label: impl Into<String>) -> Self {
        Self::new(key, label, Then::Nothing).with(look, None)
    }

    fn go(key: &'static str, label: &str, page: Page) -> Self {
        Self::new(key, label, Then::Go(page))
    }

    fn emit(key: &'static str, label: &str, event: ChatsEvent) -> Self {
        Self::new(key, label, Then::Emit(event))
    }

    fn cancel(key: &'static str) -> Self {
        Self::new(key, "Cancel", Then::Close)
    }

    /// Whether the keyboard can stop on this row.
    pub(super) fn selectable(&self) -> bool {
        matches!(self.look, Look::Plain | Look::Danger)
    }

    /// Whether choosing the row opens another page (it shows a chevron).
    pub(super) fn opens_page(&self) -> bool {
        matches!(self.then, Then::Go(page) if page != Page::Confirm && page != Page::Rename)
    }
}

/// An open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Menu {
    pub subject: Subject,
    pub page: Page,
    /// The row the keyboard is on.
    pub cursor: usize,
}

impl Menu {
    pub(super) fn new(subject: Subject, page: Page, others: &[(String, String)]) -> Self {
        let mut menu = Self {
            subject,
            page,
            cursor: 0,
        };
        menu.cursor = first_selectable(&menu.items(others));
        menu
    }

    /// The rows of the page. `others` are the projects a chat can move to
    /// (id, name): the listed ones, never an archived one.
    pub(super) fn items(&self, others: &[(String, String)]) -> Vec<Item> {
        match (&self.subject, self.page) {
            (Subject::Chat { .. }, Page::Main) => chat_main(&self.subject),
            (Subject::Chat { id, project, .. }, Page::Move) => {
                chat_move(id, project.as_deref(), others)
            }
            (Subject::Project { .. }, Page::Main) => project_main(&self.subject),
            (Subject::Project { roots, .. }, Page::Directories) => {
                project_directories(self.subject.id(), roots)
            }
            (subject, Page::Confirm) => confirm(subject),
            _ => Vec::new(),
        }
    }
}

fn first_selectable(items: &[Item]) -> usize {
    items.iter().position(Item::selectable).unwrap_or(0)
}

fn chat_main(subject: &Subject) -> Vec<Item> {
    let Subject::Chat {
        id,
        root,
        project,
        pinned,
        ..
    } = subject
    else {
        return Vec::new();
    };
    let new_chat = match project {
        Some(project) => ChatsEvent::NewIn {
            project: project.clone(),
            root: root.clone(),
        },
        None => ChatsEvent::NewStandalone,
    };
    let op = |op| ChatsEvent::Chat { id: id.clone(), op };
    let mover = if project.is_some() {
        "Move to project…"
    } else {
        "Add to project…"
    };
    let mut items = vec![
        Item::emit("new", "New chat", new_chat).with(Look::Plain, Some(Glyph::Plus)),
        Item::emit("branch", "Branch chat", op(ChatOp::Branch))
            .with(Look::Plain, Some(Glyph::Branch)),
        Item::go("rename", "Rename", Page::Rename),
        Item::emit(
            "pin",
            if *pinned { "Unpin" } else { "Pin to top" },
            op(ChatOp::Pin(!pinned)),
        ),
        Item::go("move", mover, Page::Move),
    ];
    if project.is_some() {
        items.push(Item::emit(
            "remove",
            "Remove from project",
            op(ChatOp::Move(None)),
        ));
    }
    items.extend([
        Item::emit("archive", "Archive", op(ChatOp::Archive(true))),
        Item::go("delete", "Delete", Page::Confirm).with(Look::Danger, None),
        Item::cancel("cancel"),
    ]);
    items
}

fn chat_move(id: &str, project: Option<&str>, others: &[(String, String)]) -> Vec<Item> {
    let op = |op| ChatsEvent::Chat {
        id: id.to_owned(),
        op,
    };
    let title = if project.is_some() {
        "Move to project"
    } else {
        "Add to project"
    };
    let mut items = vec![Item::text("move-title", Look::Heading, title)];
    items.extend(
        others
            .iter()
            .filter(|(other, _)| Some(other.as_str()) != project)
            .map(|(other, name)| {
                Item::emit("project", name, op(ChatOp::Move(Some(other.clone()))))
                    .with(Look::Plain, Some(Glyph::Folder))
            }),
    );
    if project.is_some() {
        items.push(Item::emit("none", "No project", op(ChatOp::Move(None))));
    }
    items.push(
        Item::emit(
            "new-project",
            "New project…",
            ChatsEvent::Pick(PickFor::NewProject {
                chat: id.to_owned(),
            }),
        )
        .with(Look::Plain, Some(Glyph::Plus)),
    );
    items.push(Item::cancel("cancel"));
    items
}

fn project_main(subject: &Subject) -> Vec<Item> {
    let Subject::Project {
        id, roots, pinned, ..
    } = subject
    else {
        return Vec::new();
    };
    let op = |op| ChatsEvent::Project { id: id.clone(), op };
    let mut items = vec![
        Item::emit(
            "new",
            "New chat here",
            ChatsEvent::NewIn {
                project: id.clone(),
                root: roots.first().cloned().unwrap_or_default(),
            },
        )
        .with(Look::Plain, Some(Glyph::Plus)),
        Item::text("dirs-title", Look::Heading, "Directories"),
    ];
    items.extend(
        roots
            .iter()
            .map(|root| Item::text("dir", Look::Note, root.clone())),
    );
    items.push(Item::emit(
        "add-dir",
        "Add directory…",
        ChatsEvent::Pick(PickFor::AddDirectory {
            project: id.clone(),
        }),
    ));
    items.push(if roots.len() > 1 {
        Item::go("remove-dir", "Remove directory…", Page::Directories)
    } else {
        Item::text(
            "remove-dir",
            Look::Disabled,
            "Remove directory… (a project keeps one)",
        )
    });
    items.extend([
        Item::go("rename", "Rename", Page::Rename),
        Item::emit(
            "pin",
            if *pinned { "Unpin" } else { "Pin to top" },
            op(ProjectOp::Pin(!pinned)),
        ),
        Item::emit("archive", "Archive", op(ProjectOp::Archive(true))),
        Item::go("delete", "Delete", Page::Confirm).with(Look::Danger, None),
        Item::cancel("cancel"),
    ]);
    items
}

fn project_directories(project: &str, roots: &[String]) -> Vec<Item> {
    let mut items = vec![Item::text("dirs-title", Look::Heading, "Remove directory")];
    items.extend(roots.iter().map(|root| {
        let op = ProjectOp::RemoveDirectory(root.clone());
        let event = ChatsEvent::Project {
            id: project.to_owned(),
            op,
        };
        Item::new("dir-remove", root.clone(), Then::Emit(event))
    }));
    items.push(Item::cancel("cancel"));
    items
}

fn confirm(subject: &Subject) -> Vec<Item> {
    let (question, note, event) = match subject {
        Subject::Chat { id, root, .. } => (
            "Delete this chat?".to_owned(),
            Some("Its history is removed for good.".to_owned()),
            ChatsEvent::Delete {
                root: root.clone(),
                id: id.clone(),
            },
        ),
        Subject::Project {
            id, name, chats, ..
        } => (
            format!("Delete project \"{name}\"?"),
            Some(match chats {
                0 => "It has no chats.".to_owned(),
                1 => "Its 1 chat becomes a standalone chat.".to_owned(),
                n => format!("Its {n} chats become standalone chats."),
            }),
            ChatsEvent::Project {
                id: id.clone(),
                op: ProjectOp::Delete,
            },
        ),
    };
    let mut items = vec![Item::text("question", Look::Warning, question)];
    items.extend(note.map(|note| Item::text("note", Look::Note, note)));
    items.push(Item::new("confirm-delete", "Delete", Then::Emit(event)).with(Look::Danger, None));
    items.push(Item::cancel("cancel-delete"));
    items
}

impl ChatsPanel {
    /// The rows of the open menu's page.
    pub(super) fn menu_items(&self) -> Vec<Item> {
        self.action
            .as_ref()
            .map(|menu| menu.items(&self.listed.move_targets()))
            .unwrap_or_default()
    }

    /// The subject of a chat or project row of the list.
    pub(super) fn subject_of(&self, row: &ChatRow) -> Option<Subject> {
        match row {
            ChatRow::Chat { id, .. } => self.listed.chat_subject(id),
            ChatRow::Project(id) => self.listed.project_subject(id),
            _ => None,
        }
    }

    /// Opens the menu of `subject` on `page`; the list takes the focus so
    /// the keys reach it. Opening the menu that is already open closes it.
    pub(super) fn open_menu(
        &mut self,
        subject: Subject,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .action
            .as_ref()
            .is_some_and(|menu| menu.subject == subject && menu.page == Page::Main);
        if same && page == Page::Main {
            self.clear_action(cx);
            return;
        }
        self.drop_rename();
        self.action = Some(Menu::new(subject, page, &self.listed.move_targets()));
        match page {
            Page::Rename => self.begin_rename(window, cx),
            _ => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    /// Moves the keyboard on the rows by `step`, over the ones it can stop
    /// on, wrapping.
    fn move_cursor(&mut self, step: isize, cx: &mut Context<Self>) {
        let items = self.menu_items();
        let stops: Vec<usize> = (0..items.len())
            .filter(|&ix| items[ix].selectable())
            .collect();
        let Some(menu) = self.action.as_mut() else {
            return;
        };
        let Some(at) = stops.iter().position(|&ix| ix >= menu.cursor) else {
            return;
        };
        let len = isize::try_from(stops.len()).unwrap_or(1);
        let next = (isize::try_from(at).unwrap_or(0) + step).rem_euclid(len);
        menu.cursor = stops[usize::try_from(next).unwrap_or(0)];
        cx.notify();
    }

    /// Does what row `ix` of the open menu says.
    pub(super) fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.menu_items();
        let Some(item) = items.get(ix).filter(|item| item.selectable()).cloned() else {
            return;
        };
        match item.then {
            Then::Go(Page::Rename) => self.begin_rename(window, cx),
            Then::Go(page) => self.go_page(page, cx),
            Then::Emit(event) => {
                self.clear_action(cx);
                cx.emit(event);
            }
            Then::Close => self.cancel_action(cx),
            Then::Nothing => {}
        }
    }

    /// A key while a menu is open (and its name field is not).
    /// Returns whether the key was the menu's.
    pub(super) fn menu_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.action.as_ref() else {
            return false;
        };
        let (page, cursor) = (menu.page, menu.cursor);
        if page == Page::Rename {
            return false;
        }
        match key {
            "up" | "k" => self.move_cursor(-1, cx),
            "down" | "j" => self.move_cursor(1, cx),
            "enter" | "space" => self.activate(cursor, window, cx),
            "right" if self.menu_items().get(cursor).is_some_and(Item::opens_page) => {
                self.activate(cursor, window, cx);
            }
            "y" if page == Page::Confirm => self.activate(cursor, window, cx),
            "left" | "backspace" if page != Page::Main => self.go_page(Page::Main, cx),
            "left" | "backspace" => self.cancel_action(cx),
            _ => {}
        }
        true
    }

    /// Shows `page` of the open menu, the keyboard on its first row.
    fn go_page(&mut self, page: Page, cx: &mut Context<Self>) {
        let targets = self.listed.move_targets();
        if let Some(menu) = &mut self.action {
            menu.page = page;
            menu.cursor = first_selectable(&menu.items(&targets));
        }
        cx.notify();
    }

    /// `m` on the selected row: its menu.
    pub(super) fn open_menu_on_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let subject = self.selected.as_ref().and_then(|row| self.subject_of(row));
        if let Some(subject) = subject {
            self.open_menu(subject, Page::Main, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(project: Option<&str>, pinned: bool) -> Subject {
        Subject::Chat {
            id: "c1".into(),
            root: project.map(|_| "/w/api".to_owned()).unwrap_or_default(),
            title: "Fix".into(),
            project: project.map(str::to_owned),
            pinned,
        }
    }

    fn project(roots: &[&str], pinned: bool, chats: usize) -> Subject {
        Subject::Project {
            id: "p1".into(),
            name: "api".into(),
            roots: roots.iter().map(|root| (*root).to_owned()).collect(),
            pinned,
            chats,
        }
    }

    /// The labels of a page, `>` after the ones that open another page,
    /// `!` before dangerous ones, `-` before the ones the keyboard skips.
    fn shown(subject: Subject, page: Page) -> Vec<String> {
        let others = [
            ("p2".to_owned(), "web".to_owned()),
            ("p1".to_owned(), "api".to_owned()),
        ];
        Menu::new(subject, page, &others)
            .items(&others)
            .into_iter()
            .map(|item| {
                let lead = match item.look {
                    Look::Danger => "!",
                    Look::Plain => "",
                    Look::Warning => "~",
                    _ => "-",
                };
                format!(
                    "{lead}{}{}",
                    item.label,
                    if item.opens_page() { " >" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn each_page_lists_the_rows_of_the_mock_in_its_order() {
        let cases = [
            (
                chat(Some("p1"), false),
                Page::Main,
                "New chat|Branch chat|Rename|Pin to top|Move to project… >|Remove from project|Archive|!Delete|Cancel",
            ),
            (
                chat(None, true),
                Page::Main,
                "New chat|Branch chat|Rename|Unpin|Add to project… >|Archive|!Delete|Cancel",
            ),
            (
                chat(Some("p1"), false),
                Page::Move,
                "-Move to project|web|No project|New project…|Cancel",
            ),
            (
                chat(None, false),
                Page::Move,
                "-Add to project|web|api|New project…|Cancel",
            ),
            (
                project(&["/w/api", "/w/lib"], false, 3),
                Page::Main,
                "New chat here|-Directories|-/w/api|-/w/lib|Add directory…|Remove directory… >|Rename|Pin to top|Archive|!Delete|Cancel",
            ),
            (
                project(&["/w/api"], true, 0),
                Page::Main,
                "New chat here|-Directories|-/w/api|Add directory…|-Remove directory… (a project keeps one)|Rename|Unpin|Archive|!Delete|Cancel",
            ),
            (
                project(&["/w/api", "/w/lib"], false, 3),
                Page::Directories,
                "-Remove directory|/w/api|/w/lib|Cancel",
            ),
            (
                project(&["/w/api"], false, 2),
                Page::Confirm,
                "~Delete project \"api\"?|-Its 2 chats become standalone chats.|!Delete|Cancel",
            ),
            (
                project(&["/w/api"], false, 1),
                Page::Confirm,
                "~Delete project \"api\"?|-Its 1 chat becomes a standalone chat.|!Delete|Cancel",
            ),
            (
                chat(None, false),
                Page::Confirm,
                "~Delete this chat?|-Its history is removed for good.|!Delete|Cancel",
            ),
        ];
        for (subject, page, want) in cases {
            let rows = shown(subject.clone(), page).join("|");
            assert_eq!(rows, want, "{subject:?} {page:?}");
        }
    }

    #[test]
    fn the_keyboard_starts_on_the_first_row_it_can_stop_on() {
        let others = [("p2".to_owned(), "web".to_owned())];
        let cases = [
            (chat(None, false), Page::Main, 0),
            (chat(None, false), Page::Move, 1),
            (project(&["/w/a"], false, 0), Page::Confirm, 2),
        ];
        for (subject, page, cursor) in cases {
            let menu = Menu::new(subject.clone(), page, &others);
            assert_eq!(menu.cursor, cursor, "{subject:?} {page:?}");
            assert!(menu.items(&others)[cursor].selectable());
        }
    }
}
