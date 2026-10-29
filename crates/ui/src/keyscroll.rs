//! Keyboard scrolling for the panels that scroll a plain element (plugin
//! modals, the approval preview): `Up`/`Down` by a line, `PageUp`/
//! `PageDown` by most of a page, `Home`/`End` to the ends.
//!
//! And the selectable lists (pickers, sidebars, outlines): [`list`] lays
//! the rows out in a scrolled column with a scrollbar, and
//! [`ensure_visible`] keeps the selected row in view as the keys move it.

use gpui::{
    AnyElement, Div, ElementId, InteractiveElement as _, ParentElement as _, Pixels, ScrollHandle,
    Stateful, StatefulInteractiveElement as _, Styled as _, div, point, px,
};
use gpui_component::scroll::{ScrollableElement as _, ScrollbarAxis};

/// Pixels a line of `Up`/`Down` scrolls.
const LINE: f32 = 40.0;
/// Share of the visible height a page scrolls.
const PAGE: f32 = 0.9;

/// Scrolls `handle` for the named key (`up`, `pagedown`...); returns
/// whether the key was one of them.
pub fn scroll_key(handle: &ScrollHandle, key: &str) -> bool {
    let page = handle.bounds().size.height * PAGE;
    let max = handle.max_offset().y;
    let target = match key {
        "up" => offset(handle) - px(LINE),
        "down" => offset(handle) + px(LINE),
        "pageup" => offset(handle) - page,
        "pagedown" => offset(handle) + page,
        "home" => px(0.),
        "end" => max,
        _ => return false,
    };
    // GPUI offsets grow negative as the content moves up.
    let clamped = target.clamp(px(0.), max.max(px(0.)));
    handle.set_offset(point(handle.offset().x, -clamped));
    true
}

/// A column of `rows` scrolled by `handle`, with a vertical scrollbar.
///
/// The rows are its direct children, so row `ix` is item `ix` of
/// `handle` ([`ensure_visible`]). `min_h_0` lets it shrink inside a flex
/// column instead of pushing its last rows out of the parent; give it
/// `flex_1` or a height where it sits.
pub fn list(
    id: impl Into<ElementId>,
    handle: &ScrollHandle,
    rows: impl IntoIterator<Item = AnyElement>,
) -> Stateful<Div> {
    div()
        .id(id)
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(handle)
        .children(rows)
        .scrollbar(handle, ScrollbarAxis::Vertical)
}

/// Scrolls the [`list`] of `handle` just enough, on its next layout, for
/// row `ix` to be in view.
pub fn ensure_visible(handle: &ScrollHandle, ix: usize) {
    handle.scroll_to_item(ix);
}

/// Whether row `ix` of the [`list`] of `handle` was laid out inside the
/// visible area (allowing a pixel of rounding). For tests.
pub fn is_visible(handle: &ScrollHandle, ix: usize) -> bool {
    let Some(row) = handle.bounds_for_item(ix) else {
        return false;
    };
    let view = handle.bounds();
    // Item bounds are where the rows were laid out, before the scroll
    // moves them.
    let offset = handle.offset().y;
    let (top, bottom) = (row.top() + offset, row.bottom() + offset);
    top >= view.top() - px(1.) && bottom <= view.bottom() + px(1.)
}

/// How far down the content is scrolled, as a positive distance.
fn offset(handle: &ScrollHandle) -> Pixels {
    -handle.offset().y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_do_nothing() {
        let handle = ScrollHandle::new();
        assert!(!scroll_key(&handle, "x"));
        assert!(scroll_key(&handle, "down"));
        // Nothing to scroll: it stays at the top.
        assert_eq!(handle.offset().y, px(0.));
    }
}
