//! Block registry: one module per rich block kind.
//!
//! Each module exposes `render(entry, window, cx)`. The timeline dispatches
//! here for every kind it does not draw itself, so the wave-3 block tasks
//! work in separate files. Renderers get the window and a mutable `App`
//! because some keep GPUI state per block (a table's `TableView`, a
//! diagram's pan and zoom) that is created and updated while drawing.

pub mod ansi;
pub mod chart;
pub mod computed;
pub mod custom;
pub mod diff;
pub mod findings;
pub mod form;
pub mod image;
pub mod image_view;
pub mod json;
pub mod markdown;
pub mod math;
pub mod measured;
pub mod mermaid;
pub mod model_view;
pub mod plot;
pub mod shell;
pub mod sources;
pub mod stage;
pub mod svg;
pub mod table;
pub mod test_report;
pub mod thinking;

use std::any::TypeId;
use std::collections::HashMap;

use domain::block::{BlockId, BlockKind};
use gpui::{
    AnyElement, AnyEntity, App, AppContext as _, Context, Entity, Global, IntoElement as _,
    ParentElement as _, Styled as _, Window, div,
};
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
        BlockKind::Shell { .. } => shell::render(entry, window, cx),
        BlockKind::Sources { .. } => sources::render(entry, window, cx),
        BlockKind::StageHeader { .. } => stage::render(entry, window, cx),
        BlockKind::Table(_) => table::render(entry, window, cx),
        BlockKind::Thinking { .. } => thinking::render(entry, window, cx),
        BlockKind::Computed { .. } => computed::render(entry, window, cx),
        // Phase 1 chat blocks; each gets its own renderer in a later piece.
        BlockKind::Form(_) => form::render(entry, window, cx),
        BlockKind::Plot(_) => plot::render(entry, window, cx),
        BlockKind::TestReport(_) => test_report::render(entry, window, cx),
        BlockKind::Findings(_) => findings::render(entry, window, cx),
        BlockKind::Dataset(_) | BlockKind::Molecule(_) | BlockKind::Plate(_) => {
            placeholder(entry, cx)
        }
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

/// Entities that block renderers keep across frames, by block and type.
///
/// The timeline list drops off-screen rows, so state kept in element state
/// (`use_keyed_state`) would reset when a block scrolls away; this map
/// keeps it for the life of the app. Blocks are never removed from a
/// session, so neither are their entries.
#[derive(Default)]
struct BlockStates(HashMap<(BlockId, TypeId), AnyEntity>);

impl Global for BlockStates {}

/// The `T` entity of block `id`, created with `init` on first use.
pub(crate) fn block_state<T: 'static>(
    id: BlockId,
    window: &mut Window,
    cx: &mut App,
    init: impl FnOnce(&mut Window, &mut Context<T>) -> T,
) -> Entity<T> {
    let key = (id, TypeId::of::<T>());
    let existing = cx.default_global::<BlockStates>().0.get(&key).cloned();
    if let Some(entity) = existing.and_then(|any| any.downcast::<T>().ok()) {
        return entity;
    }
    let entity = cx.new(|cx| init(window, cx));
    cx.default_global::<BlockStates>()
        .0
        .insert(key, entity.clone().into_any());
    entity
}

/// The `T` entity of block `id`, if one was created.
pub(crate) fn existing_block_state<T: 'static>(id: BlockId, cx: &mut App) -> Option<Entity<T>> {
    let key = (id, TypeId::of::<T>());
    cx.default_global::<BlockStates>()
        .0
        .get(&key)
        .cloned()
        .and_then(|any| any.downcast::<T>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct Counter(u32);

    #[gpui::test]
    fn block_state_is_created_once_per_block(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let (a, b) = (BlockId::new(), BlockId::new());
        let first = cx.update(|window, cx| block_state(a, window, cx, |_, _| Counter(1)));
        first.update(cx, |counter, _| counter.0 = 7);
        let again = cx.update(|window, cx| block_state(a, window, cx, |_, _| Counter(1)));
        let other = cx.update(|window, cx| block_state(b, window, cx, |_, _| Counter(2)));
        assert_eq!(first, again);
        cx.update(|_, cx| {
            assert_eq!(again.read(cx).0, 7);
            assert_eq!(other.read(cx).0, 2);
        });
    }
}
