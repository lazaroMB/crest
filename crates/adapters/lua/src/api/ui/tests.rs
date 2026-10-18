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

#[test]
fn rich_constructors_build_their_nodes() {
    use domain::block::{ImageData, ImageFit};
    let got = tree(
        r##"return app.ui.block("x", app.ui.col({
             app.ui.image("shots/a.png"),
             app.ui.image{ data = "AA==", media_type = "image/png", alt = "dot", fit = "actual" },
             app.ui.html("<b>hi</b>"),
             app.ui.json({ a = { 1, 2 } }, { expand = 2 }),
             app.ui.mermaid("graph TD; a-->b"),
             app.ui.link("docs", "https://example.com"),
             app.ui.link("https://crest.dev"),
             app.ui.diff("@@ -1 +1 @@\n-a\n+b\n"),
           }))"##,
    );
    let CustomNode::Col { children } = got else {
        panic!("not a col")
    };
    assert_eq!(
        children[0],
        CustomNode::Image {
            source: ImageData::Path {
                path: "shots/a.png".into()
            },
            media_type: None,
            alt: None,
            fit: ImageFit::Contain,
        }
    );
    assert_eq!(
        children[1],
        CustomNode::Image {
            source: ImageData::Base64 {
                data: "AA==".into()
            },
            media_type: Some("image/png".into()),
            alt: Some("dot".into()),
            fit: ImageFit::Actual,
        }
    );
    assert_eq!(
        children[2],
        CustomNode::Html {
            source: "<b>hi</b>".into()
        }
    );
    assert_eq!(
        children[3],
        CustomNode::Json {
            value: serde_json::json!({ "a": [1, 2] }),
            expand_depth: 2,
        }
    );
    assert!(matches!(children[4], CustomNode::Mermaid { .. }));
    assert_eq!(
        children[5],
        CustomNode::Link {
            text: "docs".into(),
            url: "https://example.com".into()
        }
    );
    assert_eq!(
        children[6],
        CustomNode::Link {
            text: "https://crest.dev".into(),
            url: "https://crest.dev".into()
        }
    );
    assert!(matches!(children[7], CustomNode::Diff { .. }));
}

#[test]
fn csv_becomes_a_table() {
    let got = tree(r#"return app.ui.block("x", app.ui.csv("name,n\na,1\nb,2\n"))"#);
    assert_eq!(
        got,
        CustomNode::Table(TableSpec {
            columns: vec![Column::new("name"), Column::new("n")],
            rows: vec![
                vec![Cell::Text("a".into()), Cell::Number(1.0)],
                vec![Cell::Text("b".into()), Cell::Number(2.0)],
            ],
        })
    );
    let tsv = tree(
        r#"return app.ui.block("x", app.ui.csv("1\t2", { header = false, delimiter = "\t" }))"#,
    );
    let CustomNode::Table(spec) = tsv else {
        panic!("not a table")
    };
    assert_eq!(spec.columns.len(), 2);
    assert_eq!(spec.rows, vec![vec![Cell::Number(1.0), Cell::Number(2.0)]]);
}

#[test]
fn rich_constructors_reject_bad_arguments() {
    let cases = [
        ("app.ui.image{}", "give `path` (a file) or `data`"),
        ("app.ui.image{ path = 'a', data = 'b' }", "not both"),
        ("app.ui.image{ data = 'AA==' }", "needs a media_type"),
        ("app.ui.image{ path = 'a', fit = 'zoom' }", "app.ui.image"),
        ("app.ui.image(3)", "expected a path or a table"),
        (
            "app.ui.link('x', 'file:///etc/passwd')",
            "must be an http, https or mailto URL",
        ),
        (
            "app.ui.json({}, { expand = 1.5 })",
            "`expand` must be a whole number",
        ),
        ("app.ui.csv('a\\n\"open', {})", "unclosed quote"),
        (
            "app.ui.csv('a', { delimiter = ';;' })",
            "`delimiter` must be one character",
        ),
    ];
    for (code, expected) in cases {
        let err = error_of(code);
        assert!(err.contains(expected), "{code}: {err}");
    }
}
