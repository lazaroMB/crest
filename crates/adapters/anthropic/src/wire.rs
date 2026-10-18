//! Serde types for the Anthropic Messages API (`POST /v1/messages`).
//!
//! Pure data, no HTTP. Field names and shapes follow the official API
//! reference (platform.claude.com/docs/en/api/messages). Anything the API
//! may grow over time (content blocks, deltas, citations, stream events) is
//! deserialised tolerantly: an unknown `type` becomes an `Unknown(Value)`
//! variant that keeps the raw JSON and serialises back unchanged, so a new
//! block type never breaks a response nor gets lost when echoed back.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Implements `Serialize`/`Deserialize` for an internally tagged enum whose
/// derive uses `#[serde(remote = "Self")]` and that has an
/// `Unknown(Value)` variant marked `#[serde(skip)]`.
///
/// Objects whose `type` is one of the listed tags go through the derived
/// code (so a malformed known block is still an error); any other object is
/// kept verbatim in `Unknown`.
macro_rules! tolerant_tagged {
    ($ty:ident, [$($tag:literal),+ $(,)?]) => {
        impl $ty {
            /// `type` tags handled by the typed variants.
            pub const KNOWN_TAGS: &'static [&'static str] = &[$($tag),+];
        }

        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    Self::Unknown(value) => value.serialize(serializer),
                    known => $ty::serialize(known, serializer),
                }
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = Value::deserialize(deserializer)?;
                match value.get("type").and_then(Value::as_str) {
                    Some(tag) if Self::KNOWN_TAGS.contains(&tag) => {
                        $ty::deserialize(value).map_err(serde::de::Error::custom)
                    }
                    _ => Ok(Self::Unknown(value)),
                }
            }
        }
    };
}

fn is_false(value: &bool) -> bool {
    !*value
}

// ---------------------------------------------------------------------------
// Prompt caching
// ---------------------------------------------------------------------------

/// Cache breakpoint marker: `{"type": "ephemeral"}`, optionally with a TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename = "ephemeral")]
pub struct CacheControl {
    /// Cache lifetime; the API default is five minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTtl>,
}

impl CacheControl {
    /// `{"type": "ephemeral"}` with the default TTL.
    pub const EPHEMERAL: Self = Self { ttl: None };
}

/// Lifetime of a cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    /// `"5m"` (the API default).
    #[serde(rename = "5m")]
    FiveMinutes,
    /// `"1h"`.
    #[serde(rename = "1h")]
    OneHour,
}

// ---------------------------------------------------------------------------
// Content blocks (request and response)
// ---------------------------------------------------------------------------

/// One content block, as sent in `messages[].content` or received in a
/// response's `content` (and in `content_block_start`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", remote = "Self")]
pub enum WireContentBlock {
    /// Plain text; responses may attach citations.
    Text {
        /// The text.
        text: String,
        /// Sources supporting this text (responses only; may be `null`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        citations: Option<Vec<WireCitation>>,
        /// Cache breakpoint after this block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Extended thinking. Must be echoed back unchanged, signature included.
    Thinking {
        /// Reasoning text (empty when the display mode omits it).
        thinking: String,
        /// Opaque integrity signature. Empty in `content_block_start`; it
        /// arrives later as a `signature_delta`.
        #[serde(default)]
        signature: String,
    },
    /// Thinking encrypted by the provider.
    RedactedThinking {
        /// Opaque payload.
        data: String,
    },
    /// A client tool call requested by the model.
    ToolUse {
        /// Provider id (`toolu_...`).
        id: String,
        /// Tool name.
        name: String,
        /// Arguments. `{}` in `content_block_start`; streamed as
        /// `input_json_delta`.
        #[serde(default)]
        input: Value,
    },
    /// Result of a client tool call (request only).
    ToolResult {
        /// Id of the `tool_use` block this answers.
        tool_use_id: String,
        /// Result payload; omitted when empty.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<ToolResultContent>,
        /// Whether the tool failed.
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
        /// Cache breakpoint after this block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// An inline image.
    Image {
        /// Image bytes and type.
        source: ImageSource,
        /// Cache breakpoint after this block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// A server tool call (web search, web fetch, ...) run by the API.
    ServerToolUse {
        /// Provider id (`srvtoolu_...`).
        id: String,
        /// Server tool name, e.g. `web_search`.
        name: String,
        /// Arguments (e.g. `{"query": ...}`); streamed as `input_json_delta`.
        #[serde(default)]
        input: Value,
        /// Other fields (e.g. `caller`), kept so the block round-trips.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// Results of a `web_search` server tool call.
    WebSearchToolResult {
        /// Id of the matching `server_tool_use`.
        tool_use_id: String,
        /// Result list or error object.
        content: WebSearchToolResultContent,
        /// Other fields (e.g. `caller`), kept so the block round-trips.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// Result of a `web_fetch` server tool call.
    WebFetchToolResult {
        /// Id of the matching `server_tool_use`.
        tool_use_id: String,
        /// Fetched document or error object.
        content: WebFetchToolResultContent,
        /// Other fields (e.g. `caller`), kept so the block round-trips.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// Any block type this crate does not model, kept verbatim.
    #[serde(skip)]
    Unknown(Value),
}

tolerant_tagged!(
    WireContentBlock,
    [
        "text",
        "thinking",
        "redacted_thinking",
        "tool_use",
        "tool_result",
        "image",
        "server_tool_use",
        "web_search_tool_result",
        "web_fetch_tool_result",
    ]
);

impl WireContentBlock {
    /// A text block without citations or cache marker.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            citations: None,
            cache_control: None,
        }
    }
}

/// `tool_result.content`: a bare string or a list of blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Shorthand for a single text block.
    Text(String),
    /// Text and image blocks.
    Blocks(Vec<WireContentBlock>),
}

/// Source of an image block. Only inline base64 is modelled; the API also
/// accepts `url` and `file` sources, which this crate does not send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Inline bytes.
    Base64 {
        /// `image/jpeg`, `image/png`, `image/gif` or `image/webp`.
        media_type: String,
        /// Base64-encoded bytes.
        data: String,
    },
}

/// `web_search_tool_result.content`: a list on success, an object on error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WebSearchToolResultContent {
    /// Search hits (possibly empty).
    Results(Vec<WebSearchResult>),
    /// `{"type": "web_search_tool_result_error", "error_code": ...}`.
    Error(ServerToolError),
    /// Any other shape, kept verbatim.
    Other(Value),
}

/// One `web_search_result` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchResult {
    /// Always `web_search_result`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Page URL.
    pub url: String,
    /// Page title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Opaque content that must be sent back unchanged in later turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
    /// When the page was last updated, as reported by the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,
}

/// Error object of a server tool result. The call failed but the HTTP
/// response is still 200.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerToolError {
    /// `web_search_tool_result_error` or `web_fetch_tool_result_error`.
    #[serde(rename = "type")]
    pub kind: String,
    /// E.g. `max_uses_exceeded`, `url_not_accessible`. Kept as a string
    /// because the API adds codes over time.
    pub error_code: String,
}

/// `web_fetch_tool_result.content`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WebFetchToolResultContent {
    /// The fetched page.
    Result(WebFetchResult),
    /// `{"type": "web_fetch_tool_result_error", "error_code": ...}`.
    Error(ServerToolError),
    /// Any other shape, kept verbatim.
    Other(Value),
}

/// A `web_fetch_result` object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebFetchResult {
    /// Always `web_fetch_result`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Fetched URL.
    pub url: String,
    /// A `document` block (text or base64 PDF source, title, citations
    /// config), kept as raw JSON.
    pub content: Value,
    /// ISO 8601 retrieval time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Citations
// ---------------------------------------------------------------------------

/// A citation attached to a response text block (or streamed in a
/// `citations_delta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", remote = "Self")]
pub enum WireCitation {
    /// A web search hit (always on for web search).
    WebSearchResultLocation {
        /// Source URL.
        url: String,
        /// Source title.
        #[serde(default)]
        title: Option<String>,
        /// Up to 150 characters of the cited content.
        cited_text: String,
        /// Opaque reference that must be sent back in later turns.
        encrypted_index: String,
    },
    /// Character range of a plain-text document (e.g. a web fetch result).
    CharLocation {
        /// Cited passage.
        cited_text: String,
        /// Index of the document in the request.
        document_index: u32,
        /// Document title.
        #[serde(default)]
        document_title: Option<String>,
        /// First character, inclusive.
        start_char_index: u32,
        /// Last character, exclusive.
        end_char_index: u32,
    },
    /// Page range of a PDF document.
    PageLocation {
        /// Cited passage.
        cited_text: String,
        /// Index of the document in the request.
        document_index: u32,
        /// Document title.
        #[serde(default)]
        document_title: Option<String>,
        /// First page, 1-based, inclusive.
        start_page_number: u32,
        /// Last page, exclusive.
        end_page_number: u32,
    },
    /// Any other citation type (content block, search result, ...), kept
    /// verbatim.
    #[serde(skip)]
    Unknown(Value),
}

tolerant_tagged!(
    WireCitation,
    [
        "web_search_result_location",
        "char_location",
        "page_location"
    ]
);

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// Current versioned `type` of the web search server tool (dynamic
/// filtering plus `response_inclusion`; Claude 4.6 and later models).
pub const WEB_SEARCH_TOOL_TYPE: &str = "web_search_20260318";
/// Current versioned `type` of the web fetch server tool (dynamic
/// filtering, cache bypass and `response_inclusion`).
pub const WEB_FETCH_TOOL_TYPE: &str = "web_fetch_20260318";

/// Body of `POST /v1/messages`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessagesRequest {
    /// Model id.
    pub model: String,
    /// Output token cap (thinking included).
    pub max_tokens: u32,
    /// System prompt blocks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemContent>,
    /// Conversation, oldest first.
    pub messages: Vec<WireMessage>,
    /// Client and server tools.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    /// How the model must pick tools; omitted means `auto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<WireToolChoice>,
    /// Extended thinking; omitted means the model's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<WireThinking>,
    /// Stream the answer as server-sent events.
    #[serde(skip_serializing_if = "is_false")]
    pub stream: bool,
    /// Opaque request metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<WireMetadata>,
    /// Custom stop sequences.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Sampling temperature. Deprecated by the API: models after Claude
    /// Opus 4.6 reject any value but 1.0, so leave it unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Automatic prompt caching: the API places this breakpoint on the
    /// last cacheable block and moves it forward as the conversation
    /// grows. Takes one of the four breakpoint slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl MessagesRequest {
    /// A non-streaming request with only the required fields.
    #[must_use]
    pub fn new(model: impl Into<String>, max_tokens: u32, messages: Vec<WireMessage>) -> Self {
        Self {
            model: model.into(),
            max_tokens,
            system: Vec::new(),
            messages,
            tools: Vec::new(),
            tool_choice: None,
            thinking: None,
            stream: false,
            metadata: None,
            stop_sequences: Vec::new(),
            temperature: None,
            cache_control: None,
        }
    }
}

/// One system prompt block: `{"type": "text", "text": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "text")]
pub struct SystemContent {
    /// Prompt text.
    pub text: String,
    /// Cache breakpoint after this block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// Author of a wire message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireRole {
    /// The user (and tool results).
    User,
    /// The model.
    Assistant,
}

/// One entry of `messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireMessage {
    /// Author.
    pub role: WireRole,
    /// Content blocks (always the array form).
    pub content: Vec<WireContentBlock>,
}

/// One entry of `tools`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WireTool {
    /// A client tool, executed locally.
    Custom(CustomTool),
    /// A tool executed by the API (web search, web fetch).
    Server(ServerToolDef),
}

/// A client tool definition (no `type` field).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CustomTool {
    /// Tool name.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// JSON Schema of the input.
    pub input_schema: Value,
    /// Cache breakpoint after this tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// A server tool definition such as
/// `{"type": "web_search_20260318", "name": "web_search", "max_uses": 5}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerToolDef {
    /// Versioned tool type, e.g. [`WEB_SEARCH_TOOL_TYPE`].
    #[serde(rename = "type")]
    pub tool_type: &'static str,
    /// Fixed tool name: `web_search` or `web_fetch`.
    pub name: &'static str,
    /// Cap on calls per request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// Citations over fetched documents (web fetch only; web search always
    /// cites).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<CitationsConfig>,
    /// Cache breakpoint after this tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// Who may call the tool: `["direct"]` (Claude itself) or a code
    /// execution version (dynamic filtering). Omitted means the API
    /// default, which from the `_20260209` versions on is code execution;
    /// models without programmatic tool calling must send `["direct"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_callers: Vec<&'static str>,
}

/// `allowed_callers` value for direct calls (no dynamic filtering).
pub const CALLER_DIRECT: &str = "direct";

impl ServerToolDef {
    /// The web search tool.
    #[must_use]
    pub const fn web_search(max_uses: Option<u32>) -> Self {
        Self {
            tool_type: WEB_SEARCH_TOOL_TYPE,
            name: "web_search",
            max_uses,
            citations: None,
            cache_control: None,
            allowed_callers: Vec::new(),
        }
    }

    /// The web fetch tool, with citations enabled so answers can link the
    /// fetched passages.
    #[must_use]
    pub const fn web_fetch(max_uses: Option<u32>) -> Self {
        Self {
            tool_type: WEB_FETCH_TOOL_TYPE,
            name: "web_fetch",
            max_uses,
            citations: Some(CitationsConfig { enabled: true }),
            cache_control: None,
            allowed_callers: Vec::new(),
        }
    }
}

/// `{"enabled": bool}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationsConfig {
    /// Whether citations are produced.
    pub enabled: bool,
}

/// `tool_choice`. `disable_parallel_tool_use` is not modelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireToolChoice {
    /// The model decides.
    Auto,
    /// The model must call some tool.
    Any,
    /// The model must call this tool.
    Tool {
        /// Tool name.
        name: String,
    },
    /// The model must not call tools.
    None,
}

/// `thinking`. The API also has `between_tools` (Claude Sonnet 5.5 only)
/// and an optional `display` field; neither is modelled yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireThinking {
    /// Fixed budget (older models; at least 1024 and below `max_tokens`).
    Enabled {
        /// Thinking token budget.
        budget_tokens: u32,
    },
    /// No thinking (rejected by models where thinking is always on).
    Disabled,
    /// The model decides how much to think.
    Adaptive,
}

/// `metadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireMetadata {
    /// Opaque end-user id (no PII).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Response
// ---------------------------------------------------------------------------

/// A `message` object: the non-streaming response, and the `message` of a
/// `message_start` event (which starts with empty `content` and may lack
/// `usage`, `model` or `role`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagesResponse {
    /// Message id (`msg_...`).
    pub id: String,
    /// Model that answered.
    #[serde(default)]
    pub model: String,
    /// Always `assistant`.
    #[serde(default = "assistant_role")]
    pub role: WireRole,
    /// Generated blocks.
    #[serde(default)]
    pub content: Vec<WireContentBlock>,
    /// Why generation stopped; `null` in `message_start`. Kept as a string
    /// because the API adds values (see [`crate::convert::stop_reason`]).
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// The custom stop sequence that was produced, if any.
    #[serde(default)]
    pub stop_sequence: Option<String>,
    /// Details of a `refusal` stop; `null` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<StopDetails>,
    /// Token accounting.
    #[serde(default)]
    pub usage: WireUsage,
}

fn assistant_role() -> WireRole {
    WireRole::Assistant
}

/// `stop_details` of a refused answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopDetails {
    /// Always `refusal` today.
    #[serde(rename = "type")]
    pub kind: String,
    /// Policy category (`cyber`, `bio`, ...), if any.
    #[serde(default)]
    pub category: Option<String>,
    /// Human-readable explanation, not guaranteed to be stable.
    #[serde(default)]
    pub explanation: Option<String>,
}

/// `usage` of a response or of a `message_delta` event. In
/// `message_delta` the counts are cumulative and `input_tokens` may be
/// `null`, hence the `Option`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WireUsage {
    /// Uncached input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output tokens, thinking included.
    #[serde(default)]
    pub output_tokens: u64,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Server tool calls billed in this response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUsage>,
}

/// `usage.server_tool_use`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServerToolUsage {
    /// Web searches performed.
    #[serde(default)]
    pub web_search_requests: u64,
    /// Web fetches performed.
    #[serde(default)]
    pub web_fetch_requests: u64,
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_reference_response() {
        // Response example of the API reference (container and diagnostics
        // are not modelled and are ignored).
        let raw = json!({
            "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF",
            "container": {"id": "container_011CpZohnwH4vuy7gazohgSP", "expires_at": "2019-12-27T18:11:19.117Z"},
            "content": [{
                "citations": [{
                    "cited_text": "The grass is green. The sky is blue.",
                    "document_index": 0,
                    "document_title": "My Document",
                    "end_char_index": 0,
                    "file_id": "file_011CNha8iCJcU1wXNR6q4V8w",
                    "start_char_index": 0,
                    "type": "char_location"
                }],
                "text": "Hi! My name is Claude.",
                "type": "text"
            }],
            "diagnostics": {"cache_miss_reason": {"cache_missed_input_tokens": 0, "type": "model_changed"}},
            "model": "claude-opus-5-5",
            "role": "assistant",
            "stop_details": {
                "category": "cyber",
                "explanation": "This request was declined because it conflicts with Anthropic's Usage Policy.",
                "type": "refusal"
            },
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "type": "message",
            "usage": {
                "cache_creation": {"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 0},
                "cache_creation_input_tokens": 2051,
                "cache_read_input_tokens": 2051,
                "inference_geo": "global",
                "input_tokens": 2095,
                "output_tokens": 503,
                "output_tokens_details": {"thinking_tokens": 0},
                "server_tool_use": {"web_fetch_requests": 2, "web_search_requests": 0},
                "service_tier": "standard",
                "speed": "standard"
            }
        });
        let resp: MessagesResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(resp.role, WireRole::Assistant);
        assert_eq!(
            resp.stop_details.unwrap().category.as_deref(),
            Some("cyber")
        );
        assert_eq!(
            resp.usage,
            WireUsage {
                input_tokens: Some(2095),
                output_tokens: 503,
                cache_creation_input_tokens: Some(2051),
                cache_read_input_tokens: Some(2051),
                server_tool_use: Some(ServerToolUsage {
                    web_search_requests: 0,
                    web_fetch_requests: 2
                }),
            }
        );
        assert!(matches!(
            &resp.content[0],
            WireContentBlock::Text { citations: Some(c), .. } if matches!(c[0], WireCitation::CharLocation { .. })
        ));
    }

    #[test]
    fn parses_web_search_response() {
        // From the web search tool docs.
        let raw = json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "I'll search for when Claude Shannon was born."},
                {
                    "type": "server_tool_use",
                    "id": "srvtoolu_01WYG3ziw53XMcoyKL4XcZmE",
                    "name": "web_search",
                    "input": {"query": "claude shannon birth date"}
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_01WYG3ziw53XMcoyKL4XcZmE",
                    "content": [{
                        "type": "web_search_result",
                        "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
                        "title": "Claude Shannon - Wikipedia",
                        "encrypted_content": "EqgfCioIARgBIiQ3YTAwMjY1Mi1mZjM5LTQ1NGUtODgxNC1kNjNjNTk1ZWI3Y...",
                        "page_age": "April 30, 2025"
                    }]
                },
                {"text": "Based on the search results, ", "type": "text"}
            ],
            "id": "msg_a930390d3a",
            "usage": {"input_tokens": 6039, "output_tokens": 931, "server_tool_use": {"web_search_requests": 1}},
            "stop_reason": "end_turn"
        });
        let resp: MessagesResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(resp.content.len(), 4);
        assert_eq!(resp.usage.server_tool_use.unwrap().web_search_requests, 1);
        assert_eq!(resp.usage.cache_read_input_tokens, None);
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Body of a non-2xx response:
/// `{"type": "error", "error": {"type": ..., "message": ...}, "request_id": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Always `error`.
    #[serde(rename = "type", default = "error_kind")]
    pub kind: String,
    /// The error itself.
    pub error: ApiError,
    /// Request id, useful when reporting problems to Anthropic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn error_kind() -> String {
    "error".to_owned()
}

/// The `error` object of an error body or of a stream `error` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct ApiError {
    /// E.g. `invalid_request_error`, `authentication_error`,
    /// `rate_limit_error`, `overloaded_error`. Kept as a string because the
    /// API adds types over time.
    #[serde(rename = "type")]
    pub kind: String,
    /// Human-readable message.
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn parses_error_body() {
        // From the API errors docs.
        let raw = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "messages: roles must alternate between \"user\" and \"assistant\""
            },
            "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"
        }"#;
        let body: ApiErrorBody = serde_json::from_str(raw).unwrap();
        assert_eq!(body.error.kind, "invalid_request_error");
        assert_eq!(
            body.request_id.as_deref(),
            Some("req_011CSHoEeqs5C35K2UUqR7Fy")
        );
        assert_eq!(
            body.error.to_string(),
            "invalid_request_error: messages: roles must alternate between \"user\" and \"assistant\""
        );
    }

    #[test]
    fn parses_error_body_without_request_id() {
        let raw = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let body: ApiErrorBody = serde_json::from_str(raw).unwrap();
        assert_eq!(body.error.kind, "overloaded_error");
        assert_eq!(body.request_id, None);
    }
}

// ---------------------------------------------------------------------------
// Streaming events
// ---------------------------------------------------------------------------

/// The JSON `data` of one server-sent event of a streaming response. The
/// SSE `event:` name repeats the `type`, so only `data` needs parsing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", remote = "Self")]
pub enum StreamEvent {
    /// First event: the message with empty `content`.
    MessageStart {
        /// Message skeleton (id, model, initial usage).
        message: MessagesResponse,
    },
    /// A content block opens at `index`.
    ContentBlockStart {
        /// Position in the final `content` array.
        index: usize,
        /// Initial block (empty text, `{}` tool input, ...). Server tool
        /// results arrive complete here, with no deltas.
        content_block: WireContentBlock,
    },
    /// Incremental update of the block at `index`.
    ContentBlockDelta {
        /// Block position.
        index: usize,
        /// The update.
        delta: ContentDelta,
    },
    /// The block at `index` is complete.
    ContentBlockStop {
        /// Block position.
        index: usize,
    },
    /// Top-level changes to the message (stop reason, usage).
    MessageDelta {
        /// Changed fields.
        delta: MessageDeltaBody,
        /// Cumulative usage, when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<WireUsage>,
    },
    /// Last event.
    MessageStop,
    /// Keep-alive.
    Ping,
    /// Mid-stream error (e.g. `overloaded_error`).
    Error {
        /// The error.
        error: ApiError,
    },
    /// Any event type this crate does not model, kept verbatim.
    #[serde(skip)]
    Unknown(Value),
}

tolerant_tagged!(
    StreamEvent,
    [
        "message_start",
        "content_block_start",
        "content_block_delta",
        "content_block_stop",
        "message_delta",
        "message_stop",
        "ping",
        "error",
    ]
);

/// `delta` of a `content_block_delta` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", remote = "Self")]
pub enum ContentDelta {
    /// More text for a `text` block.
    TextDelta {
        /// Text to append.
        text: String,
    },
    /// More reasoning for a `thinking` block.
    ThinkingDelta {
        /// Reasoning to append.
        thinking: String,
    },
    /// The signature of a `thinking` block, sent just before its stop.
    SignatureDelta {
        /// Full signature.
        signature: String,
    },
    /// A fragment of a `tool_use`/`server_tool_use` input. Concatenate the
    /// fragments and parse once the block stops.
    InputJsonDelta {
        /// JSON fragment (may be empty).
        partial_json: String,
    },
    /// One citation to add to the current `text` block.
    CitationsDelta {
        /// The citation.
        citation: WireCitation,
    },
    /// Any delta type this crate does not model, kept verbatim.
    #[serde(skip)]
    Unknown(Value),
}

tolerant_tagged!(
    ContentDelta,
    [
        "text_delta",
        "thinking_delta",
        "signature_delta",
        "input_json_delta",
        "citations_delta",
    ]
);

/// `delta` of a `message_delta` event.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MessageDeltaBody {
    /// Why generation stopped.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// The custom stop sequence produced, if any.
    #[serde(default)]
    pub stop_sequence: Option<String>,
    /// Details of a `refusal` stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<StopDetails>,
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn events(sse: &str) -> Vec<StreamEvent> {
        sse.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect()
    }

    #[test]
    fn parses_basic_text_stream() {
        // From the streaming docs ("Basic streaming request").
        let sse = r#"
event: message_start
data: {"type": "message_start", "message": {"id": "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 25, "output_tokens": 1}}}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence":null}, "usage": {"output_tokens": 15}}

event: message_stop
data: {"type": "message_stop"}
"#;
        let ev = events(sse);
        assert!(
            matches!(&ev[0], StreamEvent::MessageStart { message } if message.usage.input_tokens == Some(25))
        );
        assert_eq!(
            ev[1],
            StreamEvent::ContentBlockStart {
                index: 0,
                content_block: WireContentBlock::text("")
            }
        );
        assert_eq!(ev[2], StreamEvent::Ping);
        assert_eq!(
            ev[3],
            StreamEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::TextDelta {
                    text: "Hello".into()
                }
            }
        );
        assert_eq!(ev[4], StreamEvent::ContentBlockStop { index: 0 });
        assert_eq!(
            ev[5],
            StreamEvent::MessageDelta {
                delta: MessageDeltaBody {
                    stop_reason: Some("end_turn".into()),
                    ..Default::default()
                },
                usage: Some(WireUsage {
                    output_tokens: 15,
                    ..Default::default()
                }),
            }
        );
        assert_eq!(ev[6], StreamEvent::MessageStop);
    }

    #[test]
    fn parses_tool_use_and_thinking_deltas() {
        // From the streaming docs (tool use and thinking examples).
        let sse = r#"
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01T1x1fJ34qAmk2tNTrN7Up6","name":"get_weather","input":{}}}
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}
data: {"type": "message_start", "message": {"id": "msg_01...", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null}}
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "\n462 = 3 × 147 + 21"}}
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds..."}}
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}}
"#;
        let ev = events(sse);
        assert!(matches!(
            &ev[0],
            StreamEvent::ContentBlockStart { index: 1, content_block: WireContentBlock::ToolUse { name, .. } } if name == "get_weather"
        ));
        assert_eq!(
            ev[1],
            StreamEvent::ContentBlockDelta {
                index: 1,
                delta: ContentDelta::InputJsonDelta {
                    partial_json: "{\"location\":".into()
                }
            }
        );
        assert!(
            matches!(&ev[2], StreamEvent::MessageStart { message } if message.usage == WireUsage::default())
        );
        assert!(matches!(
            &ev[4],
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::ThinkingDelta { .. },
                ..
            }
        ));
        assert!(matches!(
            &ev[5],
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::SignatureDelta { .. },
                ..
            }
        ));
        assert!(matches!(
            &ev[6],
            StreamEvent::MessageDelta { usage: None, .. }
        ));
    }

    #[test]
    fn parses_web_search_stream() {
        // From the streaming docs ("Streaming request with web search tool use").
        let sse = r#"
data: {"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_014hJH82Qum7Td6UV8gDXThB","name":"web_search","input":{}}}
data: {"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_014hJH82Qum7Td6UV8gDXThB","content":[{"type":"web_search_result","title":"Weather in New York City in May 2025 (New York) - detailed Weather Forecast for a month","url":"https://world-weather.info/forecast/usa/new_york/may-2025/","encrypted_content":"Ev0DCioIAxgCIiQ3NmU4ZmI4OC1k...","page_age":null}]}}
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":10682,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":510,"server_tool_use":{"web_search_requests":1}}}
"#;
        let ev = events(sse);
        assert!(matches!(
            &ev[0],
            StreamEvent::ContentBlockStart {
                content_block: WireContentBlock::ServerToolUse { .. },
                ..
            }
        ));
        assert!(matches!(
            &ev[1],
            StreamEvent::ContentBlockStart {
                content_block: WireContentBlock::WebSearchToolResult { .. },
                ..
            }
        ));
        assert!(matches!(
            &ev[2],
            StreamEvent::MessageDelta { usage: Some(u), .. } if u.server_tool_use.is_some_and(|s| s.web_search_requests == 1)
        ));
    }

    #[test]
    fn parses_citations_delta_and_error() {
        let ev = events(
            r#"
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": {"type": "char_location", "cited_text": "The grass is green.", "document_index": 0, "document_title": "My Document", "start_char_index": 0, "end_char_index": 20}}}
data: {"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}
"#,
        );
        assert!(matches!(
            &ev[0],
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::CitationsDelta {
                    citation: WireCitation::CharLocation { .. }
                },
                ..
            }
        ));
        assert_eq!(
            ev[1],
            StreamEvent::Error {
                error: ApiError {
                    kind: "overloaded_error".into(),
                    message: "Overloaded".into()
                }
            }
        );
    }

    #[test]
    fn unknown_event_and_delta_are_tolerated() {
        let raw = r#"{"type": "future_event", "payload": {"x": 1}}"#;
        let event: StreamEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(
            event,
            StreamEvent::Unknown(serde_json::from_str(raw).unwrap())
        );

        let raw = r#"{"type": "content_block_delta", "index": 3, "delta": {"type": "future_delta", "x": 1}}"#;
        let event: StreamEvent = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            event,
            StreamEvent::ContentBlockDelta {
                index: 3,
                delta: ContentDelta::Unknown(_)
            }
        ));

        let raw = r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "fallback", "from": {"model": "a"}}}"#;
        let event: StreamEvent = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            event,
            StreamEvent::ContentBlockStart {
                content_block: WireContentBlock::Unknown(_),
                ..
            }
        ));
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn minimal_request_omits_optional_fields() {
        let req = MessagesRequest::new(
            "claude-opus-5-5",
            1024,
            vec![WireMessage {
                role: WireRole::User,
                content: vec![WireContentBlock::text("Hello")],
            }],
        );
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({
                "model": "claude-opus-5-5",
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]
            })
        );
    }

    #[test]
    fn optional_fields_use_api_shapes() {
        let mut req = MessagesRequest::new("m", 2048, vec![]);
        req.system = vec![SystemContent {
            text: "sys".into(),
            cache_control: Some(CacheControl::EPHEMERAL),
        }];
        req.tools = vec![
            WireTool::Custom(CustomTool {
                name: "get_weather".into(),
                description: "Weather".into(),
                input_schema: json!({"type": "object"}),
                cache_control: Some(CacheControl {
                    ttl: Some(CacheTtl::OneHour),
                }),
            }),
            WireTool::Server(ServerToolDef::web_search(Some(5))),
            WireTool::Server(ServerToolDef::web_fetch(None)),
        ];
        req.tool_choice = Some(WireToolChoice::Tool {
            name: "get_weather".into(),
        });
        req.thinking = Some(WireThinking::Enabled {
            budget_tokens: 1024,
        });
        req.stream = true;
        req.metadata = Some(WireMetadata {
            user_id: Some("u".into()),
        });
        req.stop_sequences = vec!["STOP".into()];
        req.cache_control = Some(CacheControl::EPHEMERAL);
        if let WireTool::Server(web_fetch) = &mut req.tools[2] {
            web_fetch.allowed_callers = vec![CALLER_DIRECT];
        }
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({
                "model": "m",
                "max_tokens": 2048,
                "system": [{"type": "text", "text": "sys", "cache_control": {"type": "ephemeral"}}],
                "messages": [],
                "tools": [
                    {
                        "name": "get_weather",
                        "description": "Weather",
                        "input_schema": {"type": "object"},
                        "cache_control": {"type": "ephemeral", "ttl": "1h"}
                    },
                    {"type": "web_search_20260318", "name": "web_search", "max_uses": 5},
                    {"type": "web_fetch_20260318", "name": "web_fetch", "citations": {"enabled": true}, "allowed_callers": ["direct"]}
                ],
                "tool_choice": {"type": "tool", "name": "get_weather"},
                "thinking": {"type": "enabled", "budget_tokens": 1024},
                "stream": true,
                "metadata": {"user_id": "u"},
                "stop_sequences": ["STOP"],
                "cache_control": {"type": "ephemeral"}
            })
        );
        assert_eq!(
            serde_json::to_value(WireThinking::Adaptive).unwrap(),
            json!({"type": "adaptive"})
        );
        assert_eq!(
            serde_json::to_value(WireToolChoice::None).unwrap(),
            json!({"type": "none"})
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> WireContentBlock {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn unknown_block_round_trips_verbatim() {
        let raw = json!({"type": "container_upload", "file_id": "file_1"});
        let block = parse(raw.clone());
        assert_eq!(block, WireContentBlock::Unknown(raw.clone()));
        assert_eq!(serde_json::to_value(&block).unwrap(), raw);
    }

    #[test]
    fn malformed_known_block_is_an_error() {
        let res = serde_json::from_value::<WireContentBlock>(json!({"type": "text"}));
        assert!(res.is_err());
    }

    #[test]
    fn text_with_web_search_citation() {
        // From the web search tool docs.
        let raw = json!({
            "text": "Claude Shannon was born on April 30, 1916, in Petoskey, Michigan",
            "type": "text",
            "citations": [{
                "type": "web_search_result_location",
                "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
                "title": "Claude Shannon - Wikipedia",
                "encrypted_index": "Eo8BCioIAhgBIiQyYjQ0OWJmZi1lNm..",
                "cited_text": "Claude Elwood Shannon (April 30, 1916 – February 24, 2001) was an American mathematician..."
            }]
        });
        let block = parse(raw.clone());
        let WireContentBlock::Text {
            citations: Some(c), ..
        } = &block
        else {
            panic!("expected cited text, got {block:?}");
        };
        assert!(matches!(c[0], WireCitation::WebSearchResultLocation { .. }));
        assert_eq!(serde_json::to_value(&block).unwrap(), raw);
    }

    #[test]
    fn text_with_null_citations() {
        let block = parse(json!({"type": "text", "text": "hi", "citations": null}));
        assert_eq!(block, WireContentBlock::text("hi"));
    }

    #[test]
    fn unknown_citation_is_kept() {
        let raw = json!({"type": "search_result_location", "cited_text": "x", "source": "s"});
        let citation: WireCitation = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(citation, WireCitation::Unknown(raw));
    }

    #[test]
    fn server_tool_blocks_round_trip() {
        // From the web search and web fetch tool docs, plus a `caller` field.
        let blocks = json!([
            {
                "type": "server_tool_use",
                "id": "srvtoolu_01WYG3ziw53XMcoyKL4XcZmE",
                "name": "web_search",
                "input": {"query": "claude shannon birth date"},
                "caller": {"type": "direct"}
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_01WYG3ziw53XMcoyKL4XcZmE",
                "content": [{
                    "type": "web_search_result",
                    "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
                    "title": "Claude Shannon - Wikipedia",
                    "encrypted_content": "EqgfCioIARgBIiQ3YTAwMjY1Mi1mZjM5LTQ1NGUtODgxNC1kNjNjNTk1ZWI3Y...",
                    "page_age": "April 30, 2025"
                }]
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_a93jad",
                "content": {"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}
            },
            {
                "type": "web_fetch_tool_result",
                "tool_use_id": "srvtoolu_01234567890abcdef",
                "content": {
                    "type": "web_fetch_result",
                    "url": "https://example.com/article",
                    "content": {
                        "type": "document",
                        "source": {"type": "text", "media_type": "text/plain", "data": "Full text content of the article..."},
                        "title": "Article Title",
                        "citations": {"enabled": true}
                    },
                    "retrieved_at": "2025-08-25T10:30:00Z"
                }
            },
            {
                "type": "web_fetch_tool_result",
                "tool_use_id": "srvtoolu_a93jad",
                "content": {"type": "web_fetch_tool_result_error", "error_code": "url_not_accessible"}
            }
        ]);
        let parsed: Vec<WireContentBlock> = serde_json::from_value(blocks.clone()).unwrap();
        assert!(matches!(
            &parsed[1],
            WireContentBlock::WebSearchToolResult { content: WebSearchToolResultContent::Results(r), .. } if r.len() == 1
        ));
        assert!(matches!(
            &parsed[2],
            WireContentBlock::WebSearchToolResult { content: WebSearchToolResultContent::Error(e), .. }
                if e.error_code == "max_uses_exceeded"
        ));
        assert!(matches!(
            &parsed[3],
            WireContentBlock::WebFetchToolResult {
                content: WebFetchToolResultContent::Result(_),
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), blocks);
    }

    #[test]
    fn known_tags_cover_every_typed_variant() {
        let samples = [
            WireContentBlock::text("t"),
            WireContentBlock::Thinking {
                thinking: String::new(),
                signature: String::new(),
            },
            WireContentBlock::RedactedThinking {
                data: String::new(),
            },
            WireContentBlock::ToolUse {
                id: "i".into(),
                name: "n".into(),
                input: json!({}),
            },
            WireContentBlock::ToolResult {
                tool_use_id: "i".into(),
                content: None,
                is_error: false,
                cache_control: None,
            },
            WireContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: "image/png".into(),
                    data: String::new(),
                },
                cache_control: None,
            },
            WireContentBlock::ServerToolUse {
                id: "i".into(),
                name: "web_search".into(),
                input: json!({}),
                extra: Map::new(),
            },
            WireContentBlock::WebSearchToolResult {
                tool_use_id: "i".into(),
                content: WebSearchToolResultContent::Results(vec![]),
                extra: Map::new(),
            },
            WireContentBlock::WebFetchToolResult {
                tool_use_id: "i".into(),
                content: WebFetchToolResultContent::Other(json!(null)),
                extra: Map::new(),
            },
        ];
        assert_eq!(samples.len(), WireContentBlock::KNOWN_TAGS.len());
        for block in samples {
            let value = serde_json::to_value(&block).unwrap();
            let tag = value["type"].as_str().unwrap();
            assert!(WireContentBlock::KNOWN_TAGS.contains(&tag), "{tag}");
            assert_eq!(parse(value), block);
        }
    }
}
