use domain::widget::timeline::{TimeUnit, TimelineRow};
use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point, size};
use serde_json::{Map, json};

use super::*;

fn span(start: f64, end: f64, label: &str) -> Span {
    Span {
        start,
        end,
        label: Some(label.into()),
        color_key: None,
        status: None,
        attrs: Map::new(),
    }
}

/// A trace-like spec: a root over 0..100 and `n - 1` children of 10 ms
/// each; the first child failed and has attributes.
fn spec(n: usize) -> TimelineSpec {
    let mut rows = vec![TimelineRow {
        id: "root".into(),
        label: "GET /orders".into(),
        depth: 0,
        group: None,
        spans: vec![span(0.0, 100.0, "GET /orders")],
        markers: Vec::new(),
    }];
    for ix in 1..n {
        let start = (ix % 9) as f64 * 10.0;
        let mut child = span(start, start + 10.0, &format!("child {ix}"));
        if ix == 1 {
            child.status = Some(SpanStatus::Error);
            child.attrs = json!({ "db.rows": 3, "db.statement": "SELECT 1" })
                .as_object()
                .cloned()
                .unwrap_or_default();
        }
        rows.push(TimelineRow {
            id: format!("c{ix}"),
            label: format!("child {ix}"),
            depth: 1,
            group: None,
            spans: vec![child],
            markers: Vec::new(),
        });
    }
    TimelineSpec {
        title: Some("trace".into()),
        unit: TimeUnit::Ms,
        rows,
        range: None,
    }
}

/// A window whose root draws the timeline of `id` for the spec it holds.
struct Host {
    id: BlockId,
    spec: TimelineSpec,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(render(self.id, &self.spec, window, cx))
    }
}

fn host(
    cx: &mut TestAppContext,
    spec: TimelineSpec,
) -> (gpui::Entity<Host>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let id = BlockId::new();
    cx.add_window_view(|_, _| Host { id, spec })
}

/// Draws the host with `spec` and returns the timeline entity.
fn draw(
    cx: &mut VisualTestContext,
    host: &gpui::Entity<Host>,
    spec: &TimelineSpec,
) -> gpui::Entity<TimelineView> {
    host.update(cx, |host, cx| {
        host.spec = spec.clone();
        cx.notify();
    });
    let root = host.clone();
    cx.draw(
        point(px(0.), px(0.)),
        size(px(800.), px(600.)),
        move |_, _| div().size_full().child(root.clone()),
    );
    let id = host.read_with(cx, |host, _| host.id);
    cx.update(|window, cx| view(id, spec, window, cx))
}

/// Clicks the element with debug selector `selector`.
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no {selector}"))
        .center();
    cx.simulate_click(at, Modifiers::none());
}

/// Window point over the middle of span `span` of row `row` once drawn.
fn over(
    view: &gpui::Entity<TimelineView>,
    cx: &mut VisualTestContext,
    row: usize,
    time: f64,
) -> Point<Pixels> {
    view.read_with(cx, |v, _| {
        let bounds = v.bounds.get();
        let width = f32::from(bounds.size.width);
        let left = gutter(width);
        let x = left + v.viewport.x_of(time, width - left);
        let y = AXIS_HEIGHT + row as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0 - v.scroll;
        bounds.origin + point(px(x), px(y))
    })
}

#[gpui::test]
fn draws_thousands_of_rows_and_scrolls_them(cx: &mut TestAppContext) {
    let big = spec(5_000);
    let (host, cx) = host(cx, big.clone());
    let view = draw(cx, &host, &big);
    let height = view.read_with(cx, |v, _| f32::from(v.bounds.get().size.height));
    assert_eq!(height, AXIS_HEIGHT + MAX_BODY_HEIGHT);
    let center = view.read_with(cx, |v, _| v.bounds.get().center());
    cx.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    assert_eq!(view.read_with(cx, |v, _| v.scroll()), 200.0);
    // Scrolling up past the top leaves the wheel to the chat.
    cx.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Pixels(point(px(0.), px(1_000.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    assert_eq!(view.read_with(cx, |v, _| v.scroll()), 0.0);
    draw(cx, &host, &big);
}

#[gpui::test]
fn ctrl_wheel_and_buttons_zoom_and_drag_pans(cx: &mut TestAppContext) {
    let spec = spec(10);
    let (host, cx) = host(cx, spec.clone());
    let view = draw(cx, &host, &spec);
    let at = over(&view, cx, 0, 25.0);
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Lines(point(0.0, 1.0)),
        modifiers: Modifiers::control(),
        touch_phase: TouchPhase::Moved,
    });
    let (zoom, viewport) = view.read_with(cx, |v, _| (v.zoom(), v.viewport()));
    assert!((zoom - ZOOM_STEP).abs() < 1e-6, "zoom {zoom}");
    // The time under the cursor stays put.
    let after = over(&view, cx, 0, 25.0);
    assert!((f32::from(after.x - at.x)).abs() < 0.5, "{viewport:?}");

    draw(cx, &host, &spec);
    let start = over(&view, cx, 0, 50.0);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    let moved = point(start.x + px(60.), start.y);
    cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
    let panned = view.read_with(cx, |v, _| v.viewport());
    assert!(panned.start < viewport.start, "{panned:?} vs {viewport:?}");
    assert!(
        view.read_with(cx, |v, _| v.selected()).is_none(),
        "a drag is not a click"
    );

    draw(cx, &host, &spec);
    click(cx, "timeline-fit");
    assert_eq!(view.read_with(cx, |v, _| v.zoom()), 1.0);
    draw(cx, &host, &spec);
    click(cx, "timeline-zoom-in");
    assert!((view.read_with(cx, |v, _| v.zoom()) - ZOOM_STEP).abs() < 1e-6);
}

#[gpui::test]
fn hover_shows_a_tooltip_and_click_selects(cx: &mut TestAppContext) {
    let spec = spec(10);
    let (host, cx) = host(cx, spec.clone());
    let view = draw(cx, &host, &spec);
    let selections = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = selections.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &TimelineEvent, _| {
            seen.borrow_mut().push(*event)
        })
    });

    // Child 1 spans 10..20 ms on row 1.
    let target = over(&view, cx, 1, 15.0);
    cx.simulate_mouse_move(target, None, Modifiers::none());
    draw(cx, &host, &spec);
    assert!(
        cx.debug_bounds("timeline-tooltip").is_some(),
        "tooltip shown"
    );

    cx.simulate_click(target, Modifiers::none());
    let selected = view.read_with(cx, |v, _| v.selected());
    assert_eq!(selected, Some(SpanRef { row: 1, span: 0 }));
    assert_eq!(*selections.borrow(), [TimelineEvent::Selected(selected)]);
    draw(cx, &host, &spec);
    assert!(cx.debug_bounds("timeline-details").is_some());

    // The root bar spans the whole row: a click on row 0 selects it.
    let root = over(&view, cx, 0, 95.0);
    cx.simulate_click(root, Modifiers::none());
    assert_eq!(
        view.read_with(cx, |v, _| v.selected()),
        Some(SpanRef { row: 0, span: 0 })
    );

    draw(cx, &host, &spec);
    click(cx, "timeline-clear");
    assert_eq!(view.read_with(cx, |v, _| v.selected()), None);
}

#[gpui::test]
fn a_new_spec_resets_the_view(cx: &mut TestAppContext) {
    let first = spec(4);
    let (host, cx) = host(cx, first.clone());
    let view = draw(cx, &host, &first);
    view.update(cx, |v, cx| v.zoom_by(4.0, None, cx));
    let mut second = spec(4);
    second.rows[0].spans[0].end = 200.0;
    draw(cx, &host, &second);
    assert_eq!(
        view.read_with(cx, |v, _| (v.zoom(), v.viewport().end)),
        (1.0, 200.0)
    );
    let empty = TimelineSpec {
        rows: Vec::new(),
        ..second
    };
    draw(cx, &host, &empty);
}
