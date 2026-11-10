//! The floating layer of the prompt: whatever popup shows (the mode menu,
//! the `/` palette, the `@`, `/model ` and `/theme ` lists, the which-key
//! hint) is drawn in one overlay anchored to the top of the input and
//! growing upward. The overlay is out of the layout, so it adds no height
//! to the prompt area and never pushes the chat; the input comes after the
//! timeline in the tree, so it paints over it.
//!
//! It is as tall as its popups, at most [`MAX_SHARE`] of the window: in a
//! very short window the rows above that are cut off (`Up`/`Down` still
//! reach them: the list scrolls with the choice).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, Bounds, Div, InteractiveElement as _, ParentElement as _, Pixels, Point, Stateful,
    Styled as _, canvas, px,
};
use gpui_component::v_flex;

/// The most of the window's height the overlay takes.
const MAX_SHARE: f32 = 0.5;

/// Where the overlay was last drawn. It floats outside the input's own
/// bounds, so a click on it is not "outside" the input.
#[derive(Debug, Default)]
pub(super) struct Float(Rc<Cell<Bounds<Pixels>>>);

impl Float {
    /// An invisible element over the overlay that notes where it is drawn.
    fn measure(&self) -> impl gpui::IntoElement {
        let area = self.0.clone();
        canvas(move |bounds, _, _| area.set(bounds), |_, (), _, _| {})
            .absolute()
            .inset_0()
    }

    /// Whether `at` is on the overlay.
    pub fn holds(&self, at: Point<Pixels>) -> bool {
        self.0.get().contains(&at)
    }

    /// The overlay for `popups` (none: no overlay, and nothing to hold),
    /// at most [`MAX_SHARE`] of a window `window_height` tall; `wide` for the
    /// list popups, which fill the input's width.
    pub fn layer(
        &self,
        popups: Vec<AnyElement>,
        wide: bool,
        window_height: Pixels,
    ) -> Option<Stateful<Div>> {
        if popups.is_empty() {
            self.0.take();
            return None;
        }
        let measure = self.measure();
        let room = px(f32::from(window_height) * MAX_SHARE);
        let mut layer = v_flex().id("input-popups").max_h(room).overflow_hidden();
        layer = layer.debug_selector(|| "input-popups".to_owned());
        let layer = layer
            .absolute()
            .bottom_full()
            .left_0()
            .children(popups)
            .child(measure);
        // A wide popup takes the input's width; the menu is as wide as its rows.
        Some(if wide { layer.right_0() } else { layer })
    }
}
