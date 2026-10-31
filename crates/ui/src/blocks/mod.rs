//! Block registry: one module per rich block kind.
//!
//! Each module exposes `render(entry, window, cx)`. The timeline dispatches
//! here for every kind it does not draw itself, so the wave-3 block tasks
//! work in separate files. Renderers get the window and a mutable `App`
//! because some keep GPUI state per block (a table's `TableView`, a
//! diagram's pan and zoom) that is created and updated while drawing.

pub mod ansi;
pub mod arrays;
pub mod bench;
pub mod cad;
pub mod chart;
pub mod computed;
pub mod copy;
pub mod custom;
pub mod dataset;
pub mod diagram;
pub mod diff;
pub mod drawing;
pub mod findings;
pub mod flame;
pub mod form;
pub mod gantt;
pub mod geomap;
pub mod ghs;
pub mod har;
pub mod ifc;
pub mod image;
pub mod image_view;
pub mod json;
pub mod logs;
pub mod markdown;
pub mod material;
pub mod math;
pub mod measured;
pub mod mermaid;
pub mod model_view;
pub mod molecule;
pub mod molecule3d;
pub mod molecule_grid;
pub mod netlist;
pub mod network;
pub mod plate;
pub mod plot;
pub mod query_plan;
pub mod reaction;
pub mod sequence;
pub mod shell;
pub mod sources;
pub mod spectrum;
pub mod stage;
pub mod svg;
pub mod table;
pub mod terraform;
pub mod test_report;
pub mod thinking;
pub mod trace;
pub mod training;
pub mod waveform;

use std::any::TypeId;
use std::collections::HashMap;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use gpui::{
    AnyElement, AnyEntity, App, AppContext as _, Context, Entity, FocusHandle, Global,
    IntoElement as _, ParentElement as _, Styled as _, WeakFocusHandle, Window, div,
};
use gpui_component::ActiveTheme as _;

use crate::timeline::kind_name;
use crate::timeline::{NO_REV, TimelineEntry};

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
        // Phase 1 chat blocks.
        BlockKind::Form(_) => form::render(entry, window, cx),
        BlockKind::Plot(_) => plot::render(entry, window, cx),
        BlockKind::TestReport(_) => test_report::render(entry, window, cx),
        BlockKind::Findings(_) => findings::render(entry, window, cx),
        BlockKind::Dataset(_) => dataset::render(entry, window, cx),
        BlockKind::Molecule(_) => molecule::render(entry, window, cx),
        BlockKind::Plate(_) => plate::render(entry, window, cx),
        // Phase 2 developer/ops viewers.
        BlockKind::Trace(_) => trace::render(entry, window, cx),
        BlockKind::FlameGraph(_) => flame::render(entry, window, cx),
        BlockKind::Logs(_) => logs::render(entry, window, cx),
        BlockKind::QueryPlan(_) => query_plan::render(entry, window, cx),
        BlockKind::Har(_) => har::render(entry, window, cx),
        BlockKind::TerraformPlan(_) => terraform::render(entry, window, cx),
        BlockKind::Diagram(_) => diagram::render(entry, window, cx),
        BlockKind::Benchmarks(_) => bench::render(entry, window, cx),
        BlockKind::Waveform(_) => waveform::render(entry, window, cx),
        BlockKind::Netlist(_) => netlist::render(entry, window, cx),
        BlockKind::Material(_) => material::render(entry, window, cx),
        BlockKind::CadModel(_) => cad::render(entry, window, cx),
        BlockKind::Drawing(_) => drawing::render(entry, window, cx),
        BlockKind::IfcSchedule(_) => ifc::render(entry, window, cx),
        BlockKind::Sequence(_) => sequence::render(entry, window, cx),
        BlockKind::Reaction(_) => reaction::render(entry, window, cx),
        BlockKind::MoleculeGrid(_) => molecule_grid::render(entry, window, cx),
        BlockKind::Spectrum(_) => spectrum::render(entry, window, cx),
        BlockKind::Molecule3d(_) => molecule3d::render(entry, window, cx),
        BlockKind::Ghs(_) => ghs::render(entry, window, cx),
        BlockKind::GeoMap(_) => geomap::render(entry, window, cx),
        BlockKind::Gantt(_) => gantt::render(entry, window, cx),
        BlockKind::ArrayFile(_) => arrays::render(entry, window, cx),
        BlockKind::Network(_) => network::render(entry, window, cx),
        BlockKind::TrainingCurves(_) => training::render(entry, window, cx),
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
struct BlockStates(HashMap<(BlockId, TypeId), Slot>);

/// One kept entity and the revision of the content it was last synced
/// with ([`synced_state`]).
struct Slot {
    entity: AnyEntity,
    rev: u64,
}

impl Global for BlockStates {}

/// The `T` entity of block `id`, created with `init` on first use.
pub(crate) fn block_state<T: 'static>(
    id: BlockId,
    window: &mut Window,
    cx: &mut App,
    init: impl FnOnce(&mut Window, &mut Context<T>) -> T,
) -> Entity<T> {
    synced_state(id, NO_REV, window, cx, init, |_, _, _| {})
}

/// [`block_state`], brought up to date with `sync` when the content it
/// shows is at a revision it has not seen: `rev` is the drawn entry's
/// [`TimelineEntry::rev`] (or one derived from it), so a block that did not
/// change skips `sync` and the comparison of its whole spec on every frame.
/// [`NO_REV`] always syncs.
pub(crate) fn synced_state<T: 'static>(
    id: BlockId,
    rev: u64,
    window: &mut Window,
    cx: &mut App,
    init: impl FnOnce(&mut Window, &mut Context<T>) -> T,
    sync: impl FnOnce(&mut T, &mut Window, &mut Context<T>),
) -> Entity<T> {
    let key = (id, TypeId::of::<T>());
    let existing = cx
        .default_global::<BlockStates>()
        .0
        .get(&key)
        .map(|slot| (slot.entity.clone(), slot.rev));
    if let Some((any, seen)) = existing
        && let Ok(entity) = any.downcast::<T>()
    {
        if rev == NO_REV || seen != rev {
            entity.update(cx, |state, cx| sync(state, window, cx));
            if let Some(slot) = cx.default_global::<BlockStates>().0.get_mut(&key) {
                slot.rev = rev;
            }
        }
        return entity;
    }
    // Made from the content being drawn: nothing to sync.
    let entity = cx.new(|cx| init(window, cx));
    cx.default_global::<BlockStates>().0.insert(
        key,
        Slot {
            entity: entity.clone().into_any(),
            rev,
        },
    );
    entity
}

/// A value worked out from the block `id` at revision `rev` (and `key`,
/// view state such as whether the block is expanded), kept until either
/// changes: a shell's styled output or a tool call's pretty input are
/// made once rather than on every frame. Values of one type are told
/// apart by `K`.
pub(crate) fn derived<K: PartialEq + 'static, T: 'static>(
    id: BlockId,
    rev: u64,
    key: K,
    window: &mut Window,
    cx: &mut App,
    make: impl FnOnce(&K) -> T,
) -> Rc<T> {
    let state = synced_state(
        id,
        rev,
        window,
        cx,
        |_, _| Derived::<K, T>(None),
        |derived, _, _| derived.0 = None,
    );
    state.update(cx, |derived, _| match &derived.0 {
        Some((made_for, value)) if *made_for == key => value.clone(),
        _ => {
            let value = Rc::new(make(&key));
            derived.0 = Some((key, value.clone()));
            value
        }
    })
}

/// What [`derived`] keeps: the key and the value made for it.
struct Derived<K, T>(Option<(K, Rc<T>)>);

/// The `T` entity of block `id`, if one was created.
pub(crate) fn existing_block_state<T: 'static>(id: BlockId, cx: &mut App) -> Option<Entity<T>> {
    let key = (id, TypeId::of::<T>());
    cx.default_global::<BlockStates>()
        .0
        .get(&key)
        .and_then(|slot| slot.entity.clone().downcast::<T>().ok())
}

/// Focus handles of the block widgets with keys of their own, by block.
#[derive(Default)]
struct BlockFocus(HashMap<BlockId, WeakFocusHandle>);

impl Global for BlockFocus {}

/// Remembers `handle` as the keyboard of the widget of block `id`, for
/// `block.focus_widget` ([`block_focus`]).
pub(crate) fn remember_focus(id: BlockId, handle: &FocusHandle, cx: &mut App) {
    let known = cx.default_global::<BlockFocus>();
    if known.0.get(&id).and_then(WeakFocusHandle::upgrade).as_ref() != Some(handle) {
        known.0.insert(id, handle.downgrade());
    }
}

/// The focus handle of the widget of block `id`, if it has keys of its own
/// and was drawn.
pub fn block_focus(id: BlockId, cx: &App) -> Option<FocusHandle> {
    cx.try_global::<BlockFocus>()?
        .0
        .get(&id)
        .and_then(WeakFocusHandle::upgrade)
}

/// A host that counts the wheel events its child lets through: a block
/// that zooms only with a modifier must leave the plain wheel to the chat.
#[cfg(test)]
pub(crate) mod wheel_host {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, Entity, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
        Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled as _, TestAppContext,
        TouchPhase, VisualTestContext, Window, div, point,
    };

    /// `child`, inside an element that counts the wheel events.
    pub struct WheelHost<V> {
        pub child: Entity<V>,
        pub wheels: Rc<Cell<usize>>,
    }

    impl<V: Render> Render for WheelHost<V> {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let wheels = self.wheels.clone();
            div()
                .size_full()
                .on_scroll_wheel(move |_, _, _| wheels.set(wheels.get() + 1))
                .child(self.child.clone())
        }
    }

    /// A window with `new` inside a [`WheelHost`]; returns the child and
    /// the count of wheel events that reached the host.
    pub fn host<V: Render + 'static>(
        cx: &mut TestAppContext,
        new: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    ) -> (Entity<V>, Rc<Cell<usize>>, &mut VisualTestContext) {
        use gpui::AppContext as _;
        let wheels = Rc::new(Cell::new(0));
        let seen = wheels.clone();
        let (host, cx) = cx.add_window_view(move |window, cx| WheelHost {
            child: cx.new(|cx| new(window, cx)),
            wheels: seen,
        });
        let child = host.read_with(cx, |host, _| host.child.clone());
        (child, wheels, cx)
    }

    /// One notch of the wheel at `at` with `modifiers`.
    pub fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, modifiers: Modifiers) {
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers,
            touch_phase: TouchPhase::Moved,
        });
    }
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

    #[gpui::test]
    fn derived_values_follow_the_revision_and_the_key(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let id = BlockId::new();
        let made = std::cell::Cell::new(0);
        let get = |rev: u64, key: bool, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                derived(id, rev, key, window, cx, |key| {
                    made.set(made.get() + 1);
                    (rev, *key)
                })
            })
        };
        let first = get(3, false, cx);
        assert!(Rc::ptr_eq(&first, &get(3, false, cx)), "kept");
        assert_eq!(*get(3, true, cx), (3, true), "another key");
        assert_eq!(*get(4, true, cx), (4, true), "another revision");
        assert_eq!(made.get(), 3);
    }

    #[gpui::test]
    fn synced_state_syncs_on_new_revisions(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let id = BlockId::new();
        let sync = |rev: u64, cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                synced_state(id, rev, window, cx, |_, _| Counter(0), |c, _, _| c.0 += 1)
            })
        };
        let state = sync(5, cx);
        sync(5, cx);
        assert_eq!(state.read_with(cx, |c, _| c.0), 0, "made at 5: in sync");
        sync(6, cx);
        sync(6, cx);
        assert_eq!(state.read_with(cx, |c, _| c.0), 1);
        sync(NO_REV, cx);
        sync(NO_REV, cx);
        assert_eq!(state.read_with(cx, |c, _| c.0), 3, "no revision: always");
    }
}
