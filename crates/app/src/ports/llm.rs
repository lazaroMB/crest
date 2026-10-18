//! LLM ports: [`LlmPort`] streams a model answer as [`LlmEvent`]s and
//! [`ModelCatalogPort`] lists the models the provider offers.
//!
//! Conventions shared by every port:
//!
//! - Traits are `#[async_trait]`, `Send + Sync` and used as `Arc<dyn Port>`.
//! - Cancellation is done by dropping the returned future or stream; the
//!   adapter must release the underlying connection when that happens and
//!   must not keep working in the background.

use async_trait::async_trait;
use domain::conversation::{Message, SessionId, StopReason, ToolUseId, TurnId, Usage};
use domain::model::{ModelId, ResolvedModel};
use futures::stream::BoxStream;
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

/// One event of a streamed model answer.
///
/// `index` is the position of the content block in the provider's answer.
/// Blocks start with their first event (`TextDelta`, `ThinkingDelta`,
/// `RedactedThinking`, `ToolUseStart` or `ServerToolUse`) and end with
/// `BlockStop`. Events of different blocks may interleave.
///
/// `Retrying` and `ModelFallback` are emitted by the adapter between
/// attempts, before the `MessageStart` of the attempt that succeeds; an
/// adapter never retries once content of an attempt has been emitted.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmEvent {
    /// The provider accepted the request; `model` is the model answering.
    MessageStart {
        /// Model actually answering (may be a fallback).
        model: ModelId,
    },
    /// A chunk of text of a text block.
    TextDelta {
        /// Block index.
        index: usize,
        /// Text to append.
        text: String,
    },
    /// A chunk of extended thinking.
    ThinkingDelta {
        /// Block index.
        index: usize,
        /// Thinking text to append.
        thinking: String,
    },
    /// The signature of a thinking block, to be sent back unchanged.
    SignatureDelta {
        /// Block index of the thinking block.
        index: usize,
        /// Signature chunk to append.
        signature: String,
    },
    /// A whole redacted thinking block (it has no deltas).
    RedactedThinking {
        /// Block index.
        index: usize,
        /// Opaque encrypted payload.
        data: String,
    },
    /// The model starts calling a client-side tool.
    ToolUseStart {
        /// Block index.
        index: usize,
        /// Provider-assigned id of the call.
        id: ToolUseId,
        /// Tool name.
        name: String,
    },
    /// A chunk of the tool input JSON; only valid once the block stops.
    ToolInputDelta {
        /// Block index of the tool use.
        index: usize,
        /// Partial JSON to append.
        partial_json: String,
    },
    /// The provider runs one of its server tools (web search or fetch).
    ///
    /// Informational: lets the UI show progress. Server tool blocks and
    /// their results are not part of the domain `Message`; what matters
    /// for evidence are the [`LlmEvent::Citation`]s they produce.
    ServerToolUse {
        /// Block index.
        index: usize,
        /// Server tool name (e.g. `web_search`).
        name: String,
    },
    /// A citation attached to the text block at `index`.
    Citation {
        /// Block index of the cited text.
        index: usize,
        /// The citation.
        citation: Citation,
    },
    /// The content block at `index` is complete.
    BlockStop {
        /// Block index.
        index: usize,
    },
    /// Usage so far for this message, cumulative: each event replaces the
    /// previous one rather than adding to it.
    Usage(Usage),
    /// The adapter gave up on `from` and switches to `to`.
    ModelFallback {
        /// Model that failed.
        from: ModelId,
        /// Next model of the fallback chain.
        to: ModelId,
        /// Error that triggered the switch.
        reason: LlmError,
    },
    /// The adapter waits and retries the same model.
    Retrying {
        /// Retry number, starting at 1.
        attempt: u32,
        /// Delay before the retry, in milliseconds.
        delay_ms: u64,
        /// Error that triggered the retry.
        reason: LlmError,
    },
    /// The answer is complete.
    MessageStop {
        /// Why the model stopped.
        stop_reason: StopReason,
    },
}

/// A source cited by the model, typically from web search or fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    /// Cited URL.
    pub url: String,
    /// Page title, when known.
    pub title: Option<String>,
    /// Quoted text supporting the claim.
    pub cited_text: String,
}

/// Errors of [`LlmPort`] and [`ModelCatalogPort`]. Messages never include
/// the API key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    /// The API key is missing, invalid or lacks permission (401/403).
    #[error("authentication failed: {message}")]
    Auth {
        /// What to fix, e.g. "set ANTHROPIC_API_KEY".
        message: String,
    },
    /// Too many requests (429).
    #[error("rate limited")]
    RateLimited {
        /// Wait suggested by the provider (`retry-after`), if any.
        retry_after_ms: Option<u64>,
    },
    /// The provider is overloaded (529).
    #[error("provider overloaded")]
    Overloaded,
    /// Other server-side failure.
    #[error("server error {status}: {message}")]
    Server {
        /// HTTP status.
        status: u16,
        /// Provider message.
        message: String,
    },
    /// The request was rejected as malformed (400/404/422).
    #[error("invalid request: {message}")]
    InvalidRequest {
        /// Provider message.
        message: String,
    },
    /// Connection, TLS or timeout failure.
    #[error("network error: {0}")]
    Network(String),
    /// The response or the stream could not be decoded.
    #[error("decode error: {0}")]
    Decode(String),
    /// The prompt does not fit in the model's context window.
    #[error("prompt is too long for the model's context window")]
    ContextTooLong,
    /// The call was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The adapter does not support this operation.
    #[error("operation not supported by this provider")]
    Unsupported,
    /// Every model of the fallback chain failed; `last` is the final error.
    #[error("all models failed; last error: {last}")]
    AllModelsFailed {
        /// Error of the last model tried.
        last: Box<LlmError>,
    },
}

impl LlmError {
    /// Whether retrying the same request later may succeed: rate limits,
    /// overload, 5xx and network errors.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Overloaded | Self::Network(_) => true,
            Self::Server { status, .. } => *status >= 500,
            Self::Auth { .. }
            | Self::InvalidRequest { .. }
            | Self::Decode(_)
            | Self::ContextTooLong
            | Self::Cancelled
            | Self::Unsupported
            | Self::AllModelsFailed { .. } => false,
        }
    }

    /// Whether, after retries are exhausted, switching to the next model of
    /// the fallback chain makes sense: overload (529) and 5xx errors.
    #[must_use]
    pub fn triggers_fallback(&self) -> bool {
        match self {
            Self::Overloaded => true,
            Self::Server { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

/// Stream of events of one model answer. Dropping it cancels the call.
pub type LlmStream = BoxStream<'static, Result<LlmEvent, LlmError>>;

/// Sends a conversation to a model and streams its answer.
///
/// The adapter owns retries with backoff and the walk along the fallback
/// chain of [`LlmRequest::model`], reporting them as
/// [`LlmEvent::Retrying`] and [`LlmEvent::ModelFallback`]. When every model
/// fails it yields [`LlmError::AllModelsFailed`].
///
/// Cancellation: drop the future or the stream. No further events are
/// produced and the HTTP connection is closed.
#[async_trait]
pub trait LlmPort: Send + Sync {
    /// Starts a model call and returns its event stream.
    ///
    /// Errors detected before streaming (e.g. [`LlmError::Auth`] when no API
    /// key is configured) are returned directly; errors during the call
    /// arrive as `Err` items of the stream, after which it ends.
    async fn stream(&self, req: LlmRequest) -> Result<LlmStream, LlmError>;

    /// Counts the input tokens `req` would consume, without running it.
    ///
    /// The default implementation returns [`LlmError::Unsupported`].
    async fn count_tokens(&self, req: &LlmRequest) -> Result<u64, LlmError> {
        let _ = req;
        Err(LlmError::Unsupported)
    }
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

    #[test]
    fn retryable_and_fallback_errors() {
        let server = |status| LlmError::Server {
            status,
            message: String::new(),
        };
        assert!(LlmError::Overloaded.is_retryable());
        assert!(LlmError::Overloaded.triggers_fallback());
        assert!(
            LlmError::RateLimited {
                retry_after_ms: None
            }
            .is_retryable()
        );
        assert!(
            !LlmError::RateLimited {
                retry_after_ms: None
            }
            .triggers_fallback()
        );
        assert!(LlmError::Network("reset".into()).is_retryable());
        assert!(server(503).is_retryable() && server(503).triggers_fallback());
        assert!(!server(418).is_retryable());
        let auth = LlmError::Auth {
            message: "set ANTHROPIC_API_KEY".into(),
        };
        assert!(!auth.is_retryable() && !auth.triggers_fallback());
        assert!(!LlmError::ContextTooLong.is_retryable());
    }

    #[test]
    fn all_models_failed_shows_last_error() {
        let err = LlmError::AllModelsFailed {
            last: Box::new(LlmError::Overloaded),
        };
        assert_eq!(
            err.to_string(),
            "all models failed; last error: provider overloaded"
        );
    }

    struct NoTokens;

    #[async_trait]
    impl LlmPort for NoTokens {
        async fn stream(&self, _req: LlmRequest) -> Result<LlmStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[tokio::test]
    async fn count_tokens_defaults_to_unsupported() {
        let port: std::sync::Arc<dyn LlmPort> = std::sync::Arc::new(NoTokens);
        let req = LlmRequest::new(resolved(), Vec::new());
        assert_eq!(port.count_tokens(&req).await, Err(LlmError::Unsupported));
        assert!(port.stream(req).await.is_ok());
    }
}
