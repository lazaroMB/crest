//! Block registry: one module per rich block kind.
//!
//! Each module exposes `render(entry, window, cx)`. The timeline dispatches
//! here for every kind it does not draw itself, so the wave-3 block tasks
//! work in separate files. Renderers get the window and a mutable `App`
//! because some keep GPUI state per block (a table's `TableState`, a
//! diagram's pan and zoom) that is created and updated while drawing.

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
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window, div};
use gpui_component::ActiveTheme as _;

use crate::timeline::TimelineEntry;
use crate::timeline::kind_name;

/// Draws a rich block, or `None` for kinds the timeline renders itself.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let element = match &entry.block.kind {
        BlockKind::Chart(_) => chart::render(entry, window, cx),
        BlockKind::Custom { .. } => custom::render(entry, window, cx),
        BlockKind::Diff(_) => diff::render(entry, window, cx),
        BlockKind::Image { .. } => image::render(entry, window, cx),
        BlockKind::Markdown { .. } => markdown::render(entry, window, cx),
        BlockKind::Mermaid(_) => mermaid::render(entry, window, cx),
        BlockKind::Sources { .. } => sources::render(entry, window, cx),
        BlockKind::StageHeader { .. } => stage::render(entry, window, cx),
        BlockKind::Table(_) => table::render(entry, window, cx),
        BlockKind::Thinking { .. } => thinking::render(entry, window, cx),
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
