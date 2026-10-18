//! LLM ports: [`LlmPort`] streams a model answer as [`LlmEvent`]s and
//! [`ModelCatalogPort`] lists the models the provider offers.
//!
//! Conventions shared by every port:
//!
//! - Traits are `#[async_trait]`, `Send + Sync` and used as `Arc<dyn Port>`.
//! - Cancellation is done by dropping the returned future or stream; the
//!   adapter must release the underlying connection when that happens and
//!   must not keep working in the background.

use domain::conversation::{Message, SessionId, TurnId};
use domain::model::ResolvedModel;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One request to the model: everything needed for a single Messages API
/// call, plus the fallback chain the adapter may walk on failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Model to use. The adapter tries `model.spec.id` first and then each
    /// of `model.spec.fallbacks` (see [`ResolvedModel::fallback_chain`]),
    /// retrying with backoff before switching models. `max_tokens` and
    /// `thinking` come from the spec unless overridden below.
    pub model: ResolvedModel,
    /// System prompt, split into blocks so cache breakpoints can be placed
    /// between the stable and the volatile parts.
    pub system: Vec<SystemBlock>,
    /// Conversation history, oldest first.
    pub messages: Vec<Message>,
    /// Client-side tools the model may call (results are produced locally).
    pub tools: Vec<ToolDefinition>,
    /// Tools executed by the provider itself (web search, web fetch).
    pub server_tools: Vec<ServerTool>,
    /// How the model must choose among the tools.
    pub tool_choice: ToolChoice,
    /// Overrides `model.spec.max_tokens` for this call only.
    pub max_tokens: Option<u32>,
    /// Data for traces and logs; never sent to the model as content.
    pub metadata: RequestMetadata,
}

impl LlmRequest {
    /// A request with no system prompt, tools or overrides.
    #[must_use]
    pub fn new(model: ResolvedModel, messages: Vec<Message>) -> Self {
        Self {
            model,
            system: Vec::new(),
            messages,
            tools: Vec::new(),
            server_tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: None,
            metadata: RequestMetadata::default(),
        }
    }

    /// The `max_tokens` the adapter must send: the override or the spec's.
    #[must_use]
    pub fn effective_max_tokens(&self) -> u32 {
        self.max_tokens.unwrap_or(self.model.spec.max_tokens)
    }
}

/// One block of the system prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemBlock {
    /// Block text.
    pub text: String,
    /// Place a prompt-cache breakpoint (`cache_control`) after this block,
    /// so everything up to and including it is cached.
    pub cache: bool,
}

impl SystemBlock {
    /// A block without a cache breakpoint.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: false,
        }
    }

    /// A block followed by a cache breakpoint.
    #[must_use]
    pub fn cached(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: true,
        }
    }
}

/// A client-side tool offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Name the model uses to call the tool (`^[a-zA-Z0-9_-]{1,128}$`).
    pub name: String,
    /// When and how to use the tool, written for the model.
    pub description: String,
    /// JSON Schema of the tool input.
    pub input_schema: serde_json::Value,
    /// Place a prompt-cache breakpoint after this tool. Tools are sent
    /// before the system prompt, so marking the last tool caches them all.
    pub cache: bool,
}

impl ToolDefinition {
    /// A tool with an explicit JSON Schema and no cache breakpoint.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            cache: false,
        }
    }

    /// A tool whose input schema is derived from `T` with `schemars`, as the
    /// component tools (`show_chart`, `show_table`, ...) do.
    #[must_use]
    pub fn for_input<T: JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::new(name, description, schemars::schema_for!(T).to_value())
    }

    /// Returns the same tool with a cache breakpoint after it.
    #[must_use]
    pub fn cached(mut self) -> Self {
        self.cache = true;
        self
    }
}

/// A tool run by the provider. Its results come back with citations (see
/// `LlmEvent::Citation`) instead of being executed locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerTool {
    /// Web search.
    WebSearch {
        /// Maximum searches per request; `None` uses the provider default.
        max_uses: Option<u32>,
    },
    /// Fetch and read a web page.
    WebFetch {
        /// Maximum fetches per request; `None` uses the provider default.
        max_uses: Option<u32>,
    },
}

/// How the model must use the offered tools.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum ToolChoice {
    /// The model decides whether to call a tool.
    #[default]
    Auto,
    /// The model must call some tool.
    Any,
    /// The model must call the named tool.
    Tool(String),
    /// The model must not call any tool.
    None,
}

/// Request data used only for tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// Session the call belongs to.
    pub session_id: Option<SessionId>,
    /// Turn the call belongs to.
    pub turn_id: Option<TurnId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::model::{ModelRole, ModelSpec, SpecSource};

    fn resolved() -> ResolvedModel {
        ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("claude-sonnet-5-5", 16_000),
            source: SpecSource::Default,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn max_tokens_override_wins_over_spec() {
        let mut req = LlmRequest::new(resolved(), vec![Message::user_text("hola")]);
        assert_eq!(req.effective_max_tokens(), 16_000);
        req.max_tokens = Some(100);
        assert_eq!(req.effective_max_tokens(), 100);
    }

    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct ShowTitle {
        title: String,
    }

    #[test]
    fn tool_schema_is_derived_from_input_type() {
        let tool = ToolDefinition::for_input::<ShowTitle>("show_title", "Shows a title").cached();
        assert!(tool.cache);
        assert_eq!(tool.input_schema["type"], "object");
        assert_eq!(tool.input_schema["properties"]["title"]["type"], "string");
    }
}
