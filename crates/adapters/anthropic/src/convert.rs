//! Pure conversions between the domain/port types and the wire types.

use application::ports::llm::Citation;
use domain::conversation::{
    ContentPart, Message, Role, StopReason, ToolResultContent, ToolUseId, Usage,
};

use crate::wire::{ImageSource, WireCitation, WireContentBlock, WireMessage, WireRole, WireUsage};

/// Converts a domain message to its wire form.
///
/// Thinking parts without a signature are dropped: the API rejects
/// unsigned thinking blocks, and only blocks produced by the API (which
/// always carry one) can be echoed back.
#[must_use]
pub fn message(message: &Message) -> WireMessage {
    WireMessage {
        role: role(message.role),
        content: message.content.iter().filter_map(content_part).collect(),
    }
}

/// Converts a domain role.
#[must_use]
pub const fn role(role: Role) -> WireRole {
    match role {
        Role::User => WireRole::User,
        Role::Assistant => WireRole::Assistant,
    }
}

/// Converts one content part; `None` for parts that cannot be sent (see
/// [`message`]).
#[must_use]
pub fn content_part(part: &ContentPart) -> Option<WireContentBlock> {
    Some(match part {
        ContentPart::Text { text } => WireContentBlock::text(text.clone()),
        ContentPart::Thinking {
            thinking,
            signature,
        } => WireContentBlock::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone()?,
        },
        ContentPart::RedactedThinking { data } => {
            WireContentBlock::RedactedThinking { data: data.clone() }
        }
        ContentPart::ToolUse { id, name, input } => WireContentBlock::ToolUse {
            id: id.as_str().to_owned(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentPart::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => WireContentBlock::ToolResult {
            tool_use_id: tool_use_id.as_str().to_owned(),
            content: (!content.is_empty()).then(|| {
                crate::wire::ToolResultContent::Blocks(
                    content.iter().map(tool_result_item).collect(),
                )
            }),
            is_error: *is_error,
            cache_control: None,
        },
        ContentPart::Image { media_type, data } => image(media_type, data),
    })
}

fn tool_result_item(item: &ToolResultContent) -> WireContentBlock {
    match item {
        ToolResultContent::Text { text } => WireContentBlock::text(text.clone()),
        ToolResultContent::Image { media_type, data } => image(media_type, data),
    }
}

fn image(media_type: &str, data: &str) -> WireContentBlock {
    WireContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: media_type.to_owned(),
            data: data.to_owned(),
        },
        cache_control: None,
    }
}

/// Converts a response block to a domain content part.
///
/// Returns `None` for blocks the domain does not model yet: server tool
/// calls and results, client-only blocks (`tool_result`) and unknown
/// types. Citations on text are not kept; read them with [`citation`].
#[must_use]
pub fn response_block(block: WireContentBlock) -> Option<ContentPart> {
    match block {
        WireContentBlock::Text { text, .. } => Some(ContentPart::Text { text }),
        WireContentBlock::Thinking {
            thinking,
            signature,
        } => Some(ContentPart::Thinking {
            thinking,
            signature: (!signature.is_empty()).then_some(signature),
        }),
        WireContentBlock::RedactedThinking { data } => Some(ContentPart::RedactedThinking { data }),
        WireContentBlock::ToolUse { id, name, input } => Some(ContentPart::ToolUse {
            id: ToolUseId::new(id),
            name,
            input,
        }),
        WireContentBlock::Image {
            source: ImageSource::Base64 { media_type, data },
            ..
        } => Some(ContentPart::Image { media_type, data }),
        WireContentBlock::ToolResult { .. }
        | WireContentBlock::ServerToolUse { .. }
        | WireContentBlock::WebSearchToolResult { .. }
        | WireContentBlock::WebFetchToolResult { .. }
        | WireContentBlock::Unknown(_) => None,
    }
}

/// Converts a web search citation to the port's [`Citation`]. Document
/// citations (`char_location`, `page_location`) carry no URL of their own
/// and return `None`; their URL is the one of the cited web fetch result.
#[must_use]
pub fn citation(citation: &WireCitation) -> Option<Citation> {
    match citation {
        WireCitation::WebSearchResultLocation {
            url,
            title,
            cited_text,
            ..
        } => Some(Citation {
            url: url.clone(),
            title: title.clone(),
            cited_text: cited_text.clone(),
        }),
        WireCitation::CharLocation { .. }
        | WireCitation::PageLocation { .. }
        | WireCitation::Unknown(_) => None,
    }
}

/// Converts API usage to domain usage; missing counts are zero and server
/// tool counts are dropped (the domain has no field for them).
#[must_use]
pub fn usage(usage: &WireUsage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens.unwrap_or(0),
        output_tokens: usage.output_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or(0),
        cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
    }
}

/// Parses an API `stop_reason`. `model_context_window_exceeded` maps to
/// [`StopReason::MaxTokens`] (generation was cut by a token limit); unknown
/// values return `None` so the caller decides.
#[must_use]
pub fn stop_reason(reason: &str) -> Option<StopReason> {
    Some(match reason {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "pause_turn" => StopReason::PauseTurn,
        "refusal" => StopReason::Refusal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_with_tool_use_result_and_image() {
        let id = ToolUseId::new("toolu_01");
        let history = [
            Message::new(
                Role::User,
                vec![
                    ContentPart::text("What is in this screenshot?"),
                    ContentPart::Image {
                        media_type: "image/png".into(),
                        data: "iVBORw0KGgo=".into(),
                    },
                ],
            ),
            Message::assistant(vec![
                ContentPart::Thinking {
                    thinking: "Let me read the file.".into(),
                    signature: Some("sig".into()),
                },
                ContentPart::Thinking {
                    thinking: "unsigned, dropped".into(),
                    signature: None,
                },
                ContentPart::RedactedThinking {
                    data: "opaque".into(),
                },
                ContentPart::ToolUse {
                    id: id.clone(),
                    name: "read_file".into(),
                    input: json!({"path": "a.rs"}),
                },
            ]),
            Message::new(
                Role::User,
                vec![
                    ContentPart::ToolResult {
                        tool_use_id: id.clone(),
                        content: vec![
                            ToolResultContent::text("fn main() {}"),
                            ToolResultContent::Image {
                                media_type: "image/jpeg".into(),
                                data: "/9j/".into(),
                            },
                        ],
                        is_error: false,
                    },
                    ContentPart::ToolResult {
                        tool_use_id: id,
                        content: vec![],
                        is_error: true,
                    },
                ],
            ),
        ];
        let wire: Vec<WireMessage> = history.iter().map(message).collect();
        insta::assert_json_snapshot!(wire);
    }

    #[test]
    fn response_blocks_to_domain() {
        let blocks: Vec<WireContentBlock> = serde_json::from_value(json!([
            {"type": "thinking", "thinking": "hmm", "signature": "sig"},
            {"type": "thinking", "thinking": "", "signature": ""},
            {"type": "text", "text": "hi", "citations": null},
            {"type": "tool_use", "id": "toolu_1", "name": "grep", "input": {"q": "x"}},
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}},
            {"type": "container_upload", "file_id": "f"}
        ]))
        .unwrap();
        let parts: Vec<ContentPart> = blocks.into_iter().filter_map(response_block).collect();
        assert_eq!(
            parts,
            vec![
                ContentPart::Thinking {
                    thinking: "hmm".into(),
                    signature: Some("sig".into())
                },
                ContentPart::Thinking {
                    thinking: String::new(),
                    signature: None
                },
                ContentPart::text("hi"),
                ContentPart::ToolUse {
                    id: ToolUseId::new("toolu_1"),
                    name: "grep".into(),
                    input: json!({"q": "x"})
                },
            ]
        );
    }

    #[test]
    fn domain_parts_round_trip_through_wire() {
        let parts = [
            ContentPart::text("t"),
            ContentPart::Thinking {
                thinking: "r".into(),
                signature: Some("s".into()),
            },
            ContentPart::RedactedThinking { data: "d".into() },
            ContentPart::ToolUse {
                id: ToolUseId::new("toolu_9"),
                name: "n".into(),
                input: json!({}),
            },
            ContentPart::Image {
                media_type: "image/gif".into(),
                data: "R0lG".into(),
            },
        ];
        for part in parts {
            assert_eq!(content_part(&part).and_then(response_block), Some(part));
        }
    }

    #[test]
    fn web_search_citation_to_port() {
        let wire: WireCitation = serde_json::from_value(json!({
            "type": "web_search_result_location",
            "url": "https://en.wikipedia.org/wiki/Claude_Shannon",
            "title": "Claude Shannon - Wikipedia",
            "encrypted_index": "Eo8B",
            "cited_text": "Claude Elwood Shannon"
        }))
        .unwrap();
        assert_eq!(
            citation(&wire),
            Some(Citation {
                url: "https://en.wikipedia.org/wiki/Claude_Shannon".into(),
                title: Some("Claude Shannon - Wikipedia".into()),
                cited_text: "Claude Elwood Shannon".into(),
            })
        );
    }

    #[test]
    fn usage_defaults_missing_counts_to_zero() {
        let wire = WireUsage {
            input_tokens: Some(2095),
            output_tokens: 503,
            cache_creation_input_tokens: Some(7),
            cache_read_input_tokens: None,
            server_tool_use: None,
        };
        assert_eq!(
            usage(&wire),
            Usage {
                input_tokens: 2095,
                output_tokens: 503,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 7,
            }
        );
    }

    #[test]
    fn stop_reasons() {
        assert_eq!(stop_reason("end_turn"), Some(StopReason::EndTurn));
        assert_eq!(stop_reason("tool_use"), Some(StopReason::ToolUse));
        assert_eq!(stop_reason("max_tokens"), Some(StopReason::MaxTokens));
        assert_eq!(
            stop_reason("model_context_window_exceeded"),
            Some(StopReason::MaxTokens)
        );
        assert_eq!(stop_reason("stop_sequence"), Some(StopReason::StopSequence));
        assert_eq!(stop_reason("pause_turn"), Some(StopReason::PauseTurn));
        assert_eq!(stop_reason("refusal"), Some(StopReason::Refusal));
        assert_eq!(stop_reason("something_new"), None);
    }
}
