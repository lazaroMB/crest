//! The results of MCP tools drawn with crest's own blocks.
//!
//! A tool result reaches the model as its text content, as before; the
//! user also gets a block when the result has something to draw:
//!
//! - its `structuredContent`, with the best block for its shape: a MIME
//!   bundle ([`super::mime`]), an Adaptive Card or A2UI messages
//!   (read-only: a tool result cannot be answered), records as a table (a
//!   list of objects, or an object holding one such list beside scalar
//!   fields), anything else as a JSON tree. Column and heading titles come
//!   from the tool's `outputSchema` when it gives them;
//! - content items meant for the user: images, text annotated for the
//!   user only (`audience: ["user"]`), and embedded resources of a media
//!   type crest draws (cards, Vega-Lite, the `Computed` envelope,
//!   markdown, images).
//!
//! Items meant only for the model (`audience: ["assistant"]`) are never
//! drawn, and user-only items never reach the model (see
//! `McpToolResult::to_tool_result_content`). A tool with an MCP Apps view
//! (`_meta.ui.resourceUri`, a `ui://` HTML resource) gets a note: crest
//! does not run those views, and draws the data natively instead.

use domain::block::{BlockKind, Cell, Column, CustomNode, ImageData, ImageFit, TableSpec};
use domain::interop::{self as types, MimeBundle, json_size};
use serde_json::{Map, Value};

use super::card::{clip, placeholder};
use super::mime::{self, Drawn};
use super::{a2ui, adaptive};
use crate::ports::mcp::{McpContent, McpTool, McpToolResult};

/// Largest structured content drawn, as JSON.
pub const MAX_STRUCTURED_BYTES: usize = 4 * 1024 * 1024;
/// Most rows of a records table.
pub const MAX_ROWS: usize = 1000;
/// Most columns of a records table.
pub const MAX_COLUMNS: usize = 50;

/// Media types of embedded resources drawn for the user even when they
/// are meant for the model too.
const RICH: [&str; 9] = [
    types::A2UI,
    types::ADAPTIVE_CARD,
    types::ADAPTIVE_CARD_JSON,
    types::CREST_COMPUTED,
    types::VEGA_LITE_V5,
    types::VEGA_LITE_V4,
    types::MARKDOWN,
    types::SVG,
    types::LATEX,
];

/// The block for the result of `tool`, or `None` when it has nothing to
/// draw beyond its text.
#[must_use]
pub fn render(tool: &McpTool, result: &McpToolResult) -> Option<BlockKind> {
    let origin = tool.qualified_name();
    let mut parts: Vec<CustomNode> = Vec::new();
    let mut computed = None;
    let view = tool.ui.resource_uri.clone().or_else(|| {
        result
            .meta
            .as_ref()
            .and_then(|m| m.get("ui"))
            .and_then(|u| u.get("resourceUri"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if let Some(uri) = view.filter(|u| u.starts_with("ui://")) {
        parts.push(CustomNode::Text {
            text: format!(
                "This tool has an MCP Apps view ({}), an HTML page crest does not run; its data \
                 is shown here instead.",
                clip(&uri)
            ),
        });
    }
    if let Some(structured) = result.structured_content.as_ref().filter(|v| !v.is_null()) {
        match structured_node(structured, tool.output_schema.as_ref(), &origin) {
            Part::Node(node) => parts.push(node),
            Part::Computed(view) => {
                parts.push(view.node());
                computed = Some(view);
            }
        }
    }
    for item in result.content.iter().filter(|i| i.audience().to_user()) {
        if let Some(node) = content_node(item, &origin) {
            parts.push(node);
        }
    }
    if parts.is_empty() {
        return None;
    }
    if parts.len() == 1
        && let Some(view) = computed
    {
        return Some(view.block(&origin));
    }
    let tree = if parts.len() == 1 {
        parts.pop()?
    } else {
        CustomNode::Col { children: parts }
    };
    Some(super::block(format!("result · {origin}"), tree))
}

/// What structured content became.
enum Part {
    Node(CustomNode),
    Computed(mime::MimeView),
}

fn structured_node(value: &Value, schema: Option<&Value>, origin: &str) -> Part {
    if json_size(value) > MAX_STRUCTURED_BYTES {
        return Part::Node(placeholder(
            "structured content".into(),
            Some(format!(
                "larger than {} MiB; the model still got its text",
                MAX_STRUCTURED_BYTES / (1024 * 1024)
            )),
            &Value::Null,
        ));
    }
    if let Some(bundle) = MimeBundle::from_value(value)
        && let Ok(view) = mime::render(&bundle, origin)
    {
        return match view.kind {
            Drawn::Computed { .. } => Part::Computed(view),
            Drawn::Node(_) => Part::Node(view.node()),
        };
    }
    let titled =
        |node: CustomNode| match schema.and_then(|s| s.get("title")).and_then(Value::as_str) {
            Some(title) => CustomNode::Card {
                title: Some(clip(title)),
                children: vec![node],
            },
            None => node,
        };
    let is_card = value.get("type").and_then(Value::as_str) == Some("AdaptiveCard")
        || value
            .get("contentType")
            .and_then(Value::as_str)
            .is_some_and(|t| t == types::ADAPTIVE_CARD || t == types::ADAPTIVE_CARD_JSON);
    if is_card && let Ok(card) = adaptive::map(value, false) {
        return Part::Node(card.card.tree());
    }
    if looks_like_a2ui(value)
        && let Ok(card) = a2ui::map(value, false)
    {
        return Part::Node(card.card.tree());
    }
    if let Some(table) = records(value, schema.and_then(|s| s.get("items")), None) {
        return Part::Node(titled(table));
    }
    if let Value::Object(map) = value {
        let lists: Vec<(&String, &Value)> = map.iter().filter(|(_, v)| is_records(v)).collect();
        if let [(key, list)] = lists.as_slice() {
            let item_schema = schema
                .and_then(|s| s.get("properties"))
                .and_then(|p| p.get(key.as_str()))
                .and_then(|p| p.get("items"));
            if let Some(table) = records(list, item_schema, Some(key)) {
                let mut children = Vec::new();
                let scalars = facts(map, schema);
                if let Some(scalars) = scalars {
                    children.push(scalars);
                }
                children.push(table);
                let others = map
                    .iter()
                    .filter(|(k, v)| k != key && (v.is_object() || v.is_array()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Map<_, _>>();
                if !others.is_empty() {
                    children.push(CustomNode::Json {
                        value: Value::Object(others),
                        expand_depth: 1,
                    });
                }
                return Part::Node(titled(CustomNode::Col { children }));
            }
        }
    }
    Part::Node(titled(CustomNode::Json {
        value: value.clone(),
        expand_depth: 2,
    }))
}

/// Whether `value` looks like A2UI messages.
fn looks_like_a2ui(value: &Value) -> bool {
    const KEYS: [&str; 5] = [
        "createSurface",
        "updateComponents",
        "surfaceUpdate",
        "beginRendering",
        "updateDataModel",
    ];
    let message = |v: &Value| KEYS.iter().any(|k| v.get(*k).is_some());
    match value {
        Value::Array(items) => !items.is_empty() && items.iter().any(message),
        Value::Object(map) => {
            message(value)
                || map
                    .get("messages")
                    .and_then(Value::as_array)
                    .is_some_and(|m| m.iter().any(message))
        }
        _ => false,
    }
}

/// Whether `value` is a non-empty list of objects.
fn is_records(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_object))
}

/// A scalar as a table cell; nested values as their JSON, clipped.
fn cell(value: Option<&Value>) -> Cell {
    match value {
        None | Some(Value::Null) => Cell::Null,
        Some(Value::Bool(b)) => Cell::Bool(*b),
        Some(Value::Number(n)) => n
            .as_f64()
            .filter(|v| v.is_finite())
            .map_or_else(|| Cell::Text(n.to_string()), Cell::Number),
        Some(Value::String(s)) => Cell::Text(clip(s)),
        Some(other) => {
            let mut text = other.to_string();
            if text.len() > 200 {
                let mut end = 200;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                text.push('…');
            }
            Cell::Text(text)
        }
    }
}

/// The title a schema gives property `name`, or the name.
fn title(schema: Option<&Value>, name: &str) -> String {
    schema
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.get(name))
        .and_then(|p| p.get("title"))
        .and_then(Value::as_str)
        .map_or_else(|| name.to_owned(), str::to_owned)
}

/// A list of objects as a table: columns in first-seen order (at most
/// [`MAX_COLUMNS`]), rows up to [`MAX_ROWS`] with a note past them.
fn records(
    value: &Value,
    item_schema: Option<&Value>,
    heading: Option<&str>,
) -> Option<CustomNode> {
    if !is_records(value) {
        return None;
    }
    let items = value.as_array()?;
    let mut names: Vec<String> = Vec::new();
    for item in items.iter().take(MAX_ROWS) {
        for key in item.as_object()?.keys() {
            if !names.contains(key) && names.len() < MAX_COLUMNS {
                names.push(key.clone());
            }
        }
    }
    let rows = items
        .iter()
        .take(MAX_ROWS)
        .map(|item| names.iter().map(|n| cell(item.get(n))).collect())
        .collect();
    let table = CustomNode::Table(TableSpec {
        columns: names
            .iter()
            .map(|n| Column::new(title(item_schema, n)))
            .collect(),
        rows,
    });
    let mut children = Vec::new();
    if let Some(heading) = heading {
        children.push(CustomNode::Markdown {
            source: format!("**{}**", clip(heading)),
        });
    }
    children.push(table);
    if items.len() > MAX_ROWS {
        children.push(CustomNode::Text {
            text: format!("First {MAX_ROWS} of {} rows.", items.len()),
        });
    }
    Some(if children.len() == 1 {
        children.pop()?
    } else {
        CustomNode::Col { children }
    })
}

/// The scalar fields of an object as a two-column table.
fn facts(map: &Map<String, Value>, schema: Option<&Value>) -> Option<CustomNode> {
    let rows: Vec<Vec<Cell>> = map
        .iter()
        .filter(|(_, v)| !v.is_object() && !v.is_array())
        .map(|(k, v)| vec![Cell::Text(title(schema, k)), cell(Some(v))])
        .collect();
    (!rows.is_empty()).then(|| {
        CustomNode::Table(TableSpec {
            columns: vec![Column::new("Field"), Column::new("Value")],
            rows,
        })
    })
}

/// The node for a content item shown to the user, if it has one.
fn content_node(item: &McpContent, origin: &str) -> Option<CustomNode> {
    match item {
        McpContent::Text { text, audience } if !audience.to_model() => {
            Some(CustomNode::Markdown { source: clip(text) })
        }
        McpContent::Text { .. } => None,
        McpContent::Image {
            media_type, data, ..
        } => {
            let media_type = types::essence(media_type);
            if media_type == types::SVG {
                let bundle = MimeBundle {
                    data: [(types::SVG.to_owned(), Value::String(decode_svg(data)?))].into(),
                    metadata: Default::default(),
                };
                return mime::render(&bundle, origin).ok().map(|v| v.node());
            }
            media_type.starts_with("image/").then(|| CustomNode::Image {
                source: ImageData::Base64 { data: data.clone() },
                media_type: Some(media_type),
                alt: None,
                fit: ImageFit::Contain,
            })
        }
        McpContent::Resource {
            text: Some(text),
            mime_type: Some(mime_type),
            audience,
            ..
        } => {
            let media_type = types::essence(mime_type);
            if !RICH.contains(&media_type.as_str()) && audience.to_model() {
                return None;
            }
            let value = if media_type.ends_with("json") || media_type == types::A2UI {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
            } else {
                Value::String(text.clone())
            };
            let bundle = MimeBundle {
                data: [(media_type, value)].into(),
                metadata: Default::default(),
            };
            mime::render(&bundle, origin).ok().map(|v| v.node())
        }
        McpContent::Resource { .. } => None,
    }
}

/// The text of a base64 SVG.
fn decode_svg(data: &str) -> Option<String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests;
