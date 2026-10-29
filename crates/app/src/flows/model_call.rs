//! The plumbing of one model call, shared by research nodes and the
//! discovery engine: build the request, stream and assemble the answer,
//! meter it, and build the conversation of the one JSON retry.

use domain::conversation::{ContentPart, Message};
use domain::model::ResolvedModel;
use futures::StreamExt;

use crate::flows::research::json::retry_message;
use crate::flows::research::usage::UsageMeter;
use crate::ports::llm::{
    AssembledMessage, LlmError, LlmPort, LlmRequest, MessageAssembler, SystemBlock,
};

/// A request to `model` with `system` (omitted when empty) and `messages`.
pub(crate) fn request(model: ResolvedModel, system: &str, messages: Vec<Message>) -> LlmRequest {
    let mut req = LlmRequest::new(model, messages);
    if !system.is_empty() {
        req.system = vec![SystemBlock::new(system)];
    }
    req
}

/// Streams `req`, assembles the answer and records its usage (one call) in
/// `meter`. `on_first_event` runs once, when the first stream event
/// arrives.
pub(crate) async fn stream_metered(
    llm: &dyn LlmPort,
    req: LlmRequest,
    meter: &UsageMeter,
    on_first_event: impl FnOnce(),
) -> Result<AssembledMessage, LlmError> {
    let model = req.model.spec.id.clone();
    let mut stream = llm.stream(req).await?;
    let mut assembler = MessageAssembler::new();
    let mut on_first_event = Some(on_first_event);
    while let Some(event) = stream.next().await {
        if let Some(first) = on_first_event.take() {
            first();
        }
        assembler
            .push(event?)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    let assembled = assembler
        .finish()
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    meter.record(
        assembled.model.as_ref().unwrap_or(&model),
        assembled.usage,
        1,
    );
    Ok(assembled)
}

/// `messages` followed by the rejected `answer` and the message asking
/// for it again with the `error`: the conversation of the one retry.
pub(crate) fn retry_conversation(
    mut messages: Vec<Message>,
    answer: String,
    error: &str,
) -> Vec<Message> {
    messages.push(Message::assistant(vec![ContentPart::text(answer)]));
    messages.push(Message::user_text(retry_message(error)));
    messages
}
