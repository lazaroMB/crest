//! OpenAI Chat Completions (`POST /chat/completions`, streaming): the
//! request body built from an [`LlmRequest`] and the [`ChatDecoder`] of
//! its answer.
//!
//! Request mapping:
//!
//! - The system blocks become one `system` message; cache breakpoints and
//!   server tools are not sent (the API has neither).
//! - Tool results of a user message become `tool` messages, sent before the
//!   rest of that message; images go as `image_url` data URLs.
//! - Assistant text and tool calls become one assistant message; its
//!   thinking goes back as `reasoning_content` (reasoning models such as
//!   DeepSeek and Kimi want it during a tool loop; others ignore it).
//!   Redacted thinking and provider blocks of other providers are dropped.
//!
//! Stream mapping: each `choices[0].delta` adds to the open block of its
//! kind (`reasoning_content`/`reasoning` → thinking, `content` → text, each
//! `tool_calls[i]` → a tool use); a new kind closes the previous text or
//! thinking block. `finish_reason` and the final `usage` chunk become
//! [`LlmEvent::Usage`] and [`LlmEvent::MessageStop`] at `[DONE]` or at the
//! end of the stream.

use std::collections::BTreeMap;

use application::ports::llm::{LlmError, LlmEvent, LlmRequest, ToolChoice};
use domain::conversation::{ContentPart, Role, StopReason, ToolResultContent, ToolUseId, Usage};
use domain::model::ModelId;
use serde_json::{Map, Value, json};

use crate::client::from_stream_error;
use crate::sse::{Decoder, json as parse};

/// The body of a streaming Chat Completions request for `model`.
#[must_use]
pub fn build(req: &LlmRequest, model: &ModelId) -> Value {
    let mut messages = Vec::new();
    let system = system_text(req);
    if !system.is_empty() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &req.messages {
        match message.role {
            Role::User => user_messages(&message.content, &mut messages),
            Role::Assistant => messages.extend(assistant_message(&message.content)),
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(model.as_str()));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("max_tokens".into(), json!(req.effective_max_tokens()));
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({ "include_usage": true }));
    if !req.tools.is_empty() && req.tool_choice != ToolChoice::None {
        let tools = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        let choice = match &req.tool_choice {
            ToolChoice::Auto | ToolChoice::None => json!("auto"),
            ToolChoice::Any => json!("required"),
            ToolChoice::Tool(name) => json!({ "type": "function", "function": { "name": name } }),
        };
        body.insert("tool_choice".into(), choice);
    }
    Value::Object(body)
}

/// The system blocks joined by blank lines.
#[must_use]
pub fn system_text(req: &LlmRequest) -> String {
    req.system
        .iter()
        .map(|block| block.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A base64 image as a data URL.
#[must_use]
pub fn data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

/// The text of a tool result; images become a note, since tool messages
/// only take text.
#[must_use]
pub fn tool_result_text(content: &[ToolResultContent], is_error: bool) -> String {
    let text = content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text { text } => text.clone(),
            ToolResultContent::Image { media_type, .. } => format!("[image {media_type} omitted]"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    if is_error {
        format!("Error: {text}")
    } else {
        text
    }
}

fn user_messages(content: &[ContentPart], out: &mut Vec<Value>) {
    let mut parts = Vec::new();
    for part in content {
        match part {
            ContentPart::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => out.push(json!({
                "role": "tool",
                "tool_call_id": tool_use_id.to_string(),
                "content": tool_result_text(content, *is_error),
            })),
            ContentPart::Text { text } => parts.push(json!({ "type": "text", "text": text })),
            ContentPart::Image { media_type, data } => parts.push(json!({
                "type": "image_url",
                "image_url": { "url": data_url(media_type, data) },
            })),
            _ => {}
        }
    }
    if parts.is_empty() {
        return;
    }
    let content = match parts.as_slice() {
        [single] if single["type"] == "text" => single["text"].clone(),
        _ => Value::Array(parts),
    };
    out.push(json!({ "role": "user", "content": content }));
}

fn assistant_message(content: &[ContentPart]) -> Option<Value> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for part in content {
        match part {
            ContentPart::Text { text: t } => text.push_str(t),
            ContentPart::Thinking { thinking, .. } => reasoning.push_str(thinking),
            ContentPart::ToolUse { id, name, input } => calls.push(json!({
                "id": id.to_string(),
                "type": "function",
                "function": { "name": name, "arguments": input.to_string() },
            })),
            _ => {}
        }
    }
    if text.is_empty() && calls.is_empty() {
        return None;
    }
    let mut message = Map::new();
    message.insert("role".into(), json!("assistant"));
    message.insert(
        "content".into(),
        if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        },
    );
    if !reasoning.is_empty() {
        message.insert("reasoning_content".into(), json!(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(calls));
    }
    Some(Value::Object(message))
}

/// Kind of the text or thinking block that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plain {
    Text,
    Thinking,
}

/// Decodes the chunks of a Chat Completions stream.
#[derive(Debug)]
pub struct ChatDecoder {
    requested: ModelId,
    started: bool,
    next_block: usize,
    /// The open text or thinking block and its index.
    plain: Option<(Plain, usize)>,
    /// Block index of each tool call, by its index in `tool_calls`.
    tools: BTreeMap<u64, usize>,
    finish: Option<String>,
    usage: Option<Usage>,
    stopped: bool,
}

impl ChatDecoder {
    /// A decoder for an answer of `requested`.
    #[must_use]
    pub fn new(requested: ModelId) -> Self {
        Self {
            requested,
            started: false,
            next_block: 0,
            plain: None,
            tools: BTreeMap::new(),
            finish: None,
            usage: None,
            stopped: false,
        }
    }

    fn start(&mut self, chunk: &Value, out: &mut Vec<LlmEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        let model = chunk
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map_or_else(|| self.requested.clone(), ModelId::new);
        out.push(LlmEvent::MessageStart { model });
    }

    fn close_plain(&mut self, out: &mut Vec<LlmEvent>) {
        if let Some((_, index)) = self.plain.take() {
            out.push(LlmEvent::BlockStop { index });
        }
    }

    fn plain_delta(&mut self, kind: Plain, delta: &str, out: &mut Vec<LlmEvent>) {
        if delta.is_empty() {
            return;
        }
        let index = match self.plain {
            Some((open, index)) if open == kind => index,
            _ => {
                self.close_plain(out);
                let index = self.next_block;
                self.next_block += 1;
                self.plain = Some((kind, index));
                index
            }
        };
        out.push(match kind {
            Plain::Text => LlmEvent::TextDelta {
                index,
                text: delta.to_owned(),
            },
            Plain::Thinking => LlmEvent::ThinkingDelta {
                index,
                thinking: delta.to_owned(),
            },
        });
    }

    fn tool_delta(&mut self, call: &Value, out: &mut Vec<LlmEvent>) {
        let position = call.get("index").and_then(Value::as_u64).unwrap_or(0);
        let function = call.get("function");
        let index = if let Some(index) = self.tools.get(&position) {
            *index
        } else {
            self.close_plain(out);
            let index = self.next_block;
            self.next_block += 1;
            self.tools.insert(position, index);
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map_or_else(|| format!("call_{index}"), str::to_owned);
            let name = function
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            out.push(LlmEvent::ToolUseStart {
                index,
                id: ToolUseId::from(id.as_str()),
                name,
            });
            index
        };
        if let Some(arguments) = function
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .filter(|a| !a.is_empty())
        {
            out.push(LlmEvent::ToolInputDelta {
                index,
                partial_json: arguments.to_owned(),
            });
        }
    }

    fn stop(&mut self) -> Vec<LlmEvent> {
        let mut out = Vec::new();
        self.close_plain(&mut out);
        // Some servers say `stop` after tool calls; the calls still wait for
        // their results.
        let called = !self.tools.is_empty();
        for index in std::mem::take(&mut self.tools).into_values() {
            out.push(LlmEvent::BlockStop { index });
        }
        if let Some(usage) = self.usage.take() {
            out.push(LlmEvent::Usage(usage));
        }
        let stop_reason = match self.finish.as_deref() {
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal,
            Some("tool_calls" | "function_call") => StopReason::ToolUse,
            _ if called => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        out.push(LlmEvent::MessageStop { stop_reason });
        self.stopped = true;
        out
    }
}

fn usage(value: &Value) -> Usage {
    let count = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0);
    let prompt = count(value.get("prompt_tokens"));
    let cached = count(
        value
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens")),
    )
    .max(count(value.get("prompt_cache_hit_tokens")));
    Usage {
        input_tokens: prompt.saturating_sub(cached),
        output_tokens: count(value.get("completion_tokens")),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: 0,
    }
}

impl Decoder for ChatDecoder {
    fn decode(&mut self, data: &str) -> Result<Vec<LlmEvent>, LlmError> {
        if data.trim() == "[DONE]" {
            return Ok(if self.started {
                self.stop()
            } else {
                Vec::new()
            });
        }
        let chunk = parse(data)?;
        if let Some(error) = chunk.get("error") {
            return Err(from_stream_error(error));
        }
        let mut out = Vec::new();
        self.start(&chunk, &mut out);
        if let Some(value) = chunk.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(usage(value));
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(out);
        };
        if let Some(delta) = choice.get("delta") {
            let reasoning = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str);
            if let Some(reasoning) = reasoning {
                self.plain_delta(Plain::Thinking, reasoning, &mut out);
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                self.plain_delta(Plain::Text, text, &mut out);
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    self.tool_delta(call, &mut out);
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(reason.to_owned());
        }
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<LlmEvent>, LlmError> {
        if self.stopped {
            return Ok(Vec::new());
        }
        // Some servers close the stream without `[DONE]` after the finish
        // reason; without one the answer was cut.
        if self.finish.is_some() {
            return Ok(self.stop());
        }
        Err(LlmError::Network(
            "the stream ended before the answer finished".into(),
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

    const TEXT: &str = include_str!("../tests/fixtures/chat_text.sse");
    const TOOLS: &str = include_str!("../tests/fixtures/chat_tool_calls.sse");

    fn decode_all(fixture: &str) -> Vec<LlmEvent> {
        let mut decoder = ChatDecoder::new(ModelId::from("glm-5.3"));
        let mut events = Vec::new();
        for line in fixture.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                events.extend(decoder.decode(data).unwrap());
            }
        }
        events.extend(decoder.finish().unwrap());
        events
    }

    fn assemble(events: Vec<LlmEvent>) -> application::ports::llm::AssembledMessage {
        let mut assembler = MessageAssembler::new();
        for event in events {
            assembler.push(event).unwrap();
        }
        assembler.finish().unwrap()
    }

    #[test]
    fn text_and_reasoning_stream_into_blocks() {
        let events = decode_all(TEXT);
        insta::assert_debug_snapshot!(events);
        let message = assemble(events);
        assert_eq!(message.stop_reason, StopReason::EndTurn);
        assert_eq!(message.usage.output_tokens, 12);
        assert_eq!(message.usage.cache_read_input_tokens, 4);
        assert_eq!(message.usage.input_tokens, 16);
        assert!(matches!(
            message.message.content.as_slice(),
            [ContentPart::Thinking { signature: None, .. }, ContentPart::Text { text }]
                if text == "Hola, ¿qué tal?"
        ));
    }

    #[test]
    fn tool_calls_stream_into_tool_uses() {
        let message = assemble(decode_all(TOOLS));
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        let [
            ContentPart::Text { .. },
            ContentPart::ToolUse { id, name, input },
            ContentPart::ToolUse {
                name: second,
                input: none,
                ..
            },
        ] = message.message.content.as_slice()
        else {
            panic!("{:?}", message.message.content)
        };
        assert_eq!(id.to_string(), "call_a");
        assert_eq!(name, "find_file");
        assert_eq!(input, &json!({ "query": "lib" }));
        assert_eq!(second, "git_status");
        assert_eq!(none, &json!({}));
    }

    #[test]
    fn a_stream_cut_before_the_finish_is_an_error() {
        let mut decoder = ChatDecoder::new(ModelId::from("glm-5.3"));
        decoder
            .decode(r#"{"model":"glm-5.3","choices":[{"delta":{"content":"Ho"}}]}"#)
            .unwrap();
        assert!(matches!(decoder.finish(), Err(LlmError::Network(_))));
        let mut decoder = ChatDecoder::new(ModelId::from("glm-5.3"));
        assert!(matches!(
            decoder.decode(r#"{"error":{"message":"maximum context length exceeded"}}"#),
            Err(LlmError::ContextTooLong)
        ));
    }

    #[test]
    fn requests_carry_history_tools_and_images() {
        let model = ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("glm-5.3", 4096),
            source: SpecSource::Default,
            warnings: vec![],
        };
        let mut req = LlmRequest::new(
            model,
            vec![
                Message::user("mira esto", vec![]),
                Message::assistant(vec![
                    ContentPart::Thinking {
                        thinking: "busco".into(),
                        signature: Some("sig".into()),
                    },
                    ContentPart::text("Busco."),
                    ContentPart::ToolUse {
                        id: ToolUseId::from("call_1"),
                        name: "find_file".into(),
                        input: json!({ "query": "lib" }),
                    },
                    ContentPart::ProviderBlock {
                        raw: json!({ "type": "server_tool_use" }),
                    },
                ]),
                Message::new(
                    Role::User,
                    vec![
                        ContentPart::tool_result_text(ToolUseId::from("call_1"), "src/lib.rs"),
                        ContentPart::Image {
                            media_type: "image/png".into(),
                            data: "AAAA".into(),
                        },
                    ],
                ),
            ],
        );
        req.system = vec![SystemBlock::cached("Eres crest."), SystemBlock::new("Hoy.")];
        req.tools = vec![
            ToolDefinition::new("find_file", "Finds files", json!({ "type": "object" })).cached(),
        ];
        insta::assert_json_snapshot!(build(&req, &ModelId::from("glm-5.3")));
    }
}
