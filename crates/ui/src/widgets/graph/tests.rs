use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point, size};
use serde_json::json;

use super::*;

/// A small service graph: a gateway calling three services, two of them
/// sharing a database; `kind` and `group` set on some nodes.
fn spec() -> GraphSpec {
    serde_json::from_value(json!({
        "title": "services",
        "nodes": [
            { "id": "gw", "label": "gateway", "kind": "edge" },
            { "id": "users", "group": "core", "attrs": { "port": 8080 } },
            { "id": "orders", "group": "core" },
            { "id": "search" },
            { "id": "db", "kind": "store" },
            { "id": "lonely" }
        ],
        "edges": [
            { "from": "gw", "to": "users", "label": "REST" },
            { "from": "gw", "to": "orders" },
            { "from": "gw", "to": "search" },
            { "from": "users", "to": "db", "weight": 3 },
            { "from": "orders", "to": "db", "weight": 1 }
        ],
        "groups": [{ "id": "core", "label": "Core" }]
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

/// A window whose root draws the graph of `id` for the spec it holds.
struct Host {
    id: BlockId,
    spec: GraphSpec,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(render(self.id, &self.spec, window, cx))
    }
}

fn host(cx: &mut TestAppContext, spec: GraphSpec) -> (gpui::Entity<Host>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let id = BlockId::new();
    cx.add_window_view(|_, _| Host { id, spec })
}

/// Draws the host until the layout is in and painted; returns the graph.
fn draw(cx: &mut VisualTestContext, host: &gpui::Entity<Host>) -> gpui::Entity<GraphView> {
    let frame = |cx: &mut VisualTestContext| {
        let root = host.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(800.), px(600.)),
            move |_, _| div().size_full().child(root.clone()),
        );
        cx.run_until_parked();
    };
    frame(cx);
    frame(cx);
    let (id, spec) = host.read_with(cx, |host, _| (host.id, host.spec.clone()));
    cx.update(|window, cx| view(id, &spec, window, cx))
}

/// Window point over the centre of node `ix`.
fn over(view: &gpui::Entity<GraphView>, cx: &mut VisualTestContext, ix: usize) -> Point<Pixels> {
    view.read_with(cx, |v, _| {
        let rect = v
            .geometry()
            .map(|g| g.nodes[ix])
            .unwrap_or_else(|| panic!("not laid out"));
        v.to_window(rect.centre())
    })
}

#[gpui::test]
fn lays_out_off_thread_and_fits_the_canvas(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, spec());
    let view = draw(cx, &host);
    let (nodes, groups, scale, crossings) = view.read_with(cx, |v, _| {
        let g = v.geometry().unwrap_or_else(|| panic!("no layout"));
        (g.nodes.len(), g.groups.len(), v.scale(), g.crossings)
    });
    assert_eq!((nodes, groups), (6, 1));
    assert!(scale > 0.0 && scale <= 1.0, "{scale}");
    assert_eq!(crossings.map(|c| c.1), Some(0));
    assert!(cx.debug_bounds("graph-message").is_none());
    // Every node is inside the canvas once fitted.
    let canvas = cx
        .debug_bounds("graph-canvas")
        .unwrap_or_else(|| panic!("no canvas"));
    for ix in 0..6 {
        let at = over(&view, cx, ix);
        assert!(
            canvas.contains(&at),
            "node {ix} at {at:?} outside {canvas:?}"
        );
    }
}

#[gpui::test]
fn zoom_and_drag_move_the_view(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, spec());
    let view = draw(cx, &host);
    let fitted = view.read_with(cx, |v, _| v.scale());
    let click = |cx: &mut VisualTestContext, selector: &'static str| {
        let at = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no {selector}"))
            .center();
        cx.simulate_click(at, Modifiers::none());
    };
    click(cx, "graph-zoom-in");
    let zoomed = view.read_with(cx, |v, _| v.scale());
    assert!(
        (zoomed / fitted - ZOOM_STEP).abs() < 1e-3,
        "{fitted} → {zoomed}"
    );
    // Ctrl + wheel zooms around the cursor: the node under it stays.
    let at = over(&view, cx, 0);
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(40.))),
        modifiers: Modifiers::control(),
        touch_phase: TouchPhase::Moved,
    });
    let (scale, after) = (view.read_with(cx, |v, _| v.scale()), over(&view, cx, 0));
    assert!(scale > zoomed);
    assert!((f32::from(after.x - at.x)).abs() < 0.5 && (f32::from(after.y - at.y)).abs() < 0.5);
    // A drag pans.
    let start = cx
        .debug_bounds("graph-canvas")
        .unwrap_or_else(|| panic!("no canvas"))
        .center();
    let before = over(&view, cx, 0);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        start + point(px(30.), px(10.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        start + point(px(30.), px(10.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let moved = over(&view, cx, 0);
    assert!(
        (f32::from(moved.x - before.x) - 30.0).abs() < 0.5,
        "{before:?} → {moved:?}"
    );
    click(cx, "graph-fit");
    assert!((view.read_with(cx, |v, _| v.scale()) - fitted).abs() < 1e-3);
}

#[gpui::test]
fn a_click_focuses_the_neighbourhood(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, spec());
    let view = draw(cx, &host);
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &GraphEvent, _| {
            seen.borrow_mut().push(*event);
        })
    });
    let users = over(&view, cx, 1);
    cx.simulate_click(users, Modifiers::none());
    let near = view.read_with(cx, |v, _| v.neighbourhood());
    assert_eq!(near, Some(BTreeSet::from([0, 1, 4])));
    draw(cx, &host);
    assert!(cx.debug_bounds("graph-details").is_some());
    // A second click on it clears the focus; Escape does too.
    cx.simulate_click(users, Modifiers::none());
    assert_eq!(view.read_with(cx, |v, _| v.focus()), None);
    let db = over(&view, cx, 4);
    cx.simulate_click(db, Modifiers::none());
    assert_eq!(view.read_with(cx, |v, _| v.focus()), Some(4));
    cx.simulate_keystrokes("escape");
    assert_eq!(view.read_with(cx, |v, _| v.focus()), None);
    assert_eq!(
        *events.borrow(),
        [
            GraphEvent::Focused(Some(1)),
            GraphEvent::Focused(None),
            GraphEvent::Focused(Some(4)),
            GraphEvent::Focused(None)
        ]
    );
}

#[gpui::test]
fn graphs_past_the_limit_say_so(cx: &mut TestAppContext) {
    let nodes: Vec<_> = (0..=DRAW_LIMIT)
        .map(|i| json!({ "id": format!("n{i}") }))
        .collect();
    let big: GraphSpec =
        serde_json::from_value(json!({ "nodes": nodes })).unwrap_or_else(|e| panic!("{e}"));
    let (host, cx) = host(cx, big);
    let view = draw(cx, &host);
    assert!(matches!(
        view.read_with(cx, |v, _| v.state().clone()),
        LayoutState::Failed(reason) if reason.contains("too many")
    ));
    assert!(cx.debug_bounds("graph-message").is_some());
}

#[gpui::test]
fn a_new_spec_lays_out_again(cx: &mut TestAppContext) {
    let (host, cx) = host(cx, spec());
    let view = draw(cx, &host);
    let users = over(&view, cx, 1);
    cx.simulate_click(users, Modifiers::none());
    let mut next = spec();
    next.layout = domain::widget::graph::LayoutKind::Grid;
    host.update(cx, |host, cx| {
        host.spec = next;
        cx.notify();
    });
    let view = draw(cx, &host);
    view.read_with(cx, |v, _| {
        assert_eq!(v.focus(), None);
        assert!(v.geometry().is_some_and(|g| g.crossings.is_none()));
    });
}
