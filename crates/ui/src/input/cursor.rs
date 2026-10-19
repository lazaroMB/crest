//! The terminal-style block cursor of the prompt.
//!
//! gpui-component's editor only draws a thin caret, so the prompt hides it
//! ([`HiddenCaret`]) and paints its own block over the character under the
//! cursor, inverted ([`paint_cursor`]), from the caret geometry the editor
//! computed in the same frame. [`Blink`] is the blinking: every
//! [`BLINK_INTERVAL`] it toggles, and it stays on while keys are typed.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, BorderStyle, Bounds, Element, ElementId, Entity, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, TextAlign, Window, fill,
    outline, px, size,
};
use gpui_base::input::{DiagnosticColors, InputEditorStyle};
use gpui_component::ActiveTheme as _;
use gpui_component::input::TextareaState;

/// Time between two blinks.
pub const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// The blink state of the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blink {
    visible: bool,
    last_key: Option<Instant>,
}

impl Default for Blink {
    fn default() -> Self {
        Self {
            visible: true,
            last_key: None,
        }
    }
}

impl Blink {
    /// Whether the cursor is drawn now.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// A key was typed: the cursor shows and stays for a whole interval.
    pub fn keyed(&mut self, now: Instant) {
        self.visible = true;
        self.last_key = Some(now);
    }

    /// The interval elapsed: toggles, unless a key was typed within it.
    /// Returns whether the cursor changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let typing = self
            .last_key
            .is_some_and(|at| now.saturating_duration_since(at) < BLINK_INTERVAL);
        if typing {
            let changed = !self.visible;
            self.visible = true;
            return changed;
        }
        self.visible = !self.visible;
        true
    }
}

/// How the cursor is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// A filled block with the character under it inverted.
    Block,
    /// A block outline.
    Hollow,
}

/// Paints the cursor of `editor` with `shape`: `color` for the block and
/// `inverted` for the character under it. Call it after the editor
/// prepainted, in the same frame.
pub fn paint_cursor(
    editor: &Entity<TextareaState>,
    shape: CursorShape,
    color: Hsla,
    inverted: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let state = editor.read(cx);
    let Some((caret, _)) = state.cursor_layout() else {
        return;
    };
    let text = state.value();
    let under = text
        .get(state.cursor()..)
        .and_then(|rest| rest.chars().next())
        .filter(|c| !c.is_control())
        .unwrap_or(' ');
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let glyph = SharedString::from(under.to_string());
    let mut run = style.to_run(glyph.len());
    run.color = inverted;
    let shaped = window
        .text_system()
        .shape_line(glyph, font_size, &[run], None);
    let width = if shaped.width > px(0.) {
        shaped.width
    } else {
        font_size * 0.6
    };
    let block = Bounds::new(caret.origin, size(width, caret.size.height));
    match shape {
        CursorShape::Block => {
            window.paint_quad(fill(block, color));
            shaped
                .paint(
                    block.origin,
                    block.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }
        CursorShape::Hollow => window.paint_quad(outline(block, color, BorderStyle::Solid)),
    }
}

/// Lays out and paints `child` (the prompt's `Textarea`) with the editor's
/// own caret made invisible: after the textarea projected the theme onto
/// its state, this projects the same colors with a transparent caret.
pub struct HiddenCaret {
    child: AnyElement,
    editor: Entity<TextareaState>,
}

impl HiddenCaret {
    /// Wraps the textarea of `editor`.
    pub fn new(child: impl IntoElement, editor: Entity<TextareaState>) -> Self {
        Self {
            child: child.into_any_element(),
            editor,
        }
    }
}

/// The editor style gpui-component's `Input` projects, without a caret.
/// Its alpha is not zero: a zero alpha means "unset" and would be filled
/// in from the palette.
fn style_without_caret(cx: &App) -> InputEditorStyle {
    let theme = cx.theme();
    let status = &theme.highlight_theme.style.status;
    InputEditorStyle {
        foreground: theme.foreground,
        muted_foreground: theme.muted_foreground,
        background: theme
            .highlight_theme
            .style
            .editor_background
            .unwrap_or_else(|| theme.input_background()),
        border: theme.border,
        selection: theme.selection,
        caret: theme.caret.opacity(0.001),
        diagnostics: DiagnosticColors {
            error: status.error(cx),
            warning: status.warning(cx),
            info: status.info(cx),
            hint: status.hint(cx),
        },
        highlight_styles: theme.highlight_theme.clone(),
        editor_invisible: theme.highlight_theme.style.editor_invisible,
        editor_active_line: theme.highlight_theme.style.editor_active_line,
        editor_gutter_background: theme.highlight_theme.style.editor_gutter_background,
        fold_icon_renderer: None,
    }
}

impl IntoElement for HiddenCaret {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for HiddenCaret {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout = self.child.request_layout(window, cx);
        let style = style_without_caret(cx);
        self.editor
            .update(cx, |editor, _| editor.set_editor_style(style));
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blinks_every_interval() {
        let start = Instant::now();
        let mut blink = Blink::default();
        assert!(blink.visible());
        assert!(blink.tick(start + BLINK_INTERVAL));
        assert!(!blink.visible());
        assert!(blink.tick(start + BLINK_INTERVAL * 2));
        assert!(blink.visible());
    }

    #[test]
    fn typing_keeps_it_on() {
        let start = Instant::now();
        let mut blink = Blink::default();
        blink.tick(start);
        assert!(!blink.visible());
        blink.keyed(start + Duration::from_millis(100));
        assert!(blink.visible());
        assert!(
            !blink.tick(start + Duration::from_millis(500)),
            "typed 400 ms ago"
        );
        assert!(blink.visible());
        assert!(blink.tick(start + Duration::from_millis(1000)));
        assert!(!blink.visible());
    }
}
