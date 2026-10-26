//! Tools of MCP servers, offered to the model like any built-in tool.
//!
//! [`McpTool`] wraps a tool advertised through the [`McpPort`] under its
//! qualified name, `mcp__<server>__<tool>`, and calls it through the port.
//! Its effect is [`ToolEffect::Execute`] unless told otherwise, so every
//! call goes through the same approval as running a command.
//! [`register_mcp_tools`] adds the tools of every ready server, except
//! those whose MCP Apps visibility leaves the model out (`_meta.ui.
//! visibility` without `"model"`: only their own HTML view may call them,
//! and crest runs no such view).
//!
//! The model gets a result's text content as before (items annotated for
//! the user only are left out); the user also gets a block drawn from its
//! `structuredContent` and user-facing items, through
//! [`crate::interop::structured`].

use std::sync::Arc;

use async_trait::async_trait;
use domain::permission::ToolEffect;
use futures::FutureExt;
use serde_json::Value;

use super::registry::{PreparedCall, RegisterError, Tool, ToolContext, ToolError, ToolOutput};
use crate::ports::approval::ApprovalPreview;
use crate::ports::mcp::{self, McpError, McpPort};

/// Text sent to the model when a tool returns no content.
const NO_OUTPUT: &str = "(no output)";

/// A tool of an MCP server, called through the [`McpPort`].
pub struct McpTool {
    spec: mcp::McpTool,
    qualified: String,
    description: String,
    effect: ToolEffect,
    port: Arc<dyn McpPort>,
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool")
            .field("spec", &self.spec)
            .field("effect", &self.effect)
            .finish_non_exhaustive()
    }
}

impl McpTool {
    /// The tool described by `spec`, called through `port`, with effect
    /// [`ToolEffect::Execute`].
    #[must_use]
    pub fn new(spec: mcp::McpTool, port: Arc<dyn McpPort>) -> Self {
        let description = if spec.description.trim().is_empty() {
            format!("Tool `{}` of the MCP server `{}`.", spec.name, spec.server)
        } else {
            spec.description.clone()
        };
        Self {
            qualified: spec.qualified_name(),
            description,
            effect: ToolEffect::Execute,
            spec,
            port,
        }
    }

    /// Returns the same tool declaring `effect` instead.
    #[must_use]
    pub fn with_effect(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }

    /// The tool as advertised by its server.
    #[must_use]
    pub fn spec(&self) -> &mcp::McpTool {
        &self.spec
    }
}

/// The tool error for a failed call to `qualified`.
fn to_tool_error(qualified: &str, err: McpError) -> ToolError {
    match err {
        McpError::Cancelled => ToolError::Cancelled,
        McpError::InvalidInput { message, .. } => ToolError::InvalidInput {
            tool: qualified.to_owned(),
            message,
        },
        other => ToolError::Execution(format!("MCP tool `{qualified}`: {other}")),
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.spec.input_schema.clone()
    }

    fn effect(&self, _input: &Value) -> ToolEffect {
        self.effect
    }

    async fn prepare(&self, input: Value, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let input = match input {
            Value::Null => Value::Object(serde_json::Map::new()),
            Value::Object(_) => input,
            _ => {
                return Err(ToolError::InvalidInput {
                    tool: self.qualified.clone(),
                    message: "the input must be an object".into(),
                });
            }
        };
        let subject = input.to_string();
        let preview = ApprovalPreview::Text {
            text: format!(
                "MCP server `{}`: {} {subject}",
                self.spec.server, self.spec.name
            ),
        };
        let port = Arc::clone(&self.port);
        let spec = self.spec.clone();
        let server = self.spec.server.clone();
        let name = self.spec.name.clone();
        let qualified = self.qualified.clone();
        let run = async move {
            let result = port
                .call_tool(&server, &name, input)
                .await
                .map_err(|err| to_tool_error(&qualified, err))?;
            let mut content = result.to_tool_result_content();
            if content.is_empty() {
                content.push(domain::conversation::ToolResultContent::text(NO_OUTPUT));
            }
            Ok(ToolOutput {
                content,
                is_error: result.is_error,
                block_update: crate::interop::structured::render(&spec, &result),
            })
        };
        Ok(PreparedCall::new(subject, run.boxed()).with_preview(preview))
    }
}

/// Registers the tools of every ready server of `port` as [`McpTool`]s.
///
/// Returns the tools that could not be registered (a name taken by another
/// tool, or one the model would reject); the rest are registered anyway.
///
/// # Errors
///
/// The error of [`McpPort::list_tools`].
pub async fn register_mcp_tools(
    registry: &mut super::registry::ToolRegistry,
    port: Arc<dyn McpPort>,
) -> Result<Vec<RegisterError>, McpError> {
    let mut skipped = Vec::new();
    for spec in port.list_tools().await? {
        if !spec.ui.model {
            tracing::info!(
                target: "crest::mcp",
                "MCP tool {} not offered: its visibility is app-only",
                spec.qualified_name()
            );
            continue;
        }
        let tool = McpTool::new(spec, Arc::clone(&port));
        if let Err(err) = registry.register(Arc::new(tool)) {
            tracing::warn!(target: "crest::mcp", "MCP tool not offered: {err}");
            skipped.push(err);
        }
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::ports::mcp::{McpContent, McpServerState, McpToolResult};
    use crate::testing::llm::{FakeMcp, McpCall, text_result};
    use crate::tools::registry::ToolRegistry;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    async fn registry(fake: FakeMcp) -> (ToolRegistry, Arc<FakeMcp>, Vec<RegisterError>) {
        let fake = Arc::new(fake);
        let mut registry = ToolRegistry::new();
        let skipped = register_mcp_tools(&mut registry, fake.clone())
            .await
            .unwrap();
        (registry, fake, skipped)
    }

    #[tokio::test]
    async fn registers_ready_tools_under_qualified_names() {
        let fake = FakeMcp::new()
            .with_text_tool("github", "create_issue", "ok")
            .with_tool("echo", "add", "", |_| Ok(text_result("3")))
            .with_text_tool("down", "x", "never")
            .with_server_state(
                "down",
                McpServerState::Failed {
                    message: "boom".into(),
                },
            );
        let (registry, _, skipped) = registry(fake).await;
        assert!(skipped.is_empty());
        assert_eq!(
            registry.names(),
            ["mcp__echo__add", "mcp__github__create_issue"]
        );
        let defs = registry.definitions();
        assert_eq!(defs[0].description, "Tool `add` of the MCP server `echo`.");
        assert_eq!(defs[1].description, "Fake MCP tool");
        assert_eq!(defs[1].input_schema, json!({ "type": "object" }));
        let tool = registry.get("mcp__echo__add").unwrap();
        assert_eq!(tool.effect(&json!({})), ToolEffect::Execute);
    }

    #[tokio::test]
    async fn invalid_or_duplicate_names_are_skipped() {
        let fake = FakeMcp::new()
            .with_text_tool("s", "read.file", "x")
            .with_text_tool("s", "ok", "x");
        let fake = Arc::new(fake);
        let mut registry = ToolRegistry::new();
        let skipped = register_mcp_tools(&mut registry, fake.clone())
            .await
            .unwrap();
        assert_eq!(
            skipped,
            [RegisterError::InvalidName("mcp__s__read.file".into())]
        );
        let again = register_mcp_tools(&mut registry, fake).await.unwrap();
        assert_eq!(again.len(), 2);
        assert_eq!(registry.names(), ["mcp__s__ok"]);
    }

    #[tokio::test]
    async fn calls_go_through_the_port() {
        let fake = FakeMcp::new().with_tool("echo", "add", "Adds", |input| {
            let sum = input["a"].as_i64().unwrap() + input["b"].as_i64().unwrap();
            Ok(text_result(sum.to_string()))
        });
        let (registry, fake, _) = registry(fake).await;
        let input = json!({ "a": 2, "b": 3 });
        let out = registry
            .dispatch("mcp__echo__add", input.clone(), &ctx())
            .await
            .unwrap();
        assert_eq!(out, ToolOutput::text("5"));
        assert_eq!(
            fake.calls(),
            [McpCall {
                server: "echo".into(),
                name: "add".into(),
                input
            }]
        );
    }

    #[tokio::test]
    async fn results_and_errors_reach_the_model() {
        let fake = FakeMcp::new()
            .with_tool("s", "fails", "", |_| {
                Ok(McpToolResult {
                    content: vec![McpContent::text("no")],
                    is_error: true,
                    ..McpToolResult::default()
                })
            })
            .with_tool("s", "empty", "", |_| Ok(McpToolResult::default()))
            .with_tool("s", "broken", "", |_| {
                Err(McpError::Timeout { server: "s".into() })
            })
            .with_tool("s", "stopped", "", |_| Err(McpError::Cancelled));
        let (registry, _, _) = registry(fake).await;
        let out = registry
            .dispatch("mcp__s__fails", json!({}), &ctx())
            .await
            .unwrap();
        assert_eq!(out, ToolOutput::error("no"));
        let out = registry
            .dispatch("mcp__s__empty", Value::Null, &ctx())
            .await
            .unwrap();
        assert_eq!(out, ToolOutput::text(NO_OUTPUT));
        let err = registry
            .dispatch("mcp__s__broken", json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ToolError::Execution("MCP tool `mcp__s__broken`: MCP server `s` timed out".into())
        );
        let err = registry
            .dispatch("mcp__s__stopped", json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err, ToolError::Cancelled);
        let err = registry
            .dispatch("mcp__s__fails", json!([1]), &ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn app_only_tools_are_hidden_and_structured_results_drawn() {
        let spec = |name: &str, ui: mcp::McpToolUi| mcp::McpTool {
            server: "shop".into(),
            name: name.into(),
            description: String::new(),
            input_schema: json!({ "type": "object" }),
            output_schema: Some(json!({ "type": "object" })),
            ui,
        };
        let app_only = mcp::McpToolUi {
            model: false,
            ..mcp::McpToolUi::default()
        };
        let fake = FakeMcp::new()
            .with_spec(spec("refresh", app_only), |_| Ok(text_result("x")))
            .with_spec(spec("orders", mcp::McpToolUi::default()), |_| {
                Ok(McpToolResult {
                    content: vec![McpContent::text("2 orders")],
                    structured_content: Some(json!([{ "id": "A1" }, { "id": "B2" }])),
                    ..McpToolResult::default()
                })
            });
        let (registry, _, skipped) = registry(fake).await;
        assert!(skipped.is_empty());
        assert_eq!(registry.names(), ["mcp__shop__orders"]);
        let out = registry
            .dispatch("mcp__shop__orders", json!({}), &ctx())
            .await
            .unwrap();
        assert_eq!(out.text_content(), "2 orders");
        let Some(domain::block::BlockKind::Custom { tree, .. }) = &out.block_update else {
            panic!("{out:?}");
        };
        assert!(
            matches!(tree, domain::block::CustomNode::Table(_)),
            "{tree:?}"
        );
    }

    #[tokio::test]
    async fn the_preview_names_the_server_and_the_effect_can_change() {
        let fake: Arc<dyn McpPort> = Arc::new(FakeMcp::new());
        let spec = mcp::McpTool {
            server: "github".into(),
            name: "list_issues".into(),
            description: "Lists issues".into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            ui: mcp::McpToolUi::default(),
        };
        let tool = McpTool::new(spec, fake);
        let prepared = tool.prepare(json!({ "repo": "x" }), &ctx()).await.unwrap();
        assert_eq!(prepared.subject, r#"{"repo":"x"}"#);
        assert_eq!(
            prepared.preview,
            Some(ApprovalPreview::Text {
                text: r#"MCP server `github`: list_issues {"repo":"x"}"#.into()
            })
        );
        let tool = tool.with_effect(ToolEffect::Network);
        assert_eq!(tool.effect(&json!({})), ToolEffect::Network);
        assert_eq!(tool.spec().name, "list_issues");
    }
}
