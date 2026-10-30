//! `ask_user`: the model asks the user structured questions as a form.
//!
//! The input is the questions as a JSON Schema subset (see
//! [`domain::form::schema`]), an optional title and description. The
//! input is read forgivingly, since models often get it slightly wrong:
//! a missing title comes from the description or the first field, unknown
//! keys are ignored, `questions` stands for `schema`, and a schema given
//! flat (its `properties` at the top level) is taken as the schema.
//! Preparing the call
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
with bounds, yes/no, short or long text) instead of asking in prose. Only `schema` is required; \
give a short `title` (the heading) and a `description` when useful. `schema` is a JSON Schema \
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

/// Heading of a form whose call gave none and nothing to take it from.
pub const DEFAULT_TITLE: &str = "A few questions";

/// Longest heading taken from the description.
const MAX_DERIVED_TITLE: usize = 80;

// Deserialized by hand (`AskUser::from_value`): unknown keys are ignored,
// `questions` is an alias of `schema`, and an input with `properties` but
// no `schema` is the schema itself. The doc comment below goes to the model.
/// Input of `ask_user`.
#[derive(Debug, Clone, PartialEq, JsonSchema)]
pub struct AskUser {
    /// Heading of the form: what you need to know. Taken from the
    /// description (or the first question) when absent.
    #[serde(default)]
    pub title: Option<String>,
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

impl AskUser {
    /// Reads a call's input.
    ///
    /// # Errors
    ///
    /// The input is not an object, has no schema, or a key has the wrong
    /// type (named in the message).
    pub fn from_value(input: Value) -> Result<Self, String> {
        let Value::Object(mut map) = input else {
            return Err("invalid type: expected an object".into());
        };
        let text = |map: &mut serde_json::Map<String, Value>, key: &str| match map.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            Some(other) => Err(format!(
                "invalid type for `{key}`: expected a string, got {}",
                json_type(&other)
            )),
        };
        let title = text(&mut map, "title")?;
        let description = text(&mut map, "description")?;
        let submit_label = text(&mut map, "submit_label")?;
        let schema = match map.remove("schema").or_else(|| map.remove("questions")) {
            Some(schema) if !schema.is_null() => schema,
            // The schema given flat: its keys are the input's.
            _ if map.contains_key("properties") => {
                let mut schema = serde_json::Map::new();
                schema.insert("type".into(), Value::String("object".into()));
                for key in ["properties", "required"] {
                    if let Some(value) = map.remove(key) {
                        schema.insert(key.into(), value);
                    }
                }
                Value::Object(schema)
            }
            _ => return Err("missing field `schema`".into()),
        };
        Ok(Self {
            title,
            description,
            schema,
            submit_label,
        })
    }
}

impl<'de> Deserialize<'de> for AskUser {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

/// The JSON type of `value`, for messages.
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The first sentence of `text` (up to `.`, `?`, `!` or a line break),
/// when it is short enough to be a heading.
fn first_sentence(text: &str) -> Option<String> {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|&(at, c)| {
            matches!(c, '\n' | '?' | '!')
                || (c == '.' && text[at + 1..].starts_with(char::is_whitespace))
        })
        .map_or(
            text.len(),
            |(at, c)| if c == '\n' { at } else { at + c.len_utf8() },
        );
    let sentence = text[..end].trim().trim_end_matches('.').trim();
    (!sentence.is_empty() && sentence.chars().count() <= MAX_DERIVED_TITLE)
        .then(|| sentence.to_owned())
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
    let fields = fields_from_schema(&input.schema).map_err(|err| invalid(err.to_string()))?;
    let mut description = input.description.filter(|d| !d.trim().is_empty());
    let given = input
        .title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    let title = match given {
        Some(title) => title,
        None => {
            let from_description = description.as_deref().and_then(first_sentence);
            // A description that is only that sentence is not shown twice.
            if from_description.as_deref().is_some_and(|title| {
                description
                    .as_deref()
                    .map(|d| d.trim().trim_end_matches('.'))
                    == Some(title)
            }) {
                description = None;
            }
            from_description
                .or_else(|| {
                    fields
                        .first()
                        .and_then(|field| field.label.clone())
                        .filter(|label| !label.trim().is_empty())
                })
                .unwrap_or_else(|| DEFAULT_TITLE.to_owned())
        }
    };
    let spec = FormSpec {
        title,
        description,
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
        let mut number_title = input();
        number_title["title"] = json!(3);
        let mut no_schema = input();
        no_schema.as_object_mut().unwrap().remove("schema");
        for (bad, needle) in [
            (nested, "properties.alpha"),
            (bad_default, "\"alpha\""),
            (number_title, "`title`"),
            (no_schema, "Missing required: `schema`"),
        ] {
            let err = tool.prepare(bad, &ctx()).await.unwrap_err();
            let ToolError::InvalidInput { message, .. } = &err else {
                panic!("{err:?}");
            };
            assert!(message.contains(needle), "{message}");
        }
        assert!(port.requests().is_empty());
    }

    /// The pending form a call shows.
    async fn shown(input: Value) -> FormSpec {
        let call = tool(&Arc::new(ScriptedForm::default()))
            .prepare(input, &ctx())
            .await
            .unwrap();
        match call.block {
            Some(BlockKind::Form(spec)) => spec,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_missing_title_comes_from_the_description_or_the_first_field() {
        let mut no_title = input();
        no_title.as_object_mut().unwrap().remove("title");
        let spec = shown(no_title.clone()).await;
        assert_eq!(spec.title, "Pick how to compare the groups");
        assert_eq!(spec.description, None, "not shown twice");

        no_title["description"] = json!("Which test? The groups differ in size.");
        let spec = shown(no_title.clone()).await;
        assert_eq!(spec.title, "Which test?");
        assert_eq!(
            spec.description.as_deref(),
            Some("Which test? The groups differ in size.")
        );

        no_title["title"] = json!("  ");
        no_title["description"] = json!(null);
        assert_eq!(
            shown(no_title.clone()).await.title,
            "Test",
            "the first field"
        );

        no_title["schema"] = json!({"properties": {"x": {"type": "string"}}});
        assert_eq!(shown(no_title).await.title, DEFAULT_TITLE);
    }

    #[tokio::test]
    async fn unknown_keys_are_ignored_and_a_flat_schema_or_questions_is_the_schema() {
        let mut extra = input();
        extra["urgency"] = json!("high");
        assert_eq!(shown(extra).await.title, "Test settings");

        let flat = json!({
            "title": "Flat",
            "type": "object",
            "properties": {
                "name": {"type": "string", "title": "Name"},
                "age": {"type": "integer"}
            },
            "required": ["name"]
        });
        let spec = shown(flat).await;
        let fields: Vec<(&str, bool)> = spec
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.required))
            .collect();
        assert_eq!(fields, [("name", true), ("age", false)]);

        let mut questions = input();
        let schema = questions.as_object_mut().unwrap().remove("schema").unwrap();
        questions["questions"] = schema;
        assert_eq!(shown(questions).await.fields.len(), 3);
    }
}
