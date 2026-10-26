//! Copying the text selected with the mouse in the timeline.
//!
//! Ctrl+C belongs to the prompt, where it works as in a terminal (clear the
//! prompt, interrupt the turn). Selecting text with the mouse gives the
//! timeline the focus, and there Ctrl+C copies, as in Windows Terminal.
//! `Ctrl+Shift+C` and `Ctrl+Insert` copy from anywhere and never interrupt.
//! While text is selected, a small hint next to the mouse offers to copy it.

use std::time::Duration;

use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels,
    Point, SharedString, StatefulInteractiveElement as _, Styled as _, Window, anchored, deferred,
    div, point, px,
};
use gpui_component::ActiveTheme as _;

/// How long the hint says "Copied" before it goes away.
pub const COPIED_TTL: Duration = Duration::from_millis(1200);

/// The chord that copies from anywhere, as the hint shows it.
pub fn copy_chord() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘C"
    } else {
        "Ctrl+Shift+C"
    }
}

/// The text selected in the window (the timeline's blocks), trimmed; empty
/// when there is none.
pub fn selected_text(window: &mut Window, cx: &mut App) -> String {
    gpui_base::TextSelection::selected_text(window, cx)
        .trim()
        .to_owned()
}

/// The hint shown next to a selection.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyHint {
    /// Where the mouse was released, in window coordinates.
    pub at: Point<Pixels>,
    /// The selected text when the hint appeared: clicking the hint copies
    /// it even if the click itself clears the selection.
    pub text: String,
    /// Whether it was copied (the hint then says so, briefly).
    pub copied: bool,
}

impl CopyHint {
    /// The text the hint shows.
    pub fn label(&self) -> SharedString {
        if self.copied {
            "Copied".into()
        } else {
            format!("Copy · {}", copy_chord()).into()
        }
    }
}

/// The hint, floating above the timeline just below and right of the mouse.
/// `on_click` copies.
pub fn render_hint(
    hint: &CopyHint,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let chip = div()
        .id("copy-hint")
        .debug_selector(|| "copy-hint".to_owned())
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .text_xs()
        .shadow_md()
        .cursor_pointer()
        .child(hint.label())
        // Keep the press from starting a new selection under the hint.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Releasing here must not reopen the hint as "Copy" over the copy.
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        });
    deferred(
        anchored()
            .position(hint.at + point(px(8.), px(12.)))
            .snap_to_window_with_margin(px(8.))
            .child(chip),
    )
    .with_priority(1)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_names_the_chord_then_confirms() {
        let mut hint = CopyHint {
            at: point(px(0.), px(0.)),
            text: "x".into(),
            copied: false,
        };
        assert_eq!(
            hint.label(),
            SharedString::from(format!("Copy · {}", copy_chord()))
        );
        hint.copied = true;
        assert_eq!(hint.label(), SharedString::from("Copied"));
    }
}
