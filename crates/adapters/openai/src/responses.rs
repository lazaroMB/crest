//! OpenAI Responses (`POST /responses`, streaming): the request body built
//! from an [`LlmRequest`] and the [`ResponsesDecoder`] of its answer.
//!
//! Request mapping: the system blocks become `instructions`; the history
//! goes whole in `input` (`store: false`, nothing is kept on the server):
//! user text and images as `input_text`/`input_image`, assistant text as
//! `output_text`, tool calls as `function_call` and tool results as
//! `function_call_output`. Thinking, redacted thinking and provider blocks
//! are not sent back (reasoning items need the server's encrypted state).
//! `ThinkingConfig` maps to a reasoning effort with summaries.
//!
//! Stream mapping, by `output_index`: `response.created` →
//! [`LlmEvent::MessageStart`]; text deltas → text blocks; reasoning
//! summary (or text) deltas → thinking blocks; a `function_call` item →
//! [`LlmEvent::ToolUseStart`] and its argument deltas;
//! `response.output_item.done` closes the item's block;
//! `response.completed`/`response.incomplete` → usage and
//! [`LlmEvent::MessageStop`]; `response.failed` and `error` → errors.

use std::collections::BTreeMap;

use application::ports::llm::{LlmError, LlmEvent, LlmRequest, ToolChoice};
use domain::conversation::{ContentPart, Role, StopReason, ToolUseId, Usage};
use domain::model::{ModelId, ThinkingConfig};
use serde_json::{Map, Value, json};

use crate::chat::{data_url, system_text, tool_result_text};
use crate::client::from_stream_error;
use crate::sse::{Decoder, json as parse};

/// The body of a streaming Responses request for `model`.
#[must_use]
pub fn build(req: &LlmRequest, model: &ModelId) -> Value {
    let mut input = Vec::new();
    for message in &req.messages {
        match message.role {
            Role::User => user_items(&message.content, &mut input),
            Role::Assistant => assistant_items(&message.content, &mut input),
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(model.as_str()));
    let instructions = system_text(req);
    if !instructions.is_empty() {
        body.insert("instructions".into(), json!(instructions));
    }
    body.insert("input".into(), Value::Array(input));
    body.insert(
        "max_output_tokens".into(),
        json!(req.effective_max_tokens()),
    );
    body.insert("stream".into(), json!(true));
    body.insert("store".into(), json!(false));
    if let Some(effort) = effort(req.model.spec.thinking) {
        body.insert(
            "reasoning".into(),
            json!({ "effort": effort, "summary": "auto" }),
        );
    }
    if !req.tools.is_empty() && req.tool_choice != ToolChoice::None {
        let tools = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        let choice = match &req.tool_choice {
            ToolChoice::Auto | ToolChoice::None => json!("auto"),
            ToolChoice::Any => json!("required"),
            ToolChoice::Tool(name) => json!({ "type": "function", "name": name }),
        };
        body.insert("tool_choice".into(), choice);
    }
    Value::Object(body)
}

/// The reasoning effort of a thinking configuration.
fn effort(thinking: ThinkingConfig) -> Option<&'static str> {
    match thinking {
        ThinkingConfig::Disabled => None,
        ThinkingConfig::Adaptive => Some("medium"),
        ThinkingConfig::Enabled { budget_tokens } if budget_tokens < 4_000 => Some("low"),
        ThinkingConfig::Enabled { budget_tokens } if budget_tokens < 16_000 => Some("medium"),
        ThinkingConfig::Enabled { .. } => Some("high"),
    }
}

fn user_items(content: &[ContentPart], out: &mut Vec<Value>) {
    let mut parts = Vec::new();
    for part in content {
        match part {
            ContentPart::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => out.push(json!({
                "type": "function_call_output",
                "call_id": tool_use_id.to_string(),
                "output": tool_result_text(content, *is_error),
            })),
            ContentPart::Text { text } => parts.push(json!({ "type": "input_text", "text": text })),
            ContentPart::Image { media_type, data } => parts.push(json!({
                "type": "input_image",
                "image_url": data_url(media_type, data),
            })),
            _ => {}
        }
    }
    if !parts.is_empty() {
        out.push(json!({ "role": "user", "content": parts }));
    }
}

fn assistant_items(content: &[ContentPart], out: &mut Vec<Value>) {
    for part in content {
        match part {
            ContentPart::Text { text } if !text.is_empty() => out.push(json!({
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text }],
            })),
            ContentPart::ToolUse { id, name, input } => out.push(json!({
                "type": "function_call",
                "call_id": id.to_string(),
                "name": name,
                "arguments": input.to_string(),
            })),
            _ => {}
        }
    }
}

/// Kind of the block of one output item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Text,
    Thinking,
    Tool,
}

/// Decodes the events of a Responses stream.
#[derive(Debug)]
pub struct ResponsesDecoder {
    requested: ModelId,
    started: bool,
    /// Blocks started, by output index.
    blocks: BTreeMap<u64, Item>,
    called: bool,
    stopped: bool,
}

impl ResponsesDecoder {
    /// A decoder for an answer of `requested`.
    #[must_use]
    pub fn new(requested: ModelId) -> Self {
        Self {
            requested,
            started: false,
            blocks: BTreeMap::new(),
            called: false,
            stopped: false,
        }
    }

    fn start(&mut self, response: Option<&Value>, out: &mut Vec<LlmEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let model = response
            .and_then(|r| r.get("model"))
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map_or_else(|| self.requested.clone(), ModelId::new);
        out.push(LlmEvent::MessageStart { model });
    }

    fn delta(&mut self, kind: Item, index: u64, delta: &str, out: &mut Vec<LlmEvent>) {
        let Ok(block) = usize::try_from(index) else {
            return;
        };
        match self.blocks.get(&index) {
            Some(open) if *open != kind => return,
            Some(_) => {}
            None if delta.is_empty() => return,
            None => {
                self.blocks.insert(index, kind);
            }
        }
        out.push(match kind {
            Item::Text => LlmEvent::TextDelta {
                index: block,
                text: delta.to_owned(),
            },
            Item::Thinking => LlmEvent::ThinkingDelta {
                index: block,
                thinking: delta.to_owned(),
            },
            Item::Tool => LlmEvent::ToolInputDelta {
                index: block,
                partial_json: delta.to_owned(),
            },
        });
    }

    fn complete(&mut self, response: Option<&Value>, out: &mut Vec<LlmEvent>) {
        for index in std::mem::take(&mut self.blocks).into_keys() {
            if let Ok(index) = usize::try_from(index) {
                out.push(LlmEvent::BlockStop { index });
            }
        }
        if let Some(value) = response
            .and_then(|r| r.get("usage"))
            .filter(|u| u.is_object())
        {
            out.push(LlmEvent::Usage(usage(value)));
        }
        let incomplete = response
            .and_then(|r| r.get("incomplete_details"))
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str);
        let stop_reason = match incomplete {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            _ if self.called => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        out.push(LlmEvent::MessageStop { stop_reason });
        self.stopped = true;
    }
}

fn usage(value: &Value) -> Usage {
    let count = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0);
    let input = count(value.get("input_tokens"));
    let cached = count(
        value
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens")),
    );
    Usage {
        input_tokens: input.saturating_sub(cached),
        output_tokens: count(value.get("output_tokens")),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
    }
}

impl Decoder for ResponsesDecoder {
    fn decode(&mut self, data: &str) -> Result<Vec<LlmEvent>, LlmError> {
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let event = parse(data)?;
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let text = || {
            event
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let mut out = Vec::new();
        match kind {
            "response.created" | "response.in_progress" => {
                self.start(event.get("response"), &mut out);
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.start(None, &mut out);
                self.delta(Item::Text, index, text(), &mut out);
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.start(None, &mut out);
                self.delta(Item::Thinking, index, text(), &mut out);
            }
            "response.output_item.added" => {
                self.start(None, &mut out);
                let item = event.get("item");
                if item.and_then(|i| i.get("type")).and_then(Value::as_str) == Some("function_call")
                    && let Ok(block) = usize::try_from(index)
                    && !self.blocks.contains_key(&index)
                {
                    let item = item.unwrap_or(&Value::Null);
                    let field = |name: &str| item.get(name).and_then(Value::as_str);
                    let id = field("call_id")
                        .or_else(|| field("id"))
                        .map_or_else(|| format!("call_{index}"), str::to_owned);
                    let name = field("name").unwrap_or_default().to_owned();
                    self.blocks.insert(index, Item::Tool);
                    self.called = true;
                    out.push(LlmEvent::ToolUseStart {
                        index: block,
                        id: ToolUseId::from(id.as_str()),
                        name,
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                self.delta(Item::Tool, index, text(), &mut out);
            }
            "response.output_item.done" => {
                if self.blocks.remove(&index).is_some()
                    && let Ok(index) = usize::try_from(index)
                {
                    out.push(LlmEvent::BlockStop { index });
                }
            }
            "response.completed" | "response.incomplete" => {
                self.start(event.get("response"), &mut out);
                self.complete(event.get("response"), &mut out);
            }
            "response.failed" => {
                let error = event
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .cloned()
                    .unwrap_or(Value::Null);
                return Err(from_stream_error(&error));
            }
            "error" => return Err(from_stream_error(event.get("error").unwrap_or(&event))),
            _ => {}
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        if self.stopped {
            return Ok(Vec::new());
        }
        Err(LlmError::Network(
            "the stream ended before response.completed".into(),
        ))
    }

    fn is_stopped(&self) -> bool {
        self.stopped
    }
}

#[cfg(test)]
mod tests {
    use application::ports::llm::{MessageAssembler, SystemBlock, ToolDefinition};
    use domain::conversation::Message;
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};

    use super::*;

    const TOOLS: &str = include_str!("../tests/fixtures/responses_tool_call.sse");

    fn decode_all(fixture: &str) -> Vec<LlmEvent> {
        let mut decoder = ResponsesDecoder::new(ModelId::from("gpt-6-luna"));
        let mut events = Vec::new();
        for line in fixture.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                events.extend(decoder.decode(data).unwrap());
            }
        }
        events.extend(decoder.finish().unwrap());
        events
    }

    #[test]
    fn reasoning_text_and_a_tool_call_stream_into_blocks() {
        let events = decode_all(TOOLS);
        insta::assert_debug_snapshot!(events);
        let mut assembler = MessageAssembler::new();
        for event in events {
            assembler.push(event).unwrap();
        }
        let message = assembler.finish().unwrap();
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert_eq!(message.usage.input_tokens, 70);
        assert_eq!(message.usage.cache_read_input_tokens, 30);
        let [
            ContentPart::Thinking { .. },
            ContentPart::Text { text },
            ContentPart::ToolUse { id, name, input },
        ] = message.message.content.as_slice()
        else {
            panic!("{:?}", message.message.content)
        };
        assert_eq!(text, "Lo busco.");
        assert_eq!(id.to_string(), "call_x");
        assert_eq!(name, "find_file");
        assert_eq!(input, &json!({ "query": "lib" }));
    }

    #[test]
    fn failures_are_typed_errors() {
        let mut decoder = ResponsesDecoder::new(ModelId::from("gpt-6-luna"));
        let failed = r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"slow down"}}}"#;
        assert!(matches!(
            decoder.decode(failed),
            Err(LlmError::RateLimited { .. })
        ));
        let mut decoder = ResponsesDecoder::new(ModelId::from("gpt-6-luna"));
        decoder
            .decode(r#"{"type":"response.created","response":{"model":"gpt-6-luna"}}"#)
            .unwrap();
        assert!(matches!(decoder.finish(), Err(LlmError::Network(_))));
    }

    #[test]
    fn requests_carry_instructions_history_and_tools() {
        let model = ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("gpt-6-luna", 8192).with_thinking(ThinkingConfig::Enabled {
                budget_tokens: 8_000,
            }),
            source: SpecSource::Default,
            warnings: vec![],
        };
        let mut req = LlmRequest::new(
            model,
            vec![
                Message::user("busca lib", vec![]),
                Message::assistant(vec![
                    ContentPart::Thinking {
                        thinking: "busco".into(),
                        signature: None,
                    },
                    ContentPart::text("Lo busco."),
                    ContentPart::ToolUse {
                        id: ToolUseId::from("call_x"),
                        name: "find_file".into(),
                        input: json!({ "query": "lib" }),
                    },
                ]),
                Message::new(
                    Role::User,
                    vec![ContentPart::tool_result_text(
                        ToolUseId::from("call_x"),
                        "src/lib.rs",
                    )],
                ),
            ],
        );
        req.system = vec![SystemBlock::cached("Eres crest.")];
        req.tools = vec![ToolDefinition::new(
            "find_file",
            "Finds files",
            json!({ "type": "object" }),
        )];
        req.tool_choice = ToolChoice::Tool("find_file".into());
        insta::assert_json_snapshot!(build(&req, &ModelId::from("gpt-6-luna")));
    }
}
