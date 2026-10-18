//! Structured answers of the model: the JSON object of an answer, parsed
//! with serde and validated, and the messages of the one retry the nodes
//! allow (see [`NodeContext::ask_json`]).
//!
//! [`NodeContext::ask_json`]: super::node::NodeContext::ask_json

use domain::conversation::{ContentPart, Role, Session};
use domain::research::Stage;
use serde::de::DeserializeOwned;

use super::node::NodeError;

/// Start of the message sent back after an invalid answer.
pub const RETRY_PREFIX: &str = "Tu respuesta anterior no es válida";

/// From the first `{` to the last `}` of `text`, if any.
fn braces(text: &str) -> Option<&str> {
    match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => Some(&text[start..=end]),
        _ => None,
    }
}

/// The body of the first code fence of `text` (```` ```json ```` or plain).
fn fence_body(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("```")?;
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.split_once("```").map(|(body, _)| body)
}

/// The JSON object of a model answer: from its first `{` to its last `}`,
/// so code fences and stray text around it are ignored; when that span
/// does not parse, the object inside the first code fence.
///
/// # Errors
///
/// The serde error (of the whole span), as text to send back to the model.
pub fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let Some(span) = braces(text) else {
        return Err("no hay ningún objeto JSON en la respuesta".into());
    };
    let error = match serde_json::from_str(span) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    fence_body(text)
        .and_then(braces)
        .filter(|fenced| *fenced != span)
        .and_then(|fenced| serde_json::from_str(fenced).ok())
        .ok_or_else(|| format!("JSON inválido: {error}"))
}

/// [`parse_json`], then `validate`.
///
/// # Errors
///
/// The parse or validation error, as text for the model.
pub fn parse_valid<T: DeserializeOwned>(
    text: &str,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<T, String> {
    let value = parse_json(text)?;
    validate(&value)?;
    Ok(value)
}

/// The message sent back to the model when its answer is invalid.
#[must_use]
pub fn retry_message(error: &str) -> String {
    format!(
        "{RETRY_PREFIX}: {error}. Responde de nuevo solo con el JSON corregido, sin texto \
         alrededor."
    )
}

/// The error of a node whose model answered invalid JSON twice.
#[must_use]
pub fn invalid_answer(stage: Stage, error: &str) -> NodeError {
    NodeError::Failed(format!(
        "{}: el modelo devolvió dos veces un JSON inválido: {error}",
        stage.name_es()
    ))
}

/// The text of the last assistant message of the last turn of `session`.
#[must_use]
pub fn final_text(session: &Session) -> String {
    session
        .turns
        .last()
        .and_then(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
        })
        .map(|m| text_of(&m.content))
        .unwrap_or_default()
}

/// The text parts of `parts`, joined and trimmed.
#[must_use]
pub fn text_of(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct A {
        a: serde_json::Value,
    }

    #[test]
    fn json_is_found_inside_prose_and_fences() {
        let a = |t: &str| parse_json::<A>(t).map(|v| v.a);
        assert_eq!(a("Aquí va:\n{\"a\": 1}\nlisto"), Ok(1.into()));
        assert_eq!(a("```json\n{\"a\": {}}\n```"), Ok(serde_json::json!({})));
        // A fence inside a string of the object does not confuse it.
        assert_eq!(
            a("{\"a\": \"```mermaid\\ngraph TD\\n```\"}"),
            Ok("```mermaid\ngraph TD\n```".into())
        );
        // Braces in the prose around a fenced object.
        assert_eq!(a("uso {x}:\n```json\n{\"a\": 2}\n```\n{fin}"), Ok(2.into()));
        assert!(a("nada").unwrap_err().contains("ningún objeto"));
        assert!(a("{\"b\": 1}").unwrap_err().starts_with("JSON inválido"));
    }

    #[test]
    fn validation_errors_are_returned_as_text() {
        let err = parse_valid::<A>("{\"a\": 1}", |_| Err("no".into())).unwrap_err();
        assert_eq!(err, "no");
        assert!(retry_message("falta x").starts_with(RETRY_PREFIX));
        let NodeError::Failed(m) = invalid_answer(Stage::Brainstorm, "e") else {
            panic!("failed");
        };
        assert!(m.contains("Lluvia de ideas"), "{m}");
    }
}
