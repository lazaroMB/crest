//! One structured answer from the model: a forced tool call whose input is
//! the JSON, with JSON in the text as fallback for adapters that ignore
//! `tool_choice` (as the research flow does).

use domain::conversation::{ContentPart, Message};
use domain::model::ResolvedModel;
use futures::StreamExt as _;
use serde_json::Value;

use crate::ports::llm::{
    LlmError, LlmPort, LlmRequest, MessageAssembler, SystemBlock, ToolChoice, ToolDefinition,
};

/// The answer, or why it is not JSON (`Err` inside), or the transport's
/// error (outer `Err`).
pub(crate) async fn ask_json(
    llm: &dyn LlmPort,
    model: ResolvedModel,
    system: &str,
    prompt: &str,
    tool: &ToolDefinition,
) -> Result<Result<Value, String>, LlmError> {
    let mut req = LlmRequest::new(model, vec![Message::user_text(prompt)]);
    req.system = vec![SystemBlock::new(system)];
    req.tools = vec![tool.clone()];
    req.tool_choice = ToolChoice::Tool(tool.name.clone());
    let mut stream = llm.stream(req).await?;
    let mut assembler = MessageAssembler::new();
    while let Some(event) = stream.next().await {
        assembler
            .push(event?)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    let message = assembler
        .finish()
        .map_err(|e| LlmError::Decode(e.to_string()))?
        .message;
    let mut text = String::new();
    for part in message.content {
        match part {
            ContentPart::ToolUse { name, input, .. } if name == tool.name => return Ok(Ok(input)),
            ContentPart::Text { text: t } => text.push_str(&t),
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Ok(Err(format!(
            "you neither called the tool {} nor answered JSON",
            tool.name
        )));
    }
    let trimmed = text.trim();
    let candidate = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(s), Some(e)) if s < e => &trimmed[s..=e],
        _ => trimmed,
    };
    Ok(serde_json::from_str(candidate).map_err(|e| format!("the JSON could not be read: {e}")))
}
