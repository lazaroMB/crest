//! LLM ports: [`LlmPort`] streams a model answer as [`LlmEvent`]s and
//! [`ModelCatalogPort`] lists the models the provider offers.
//!
//! Conventions shared by every port:
//!
//! - Traits are `#[async_trait]`, `Send + Sync` and used as `Arc<dyn Port>`.
//! - Cancellation is done by dropping the returned future or stream; the
//!   adapter must release the underlying connection when that happens and
//!   must not keep working in the background.

use std::collections::BTreeMap;

use async_trait::async_trait;
use domain::conversation::{ContentPart, Message, SessionId, StopReason, ToolUseId, TurnId, Usage};
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
/// `RedactedThinking`, `ToolUseStart`, `ServerToolUse` or `ProviderBlock`)
/// and end with `BlockStop`. Events of different blocks may interleave.
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
    /// Informational: lets the UI show progress. The block's content for
    /// the history arrives later as an [`LlmEvent::ProviderBlock`] at the
    /// same index; what matters for evidence are the
    /// [`LlmEvent::Citation`]s the search produces.
    ServerToolUse {
        /// Block index.
        index: usize,
        /// Server tool name (e.g. `web_search`).
        name: String,
    },
    /// The complete JSON of a provider-specific block (a server tool call
    /// or its result), emitted once, right before the block's `BlockStop`.
    ///
    /// It becomes a [`ContentPart::ProviderBlock`] at position `index` of
    /// the assistant message, so the adapter can send it back unchanged in
    /// later turns. It may follow a [`LlmEvent::ServerToolUse`] of the same
    /// index or start the block itself (results have no deltas).
    ProviderBlock {
        /// Block index.
        index: usize,
        /// The block as provider JSON.
        raw: serde_json::Value,
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

    /// Whether `model` takes images in user messages. The default is
    /// `false`: an adapter opts in for the models it knows accept them.
    fn accepts_images(&self, model: &ModelId) -> bool {
        let _ = model;
        false
    }
}

/// A model offered by the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Id to use in requests.
    pub id: ModelId,
    /// Human-readable name.
    pub display_name: String,
    /// Release date as reported by the provider (RFC 3339), if any.
    pub created_at: Option<String>,
}

/// Lists the models the provider offers, to validate the role
/// configuration at startup (`ModelPolicy::validate_against`).
///
/// Cancellation: drop the future.
#[async_trait]
pub trait ModelCatalogPort: Send + Sync {
    /// Every available model, following pagination to the end.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError>;
}

/// Error turning an event stream into a message: the stream broke the
/// protocol described in [`LlmEvent`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssembleError {
    /// An event refers to a block that was never started.
    #[error("event for unknown block {index}")]
    UnknownBlock {
        /// Block index.
        index: usize,
    },
    /// A block was started twice.
    #[error("block {index} started twice")]
    DuplicateBlock {
        /// Block index.
        index: usize,
    },
    /// An event does not match the kind of its block (e.g. text into a
    /// tool use).
    #[error("event does not match the kind of block {index}")]
    BlockKindMismatch {
        /// Block index.
        index: usize,
    },
    /// An event arrived for a block that already stopped.
    #[error("block {index} already stopped")]
    BlockAlreadyStopped {
        /// Block index.
        index: usize,
    },
    /// The accumulated input of a tool use is not valid JSON.
    #[error("invalid JSON input for tool `{name}` (block {index}): {message}")]
    InvalidToolInput {
        /// Block index.
        index: usize,
        /// Tool name.
        name: String,
        /// Parser message.
        message: String,
    },
    /// The stream ended without [`LlmEvent::MessageStop`].
    #[error("stream ended before the message stopped")]
    Incomplete,
}

impl From<AssembleError> for LlmError {
    fn from(err: AssembleError) -> Self {
        Self::Decode(err.to_string())
    }
}

/// The final assistant message built by [`MessageAssembler`].
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledMessage {
    /// Assistant message with its parts in block order.
    pub message: Message,
    /// Model that answered, from [`LlmEvent::MessageStart`].
    pub model: Option<ModelId>,
    /// Last cumulative usage reported.
    pub usage: Usage,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Citations in arrival order.
    pub citations: Vec<Citation>,
}

/// A content block still receiving deltas.
#[derive(Debug, Clone, PartialEq)]
enum OpenBlock {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking(String),
    ToolUse {
        id: ToolUseId,
        name: String,
        json: String,
    },
    /// A server tool block whose content has not arrived yet; produces no
    /// domain content part unless a `ProviderBlock` fills it.
    ServerTool,
    /// A provider block with its full JSON.
    Provider(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq)]
enum BlockState {
    Open(OpenBlock),
    Stopped(Option<ContentPart>),
}

/// Pure helper that folds [`LlmEvent`]s into the final assistant
/// [`Message`], its usage and its stop reason.
///
/// Feed every event with [`push`](Self::push) and call
/// [`finish`](Self::finish) after the stream ends (or
/// [`interrupt`](Self::interrupt) if the user cancelled it). A new
/// [`LlmEvent::MessageStart`] discards everything accumulated so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageAssembler {
    model: Option<ModelId>,
    blocks: BTreeMap<usize, BlockState>,
    citations: Vec<Citation>,
    usage: Usage,
    stop_reason: Option<StopReason>,
}

impl MessageAssembler {
    /// An empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event.
    ///
    /// # Errors
    ///
    /// Returns an [`AssembleError`] when the event breaks the block
    /// protocol, or when a stopped tool use has invalid JSON input.
    pub fn push(&mut self, event: LlmEvent) -> Result<(), AssembleError> {
        match event {
            LlmEvent::MessageStart { model } => {
                *self = Self {
                    model: Some(model),
                    ..Self::default()
                };
            }
            LlmEvent::TextDelta { index, text } => {
                match self.open_or_start(index, || OpenBlock::Text(String::new()))? {
                    OpenBlock::Text(buf) => buf.push_str(&text),
                    _ => return Err(AssembleError::BlockKindMismatch { index }),
                }
            }
            LlmEvent::ThinkingDelta { index, thinking } => {
                let start = || OpenBlock::Thinking {
                    thinking: String::new(),
                    signature: None,
                };
                match self.open_or_start(index, start)? {
                    OpenBlock::Thinking { thinking: buf, .. } => buf.push_str(&thinking),
                    _ => return Err(AssembleError::BlockKindMismatch { index }),
                }
            }
            LlmEvent::SignatureDelta { index, signature } => match self.open(index)? {
                OpenBlock::Thinking { signature: sig, .. } => {
                    sig.get_or_insert_with(String::new).push_str(&signature);
                }
                _ => return Err(AssembleError::BlockKindMismatch { index }),
            },
            LlmEvent::RedactedThinking { index, data } => {
                self.start(index, OpenBlock::RedactedThinking(data))?;
            }
            LlmEvent::ToolUseStart { index, id, name } => {
                let json = String::new();
                self.start(index, OpenBlock::ToolUse { id, name, json })?;
            }
            LlmEvent::ToolInputDelta {
                index,
                partial_json,
            } => match self.open(index)? {
                OpenBlock::ToolUse { json, .. } => json.push_str(&partial_json),
                _ => return Err(AssembleError::BlockKindMismatch { index }),
            },
            LlmEvent::ServerToolUse { index, .. } => self.start(index, OpenBlock::ServerTool)?,
            LlmEvent::ProviderBlock { index, raw } => {
                match self.open_or_start(index, || OpenBlock::ServerTool)? {
                    block @ OpenBlock::ServerTool => *block = OpenBlock::Provider(raw),
                    OpenBlock::Provider(_) => return Err(AssembleError::DuplicateBlock { index }),
                    _ => return Err(AssembleError::BlockKindMismatch { index }),
                }
            }
            LlmEvent::Citation { citation, .. } => self.citations.push(citation),
            LlmEvent::BlockStop { index } => {
                let block = std::mem::replace(self.open(index)?, OpenBlock::ServerTool);
                let part = close(index, block)?;
                self.blocks.insert(index, BlockState::Stopped(part));
            }
            LlmEvent::Usage(usage) => self.usage = usage,
            LlmEvent::ModelFallback { .. } | LlmEvent::Retrying { .. } => {}
            LlmEvent::MessageStop { stop_reason } => self.stop_reason = Some(stop_reason),
        }
        Ok(())
    }

    /// Builds the final message once the stream has ended. Blocks that never
    /// got a [`LlmEvent::BlockStop`] are closed implicitly.
    ///
    /// # Errors
    ///
    /// [`AssembleError::Incomplete`] if no [`LlmEvent::MessageStop`] was
    /// seen, or [`AssembleError::InvalidToolInput`] for an implicitly closed
    /// tool use with invalid JSON.
    pub fn finish(self) -> Result<AssembledMessage, AssembleError> {
        let stop_reason = self.stop_reason.ok_or(AssembleError::Incomplete)?;
        let mut parts = Vec::with_capacity(self.blocks.len());
        for (index, state) in self.blocks {
            let part = match state {
                BlockState::Open(block) => close(index, block)?,
                BlockState::Stopped(part) => part,
            };
            parts.extend(part);
        }
        Ok(AssembledMessage {
            message: Message::assistant(parts),
            model: self.model,
            usage: self.usage,
            stop_reason,
            citations: self.citations,
        })
    }

    /// Builds what was received before the user interrupted the stream,
    /// with [`StopReason::Interrupted`].
    ///
    /// Text and thinking are kept; tool uses and provider blocks are
    /// dropped, finished or not, because a tool call without its result
    /// cannot be sent back to the model.
    #[must_use]
    pub fn interrupt(self) -> AssembledMessage {
        let parts = self
            .blocks
            .into_iter()
            .filter_map(|(index, state)| match state {
                BlockState::Open(block) => close(index, block).ok().flatten(),
                BlockState::Stopped(part) => part,
            })
            .filter(|part| {
                !matches!(
                    part,
                    ContentPart::ToolUse { .. } | ContentPart::ProviderBlock { .. }
                )
            })
            .collect();
        AssembledMessage {
            message: Message::assistant(parts),
            model: self.model,
            usage: self.usage,
            stop_reason: StopReason::Interrupted,
            citations: self.citations,
        }
    }

    /// The open block at `index`.
    fn open(&mut self, index: usize) -> Result<&mut OpenBlock, AssembleError> {
        match self.blocks.get_mut(&index) {
            Some(BlockState::Open(block)) => Ok(block),
            Some(BlockState::Stopped(_)) => Err(AssembleError::BlockAlreadyStopped { index }),
            None => Err(AssembleError::UnknownBlock { index }),
        }
    }

    /// The open block at `index`, starting it with `start` if it is new.
    fn open_or_start(
        &mut self,
        index: usize,
        start: impl FnOnce() -> OpenBlock,
    ) -> Result<&mut OpenBlock, AssembleError> {
        self.blocks
            .entry(index)
            .or_insert_with(|| BlockState::Open(start()));
        self.open(index)
    }

    /// Starts a new block at `index`.
    fn start(&mut self, index: usize, block: OpenBlock) -> Result<(), AssembleError> {
        if self.blocks.contains_key(&index) {
            return Err(AssembleError::DuplicateBlock { index });
        }
        self.blocks.insert(index, BlockState::Open(block));
        Ok(())
    }
}

/// Turns a finished block into its content part, parsing tool input JSON
/// (an empty input means `{}`).
fn close(index: usize, block: OpenBlock) -> Result<Option<ContentPart>, AssembleError> {
    Ok(match block {
        OpenBlock::Text(text) => Some(ContentPart::Text { text }),
        OpenBlock::Thinking {
            thinking,
            signature,
        } => Some(ContentPart::Thinking {
            thinking,
            signature,
        }),
        OpenBlock::RedactedThinking(data) => Some(ContentPart::RedactedThinking { data }),
        OpenBlock::ToolUse { id, name, json } => {
            let input = if json.trim().is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                match serde_json::from_str(&json) {
                    Ok(input) => input,
                    Err(err) => {
                        return Err(AssembleError::InvalidToolInput {
                            index,
                            name,
                            message: err.to_string(),
                        });
                    }
                }
            };
            Some(ContentPart::ToolUse { id, name, input })
        }
        OpenBlock::ServerTool => None,
        OpenBlock::Provider(raw) => Some(ContentPart::ProviderBlock { raw }),
    })
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

    fn assemble(events: Vec<LlmEvent>) -> Result<AssembledMessage, AssembleError> {
        let mut asm = MessageAssembler::new();
        for event in events {
            asm.push(event)?;
        }
        asm.finish()
    }

    fn start() -> LlmEvent {
        LlmEvent::MessageStart {
            model: ModelId::new("claude-sonnet-5-5"),
        }
    }

    fn stop(stop_reason: StopReason) -> LlmEvent {
        LlmEvent::MessageStop { stop_reason }
    }

    fn text(index: usize, text: &str) -> LlmEvent {
        LlmEvent::TextDelta {
            index,
            text: text.into(),
        }
    }

    fn tool_start(index: usize, id: &str, name: &str) -> LlmEvent {
        LlmEvent::ToolUseStart {
            index,
            id: ToolUseId::new(id),
            name: name.into(),
        }
    }

    fn json(index: usize, partial_json: &str) -> LlmEvent {
        LlmEvent::ToolInputDelta {
            index,
            partial_json: partial_json.into(),
        }
    }

    fn block_stop(index: usize) -> LlmEvent {
        LlmEvent::BlockStop { index }
    }

    #[test]
    fn text_in_several_deltas_with_usage() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 3,
            ..Usage::default()
        };
        let out = assemble(vec![
            start(),
            LlmEvent::Usage(Usage {
                input_tokens: 10,
                ..Usage::default()
            }),
            text(0, "Hola, "),
            text(0, "mundo"),
            block_stop(0),
            LlmEvent::Usage(usage),
            stop(StopReason::EndTurn),
        ])
        .unwrap();
        assert_eq!(out.message.role, domain::conversation::Role::Assistant);
        assert_eq!(out.message.content, vec![ContentPart::text("Hola, mundo")]);
        assert_eq!(out.usage, usage);
        assert_eq!(out.stop_reason, StopReason::EndTurn);
        assert_eq!(out.model, Some(ModelId::new("claude-sonnet-5-5")));
    }

    #[test]
    fn thinking_with_signature_precedes_text() {
        let out = assemble(vec![
            start(),
            LlmEvent::ThinkingDelta {
                index: 0,
                thinking: "pien".into(),
            },
            LlmEvent::ThinkingDelta {
                index: 0,
                thinking: "so".into(),
            },
            LlmEvent::SignatureDelta {
                index: 0,
                signature: "sig".into(),
            },
            block_stop(0),
            text(1, "listo"),
            block_stop(1),
            stop(StopReason::EndTurn),
        ])
        .unwrap();
        assert_eq!(
            out.message.content,
            vec![
                ContentPart::Thinking {
                    thinking: "pienso".into(),
                    signature: Some("sig".into()),
                },
                ContentPart::text("listo"),
            ]
        );
    }

    #[test]
    fn interleaved_tool_uses_are_ordered_by_index() {
        let out = assemble(vec![
            start(),
            tool_start(1, "toolu_b", "read_file"),
            tool_start(0, "toolu_a", "grep"),
            json(1, "{\"path\":"),
            json(0, "{\"pattern\""),
            json(1, "\"src/main.rs\"}"),
            json(0, ":\"fn main\"}"),
            block_stop(1),
            block_stop(0),
            tool_start(2, "toolu_c", "list_dir"),
            block_stop(2),
            stop(StopReason::ToolUse),
        ])
        .unwrap();
        let tool = |id: &str, name: &str, input| ContentPart::ToolUse {
            id: ToolUseId::new(id),
            name: name.into(),
            input,
        };
        assert_eq!(
            out.message.content,
            vec![
                tool(
                    "toolu_a",
                    "grep",
                    serde_json::json!({ "pattern": "fn main" })
                ),
                tool(
                    "toolu_b",
                    "read_file",
                    serde_json::json!({ "path": "src/main.rs" })
                ),
                tool("toolu_c", "list_dir", serde_json::json!({})),
            ]
        );
        assert_eq!(out.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn invalid_tool_json_is_a_typed_error() {
        let mut asm = MessageAssembler::new();
        asm.push(start()).unwrap();
        asm.push(tool_start(0, "toolu_a", "grep")).unwrap();
        asm.push(json(0, "{\"pattern\":")).unwrap();
        let err = asm.push(block_stop(0)).unwrap_err();
        assert!(matches!(
            err,
            AssembleError::InvalidToolInput { index: 0, ref name, .. } if name == "grep"
        ));
        assert!(matches!(LlmError::from(err), LlmError::Decode(_)));
    }

    #[test]
    fn protocol_violations_are_rejected() {
        let mut asm = MessageAssembler::new();
        asm.push(start()).unwrap();
        assert_eq!(
            asm.push(json(3, "{}")),
            Err(AssembleError::UnknownBlock { index: 3 })
        );
        asm.push(tool_start(0, "toolu_a", "grep")).unwrap();
        assert_eq!(
            asm.push(text(0, "x")),
            Err(AssembleError::BlockKindMismatch { index: 0 })
        );
        assert_eq!(
            asm.push(tool_start(0, "toolu_b", "grep")),
            Err(AssembleError::DuplicateBlock { index: 0 })
        );
        asm.push(block_stop(0)).unwrap();
        assert_eq!(
            asm.push(json(0, "{}")),
            Err(AssembleError::BlockAlreadyStopped { index: 0 })
        );
        assert_eq!(asm.finish(), Err(AssembleError::Incomplete));
    }

    #[test]
    fn provider_blocks_take_their_index_position() {
        let call = serde_json::json!({"type": "server_tool_use", "id": "srvtoolu_1"});
        let result =
            serde_json::json!({"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1"});
        let out = assemble(vec![
            start(),
            text(0, "Busco."),
            block_stop(0),
            LlmEvent::ServerToolUse {
                index: 1,
                name: "web_search".into(),
            },
            LlmEvent::ProviderBlock {
                index: 1,
                raw: call.clone(),
            },
            block_stop(1),
            LlmEvent::ProviderBlock {
                index: 2,
                raw: result.clone(),
            },
            block_stop(2),
            text(3, "Listo."),
            block_stop(3),
            stop(StopReason::EndTurn),
        ])
        .unwrap();
        assert_eq!(
            out.message.content,
            vec![
                ContentPart::text("Busco."),
                ContentPart::ProviderBlock { raw: call },
                ContentPart::ProviderBlock { raw: result },
                ContentPart::text("Listo."),
            ]
        );
    }

    #[test]
    fn provider_block_protocol_violations() {
        let raw = serde_json::json!({"type": "server_tool_use"});
        let provider = |index| LlmEvent::ProviderBlock {
            index,
            raw: raw.clone(),
        };
        let mut asm = MessageAssembler::new();
        asm.push(start()).unwrap();
        asm.push(text(0, "x")).unwrap();
        assert_eq!(
            asm.push(provider(0)),
            Err(AssembleError::BlockKindMismatch { index: 0 })
        );
        asm.push(provider(1)).unwrap();
        assert_eq!(
            asm.push(provider(1)),
            Err(AssembleError::DuplicateBlock { index: 1 })
        );
        asm.push(block_stop(1)).unwrap();
        assert_eq!(
            asm.push(provider(1)),
            Err(AssembleError::BlockAlreadyStopped { index: 1 })
        );
        // Interrupted answers drop provider blocks like tool uses.
        let out = asm.interrupt();
        assert_eq!(out.message.content, vec![ContentPart::text("x")]);
    }

    #[test]
    fn server_tools_keep_citations_but_no_parts() {
        let citation = Citation {
            url: "https://docs.rs".into(),
            title: Some("Docs.rs".into()),
            cited_text: "docs".into(),
        };
        let out = assemble(vec![
            start(),
            LlmEvent::ServerToolUse {
                index: 0,
                name: "web_search".into(),
            },
            block_stop(0),
            text(1, "Según docs.rs"),
            LlmEvent::Citation {
                index: 1,
                citation: citation.clone(),
            },
            block_stop(1),
            stop(StopReason::EndTurn),
        ])
        .unwrap();
        assert_eq!(
            out.message.content,
            vec![ContentPart::text("Según docs.rs")]
        );
        assert_eq!(out.citations, vec![citation]);
    }

    #[test]
    fn new_message_start_resets_and_open_blocks_close_on_finish() {
        let out = assemble(vec![
            start(),
            text(0, "intento fallido"),
            LlmEvent::ModelFallback {
                from: ModelId::new("claude-sonnet-5-5"),
                to: ModelId::new("claude-haiku-5-5"),
                reason: LlmError::Overloaded,
            },
            LlmEvent::MessageStart {
                model: ModelId::new("claude-haiku-5-5"),
            },
            text(0, "ok"),
            stop(StopReason::MaxTokens),
        ])
        .unwrap();
        assert_eq!(out.message.content, vec![ContentPart::text("ok")]);
        assert_eq!(out.model, Some(ModelId::new("claude-haiku-5-5")));
    }

    #[test]
    fn interrupt_keeps_text_and_drops_tool_uses() {
        let mut asm = MessageAssembler::new();
        for event in [
            start(),
            text(0, "voy a buscar"),
            block_stop(0),
            tool_start(1, "toolu_a", "grep"),
            json(1, "{}"),
            block_stop(1),
            tool_start(2, "toolu_b", "grep"),
            json(2, "{\"pat"),
        ] {
            asm.push(event).unwrap();
        }
        let out = asm.interrupt();
        assert_eq!(out.message.content, vec![ContentPart::text("voy a buscar")]);
        assert_eq!(out.stop_reason, StopReason::Interrupted);
    }

    struct OneModel;

    #[async_trait]
    impl ModelCatalogPort for OneModel {
        async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
            Ok(vec![ModelInfo {
                id: ModelId::new("claude-haiku-5-5"),
                display_name: "Claude Haiku 5.5".into(),
                created_at: None,
            }])
        }
    }

    #[tokio::test]
    async fn model_catalog_is_object_safe() {
        let port: std::sync::Arc<dyn ModelCatalogPort> = std::sync::Arc::new(OneModel);
        let models = port.list_models().await.unwrap();
        assert_eq!(models[0].id, ModelId::new("claude-haiku-5-5"));
    }
}
