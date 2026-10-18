use domain::block::{Cell, Column, TableSpec};

use super::*;
use crate::callbacks::{self, CallbackIds};
use crate::config::api::Phase;
use crate::sandbox;

/// Evaluates `code` with the `app` table of plugin `todo` and converts
/// what it returned like a tool result.
fn eval(code: &str) -> mlua::Result<PluginOutput> {
    let lua = sandbox::new_lua(None)?;
    callbacks::install(&lua, CallbackIds::default())?;
    let phase = Phase::new(&lua, "init.lua", "todo")?;
    let value: Value = lua
        .load(code)
        .set_name("@init.lua")
        .set_environment(phase.env.clone())
        .eval()?;
    Ok(output(convert::to_json(&lua, value)?))
}

fn tree(code: &str) -> CustomNode {
    match eval(code).unwrap() {
        PluginOutput::Block(BlockKind::Custom { plugin, tree, .. }) => {
            assert_eq!(plugin, "todo");
            tree
        }
        other => panic!("not a custom block: {other:?}"),
    }
}

fn text(s: &str) -> CustomNode {
    CustomNode::Text { text: s.into() }
}

fn error_of(code: &str) -> String {
    eval(code).unwrap_err().to_string()
}

#[test]
fn list_turns_strings_into_text_nodes() {
    let got = eval(r#"return app.ui.block("todo", app.ui.list({ "a", "b", 3 }))"#).unwrap();
    assert_eq!(
        got,
        PluginOutput::Block(BlockKind::Custom {
            plugin: "todo".into(),
            name: "todo".into(),
            tree: CustomNode::List {
                items: vec![text("a"), text("b"), text("3")],
            },
        })
    );
}

#[test]
fn layouts_nest_every_primitive() {
    let got = tree(
        r#"return app.ui.block("x", app.ui.col({
             app.ui.text("hola"),
             app.ui.row({ app.ui.code("lua", "x = 1"), app.ui.markdown("**b**") }),
             app.ui.list({}),
           }))"#,
    );
    assert_eq!(
        got,
        CustomNode::Col {
            children: vec![
                text("hola"),
                CustomNode::Row {
                    children: vec![
                        CustomNode::Code {
                            lang: Some("lua".into()),
                            source: "x = 1".into(),
                        },
                        CustomNode::Markdown {
                            source: "**b**".into(),
                        },
                    ],
                },
                CustomNode::List { items: vec![] },
            ],
        }
    );
}

#[test]
fn tables_take_column_names_or_definitions() {
    let got = tree(
        r#"return app.ui.block("t", app.ui.table({
             columns = { "tarea", { name = "n", align = "right" } },
             rows = { { "parser", 2 }, { "docs", 1.5 } },
           }))"#,
    );
    let CustomNode::Table(spec) = got else {
        panic!("not a table: {got:?}");
    };
    assert_eq!(
        spec,
        TableSpec {
            columns: vec![
                Column::new("tarea"),
                Column {
                    name: "n".into(),
                    align: Some(domain::block::Align::Right),
                },
            ],
            rows: vec![
                vec![Cell::Text("parser".into()), Cell::Number(2.0)],
                vec![Cell::Text("docs".into()), Cell::Number(1.5)],
            ],
        }
    );
}

#[test]
fn charts_default_to_bars() {
    let got = tree(
        r#"return app.ui.block("c", app.ui.chart({
             labels = { "lun", "mar" },
             series = { { name = "hechas", values = { 1, 3 } } },
           }))"#,
    );
    let CustomNode::Chart(spec) = got else {
        panic!("not a chart: {got:?}");
    };
    assert_eq!(spec.kind, domain::block::ChartKind::Bar);
    assert_eq!(spec.series[0].values, [1.0, 3.0]);
}

#[test]
fn invalid_trees_raise_errors() {
    let short_row =
        error_of(r#"return app.ui.table({ columns = { "a", "b" }, rows = { { 1 } } })"#);
    assert!(
        short_row.contains("app.ui.table") && short_row.contains("row 0"),
        "{short_row}"
    );
    let no_series = error_of(r#"return app.ui.chart({ labels = { "a" } })"#);
    assert!(no_series.contains("app.ui.chart"), "{no_series}");
    let bad_child = error_of(r#"return app.ui.row({ { nope = 1 } })"#);
    assert!(
        bad_child.contains("item 1") && bad_child.contains("not a ui node"),
        "{bad_child}"
    );
    let bad_list = error_of(r#"return app.ui.list("a")"#);
    assert!(bad_list.contains("expected a list"), "{bad_list}");
    let bad_tree = error_of(r#"return app.ui.block("x", true)"#);
    assert!(bad_tree.contains("app.ui.block"), "{bad_tree}");
}

#[test]
fn plain_values_stay_values() {
    assert_eq!(
        eval(r#"return { a = 1 }"#).unwrap(),
        PluginOutput::Value(json!({ "a": 1 }))
    );
    // A block nested in other data is just data.
    let nested = eval(r#"return { b = app.ui.block("x", app.ui.text("t")) }"#).unwrap();
    assert!(matches!(nested, PluginOutput::Value(_)), "{nested:?}");
}
