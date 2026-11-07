//! What the chat's scroll position says about its blocks: which one is at
//! the top (the outline follows it as the person scrolls) and whether one
//! is already on screen (choosing it then needs no scroll).

use gpui::{ListState, Pixels, px};

/// How far below the top edge a block must start to still count as "at
/// the top": the block under this line is the current one.
pub const TOP_LINE: Pixels = px(24.);

/// The block at the top of the viewport: the last one that starts above
/// [`TOP_LINE`], or `None` for a chat that fits the viewport. At the very bottom it is the last block, even when that
/// one is too short to reach the line.
pub fn block_at_top(list: &ListState) -> Option<usize> {
    let len = list.item_count();
    if len == 0 {
        return None;
    }
    let view = list.viewport_bounds();
    // A chat that fits whole has nothing to scroll through.
    if let (Some(head), Some(foot)) = (list.bounds_for_item(0), list.bounds_for_item(len - 1))
        && foot.bottom() - head.top() <= view.size.height
    {
        return None;
    }
    // The last block laid out down to the viewport's foot is the end,
    // also when the list cannot tell (it has heights it never measured).
    let at_end = list.is_scrolled_to_end() == Some(true)
        || list
            .bounds_for_item(len - 1)
            .is_some_and(|bounds| bounds.bottom() <= view.bottom() + px(1.));
    if at_end {
        return Some(len - 1);
    }
    let line = view.top() + TOP_LINE;
    let first = list.logical_scroll_top().item_ix.min(len - 1);
    // A block not laid out yet is below the viewport: the one before it wins.
    let below = (first..len).find(|ix| {
        list.bounds_for_item(*ix)
            .is_none_or(|bounds| bounds.top() > line)
    });
    Some(match below {
        Some(ix) if ix > first => ix - 1,
        Some(ix) => ix,
        None => len - 1,
    })
}

/// Whether block `ix` is laid out whole inside the viewport.
pub fn is_on_screen(list: &ListState, ix: usize) -> bool {
    let view = list.viewport_bounds();
    list.bounds_for_item(ix)
        .is_some_and(|bounds| bounds.top() >= view.top() && bounds.bottom() <= view.bottom())
}
