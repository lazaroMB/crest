//! The rows of the full list: project headers, chats and the section
//! headings. Menus are drawn by [`super::menu_view`], the Archived section
//! by [`super::archived`].

use gpui::{
    AnyElement, App, Context, Div, Entity, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use super::glyph::{Glyph, icon_button};

use super::{ChatEntry, ChatRow, ChatsEvent, ChatsPanel, ProjectChats};

impl ChatsPanel {
    /// One element per row of [`Self::rows`]: a project header (with its
    /// "No chats yet" line), a chat or an Archived row, so the scroll can
    /// keep the selected row in view.
    pub(super) fn row_elements(&self, focused: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let archived = self.archived_elements(focused, cx);
        if self.visible.projects.is_empty() && self.visible.chats.is_empty() {
            let text = if self.searching() {
                "No match"
            } else {
                "No chats yet"
            };
            let mut out = vec![no_chats(text, px(8.), cx)];
            out.extend(archived);
            return out;
        }
        let mut out = self.project_elements(focused, cx);
        out.extend(self.loose_elements(focused, cx));
        out.extend(archived);
        out
    }
    /// The projects: each header, and under an unfolded one its chats.
    fn project_elements(&self, focused: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for (index, project) in self.visible.projects.iter().enumerate() {
            let header = self.project_header(index, project, focused, cx);
            out.push(under_heading(index == 0, "PROJECTS", header, cx));
            if !self.is_open(&project.id) {
                continue;
            }
            for (row, chat) in project.chats.iter().enumerate() {
                let (project, root) = (Some(project.id.as_str()), project.root.as_str());
                let place = Place {
                    index,
                    row,
                    project,
                    root,
                };
                out.push(self.chat_row(place, chat, focused, cx));
            }
        }
        out
    }

    /// The chats in no project, or the hint that says how they appear. The
    /// hint is not a row: it comes last, so the rows keep their places.
    fn loose_elements(&self, focused: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let index = self.visible.projects.len();
        let mut out = Vec::new();
        for (row, chat) in self.visible.chats.iter().enumerate() {
            let place = Place {
                index,
                row,
                project: None,
                root: "",
            };
            let item = self.chat_row(place, chat, focused, cx);
            out.push(under_heading(row == 0, "CHATS", item, cx));
        }
        if self.visible.chats.is_empty() && !self.searching() {
            out.push(with_heading("CHATS", loose_hint(cx), cx));
        }
        out
    }

    fn project_header(
        &self,
        index: usize,
        project: &ProjectChats,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (muted, ring) = (cx.theme().muted_foreground, cx.theme().ring);
        let expanded = self.is_open(&project.id);
        let (id, menu_id) = (project.id.clone(), project.id.clone());
        let keyed = focused && self.selected == Some(ChatRow::Project(project.id.clone()));
        let new_chat = ChatsEvent::NewIn {
            project: project.id.clone(),
            root: project.root.clone(),
        };
        let group = SharedString::from(format!("project-row-{index}"));
        let name = self.row_title(true, &project.id, &project.name, cx);
        let shown = keyed || self.menu_is_on(true, &project.id);
        let more = format!("chats-project-more-{index}");
        let mut header = row_frame(format!("chats-project-{index}"), group.clone(), cx)
            .px_2()
            .py_1()
            .on_mouse_down(MouseButton::Right, on_right_click((true, menu_id), cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.clear_action(cx);
                this.toggle(&id, cx);
            }))
            .child(
                div()
                    .w(px(12.))
                    .text_color(muted)
                    .child(if expanded { "▾" } else { "▸" }),
            )
            .child(name)
            .children(project.pinned.then(|| pin_mark(muted)))
            .children((project.dirs() > 1).then(|| dirs_mark(project.dirs(), muted)))
            .child(new_in_project(index, new_chat, cx))
            .child(more_button(
                More {
                    selector: more,
                    group,
                    target: (true, project.id.clone()),
                    shown,
                },
                cx,
            ));
        if keyed {
            header = header.border_color(ring).rounded_sm();
        }
        let mut block = v_flex().child(header);
        block = block.children(self.popup(true, &project.id, &format!("p{index}"), cx));
        if expanded && project.chats.is_empty() {
            block = block.child(no_chats("No chats yet", px(22.), cx));
        }
        block.into_any_element()
    }

    fn chat_row(
        &self,
        place: Place<'_>,
        chat: &ChatEntry,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Place { index, row, .. } = place;
        let (muted, active, ring) = (
            cx.theme().muted_foreground,
            cx.theme().list_active,
            cx.theme().ring,
        );
        let selected = self.current_id.as_deref() == Some(chat.id.as_str());
        let here = ChatRow::Chat {
            project: place.project.map(str::to_owned),
            id: chat.id.clone(),
        };
        let keyed = focused && self.selected == Some(here);
        let badge = self
            .activity
            .get(&chat.id)
            .map(|waiting| if *waiting { "⏳" } else { "●" });
        let (root, id, menu_id) = (place.root.to_owned(), chat.id.clone(), chat.id.clone());
        let group = SharedString::from(format!("chat-row-{index}-{row}"));
        let title = self.row_title(false, &chat.id, &chat.title, cx);
        let shown = selected || keyed || self.menu_is_on(false, &chat.id);
        let more = format!("chats-more-{index}-{row}");
        let mut item = row_frame(format!("chats-session-{index}-{row}"), group.clone(), cx)
            .pl(px(22.))
            .pr_2()
            .py_1()
            .on_mouse_down(MouseButton::Right, on_right_click((false, menu_id), cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.clear_action(cx);
                if this.current_id.as_deref() != Some(id.as_str()) {
                    let (root, id) = (root.clone(), id.clone());
                    cx.emit(ChatsEvent::Open { root, id });
                }
            }))
            .child(title)
            .children(chat_marks(chat, badge, muted))
            .child(more_button(
                More {
                    selector: more,
                    group,
                    target: (false, chat.id.clone()),
                    shown,
                },
                cx,
            ));
        if selected {
            item = item.bg(active).rounded_sm();
        }
        if keyed {
            let selector = format!("chats-keyed-{index}-{row}");
            item = item
                .border_color(ring)
                .rounded_sm()
                .debug_selector(move || selector.clone());
        }
        v_flex()
            .child(item)
            .children(self.popup(false, &chat.id, &format!("{index}-{row}"), cx))
            .into_any_element()
    }
}

/// The frame every row shares: its selector and group, the transparent edge
/// that a key paints, and the fill under the pointer.
fn row_frame(selector: String, group: SharedString, cx: &App) -> Stateful<Div> {
    let active = cx.theme().list_active;
    h_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .group(group)
        .w_full()
        .gap_1()
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(move |style| style.bg(active).rounded_sm())
}

/// What a chat's row shows after its title: a pin, the branch icon and the
/// badge of a turn running out of sight.
fn chat_marks(chat: &ChatEntry, badge: Option<&'static str>, muted: gpui::Hsla) -> Vec<AnyElement> {
    let mut marks = Vec::new();
    if chat.pinned {
        marks.push(pin_mark(muted));
    }
    if let Some(branch) = &chat.branch {
        marks.push(branch_mark(branch.banner(), muted));
    }
    if let Some(badge) = badge {
        marks.push(
            div()
                .debug_selector(|| "chats-activity".to_owned())
                .text_xs()
                .text_color(muted)
                .child(badge)
                .into_any_element(),
        );
    }
    marks
}

/// The `N dirs` of a project with more than one directory.
fn dirs_mark(dirs: usize, muted: gpui::Hsla) -> AnyElement {
    div()
        .text_xs()
        .text_color(muted)
        .child(format!("{dirs} dirs"))
        .into_any_element()
}

/// The `+` of a project row: a new chat in that project.
fn new_in_project(index: usize, event: ChatsEvent, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let click = cx.listener(move |_, _, _, cx| {
        cx.stop_propagation();
        cx.emit(event.clone());
    });
    icon_button(format!("chats-project-new-{index}"), Glyph::Plus, click, cx).into_any_element()
}

/// The `...` of a row: its menu.
struct More {
    selector: String,
    /// The row's group: the button shows while the pointer is on the row.
    group: SharedString,
    /// Whether the row is a project, and its id.
    target: (bool, String),
    /// Shows without the pointer (the row is open or keyed).
    shown: bool,
}

fn more_button(more: More, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let More {
        selector,
        group,
        target: (project, target),
        shown,
    } = more;
    let click = cx.listener(move |this, _, window, cx| {
        cx.stop_propagation();
        this.open_menu_of(project, &target, window, cx);
    });
    icon_button(selector, Glyph::More, click, cx)
        .when(!shown, |button| {
            button
                .invisible()
                .group_hover(group, |style| style.visible())
        })
        .into_any_element()
}

/// A right click on a row opens its menu (`target` is whether the row is a
/// project, and its id).
fn on_right_click(
    target: (bool, String),
    cx: &mut Context<ChatsPanel>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
        cx.stop_propagation();
        this.open_menu_of(target.0, &target.1, window, cx);
    })
}

/// The name field in place of a row's label. A click on it does not fold or
/// open the row, and a click away saves a changed name.
pub(super) fn rename_field(input: &Entity<InputState>, cx: &mut Context<ChatsPanel>) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Left {
                this.click_outside(cx);
            }
        }))
        .child(Input::new(input).small())
        .into_any_element()
}

/// The small pin of a pinned row.
fn pin_mark(muted: gpui::Hsla) -> AnyElement {
    Glyph::Pin
        .icon()
        .xsmall()
        .text_color(muted)
        .into_any_element()
}

/// The branch icon of a branched chat; hovering it says where from.
fn branch_mark(banner: String, muted: gpui::Hsla) -> AnyElement {
    let title = SharedString::from(banner);
    div()
        .id("chats-branch")
        .debug_selector(|| "chats-branch".to_owned())
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(title.clone()).build(window, cx)
        })
        .child(Glyph::Branch.icon().xsmall().text_color(muted))
        .into_any_element()
}

/// The muted "No chats yet" line, indented by `indent`.
pub(super) fn no_chats(text: &'static str, indent: gpui::Pixels, cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "chats-empty".to_owned())
        .pl(indent)
        .pr_2()
        .py_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// Where a chat row sits: its group's position, its own, and the group.
#[derive(Clone, Copy)]
struct Place<'a> {
    index: usize,
    row: usize,
    /// The project's id; `None` for the chats in no project.
    project: Option<&'a str>,
    /// The directory the chat opens in; empty for a chat with none.
    root: &'a str,
}

/// `element` under the small heading of a section.
fn with_heading(title: &'static str, element: AnyElement, cx: &App) -> AnyElement {
    v_flex()
        .child(
            div()
                .px_2()
                .pt_2()
                .pb_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
        .child(element)
        .into_any_element()
}

/// `element`, under the heading `title` when it is the `first` of its section.
fn under_heading(first: bool, title: &'static str, element: AnyElement, cx: &App) -> AnyElement {
    if first {
        with_heading(title, element, cx)
    } else {
        element
    }
}

/// What the chats section says while it has no chat.
fn loose_hint(cx: &App) -> AnyElement {
    div()
        .debug_selector(|| "chats-loose-empty".to_owned())
        .px_2()
        .py_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child("Chats without a project show here")
        .into_any_element()
}
