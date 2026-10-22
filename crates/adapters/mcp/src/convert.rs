//! Conversions between rmcp's model and the port types.
//!
//! Tools are adapted to what every model provider accepts: names become
//! `[a-zA-Z0-9_-]` and short enough for `mcp__<server>__<tool>` to fit in
//! 64 characters (the OpenAI limit), and input schemas become a plain
//! top-level object (no `$schema`, local `$ref`s inlined, a top-level
//! `anyOf`/`oneOf`/`allOf` merged into one object), which is what the
//! Anthropic and OpenAI APIs require.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use application::ports::mcp::{McpContent, McpError, McpTool, McpToolResult, qualified_name};
use rmcp::ServiceError;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents, Tool};
use serde_json::{Map, Value, json};

/// Longest qualified tool name offered to the model.
const MAX_QUALIFIED: usize = 64;
/// Shortest tool part kept when the server name is long.
const MIN_NAME: usize = 16;
/// Deepest chain of `$ref`s inlined.
const MAX_REF_DEPTH: usize = 8;

/// The port's view of `tool`, advertised by `server`, and the name the
/// server knows it by. The port name is unique within `taken`.
pub(crate) fn tool(server: &str, tool: Tool, taken: &[String]) -> (McpTool, String) {
    let original = tool.name.into_owned();
    let mut name = tool_name(server, &original);
    if taken.contains(&name) {
        name = with_hash(&name, &original, budget(server));
    }
    let mcp = McpTool {
        server: server.to_owned(),
        name,
        description: tool
            .description
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        input_schema: schema((*tool.input_schema).clone()),
    };
    (mcp, original)
}

/// Room for the tool part of `mcp__<server>__<tool>`.
fn budget(server: &str) -> usize {
    MAX_QUALIFIED
        .saturating_sub(qualified_name(server, "").len())
        .max(MIN_NAME)
}

/// `original` with only `[a-zA-Z0-9_-]`, cut to the budget of `server`.
fn tool_name(server: &str, original: &str) -> String {
    let clean: String = original
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = if clean.is_empty() {
        "tool".to_owned()
    } else {
        clean
    };
    let budget = budget(server);
    if clean.len() <= budget {
        return clean;
    }
    with_hash(&clean, original, budget)
}

/// `name` cut to leave room for `_` and a short hash of `original`.
fn with_hash(name: &str, original: &str, budget: usize) -> String {
    let mut hasher = DefaultHasher::new();
    original.hash(&mut hasher);
    let hash = format!("{:06x}", hasher.finish() & 0xff_ffff);
    let keep = budget.saturating_sub(hash.len() + 1).min(name.len());
    format!("{}_{hash}", &name[..keep])
}

/// An input schema every provider accepts.
pub(crate) fn schema(mut root: Map<String, Value>) -> Value {
    root.remove("$schema");
    let defs = root
        .remove("$defs")
        .or_else(|| root.remove("definitions"))
        .and_then(|d| d.as_object().cloned())
        .unwrap_or_default();
    let mut root = Value::Object(root);
    inline_refs(&mut root, &defs, 0);
    let Value::Object(mut root) = root else {
        unreachable!("an object stays an object")
    };
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(variants)) = root.remove(key) {
            merge_variants(&mut root, &variants, key == "allOf");
        }
    }
    root.insert("type".into(), json!("object"));
    root.entry("properties").or_insert_with(|| json!({}));
    Value::Object(root)
}

/// Replaces `{"$ref": "#/$defs/X"}` (or `#/definitions/X`) by a copy of
/// `X`, down to [`MAX_REF_DEPTH`]; deeper or foreign refs become `{}`.
fn inline_refs(value: &mut Value, defs: &Map<String, Value>, depth: usize) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get("$ref") {
                let name = target
                    .strip_prefix("#/$defs/")
                    .or_else(|| target.strip_prefix("#/definitions/"));
                let mut resolved = match (name.and_then(|n| defs.get(n)), depth < MAX_REF_DEPTH) {
                    (Some(def), true) => def.clone(),
                    _ => json!({}),
                };
                inline_refs(&mut resolved, defs, depth + 1);
                map.remove("$ref");
                if let Value::Object(resolved) = resolved {
                    for (k, v) in resolved {
                        map.entry(k).or_insert(v);
                    }
                }
                return;
            }
            for child in map.values_mut() {
                inline_refs(child, defs, depth);
            }
        }
        Value::Array(items) => {
            for item in items {
                inline_refs(item, defs, depth);
            }
        }
        _ => {}
    }
}

/// Merges the object variants of a top-level combinator into `root`: every
/// property of any variant, and as required only what all of them require
/// (or, for `allOf`, what any of them requires).
fn merge_variants(root: &mut Map<String, Value>, variants: &[Value], all: bool) {
    let mut properties = root
        .remove("properties")
        .and_then(|p| p.as_object().cloned())
        .unwrap_or_default();
    let mut required: Option<Vec<Value>> =
        root.remove("required").and_then(|r| r.as_array().cloned());
    let base_required = required.clone();
    let mut first = true;
    for variant in variants {
        let Some(variant) = variant.as_object() else {
            continue;
        };
        if let Some(Value::Object(props)) = variant.get("properties") {
            for (k, v) in props {
                properties.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let these = variant
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if all {
            let list = required.get_or_insert_with(Vec::new);
            for r in these {
                if !list.contains(&r) {
                    list.push(r);
                }
            }
        } else if first {
            let mut list = base_required.clone().unwrap_or_default();
            list.extend(these);
            required = Some(list);
        } else if let Some(list) = &mut required {
            let base = base_required.clone().unwrap_or_default();
            list.retain(|r| base.contains(r) || these.contains(r));
        }
        first = false;
    }
    root.insert("properties".into(), Value::Object(properties));
    if let Some(required) = required.filter(|r| !r.is_empty()) {
        root.insert("required".into(), Value::Array(required));
    }
}

/// The port's view of a tool result. Audio has no equivalent and becomes a
/// short note; structured content becomes its JSON when no text says it.
pub(crate) fn result(result: CallToolResult) -> McpToolResult {
    let mut content: Vec<McpContent> = result.content.into_iter().map(content).collect();
    let has_text = content.iter().any(|c| matches!(c, McpContent::Text { .. }));
    if !has_text && let Some(structured) = result.structured_content {
        content.push(McpContent::Text {
            text: structured.to_string(),
        });
    }
    McpToolResult {
        content,
        is_error: result.is_error.unwrap_or(false),
    }
}

fn content(block: ContentBlock) -> McpContent {
    match block {
        ContentBlock::Text(text) => McpContent::Text { text: text.text },
        ContentBlock::Image(image) => McpContent::Image {
            media_type: image.mime_type,
            data: image.data,
        },
        ContentBlock::Audio(audio) => McpContent::Text {
            text: format!("[audio content ({}) omitted]", audio.mime_type),
        },
        ContentBlock::Resource(embedded) => match embedded.resource {
            ResourceContents::TextResourceContents { uri, text, .. } => McpContent::Resource {
                uri,
                text: Some(text),
            },
            ResourceContents::BlobResourceContents { uri, .. } => {
                McpContent::Resource { uri, text: None }
            }
            // `ResourceContents` is non-exhaustive.
            other => McpContent::Text {
                text: serde_json::to_string(&other).unwrap_or_default(),
            },
        },
        ContentBlock::ResourceLink(link) => {
            let about = [Some(link.name.clone()), link.description.clone()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(": ");
            McpContent::Resource {
                uri: link.uri.clone(),
                text: (!about.is_empty()).then_some(about),
            }
        }
        // `ContentBlock` is non-exhaustive.
        other => McpContent::Text {
            text: serde_json::to_string(&other).unwrap_or_default(),
        },
    }
}

/// The port error for a failed request to `server`.
pub(crate) fn error(server: &str, err: ServiceError) -> McpError {
    let server = server.to_owned();
    match err {
        ServiceError::McpError(data) => McpError::Protocol {
            server,
            message: data.message.into_owned(),
        },
        ServiceError::UnexpectedResponse => McpError::Protocol {
            server,
            message: err.to_string(),
        },
        ServiceError::Timeout { .. } => McpError::Timeout { server },
        ServiceError::Cancelled { .. } => McpError::Cancelled,
        other => McpError::Transport {
            server,
            message: other.to_string(),
        },
    }
}

/// Whether `err` means the connection is gone for good.
pub(crate) fn is_disconnect(err: &ServiceError) -> bool {
    matches!(err, ServiceError::TransportClosed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::ErrorData;
    use serde_json::json;

    use super::*;

    #[test]
    fn tools_keep_name_description_and_schema() {
        let schema = json!({ "type": "object", "properties": { "a": { "type": "integer" } } });
        let serde_json::Value::Object(schema_map) = schema.clone() else {
            unreachable!()
        };
        let rmcp_tool = Tool::new("add", "Adds two numbers", Arc::new(schema_map));
        assert_eq!(
            tool("calc", rmcp_tool, &[]),
            (
                McpTool {
                    server: "calc".into(),
                    name: "add".into(),
                    description: "Adds two numbers".into(),
                    input_schema: schema,
                },
                "add".to_owned()
            )
        );
    }

    #[test]
    fn names_are_cleaned_shortened_and_unique() {
        let schema = Arc::new(Map::new());
        let (dotted, original) = tool("fs", Tool::new("files.read", "", schema.clone()), &[]);
        assert_eq!(dotted.name, "files_read");
        assert_eq!(original, "files.read");
        let long = "a".repeat(100);
        let (long_tool, _) = tool("github", Tool::new(long.clone(), "", schema.clone()), &[]);
        assert!(
            long_tool.qualified_name().len() <= MAX_QUALIFIED,
            "{}",
            long_tool.name
        );
        let (again, _) = tool(
            "fs",
            Tool::new("files/read", "", schema),
            &["files_read".into()],
        );
        assert_ne!(again.name, "files_read");
        assert!(again.name.starts_with("files_read_"), "{}", again.name);
    }

    #[test]
    fn schemas_become_plain_objects() {
        let Value::Object(raw) = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "anyOf": [
                { "type": "object", "properties": { "path": { "$ref": "#/$defs/Path" } }, "required": ["path"] },
                { "type": "object", "properties": { "path": { "type": "string" }, "line": { "type": "integer" } }, "required": ["path", "line"] }
            ],
            "$defs": { "Path": { "type": "string", "description": "A path" } }
        }) else {
            unreachable!()
        };
        assert_eq!(
            schema(raw),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "A path" },
                    "line": { "type": "integer" }
                },
                "required": ["path"]
            })
        );
        assert_eq!(
            schema(Map::new()),
            json!({ "type": "object", "properties": {} })
        );
    }

    #[test]
    fn results_map_every_content_kind() {
        let mut call = CallToolResult::success(vec![
            ContentBlock::text("ok"),
            ContentBlock::image("iVBO", "image/png"),
            ContentBlock::embedded_text("file:///a.txt", "hola"),
            ContentBlock::audio("UklG", "audio/wav"),
        ]);
        call.is_error = Some(true);
        assert_eq!(
            result(call),
            McpToolResult {
                content: vec![
                    McpContent::Text { text: "ok".into() },
                    McpContent::Image {
                        media_type: "image/png".into(),
                        data: "iVBO".into(),
                    },
                    McpContent::Resource {
                        uri: "file:///a.txt".into(),
                        text: Some("hola".into()),
                    },
                    McpContent::Text {
                        text: "[audio content (audio/wav) omitted]".into(),
                    },
                ],
                is_error: true,
            }
        );
    }

    #[test]
    fn structured_only_results_become_json_text() {
        let mut call = CallToolResult::success(Vec::new());
        call.structured_content = Some(json!({ "sum": 3 }));
        let mapped = result(call);
        assert_eq!(
            mapped.content,
            vec![McpContent::Text {
                text: r#"{"sum":3}"#.into()
            }]
        );
        assert!(!mapped.is_error);
    }

    #[test]
    fn service_errors_map_to_port_errors() {
        assert_eq!(
            error(
                "s",
                ServiceError::McpError(ErrorData::invalid_params("bad a", None))
            ),
            McpError::Protocol {
                server: "s".into(),
                message: "bad a".into(),
            }
        );
        assert_eq!(
            error("s", ServiceError::TransportClosed),
            McpError::Transport {
                server: "s".into(),
                message: "Transport closed".into(),
            }
        );
        assert!(is_disconnect(&ServiceError::TransportClosed));
        assert!(!is_disconnect(&ServiceError::UnexpectedResponse));
    }
}
