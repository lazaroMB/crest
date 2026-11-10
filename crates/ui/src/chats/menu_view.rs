//! The floating popup of a chat or project menu ([`super::menu`] holds the
//! rows). It is drawn in a `deferred` + `anchored` layer: out of the
//! layout, so opening it never moves the rows under it, and over the list
//! instead of clipped by its scroll.

use gpui::{
    AnyElement, Context, Div, Entity, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, anchored, deferred, div, point, px,
};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use super::menu::{Item, Look, Page};
use super::rows::rename_field;
use super::{ChatsPanel, Subject, label};

/// How wide a menu is.
const WIDTH: f32 = 216.0;
/// The most a menu grows before it scrolls.
const MAX_HEIGHT: f32 = 360.0;

impl ChatsPanel {
    /// The popup of the open menu when it is about the chat or project
    /// `id` (`project` tells which), placed where it is added: under that
    /// row. `tag` names the rows in selectors. `None` when the menu is
    /// about another row, or on the name field (that has no popup).
    pub(super) fn popup(
        &self,
        project: bool,
        id: &str,
        tag: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self
            .action
            .as_ref()
            .filter(|menu| menu.subject.is(project, id) && menu.page != Page::Rename)?;
        let rows = popup_rows(&self.menu_items(), menu.cursor, tag, cx);
        let layer = anchored()
            .offset(point(px(8.), px(0.)))
            .snap_to_window_with_margin(px(8.))
            .child(frame(cx).children(rows));
        Some(deferred(layer).with_priority(2).into_any_element())
    }

    /// The name field of the chat or project `id`, while it is renamed.
    pub(super) fn renaming(&self, project: bool, id: &str) -> Option<&Entity<InputState>> {
        self.action
            .as_ref()
            .filter(|menu| menu.page == Page::Rename && menu.subject.is(project, id))
            .and(self.rename_input.as_ref())
    }

    /// What a row shows as its name: the text, or its field while it is
    /// renamed.
    pub(super) fn row_title(
        &self,
        project: bool,
        id: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.renaming(project, id) {
            Some(input) => rename_field(input, cx),
            None => label(text).into_any_element(),
        }
    }

    /// Whether the open menu is about this row (the name field counts).
    pub(super) fn menu_is_on(&self, project: bool, id: &str) -> bool {
        self.action
            .as_ref()
            .is_some_and(|menu| menu.subject.is(project, id))
    }

    /// Opens the menu of the chat or project `id` (a right click or the
    /// `...` button).
    pub(super) fn open_menu_of(
        &mut self,
        project: bool,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subject: Option<Subject> = if project {
            self.listed.project_subject(id)
        } else {
            self.listed.chat_subject(id)
        };
        if let Some(subject) = subject {
            self.open_menu(subject, Page::Main, window, cx);
        }
    }
}

/// The box of the menu: its look, and the clicks that close it or keep the
/// focus where it is.
fn frame(cx: &mut Context<ChatsPanel>) -> Stateful<Div> {
    let theme = cx.theme();
    let (popover, border) = (theme.popover, theme.border);
    v_flex()
        .id("chats-menu")
        .debug_selector(|| "chats-menu".to_owned())
        .w(px(WIDTH))
        .max_h(px(MAX_HEIGHT))
        .overflow_y_scroll()
        .py_1()
        .border_1()
        .border_color(border)
        .rounded_md()
        .bg(popover)
        .shadow_md()
        .text_sm()
        .occlude()
        // A click elsewhere closes it (a right click on another row opens
        // that one's).
        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Left {
                this.cancel_action(cx);
            }
        }))
        // A click inside keeps the focus where it is.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// The rows of a page. Rows of one kind (the projects, the directories) are
/// told apart by their order in the selector; the first keeps the plain name.
fn popup_rows(
    items: &[Item],
    cursor: usize,
    tag: &str,
    cx: &mut Context<ChatsPanel>,
) -> Vec<AnyElement> {
    let mut seen: Vec<&str> = Vec::new();
    let mut rows = Vec::new();
    for (ix, item) in items.iter().enumerate() {
        let before = seen.iter().filter(|key| **key == item.key).count();
        seen.push(item.key);
        let suffix = if before == 0 {
            String::new()
        } else {
            format!("-{before}")
        };
        let at = RowAt {
            ix,
            on_cursor: ix == cursor,
            selector: format!("chats-{}-{tag}{suffix}", item.key),
        };
        rows.push(menu_row(item, at, cx));
    }
    rows
}

/// Where a row is in its menu.
struct RowAt {
    ix: usize,
    /// The keyboard is on it.
    on_cursor: bool,
    selector: String,
}

fn menu_row(item: &Item, at: RowAt, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let theme = cx.theme();
    let (danger, muted) = (theme.danger, theme.muted_foreground);
    let label = SharedString::from(item.label.clone());
    let selector = at.selector.clone();
    let row = h_flex()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .w_full()
        .gap_2()
        .px_2();
    match item.look {
        Look::Heading => row.pt_1().text_xs().text_color(muted).child(label),
        Look::Note => row.text_xs().text_color(muted).child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(label),
        ),
        Look::Warning => row.py_1().text_color(danger).child(label),
        Look::Disabled => row.py_1().text_color(muted).child(label),
        Look::Plain | Look::Danger => return choice_row(row, item, &at, cx),
    }
    .into_any_element()
}

/// A row that does something: under the keyboard or the pointer it has a
/// fill, a click runs it.
fn choice_row(
    row: Stateful<gpui::Div>,
    item: &Item,
    at: &RowAt,
    cx: &mut Context<ChatsPanel>,
) -> AnyElement {
    let theme = cx.theme();
    let (danger, muted, active) = (theme.danger, theme.muted_foreground, theme.list_active);
    let ix = at.ix;
    let mut row = row
        .py_1()
        .cursor_pointer()
        .hover(move |style| style.bg(active))
        .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)));
    if at.on_cursor {
        row = row.bg(active);
    }
    if item.look == Look::Danger {
        row = row.text_color(danger);
    }
    let mark = item.mark.map(|mark| mark.icon().small().text_color(muted));
    row.children(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(SharedString::from(item.label.clone())),
        )
        .children(
            item.opens_page()
                .then(|| div().text_color(muted).child("›")),
        )
        .into_any_element()
}
