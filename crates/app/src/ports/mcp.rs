//! MCP port: tools exposed by Model Context Protocol servers.
//!
//! MCP tools are offered to the model under a qualified name,
//! `mcp__<server>__<tool>`, so tools of different servers never clash with
//! each other or with built-in tools. See [`qualified_name`] and
//! [`parse_qualified_name`].

use async_trait::async_trait;
use domain::conversation::ToolResultContent;
use serde::{Deserialize, Serialize};

/// Prefix of every MCP tool name as seen by the model.
pub const QUALIFIED_PREFIX: &str = "mcp__";

/// Separator between server and tool in a qualified name.
const SEPARATOR: &str = "__";

/// Name under which `tool` of `server` is offered to the model:
/// `mcp__<server>__<tool>`.
///
/// Server names must not contain `__`; tool names may.
#[must_use]
pub fn qualified_name(server: &str, tool: &str) -> String {
    format!("{QUALIFIED_PREFIX}{server}{SEPARATOR}{tool}")
}

/// Inverse of [`qualified_name`]: splits `mcp__<server>__<tool>` into
/// `(server, tool)`. Returns `None` for names that are not MCP tools or
/// have an empty server or tool.
#[must_use]
pub fn parse_qualified_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(QUALIFIED_PREFIX)?;
    let (server, tool) = rest.split_once(SEPARATOR)?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// A tool advertised by an MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    /// Name of the server, as declared in the Lua config.
    pub server: String,
    /// Tool name within the server (not qualified).
    pub name: String,
    /// Description for the model.
    pub description: String,
    /// JSON Schema of the tool input.
    pub input_schema: serde_json::Value,
}

impl McpTool {
    /// The name the model uses for this tool (see [`qualified_name`]).
    #[must_use]
    pub fn qualified_name(&self) -> String {
        qualified_name(&self.server, &self.name)
    }
}

/// Result of an MCP tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolResult {
    /// Content items returned by the tool.
    pub content: Vec<McpContent>,
    /// Whether the tool reported a failure (the call itself succeeded).
    pub is_error: bool,
}

impl McpToolResult {
    /// Converts the content to the domain shape sent back to the model.
    /// Embedded resources become text prefixed with their URI.
    #[must_use]
    pub fn to_tool_result_content(&self) -> Vec<ToolResultContent> {
        self.content
            .iter()
            .map(|item| match item {
                McpContent::Text { text } => ToolResultContent::text(text.clone()),
                McpContent::Image { media_type, data } => ToolResultContent::Image {
                    media_type: media_type.clone(),
                    data: data.clone(),
                },
                McpContent::Resource { uri, text } => {
                    ToolResultContent::text(format!("{uri}\n{}", text.as_deref().unwrap_or("")))
                }
            })
            .collect()
    }
}

/// One content item of an MCP tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// An inline image.
    Image {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Base64-encoded bytes.
        data: String,
    },
    /// An embedded resource.
    Resource {
        /// Resource URI.
        uri: String,
        /// Text contents, if the resource is textual.
        text: Option<String>,
    },
}

/// Connection state of a configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    /// Server name, as declared in the Lua config.
    pub name: String,
    /// Current state.
    pub state: McpServerState,
}

/// Lifecycle of an MCP server connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum McpServerState {
    /// Starting or handshaking.
    Starting,
    /// Connected and serving tools.
    Ready {
        /// Number of tools it advertises.
        tool_count: usize,
    },
    /// Failed to start or lost the connection.
    Failed {
        /// What went wrong.
        message: String,
    },
    /// Stopped on purpose (disabled in config or shut down).
    Stopped,
}

/// Errors of [`McpPort`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// No server with this name is configured.
    #[error("unknown MCP server `{0}`")]
    UnknownServer(String),
    /// The server does not advertise this tool.
    #[error("MCP server `{server}` has no tool `{name}`")]
    UnknownTool {
        /// Server name.
        server: String,
        /// Tool name.
        name: String,
    },
    /// The server is configured but not ready.
    #[error("MCP server `{server}` is unavailable: {message}")]
    Unavailable {
        /// Server name.
        server: String,
        /// Why (still starting, crashed, ...).
        message: String,
    },
    /// The input does not match the tool's schema.
    #[error("invalid input for MCP tool `{name}`: {message}")]
    InvalidInput {
        /// Tool name.
        name: String,
        /// Validation message.
        message: String,
    },
    /// Process, stdio or HTTP failure talking to the server.
    #[error("transport error with MCP server `{server}`: {message}")]
    Transport {
        /// Server name.
        server: String,
        /// Underlying error.
        message: String,
    },
    /// The server answered with a JSON-RPC error or a malformed message.
    #[error("protocol error from MCP server `{server}`: {message}")]
    Protocol {
        /// Server name.
        server: String,
        /// Error message.
        message: String,
    },
    /// The server did not answer in time.
    #[error("MCP server `{server}` timed out")]
    Timeout {
        /// Server name.
        server: String,
    },
    /// The call was cancelled.
    #[error("cancelled")]
    Cancelled,
}

/// Exposes the tools of the configured MCP servers (stdio and HTTP).
///
/// The port only lists and calls tools; approvals are applied by the use
/// case exactly as for built-in tools.
///
/// Cancellation: drop the future. The adapter sends an MCP cancellation
/// notification for an in-flight call when possible.
#[async_trait]
pub trait McpPort: Send + Sync {
    /// Tools of every ready server. Servers that are not ready are skipped
    /// (their state is visible through [`servers`](Self::servers)).
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError>;

    /// Calls `name` on `server` with a JSON `input`. Both names are the
    /// unqualified ones (see [`parse_qualified_name`]).
    async fn call_tool(
        &self,
        server: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError>;

    /// State of every configured server, in config order.
    async fn servers(&self) -> Vec<McpServerStatus>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_round_trips() {
        let name = qualified_name("github", "create_issue");
        assert_eq!(name, "mcp__github__create_issue");
        assert_eq!(
            parse_qualified_name(&name),
            Some(("github", "create_issue"))
        );
    }

    #[test]
    fn tool_names_may_contain_the_separator() {
        let name = qualified_name("fs", "read__raw");
        assert_eq!(parse_qualified_name(&name), Some(("fs", "read__raw")));
    }

    #[test]
    fn non_mcp_names_do_not_parse() {
        assert_eq!(parse_qualified_name("grep"), None);
        assert_eq!(parse_qualified_name("mcp__github"), None);
        assert_eq!(parse_qualified_name("mcp____tool"), None);
        assert_eq!(parse_qualified_name("mcp__github__"), None);
    }

    #[test]
    fn result_content_maps_to_domain() {
        let result = McpToolResult {
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
            ],
            is_error: false,
        };
        assert_eq!(
            result.to_tool_result_content(),
            vec![
                ToolResultContent::text("ok"),
                ToolResultContent::Image {
                    media_type: "image/png".into(),
                    data: "iVBO".into(),
                },
                ToolResultContent::text("file:///a.txt\nhola"),
            ]
        );
    }

    struct Echo;

    #[async_trait]
    impl McpPort for Echo {
        async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![McpTool {
                server: "demo".into(),
                name: "echo".into(),
                description: "Echoes its input".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }])
        }

        async fn call_tool(
            &self,
            server: &str,
            name: &str,
            input: serde_json::Value,
        ) -> Result<McpToolResult, McpError> {
            if server != "demo" {
                return Err(McpError::UnknownServer(server.into()));
            }
            assert_eq!(name, "echo");
            Ok(McpToolResult {
                content: vec![McpContent::Text {
                    text: input.to_string(),
                }],
                is_error: false,
            })
        }

        async fn servers(&self) -> Vec<McpServerStatus> {
            vec![McpServerStatus {
                name: "demo".into(),
                state: McpServerState::Ready { tool_count: 1 },
            }]
        }
    }

    #[tokio::test]
    async fn port_routes_qualified_calls() {
        let port: std::sync::Arc<dyn McpPort> = std::sync::Arc::new(Echo);
        let tools = port.list_tools().await.unwrap();
        let qualified = tools[0].qualified_name();
        let (server, name) = parse_qualified_name(&qualified).unwrap();
        let result = port
            .call_tool(server, name, serde_json::json!({ "x": 1 }))
            .await
            .unwrap();
        assert_eq!(
            result.content,
            vec![McpContent::Text {
                text: "{\"x\":1}".into()
            }]
        );
        assert_eq!(
            port.call_tool("other", "echo", serde_json::Value::Null)
                .await,
            Err(McpError::UnknownServer("other".into()))
        );
        assert_eq!(port.servers().await.len(), 1);
    }
}
