//! `app.keymap.set` and `app.commands.register`.
//!
//! ```lua
//! app.keymap.set("n", "<leader>r", "session.resume")
//! app.keymap.set({ "n", "i" }, "<C-x>", function() app.cmd("clear") end)
//! app.commands.register("review", function(args) ... end)
//! ```
//!
//! A keymap's action is a built-in action by name or a Lua function; a
//! command (`:review args`) is a function receiving the arguments. The
//! functions stay in the VM as callbacks; bad arguments become diagnostics
//! with the caller's line.

use std::sync::Arc;

use application::ports::plugin::{CommandSpec, KeyAction, KeyMode, KeymapSpec};
use mlua::{Function, Lua, Table, Value};

use crate::callbacks;
use crate::config::api::{Origin, Sink, lock, report};

/// Adds `keymap` and `commands` to the `app` table of one phase.
pub(crate) fn install(
    lua: &Lua,
    app: &Table,
    sink: &Sink,
    origin: &Arc<Origin>,
) -> mlua::Result<()> {
    let keymap = lua.create_table()?;
    keymap.set("set", keymap_set(lua, sink, origin)?)?;
    app.set("keymap", keymap)?;
    let commands = lua.create_table()?;
    commands.set("register", commands_register(lua, sink, origin)?)?;
    app.set("commands", commands)?;
    Ok(())
}

fn key_mode(name: &str) -> Option<KeyMode> {
    match name {
        "i" | "insert" => Some(KeyMode::Insert),
        "n" | "normal" => Some(KeyMode::Normal),
        "c" | "command" => Some(KeyMode::Command),
        _ => None,
    }
}

fn key_modes(value: &Value) -> Result<Vec<KeyMode>, String> {
    let names: Vec<String> = match value {
        Value::String(s) => vec![s.to_str().map_err(|e| e.to_string())?.to_owned()],
        Value::Table(t) => t
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .map_err(|e| e.to_string())?,
        other => {
            return Err(format!(
                "mode must be a string or a list, got {}",
                other.type_name()
            ));
        }
    };
    names
        .iter()
        .map(|n| {
            key_mode(n).ok_or_else(|| format!("unknown mode `{n}` (use \"n\", \"i\" or \"c\")"))
        })
        .collect()
}

fn keymap_set(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, (mode, lhs, action): (Value, Value, Value)| {
        let parsed = (|| {
            let modes = key_modes(&mode)?;
            let Value::String(lhs) = lhs else {
                return Err("keymap.set: the key sequence must be a string".to_owned());
            };
            let lhs = lhs.to_str().map_err(|e| e.to_string())?.to_owned();
            let action = match action {
                Value::String(s) => {
                    KeyAction::Builtin(s.to_str().map_err(|e| e.to_string())?.to_owned())
                }
                Value::Function(f) => {
                    KeyAction::Callback(callbacks::register(lua, f).map_err(|e| e.to_string())?)
                }
                other => {
                    return Err(format!(
                        "keymap.set: the action must be a string or a function, got {}",
                        other.type_name()
                    ));
                }
            };
            Ok(modes
                .into_iter()
                .map(|mode| KeymapSpec {
                    mode,
                    lhs: lhs.clone(),
                    action: action.clone(),
                })
                .collect::<Vec<_>>())
        })();
        match parsed {
            Ok(specs) => lock(&sink).keymaps.extend(specs),
            Err(message) => report(lua, &sink, &origin, message),
        }
        Ok(())
    })
}

fn commands_register(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, (name, f): (Value, Value)| {
        match (name, f) {
            (Value::String(name), Value::Function(f)) => {
                let name = name.to_str()?.trim_start_matches('/').to_owned();
                let callback = callbacks::register(lua, f)?;
                lock(&sink).commands.push(CommandSpec { name, callback });
            }
            _ => report(
                lua,
                &sink,
                &origin,
                "commands.register expects a name and a function".into(),
            ),
        }
        Ok(())
    })
}
