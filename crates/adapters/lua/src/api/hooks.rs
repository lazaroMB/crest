//! `app.on`: hooks the agent loop runs around every tool call.
//!
//! ```lua
//! app.on("before_tool", function(ev)
//!   if ev.name == "run_command" and ev.input.command == "rm" then
//!     return { deny = true, reason = "rm is not allowed" }
//!   end
//! end)
//! ```
//!
//! A `before_tool` hook gets `{ type, name, input }` and returns `nil` to go
//! on, `{ deny = true, reason = "..." }` to stop the call, or
//! `{ input = {...} }` to run it with another input. An `after_tool` hook
//! gets `{ type, name, input, output }`; what it returns is ignored.

use std::sync::Arc;

use application::ports::plugin::{HookKind, HookOutcome, HookSpec};
use mlua::{Function, Lua, Value};

use crate::callbacks;
use crate::config::api::{Origin, Sink, lock, report};

fn kind(name: &str) -> Option<HookKind> {
    match name {
        "before_tool" => Some(HookKind::BeforeTool),
        "after_tool" => Some(HookKind::AfterTool),
        _ => None,
    }
}

/// Builds `app.on` for one phase.
pub(crate) fn on(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, (event, f): (String, Value)| {
        let Some(kind) = kind(&event) else {
            report(
                lua,
                &sink,
                &origin,
                format!("app.on: unknown event `{event}` (use \"before_tool\" or \"after_tool\")"),
            );
            return Ok(());
        };
        let Value::Function(f) = f else {
            report(lua, &sink, &origin, "app.on expects a function".into());
            return Ok(());
        };
        let callback = callbacks::register(lua, f)?;
        lock(&sink).hooks.push(HookSpec {
            kind,
            callback,
            plugin: origin.plugin.clone(),
        });
        Ok(())
    })
}

/// Reads what a hook returned.
pub(crate) fn outcome(ret: &serde_json::Value) -> HookOutcome {
    let serde_json::Value::Object(map) = ret else {
        return HookOutcome::Continue;
    };
    if map.get("deny").and_then(serde_json::Value::as_bool) == Some(true) {
        let reason = map
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        return HookOutcome::Deny { reason };
    }
    match map.get("input") {
        Some(input) => HookOutcome::ReplaceInput(input.clone()),
        None => HookOutcome::Continue,
    }
}
