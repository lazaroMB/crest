use gpui::{Modifiers, TestAppContext, VisualTestContext, point, size};
use serde_json::json;

use super::*;

/// A query plan: a join over two scans, one of them with a child.
fn plan() -> TreeSpec {
    serde_json::from_value(json!({
        "title": "plan",
        "columns": [
            { "name": "time", "kind": "duration" },
            { "name": "rows", "kind": "number" },
            { "name": "share", "kind": "bar", "unit": "%" }
        ],
        "nodes": [{
            "label": "Hash Join", "values": { "time": 12.5, "rows": 1000, "share": 100 },
            "children": [
                { "label": "Seq Scan users", "badge": "hot", "tone": "error",
                  "values": { "time": 9.75, "rows": 120000, "share": 50 },
                  "children": [{ "label": "Filter", "values": {} }] },
                { "label": "Index Scan orders", "values": { "time": 0.25, "rows": 3 } }
            ]
        }]
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn rows_carry_cells_and_bar_lengths() {
    let rows = rows(&plan());
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1].cells[0].as_ref(), "9.75 ms");
    assert_eq!(rows[1].bars, [None, None, Some(0.5)]);
    assert_eq!(rows[3].bars, [None, None, None]);
    assert!(rows[0].has_children(0) && rows[1].has_children(1) && !rows[2].has_children(2));
    let open = HashSet::from([0]);
    assert_eq!(visible(&rows, &open, MAX_ROWS), (vec![0, 1, 3], 0));
    assert_eq!(visible(&rows, &HashSet::from([0, 1]), 2), (vec![0, 1], 2));
    assert_eq!(visible(&rows, &HashSet::new(), MAX_ROWS), (vec![0], 0));
}

struct Host {
    id: BlockId,
    spec: TreeSpec,
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
) -> (
    gpui::Entity<Host>,
    gpui::Entity<TreeView>,
    &mut VisualTestContext,
) {
    cx.update(gpui_component::init);
    let id = BlockId::new();
    let (host, cx) = cx.add_window_view(|_, _| Host { id, spec: plan() });
    draw(cx, &host);
    let view = cx.update(|window, cx| view(id, &plan(), window, cx));
    (host, view, cx)
}

fn draw(cx: &mut VisualTestContext, host: &gpui::Entity<Host>) {
    let root = host.clone();
    cx.draw(
        point(px(0.), px(0.)),
        size(px(900.), px(600.)),
        move |_, _| div().size_full().child(root.clone()),
    );
    cx.run_until_parked();
}

/// Clicks the element `selector` after drawing the host.
fn click(cx: &mut VisualTestContext, host: &gpui::Entity<Host>, selector: &'static str) {
    draw(cx, host);
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no {selector}"))
        .center();
    cx.simulate_click(at, Modifiers::none());
}

#[gpui::test]
fn rows_open_close_and_expand_all(cx: &mut TestAppContext) {
    let (host, view, cx) = host(cx);
    // Two levels show at first: the join and its two scans.
    assert_eq!(view.read_with(cx, |v, _| v.visible()), [0, 1, 3]);
    assert!(cx.debug_bounds("tree-row-2").is_none());
    click(cx, &host, "tree-row-1");
    assert!(view.read_with(cx, |v, _| v.is_open(1)));
    draw(cx, &host);
    assert!(cx.debug_bounds("tree-row-2").is_some());
    // A leaf does not toggle.
    click(cx, &host, "tree-row-2");
    assert!(!view.read_with(cx, |v, _| v.is_open(2)));
    click(cx, &host, "tree-row-0");
    assert_eq!(view.read_with(cx, |v, _| v.visible()), [0]);
    click(cx, &host, "tree-expand-all");
    assert_eq!(view.read_with(cx, |v, _| v.visible()), [0, 1, 2, 3]);
    click(cx, &host, "tree-collapse-all");
    assert_eq!(view.read_with(cx, |v, _| v.visible()), [0]);
}

#[gpui::test]
fn a_new_spec_resets_the_open_rows(cx: &mut TestAppContext) {
    let (host, tree, cx) = host(cx);
    click(cx, &host, "tree-collapse-all");
    let mut next = plan();
    next.expand_depth = 3;
    host.update(cx, |host, cx| {
        host.spec = next.clone();
        cx.notify();
    });
    draw(cx, &host);
    let id = host.read_with(cx, |host, _| host.id);
    let view2 = cx.update(|window, cx| view(id, &next, window, cx));
    assert_eq!(view2, tree);
    assert_eq!(tree.read_with(cx, |v, _| v.visible()), [0, 1, 2, 3]);
}
