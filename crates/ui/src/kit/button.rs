//! The small text buttons of block toolbars.

use gpui::{
    App, FontWeight, InteractiveElement as _, ParentElement as _, SharedString, Stateful,
    Styled as _, div,
};
use gpui_component::ActiveTheme as _;

use super::tokens::{Radius, Space, TokenStyled as _};
use crate::theme::token;

/// A toolbar button in the accent colors; `id` doubles as its debug
/// selector.
pub fn tool_button(
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let accent_hover = token(cx, "accent.hover.bg");
    base(id, label, cx)
        .bg(token(cx, "accent.bg"))
        .hover(move |style| style.bg(accent_hover))
        .text_color(token(cx, "accent.fg"))
        .font_weight(FontWeight::SEMIBOLD)
}

/// A toolbar button in the secondary (button) colors, for actions that
/// should not draw the eye; `id` doubles as its debug selector.
pub fn quiet_button(
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let theme = cx.theme();
    let hover = theme.secondary_hover;
    base(id, label, cx)
        .bg(theme.secondary)
        .hover(move |style| style.bg(hover))
        .text_color(theme.secondary_foreground)
}

/// A toolbar button in the danger colors, for the action that destroys
/// something (the confirmation of a delete); `id` doubles as its debug
/// selector.
pub fn danger_button(
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let theme = cx.theme();
    let hover = theme.danger_hover;
    base(id, label, cx)
        .bg(theme.danger)
        .hover(move |style| style.bg(hover))
        .text_color(theme.danger_foreground)
        .font_weight(FontWeight::SEMIBOLD)
}

fn base(id: &'static str, label: impl Into<SharedString>, cx: &App) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .tk_px(Space::Sm, cx)
        .tk_rounded(Radius::Sm, cx)
        .cursor_pointer()
        .child(label.into())
}
