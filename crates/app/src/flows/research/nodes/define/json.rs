//! Structured answers of the model: JSON parsed with serde, validated, and
//! asked again once with the error when invalid. Shared by the nodes of
//! the definition and solution-search stages.

use domain::conversation::{ContentPart, Message};
use serde::de::DeserializeOwned;

use crate::flows::research::node::{NodeContext, NodeError};

/// Sent after an invalid answer, followed by the error.
const RETRY_PROMPT: &str = "Tu respuesta no es válida. Responde solo con el JSON corregido, \
sin texto alrededor. Error:";

/// Parses the JSON object of `text`, ignoring code fences and any text
/// around the outermost braces.
///
/// # Errors
///
/// The serde error, as text for the model.
pub(crate) fn parse<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let start = text.find('{');
    let end = text.rfind('}');
    let json = match (start, end) {
        (Some(s), Some(e)) if s < e => &text[s..=e],
        _ => return Err("no hay ningún objeto JSON en la respuesta".into()),
    };
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Asks `prompt` and parses the answer as `T`, checked by `validate`;
/// asks once more with the error if it is invalid.
///
/// # Errors
///
/// The model's errors, or [`NodeError::Failed`] when the second answer is
/// still invalid.
pub(crate) async fn ask_json<T, F>(
    ctx: &NodeContext,
    system: &str,
    prompt: &str,
    validate: F,
) -> Result<T, NodeError>
where
    T: DeserializeOwned,
    F: Fn(&T) -> Result<(), String>,
{
    let answer = ctx.ask(system, prompt).await?;
    fix_json(
        ctx,
        system,
        vec![Message::user_text(prompt)],
        answer,
        validate,
    )
    .await
}

/// Parses `answer`, the model's reply to `messages`, as `T`; if invalid,
/// sends the error back once in the same conversation (without tools).
///
/// # Errors
///
/// As [`ask_json`].
pub(crate) async fn fix_json<T, F>(
    ctx: &NodeContext,
    system: &str,
    mut messages: Vec<Message>,
    answer: String,
    validate: F,
) -> Result<T, NodeError>
where
    T: DeserializeOwned,
    F: Fn(&T) -> Result<(), String>,
{
    let checked = |text: &str| parse::<T>(text).and_then(|v| validate(&v).map(|()| v));
    let error = match checked(&answer) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    messages.push(Message::assistant(vec![ContentPart::text(answer)]));
    messages.push(Message::user_text(format!("{RETRY_PROMPT} {error}")));
    let again = ctx.complete(ctx.request(system, messages)).await?;
    checked(&again).map_err(|e| {
        NodeError::Failed(format!(
            "{}: el modelo devolvió un JSON inválido dos veces: {e}",
            ctx.stage().name_es()
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::ModelPolicy;
    use domain::research::{ResearchState, Stage};
    use serde::Deserialize;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Answer {
        items: Vec<String>,
    }

    fn non_empty(a: &Answer) -> Result<(), String> {
        if a.items.is_empty() {
            Err("`items` está vacío".into())
        } else {
            Ok(())
        }
    }

    fn ctx(llm: Arc<ScriptedLlm>) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), Arc::new(RecordingSink::new()));
        NodeContext::new(Stage::Brainstorm, deps, "p", ResearchState::default())
    }

    #[test]
    fn parse_ignores_fences_and_prose() {
        let text = "Aquí va:\n```json\n{\"items\": [\"a\"]}\n```";
        assert_eq!(parse::<Answer>(text).unwrap().items, ["a"]);
        assert!(parse::<Answer>("sin json").is_err());
    }

    #[tokio::test]
    async fn retries_once_with_the_error() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("{\"items\": []}"),
            ScriptedResponse::text("{\"items\": [\"a\"]}"),
        ]));
        let ctx = ctx(Arc::clone(&llm));
        let answer: Answer = ask_json(&ctx, "s", "dame items", non_empty).await.unwrap();
        assert_eq!(answer.items, ["a"]);
        let retry = llm.last_request().unwrap();
        assert_eq!(retry.messages.len(), 3);
        let ContentPart::Text { text } = &retry.messages[2].content[0] else {
            panic!("expected text");
        };
        assert!(text.contains("`items` está vacío"));
    }

    #[tokio::test]
    async fn fails_after_two_invalid_answers() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("no"),
            ScriptedResponse::text("tampoco"),
        ]));
        let err = ask_json::<Answer, _>(&ctx(llm), "s", "p", non_empty)
            .await
            .unwrap_err();
        assert!(matches!(err, NodeError::Failed(m) if m.contains("Lluvia de ideas")));
    }
}
