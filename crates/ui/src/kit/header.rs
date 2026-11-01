//! The header row of a block: a title that takes the room left, muted
//! captions and a toolbar of buttons.
//!
//! ```ignore
//! kit::header(cx)
//!     .child(kit::title(title))
//!     .child(kit::caption("12 items", cx))
//!     .child(kit::toolbar(cx).child(kit::tool_button("fit", "fit", cx)))
//! ```

use gpui::{App, FontWeight, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::{ActiveTheme as _, h_flex};

use super::tokens::{Space, TextRole, TokenStyled as _};

/// The header row: its children in a line, `Space::Md` apart.
pub fn header(cx: &App) -> gpui::Div {
    h_flex().tk_gap(Space::Md, cx)
}

/// The title of a header: semibold, on one line, shortened with an
/// ellipsis, taking the room the rest leaves.
pub fn title(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A muted caption: a count, a hint, what the block shows.
pub fn caption(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .tk_text(TextRole::Caption, cx)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A row of toolbar buttons and readouts, in caption text.
pub fn toolbar(cx: &App) -> gpui::Div {
    h_flex()
        .tk_gap(Space::Xs, cx)
        .tk_text(TextRole::Caption, cx)
}
