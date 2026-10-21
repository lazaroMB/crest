//! Side questions while a flow waits at a gate.
//!
//! A flow at a gate takes the plain lines of the chat as its answer. When
//! the user needs to ask something else first (or look something up to
//! answer), the line goes to the normal chat instead, with the gate as
//! context ([`preamble`]), and the flow keeps waiting. This module holds
//! the pieces both flows share:
//!
//! - [`GateInfo`]: what the waiting gate asks.
//! - [`classify`]: whether a line answers the gate or is a side question
//!   (only with the decision model; without it every line answers, as
//!   before).
//! - [`wants_return`]: whether a line of the side chat says to go back to
//!   the flow ("listo, continúa").
//! - [`proposal_context`] and [`last_answer`]: the chat turn that turns the
//!   side conversation into a proposed answer, and reading it back.

use std::fmt;

use domain::conversation::{ContentPart, Role, Turn};
use domain::decision::{DecisionRequest, Question};

use super::research::gate::{is_approval, normalize};
use crate::decide::Decider;

/// Confidence from which the decision model's reading of a line is taken.
/// Higher than the gates' approval threshold: sending a real answer to the
/// chat is worse than answering with a question.
pub const SIDE_CONFIDENCE: f32 = 0.8;

/// Which flow waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    /// `/research`.
    Research,
    /// `/implement`.
    Implement,
}

impl FlowKind {
    /// Name shown to the user.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Implement => "implementation",
        }
    }
}

/// A gate waiting for the user, as the side chat sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateInfo {
    /// Which flow waits.
    pub flow: FlowKind,
    /// Stage (or step) that waits, as shown to the user.
    pub stage: String,
    /// What the gate asks.
    pub prompt: String,
    /// What the flow is about: the problem or the plan.
    pub topic: Option<String>,
}

impl fmt::Display for GateInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} flow waits at {}", self.flow.name(), self.stage)
    }
}

/// What a line typed at a gate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineIntent {
    /// It answers, approves or corrects what the gate asks.
    Answer,
    /// It asks something else, or the user wants to look into something
    /// before answering.
    SideQuestion,
}

/// Whether `text`, typed while `gate` waits, answers it or is a side
/// question. A short "yes" or a command always answers; without a decision
/// model (or when it fails, or is not sure) the line answers too.
pub async fn classify(text: &str, gate: &GateInfo, decider: &Decider) -> LineIntent {
    if is_approval(text) || text.trim_start().starts_with('/') || !decider.is_available() {
        return LineIntent::Answer;
    }
    let state = format!(
        "The {} flow asked the user: {}\n\nThe user wrote: {text}",
        gate.flow.name(),
        gate.prompt
    );
    let request = DecisionRequest::new(state).ask(
        "line",
        Question::choice(
            "Is the user's line an answer to what the flow asked?",
            [
                (
                    "answer",
                    "answers, approves, rejects or corrects what the flow asked",
                ),
                (
                    "side",
                    "asks something else, or wants to look something up before answering",
                ),
            ],
        ),
    );
    let side = decider.decide(&request).await.is_some_and(|r| {
        r.choice("line")
            .is_some_and(|(c, p)| c == "side" && p >= SIDE_CONFIDENCE)
    });
    if side {
        LineIntent::SideQuestion
    } else {
        LineIntent::Answer
    }
}

/// Context that goes before each prompt of the side chat (the timeline
/// shows the prompt alone).
#[must_use]
pub fn preamble(gate: &GateInfo) -> String {
    let topic = gate
        .topic
        .as_deref()
        .map(|t| format!(" It is about: {t}."))
        .unwrap_or_default();
    format!(
        "[crest] The user stepped out of the {flow} flow, which waits at the stage \
         \"{stage}\" with this question:\n\n{prompt}\n\n{topic_line}Help the user with what \
         they ask below; they may be looking into something to answer that question. Do not \
         answer the flow for them: they go back to it when they are ready.",
        flow = gate.flow.name(),
        stage = gate.stage,
        prompt = gate.prompt.trim(),
        topic_line = if topic.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", topic.trim())
        },
    )
}

/// Context of the chat turn that proposes the answer to the gate from the
/// side conversation, when the user goes back to the flow.
#[must_use]
pub fn proposal_context(gate: &GateInfo) -> String {
    format!(
        "[crest] The user is going back to the {flow} flow, which waits at the stage \
         \"{stage}\" with this question:\n\n{prompt}\n\nFrom this conversation (and the \
         user's line below), write the answer the user gives to that question. Reply with \
         the answer alone, short and in the user's language: no preamble, no tool calls.",
        flow = gate.flow.name(),
        stage = gate.stage,
        prompt = gate.prompt.trim(),
    )
}

/// Words a line that only says "back to the flow" is made of.
const RETURN_FILLER: &[&str] = &[
    "ok", "okay", "vale", "dale", "ya", "bien", "perfecto", "listo", "lista", "esta", "eso", "es",
    "todo", "al", "el", "a", "con", "la", "de", "nuevo", "the", "to", "go", "now", "back", "flujo",
    "flow", "done", "great", "good", "thanks", "gracias", "entonces",
];

/// Words that make such a line ask to go back.
const RETURN_VERBS: &[&str] = &[
    "continua",
    "continuar",
    "continuemos",
    "continue",
    "sigue",
    "seguir",
    "sigamos",
    "volvamos",
    "volver",
    "vuelve",
    "regresa",
    "regresemos",
    "retoma",
    "retomar",
    "retomemos",
    "adelante",
    "resume",
    "proceed",
    "back",
];

/// Whether `text`, typed in the side chat, only says to go back to the
/// flow: "listo, continúa", "volvamos al flujo", "back to the flow"…
#[must_use]
pub fn is_return(text: &str) -> bool {
    let normalized = normalize(text);
    let words: Vec<&str> = normalized
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    !words.is_empty()
        && words.iter().any(|w| RETURN_VERBS.contains(w))
        && words
            .iter()
            .all(|w| RETURN_VERBS.contains(w) || RETURN_FILLER.contains(w))
}

/// Whether `text`, typed in the side chat, says to go back to the flow:
/// [`is_return`], or free text the decision model confidently reads as
/// such ("listo, la respuesta es X, continúa").
pub async fn wants_return(text: &str, gate: &GateInfo, decider: &Decider) -> bool {
    if is_return(text) {
        return true;
    }
    if is_approval(text) || !decider.is_available() {
        return false;
    }
    let state = format!(
        "The user stepped out of the {} flow to chat; it waits for the answer to: {}\n\nThe \
         user wrote: {text}",
        gate.flow.name(),
        gate.prompt
    );
    let request = DecisionRequest::new(state).ask(
        "line",
        Question::choice(
            "Does the user say to go back to the flow?",
            [
                (
                    "return",
                    "says they are done and to go back to (or continue) the flow",
                ),
                ("chat", "keeps asking or talking"),
            ],
        ),
    );
    decider.decide(&request).await.is_some_and(|r| {
        r.choice("line")
            .is_some_and(|(c, p)| c == "return" && p >= SIDE_CONFIDENCE)
    })
}

/// The text of the last assistant message of `turns`, trimmed; `None` when
/// it has no text.
#[must_use]
pub fn last_answer(turns: &[Turn]) -> Option<String> {
    let message = turns
        .last()?
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)?;
    let text: Vec<&str> = message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let text = text.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::conversation::Message;

    use super::*;
    use crate::decide::DecisionOptions;
    use crate::testing::decision::{ScriptedDecider, choice};

    fn gate() -> GateInfo {
        GateInfo {
            flow: FlowKind::Research,
            stage: "Definition and constraints".into(),
            prompt: "Which latency is acceptable?".into(),
            topic: Some("the cache is slow".into()),
        }
    }

    fn deciding(option: &'static str, p: f32) -> Decider {
        Decider::new(
            Arc::new(ScriptedDecider::answering(move |_, _| choice(option, p))),
            DecisionOptions::default(),
        )
    }

    #[tokio::test]
    async fn only_a_sure_decision_model_turns_a_line_into_a_side_question() {
        let line = "¿qué latencia tiene hoy el endpoint de búsqueda?";
        assert_eq!(
            classify(line, &gate(), &Decider::none()).await,
            LineIntent::Answer
        );
        assert_eq!(
            classify(line, &gate(), &deciding("side", 0.9)).await,
            LineIntent::SideQuestion
        );
        assert_eq!(
            classify(line, &gate(), &deciding("side", 0.6)).await,
            LineIntent::Answer
        );
        assert_eq!(
            classify(line, &gate(), &deciding("answer", 0.9)).await,
            LineIntent::Answer
        );
        // Yeses and commands never ask the model.
        for line in ["ok", "/back 3"] {
            assert_eq!(
                classify(line, &gate(), &deciding("side", 0.99)).await,
                LineIntent::Answer
            );
        }
    }

    #[test]
    fn return_lines_only_say_to_go_back() {
        for yes in [
            "listo, continúa",
            "Listo. Continua!",
            "volvamos al flujo",
            "ok, sigue con el flujo",
            "back to the flow",
            "done, continue",
            "retomemos",
        ] {
            assert!(is_return(yes), "{yes:?}");
        }
        for no in [
            "",
            "ok",
            "listo",
            "¿y si continúa fallando?",
            "continúa investigando la latencia del cache",
        ] {
            assert!(!is_return(no), "{no:?}");
        }
    }

    #[tokio::test]
    async fn the_decision_model_reads_returns_with_an_answer() {
        let line = "listo, la respuesta es 200 ms, continúa";
        assert!(!wants_return(line, &gate(), &Decider::none()).await);
        assert!(wants_return(line, &gate(), &deciding("return", 0.9)).await);
        assert!(!wants_return(line, &gate(), &deciding("chat", 0.9)).await);
        assert!(wants_return("listo, continúa", &gate(), &Decider::none()).await);
    }

    #[test]
    fn contexts_carry_the_gate() {
        let preamble = preamble(&gate());
        assert!(
            preamble.contains("Which latency is acceptable?")
                && preamble.contains("Definition and constraints")
                && preamble.contains("the cache is slow"),
            "{preamble}"
        );
        let proposal = proposal_context(&gate());
        assert!(
            proposal.contains("Which latency is acceptable?"),
            "{proposal}"
        );
    }

    #[test]
    fn the_last_answer_is_the_text_of_the_last_assistant_message() {
        let mut turn = Turn::new(Message::user_text("q"));
        assert_eq!(last_answer(std::slice::from_ref(&turn)), None);
        turn.messages.push(Message::assistant(vec![
            ContentPart::text("  200 ms "),
            ContentPart::text("p95"),
        ]));
        assert_eq!(last_answer(&[turn]).as_deref(), Some("200 ms \np95"));
        assert_eq!(last_answer(&[]), None);
    }
}
