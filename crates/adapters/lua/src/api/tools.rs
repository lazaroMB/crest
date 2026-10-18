//! `app.tools.register`: tools written in Lua and offered to the model.
//!
//! ```lua
//! app.tools.register({
//!   name = "todo_list",
//!   description = "Shows the project's pending tasks",
//!   schema = { type = "object", properties = { items = { type = "array" } } },
//!   handler = function(input) return input.items end,
//! })
//! ```
//!
//! The handler receives the model's input as a table and its return value
//! goes back to the model: a string as is, anything else as JSON. Errors
//! and runs stopped by the instruction limit reach the model as errors.
//!
//! `effect` tells the permission policy what a call does: `"read"`,
//! `"write"`, `"execute"` or `"network"`. It defaults to `"execute"`, so an
//! undeclared tool is approved like a command; a read-only tool should say
//! `effect = "read"` to run without asking.

use std::sync::Arc;

use application::ports::plugin::PluginToolSpec;
use domain::permission::ToolEffect;
use mlua::{Function, Lua, Value};
use serde_json::json;

use crate::config::api::{Origin, Sink, ToolReg, lock, report};
use crate::{callbacks, convert};

/// Reads the `effect` field of a tool definition.
fn effect(value: &Value) -> Result<ToolEffect, String> {
    let name = match value {
        Value::Nil => return Ok(ToolEffect::Execute),
        Value::String(s) => s.to_str().map_err(|e| e.to_string())?.to_owned(),
        other => {
            return Err(format!(
                "`effect` must be a string, got {}",
                other.type_name()
            ));
        }
    };
    match name.as_str() {
        "read" => Ok(ToolEffect::Read),
        "write" => Ok(ToolEffect::Write),
        "execute" => Ok(ToolEffect::Execute),
        "network" => Ok(ToolEffect::Network),
        _ => Err(format!(
            "unknown effect `{name}` (use \"read\", \"write\", \"execute\" or \"network\")"
        )),
    }
}

/// Builds `app.tools.register` for one phase.
pub(crate) fn register(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, def: Value| {
        let Value::Table(def) = def else {
            report(lua, &sink, &origin, "tools.register expects a table".into());
            return Ok(());
        };
        let name: Option<String> = def.get("name")?;
        let handler: Option<Function> = def.get("handler")?;
        let (Some(name), Some(handler)) = (name.filter(|n| !n.is_empty()), handler) else {
            report(
                lua,
                &sink,
                &origin,
                "tools.register needs a `name` and a `handler` function".into(),
            );
            return Ok(());
        };
        let effect = match effect(&def.get::<Value>("effect")?) {
            Ok(effect) => effect,
            Err(message) => {
                report(
                    lua,
                    &sink,
                    &origin,
                    format!("tools.register `{name}`: {message}"),
                );
                return Ok(());
            }
        };
        let description: Option<String> = def.get("description")?;
        let input_schema = match def.get::<Value>("schema")? {
            Value::Nil => json!({ "type": "object" }),
            schema => convert::to_json(lua, schema)?,
        };
        let handler = callbacks::register(lua, handler)?;
        lock(&sink).tools.push(ToolReg {
            spec: PluginToolSpec {
                name,
                description: description.unwrap_or_default(),
                input_schema,
                plugin: origin.plugin.clone(),
                effect,
            },
            handler,
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::CallbackIds;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn register(code: &str) -> crate::config::api::Collected {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        let phase = Phase::new(&lua, "init.lua", "todo").unwrap();
        lua.load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .exec()
            .unwrap();
        phase.take()
    }

    fn effects(code: &str) -> Vec<ToolEffect> {
        register(code).tools.iter().map(|t| t.spec.effect).collect()
    }

    #[test]
    fn effect_defaults_to_execute() {
        let got = effects("app.tools.register({ name = 't', handler = function() end })");
        assert_eq!(got, [ToolEffect::Execute]);
    }

    #[test]
    fn declared_effects_are_read() {
        let got = effects(
            r#"for _, e in ipairs({ "read", "write", "execute", "network" }) do
                 app.tools.register({ name = e, effect = e, handler = function() end })
               end"#,
        );
        assert_eq!(
            got,
            [
                ToolEffect::Read,
                ToolEffect::Write,
                ToolEffect::Execute,
                ToolEffect::Network
            ]
        );
    }

    #[test]
    fn a_bad_effect_is_a_diagnostic_and_skips_the_tool() {
        let got = register(
            "app.tools.register({ name = 't', effect = 'delete', handler = function() end })\n\
             app.tools.register({ name = 'u', effect = 3, handler = function() end })",
        );
        assert!(got.tools.is_empty());
        let messages: Vec<_> = got.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            messages[0].contains("unknown effect `delete`"),
            "{messages:?}"
        );
        assert!(messages[1].contains("must be a string"), "{messages:?}");
        assert_eq!(got.diagnostics[1].line, Some(2));
    }
}
