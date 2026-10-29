use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
use serde_json::json;

use super::*;

fn spec(n: usize, follow: bool) -> ListSpec {
    let mut items = vec![
        json!({ "level": "info", "ts": "12:00:00", "text": "started", "fields": { "service": "api" } }),
        json!({ "level": "error", "ts": "12:00:01", "text": "Connection refused",
                "fields": { "service": "db", "port": 5432 } }),
        json!({ "level": "warn", "ts": "12:00:02", "text": "slow query", "fields": { "service": "db" } }),
    ];
    for ix in 3..n {
        items.push(json!({ "level": "debug", "text": format!("tick {ix}") }));
    }
    serde_json::from_value(json!({
        "title": "logs",
        "columns": ["service"],
        "facets": ["service"],
        "follow": follow,
        "items": items,
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

struct Host {
    id: BlockId,
    spec: ListSpec,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(render(self.id, &self.spec, window, cx))
    }
}

fn host(cx: &mut TestAppContext, spec: ListSpec) -> (Entity<ListView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let id = BlockId::new();
    let lookup = spec.clone();
    let (_, cx) = cx.add_window_view(|_, _| Host { id, spec });
    draw(cx);
    let view = cx.update(|window, cx| view(id, &lookup, window, cx));
    (view, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = cx
        .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("no {selector}"))
        .center();
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn typing_filters_and_a_bad_regex_says_why(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(10, false));
    assert_eq!(view.read_with(cx, |v, _| v.visible().len()), 10);
    click(cx, "filter-list-input");
    assert!(cx.update(|window, cx| crate::widgets::owns_focus(window, cx)));
    cx.simulate_input("refused");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [1]);
    view.update(cx, |v, cx| v.set_query("/(tick/", cx));
    assert!(view.read_with(cx, |v, _| v.query_error().is_some()));
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [1], "kept");
    view.update(cx, |v, cx| v.set_query(r"/tick [5-6]$/", cx));
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [5, 6]);
    draw(cx);
    assert!(cx.debug_bounds("filter-list-item-5").is_some());
}

#[gpui::test]
fn chips_filter_by_level_and_field(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(6, false));
    click(cx, "filter-list-chip-level-error");
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [1]);
    click(cx, "filter-list-chip-level-warn");
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [1, 2]);
    click(cx, "filter-list-chip-service-api");
    assert_eq!(
        view.read_with(cx, |v, _| v.visible().to_vec()),
        Vec::<usize>::new()
    );
    click(cx, "filter-list-chip-level-error");
    click(cx, "filter-list-chip-level-warn");
    assert_eq!(view.read_with(cx, |v, _| v.visible().to_vec()), [0]);
}

#[gpui::test]
fn a_click_opens_a_row_to_its_fields_and_copy_takes_the_visible(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(5, false));
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _sub = cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &ListEvent, _| {
            seen.borrow_mut().push(*event)
        })
    });
    click(cx, "filter-list-item-1");
    assert!(view.read_with(cx, |v, _| v.is_open(1)));
    assert_eq!(*events.borrow(), [ListEvent::Selected(Some(1))]);
    draw(cx);
    assert!(
        cx.debug_bounds("filter-list-field-1-1").is_some(),
        "port shown"
    );
    click(cx, "filter-list-item-1");
    assert!(!view.read_with(cx, |v, _| v.is_open(1)));

    view.update(cx, |v, cx| v.set_query("db", cx));
    draw(cx);
    click(cx, "filter-list-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some(
            "12:00:01 ERROR Connection refused service=db port=5432\n12:00:02 WARN slow query service=db\n"
        )
    );
}

#[gpui::test]
fn follow_keeps_the_tail_in_view_of_a_long_list(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(100_000, true));
    draw(cx);
    let at_end = view.read_with(cx, |v, _| v.scroll_handle().is_scrolled_to_end());
    assert_eq!(at_end, Some(true));
    assert!(cx.debug_bounds("filter-list-item-99999").is_some());
    assert!(
        cx.debug_bounds("filter-list-item-0").is_none(),
        "virtualized"
    );
    click(cx, "filter-list-follow");
    assert!(!view.read_with(cx, |v, _| v.follow()));
}

#[test]
fn steps_stop_at_the_ends_and_restart_when_hidden() {
    let visible = [2, 5, 7];
    assert_eq!(step(&visible, None, Step::Next), Some(2));
    assert_eq!(step(&visible, None, Step::Prev), Some(7));
    assert_eq!(step(&visible, Some(5), Step::Next), Some(7));
    assert_eq!(step(&visible, Some(7), Step::Next), Some(7));
    assert_eq!(step(&visible, Some(2), Step::Prev), Some(2));
    assert_eq!(step(&visible, Some(3), Step::Next), Some(2), "hidden");
    assert_eq!(step(&visible, Some(5), Step::Last), Some(7));
    assert_eq!(step(&[], None, Step::Next), None);
}

#[gpui::test]
fn the_arrows_move_the_selection_in_view_and_enter_opens_it(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(200, false));
    click(cx, "filter-list-item-0");
    view.update(cx, |v, cx| v.toggle_item(0, cx));
    for _ in 0..30 {
        cx.simulate_keystrokes("down");
    }
    draw(cx);
    assert_eq!(view.read_with(cx, |v, _| v.selected()), Some(30));
    let body = cx.debug_bounds("filter-list-body").expect("body");
    let row = cx
        .debug_bounds("filter-list-item-30")
        .expect("the selected row is drawn");
    assert!(
        row.top() >= body.top() && row.bottom() <= body.bottom() + px(1.),
        "row {row:?} in {body:?}"
    );
    cx.simulate_keystrokes("enter");
    assert!(view.read_with(cx, |v, _| v.is_open(30)), "Enter opens it");
    cx.simulate_keystrokes("home");
    assert_eq!(view.read_with(cx, |v, _| v.selected()), Some(0));
}

#[gpui::test]
fn down_in_the_filter_goes_to_the_rows(cx: &mut TestAppContext) {
    let (view, cx) = host(cx, spec(10, false));
    click(cx, "filter-list-input");
    cx.simulate_keystrokes("down");
    assert_eq!(view.read_with(cx, |v, _| v.selected()), Some(0));
    cx.update(|window, cx| {
        assert!(view.read(cx).focus_handle().is_focused(window));
    });
    cx.simulate_keystrokes("down");
    assert_eq!(view.read_with(cx, |v, _| v.selected()), Some(1));
}
