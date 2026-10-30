//! Keyboard scrolling for the panels that scroll a plain element (plugin
//! modals, the approval preview): `Up`/`Down` by a line, `PageUp`/
//! `PageDown` by most of a page, `Home`/`End` to the ends.
//!
//! And the selectable lists (pickers, sidebars, outlines): [`list`] lays
//! the rows out in a scrolled column with a scrollbar, and
//! [`ensure_visible`] keeps the selected row in view as the keys move it.

use gpui::{
    AnyElement, Div, ElementId, InteractiveElement as _, ListOffset, ListState, ParentElement as _,
    Pixels, ScrollHandle, Stateful, StatefulInteractiveElement as _, Styled as _, div, point, px,
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

/// [`scroll_key`] for a virtualized `gpui::list` (a scrollable
/// `TextView`): the same keys move `list` by a line, a page or to an end.
pub fn scroll_list_key(list: &ListState, key: &str) -> bool {
    let page = list.viewport_bounds().size.height * PAGE;
    match key {
        "up" => list.scroll_by(-px(LINE)),
        "down" => list.scroll_by(px(LINE)),
        "pageup" => list.scroll_by(-page),
        "pagedown" => list.scroll_by(page),
        "home" => list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        }),
        "end" => list.scroll_to_end(),
        _ => return false,
    }
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

/// Whether the [`list`] of `handle` has been laid out once. Before that
/// GPUI drops an [`ensure_visible`] request (it does not know yet that the
/// element scrolls): a list that opens on a row asks again next frame.
pub fn laid_out(handle: &ScrollHandle) -> bool {
    handle.bounds().size.height > px(0.)
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
    fn list_keys_are_the_same_keys() {
        let list = ListState::new(3, gpui::ListAlignment::Top, px(0.));
        assert!(!scroll_list_key(&list, "x"));
        for key in ["up", "down", "pageup", "pagedown", "home", "end"] {
            assert!(scroll_list_key(&list, key), "{key}");
        }
        assert_eq!(
            list.logical_scroll_top().item_ix,
            3,
            "end is past the last item"
        );
        scroll_list_key(&list, "home");
        assert_eq!(list.logical_scroll_top().item_ix, 0);
    }

    #[test]
    fn unknown_keys_do_nothing() {
        let handle = ScrollHandle::new();
        assert!(!scroll_key(&handle, "x"));
        assert!(scroll_key(&handle, "down"));
        // Nothing to scroll: it stays at the top.
        assert_eq!(handle.offset().y, px(0.));
    }
}
