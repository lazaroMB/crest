//! Structured answers from the model: a forced tool call whose input is
//! the JSON, with JSON in the text as fallback for adapters that ignore
//! `tool_choice`. Invalid answers are asked again with their errors.

use domain::conversation::{ContentPart, Message};
use futures::StreamExt;
use schemars::JsonSchema;
use serde_json::Value;

use crate::flows::research::node::{NodeContext, NodeError};
use crate::ports::llm::{LlmError, MessageAssembler, ToolChoice, ToolDefinition};

/// Appended to the prompt when the previous answer was invalid.
const RETRY_PROMPT: &str = "Your previous answer is not valid for these reasons:";

/// Closes [`RETRY_PROMPT`].
const RETRY_CLOSING: &str = "Fix every reason and send the whole answer again with the same tool.";

/// A tool named `name` whose input schema is the JSON Schema of `T`.
///
/// # Errors
///
/// [`NodeError::Failed`] when the schema does not serialize.
pub(crate) fn submit_tool<T: JsonSchema>(
    name: &str,
    description: &str,
) -> Result<ToolDefinition, NodeError> {
    let schema = serde_json::to_value(schemars::schema_for!(T))
        .map_err(|e| NodeError::Failed(format!("schema of {name}: {e}")))?;
    Ok(ToolDefinition::new(name, description, schema))
}

/// Asks `prompt` until `check` accepts the answer, at most `1 + retries`
/// times. Each retry repeats the prompt with the errors of the previous
/// answer and adds a line to `notes`, for the caller to write to the
/// journal (it only reads the context, so several can run at once).
///
/// # Errors
///
/// The model's errors, or [`NodeError::Failed`] with the last errors when
/// no answer was valid.
pub(crate) async fn ask_valid<T>(
    ctx: &NodeContext,
    notes: &mut Vec<String>,
    system: &str,
    prompt: &str,
    tool: &ToolDefinition,
    retries: usize,
    mut check: impl FnMut(Value) -> Result<T, Vec<String>>,
) -> Result<T, NodeError> {
    let mut errors: Vec<String> = Vec::new();
    for attempt in 0..=retries {
        let text = if errors.is_empty() {
            prompt.to_owned()
        } else {
            let list: String = errors.iter().map(|e| format!("\n- {e}")).collect();
            format!("{prompt}\n\n{RETRY_PROMPT}{list}\n\n{RETRY_CLOSING}")
        };
        let answer = ask_json(ctx, system, &text, tool).await?;
        match answer.and_then(&mut check) {
            Ok(value) => return Ok(value),
            Err(found) => {
                errors = found;
                if attempt < retries {
                    notes.push(format!(
                        "Invalid answer from {}; asking again: {}",
                        tool.name,
                        errors.join("; ")
                    ));
                }
            }
        }
    }
    Err(NodeError::Failed(format!(
        "{} invalid after {} attempts: {}",
        tool.name,
        retries + 1,
        errors.join("; ")
    )))
}

/// One request forcing `tool`; the outer error is the transport's, the
/// inner one an answer that is not JSON.
async fn ask_json(
    ctx: &NodeContext,
    system: &str,
    prompt: &str,
    tool: &ToolDefinition,
) -> Result<Result<Value, Vec<String>>, NodeError> {
    let mut req = ctx.request(system, vec![Message::user_text(prompt)]);
    req.tools = vec![tool.clone()];
    req.tool_choice = ToolChoice::Tool(tool.name.clone());
    let model = req.model.spec.id.clone();
    let mut stream = ctx.llm().stream(req).await?;
    let mut assembler = MessageAssembler::new();
    while let Some(event) = stream.next().await {
        assembler
            .push(event?)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    let assembled = assembler
        .finish()
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    let model = assembled.model.unwrap_or(model);
    ctx.record_usage(&model, assembled.usage, 1);
    let message = assembled.message;
    let mut text = String::new();
    for part in message.content {
        match part {
            ContentPart::ToolUse { name, input, .. } if name == tool.name => {
                return Ok(Ok(input));
            }
            ContentPart::Text { text: t } => text.push_str(&t),
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Ok(Err(vec![format!(
            "you neither called the tool {} nor answered JSON",
            tool.name
        )]));
    }
    Ok(json_in_text(&text).map_err(|e| vec![e]))
}

/// The JSON object in `text`: the whole text, a fenced block or the span
/// from the first `{` to the last `}`.
fn json_in_text(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    let candidate = match (start, end) {
        (Some(s), Some(e)) if s < e => &trimmed[s..=e],
        _ => trimmed,
    };
    serde_json::from_str(candidate).map_err(|e| format!("the JSON could not be read: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_found_in_fenced_text() {
        let value = json_in_text("Here it is:\n```json\n{\"a\": 1}\n```\n").unwrap();
        assert_eq!(value["a"], 1);
        assert!(json_in_text("nothing").is_err());
    }
}
