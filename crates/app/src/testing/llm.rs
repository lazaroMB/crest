//! Fakes of the model ports: [`LlmPort`](crate::ports::llm::LlmPort) driven
//! by a script of answers.
//!
//! A [`ScriptedResponse`] describes one model answer at a high level
//! (text, thinking, tool uses) and expands into the exact event sequence a
//! real adapter would stream: `MessageStart`, an initial `Usage`, each block
//! split into several deltas and closed with `BlockStop`, a final `Usage`
//! and `MessageStop`. Consumers therefore exercise
//! [`MessageAssembler`](crate::ports::llm::MessageAssembler) for real.

use domain::conversation::{StopReason, ToolUseId, Usage};
use domain::model::ModelId;

use crate::ports::llm::{LlmEvent, LlmRequest};

/// Signature attached to scripted thinking blocks.
pub const FAKE_SIGNATURE: &str = "fake-signature";

/// One content block of a scripted answer.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptBlock {
    /// A text block.
    Text(String),
    /// An extended thinking block with its signature.
    Thinking {
        /// Thinking text.
        thinking: String,
        /// Signature sent after the thinking deltas.
        signature: String,
    },
    /// A client-side tool call.
    ToolUse {
        /// Provider id of the call.
        id: ToolUseId,
        /// Tool name.
        name: String,
        /// Tool input.
        input: serde_json::Value,
    },
}

/// One scripted model answer, built with the constructors below and
/// combined with the `then_*` methods.
///
/// ```ignore
/// # use application::testing::llm::ScriptedResponse;
/// let answer = ScriptedResponse::text("Busco la función.")
///     .then_tool_use("toolu_1", "grep", serde_json::json!({ "pattern": "fn main" }));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptedResponse {
    blocks: Vec<ScriptBlock>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    model: Option<ModelId>,
}

impl ScriptedResponse {
    /// An answer with no blocks; add them with the `then_*` methods.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            blocks: Vec::new(),
            stop_reason: None,
            usage: None,
            model: None,
        }
    }

    /// An answer made of one text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::empty().then_text(text)
    }

    /// An answer made of one tool use.
    #[must_use]
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::empty().then_tool_use(id, name, input)
    }

    /// An answer starting with a thinking block.
    #[must_use]
    pub fn thinking(thinking: impl Into<String>) -> Self {
        Self::empty().then_thinking(thinking)
    }

    /// Appends a text block.
    #[must_use]
    pub fn then_text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(ScriptBlock::Text(text.into()));
        self
    }

    /// Appends a tool use.
    #[must_use]
    pub fn then_tool_use(
        mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        self.blocks.push(ScriptBlock::ToolUse {
            id: ToolUseId::new(id),
            name: name.into(),
            input,
        });
        self
    }

    /// Appends a thinking block signed with [`FAKE_SIGNATURE`].
    #[must_use]
    pub fn then_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.blocks.push(ScriptBlock::Thinking {
            thinking: thinking.into(),
            signature: FAKE_SIGNATURE.to_owned(),
        });
        self
    }

    /// Overrides the stop reason (by default `ToolUse` if the answer has a
    /// tool use and `EndTurn` otherwise).
    #[must_use]
    pub fn with_stop_reason(mut self, stop_reason: StopReason) -> Self {
        self.stop_reason = Some(stop_reason);
        self
    }

    /// Overrides the final usage (by default estimated from the sizes of
    /// the request and of the answer, at about 4 bytes per token).
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Overrides the model reported in `MessageStart` (by default the
    /// primary model of the request).
    #[must_use]
    pub fn with_model(mut self, model: impl Into<ModelId>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// The content blocks of the answer.
    #[must_use]
    pub fn blocks(&self) -> &[ScriptBlock] {
        &self.blocks
    }

    /// The stop reason the answer ends with.
    #[must_use]
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason.unwrap_or_else(|| {
            if self
                .blocks
                .iter()
                .any(|block| matches!(block, ScriptBlock::ToolUse { .. }))
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        })
    }

    /// The events a real adapter would stream for this answer to `req`.
    #[must_use]
    pub fn events_for(&self, req: &LlmRequest) -> Vec<LlmEvent> {
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| req.model.spec.id.clone());
        let usage = self.usage.unwrap_or_else(|| Usage {
            input_tokens: estimate_input_tokens(req),
            output_tokens: self.estimate_output_tokens(),
            ..Usage::default()
        });
        let mut events = vec![
            LlmEvent::MessageStart { model },
            LlmEvent::Usage(Usage {
                output_tokens: 1,
                ..usage
            }),
        ];
        for (index, block) in self.blocks.iter().enumerate() {
            push_block_events(&mut events, index, block);
        }
        events.push(LlmEvent::Usage(usage));
        events.push(LlmEvent::MessageStop {
            stop_reason: self.stop_reason(),
        });
        events
    }

    fn estimate_output_tokens(&self) -> u64 {
        let bytes: usize = self
            .blocks
            .iter()
            .map(|block| match block {
                ScriptBlock::Text(text) => text.len(),
                ScriptBlock::Thinking { thinking, .. } => thinking.len(),
                ScriptBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            })
            .sum();
        tokens(bytes)
    }
}

/// Appends the events of one block: its start, its deltas and `BlockStop`.
fn push_block_events(events: &mut Vec<LlmEvent>, index: usize, block: &ScriptBlock) {
    match block {
        ScriptBlock::Text(text) => {
            events.extend(
                split(text, 3)
                    .into_iter()
                    .map(|text| LlmEvent::TextDelta { index, text }),
            );
        }
        ScriptBlock::Thinking {
            thinking,
            signature,
        } => {
            events.extend(
                split(thinking, 3)
                    .into_iter()
                    .map(|thinking| LlmEvent::ThinkingDelta { index, thinking }),
            );
            events.push(LlmEvent::SignatureDelta {
                index,
                signature: signature.clone(),
            });
        }
        ScriptBlock::ToolUse { id, name, input } => {
            events.push(LlmEvent::ToolUseStart {
                index,
                id: id.clone(),
                name: name.clone(),
            });
            events.extend(
                split(&input.to_string(), 3)
                    .into_iter()
                    .map(|partial_json| LlmEvent::ToolInputDelta {
                        index,
                        partial_json,
                    }),
            );
        }
    }
    events.push(LlmEvent::BlockStop { index });
}

/// Splits `text` into at most `parts` non-empty chunks on char boundaries;
/// an empty text gives one empty chunk, so the block still starts.
fn split(text: &str, parts: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    let size = chars.len().div_ceil(parts.max(1));
    chars.chunks(size).map(|c| c.iter().collect()).collect()
}

/// Rough token count of `bytes` bytes of text.
fn tokens(bytes: usize) -> u64 {
    u64::try_from(bytes.div_ceil(4)).unwrap_or(u64::MAX).max(1)
}

/// Rough input token count of a request: system prompt, history and tools.
fn estimate_input_tokens(req: &LlmRequest) -> u64 {
    let system: usize = req.system.iter().map(|block| block.text.len()).sum();
    let messages = serde_json::to_string(&req.messages).map_or(0, |s| s.len());
    let tools = serde_json::to_string(&req.tools).map_or(0, |s| s.len());
    tokens(system + messages + tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::llm::{AssembledMessage, MessageAssembler};
    use domain::conversation::{ContentPart, Message, Role};
    use domain::model::{ModelRole, ModelSpec, ResolvedModel, SpecSource};
    use serde_json::json;

    fn request(text: &str) -> LlmRequest {
        let model = ResolvedModel {
            role: ModelRole::Main,
            spec: ModelSpec::new("claude-sonnet-5-5", 16_000),
            source: SpecSource::Default,
            warnings: Vec::new(),
        };
        LlmRequest::new(model, vec![Message::user_text(text)])
    }

    fn assemble(events: Vec<LlmEvent>) -> AssembledMessage {
        let mut asm = MessageAssembler::new();
        for event in events {
            asm.push(event).unwrap();
        }
        asm.finish().unwrap()
    }

    #[test]
    fn split_keeps_chars_and_limits_parts() {
        assert_eq!(split("", 3), vec![String::new()]);
        assert_eq!(split("ab", 3), vec!["a", "b"]);
        assert_eq!(split("ñandú!", 3), vec!["ña", "nd", "ú!"]);
    }

    #[test]
    fn stop_reason_follows_the_blocks() {
        assert_eq!(
            ScriptedResponse::text("hola").stop_reason(),
            StopReason::EndTurn
        );
        let tool = ScriptedResponse::text("busco").then_tool_use("toolu_1", "grep", json!({}));
        assert_eq!(tool.stop_reason(), StopReason::ToolUse);
        let capped = ScriptedResponse::text("…").with_stop_reason(StopReason::MaxTokens);
        assert_eq!(capped.stop_reason(), StopReason::MaxTokens);
    }

    #[test]
    fn tool_use_input_is_streamed_in_several_deltas() {
        let input = json!({ "pattern": "fn main", "path": "src" });
        let events = ScriptedResponse::tool_use("toolu_1", "grep", input.clone())
            .events_for(&request("busca main"));
        let deltas = events
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolInputDelta { .. }))
            .count();
        assert_eq!(deltas, 3);
        assert!(matches!(
            events.first(),
            Some(LlmEvent::MessageStart { model }) if model.as_str() == "claude-sonnet-5-5"
        ));
        assert!(matches!(
            events.last(),
            Some(LlmEvent::MessageStop {
                stop_reason: StopReason::ToolUse
            })
        ));
        let out = assemble(events);
        assert_eq!(
            out.message.content,
            vec![ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "grep".into(),
                input,
            }]
        );
    }

    #[test]
    fn thinking_text_and_usage_assemble() {
        let out = assemble(
            ScriptedResponse::thinking("pienso un poco")
                .then_text("Hola, ¿en qué te ayudo?")
                .with_model("claude-haiku-5-5")
                .events_for(&request("hola")),
        );
        assert_eq!(out.message.role, Role::Assistant);
        assert_eq!(
            out.message.content,
            vec![
                ContentPart::Thinking {
                    thinking: "pienso un poco".into(),
                    signature: Some(FAKE_SIGNATURE.into()),
                },
                ContentPart::text("Hola, ¿en qué te ayudo?"),
            ]
        );
        assert_eq!(out.model, Some(ModelId::new("claude-haiku-5-5")));
        assert!(out.usage.input_tokens > 0 && out.usage.output_tokens > 1);
        assert_eq!(out.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn usage_override_is_the_final_usage() {
        let usage = Usage {
            input_tokens: 120,
            output_tokens: 7,
            cache_read_input_tokens: 100,
            cache_creation_input_tokens: 0,
        };
        let out = assemble(
            ScriptedResponse::text("ok")
                .with_usage(usage)
                .events_for(&request("x")),
        );
        assert_eq!(out.usage, usage);
    }
}
