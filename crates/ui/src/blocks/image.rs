//! Renderer of the `image` block kind; placeholder until its task lands.

use gpui::{AnyElement, App, Window};

use crate::timeline::TimelineEntry;

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    super::placeholder(entry, cx)
}
