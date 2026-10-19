//! What the user can answer at the gates of the implementation flow.
//!
//! Every gate accepts the same words, so the chat reads the same at each:
//! "yes"/"continue" goes on, "retry", "skip", "abort" and "back to spec"
//! (the Spanish words still work); anything else is free text (feedback)
//! for the step.

use domain::decision::{DecisionRequest, Question};

use crate::decide::Decider;
use crate::flows::research::gate::{is_approval, normalize};

/// The user's answer at a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateChoice {
    /// Go on (`yes`, `ok`, `continue`…).
    Continue,
    /// Try again (`retry`).
    Retry,
    /// Skip the current task (`skip`).
    Skip,
    /// Stop the flow (`abort`).
    Abort,
    /// Stop and go back to the Spec node of the research flow
    /// (`back to spec`).
    BackToSpec,
    /// Anything else, trimmed: instructions or feedback for the step.
    Other(String),
}

/// Parses an answer typed at a gate (case and accents ignored).
#[must_use]
pub fn parse_choice(text: &str) -> GateChoice {
    if is_approval(text) {
        return GateChoice::Continue;
    }
    let normalized = normalize(text);
    let word = normalized.trim_end_matches(['.', '!', ',']).trim();
    let word = word.strip_prefix('/').unwrap_or(word);
    match word {
        "continue" | "go on" | "proceed" => GateChoice::Continue,
        "reintentar" | "reintenta" | "otra vez" | "retry" | "try again" => GateChoice::Retry,
        "saltar" | "salta" | "saltar tarea" | "saltar la tarea" | "skip" | "skip task"
        | "skip the task" => GateChoice::Skip,
        "abortar" | "aborta" | "abort" | "cancelar" | "detener" | "cancel" | "stop" => {
            GateChoice::Abort
        }
        "back to spec"
        | "back to the spec"
        | "go back to spec"
        | "go back to the spec"
        | "volver al spec"
        | "volver a spec"
        | "volver al nodo spec"
        | "spec"
        | "back spec"
        | "back" => GateChoice::BackToSpec,
        _ => GateChoice::Other(text.trim().to_owned()),
    }
}

/// Confidence from which the decision model's reading of an answer is
/// taken.
const CHOICE_CONFIDENCE: f32 = 0.7;

/// Reads an answer typed at a gate: the fixed phrases of
/// [`parse_choice`], then, for free text, the decision model (when it is
/// confident the text means one of the choices rather than instructions).
pub async fn interpret(text: &str, decider: &Decider) -> GateChoice {
    let parsed = parse_choice(text);
    if !matches!(parsed, GateChoice::Other(_)) || !decider.is_available() {
        return parsed;
    }
    let request = DecisionRequest::new(text).ask(
        "choice",
        Question::choice(
            "The implementation flow stopped at a gate and the user typed this answer. What \
             does it ask for?",
            [
                ("continue", "go on (approve, accept as it is)"),
                ("retry", "try the same step again"),
                ("skip", "skip the current task"),
                ("abort", "stop the implementation"),
                (
                    "back_to_spec",
                    "go back to the Spec or the plan to change it",
                ),
                (
                    "instructions",
                    "gives instructions or details for the next attempt",
                ),
            ],
        ),
    );
    let picked = decider.decide(&request).await.and_then(|r| {
        r.choice("choice")
            .filter(|(_, confidence)| *confidence >= CHOICE_CONFIDENCE)
            .map(|(choice, _)| choice.to_owned())
    });
    match picked.as_deref() {
        Some("continue") => GateChoice::Continue,
        Some("retry") => GateChoice::Retry,
        Some("skip") => GateChoice::Skip,
        Some("abort") => GateChoice::Abort,
        Some("back_to_spec") => GateChoice::BackToSpec,
        _ => parsed,
    }
}

/// The options line shown at the gates that stop on a problem.
pub const PROBLEM_OPTIONS: &str = "Answer \"retry\" (or write instructions for the \
agent), \"continue\" to go on with the task as it is, \"skip\" to leave it without a \
commit, \"abort\", or \"back to spec\" if the plan turned out to be unworkable.";

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::decide::DecisionOptions;
    use crate::testing::decision::{ScriptedDecider, choice};

    fn decider(option: &'static str, confidence: f32) -> (Decider, Arc<ScriptedDecider>) {
        let port = Arc::new(ScriptedDecider::answering(move |_, _| {
            choice(option, confidence)
        }));
        let decider = Decider::new(Arc::clone(&port) as _, DecisionOptions::default());
        (decider, port)
    }

    #[tokio::test]
    async fn free_text_is_read_by_the_decision_model_when_it_is_sure() {
        let (sure, port) = decider("retry", 0.9);
        assert_eq!(interpret("retry", &sure).await, GateChoice::Retry);
        assert!(port.requests().is_empty(), "fixed phrases need no model");
        assert_eq!(
            interpret("dale otra vuelta por favor", &sure).await,
            GateChoice::Retry
        );
        assert_eq!(port.requests().len(), 1);

        let (unsure, _) = decider("abort", 0.4);
        assert_eq!(
            interpret("hmm no sé", &unsure).await,
            GateChoice::Other("hmm no sé".into())
        );
        let (instructions, _) = decider("instructions", 0.95);
        assert_eq!(
            interpret("use a Mutex", &instructions).await,
            GateChoice::Other("use a Mutex".into())
        );
        assert_eq!(
            interpret("dale otra vuelta", &Decider::none()).await,
            GateChoice::Other("dale otra vuelta".into())
        );
    }

    #[test]
    fn parses_the_gate_words() {
        assert_eq!(parse_choice("Sí"), GateChoice::Continue);
        assert_eq!(parse_choice("continuar."), GateChoice::Continue);
        assert_eq!(parse_choice("Reintentar"), GateChoice::Retry);
        assert_eq!(parse_choice("saltar tarea"), GateChoice::Skip);
        assert_eq!(parse_choice("/abort"), GateChoice::Abort);
        assert_eq!(parse_choice("Volver al Spec"), GateChoice::BackToSpec);
        assert_eq!(parse_choice("Continue."), GateChoice::Continue);
        assert_eq!(parse_choice("retry"), GateChoice::Retry);
        assert_eq!(parse_choice("Skip"), GateChoice::Skip);
        assert_eq!(parse_choice("abort"), GateChoice::Abort);
        assert_eq!(parse_choice("Back to spec"), GateChoice::BackToSpec);
        assert_eq!(
            parse_choice("  use a Mutex instead of RwLock "),
            GateChoice::Other("use a Mutex instead of RwLock".into())
        );
    }
}
