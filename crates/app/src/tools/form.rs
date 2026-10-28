//! `ask_user`: the model asks the user structured questions as a form.
//!
//! The input is a title, an optional description and the questions as a
//! JSON Schema subset (see [`domain::form::schema`]). Preparing the call
//! converts and validates the schema (a bad one comes back to the model as
//! an input error naming the property) and adds a pending `Form` block;
//! running it awaits the [`FormPort`] with that block's id. A submitted
//! answer is checked against the fields ([`FormSpec::check_answer`]); an
//! invalid one is asked again with the reason, up to [`MAX_ATTEMPTS`]
//! times. The block then shows the answer read-only and the model gets
//! the values as JSON. A user who types in the chat instead of filling the
//! form closes it too: the block shows their words and the model gets
//! them as the answer. A cancelled form is marked so in the block and
//! answers the model with an error saying the user declined, so the turn
//! goes on; a turn interrupted while the form waits ends the same way
//! ([`PreparedCall::on_interrupt`]). The call only reads: it needs no
//! approval.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use domain::block::{BlockId, BlockKind};
use domain::form::{FormSpec, FormState, fields_from_schema};
use domain::permission::ToolEffect;
use futures::FutureExt as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use crate::ports::form::{FormPort, FormRequest, FormRequestId, FormResponse};

/// Name of the tool.
pub const ASK_USER: &str = "ask_user";

/// Answers rejected before the call gives up.
pub const MAX_ATTEMPTS: usize = 3;

/// What every closed form tells the model, so it does not loop on it.
const DO_NOT_RE_ASK: &str =
    "Do not ask the same form again; go on without these answers or ask in prose.";

/// What the system prompt says about `ask_user`.
pub const FORM_TOOL_PROMPT: &str = "\
# Asking the user

When you need structured input from the user (choices among options, several values, \
settings, yes/no decisions) call `ask_user` with a form instead of asking in prose. It waits \
for the answer and returns it as JSON. Ask for dates with `format: \"date\"` (one day) or \
`format: \"date-range\"` (a period), never as free text or as options. List in `required` \
every field you cannot go on without. The user may answer in the chat instead of filling the \
form: you then get their words, take them as the answer. If the user cancels, do not ask the \
same form again: go on without the answers or ask in prose.";

const DESCRIPTION: &str = "Shows the user a form in the timeline and waits until they submit \
or cancel it. Use it when you need structured answers (pick one or several options, numbers \
with bounds, yes/no, short or long text) instead of asking in prose. `schema` is a JSON Schema \
object: `properties` are the fields in display order and `required` lists the mandatory ones. \
Each property has `type` string, number, integer, boolean or array, plus optional `title` (the \
label), `description` and `default`. Strings take `minLength`/`maxLength` (whole numbers) and \
`format: \"multiline\"` for long text. Dates are never free text: `format: \"date\"` shows a \
date picker and returns `YYYY-MM-DD`, `format: \"date-range\"` shows a range picker and \
returns `{\"start\": \"YYYY-MM-DD\", \"end\": \"YYYY-MM-DD\"}`, both bounded by \
`formatMinimum`/`formatMaximum` (`email`, `uri` and `date-time` are shown as text); numbers and \
integers take `minimum`/`maximum`; one choice is a string with `enum` (labels in \
`enumNames`) or `oneOf: [{const, title}]`; several choices are `{type: \"array\", items: \
{type: \"string\", enum: [...]}}` with `minItems`/`maxItems` (whole numbers). Annotations \
such as `examples`, `pattern` or `readOnly` are accepted but not enforced; nested objects, \
`$ref`, `anyOf`, `allOf` and `const` are rejected. Returns the submitted values by field name \
(empty optional fields are absent), the user's words when they answer in the chat instead, or \
an error when the user cancels.";

/// Input of `ask_user`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUser {
    /// Heading of the form: what you need to know.
    pub title: String,
    /// Why you ask, shown under the heading.
    #[serde(default)]
    pub description: Option<String>,
    /// The questions: a JSON Schema object with `properties` (one per
    /// field, in order) and `required`.
    #[schemars(extend("type" = "object"))]
    pub schema: Value,
    /// Text of the submit button; `Submit` when absent.
    #[serde(default)]
    pub submit_label: Option<String>,
}

/// The `ask_user` tool.
pub struct AskUserTool {
    port: Arc<dyn FormPort>,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for AskUserTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskUserTool").finish_non_exhaustive()
    }
}

impl AskUserTool {
    /// A tool asking through `port`.
    #[must_use]
    pub fn new(port: Arc<dyn FormPort>) -> Self {
        Self {
            port,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool: ASK_USER.to_owned(),
        message: message.into(),
    }
}

/// The form of `input`, pending.
///
/// # Errors
///
/// An input error naming the property the schema cannot express, or the
/// field whose bounds or default are wrong.
pub fn form_spec(input: AskUser) -> Result<FormSpec, ToolError> {
    if input.title.trim().is_empty() {
        return Err(invalid("`title` is empty"));
    }
    let fields = fields_from_schema(&input.schema).map_err(|err| invalid(err.to_string()))?;
    let spec = FormSpec {
        title: input.title,
        description: input.description.filter(|d| !d.trim().is_empty()),
        fields,
        submit_label: input.submit_label.filter(|l| !l.trim().is_empty()),
        state: FormState::Pending,
        answer: None,
        reply: None,
    };
    spec.validate().map_err(|err| invalid(err.to_string()))?;
    Ok(spec)
}

#[async_trait]
impl TypedTool for AskUserTool {
    type Input = AskUser;

    fn name(&self) -> &str {
        ASK_USER
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn effect(&self, _input: &AskUser) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: AskUser, ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let spec = form_spec(input)?;
        // Derived from the call: asked again after a restart, the form
        // lands in the block the timeline already shows.
        let block_id = BlockId::for_tool_use(ctx.tool_use_id.as_str(), "form");
        let port = Arc::clone(&self.port);
        let next_id = Arc::clone(&self.next_id);
        let tool_use_id = ctx.tool_use_id.clone();
        let subject = spec.title.clone();
        let shown = BlockKind::Form(spec.clone());
        // Interrupted while the user was filling it: closed like a cancel.
        let interrupted = {
            let mut done = spec.clone();
            done.state = FormState::Cancelled;
            let text = format!(
                "{}: the turn was interrupted while the form waited. {DO_NOT_RE_ASK}",
                ToolError::Cancelled
            );
            ToolOutput::error(text).with_block_update(BlockKind::Form(done))
        };
        let run = async move {
            let mut error = None;
            for _ in 0..MAX_ATTEMPTS {
                let request = FormRequest {
                    id: FormRequestId(next_id.fetch_add(1, Ordering::Relaxed)),
                    block_id,
                    tool_use_id: tool_use_id.clone(),
                    spec: spec.clone(),
                    error: error.take(),
                };
                match port.ask(request).await {
                    FormResponse::Submitted { answer } => match spec.check_answer(&answer) {
                        Ok(()) => {
                            let json = serde_json::to_string_pretty(&answer)
                                .map_err(|err| ToolError::Execution(err.to_string()))?;
                            let mut done = spec;
                            done.state = FormState::Submitted;
                            done.answer = Some(answer);
                            let text = format!("The user submitted the form:\n{json}");
                            return Ok(
                                ToolOutput::text(text).with_block_update(BlockKind::Form(done))
                            );
                        }
                        Err(err) => error = Some(err.to_string()),
                    },
                    FormResponse::Replied { text } => {
                        let mut done = spec;
                        done.state = FormState::Replied;
                        done.reply = Some(text.clone());
                        let text = format!(
                            "The user did not fill the form and answered in the chat instead:\n{text}"
                        );
                        return Ok(ToolOutput::text(text).with_block_update(BlockKind::Form(done)));
                    }
                    FormResponse::Cancelled { reason } => {
                        let mut done = spec;
                        done.state = FormState::Cancelled;
                        let why = reason.map(|r| format!(" ({r})")).unwrap_or_default();
                        let text = format!(
                            "{}: the user closed the form without answering{why}. {DO_NOT_RE_ASK}",
                            ToolError::Cancelled
                        );
                        return Ok(ToolOutput::error(text).with_block_update(BlockKind::Form(done)));
                    }
                }
            }
            let mut done = spec;
            done.state = FormState::Cancelled;
            let text = format!(
                "the answer was rejected {MAX_ATTEMPTS} times (last: {}); the form was closed",
                error.unwrap_or_default()
            );
            Ok(ToolOutput::error(text).with_block_update(BlockKind::Form(done)))
        };
        Ok(PreparedCall::new(subject, run.boxed())
            .with_block_as(block_id, shown)
            .with_interrupt_output(interrupted))
    }
}

/// Registers `ask_user`, asking through `port`.
///
/// # Errors
///
/// [`RegisterError::Duplicate`] when a tool of that name is registered.
pub fn register_form_tool(
    registry: &mut ToolRegistry,
    port: Arc<dyn FormPort>,
) -> Result<(), RegisterError> {
    registry.register_typed(AskUserTool::new(port))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::testing::form::ScriptedForm;
    use crate::tools::registry::{Tool, Typed};

    fn ctx() -> ToolContext {
        ToolContext::new(
            SessionId::new(),
            TurnId::new(),
            ToolUseId::new("toolu_form"),
        )
    }

    fn input() -> Value {
        json!({
            "title": "Test settings",
            "description": "Pick how to compare the groups",
            "schema": {
                "type": "object",
                "properties": {
                    "test": {"type": "string", "enum": ["welch", "student"], "title": "Test"},
                    "alpha": {"type": "number", "minimum": 0, "maximum": 1, "default": 0.05},
                    "notes": {"type": "string", "format": "multiline"}
                },
                "required": ["test"]
            }
        })
    }

    fn answer(value: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    fn tool(port: &Arc<ScriptedForm>) -> Typed<AskUserTool> {
        Typed(AskUserTool::new(Arc::clone(port) as Arc<dyn FormPort>))
    }

    fn form(output: &ToolOutput) -> &FormSpec {
        match &output.block_update {
            Some(BlockKind::Form(spec)) => spec,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn definition() {
        let tool = tool(&Arc::new(ScriptedForm::default()));
        insta::assert_json_snapshot!(
            "ask_user_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        assert_eq!(tool.effect(&input()), ToolEffect::Read);
        assert!(FORM_TOOL_PROMPT.contains("`ask_user`"));
    }

    #[tokio::test]
    async fn a_submitted_form_returns_the_answers_and_shows_them() {
        let port = Arc::new(ScriptedForm::new([FormResponse::Submitted {
            answer: answer(json!({"test": "welch", "alpha": 0.01})),
        }]));
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        assert_eq!(call.subject, "Test settings");
        let (Some(BlockKind::Form(pending)), Some(block_id)) = (&call.block, call.block_id) else {
            panic!("{call:?}");
        };
        assert_eq!(pending.state, FormState::Pending);
        assert_eq!(pending.fields.len(), 3);
        let output = call.run.await.unwrap();
        assert!(!output.is_error);
        let text = output.text_content();
        assert!(text.contains("\"alpha\": 0.01"), "{text}");
        assert!(text.contains("\"test\": \"welch\""), "{text}");
        let done = form(&output);
        assert_eq!(done.state, FormState::Submitted);
        assert_eq!(done.answer.as_ref().unwrap()["test"], "welch");
        let requests = port.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].block_id, block_id);
        assert_eq!(requests[0].tool_use_id, ToolUseId::new("toolu_form"));
        assert_eq!(requests[0].error, None);
    }

    #[tokio::test]
    async fn an_invalid_answer_is_asked_again_with_the_reason() {
        let port = Arc::new(ScriptedForm::new([
            FormResponse::Submitted {
                answer: answer(json!({"alpha": 0.01})),
            },
            FormResponse::Submitted {
                answer: answer(json!({"test": "student"})),
            },
        ]));
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        let output = call.run.await.unwrap();
        assert!(!output.is_error, "{}", output.text_content());
        let requests = port.requests();
        assert_eq!(requests.len(), 2);
        assert_ne!(requests[0].id, requests[1].id);
        assert_eq!(requests[0].block_id, requests[1].block_id);
        let reason = requests[1].error.as_deref().unwrap();
        assert!(reason.contains("\"test\" is required"), "{reason}");
        assert_eq!(form(&output).answer.as_ref().unwrap()["test"], "student");
    }

    #[tokio::test]
    async fn answers_rejected_too_often_close_the_form() {
        let bad = || FormResponse::Submitted {
            answer: answer(json!({"test": "other"})),
        };
        let port = Arc::new(ScriptedForm::new([bad(), bad(), bad(), bad()]));
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        let output = call.run.await.unwrap();
        assert!(output.is_error);
        assert_eq!(port.requests().len(), MAX_ATTEMPTS);
        assert_eq!(form(&output).state, FormState::Cancelled);
    }

    #[tokio::test]
    async fn a_reply_in_the_chat_is_the_answer() {
        let port = Arc::new(ScriptedForm::new([FormResponse::Replied {
            text: "mejor en Python".into(),
        }]));
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        let output = call.run.await.unwrap();
        assert!(!output.is_error);
        let text = output.text_content();
        assert!(text.contains("answered in the chat"), "{text}");
        assert!(text.ends_with("mejor en Python"), "{text}");
        let done = form(&output);
        assert_eq!(done.state, FormState::Replied);
        assert_eq!(done.reply.as_deref(), Some("mejor en Python"));
        assert_eq!(done.answer, None);
    }

    #[tokio::test]
    async fn a_cancelled_form_tells_the_model_the_user_declined() {
        let port = Arc::new(ScriptedForm::new([FormResponse::cancelled("closed")]));
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        let output = call.run.await.unwrap();
        assert!(output.is_error);
        let text = output.text_content();
        assert!(text.starts_with("cancelled by user"), "{text}");
        assert!(text.contains("(closed)"), "{text}");
        let done = form(&output);
        assert_eq!(done.state, FormState::Cancelled);
        assert_eq!(done.answer, None);
    }

    #[tokio::test]
    async fn an_interruption_closes_the_form_and_tells_the_model_not_to_re_ask() {
        let port = Arc::new(ScriptedForm::default());
        let call = tool(&port).prepare(input(), &ctx()).await.unwrap();
        let output = call.on_interrupt.unwrap();
        assert!(output.is_error);
        let text = output.text_content();
        assert!(text.starts_with("cancelled by user"), "{text}");
        assert!(text.contains("Do not ask the same form again"), "{text}");
        assert_eq!(form(&output).state, FormState::Cancelled);
        assert!(port.requests().is_empty(), "nothing was asked");
    }

    #[tokio::test]
    async fn bad_schemas_are_input_errors_naming_the_property() {
        let port = Arc::new(ScriptedForm::default());
        let tool = tool(&port);
        let mut nested = input();
        nested["schema"]["properties"]["alpha"] = json!({"type": "object"});
        let mut bad_default = input();
        bad_default["schema"]["properties"]["alpha"]["default"] = json!(2);
        let mut no_title = input();
        no_title["title"] = json!(" ");
        for (bad, needle) in [
            (nested, "properties.alpha"),
            (bad_default, "\"alpha\""),
            (no_title, "`title`"),
        ] {
            let err = tool.prepare(bad, &ctx()).await.unwrap_err();
            let ToolError::InvalidInput { message, .. } = &err else {
                panic!("{err:?}");
            };
            assert!(message.contains(needle), "{message}");
        }
        assert!(port.requests().is_empty());
    }
}
