//! The sidebar in its three widths: the full list, the strip of icons and
//! the hidden state that holds only the Crest mark.
//!
//! The panel is always laid out as an overlay on the top left of the
//! window's body (`absolute`); [`SidebarLayout`] tells the window how much
//! room to leave beside it, so the conversation never sits under the strip
//! or the full list. Hidden keeps no column: the rows that hover or focus
//! reveal are a floating bar over the conversation, which does not move.

use gpui::Hsla;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, deferred, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, v_flex};

use super::model::CurrentChat;
use super::model::{
    FULL_WIDTH, MARK_BAND, RailState, STRIP_WIDTH, SidebarWidth, initial, rail_tokens,
};
use super::{ChatsEvent, ChatsPanel, ProjectChats, label};

/// Width of the hidden state's mark zone while the rows are closed.
const MARK_ZONE: f32 = 36.0;

/// What the window leaves for the sidebar in its body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarLayout {
    /// A column kept free on the left: the conversation starts after it.
    pub reserve: Pixels,
    /// A band kept free on the top: the conversation starts below the
    /// mark while it is hidden and closed.
    pub band: Pixels,
}

impl ChatsPanel {
    /// How wide the sidebar is.
    #[must_use]
    pub fn width(&self) -> SidebarWidth {
        self.width
    }

    /// Sets the width; the window hears of a change as
    /// [`ChatsEvent::Sidebar`]. Leaving the full width closes the search.
    pub fn set_width(&mut self, width: SidebarWidth, cx: &mut Context<Self>) {
        if width == self.width {
            return;
        }
        self.width = width;
        self.pointer_on_rail = false;
        self.pointer_on_project = None;
        if width != SidebarWidth::Full {
            self.close_search(cx);
        }
        cx.emit(ChatsEvent::Sidebar(width));
        cx.notify();
    }

    /// `panel.chats`: full, strip, hidden, and full again.
    pub fn cycle(&mut self, cx: &mut Context<Self>) {
        self.set_width(self.width.next(), cx);
    }

    /// Hidden, with the pointer or the keyboard on its mark: its rows show.
    fn revealed(&self, window: &Window) -> bool {
        self.width == SidebarWidth::Hidden
            && (self.pointer_on_rail || self.focus_handle.is_focused(window))
    }

    /// The room the window leaves for the sidebar now: its column, or for
    /// hidden a band under the mark. Showing the rows of the hidden state
    /// takes none, they float.
    #[must_use]
    pub fn layout(&self) -> SidebarLayout {
        let (reserve, band) = match self.width {
            SidebarWidth::Hidden => (0.0, MARK_BAND),
            width => (width.column(), 0.0),
        };
        SidebarLayout {
            reserve: px(reserve),
            band: px(band),
        }
    }

    /// Keys on the focused mark of the hidden state: `Enter` or `Space`
    /// open the full sidebar, `Esc` gives the keyboard back to the prompt.
    pub(super) fn on_rail_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "enter" | "space" => self.set_width(SidebarWidth::Full, cx),
            "escape" => cx.emit(ChatsEvent::Dismissed),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The strip's project under the pointer, or no longer.
    fn hover_project(&mut self, id: &str, hovered: bool, cx: &mut Context<Self>) {
        let on = self.pointer_on_project.as_deref() == Some(id);
        if hovered == on {
            return;
        }
        self.pointer_on_project = hovered.then(|| id.to_owned());
        cx.notify();
    }

    /// A project's initial: its newest chat, or a new one in the project.
    fn open_project(&mut self, project: &ProjectChats, cx: &mut Context<Self>) {
        match project.chats.first() {
            Some(chat) => cx.emit(ChatsEvent::Open {
                root: project.root.clone(),
                id: chat.id.clone(),
            }),
            None => cx.emit(ChatsEvent::NewIn {
                project: project.id.clone(),
                root: project.root.clone(),
            }),
        }
    }
}

/// What a click on a button of the rail does to the panel.
type Click = Box<dyn Fn(&mut ChatsPanel, &mut Window, &mut Context<ChatsPanel>)>;

/// A click that sets the sidebar's width.
fn go(width: SidebarWidth) -> Click {
    Box::new(move |this, _, cx| this.set_width(width, cx))
}

/// A click that starts a chat with no project and no directory.
fn new_chat() -> Click {
    Box::new(|_, _, cx| cx.emit(ChatsEvent::NewStandalone))
}

/// A button of the rail: its id (also its debug selector), icon and label.
struct RailItem {
    id: &'static str,
    icon: IconName,
    title: &'static str,
}

const NEW_CHAT: RailItem = RailItem {
    id: "sidebar-new",
    icon: IconName::Plus,
    title: "New chat",
};
const SEARCH: RailItem = RailItem {
    id: "sidebar-search",
    icon: IconName::Search,
    title: "Search",
};
const PROJECTS: RailItem = RailItem {
    id: "sidebar-projects",
    icon: IconName::Folder,
    title: "Projects",
};

/// The overlay every width shares: pinned top left, `width` wide, with the
/// panel's keyboard focus and keys.
fn frame(panel: &ChatsPanel, width: f32, cx: &mut Context<ChatsPanel>) -> Stateful<gpui::Div> {
    let theme = cx.theme();
    let (sidebar, foreground, border) = (theme.sidebar, theme.sidebar_foreground, theme.border);
    v_flex()
        .id("chats-panel")
        .debug_selector(|| "chats-panel".to_owned())
        .track_focus(&panel.focus_handle)
        .on_key_down(cx.listener(ChatsPanel::on_list_key))
        .absolute()
        .top_0()
        .left_0()
        .w(px(width))
        .h_full()
        .border_r_1()
        .border_color(border)
        .bg(sidebar)
        .text_color(foreground)
}

/// The ink and the fill of a rail control in `state`, from the theme.
fn rail_colors(state: RailState, cx: &App) -> (Hsla, Hsla) {
    let (ink, fill) = rail_tokens(state);
    (crate::theme::token(cx, ink), crate::theme::token(cx, fill))
}

/// The look of a rail button: the same ink in every state, a fill under
/// the pointer and while pressed. Set from tokens so the icon never takes
/// the colour of the fill behind it.
fn rail_look(cx: &App) -> ButtonCustomVariant {
    let (ink, hover) = rail_colors(RailState::Hover, cx);
    ButtonCustomVariant::new(cx)
        .foreground(ink)
        .hover(hover)
        .active(hover)
}

/// The Crest mark as a button that sets the width `to`; `title` says so.
fn mark_button(title: &'static str, to: SidebarWidth, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let click = go(to);
    div()
        .debug_selector(|| "sidebar-mark".to_owned())
        .child(
            Button::new("sidebar-mark")
                .custom(rail_look(cx))
                .small()
                .tooltip(title)
                .child(crate::brand::mark(13.0))
                .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx))),
        )
        .into_any_element()
}

/// An icon button of the strip.
fn icon_button(item: RailItem, click: Click, cx: &mut Context<ChatsPanel>) -> AnyElement {
    div()
        .debug_selector(|| item.id.to_owned())
        .child(
            Button::new(item.id)
                .custom(rail_look(cx))
                .icon(item.icon)
                .tooltip(item.title)
                .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx))),
        )
        .into_any_element()
}

/// The strip's rows under the mark: new chat, search, projects, a rule and
/// one initial per project. Shared by the strip and the hidden reveal.
fn strip_rows(panel: &ChatsPanel, cx: &mut Context<ChatsPanel>) -> Vec<AnyElement> {
    let border = cx.theme().border;
    let search: Click = Box::new(ChatsPanel::open_search);
    let mut rows = vec![
        icon_button(NEW_CHAT, new_chat(), cx),
        icon_button(SEARCH, search, cx),
        icon_button(PROJECTS, go(SidebarWidth::Full), cx),
        div()
            .w(px(20.))
            .my_1()
            .border_t_1()
            .border_color(border)
            .into_any_element(),
    ];
    for (index, project) in panel.listed.projects.iter().enumerate() {
        rows.push(project_initial(panel, index, project, cx));
    }
    rows
}

/// The label that floats beside a project's initial.
fn name_tag(name: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    deferred(
        div()
            .debug_selector(|| "sidebar-initial-name".to_owned())
            .absolute()
            .left(px(36.))
            .top(px(1.))
            .px_2()
            .py(px(2.))
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .bg(theme.popover)
            .text_color(theme.foreground)
            .whitespace_nowrap()
            .child(SharedString::from(name.to_owned())),
    )
    .with_priority(1)
}

/// A project's initial, with its name beside it while the pointer is on it.
fn project_initial(
    panel: &ChatsPanel,
    index: usize,
    project: &ProjectChats,
    cx: &mut Context<ChatsPanel>,
) -> AnyElement {
    let theme = cx.theme();
    let (border, ring) = (theme.border, theme.ring);
    let current = CurrentChat {
        root: &panel.current_root,
        project: panel.current_project.as_deref(),
        id: None,
    };
    let open = current.is_in(project);
    let named = panel.pointer_on_project.as_deref() == Some(project.id.as_str());
    let state = match (open, named) {
        (true, _) => RailState::Active,
        (false, true) => RailState::Hover,
        (false, false) => RailState::Idle,
    };
    let (ink, fill) = rail_colors(state, cx);
    let edge = if state == RailState::Idle {
        border
    } else {
        ring
    };
    let selector = format!("sidebar-initial-{index}");
    let (hover_id, click_project) = (project.id.clone(), project.clone());
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .relative()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(edge)
        .rounded_md()
        .when(state != RailState::Idle, |item| item.bg(fill))
        .text_sm()
        .text_color(ink)
        .cursor_pointer()
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            this.hover_project(&hover_id, *hovered, cx);
        }))
        .on_click(cx.listener(move |this, _, _, cx| this.open_project(&click_project, cx)))
        .child(initial(&project.name))
        .when(named, |item| item.child(name_tag(&project.name, cx)))
        .into_any_element()
}

/// The full sidebar: mark and name, new chat, search, then the list.
pub(super) fn full(
    panel: &ChatsPanel,
    window: &Window,
    cx: &mut Context<ChatsPanel>,
) -> AnyElement {
    let focused = panel.focus_handle.is_focused(window);
    let rows = panel.row_elements(focused, cx);
    let toggle: Click = Box::new(toggle_search);
    frame(panel, FULL_WIDTH, cx)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .pt_1()
                .child(mark_button("Hide the sidebar", SidebarWidth::Hidden, cx))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Crest")),
        )
        .child(nav_row("chats-new", NEW_CHAT, new_chat(), cx))
        .child(nav_row("chats-search", SEARCH, toggle, cx))
        .children(
            panel
                .search
                .as_ref()
                .map(|input| div().px_2().pb_1().child(Input::new(input))),
        )
        .child(crate::keyscroll::list("chats-list", &panel.scroll, rows).flex_1())
        .into_any_element()
}

fn toggle_search(this: &mut ChatsPanel, window: &mut Window, cx: &mut Context<ChatsPanel>) {
    if this.search.is_some() {
        this.close_search(cx);
        cx.emit(ChatsEvent::Dismissed);
    } else {
        this.open_search(window, cx);
    }
}

/// A row of the full sidebar's top: an icon and a label. `id` is its
/// selector (the strip's button of the same item has its own).
fn nav_row(
    id: &'static str,
    item: RailItem,
    click: Click,
    cx: &mut Context<ChatsPanel>,
) -> impl IntoElement {
    let (active, muted) = (cx.theme().list_active, cx.theme().muted_foreground);
    div()
        .id(id)
        .debug_selector(|| id.to_owned())
        .flex()
        .items_center()
        .gap_2()
        .mx_1()
        .px_2()
        .py_1()
        .cursor_pointer()
        .hover(move |style| style.bg(active).rounded_sm())
        .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
        .child(Icon::new(item.icon).small().text_color(muted))
        .child(label(item.title))
}

/// The strip: a fixed column with the mark and the rows.
pub(super) fn strip(panel: &ChatsPanel, cx: &mut Context<ChatsPanel>) -> AnyElement {
    let rows = strip_rows(panel, cx);
    frame(panel, STRIP_WIDTH, cx)
        .items_center()
        .gap_1()
        .pt_1()
        .child(mark_button("Show the full sidebar", SidebarWidth::Full, cx))
        .children(rows)
        .into_any_element()
}

/// Hidden: only the mark, top left. Pointer hover or keyboard focus on it
/// shows the strip's rows as a floating bar over the conversation (its own
/// fill, edge and shadow, never taller than the body); the window leaves no
/// column for it, so nothing moves. Leaving hides them without changing the
/// width. Clicking the mark opens the full sidebar.
pub(super) fn hidden(
    panel: &ChatsPanel,
    window: &Window,
    cx: &mut Context<ChatsPanel>,
) -> AnyElement {
    let open = panel.revealed(window);
    let rows = if open {
        strip_rows(panel, cx)
    } else {
        Vec::new()
    };
    let mark = mark_button("Show the sidebar", SidebarWidth::Full, cx);
    let zone = if open {
        // Floating: as tall as its rows, at most the body, and it takes the
        // pointer so the conversation under it does not.
        frame(panel, STRIP_WIDTH, cx)
            .h_auto()
            .max_h_full()
            .pb_1()
            .border_1()
            .rounded_br_md()
            .shadow_md()
            .occlude()
    } else {
        // Closed: no frame, just the mark's own corner.
        frame(panel, MARK_ZONE, cx)
            .h(px(MARK_BAND))
            .border_0()
            .bg(gpui::transparent_black())
    };
    zone.items_center()
        .gap_1()
        .pt_1()
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            this.pointer_on_rail = *hovered;
            cx.notify();
        }))
        .child(mark)
        .child(
            // The rows scroll when the body is too short for them.
            v_flex()
                .id("sidebar-hidden-rows")
                .items_center()
                .gap_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use application_core::theme::{BUILTIN_NAMES, ThemeCatalog};

    use super::super::model::{RailState, rail_tokens};

    /// In every built-in theme and every state, a rail control's ink is not
    /// its own fill and reads on it, the fill laid over the bar's own
    /// background (the sidebar's).
    #[test]
    fn a_rail_control_never_paints_its_ink_in_the_colour_of_its_fill() {
        for name in BUILTIN_NAMES {
            let theme = ThemeCatalog::new().resolve_or_default(name, None).theme;
            let bar = theme.color("sidebar.bg");
            for state in [RailState::Idle, RailState::Hover, RailState::Active] {
                let (ink, fill) = rail_tokens(state);
                assert_ne!(ink, fill, "{name} {state:?}: one token for both");
                let ink = theme.color(ink);
                let surface = theme.color(fill).over(bar);
                assert_ne!(ink, surface, "{name} {state:?}: ink is its fill");
                let ratio = ink.contrast_ratio(surface);
                assert!(ratio >= 4.5, "{name} {state:?}: contrast {ratio:.2}");
            }
        }
    }
}
