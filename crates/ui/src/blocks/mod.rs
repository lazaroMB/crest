//! Block registry: one module per rich block kind.
//!
//! Each module has a unit `View` implementing [`BlockView`] (how the kind
//! is drawn, how wide it is, the forms it copies in...) and is listed once
//! in [`VIEWS`]; the timeline looks a block's view up by its [`BlockTag`]
//! for every kind it does not draw itself (text, tool calls and errors,
//! [`TIMELINE_DRAWN`]). Renderers get the window and a mutable `App`
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
use std::sync::OnceLock;

use domain::block::{BlockId, BlockKind, BlockTag};
use gpui::{
    AnyElement, AnyEntity, App, AppContext as _, Context, Entity, FocusHandle, Global,
    IntoElement as _, ParentElement as _, Styled as _, WeakFocusHandle, Window, div,
};
use gpui_component::ActiveTheme as _;

use self::copy::CopyFormat;
use crate::timeline::{BlockWidth, NO_REV, TimelineEntry};

/// How the timeline draws one kind of block, and what it knows about it.
/// One unit struct per module implements it and is listed in [`VIEWS`];
/// everything but [`tag`](Self::tag) and [`render`](Self::render) has a
/// default.
pub trait BlockView: Sync + 'static {
    /// The kind this view draws.
    fn tag(&self) -> BlockTag;

    /// Draws `entry`, a block of this view's kind.
    fn render(&self, entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement;

    /// Which column width the kind uses; rich blocks are wide.
    fn width(&self) -> BlockWidth {
        BlockWidth::Wide
    }

    /// The forms `kind` (of this view's kind) can be copied in
    /// ([`copy::formats`]); none by default.
    fn copy_formats(&self, _kind: &BlockKind) -> Vec<CopyFormat> {
        Vec::new()
    }

    /// Whether the kind has an alternate view (a chart's table, a
    /// document's source) toggled with `t`.
    fn has_alt_view(&self) -> bool {
        false
    }

    /// Whether a new block of this kind starts expanded.
    fn expanded_by_default(&self, _kind: &BlockKind) -> bool {
        true
    }

    /// Whether the block draws its own copy buttons, so the timeline
    /// shows no copy row under it ([`copy::row`]).
    fn own_copy_buttons(&self) -> bool {
        false
    }
}

/// Every block view; a new kind adds its line here.
pub static VIEWS: &[&dyn BlockView] = &[
    &chart::View,
    &custom::View,
    &diff::View,
    &image::View,
    &markdown::View,
    &mermaid::View,
    &shell::View,
    &sources::View,
    &stage::View,
    &table::View,
    &thinking::View,
    &computed::View,
    // Phase 1 chat blocks.
    &form::View,
    &plot::View,
    &test_report::View,
    &findings::View,
    &dataset::View,
    &molecule::View,
    &plate::View,
    // Phase 2 developer/ops viewers.
    &trace::View,
    &flame::View,
    &logs::View,
    &query_plan::View,
    &har::View,
    &terraform::View,
    &diagram::View,
    &bench::View,
    &waveform::View,
    &netlist::View,
    &material::View,
    &cad::View,
    &drawing::View,
    &ifc::View,
    &sequence::View,
    &reaction::View,
    &molecule_grid::View,
    &spectrum::View,
    &molecule3d::View,
    &ghs::View,
    &geomap::View,
    &gantt::View,
    &arrays::View,
    &network::View,
    &training::View,
];

/// The kinds `timeline/render.rs` draws itself, so they have no view.
pub const TIMELINE_DRAWN: &[BlockTag] = &[BlockTag::Text, BlockTag::ToolCall, BlockTag::Error];

/// The view of the kind `tag`, or `None` for [`TIMELINE_DRAWN`] kinds.
pub fn view(tag: BlockTag) -> Option<&'static dyn BlockView> {
    // Built once: it is looked up for every block on every frame.
    static BY_TAG: OnceLock<HashMap<BlockTag, &'static dyn BlockView>> = OnceLock::new();
    BY_TAG
        .get_or_init(|| VIEWS.iter().map(|view| (view.tag(), *view)).collect())
        .get(&tag)
        .copied()
}

/// Draws a rich block, or `None` for kinds the timeline renders itself.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    view(entry.block.kind.tag()).map(|view| view.render(entry, window, cx))
}

/// The `[kind]` stand-in used until a block's renderer exists.
pub(crate) fn placeholder(entry: &TimelineEntry, cx: &App) -> AnyElement {
    div()
        .text_color(cx.theme().muted_foreground)
        .child(format!("[{}]", entry.block.kind.tag().name()))
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

    #[test]
    fn every_kind_has_one_view_or_is_drawn_by_the_timeline() {
        for &tag in BlockTag::ALL {
            let views = VIEWS.iter().filter(|view| view.tag() == tag).count();
            let drawn = TIMELINE_DRAWN.contains(&tag);
            assert_eq!(views + usize::from(drawn), 1, "{tag:?}");
        }
    }

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
