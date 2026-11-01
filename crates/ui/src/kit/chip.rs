//! Chips: small labels in a color (a count, a severity, a tag), and the
//! toggle chips that filter a block.

use gpui::{
    App, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, div,
};
use gpui_component::ActiveTheme as _;

use super::tokens::{Radius, Space, TextRole, TokenStyled as _};

/// How a [`chip`] carries its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    /// A faint fill of the color, the text in the color.
    Soft,
    /// A border in the color, the text in the color.
    Outline,
}

/// A small label in `tint`.
pub fn chip(label: impl Into<SharedString>, tint: Hsla, kind: ChipKind, cx: &App) -> gpui::Div {
    let chip = div()
        .flex_none()
        .tk_px(Space::Xs, cx)
        .tk_rounded(Radius::Sm, cx)
        .tk_text(TextRole::Caption, cx)
        .text_color(tint);
    match kind {
        ChipKind::Soft => chip.bg(tint.opacity(0.15)),
        ChipKind::Outline => chip.border_1().border_color(tint),
    }
    .child(label.into())
}

/// A chip that filters a block by a category drawn in `tint` (a
/// severity, a test outcome): the label always in `tint`, with a border
/// and a faint fill in it while the filter is `on`. The caller adds the id
/// and the click handler.
pub fn filter_chip(label: impl Into<SharedString>, tint: Hsla, on: bool, cx: &App) -> gpui::Div {
    let chip = clickable(cx).text_color(tint);
    if on {
        chip.border_color(tint).bg(tint.opacity(0.15))
    } else {
        chip.border_color(cx.theme().border)
    }
    .child(label.into())
}

/// A chip that picks an option or a mode: in the accent color while
/// `on`, muted while off. The caller adds the id and the click handler.
pub fn toggle_chip(label: impl Into<SharedString>, on: bool, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    let chip = clickable(cx);
    if on {
        let accent = theme.primary;
        chip.border_color(accent)
            .bg(accent.opacity(0.15))
            .text_color(accent)
    } else {
        let hover = theme.muted;
        chip.border_color(theme.border)
            .text_color(theme.muted_foreground)
            .hover(move |style| style.bg(hover))
    }
    .child(label.into())
}

fn clickable(cx: &App) -> gpui::Div {
    div()
        .flex_none()
        .tk_px(Space::Sm, cx)
        .tk_rounded(Radius::Sm, cx)
        .border_1()
        .cursor_pointer()
}

/// A muted caption chip (a zoom readout, a count).
pub fn readout(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    let theme = cx.theme();
    div()
        .tk_px(Space::Xs, cx)
        .tk_rounded(Radius::Sm, cx)
        .bg(theme.muted)
        .text_color(theme.muted_foreground)
        .child(text.into())
}
