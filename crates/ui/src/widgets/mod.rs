//! Shared widgets: a timeline of spans, a filterable list, a node-link
//! graph and a tree table.
//!
//! Each widget draws one spec of [`domain::widget`] and keeps its state
//! (zoom, filter, open rows) in an entity per id, through
//! [`crate::blocks::block_state`], so it survives the timeline dropping
//! off-screen rows. `Custom` block nodes use them
//! ([`crate::blocks::custom`]), and domain blocks (traces, logs, plans,
//! waveforms) call the same `view`/`render` functions with a spec their
//! parser built.

pub mod timeline;

use gpui::{
    App, FontWeight, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Stateful,
    Styled as _, div,
};
use gpui_component::ActiveTheme as _;

use crate::theme::token;

/// Theme color of palette slot `slot`, cycling the five chart colors.
pub fn palette(slot: usize, cx: &App) -> Hsla {
    crate::blocks::chart::series_color(slot, cx)
}

/// A small toolbar button in the accent colors, as the diagram views
/// have; `id` doubles as its debug selector.
pub fn tool_button(
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let (accent, accent_hover, accent_fg) = (
        token(cx, "accent.bg"),
        token(cx, "accent.hover.bg"),
        token(cx, "accent.fg"),
    );
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .px_1p5()
        .rounded_sm()
        .bg(accent)
        .hover(move |style| style.bg(accent_hover))
        .text_color(accent_fg)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(label.into())
}

/// A muted caption chip (a zoom readout, a count).
pub fn readout(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .px_1()
        .rounded_sm()
        .bg(cx.theme().muted)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
