//! What the user can answer at the gates of the implementation flow.
//!
//! Every gate accepts the same words, so the chat reads the same at each:
//! "yes"/"continue" goes on, "retry", "skip", "abort" and "back to spec"
//! (the Spanish words still work); anything else is free text (feedback)
//! for the step.

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

/// The options line shown at the gates that stop on a problem.
pub const PROBLEM_OPTIONS: &str = "Answer \"retry\" (or write instructions for the \
agent), \"continue\" to go on with the task as it is, \"skip\" to leave it without a \
commit, \"abort\", or \"back to spec\" if the plan turned out to be unworkable.";

#[cfg(test)]
mod tests {
    use super::*;

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
