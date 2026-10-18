//! Pure conversions between the domain/port types and the wire types.

use application::ports::llm::{Citation, LlmRequest, ServerTool, ToolChoice, ToolDefinition};
use domain::conversation::{
    ContentPart, Message, Role, StopReason, ToolResultContent, ToolUseId, Usage,
};
use domain::model::{ModelId, ThinkingConfig};

use crate::wire::{
    CacheControl, CustomTool, ImageSource, MessagesRequest, ServerToolDef, SystemContent,
    WireCitation, WireContentBlock, WireMessage, WireRole, WireThinking, WireTool, WireToolChoice,
    WireUsage,
};

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
        // Echoed verbatim: `Unknown` serialises its JSON unchanged.
        ContentPart::ProviderBlock { raw } => WireContentBlock::Unknown(raw.clone()),
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
/// Server tool calls and results, and block types this crate does not
/// model, become [`ContentPart::ProviderBlock`] with their full JSON, since
/// the API requires assistant blocks back unchanged in later turns. Returns
/// `None` for request-only blocks (`tool_result`). Citations on text are
/// not kept; read them with [`citation`].
#[must_use]
pub fn response_block(block: WireContentBlock) -> Option<ContentPart> {
    match block {
        block @ (WireContentBlock::ServerToolUse { .. }
        | WireContentBlock::WebSearchToolResult { .. }
        | WireContentBlock::WebFetchToolResult { .. }) => provider_block(&block),
        WireContentBlock::Unknown(raw) => Some(ContentPart::ProviderBlock { raw }),
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
        WireContentBlock::ToolResult { .. } => None,
    }
}

/// Wraps a wire block as an opaque [`ContentPart::ProviderBlock`].
#[must_use]
pub fn provider_block(block: &WireContentBlock) -> Option<ContentPart> {
    serde_json::to_value(block)
        .ok()
        .map(|raw| ContentPart::ProviderBlock { raw })
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

/// Builds the Messages API body of `req` for one model of its fallback
/// chain.
///
/// - `max_tokens` is [`LlmRequest::effective_max_tokens`].
/// - System blocks with `cache: true` get a `cache_control` breakpoint.
/// - Client tools come first; only the last one marked `cache: true` gets
///   a breakpoint, since one breakpoint already caches every tool before
///   it. Server tools follow, after the cached prefix.
/// - Thinking comes from `model.spec.thinking`. `Disabled` omits the
///   parameter instead of sending `{"type": "disabled"}`: older models then
///   run without thinking, while models where thinking is always on reject
///   an explicit `disabled` with a 400.
/// - `tool_choice` is omitted when it is `auto` (the API default) or when
///   there are no tools. With thinking on, the API only accepts `auto` and
///   `none`, so a forced choice (`any`, a named tool) is downgraded to
///   `auto`; models that reject forced tool use even without thinking are
///   not handled here.
/// - `temperature` is never set (rejected by recent models) and
///   `metadata` is not sent: [`LlmRequest::metadata`] is for local traces.
#[must_use]
pub fn messages_request(req: &LlmRequest, model: &ModelId, stream: bool) -> MessagesRequest {
    let thinking = thinking(req.model.spec.thinking);
    let has_tools = !req.tools.is_empty() || !req.server_tools.is_empty();
    let mut out = MessagesRequest::new(
        model.as_str(),
        req.effective_max_tokens(),
        req.messages.iter().map(message).collect(),
    );
    out.system = req
        .system
        .iter()
        .map(|block| SystemContent {
            text: block.text.clone(),
            cache_control: block.cache.then_some(CacheControl::EPHEMERAL),
        })
        .collect();
    out.tools = tools(&req.tools, &req.server_tools);
    out.tool_choice = if has_tools {
        tool_choice(&req.tool_choice, thinking.is_some())
    } else {
        None
    };
    out.thinking = thinking;
    out.stream = stream;
    out
}

fn tools(custom: &[ToolDefinition], server: &[ServerTool]) -> Vec<WireTool> {
    let last_cached = custom.iter().rposition(|tool| tool.cache);
    let custom = custom.iter().enumerate().map(|(i, tool)| {
        WireTool::Custom(CustomTool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            cache_control: (Some(i) == last_cached).then_some(CacheControl::EPHEMERAL),
        })
    });
    let server = server.iter().map(|tool| {
        WireTool::Server(match *tool {
            ServerTool::WebSearch { max_uses } => ServerToolDef::web_search(max_uses),
            ServerTool::WebFetch { max_uses } => ServerToolDef::web_fetch(max_uses),
        })
    });
    custom.chain(server).collect()
}

/// Maps the domain thinking config (see [`messages_request`] for why
/// `Disabled` is omitted).
#[must_use]
pub const fn thinking(config: ThinkingConfig) -> Option<WireThinking> {
    match config {
        ThinkingConfig::Disabled => None,
        ThinkingConfig::Enabled { budget_tokens } => Some(WireThinking::Enabled { budget_tokens }),
        ThinkingConfig::Adaptive => Some(WireThinking::Adaptive),
    }
}

pub(crate) fn tool_choice(choice: &ToolChoice, thinking: bool) -> Option<WireToolChoice> {
    match choice {
        ToolChoice::Auto => None,
        ToolChoice::Any | ToolChoice::Tool(_) if thinking => None,
        ToolChoice::Any => Some(WireToolChoice::Any),
        ToolChoice::Tool(name) => Some(WireToolChoice::Tool { name: name.clone() }),
        ToolChoice::None => Some(WireToolChoice::None),
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use application::ports::llm::SystemBlock;
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use serde_json::json;

    fn resolved(spec: ModelSpec) -> ResolvedModel {
        ResolvedModel {
            role: ModelRole::Main,
            spec,
            source: SpecSource::Default,
            warnings: vec![],
        }
    }

    fn body(req: &LlmRequest, stream: bool) -> serde_json::Value {
        serde_json::to_value(messages_request(req, &req.model.spec.id, stream)).unwrap()
    }

    #[test]
    fn minimal_request() {
        let req = LlmRequest::new(
            resolved(ModelSpec::new("claude-haiku-5-5", 4000)),
            vec![Message::user_text("Hello")],
        );
        insta::assert_json_snapshot!(messages_request(&req, &req.model.spec.id, false));
    }

    #[test]
    fn request_with_tools_cache_thinking_and_server_tools() {
        let spec = ModelSpec::new("claude-opus-4-6", 16000)
            .with_thinking(ThinkingConfig::Enabled {
                budget_tokens: 8000,
            })
            .with_fallbacks(["claude-sonnet-4-6"]);
        let mut req = LlmRequest::new(resolved(spec), vec![Message::user_text("Find the bug")]);
        req.system = vec![
            SystemBlock::cached("You are crest, a coding agent."),
            SystemBlock::new("Workspace: /tmp/project"),
        ];
        // Keys in sorted order so the snapshot does not depend on whether
        // serde_json's `preserve_order` feature is unified in.
        let schema = json!({
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "type": "object"
        });
        req.tools = vec![
            ToolDefinition::new("read_file", "Read a file", schema.clone()).cached(),
            ToolDefinition::new("grep", "Search file contents", schema.clone()).cached(),
            ToolDefinition::new("shell", "Run a command", schema),
        ];
        req.server_tools = vec![
            ServerTool::WebSearch { max_uses: Some(5) },
            ServerTool::WebFetch { max_uses: None },
        ];
        req.max_tokens = Some(20000);
        insta::assert_json_snapshot!(messages_request(&req, &req.model.spec.id, true));

        // Same request sent to the fallback model.
        let fallback = &req.model.spec.fallbacks[0];
        assert_eq!(
            messages_request(&req, fallback, true).model,
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn thinking_variants() {
        assert_eq!(thinking(ThinkingConfig::Disabled), None);
        assert_eq!(
            thinking(ThinkingConfig::Adaptive),
            Some(WireThinking::Adaptive)
        );
        assert_eq!(
            thinking(ThinkingConfig::Enabled {
                budget_tokens: 2048
            }),
            Some(WireThinking::Enabled {
                budget_tokens: 2048
            })
        );
    }

    #[test]
    fn tool_choice_rules() {
        let tool = || ToolDefinition::new("t", "d", json!({"type": "object"}));
        let mut req = LlmRequest::new(resolved(ModelSpec::new("m", 1024)), vec![]);
        req.tool_choice = ToolChoice::Tool("t".into());
        // No tools: tool_choice is never sent.
        assert_eq!(body(&req, false).get("tool_choice"), None);

        req.tools = vec![tool()];
        assert_eq!(
            body(&req, false)["tool_choice"],
            json!({"type": "tool", "name": "t"})
        );
        req.tool_choice = ToolChoice::Any;
        assert_eq!(body(&req, false)["tool_choice"], json!({"type": "any"}));
        req.tool_choice = ToolChoice::None;
        assert_eq!(body(&req, false)["tool_choice"], json!({"type": "none"}));
        req.tool_choice = ToolChoice::Auto;
        assert_eq!(body(&req, false).get("tool_choice"), None);

        // With thinking on, forced choices fall back to auto; none stays.
        req.model.spec.thinking = ThinkingConfig::Adaptive;
        req.tool_choice = ToolChoice::Any;
        assert_eq!(body(&req, false).get("tool_choice"), None);
        req.tool_choice = ToolChoice::Tool("t".into());
        assert_eq!(body(&req, false).get("tool_choice"), None);
        req.tool_choice = ToolChoice::None;
        assert_eq!(body(&req, false)["tool_choice"], json!({"type": "none"}));
    }

    #[test]
    fn server_tools_alone_allow_tool_choice() {
        let mut req = LlmRequest::new(resolved(ModelSpec::new("m", 1024)), vec![]);
        req.server_tools = vec![ServerTool::WebSearch { max_uses: None }];
        req.tool_choice = ToolChoice::None;
        let body = body(&req, false);
        assert_eq!(
            body["tools"],
            json!([{"type": "web_search_20260318", "name": "web_search"}])
        );
        assert_eq!(body["tool_choice"], json!({"type": "none"}));
    }
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
            {"type": "tool_result", "tool_use_id": "toolu_0"},
            {"type": "code_execution_tool_result", "tool_use_id": "srvtoolu_1", "content": {}}
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
                ContentPart::ProviderBlock {
                    raw: json!({"type": "code_execution_tool_result", "tool_use_id": "srvtoolu_1", "content": {}})
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
    fn server_tool_blocks_round_trip_as_provider_blocks() {
        let raw = json!([
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "q"}, "caller": {"type": "direct"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
                {"type": "web_search_result", "url": "https://a.b", "title": "A", "encrypted_content": "Eq", "page_age": null}
            ]},
            {"type": "web_fetch_tool_result", "tool_use_id": "srvtoolu_2", "content": {"type": "web_fetch_tool_result_error", "error_code": "url_not_accessible"}}
        ]);
        let blocks: Vec<WireContentBlock> = serde_json::from_value(raw.clone()).unwrap();
        let parts: Vec<ContentPart> = blocks.into_iter().filter_map(response_block).collect();
        assert_eq!(parts.len(), 3);
        assert!(
            matches!(&parts[0], ContentPart::ProviderBlock { raw } if raw["name"] == "web_search")
        );
        let echoed = message(&Message::assistant(parts));
        // `page_age: null` is dropped by the typed model; everything the API
        // needs back (ids, encrypted content, caller) is kept.
        let mut expected = raw;
        expected[1]["content"][0]
            .as_object_mut()
            .unwrap()
            .remove("page_age");
        assert_eq!(serde_json::to_value(&echoed.content).unwrap(), expected);
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
