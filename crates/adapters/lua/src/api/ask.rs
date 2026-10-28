//! `app.ask{...}`: a form shown to the user from a plugin tool or command,
//! answered in place, the same form `ask_user` shows.
//!
//! ```lua
//! local answer, why = app.ask{
//!   title = 'Deploy',
//!   schema = {
//!     properties = {
//!       env = { type = 'string', enum = { 'staging', 'prod' } },
//!       dry_run = { type = 'boolean', default = true },
//!     },
//!     required = { 'env' },
//!   },
//!   order = { 'env', 'dry_run' },
//! }
//! if not answer then return 'cancelled: ' .. why end
//! ```
//!
//! The fields are those of `ask_user` (`title`, `description`, `schema`,
//! `submit_label`) plus `order`, the property names in display order: Lua
//! tables have no order, so without it the fields are sorted by name. The
//! schema may also be a JSON string, which keeps its order.
//!
//! The call adds a `Form` block to the timeline through the front end's
//! event sink and waits for the answer through the [`FormPort`]: the
//! submitted values by field name, or `nil` and the reason when the user
//! cancels (or nobody can answer). A prompt sent while the form waits
//! answers it in prose: `nil`, [`REPLIED`] and the words as a third
//! value. Like the other blocking calls, it
//! holds the VM thread while it waits, so other plugin calls (hooks,
//! commands) wait too; use it in tools and commands, not in hooks. The
//! block belongs to this run, like plugin modals: it is not saved with
//! the session. An answer that does not fit the fields is asked again
//! with the reason, up to [`MAX_ATTEMPTS`] times.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::ports::events::DomainEvent;
use application::ports::form::{FormPort, FormRequest, FormRequestId, FormResponse};
use application::tools::form::{AskUser, MAX_ATTEMPTS, form_spec};
use domain::block::{Block, BlockKind};
use domain::conversation::ToolUseId;
use domain::form::{FormSpec, FormState};
use mlua::{Lua, MultiValue, Table, Value};
use serde_json::{Map, Value as Json};

use crate::caps::context;
use crate::config::api::Origin;
use crate::convert;

/// Ids of the requests `app.ask` sends, unique in the process.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

fn error(message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.ask: {message}"))
}

/// The form a call describes, pending, with its fields in `order`.
fn spec(lua: &Lua, args: &Table) -> mlua::Result<FormSpec> {
    let mut fields = Map::new();
    for key in ["title", "description", "submit_label"] {
        let value = convert::to_json(lua, args.get::<Value>(key)?)?;
        if !value.is_null() {
            fields.insert(key.into(), value);
        }
    }
    let schema = match args.get::<Value>("schema")? {
        Value::String(text) => serde_json::from_str(&text.to_str()?)
            .map_err(|e| error(format!("`schema` is not valid JSON: {e}")))?,
        Value::Table(table) => convert::to_json(lua, Value::Table(table))?,
        other => {
            return Err(error(format!(
                "`schema` must be a table or a JSON string, got {}",
                other.type_name()
            )));
        }
    };
    fields.insert("schema".into(), schema);
    let input: AskUser = serde_json::from_value(Json::Object(fields)).map_err(error)?;
    let mut spec = form_spec(input).map_err(error)?;
    if let Value::Table(order) = args.get::<Value>("order")? {
        let names: Vec<String> = order
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?;
        let rank = |name: &str| names.iter().position(|n| n == name).unwrap_or(usize::MAX);
        spec.fields.sort_by_key(|f| rank(&f.name));
    }
    Ok(spec)
}

/// Reason of `app.ask` returning `nil`: the user replied in the chat (with
/// the words), cancelled the form, or answered wrong too many times.
#[derive(Debug)]
pub(crate) struct Refused {
    reason: String,
    reply: Option<String>,
}

/// The `why` of a form answered in the chat instead.
pub(crate) const REPLIED: &str = "the user answered in the chat instead";

/// Shows `spec` and waits for an answer that fits it: `Ok(answer)`, or
/// `Err(reason)` when the form was cancelled or kept being rejected. The
/// block is updated to its final state either way.
fn ask(
    lua: &Lua,
    origin: &Origin,
    port: &Arc<dyn FormPort>,
    spec: FormSpec,
) -> mlua::Result<Result<BTreeMap<String, Json>, Refused>> {
    let ctx = context(lua)?;
    let events = ctx
        .events
        .as_ref()
        .ok_or_else(|| error("no window to show the form in"))?;
    let mut block = Block::new(BlockKind::Form(spec.clone()));
    events.0.emit(DomainEvent::BlockAdded {
        block: block.clone(),
    });
    let tool_use_id = ToolUseId::new(format!("plugin:{}", origin.plugin));
    let mut rejected = None;
    let mut outcome = Err(Refused {
        reason: String::new(),
        reply: None,
    });
    for _ in 0..MAX_ATTEMPTS {
        let request = FormRequest {
            id: FormRequestId(NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)),
            block_id: block.id,
            tool_use_id: tool_use_id.clone(),
            spec: spec.clone(),
            error: rejected.take(),
        };
        match ctx.block_on(port.ask(request))? {
            FormResponse::Submitted { answer } => match spec.check_answer(&answer) {
                Ok(()) => {
                    outcome = Ok(answer);
                    break;
                }
                Err(err) => {
                    let reason = err.to_string();
                    outcome = Err(Refused {
                        reason: format!(
                            "the answer was rejected {MAX_ATTEMPTS} times (last: {reason})"
                        ),
                        reply: None,
                    });
                    rejected = Some(reason);
                }
            },
            FormResponse::Replied { text } => {
                outcome = Err(Refused {
                    reason: REPLIED.into(),
                    reply: Some(text),
                });
                break;
            }
            FormResponse::Cancelled { reason } => {
                outcome = Err(Refused {
                    reason: reason.unwrap_or_else(|| "the user closed the form".into()),
                    reply: None,
                });
                break;
            }
        }
    }
    let mut done = spec;
    match &outcome {
        Ok(answer) => {
            done.state = FormState::Submitted;
            done.answer = Some(answer.clone());
        }
        Err(Refused {
            reply: Some(text), ..
        }) => {
            done.state = FormState::Replied;
            done.reply = Some(text.clone());
        }
        Err(_) => done.state = FormState::Cancelled,
    }
    block.kind = BlockKind::Form(done);
    events.0.emit(DomainEvent::BlockUpdated { block });
    Ok(outcome)
}

/// The `app.ask` function of one phase.
pub(crate) fn function(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<mlua::Function> {
    let origin = Arc::clone(origin);
    lua.create_function(move |lua, args: Value| {
        let Value::Table(args) = args else {
            return Err(error("expected a table with `title` and `schema`"));
        };
        let port = context(lua)?
            .forms()
            .ok_or_else(|| error("forms are not available here (only once the app has started)"))?;
        let spec = spec(lua, &args)?;
        match ask(lua, &origin, &port, spec)? {
            Ok(answer) => {
                let map: Map<String, Json> = answer.into_iter().collect();
                Ok(MultiValue::from_vec(vec![convert::to_lua(
                    lua,
                    &Json::Object(map),
                )?]))
            }
            Err(Refused { reason, reply }) => {
                let mut values = vec![Value::Nil, Value::String(lua.create_string(&reason)?)];
                if let Some(text) = reply {
                    values.push(Value::String(lua.create_string(&text)?));
                }
                Ok(MultiValue::from_vec(values))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use application::testing::form::ScriptedForm;
    use application::testing::system::RecordingSink;
    use serde_json::json;

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::caps::Context;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run(
        code: &str,
        form: Option<Arc<ScriptedForm>>,
    ) -> (mlua::Result<Json>, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let result = (|| {
            let lua = sandbox::new_lua(None)?;
            callbacks::install(&lua, CallbackIds::default())?;
            let events = Arc::clone(&sink) as Arc<dyn application::ports::events::EventSink>;
            let ctx = Context::new(None, false, Some(Vec::new())).with_events(Some(events));
            if let Some(form) = form {
                ctx.provide_forms(form);
            }
            lua.set_app_data(Arc::new(ctx));
            let phase = Phase::new(&lua, "init.lua", "deploy")?;
            let value: Value = lua
                .load(code)
                .set_name("@init.lua")
                .set_environment(phase.env.clone())
                .eval()?;
            convert::to_json(&lua, value)
        })();
        (result, sink)
    }

    const ASK: &str = "
        local answer, why = app.ask{
          title = 'Deploy',
          schema = {
            properties = {
              env = { type = 'string', enum = { 'staging', 'prod' } },
              count = { type = 'integer', minimum = 1 },
            },
            required = { 'env' },
          },
          order = { 'env', 'count' },
        }
        return { answer = answer, why = why }";

    fn forms(sink: &RecordingSink) -> Vec<FormSpec> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                    match block.kind {
                        BlockKind::Form(spec) => Some(spec),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_submitted_form_returns_the_answer_and_shows_it() {
        let form = Arc::new(ScriptedForm::new(vec![FormResponse::Submitted {
            answer: BTreeMap::from([("env".to_owned(), json!("prod"))]),
        }]));
        let (out, sink) = run(ASK, Some(Arc::clone(&form)));
        assert_eq!(out.unwrap(), json!({ "answer": { "env": "prod" } }));
        let shown = forms(&sink);
        assert_eq!(shown.len(), 2);
        let names: Vec<&str> = shown[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["env", "count"]);
        assert_eq!(shown[0].state, FormState::Pending);
        assert_eq!(shown[1].state, FormState::Submitted);
        let requests = form.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tool_use_id, ToolUseId::new("plugin:deploy"));
    }

    #[test]
    fn a_cancelled_or_rejected_form_returns_nil_and_why() {
        let form = Arc::new(ScriptedForm::new(vec![FormResponse::cancelled(
            "no terminal",
        )]));
        let (out, sink) = run(ASK, Some(form));
        assert_eq!(out.unwrap(), json!({ "why": "no terminal" }));
        assert_eq!(forms(&sink)[1].state, FormState::Cancelled);

        let bad = FormResponse::Submitted {
            answer: BTreeMap::from([("env".to_owned(), json!("moon"))]),
        };
        let form = Arc::new(ScriptedForm::new(vec![bad.clone(), bad.clone(), bad]));
        let (out, _) = run(ASK, Some(Arc::clone(&form)));
        let why = out.unwrap()["why"].as_str().unwrap_or_default().to_owned();
        assert!(why.contains("rejected"), "{why}");
        let requests = form.requests();
        assert_eq!(requests.len(), MAX_ATTEMPTS);
        assert!(requests[1].error.is_some());
    }

    #[test]
    fn a_form_answered_in_the_chat_returns_nil_why_and_the_words() {
        let form = Arc::new(ScriptedForm::new(vec![FormResponse::Replied {
            text: "prod, no dry run".into(),
        }]));
        let (out, sink) = run(
            "local answer, why, reply = app.ask{ title = 'Deploy', schema = { properties = { env = { type = 'string' } } } }
             return { answer = answer, why = why, reply = reply }",
            Some(form),
        );
        assert_eq!(
            out.unwrap(),
            json!({ "why": REPLIED, "reply": "prod, no dry run" })
        );
        let shown = forms(&sink);
        assert_eq!(shown[1].state, FormState::Replied);
        assert_eq!(shown[1].reply.as_deref(), Some("prod, no dry run"));
    }

    #[test]
    fn a_json_schema_keeps_its_order_and_bad_calls_raise() {
        let form = Arc::new(ScriptedForm::new(vec![FormResponse::cancelled("x")]));
        let code = r#"return app.ask{ title = 'T', schema = '{"properties": {"z": {"type": "string"}, "a": {"type": "string"}}}' }"#;
        let (_, sink) = run(code, Some(form));
        let names: Vec<String> = forms(&sink)[0]
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["z", "a"]);

        let err = |code: &str, form: Option<Arc<ScriptedForm>>| {
            run(code, form).0.unwrap_err().to_string()
        };
        assert!(err("return app.ask{ title = 'T', schema = {} }", None).contains("not available"));
        let some = || Some(Arc::new(ScriptedForm::new(Vec::new())));
        assert!(err("return app.ask(3)", some()).contains("expected a table"));
        assert!(err("return app.ask{ title = '', schema = { properties = { a = { type = 'string' } } } }", some()).contains("title"));
        assert!(err("return app.ask{ title = 'T', schema = 5 }", some()).contains("`schema`"));
    }
}
