//! Conversions between rmcp's model and the port types.

use application::ports::mcp::{McpContent, McpError, McpTool, McpToolResult};
use rmcp::ServiceError;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents, Tool};

/// The port's view of `tool`, advertised by `server`.
pub(crate) fn tool(server: &str, tool: Tool) -> McpTool {
    McpTool {
        server: server.to_owned(),
        name: tool.name.into_owned(),
        description: tool
            .description
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

/// The port's view of a tool result. Audio has no equivalent and becomes a
/// short note; a result with only structured content becomes its JSON.
pub(crate) fn result(result: CallToolResult) -> McpToolResult {
    let mut content: Vec<McpContent> = result.content.into_iter().map(content).collect();
    if content.is_empty()
        && let Some(structured) = result.structured_content
    {
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
        ContentBlock::ResourceLink(link) => McpContent::Resource {
            uri: link.uri,
            text: None,
        },
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
            tool("calc", rmcp_tool),
            McpTool {
                server: "calc".into(),
                name: "add".into(),
                description: "Adds two numbers".into(),
                input_schema: schema,
            }
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
