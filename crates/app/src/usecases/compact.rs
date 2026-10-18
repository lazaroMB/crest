//! Compaction: when the history gets close to the model's context window,
//! the oldest turns are summarized by the `compact` role and replaced by
//! one synthetic turn, so the conversation can go on.
//!
//! The synthetic turn is a user message starting with [`SUMMARY_PREFIX`]
//! followed by a short assistant acknowledgement, so roles keep
//! alternating with the turn after it. Turns are only cut at their
//! boundaries: a finished turn always answers every `tool_use` it contains,
//! so no `tool_use` is ever left without its `tool_result`. The last
//! [`CompactionPolicy::keep_recent_turns`] turns and the running turn are
//! kept intact.
//!
//! The summary turn carries the usage of the turns it replaces plus that
//! of the summary call, so the session total is preserved.

use domain::conversation::{
    ContentPart, Message, Role, Session, StopReason, ToolResultContent, Turn, Usage,
};
use domain::model::{ModelId, ResolvedModel};
use futures::StreamExt;

use crate::ports::llm::{LlmError, LlmPort, LlmRequest, MessageAssembler, SystemBlock};

/// Default context window of the models, in tokens.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;

/// Start of the user message that carries the summary.
pub const SUMMARY_PREFIX: &str = "Summary of the previous conversation:";

/// [`SUMMARY_PREFIX`] of sessions saved before crest spoke English, still
/// recognized so their summaries are not summarized again.
const LEGACY_SUMMARY_PREFIX: &str = "Resumen de la conversación anterior:";

/// Assistant answer to the summary, so roles keep alternating.
pub const SUMMARY_ACK: &str = "Understood, I will continue from this summary.";

/// Longest text kept from one content part in the transcript, in chars.
const MAX_PART_CHARS: usize = 2_000;

/// Longest transcript sent to the `compact` model, in chars (~100k
/// tokens); older text is dropped first.
const MAX_TRANSCRIPT_CHARS: usize = 400_000;

/// Instructions of the `compact` model.
const SUMMARY_INSTRUCTIONS: &str = "You compact the history of a coding \
assistant session. Write a concise summary that lets the assistant continue \
the work without the original messages. Include: the user's goals and \
requests, decisions taken and why, files read, created or modified (with \
paths), commands run and their relevant results, the current state of the \
task, and what is still pending. Keep exact names, paths and identifiers. \
Do not invent anything. Write in the language of the conversation, as plain \
Markdown with short bullet lists. Output only the summary.";

/// When and how much of the history to compact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionPolicy {
    /// Context window of the main model, in tokens.
    pub context_window_tokens: u64,
    /// Fraction of the window that triggers compaction (`0.0..=1.0`).
    pub threshold: f32,
    /// Finished turns kept verbatim after the summary.
    pub keep_recent_turns: usize,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            threshold: 0.8,
            keep_recent_turns: 2,
        }
    }
}

impl CompactionPolicy {
    /// Tokens above which the history is compacted.
    #[must_use]
    pub fn limit_tokens(&self) -> u64 {
        let threshold = f64::from(self.threshold.clamp(0.0, 1.0));
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let limit = (self.context_window_tokens as f64 * threshold) as u64;
        limit
    }

    /// Whether a history of `tokens` tokens must be compacted.
    #[must_use]
    pub fn exceeded(&self, tokens: u64) -> bool {
        tokens > self.limit_tokens()
    }
}

/// Tokens of context reported by one model call: everything it read.
#[must_use]
pub fn context_tokens(usage: &Usage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.cache_read_input_tokens)
        .saturating_add(usage.cache_creation_input_tokens)
}

/// Rough token count of `messages` (~4 bytes of JSON per token), for when
/// no usage has been reported yet.
pub fn estimate_tokens<'a>(messages: impl IntoIterator<Item = &'a Message>) -> u64 {
    let bytes: usize = messages
        .into_iter()
        .map(|m| serde_json::to_string(&m.content).map_or(0, |s| s.len()))
        .sum();
    u64::try_from(bytes.div_ceil(4)).unwrap_or(u64::MAX)
}

/// Whether `turn` is a summary made by a previous compaction.
#[must_use]
pub fn is_summary_turn(turn: &Turn) -> bool {
    turn.messages.first().is_some_and(|m| {
        m.role == Role::User
            && matches!(
                m.content.first(),
                Some(ContentPart::Text { text })
                    if text.starts_with(SUMMARY_PREFIX) || text.starts_with(LEGACY_SUMMARY_PREFIX)
            )
    })
}

/// Number of leading turns of `session` to summarize, keeping the last
/// `keep_recent` ones; `None` when there is nothing worth compacting.
#[must_use]
pub fn compactable_turns(session: &Session, keep_recent: usize) -> Option<usize> {
    let count = session.turns.len().saturating_sub(keep_recent);
    match count {
        0 => None,
        1 if is_summary_turn(&session.turns[0]) => None,
        _ => Some(count),
    }
}

/// `turns` as plain text for the `compact` model. Thinking is dropped and
/// long parts are cut; when the whole text is too long its start is cut.
#[must_use]
pub fn transcript(turns: &[Turn]) -> String {
    let mut out = String::new();
    for message in turns.iter().flat_map(|t| &t.messages) {
        let who = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        for part in &message.content {
            let line = match part {
                ContentPart::Text { text } => format!("[{who}] {}", cut(text)),
                ContentPart::ToolUse { name, input, .. } => {
                    format!("[{who} → tool {name}] {}", cut(&input.to_string()))
                }
                ContentPart::ToolResult {
                    content, is_error, ..
                } => {
                    let label = if *is_error {
                        "tool error"
                    } else {
                        "tool result"
                    };
                    format!("[{label}] {}", cut(&result_text(content)))
                }
                ContentPart::Image { .. } => format!("[{who}] <image>"),
                ContentPart::ProviderBlock { raw } => {
                    let kind = raw.get("type").and_then(|t| t.as_str()).unwrap_or("block");
                    format!("[{who}] <{kind}>")
                }
                ContentPart::Thinking { .. } | ContentPart::RedactedThinking { .. } => continue,
            };
            out.push_str(&line);
            out.push_str("\n\n");
        }
    }
    let chars = out.chars().count();
    if chars > MAX_TRANSCRIPT_CHARS {
        let tail: String = out.chars().skip(chars - MAX_TRANSCRIPT_CHARS).collect();
        out = format!("[…]\n{tail}");
    }
    out
}

fn result_text(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .map(|c| match c {
            ToolResultContent::Text { text } => text.as_str(),
            ToolResultContent::Image { .. } => "<image>",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` cut to [`MAX_PART_CHARS`].
fn cut(text: &str) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(MAX_PART_CHARS).collect();
    if chars.next().is_some() {
        format!("{kept} […]")
    } else {
        kept
    }
}

/// The request that asks `model` (the `compact` role) to summarize
/// `transcript`.
#[must_use]
pub fn summary_request(model: ResolvedModel, transcript: &str) -> LlmRequest {
    let prompt =
        format!("<conversation>\n{transcript}</conversation>\n\nSummarize the conversation above.");
    let mut req = LlmRequest::new(model, vec![Message::user_text(prompt)]);
    req.system = vec![SystemBlock::new(SUMMARY_INSTRUCTIONS)];
    req
}

/// The answer of the `compact` model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The summary text.
    pub text: String,
    /// Tokens of the call.
    pub usage: Usage,
    /// Model that answered.
    pub model: Option<ModelId>,
}

/// Runs `req` and returns the text it answers.
///
/// # Errors
///
/// The model's [`LlmError`], or [`LlmError::Decode`] when it answers no
/// text.
pub async fn summarize(llm: &dyn LlmPort, req: LlmRequest) -> Result<Summary, LlmError> {
    let mut stream = llm.stream(req).await?;
    let mut assembler = MessageAssembler::new();
    while let Some(event) = stream.next().await {
        assembler.push(event?)?;
    }
    let assembled = assembler.finish()?;
    let text = assembled
        .message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_owned();
    if text.is_empty() {
        return Err(LlmError::Decode(
            "the compact model returned an empty summary".to_owned(),
        ));
    }
    Ok(Summary {
        text,
        usage: assembled.usage,
        model: assembled.model,
    })
}

/// The synthetic turn that replaces `replaced`.
#[must_use]
pub fn summary_turn(summary: &str, replaced: &[Turn], usage: Usage) -> Turn {
    let mut turn = Turn::new(Message::user_text(format!("{SUMMARY_PREFIX}\n\n{summary}")));
    turn.messages
        .push(Message::assistant(vec![ContentPart::text(SUMMARY_ACK)]));
    turn.usage = replaced.iter().map(|t| t.usage).sum::<Usage>() + usage;
    turn.stop_reason = Some(StopReason::EndTurn);
    turn
}

/// Replaces the first `count` turns of `session` with `summary`.
pub fn replace_turns(session: &mut Session, count: usize, summary: Turn) {
    let count = count.min(session.turns.len());
    session.turns.splice(..count, std::iter::once(summary));
}

#[cfg(test)]
mod tests {
    use domain::conversation::ToolUseId;
    use domain::model::{ModelPolicy, ModelRole, ResolveRequest};
    use serde_json::json;

    use super::*;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};

    fn text_turn(question: &str, answer: &str, input_tokens: u64) -> Turn {
        let mut turn = Turn::new(Message::user_text(question));
        turn.messages
            .push(Message::assistant(vec![ContentPart::text(answer)]));
        turn.usage.input_tokens = input_tokens;
        turn.stop_reason = Some(StopReason::EndTurn);
        turn
    }

    fn tool_turn() -> Turn {
        let mut turn = Turn::new(Message::user_text("lee main"));
        turn.messages.push(Message::assistant(vec![
            ContentPart::Thinking {
                thinking: "secreto".into(),
                signature: None,
            },
            ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "read".into(),
                input: json!({ "path": "src/main.rs" }),
            },
        ]));
        turn.messages.push(Message::new(
            Role::User,
            vec![ContentPart::ToolResult {
                tool_use_id: ToolUseId::new("toolu_1"),
                content: vec![ToolResultContent::text("x".repeat(5_000))],
                is_error: false,
            }],
        ));
        turn.messages
            .push(Message::assistant(vec![ContentPart::text("hecho")]));
        turn
    }

    fn session(turns: Vec<Turn>) -> Session {
        let mut session = Session::new("/ws", 0);
        for turn in turns {
            session.push_turn(turn, 1);
        }
        session
    }

    #[test]
    fn limit_is_a_fraction_of_the_window() {
        let policy = CompactionPolicy {
            context_window_tokens: 1_000,
            threshold: 0.8,
            keep_recent_turns: 1,
        };
        assert_eq!(policy.limit_tokens(), 800);
        assert!(!policy.exceeded(800));
        assert!(policy.exceeded(801));
        assert_eq!(CompactionPolicy::default().limit_tokens(), 160_000);
    }

    #[test]
    fn context_counts_every_input_token() {
        let usage = Usage {
            input_tokens: 1,
            output_tokens: 100,
            cache_read_input_tokens: 10,
            cache_creation_input_tokens: 1_000,
        };
        assert_eq!(context_tokens(&usage), 1_011);
    }

    #[test]
    fn estimate_is_about_four_bytes_per_token() {
        let message = Message::user_text("a".repeat(400));
        let tokens = estimate_tokens([&message]);
        assert!((100..120).contains(&tokens), "{tokens}");
    }

    #[test]
    fn keeps_the_recent_turns_and_skips_a_lone_summary() {
        let s = session(vec![text_turn("a", "b", 0); 3]);
        assert_eq!(compactable_turns(&s, 2), Some(1));
        assert_eq!(compactable_turns(&s, 3), None);
        assert_eq!(compactable_turns(&s, 0), Some(3));

        let summary = summary_turn("todo", &[], Usage::default());
        assert!(is_summary_turn(&summary));
        let s = session(vec![summary, text_turn("a", "b", 0)]);
        assert_eq!(compactable_turns(&s, 1), None);
        assert_eq!(compactable_turns(&s, 0), Some(2));
    }

    #[test]
    fn transcript_drops_thinking_and_cuts_long_parts() {
        let text = transcript(&[tool_turn()]);
        assert!(text.contains("[user] lee main"));
        assert!(text.contains("[assistant → tool read] {\"path\":\"src/main.rs\"}"));
        assert!(text.contains("[tool result] xxx"));
        assert!(text.contains("[…]"));
        assert!(text.contains("[assistant] hecho"));
        assert!(!text.contains("secreto"));
        assert!(text.len() < 3_000);
    }

    #[test]
    fn summary_turn_keeps_usage_and_alternates_roles() {
        let replaced = [text_turn("a", "b", 10), text_turn("c", "d", 20)];
        let usage = Usage {
            input_tokens: 5,
            output_tokens: 7,
            ..Usage::default()
        };
        let turn = summary_turn("hicimos X", &replaced, usage);
        assert_eq!(turn.messages.len(), 2);
        assert_eq!(turn.messages[0].role, Role::User);
        assert_eq!(turn.messages[1].role, Role::Assistant);
        assert_eq!(
            turn.messages[0].content,
            vec![ContentPart::text(format!("{SUMMARY_PREFIX}\n\nhicimos X"))]
        );
        assert_eq!(turn.usage.input_tokens, 35);
        assert_eq!(turn.usage.output_tokens, 7);

        let mut s = session(replaced.to_vec());
        let recent = text_turn("e", "f", 1);
        s.push_turn(recent.clone(), 2);
        let total = s.total_usage() + usage;
        replace_turns(&mut s, 2, turn.clone());
        assert_eq!(s.turns, vec![turn, recent]);
        assert_eq!(s.total_usage(), total);
    }

    #[tokio::test]
    async fn summarize_asks_the_compact_model() {
        let llm = ScriptedLlm::new([ScriptedResponse::text("  hicimos X  ")]);
        let model = ModelPolicy::default().resolve(&ResolveRequest::new(ModelRole::Compact));
        let req = summary_request(model, &transcript(&[tool_turn()]));
        let summary = summarize(&llm, req).await.unwrap();
        assert_eq!(summary.text, "hicimos X");
        assert!(summary.usage.input_tokens > 0);

        let sent = llm.last_request().unwrap();
        assert_eq!(sent.model.role, ModelRole::Compact);
        assert!(sent.tools.is_empty());
        assert_eq!(sent.messages.len(), 1);
        assert_eq!(sent.messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn an_empty_summary_is_an_error() {
        let llm = ScriptedLlm::new([ScriptedResponse::text(" ")]);
        let model = ModelPolicy::default().resolve(&ResolveRequest::new(ModelRole::Compact));
        let err = summarize(&llm, summary_request(model, "x")).await;
        assert!(matches!(err, Err(LlmError::Decode(_))));
    }
}
