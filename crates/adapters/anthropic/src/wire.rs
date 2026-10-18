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
