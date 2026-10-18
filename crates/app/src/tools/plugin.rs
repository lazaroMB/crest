//! Tools registered by Lua plugins (`app.tools.register`), offered to the
//! model like any built-in tool.
//!
//! [`PluginTool`] wraps a [`PluginToolSpec`] and runs its handler through
//! the [`PluginHostPort`]. The handler's result becomes the `tool_result`:
//! a string as is, `nil` as `(no output)`, any other value as JSON, and a
//! block as a timeline block. Lua errors and calls stopped by the
//! instruction limit come back to the model as errors.

use std::sync::Arc;

use async_trait::async_trait;
use domain::permission::ToolEffect;
use futures::FutureExt;
use serde_json::Value;

use super::registry::{PreparedCall, Tool, ToolContext, ToolError, ToolOutput};
use crate::ports::approval::ApprovalPreview;
use crate::ports::plugin::{PluginHostPort, PluginOutput, PluginToolSpec};

/// Text sent to the model when a handler returns nothing.
const NO_OUTPUT: &str = "(no output)";

/// A plugin tool, run by the Lua host.
pub struct PluginTool {
    spec: PluginToolSpec,
    effect: ToolEffect,
    host: Arc<dyn PluginHostPort>,
}

impl std::fmt::Debug for PluginTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginTool")
            .field("spec", &self.spec)
            .field("effect", &self.effect)
            .finish_non_exhaustive()
    }
}

impl PluginTool {
    /// The tool described by `spec`, run through `host`, with the effect
    /// the spec declares.
    #[must_use]
    pub fn new(spec: PluginToolSpec, host: Arc<dyn PluginHostPort>) -> Self {
        Self {
            effect: spec.effect,
            spec,
            host,
        }
    }

    /// Returns the same tool declaring `effect` instead.
    #[must_use]
    pub fn with_effect(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }

    /// The spec it was built from.
    #[must_use]
    pub fn spec(&self) -> &PluginToolSpec {
        &self.spec
    }
}

/// Converts what a handler returned into a tool output.
fn to_output(name: &str, output: PluginOutput) -> Result<ToolOutput, ToolError> {
    match output {
        PluginOutput::Value(Value::Null) => Ok(ToolOutput::text(NO_OUTPUT)),
        PluginOutput::Value(Value::String(text)) => Ok(ToolOutput::text(text)),
        PluginOutput::Value(value) => Ok(ToolOutput::text(value.to_string())),
        PluginOutput::Block(block) => {
            Ok(ToolOutput::text("[shown to the user as a block]").with_block_update(block))
        }
        PluginOutput::Actions(_) => Err(ToolError::Execution(format!(
            "plugin tool `{name}` returned actions, which a tool cannot perform"
        ))),
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn input_schema(&self) -> Value {
        self.spec.input_schema.clone()
    }

    fn effect(&self, _input: &Value) -> ToolEffect {
        self.effect
    }

    async fn prepare(&self, input: Value, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        if !input.is_object() {
            return Err(ToolError::InvalidInput {
                tool: self.spec.name.clone(),
                message: "the input must be an object".into(),
            });
        }
        let subject = input.to_string();
        let preview = ApprovalPreview::Text {
            text: format!(
                "plugin `{}`: {} {subject}",
                self.spec.plugin, self.spec.name
            ),
        };
        let host = Arc::clone(&self.host);
        let name = self.spec.name.clone();
        let run = async move {
            let output = host
                .call_tool(&name, input)
                .await
                .map_err(|err| ToolError::Execution(format!("plugin tool `{name}`: {err}")))?;
            to_output(&name, output)
        };
        Ok(PreparedCall::new(subject, run.boxed()).with_preview(preview))
    }
}

#[cfg(test)]
mod tests {
    use domain::block::BlockKind;
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::ports::plugin::PluginError;
    use crate::testing::system::{FakePluginHost, PluginInvocation};
    use crate::tools::registry::ToolRegistry;

    fn spec(name: &str) -> PluginToolSpec {
        PluginToolSpec {
            name: name.into(),
            description: "Lists tasks.".into(),
            input_schema: json!({ "type": "object", "properties": { "items": { "type": "array" } } }),
            plugin: "todo".into(),
            effect: ToolEffect::Execute,
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn registry(host: FakePluginHost) -> (ToolRegistry, Arc<FakePluginHost>) {
        let host = Arc::new(host);
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(PluginTool::new(spec("todo_list"), host.clone())))
            .unwrap();
        (registry, host)
    }

    async fn dispatch(host: FakePluginHost, input: Value) -> Result<ToolOutput, ToolError> {
        let (registry, _) = registry(host);
        registry.dispatch("todo_list", input, &ctx()).await
    }

    #[test]
    fn offers_the_spec_to_the_model_and_executes_by_default() {
        let (registry, _) = registry(FakePluginHost::new());
        let defs = registry.definitions();
        assert_eq!(defs[0].name, "todo_list");
        assert_eq!(defs[0].description, "Lists tasks.");
        assert_eq!(defs[0].input_schema, spec("x").input_schema);
        let tool = registry.get("todo_list").unwrap();
        assert_eq!(tool.effect(&json!({})), ToolEffect::Execute);
        let declared = PluginToolSpec {
            effect: ToolEffect::Read,
            ..spec("t")
        };
        let read = PluginTool::new(declared, Arc::new(FakePluginHost::new()));
        assert_eq!(read.effect(&json!({})), ToolEffect::Read);
        let write = read.with_effect(ToolEffect::Write);
        assert_eq!(write.effect(&json!({})), ToolEffect::Write);
    }

    #[tokio::test]
    async fn runs_the_handler_with_the_input() {
        let (registry, host) = registry(FakePluginHost::new().on_tool("todo_list", |input| {
            Ok(PluginOutput::Value(input["items"].clone()))
        }));
        let input = json!({ "items": ["a", "b"] });
        let out = registry
            .dispatch("todo_list", input.clone(), &ctx())
            .await
            .unwrap();
        assert_eq!(out, ToolOutput::text(r#"["a","b"]"#));
        assert_eq!(
            host.invocations(),
            [PluginInvocation::Tool {
                name: "todo_list".into(),
                input
            }]
        );
    }

    #[tokio::test]
    async fn strings_go_as_is_and_nil_is_no_output() {
        let text =
            FakePluginHost::new().on_tool("todo_list", |_| Ok(PluginOutput::Value(json!("hecho"))));
        assert_eq!(
            dispatch(text, json!({})).await.unwrap(),
            ToolOutput::text("hecho")
        );
        let nil =
            FakePluginHost::new().on_tool("todo_list", |_| Ok(PluginOutput::Value(Value::Null)));
        assert_eq!(
            dispatch(nil, json!({})).await.unwrap(),
            ToolOutput::text(NO_OUTPUT)
        );
    }

    #[tokio::test]
    async fn blocks_become_block_updates() {
        let block = BlockKind::Text {
            markdown: "- a".into(),
        };
        let shown = block.clone();
        let host = FakePluginHost::new()
            .on_tool("todo_list", move |_| Ok(PluginOutput::Block(shown.clone())));
        let out = dispatch(host, json!({})).await.unwrap();
        assert_eq!(out.block_update, Some(block));
        assert!(!out.is_error);
    }

    #[tokio::test]
    async fn lua_failures_are_execution_errors() {
        for error in [
            PluginError::Runtime("init.lua:3: boom".into()),
            PluginError::InstructionLimit,
        ] {
            let message = error.to_string();
            let host = FakePluginHost::new().on_tool("todo_list", move |_| Err(error.clone()));
            let err = dispatch(host, json!({})).await.unwrap_err();
            assert_eq!(
                err,
                ToolError::Execution(format!("plugin tool `todo_list`: {message}"))
            );
            assert!(ToolOutput::from(err).is_error);
        }
    }

    #[tokio::test]
    async fn rejects_non_object_input_and_actions() {
        let err = dispatch(FakePluginHost::new(), json!([1]))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
        let host =
            FakePluginHost::new().on_tool("todo_list", |_| Ok(PluginOutput::Actions(Vec::new())));
        let err = dispatch(host, json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)), "{err:?}");
    }

    #[tokio::test]
    async fn the_subject_is_the_input_and_the_preview_names_the_plugin() {
        let tool = PluginTool::new(spec("todo_list"), Arc::new(FakePluginHost::new()));
        let prepared = tool.prepare(json!({ "items": [] }), &ctx()).await.unwrap();
        assert_eq!(prepared.subject, r#"{"items":[]}"#);
        assert_eq!(
            prepared.preview,
            Some(ApprovalPreview::Text {
                text: r#"plugin `todo`: todo_list {"items":[]}"#.into()
            })
        );
    }
}
