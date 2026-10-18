//! Block registry: one module per rich block kind.
//!
//! Each module exposes `render(entry, cx)`. The timeline dispatches here for
//! every kind it does not draw itself, so the wave-3 block tasks work in
//! separate files.

pub mod chart;
pub mod custom;
pub mod diff;
pub mod image;
pub mod markdown;
pub mod mermaid;
pub mod sources;
pub mod stage;
pub mod table;
pub mod thinking;

use domain::block::BlockKind;
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, div};
use gpui_component::ActiveTheme as _;

use crate::timeline::TimelineEntry;
use crate::timeline::kind_name;

/// Draws a rich block, or `None` for kinds the timeline renders itself.
pub fn render(entry: &TimelineEntry, cx: &App) -> Option<AnyElement> {
    let element = match &entry.block.kind {
        BlockKind::Chart(_) => chart::render(entry, cx),
        BlockKind::Custom { .. } => custom::render(entry, cx),
        BlockKind::Diff(_) => diff::render(entry, cx),
        BlockKind::Image { .. } => image::render(entry, cx),
        BlockKind::Markdown { .. } => markdown::render(entry, cx),
        BlockKind::Mermaid(_) => mermaid::render(entry, cx),
        BlockKind::Sources { .. } => sources::render(entry, cx),
        BlockKind::StageHeader { .. } => stage::render(entry, cx),
        BlockKind::Table(_) => table::render(entry, cx),
        BlockKind::Thinking { .. } => thinking::render(entry, cx),
        BlockKind::Text { .. } | BlockKind::ToolCall { .. } | BlockKind::Error { .. } => {
            return None;
        }
    };
    Some(element)
}

/// The `[kind]` stand-in used until a block's renderer exists.
pub(crate) fn placeholder(entry: &TimelineEntry, cx: &App) -> AnyElement {
    div()
        .text_color(cx.theme().muted_foreground)
        .child(format!("[{}]", kind_name(&entry.block.kind)))
        .into_any_element()
}
