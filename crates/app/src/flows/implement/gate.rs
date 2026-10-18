//! What the user can answer at the gates of the implementation flow.
//!
//! Every gate accepts the same words, so the chat reads the same at each:
//! «sí»/«continuar» goes on, «reintentar», «saltar», «abortar» and
//! «volver al spec»; anything else is free text (feedback) for the step.

use crate::flows::research::gate::{is_approval, normalize};

/// The user's answer at a gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateChoice {
    /// Go on (`sí`, `ok`, `continuar`…).
    Continue,
    /// Try again (`reintentar`).
    Retry,
    /// Skip the current task (`saltar`).
    Skip,
    /// Stop the flow (`abortar`).
    Abort,
    /// Stop and go back to the Spec node of the research flow
    /// (`volver al spec`).
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
        "reintentar" | "reintenta" | "otra vez" | "retry" => GateChoice::Retry,
        "saltar" | "salta" | "saltar tarea" | "saltar la tarea" | "skip" => GateChoice::Skip,
        "abortar" | "aborta" | "abort" | "cancelar" | "detener" => GateChoice::Abort,
        "volver al spec"
        | "volver a spec"
        | "volver al nodo spec"
        | "spec"
        | "back spec"
        | "back" => GateChoice::BackToSpec,
        _ => GateChoice::Other(text.trim().to_owned()),
    }
}

/// The options line shown at the gates that stop on a problem.
pub const PROBLEM_OPTIONS: &str = "Responde «reintentar» (o escribe instrucciones para \
el agente), «continuar» para seguir con la tarea como está, «saltar» para dejarla sin \
commit, «abortar» o «volver al spec» si el plan resultó inviable.";

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
        assert_eq!(
            parse_choice("  usa un Mutex en vez de RwLock "),
            GateChoice::Other("usa un Mutex en vez de RwLock".into())
        );
    }
}
