use domain::interop::INTEROP_PLUGIN;
use serde_json::json;

use super::*;
use crate::interop::plain_text;
use crate::ports::mcp::{McpAudience, McpToolUi};

fn tool(output_schema: Option<Value>, ui: McpToolUi) -> McpTool {
    McpTool {
        server: "shop".into(),
        name: "orders".into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        output_schema,
        ui,
    }
}

fn structured(value: Value) -> McpToolResult {
    McpToolResult {
        content: vec![McpContent::text(value.to_string())],
        structured_content: Some(value),
        ..McpToolResult::default()
    }
}

fn tree(block: Option<BlockKind>) -> CustomNode {
    match block {
        Some(BlockKind::Custom { plugin, tree, .. }) => {
            assert_eq!(plugin, INTEROP_PLUGIN);
            tree
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn records_become_a_table_with_schema_titles() {
    let schema = json!({
        "type": "object", "title": "Open orders",
        "properties": {
            "total": {"type": "integer", "title": "Total orders"},
            "orders": {"type": "array", "items": {"type": "object", "properties": {
                "id": {"type": "string", "title": "Order"}, "amount": {"type": "number"}
            }}}
        }
    });
    let result = structured(json!({
        "total": 2,
        "orders": [{"id": "A1", "amount": 9.5}, {"id": "B2", "amount": 3, "note": {"gift": true}}]
    }));
    let tree = tree(render(&tool(Some(schema), McpToolUi::default()), &result));
    let CustomNode::Card { title, children } = &tree else {
        panic!("{tree:?}")
    };
    assert_eq!(title.as_deref(), Some("Open orders"));
    let mut tables = Vec::new();
    tree.for_each(&mut |n| {
        if let CustomNode::Table(t) = n {
            tables.push(t.clone());
        }
    });
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].rows[0][0], Cell::Text("Total orders".into()));
    let names: Vec<&str> = tables[1].columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Order", "amount", "note"]);
    assert_eq!(tables[1].rows[1][2], Cell::Text("{\"gift\":true}".into()));
    assert!(!children.is_empty());
    assert_eq!(tree.validate(), Ok(()));
}

#[test]
fn bundles_cards_and_other_json() {
    let bundle = structured(json!({"text/markdown": "**done**", "text/plain": "done"}));
    let tree = tree(render(&tool(None, McpToolUi::default()), &bundle));
    assert_eq!(
        tree,
        CustomNode::Markdown {
            source: "**done**".into()
        }
    );
    let card = structured(json!({"type": "AdaptiveCard", "version": "1.5",
        "body": [{"type": "TextBlock", "text": "Shipped"}],
        "actions": [{"type": "Action.Submit", "title": "Undo"}]}));
    let text = plain_text(&tree_of(&card));
    assert!(
        text.contains("Shipped") && text.contains("[button: Undo]"),
        "{text}"
    );
    let nested = structured(json!({"a": {"b": [1, 2]}}));
    assert!(matches!(tree_of(&nested), CustomNode::Json { .. }));
    let huge = structured(json!({"blob": "x".repeat(MAX_STRUCTURED_BYTES)}));
    assert!(matches!(tree_of(&huge), CustomNode::Placeholder { .. }));
}

fn tree_of(result: &McpToolResult) -> CustomNode {
    tree(render(&tool(None, McpToolUi::default()), result))
}

#[test]
fn a_computed_bundle_is_a_computed_card() {
    let mut envelope = domain::computed::Computed::new(
        "ext_2",
        "units.convert",
        "unit conversion",
        domain::computed::Engine {
            name: "units-server".into(),
            version: "1".into(),
            backend: None,
            platform: None,
        },
    );
    envelope
        .values
        .insert("to".into(), domain::computed::ComputedValue::new(25.4));
    let body = json!({"type": "quantity", "from": {"value": 1.0, "unit": "in"},
        "to": {"value": 25.4, "unit": "mm"}, "factor": 25.4, "exact": true, "dimension": "length"});
    let result = structured(json!({
        "application/vnd.crest.computed+json": {"envelope": envelope, "body": body}
    }));
    let Some(BlockKind::Computed { envelope, .. }) =
        render(&tool(None, McpToolUi::default()), &result)
    else {
        panic!()
    };
    assert!(envelope.warnings[0].text.contains("mcp__shop__orders"));
}

#[test]
fn audiences_and_apps_views() {
    let result = McpToolResult {
        content: vec![
            McpContent::text("for both: in the tool output already"),
            McpContent::Text {
                text: "only for the model".into(),
                audience: McpAudience::Model,
            },
            McpContent::Text {
                text: "only for **you**".into(),
                audience: McpAudience::User,
            },
            McpContent::Image {
                media_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
                audience: McpAudience::Model,
            },
            McpContent::Resource {
                uri: "mem://card".into(),
                text: Some(
                    r#"{"type": "AdaptiveCard", "body": [{"type": "TextBlock", "text": "card"}]}"#
                        .into(),
                ),
                mime_type: Some("application/vnd.microsoft.card.adaptive".into()),
                audience: McpAudience::Both,
            },
            McpContent::Resource {
                uri: "file:///notes.txt".into(),
                text: Some("plain notes".into()),
                mime_type: Some("text/plain".into()),
                audience: McpAudience::Both,
            },
        ],
        ..McpToolResult::default()
    };
    let ui = McpToolUi {
        resource_uri: Some("ui://shop/orders.html".into()),
        ..McpToolUi::default()
    };
    let text = plain_text(&tree(render(&tool(None, ui), &result)));
    assert!(
        text.contains("MCP Apps view (ui://shop/orders.html)"),
        "{text}"
    );
    assert!(text.contains("only for **you**"), "{text}");
    assert!(text.contains("card"), "{text}");
    assert!(!text.contains("only for the model"), "{text}");
    assert!(
        !text.contains("[image"),
        "model-only image not drawn: {text}"
    );
    assert!(!text.contains("plain notes"), "{text}");
    let to_model: Vec<String> = result
        .to_tool_result_content()
        .iter()
        .filter_map(|c| match c {
            domain::conversation::ToolResultContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(to_model.iter().any(|t| t.contains("only for the model")));
    assert!(!to_model.iter().any(|t| t.contains("only for **you**")));
    let plain = McpToolResult {
        content: vec![McpContent::text("just text")],
        ..McpToolResult::default()
    };
    assert_eq!(render(&tool(None, McpToolUi::default()), &plain), None);
}
