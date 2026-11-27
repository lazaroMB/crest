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
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, h_flex};

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

/// One chip of the hint bar: a label with a tooltip and a click handler.
fn chip(
    id: &'static str,
    label: SharedString,
    tip: &'static str,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id(id)
        .debug_selector(|| id.to_owned())
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
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        // Keep the press from starting a new selection under the hint.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Releasing here must not reopen the hint as "Copy" over the copy.
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label)
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
}

/// The selection bar, floating above the timeline just below and right of
/// the mouse: "Copy" and, unless it just copied, "Add to doc". `on_copy`
/// copies; `on_add` opens (or closes) the document picker, which is `menu`
/// when open.
pub fn render_hint(
    hint: &CopyHint,
    on_copy: impl Fn(&mut Window, &mut App) + 'static,
    on_add: impl Fn(&mut Window, &mut App) + 'static,
    menu: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let bar = h_flex()
        .id("selection-bar")
        .debug_selector(|| "selection-bar".to_owned())
        .gap_1()
        .child(chip(
            "copy-hint",
            hint.label(),
            "Copy the selected text",
            on_copy,
            cx,
        ))
        .children((!hint.copied).then(|| {
            div()
                .relative()
                .child(chip(
                    "add-to-doc-hint",
                    "Add to doc ▾".into(),
                    "Add the selected text to a document",
                    on_add,
                    cx,
                ))
                .children(menu)
        }));
    deferred(
        anchored()
            .position(hint.at + point(px(8.), px(12.)))
            .snap_to_window_with_margin(px(8.))
            .child(bar),
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
