use application::ports::events::DomainEvent;
use domain::block::{Cell, ChartKind, ChartSpec, Column, Series, TableSpec};
use gpui::{TestAppContext, px};

use super::*;
use crate::blocks::table::TableView;
use crate::timeline::TimelineView;

fn text(s: &str) -> CustomNode {
    CustomNode::Text { text: s.into() }
}

fn table() -> TableSpec {
    TableSpec {
        columns: vec![Column::new("tarea"), Column::new("hecha")],
        rows: vec![
            vec![Cell::Text("parser".into()), Cell::Bool(true)],
            vec![Cell::Text("docs".into()), Cell::Bool(false)],
        ],
    }
}

fn tree() -> CustomNode {
    CustomNode::Col {
        children: vec![
            text("Pendientes"),
            CustomNode::List {
                items: vec![text("a"), text("b")],
            },
            CustomNode::Table(table()),
            CustomNode::Row {
                children: vec![
                    CustomNode::Code {
                        lang: Some("rust".into()),
                        source: "fn main() {}".into(),
                    },
                    CustomNode::Markdown {
                        source: "**ok**".into(),
                    },
                ],
            },
            CustomNode::Chart(ChartSpec {
                title: "avance".into(),
                kind: ChartKind::Bar,
                labels: vec!["lun".into(), "mar".into()],
                series: vec![Series {
                    name: "hechas".into(),
                    values: vec![1.0, 3.0],
                }],
            }),
        ],
    }
}

#[test]
fn node_ids_are_stable_and_distinct() {
    let block = BlockId::new();
    assert_eq!(node_id(block, 0), node_id(block, 0));
    assert_ne!(node_id(block, 0), node_id(block, 1));
    assert_ne!(node_id(block, 0), block);
    assert_ne!(node_id(block, 0), node_id(BlockId::new(), 0));
}

#[test]
fn code_fences_outgrow_inner_backticks() {
    assert_eq!(fenced_code(Some("lua"), "x = 1"), "```lua\nx = 1\n```\n");
    assert_eq!(fenced_code(None, "a\n"), "```\na\n```\n");
    assert!(fenced_code(None, "````").starts_with("`````\n"));
}

/// Draws `blocks` in a timeline.
fn draw(cx: &mut TestAppContext, blocks: Vec<Block>) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let count = blocks.len();
    view.update(cx, |view, cx| {
        view.apply_all(
            blocks
                .into_iter()
                .map(|block| DomainEvent::BlockAdded { block }),
            cx,
        );
    });
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(800.), px(1200.)),
        |_, _| div().size_full().child(view.clone()),
    );
    view.read_with(cx, |view, _| assert_eq!(view.state().len(), count));
}

fn custom(tree: CustomNode) -> Block {
    Block::new(BlockKind::Custom {
        plugin: "todo".into(),
        name: "todo".into(),
        tree,
    })
}

#[gpui::test]
fn draws_a_tree_with_a_list_and_a_table(cx: &mut TestAppContext) {
    let block = custom(tree());
    let id = block.id;
    draw(cx, vec![block]);
    // The table is the first stateful node: drawing created its state
    // under the derived id, holding the plugin's rows.
    let rows = cx.update(|cx| {
        let window = cx.windows()[0];
        window
            .update(cx, |_, window, cx| {
                let state =
                    crate::blocks::block_state::<TableView>(node_id(id, 0), window, cx, |_, _| {
                        panic!("the table node was not drawn")
                    });
                state.read(cx).table().spec().rows.len()
            })
            .unwrap_or_default()
    });
    assert_eq!(rows, 2);
}

#[gpui::test]
fn deep_trees_stop_at_the_depth_limit(cx: &mut TestAppContext) {
    let mut node = text("hoja");
    for _ in 0..(MAX_DEPTH * 4) {
        node = CustomNode::Col {
            children: vec![node],
        };
    }
    draw(cx, vec![custom(node)]);
}

/// A 1×1 transparent PNG.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

pub(crate) fn rich_tree() -> CustomNode {
    use domain::block::{ImageData, ImageFit};
    CustomNode::Col {
        children: vec![
            CustomNode::Image {
                source: ImageData::Base64 { data: PIXEL.into() },
                media_type: Some("image/png".into()),
                alt: Some("pixel".into()),
                fit: ImageFit::Actual,
            },
            CustomNode::Image {
                source: ImageData::Path {
                    path: "/nonexistent/plot.png".into(),
                },
                media_type: None,
                alt: None,
                fit: ImageFit::Contain,
            },
            CustomNode::Html {
                source: "<h2>Report</h2><p>ok <a href=\"https://example.com\">x</a></p>".into(),
            },
            CustomNode::Json {
                value: serde_json::json!({ "a": [1, 2, { "b": null }] }),
                expand_depth: 3,
            },
            CustomNode::Mermaid {
                source: "graph TD; a-->b".into(),
            },
            CustomNode::Diff {
                unified: "@@ -1 +1 @@\n-a\n+b\n".into(),
            },
            CustomNode::Link {
                text: "docs".into(),
                url: "https://example.com/docs".into(),
            },
            CustomNode::Model3d(domain::model3d::ModelSpec {
                name: "tri.obj".into(),
                mesh: domain::model3d::ModelMesh {
                    positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    triangles: vec![[0, 1, 2]],
                    up: domain::model3d::UpAxis::Y,
                },
            }),
        ],
    }
}

#[gpui::test]
fn draws_every_rich_node_and_links_open_the_browser(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    view.update(cx, |view, cx| {
        view.apply(
            &DomainEvent::BlockAdded {
                block: custom(rich_tree()),
            },
            cx,
        );
    });
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(800.), px(1600.)),
        |_, _| div().size_full().child(view.clone()),
    );
    cx.run_until_parked();
    let bounds = cx
        .debug_bounds("custom-link")
        .unwrap_or_else(|| panic!("link not drawn"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/docs"));
}

#[gpui::test]
fn plot_nodes_keep_their_slider_state(cx: &mut TestAppContext) {
    let plot = CustomNode::Plot(
        serde_json::from_value(serde_json::json!({
            "x_range": {"min": -3, "max": 3},
            "functions": [{"kind": "explicit", "expr": "a x^2"}],
            "params": [{"name": "a", "min": -1, "max": 1, "value": 0.5}]
        }))
        .unwrap_or_else(|e| panic!("{e}")),
    );
    let block = custom(CustomNode::Col {
        children: vec![text("parabola"), plot],
    });
    let id = block.id;
    draw(cx, vec![block]);
    // The plot is the first stateful node of the tree.
    let node = super::node_id(id, 0);
    assert!(cx.update(|cx| {
        crate::blocks::existing_block_state::<crate::blocks::plot::PlotView>(node, cx).is_some()
    }));
}

/// Bounds of the two charts of a `Row` drawn in a window `width` wide.
fn row_of_two_charts(
    cx: &mut TestAppContext,
    width: f32,
) -> (gpui::Bounds<gpui::Pixels>, gpui::Bounds<gpui::Pixels>) {
    let chart = || {
        CustomNode::Chart(ChartSpec {
            title: "avance".into(),
            kind: ChartKind::Bar,
            labels: vec!["lun".into(), "mar".into(), "mié".into()],
            series: vec![Series {
                name: "hechas".into(),
                values: vec![1.0, 3.0, 2.0],
            }],
        })
    };
    let block = custom(CustomNode::Row {
        children: vec![chart(), chart()],
    });
    let id = block.id;
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    view.update(cx, |view, cx| {
        view.apply(&DomainEvent::BlockAdded { block }, cx)
    });
    // Real frames, as in the chart tests.
    cx.simulate_resize(gpui::size(px(width), px(3000.)));
    cx.run_until_parked();
    let mut bounds = |ordinal| {
        let selector = format!("chart-plot-{}", node_id(id, ordinal));
        cx.debug_bounds(selector.leak())
            .unwrap_or_else(|| panic!("chart {ordinal} drawn"))
    };
    (bounds(0), bounds(1))
}

#[gpui::test]
fn row_children_sit_side_by_side_when_wide(cx: &mut TestAppContext) {
    let (a, b) = row_of_two_charts(cx, 1100.);
    assert_eq!(a.origin.y, b.origin.y, "{a:?} {b:?}");
    assert!(b.origin.x >= a.origin.x + a.size.width, "{a:?} {b:?}");
    assert!((a.size.width - b.size.width).abs() < px(1.));
    assert!(a.size.width > px(400.) && a.size.width < px(560.), "{a:?}");
}

#[gpui::test]
fn row_children_stack_when_narrow(cx: &mut TestAppContext) {
    let (a, b) = row_of_two_charts(cx, 500.);
    assert_eq!(a.origin.x, b.origin.x, "{a:?} {b:?}");
    assert!(b.origin.y >= a.origin.y + a.size.height, "{a:?} {b:?}");
    assert!(a.size.width > px(400.), "{a:?}");
}

/// A tree with one node of each shared widget.
fn widget_tree() -> CustomNode {
    let timeline = serde_json::json!({
        "type": "timeline",
        "title": "trace",
        "rows": [
            { "id": "a", "label": "GET /", "spans": [{ "start": 0, "end": 100, "label": "GET /" }] },
            { "id": "b", "label": "db", "depth": 1,
              "spans": [{ "start": 10, "end": 60, "status": "error" }] }
        ]
    });
    let list = serde_json::json!({
        "type": "filter_list",
        "items": [
            { "level": "error", "text": "boom", "fields": { "k": 1 } },
            { "level": "info", "text": "fine" }
        ]
    });
    let graph = serde_json::json!({
        "type": "graph",
        "nodes": [{ "id": "a" }, { "id": "b" }],
        "edges": [{ "from": "a", "to": "b" }]
    });
    let tree = serde_json::json!({
        "type": "tree",
        "columns": [{ "name": "ms", "kind": "bar" }],
        "nodes": [{ "label": "root", "values": { "ms": 2 },
                    "children": [{ "label": "leaf", "values": { "ms": 1 } }] }]
    });
    CustomNode::Col {
        children: [timeline, list, graph, tree]
            .into_iter()
            .map(|node| serde_json::from_value(node).unwrap_or_else(|e| panic!("{e}")))
            .collect(),
    }
}

#[gpui::test]
fn widget_nodes_draw_and_a_click_in_them_keeps_the_row_open(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let block = custom(widget_tree());
    let id = block.id;
    view.update(cx, |view, cx| {
        view.apply(&DomainEvent::BlockAdded { block }, cx);
    });
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(900.), px(1600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        cx.run_until_parked();
    };
    draw(cx);
    let canvas = cx
        .debug_bounds("timeline-canvas")
        .unwrap_or_else(|| panic!("timeline not drawn"));
    cx.simulate_click(canvas.center(), gpui::Modifiers::none());
    draw(cx);
    assert!(
        view.read_with(cx, |v, _| v.state().entries()[0].expanded),
        "a click in the widget does not collapse the block"
    );
    let timeline = cx.update(|_, cx| {
        crate::blocks::existing_block_state::<crate::widgets::timeline::TimelineView>(
            super::node_id(id, 0),
            cx,
        )
    });
    assert!(
        timeline.is_some(),
        "the timeline is the first stateful node"
    );
    // Keys typed in the list's filter stay there: Enter does not
    // collapse the block.
    let input = cx
        .debug_bounds("filter-list-input")
        .unwrap_or_else(|| panic!("list not drawn"));
    cx.simulate_click(input.center(), gpui::Modifiers::none());
    cx.simulate_input("boom");
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(view.read_with(cx, |v, _| v.state().entries()[0].expanded));
    let list = cx.update(|_, cx| {
        crate::blocks::existing_block_state::<crate::widgets::list::ListView>(
            super::node_id(id, 1),
            cx,
        )
    });
    let visible = list.map(|list| list.read_with(cx, |l, _| l.visible().to_vec()));
    assert_eq!(visible, Some(vec![0]));

    let formats = crate::blocks::copy::custom(&widget_tree());
    assert_eq!(formats[0].key, "spans");
    assert_eq!(formats[1].key, "lines");
    assert_eq!(formats[2].key, "dot");
    assert!(formats[2].text.contains("\"a\" -> \"b\""));
    assert_eq!(formats[3].key, "tree");
    assert_eq!(formats[3].text, "name\tms\nroot\t2\n  leaf\t1\n");
    assert!(formats[0].text.starts_with("row,label,start,end"));
}
