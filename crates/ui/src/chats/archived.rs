//! The "Archived (N)" section at the bottom of the full list: folded until
//! opened, and the only place an archived chat or project shows. Each entry
//! has Restore and Delete; Delete asks first, in the same floating menu as
//! the rest.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use super::glyph::{Glyph, icon_button};
use super::menu::{ChatOp, Page, ProjectOp};
use super::{ChatRow, ChatsEvent, ChatsPanel, label};

/// One entry of the section.
struct Entry {
    /// Its place in the section, for selectors.
    ix: usize,
    project: bool,
    id: String,
    title: String,
    /// What Restore asks for.
    restore: ChatsEvent,
}

impl Entry {
    fn project(ix: usize, id: &str, title: String) -> Self {
        let op = ProjectOp::Archive(false);
        let restore = ChatsEvent::Project {
            id: id.to_owned(),
            op,
        };
        Self {
            ix,
            project: true,
            id: id.to_owned(),
            title,
            restore,
        }
    }

    fn chat(ix: usize, id: &str, title: &str) -> Self {
        let restore = ChatsEvent::Chat {
            id: id.to_owned(),
            op: ChatOp::Archive(false),
        };
        Self {
            ix,
            project: false,
            id: id.to_owned(),
            title: title.to_owned(),
            restore,
        }
    }

    /// The keyboard row of this entry.
    fn row(&self) -> ChatRow {
        if self.project {
            ChatRow::ArchivedProject(self.id.clone())
        } else {
            ChatRow::ArchivedChat(self.id.clone())
        }
    }
}

impl ChatsPanel {
    /// Whether the Archived section is unfolded.
    #[must_use]
    pub fn archived_is_open(&self) -> bool {
        self.archived_open
    }

    /// The entries of the section, projects first.
    fn archived_entries(&self) -> Vec<Entry> {
        let archive = &self.catalog.archived;
        let projects = archive.projects.iter().map(|project| {
            let title = format!("{} ({})", project.name, project.chats);
            (true, project.id.as_str(), title)
        });
        let chats = archive
            .chats
            .iter()
            .map(|chat| (false, chat.id.as_str(), chat.title.clone()));
        projects
            .chain(chats)
            .enumerate()
            .map(|(ix, (project, id, title))| {
                if project {
                    Entry::project(ix, id, title)
                } else {
                    Entry::chat(ix, id, &title)
                }
            })
            .collect()
    }

    /// The keyboard rows of the section: its heading, and under it, when
    /// unfolded, each archived project and chat.
    pub(super) fn archived_rows(&self) -> Vec<ChatRow> {
        if self.catalog.archived.count() == 0 {
            return Vec::new();
        }
        let entries = self.archived_entries().into_iter().map(|entry| entry.row());
        let shown = self.archived_open.then_some(entries);
        std::iter::once(ChatRow::Archived)
            .chain(shown.into_iter().flatten())
            .collect()
    }

    /// Folds or unfolds the section.
    pub(super) fn toggle_archived(&mut self, cx: &mut Context<Self>) {
        self.archived_open = !self.archived_open;
        self.clear_action(cx);
    }

    /// What `key` does on a row of the section: `Enter` (or the arrows) fold
    /// it, `Enter` on an entry restores it, `Delete` asks to delete it.
    pub(super) fn archived_key(
        &mut self,
        key: &str,
        row: &ChatRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .archived_entries()
            .into_iter()
            .find(|entry| &entry.row() == row);
        match (key, entry) {
            ("enter", None) => self.toggle_archived(cx),
            ("right", None) if !self.archived_open => self.toggle_archived(cx),
            ("left", None) if self.archived_open => self.toggle_archived(cx),
            ("left", Some(_)) => {
                self.selected = Some(ChatRow::Archived);
                self.reveal_selected();
                cx.notify();
            }
            ("enter", Some(entry)) => cx.emit(entry.restore),
            ("delete", Some(entry)) => self.ask_delete_entry(&entry, window, cx),
            _ => {}
        }
    }

    /// Opens the delete question of an archived entry.
    fn ask_delete_entry(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        let subject = self.catalog.archived_subject(entry.project, &entry.id);
        if let Some(subject) = subject {
            self.open_menu(subject, Page::Confirm, window, cx);
        }
    }

    /// The heading and, unfolded, the entries of the section; nothing when
    /// nothing is archived.
    pub(super) fn archived_elements(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let count = self.catalog.archived.count();
        if count == 0 {
            return Vec::new();
        }
        let keyed = |row: &ChatRow| focused && self.selected.as_ref() == Some(row);
        let mut out = vec![heading(
            count,
            self.archived_open,
            keyed(&ChatRow::Archived),
            cx,
        )];
        if self.archived_open {
            for entry in self.archived_entries() {
                let keyed = keyed(&entry.row());
                out.push(self.archived_entry(entry, keyed, cx));
            }
        }
        out
    }

    /// One entry: its title, Restore and Delete.
    fn archived_entry(&self, entry: Entry, keyed: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, active, ring) = (theme.muted_foreground, theme.list_active, theme.ring);
        let ix = entry.ix;
        let selector = format!("chats-archived-{ix}");
        let (restore, popup) = (
            entry.restore.clone(),
            self.popup(entry.project, &entry.id, &format!("a{ix}"), cx),
        );
        let mut row = h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .w_full()
            .gap_1()
            .pl(px(22.))
            .pr_2()
            .py_1()
            .border_1()
            .border_color(gpui::transparent_black())
            .text_color(muted)
            .hover(move |style| style.bg(active).rounded_sm())
            .child(label(&entry.title))
            .child(icon_button(
                format!("chats-restore-{ix}"),
                Glyph::Restore,
                cx.listener(move |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(restore.clone());
                }),
                cx,
            ))
            .child(icon_button(
                format!("chats-archived-delete-{ix}"),
                Glyph::Trash,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.ask_delete_entry(&entry, window, cx);
                }),
                cx,
            ));
        if keyed {
            row = row.border_color(ring).rounded_sm();
        }
        v_flex().child(row).children(popup).into_any_element()
    }
}

/// The heading of the section: fold marker, icon and count.
fn heading(count: usize, open: bool, keyed: bool, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let theme = cx.theme();
    let (muted, active, ring) = (theme.muted_foreground, theme.list_active, theme.ring);
    let mut heading = h_flex()
        .id("chats-archived")
        .debug_selector(|| "chats-archived".to_owned())
        .w_full()
        .gap_1()
        .mt_2()
        .px_2()
        .py_1()
        .border_1()
        .border_color(gpui::transparent_black())
        .text_color(muted)
        .cursor_pointer()
        .hover(move |style| style.bg(active).rounded_sm())
        .on_click(cx.listener(|this, _, _, cx| this.toggle_archived(cx)))
        .child(div().w(px(12.)).child(if open { "▾" } else { "▸" }))
        .child(Glyph::Archive.icon().small())
        .child(label(&format!("Archived ({count})")));
    if keyed {
        heading = heading.border_color(ring).rounded_sm();
    }
    heading.into_any_element()
}
