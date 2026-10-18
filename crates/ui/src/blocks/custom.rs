//! Renderer of the `custom` block kind; placeholder until its task lands.

use gpui::{AnyElement, App};

use crate::timeline::TimelineEntry;

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, cx: &App) -> AnyElement {
    super::placeholder(entry, cx)
}
