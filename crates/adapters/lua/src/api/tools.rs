//! `app.tools.register`: tools written in Lua and offered to the model.
//!
//! ```lua
//! app.tools.register({
//!   name = "todo_list",
//!   description = "Muestra tareas pendientes del proyecto",
//!   schema = { type = "object", properties = { items = { type = "array" } } },
//!   handler = function(input) return input.items end,
//! })
//! ```
//!
//! The handler receives the model's input as a table and its return value
//! goes back to the model: a string as is, anything else as JSON.

use std::sync::Arc;

use application::ports::plugin::PluginToolSpec;
use mlua::{Function, Lua, Value};
use serde_json::json;

use crate::config::api::{Origin, Sink, ToolReg, lock, report};
use crate::{callbacks, convert};

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
            },
            handler,
        });
        Ok(())
    })
}
