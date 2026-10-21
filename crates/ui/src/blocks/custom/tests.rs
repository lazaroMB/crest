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
