//! Keyboard scrolling for the panels that scroll a plain element (plugin
//! modals, the approval preview): `Up`/`Down` by a line, `PageUp`/
//! `PageDown` by most of a page, `Home`/`End` to the ends.

use gpui::{Pixels, ScrollHandle, point, px};

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
