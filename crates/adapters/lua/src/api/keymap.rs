//! `app.keymap.set` and `app.commands.register`.
//!
//! ```lua
//! app.keymap.set("n", "<leader>r", "session.resume")
//! app.keymap.set({ "n", "i" }, "<C-x>", function() app.cmd("clear") end, { desc = "Clear" })
//! app.commands.register("review", function(args) ... end, { desc = "Review the changes" })
//! ```
//!
//! The optional last table (Neovim style) takes `desc`, the description the
//! help shows next to the mapping or the command.
//!
//! A keymap's action is a built-in action by name or a Lua function; a
//! command (`:review args`) is a function receiving the arguments as a
//! string. The functions stay in the VM as callbacks; bad arguments become
//! diagnostics with the caller's line.
//!
//! Inside those callbacks, `app.cmd(name)` (a built-in command or action,
//! e.g. `"clear"`) and `app.send(text, { role = "plan" })` (a prompt, with
//! an optional model role) ask the app to do something. They are recorded
//! in call order and the callback's result becomes
//! [`PluginOutput::Actions`](application::ports::plugin::PluginOutput),
//! which the UI runs once the callback returned.

use std::sync::Arc;

use application::ports::plugin::{CommandSpec, KeyAction, KeyMode, KeymapSpec, PluginAction};
use domain::model::ModelRole;
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
    app.set("cmd", lua.create_function(cmd)?)?;
    app.set("send", lua.create_function(send)?)?;
    Ok(())
}

/// Actions recorded by `app.cmd` and `app.send` during the current call.
#[derive(Debug, Default)]
struct PendingActions(Vec<PluginAction>);

fn push(lua: &Lua, action: PluginAction) {
    match lua.app_data_mut::<PendingActions>() {
        Some(mut pending) => pending.0.push(action),
        None => {
            lua.set_app_data(PendingActions(vec![action]));
        }
    }
}

/// Takes the actions recorded since the last call.
pub(crate) fn take_actions(lua: &Lua) -> Vec<PluginAction> {
    lua.app_data_mut::<PendingActions>()
        .map(|mut pending| std::mem::take(&mut pending.0))
        .unwrap_or_default()
}

fn cmd(lua: &Lua, command: Value) -> mlua::Result<()> {
    let Value::String(command) = command else {
        return Err(mlua::Error::runtime(format!(
            "app.cmd: expected a command name, got {}",
            command.type_name()
        )));
    };
    let command = command.to_str()?.trim().to_owned();
    if command.is_empty() {
        return Err(mlua::Error::runtime("app.cmd: the command is empty"));
    }
    push(lua, PluginAction::Cmd { command });
    Ok(())
}

fn send(lua: &Lua, (text, opts): (Value, Option<Table>)) -> mlua::Result<()> {
    let Value::String(text) = text else {
        return Err(mlua::Error::runtime(format!(
            "app.send: expected the prompt text, got {}",
            text.type_name()
        )));
    };
    let text = text.to_str()?.to_owned();
    let role = match opts.map(|o| o.get::<Option<String>>("role")).transpose()? {
        Some(Some(name)) => Some(
            name.parse::<ModelRole>()
                .map_err(|e| mlua::Error::runtime(format!("app.send: {e}")))?,
        ),
        _ => None,
    };
    push(lua, PluginAction::Send { text, role });
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

/// The `desc` of an optional options table (`{ desc = "..." }`).
fn description(what: &str, opts: &Value) -> Result<Option<String>, String> {
    let table = match opts {
        Value::Nil => return Ok(None),
        Value::Table(table) => table,
        other => {
            return Err(format!(
                "{what}: the options must be a table, got {}",
                other.type_name()
            ));
        }
    };
    match table.get::<Value>("desc").map_err(|e| e.to_string())? {
        Value::Nil => Ok(None),
        Value::String(s) => {
            let desc = s.to_str().map_err(|e| e.to_string())?.trim().to_owned();
            Ok((!desc.is_empty()).then_some(desc))
        }
        other => Err(format!(
            "{what}: desc must be a string, got {}",
            other.type_name()
        )),
    }
}

fn keymap_set(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(
        move |lua, (mode, lhs, action, opts): (Value, Value, Value, Value)| {
            let parsed = (|| {
                let modes = key_modes(&mode)?;
                let description = description("keymap.set", &opts)?;
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
                        description: description.clone(),
                        plugin: origin.plugin.clone(),
                    })
                    .collect::<Vec<_>>())
            })();
            match parsed {
                Ok(specs) => lock(&sink).keymaps.extend(specs),
                Err(message) => report(lua, &sink, &origin, message),
            }
            Ok(())
        },
    )
}

fn commands_register(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, (name, f, opts): (Value, Value, Value)| {
        match (name, f) {
            (Value::String(name), Value::Function(f)) => {
                let description = match description("commands.register", &opts) {
                    Ok(description) => description,
                    Err(message) => {
                        report(lua, &sink, &origin, message);
                        return Ok(());
                    }
                };
                let name = name.to_str()?.trim_start_matches('/').to_owned();
                let callback = callbacks::register(lua, f)?;
                lock(&sink).commands.push(CommandSpec {
                    name,
                    callback,
                    description,
                    plugin: origin.plugin.clone(),
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::CallbackIds;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run(code: &str) -> (Lua, mlua::Result<()>) {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        let phase = Phase::new(&lua, "init.lua", "config").unwrap();
        let ran = lua
            .load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .exec();
        (lua, ran)
    }

    #[test]
    fn cmd_and_send_are_recorded_in_order() {
        let (lua, ran) = run(r#"app.cmd("clear")
            app.send("Revisa", { role = "plan" })
            app.send("hola")"#);
        ran.unwrap();
        assert_eq!(
            take_actions(&lua),
            [
                PluginAction::Cmd {
                    command: "clear".into()
                },
                PluginAction::Send {
                    text: "Revisa".into(),
                    role: Some(ModelRole::Plan)
                },
                PluginAction::Send {
                    text: "hola".into(),
                    role: None
                },
            ]
        );
        assert!(take_actions(&lua).is_empty(), "taken once");
    }

    #[test]
    fn bad_arguments_raise_errors() {
        for code in [
            "app.cmd(3)",
            "app.cmd('  ')",
            "app.send({})",
            "app.send('x', { role = 'boss' })",
        ] {
            let (lua, ran) = run(code);
            assert!(ran.is_err(), "{code}");
            assert!(take_actions(&lua).is_empty(), "{code}");
        }
    }
}
