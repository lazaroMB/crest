//! The rows of the full list: project headers, chats, the section headings
//! and the menu of a chat.

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::{ChatActionKind, ChatEntry, ChatRow, ChatsEvent, ChatsPanel, ProjectChats, label};

impl ChatsPanel {
    /// One element per row of [`Self::rows`]: a project header (with its
    /// "No chats yet" line) or a chat (with its open menu), so the scroll
    /// can keep the selected row in view.
    pub(super) fn row_elements(&self, focused: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.visible.projects.is_empty() && self.visible.chats.is_empty() {
            let text = if self.searching() {
                "No match"
            } else {
                "No chats yet"
            };
            return vec![no_chats(text, px(8.), cx)];
        }
        let mut out = self.project_elements(focused, cx);
        out.extend(self.loose_elements(focused, cx));
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
        let theme = cx.theme();
        let (muted, active, ring) = (theme.muted_foreground, theme.list_active, theme.ring);
        let expanded = self.is_open(&project.id);
        let marker = if expanded { "▾" } else { "▸" };
        let id = project.id.clone();
        let keyed = focused && self.selected == Some(ChatRow::Project(project.id.clone()));
        let new_chat = ChatsEvent::NewIn {
            project: project.id.clone(),
            root: project.root.clone(),
        };
        let mut header = h_flex()
            .id(SharedString::from(format!("chats-project-{index}")))
            .debug_selector(move || format!("chats-project-{index}"))
            .w_full()
            .gap_1()
            .px_2()
            .py_1()
            .border_1()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .hover(move |style| style.bg(active).rounded_sm())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
            .child(div().w(px(12.)).text_color(muted).child(marker))
            .child(label(&project.name))
            .children((project.dirs > 1).then(|| {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{} dirs", project.dirs))
            }))
            .child(Self::new_in_project(index, new_chat, cx));
        if keyed {
            header = header.border_color(ring).rounded_sm();
        }
        let mut block = v_flex().child(header);
        if expanded && project.chats.is_empty() {
            block = block.child(no_chats("No chats yet", px(22.), cx));
        }
        block.into_any_element()
    }

    /// What a turn running out of sight shows on the row of the chat `id`:
    /// waiting on the user, or busy.
    fn activity_badge(&self, id: &str) -> Option<&'static str> {
        self.activity
            .get(id)
            .map(|waiting| if *waiting { "⏳" } else { "●" })
    }

    /// The `+` of a project row: a new chat in that project.
    fn new_in_project(index: usize, event: ChatsEvent, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active) = (theme.muted_foreground, theme.list_active);
        div()
            .id(SharedString::from(format!("chats-project-new-{index}")))
            .debug_selector(move || format!("chats-project-new-{index}"))
            .px_1()
            .text_color(muted)
            .cursor_pointer()
            .hover(move |style| style.bg(active).rounded_sm())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(event.clone());
            }))
            .child("+")
    }

    fn chat_row(
        &self,
        place: Place<'_>,
        chat: &ChatEntry,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Place { index, row, .. } = place;
        let theme = cx.theme();
        let (muted, active, ring) = (theme.muted_foreground, theme.list_active, theme.ring);
        let selected = self.current_id.as_deref() == Some(chat.id.as_str());
        let keyed = focused
            && self.selected
                == Some(ChatRow::Chat {
                    project: place.project.map(str::to_owned),
                    id: chat.id.clone(),
                });
        let badge = self.activity_badge(&chat.id);
        let (root, id, title) = (place.root.to_owned(), chat.id.clone(), chat.title.clone());
        let (menu_root, menu_id, menu_title) = (root.clone(), id.clone(), title.clone());
        let menu_for_chat = self.action.as_ref().is_some_and(|action| action.id == id);
        let selector = format!("chats-session-{index}-{row}");
        let mut item = h_flex()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .w_full()
            .pl(px(22.))
            .pr_2()
            .py_1()
            .border_1()
            .border_color(gpui::transparent_black())
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
        if keyed {
            let selector = format!("chats-keyed-{index}-{row}");
            item = item
                .border_color(ring)
                .rounded_sm()
                .debug_selector(move || selector.clone());
        }
        let mut block = v_flex().child(item);
        if menu_for_chat {
            block = block.child(self.menu(index, row, cx));
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
            .id("chats-menu")
            .debug_selector(|| "chats-menu".to_owned())
            // A click elsewhere closes it (a right click on another chat
            // opens that one's).
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.click_outside(cx);
                }
            }))
            // A click inside keeps the focus where it is (the rename
            // field), instead of the list taking it and the field
            // losing it.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
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
                    ))
                    .child(menu_row(
                        format!("chats-cancel-{index}-{row}"),
                        "Cancel",
                        false,
                        active,
                        danger,
                        cx.listener(|this, _, _, cx| this.cancel_action(cx)),
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
                        )
                        .child(menu_row(
                            format!("chats-cancel-rename-{index}-{row}"),
                            "Cancel",
                            false,
                            active,
                            danger,
                            cx.listener(|this, _, _, cx| this.cancel_action(cx)),
                        ));
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
                        cx.listener(|this, _, _, cx| this.cancel_action(cx)),
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
    let selector = id.clone();
    let mut row = h_flex()
        .id(SharedString::from(id))
        .debug_selector(move || selector.clone())
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

/// The muted "No chats yet" line, indented by `indent`.
fn no_chats(text: &'static str, indent: gpui::Pixels, cx: &App) -> AnyElement {
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
