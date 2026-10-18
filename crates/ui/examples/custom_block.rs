//! A `Custom` block as a Lua plugin would return it: a column with text, a
//! list, a table, code and markdown side by side, and a chart.
//!
//! `cargo run -p ui --example custom_block [--quit]`

#[path = "support/demo.rs"]
mod demo;
#[path = "support/mod.rs"]
mod support;

use domain::block::{
    Block, BlockKind, Cell, ChartKind, ChartSpec, Column, CustomNode, Series, TableSpec,
};

fn text(s: &str) -> CustomNode {
    CustomNode::Text { text: s.into() }
}

fn main() {
    let tree = CustomNode::Col {
        children: vec![
            text("Tareas pendientes"),
            CustomNode::List {
                items: vec![text("arreglar el parser"), text("documentar app.ui")],
            },
            CustomNode::Table(TableSpec {
                columns: vec![Column::new("tarea"), Column::new("hecha")],
                rows: vec![
                    vec![Cell::Text("parser".into()), Cell::Bool(false)],
                    vec![Cell::Text("docs".into()), Cell::Bool(true)],
                ],
            }),
            CustomNode::Row {
                children: vec![
                    CustomNode::Code {
                        lang: Some("lua".into()),
                        source: "return app.ui.block(\"todo\", app.ui.list(items))".into(),
                    },
                    CustomNode::Markdown {
                        source: "**2** tareas, *1* hecha".into(),
                    },
                ],
            },
            CustomNode::Chart(ChartSpec {
                title: "Avance".into(),
                kind: ChartKind::Bar,
                labels: vec!["lun".into(), "mar".into(), "mié".into()],
                series: vec![Series {
                    name: "hechas".into(),
                    values: vec![1.0, 3.0, 2.0],
                }],
            }),
        ],
    };
    let blocks = vec![Block::new(BlockKind::Custom {
        plugin: "todo".into(),
        name: "todo".into(),
        tree,
    })];
    demo::run(
        "crest — custom block",
        blocks,
        vec!["block.first", "block.toggle"],
    );
}
