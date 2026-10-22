//! The tools the model can call: the [`Tool`] trait, its typed helper
//! [`TypedTool`] and the [`ToolRegistry`] that dispatches calls by name.
//!
//! A call runs in two phases so the agent loop can ask for approval in
//! between: [`Tool::prepare`] validates the input and computes what the call
//! would do (its subject, the diff or command to show), and the returned
//! [`PreparedCall::run`] future performs it. Dropping that future cancels the
//! call; adapters kill the processes they started when that happens.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::conversation::{SessionId, ToolResultContent, ToolUseId, TurnId};
use domain::permission::ToolEffect;
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::budget::ResultBudget;
use crate::ports::approval::ApprovalPreview;
use crate::ports::llm::ToolDefinition;
use crate::privacy::Redactor;

/// Where a tool call happens; handed to [`Tool::prepare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContext {
    /// Session of the call.
    pub session_id: SessionId,
    /// Turn of the call.
    pub turn_id: TurnId,
    /// Provider id of the `tool_use` being answered.
    pub tool_use_id: ToolUseId,
}

impl ToolContext {
    /// A context for `tool_use_id` in the given session and turn.
    #[must_use]
    pub fn new(session_id: SessionId, turn_id: TurnId, tool_use_id: ToolUseId) -> Self {
        Self {
            session_id,
            turn_id,
            tool_use_id,
        }
    }
}

/// Why a tool call failed. Every variant becomes a `tool_result` with
/// `is_error` for the model; none aborts the turn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    /// No tool is registered under this name.
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    /// The input does not match the tool's schema.
    #[error("invalid input for tool `{tool}`: {message}")]
    InvalidInput {
        /// Tool name.
        tool: String,
        /// Deserializer message (field and expected type).
        message: String,
    },
    /// The tool ran and failed.
    #[error("{0}")]
    Execution(String),
    /// The call was denied by the permission policy or the user.
    #[error("{0}")]
    Denied(String),
    /// The call was cancelled before it finished.
    #[error("cancelled by user")]
    Cancelled,
}

/// What a finished tool call returns to the model and to the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Content of the `tool_result` sent to the model.
    pub content: Vec<ToolResultContent>,
    /// Whether the call failed (reported to the model as `is_error`).
    pub is_error: bool,
    /// New version of the block the call added with
    /// [`PreparedCall::with_block`], or a block to append when it added
    /// none (e.g. a chart from `show_chart`).
    pub block_update: Option<BlockKind>,
}

impl ToolOutput {
    /// A successful output with a single text item.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent::text(text)],
            is_error: false,
            block_update: None,
        }
    }

    /// A failed output with a single text item.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }

    /// Returns the same output with a block update.
    #[must_use]
    pub fn with_block_update(mut self, block: BlockKind) -> Self {
        self.block_update = Some(block);
        self
    }

    /// The text items joined by newlines; images are skipped.
    #[must_use]
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|item| match item {
                ToolResultContent::Text { text } => Some(text.as_str()),
                ToolResultContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl From<ToolError> for ToolOutput {
    fn from(err: ToolError) -> Self {
        Self::error(err.to_string())
    }
}

/// Future that performs a prepared call.
pub type ToolRun = BoxFuture<'static, Result<ToolOutput, ToolError>>;

/// A validated call, ready to be approved and run.
pub struct PreparedCall {
    /// What the call acts on (workspace path, command line, URL). The
    /// permission policy and "allow always" rules match against it, and the
    /// timeline shows it as the call's summary.
    pub subject: String,
    /// What to show in the approval dialog; `None` shows the subject.
    pub preview: Option<ApprovalPreview>,
    /// Extra block to add to the timeline before running (e.g. a `Diff`).
    pub block: Option<BlockKind>,
    /// Performs the call. Not polled until the call is approved; dropping
    /// it cancels the call.
    pub run: ToolRun,
}

impl PreparedCall {
    /// A call on `subject` performed by `run`, without preview or block.
    #[must_use]
    pub fn new(subject: impl Into<String>, run: ToolRun) -> Self {
        Self {
            subject: subject.into(),
            preview: None,
            block: None,
            run,
        }
    }

    /// Returns the same call with an approval preview.
    #[must_use]
    pub fn with_preview(mut self, preview: ApprovalPreview) -> Self {
        self.preview = Some(preview);
        self
    }

    /// Returns the same call with an extra timeline block.
    #[must_use]
    pub fn with_block(mut self, block: BlockKind) -> Self {
        self.block = Some(block);
        self
    }
}

impl fmt::Debug for PreparedCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedCall")
            .field("subject", &self.subject)
            .field("preview", &self.preview)
            .field("block", &self.block)
            .finish_non_exhaustive()
    }
}

/// A tool the model can call.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Name the model calls it by (`^[a-zA-Z0-9_-]{1,128}$`).
    fn name(&self) -> &str;

    /// When and how to use it, written for the model.
    fn description(&self) -> &str;

    /// JSON Schema of the input.
    fn input_schema(&self) -> Value;

    /// Side effect of a call with `input`, for the permission policy.
    fn effect(&self, input: &Value) -> ToolEffect;

    /// Validates `input` and prepares the call without performing it.
    ///
    /// # Errors
    ///
    /// [`ToolError::InvalidInput`] when the input does not match the schema,
    /// or [`ToolError::Execution`] when the call cannot be prepared (e.g.
    /// the file to edit does not exist).
    async fn prepare(&self, input: Value, ctx: &ToolContext) -> Result<PreparedCall, ToolError>;
}

/// A tool with a typed input: the input is deserialized with `serde` and
/// its schema derived with `schemars`. Register it wrapped in [`Typed`].
#[async_trait]
pub trait TypedTool: Send + Sync {
    /// Input of a call.
    type Input: DeserializeOwned + JsonSchema + Send + 'static;

    /// Name the model calls it by.
    fn name(&self) -> &str;

    /// When and how to use it, written for the model.
    fn description(&self) -> &str;

    /// Side effect of a call with `input`.
    fn effect(&self, input: &Self::Input) -> ToolEffect;

    /// Prepares the call without performing it.
    ///
    /// # Errors
    ///
    /// As [`Tool::prepare`].
    async fn prepare(
        &self,
        input: Self::Input,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError>;
}

/// Adapts a [`TypedTool`] to [`Tool`].
#[derive(Debug, Clone)]
pub struct Typed<T>(pub T);

impl<T: TypedTool> Typed<T> {
    fn parse(&self, input: Value) -> Result<T::Input, ToolError> {
        serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
            tool: self.0.name().to_owned(),
            message: err.to_string(),
        })
    }
}

#[async_trait]
impl<T: TypedTool> Tool for Typed<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn input_schema(&self) -> Value {
        schemars::schema_for!(T::Input).to_value()
    }

    /// An input that does not parse is treated as the strictest effect;
    /// [`prepare`](Tool::prepare) rejects it anyway.
    fn effect(&self, input: &Value) -> ToolEffect {
        self.parse(input.clone())
            .map_or(ToolEffect::Execute, |input| self.0.effect(&input))
    }

    async fn prepare(&self, input: Value, ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let input = self.parse(input)?;
        self.0.prepare(input, ctx).await
    }
}

/// Why a tool could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// Another tool already has this name.
    #[error("tool `{0}` is already registered")]
    Duplicate(String),
    /// The name does not match `^[a-zA-Z0-9_-]{1,128}$`.
    #[error("invalid tool name `{0}` (expected 1-128 of [a-zA-Z0-9_-])")]
    InvalidName(String),
}

fn valid_name(name: &str) -> bool {
    (1..=128).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Tools that come and go while the session runs (those of MCP servers
/// that connect, reconnect or change their list). The registry asks for
/// them on every lookup; a name taken by a registered tool wins.
pub trait ToolSource: Send + Sync {
    /// The tools available right now.
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

/// The tools offered to the model, by name, the redactor their results
/// go through and the budget they are truncated to.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    sources: Vec<Arc<dyn ToolSource>>,
    budget: ResultBudget,
    redactor: Redactor,
}

impl ToolRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the same registry truncating results to `budget`.
    #[must_use]
    pub fn with_budget(mut self, budget: ResultBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Budget applied to every tool result.
    #[must_use]
    pub fn budget(&self) -> ResultBudget {
        self.budget
    }

    /// Returns the same registry masking results with `redactor`
    /// (by default every built-in rule is on).
    #[must_use]
    pub fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// Redactor applied to every tool result, before the budget.
    #[must_use]
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// `output` masked by the [`redactor`](Self::redactor), then cut to
    /// the [`budget`](Self::budget): what the model and the timeline get.
    #[must_use]
    pub fn finish(&self, output: ToolOutput) -> ToolOutput {
        self.budget.apply(self.redactor.redact_output(output))
    }

    /// Adds `tool`.
    ///
    /// # Errors
    ///
    /// [`RegisterError`] when the name is invalid or already taken.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), RegisterError> {
        let name = tool.name().to_owned();
        if !valid_name(&name) {
            return Err(RegisterError::InvalidName(name));
        }
        if self.tools.contains_key(&name) {
            return Err(RegisterError::Duplicate(name));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Adds `source`, whose tools are offered next to the registered ones.
    pub fn add_source(&mut self, source: Arc<dyn ToolSource>) {
        self.sources.push(source);
    }

    /// The tools of the sources that are valid and not shadowed, by name.
    fn dynamic(&self) -> BTreeMap<String, Arc<dyn Tool>> {
        let mut out = BTreeMap::new();
        for tool in self.sources.iter().flat_map(|source| source.tools()) {
            let name = tool.name().to_owned();
            if valid_name(&name) && !self.tools.contains_key(&name) {
                out.entry(name).or_insert(tool);
            }
        }
        out
    }

    /// Every tool, registered and dynamic, by name.
    fn all(&self) -> BTreeMap<String, Arc<dyn Tool>> {
        let mut all = self.dynamic();
        all.extend(self.tools.iter().map(|(k, v)| (k.clone(), Arc::clone(v))));
        all
    }

    /// Adds a typed tool.
    ///
    /// # Errors
    ///
    /// As [`register`](Self::register).
    pub fn register_typed<T: TypedTool + 'static>(&mut self, tool: T) -> Result<(), RegisterError> {
        self.register(Arc::new(Typed(tool)))
    }

    /// The tool called `name`, registered or from a source.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Some(Arc::clone(tool));
        }
        self.sources
            .iter()
            .flat_map(|source| source.tools())
            .find(|tool| tool.name() == name)
    }

    /// Names of every tool, sorted.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.all().into_keys().collect()
    }

    /// Number of tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all().len()
    }

    /// Whether there is no tool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.dynamic().is_empty()
    }

    /// Definitions for [`LlmRequest::tools`](crate::ports::llm::LlmRequest),
    /// sorted by name so the prompt cache sees the same prefix every turn.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.all()
            .values()
            .map(|tool| ToolDefinition::new(tool.name(), tool.description(), tool.input_schema()))
            .collect()
    }

    /// Prepares a call to the tool `name`.
    ///
    /// # Errors
    ///
    /// [`ToolError::UnknownTool`] if there is no such tool, or the error of
    /// [`Tool::prepare`].
    pub async fn prepare(
        &self,
        name: &str,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let tool = self
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_owned()))?;
        tool.prepare(input, ctx).await
    }

    /// Prepares and runs a call to `name` without asking for approval,
    /// masking and truncating its output as [`finish`](Self::finish) does.
    ///
    /// # Errors
    ///
    /// As [`prepare`](Self::prepare), or the error of the run.
    pub async fn dispatch(
        &self,
        name: &str,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let output = self.prepare(name, input, ctx).await?.run.await?;
        Ok(self.finish(output))
    }
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .field("budget", &self.budget)
            .field("redactor", &self.redactor.enabled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, JsonSchema)]
    struct EchoInput {
        /// Text to echo.
        text: String,
    }

    struct Echo(&'static str);

    #[async_trait]
    impl TypedTool for Echo {
        type Input = EchoInput;

        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "Echoes its input."
        }

        fn effect(&self, _input: &EchoInput) -> ToolEffect {
            ToolEffect::Read
        }

        async fn prepare(
            &self,
            input: EchoInput,
            _ctx: &ToolContext,
        ) -> Result<PreparedCall, ToolError> {
            let subject = input.text.clone();
            Ok(PreparedCall::new(
                subject,
                async move { Ok(ToolOutput::text(input.text)) }.boxed(),
            ))
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_typed(Echo("echo")).unwrap();
        registry
    }

    #[tokio::test]
    async fn dispatches_by_name() {
        let out = registry()
            .dispatch("echo", json!({ "text": "hola" }), &ctx())
            .await
            .unwrap();
        assert_eq!(out, ToolOutput::text("hola"));
    }

    #[tokio::test]
    async fn unknown_tool_is_a_typed_error() {
        let err = registry()
            .dispatch("nope", json!({}), &ctx())
            .await
            .unwrap_err();
        assert_eq!(err, ToolError::UnknownTool("nope".into()));
    }

    #[tokio::test]
    async fn invalid_input_reports_the_serde_detail() {
        let err = registry()
            .dispatch("echo", json!({ "text": 3 }), &ctx())
            .await
            .unwrap_err();
        let ToolError::InvalidInput { tool, message } = err else {
            panic!("unexpected {err:?}");
        };
        assert_eq!(tool, "echo");
        assert!(message.contains("invalid type"), "{message}");
    }

    #[test]
    fn typed_tool_derives_its_schema_and_effect() {
        let registry = registry();
        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].input_schema["properties"]["text"]["type"], "string");
        assert_eq!(defs[0].input_schema["required"], json!(["text"]));
        let tool = registry.get("echo").unwrap();
        assert_eq!(tool.effect(&json!({ "text": "x" })), ToolEffect::Read);
        assert_eq!(tool.effect(&json!({})), ToolEffect::Execute);
    }

    #[test]
    fn register_rejects_duplicates_and_bad_names() {
        let mut registry = registry();
        assert_eq!(
            registry.register_typed(Echo("echo")),
            Err(RegisterError::Duplicate("echo".into()))
        );
        assert_eq!(
            registry.register_typed(Echo("bad name")),
            Err(RegisterError::InvalidName("bad name".into()))
        );
        assert_eq!(registry.names(), ["echo"]);
    }

    #[tokio::test]
    async fn dispatch_truncates_to_the_budget() {
        let registry = registry().with_budget(ResultBudget::new(1));
        let out = registry
            .dispatch("echo", json!({ "text": "uno\ndos" }), &ctx())
            .await
            .unwrap();
        assert!(out.text_content().starts_with("uno\n[1 more lines"));
    }

    #[test]
    fn errors_become_error_outputs() {
        let out = ToolOutput::from(ToolError::UnknownTool("x".into()));
        assert!(out.is_error);
        assert_eq!(out.text_content(), "unknown tool `x`");
    }
}
