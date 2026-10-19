//! The `app` table each config file sees and the registrations it collects.
//!
//! Declarative fields (`app.opt`, `app.models`, `app.mcp`, ...) are plain
//! tables read after the file runs; see [`super::parse`]. The functions
//! (`app.keymap.set`, `app.commands.register`, `app.tools.register`,
//! `app.on`, `app.mcp.add`) record what they receive here and keep the Lua functions as
//! callbacks. Bad arguments become diagnostics with the caller's line instead
//! of aborting the file.

use std::sync::{Arc, Mutex, PoisonError};

use application::ports::plugin::{
    CallbackId, CommandSpec, HookSpec, KeymapSpec, McpServerSpec, PluginDiagnostic, PluginToolSpec,
};
use mlua::{Function, Lua, Table};

/// Builds a phase's environment: globals readable through `__index`, its own
/// `app` and a `require` that returns that `app`.
const NEW_ENV: &str = r#"
local app, base_require = ...
local env = setmetatable({ app = app }, { __index = _G })
env.require = function(name)
  if name == "app" then return app end
  return base_require(name)
end
return env
"#;

/// A tool registered with `app.tools.register`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolReg {
    pub(crate) spec: PluginToolSpec,
    pub(crate) handler: CallbackId,
}

/// What the functions of one `app` table received.
#[derive(Debug, Default)]
pub(crate) struct Collected {
    pub(crate) keymaps: Vec<KeymapSpec>,
    pub(crate) commands: Vec<CommandSpec>,
    pub(crate) tools: Vec<ToolReg>,
    pub(crate) hooks: Vec<HookSpec>,
    /// Servers from `app.mcp.add`, in call order.
    pub(crate) mcp_servers: Vec<McpServerSpec>,
    pub(crate) diagnostics: Vec<PluginDiagnostic>,
}

/// Where registrations of one phase come from.
#[derive(Debug)]
pub(crate) struct Origin {
    /// File, for diagnostics; also the chunk name (`@file`) of its code.
    pub(crate) file: String,
    /// Plugin (or config) name, for tools and hooks.
    pub(crate) plugin: String,
    /// Its `app.plugin` declaration and whether it was approved.
    pub(crate) grant: Arc<crate::caps::Grant>,
}

/// Where the functions of one `app` table record what they receive.
pub(crate) type Sink = Arc<Mutex<Collected>>;

/// One config file's environment and what it registered.
pub(crate) struct Phase {
    /// Environment to run the file in.
    pub(crate) env: Table,
    /// The file's `app` table.
    pub(crate) app: Table,
    sink: Sink,
    origin: Arc<Origin>,
}

impl Phase {
    /// Creates the environment for `file`, attributing tools and hooks to
    /// `plugin`.
    pub(crate) fn new(lua: &Lua, file: &str, plugin: &str) -> mlua::Result<Self> {
        let sink = Sink::default();
        let origin = Arc::new(Origin {
            file: file.to_owned(),
            plugin: plugin.to_owned(),
            grant: Arc::default(),
        });
        let app = app_table(lua, &sink, &origin)?;
        let base_require: Function = lua.globals().get("require")?;
        let env: Table = lua
            .load(NEW_ENV)
            .set_name("=sandbox")
            .call((app.clone(), base_require))?;
        Ok(Self {
            env,
            app,
            sink,
            origin,
        })
    }

    /// Who registers through this phase.
    pub(crate) fn origin(&self) -> &Arc<Origin> {
        &self.origin
    }

    /// Takes what was registered so far.
    pub(crate) fn take(&self) -> Collected {
        std::mem::take(&mut *lock(&self.sink))
    }
}

pub(crate) fn lock(sink: &Sink) -> std::sync::MutexGuard<'_, Collected> {
    sink.lock().unwrap_or_else(PoisonError::into_inner)
}

fn app_table(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let app = lua.create_table()?;
    app.set("opt", lua.create_table()?)?;
    let models = lua.create_table()?;
    models.set("roles", lua.create_table()?)?;
    models.set("aliases", lua.create_table()?)?;
    app.set("models", models)?;
    app.set("permissions", lua.create_table()?)?;
    app.set("mcp", crate::api::mcp::table(lua, sink, origin)?)?;
    app.set("plugins", lua.create_table()?)?;

    crate::api::keymap::install(lua, &app, sink, origin)?;
    let tools = lua.create_table()?;
    tools.set("register", crate::api::tools::register(lua, sink, origin)?)?;
    app.set("tools", tools)?;
    app.set("on", crate::api::hooks::on(lua, sink, origin)?)?;
    app.set("ui", crate::api::ui::table(lua, origin)?)?;
    app.set("modal", crate::api::modal::table(lua, origin)?)?;
    crate::caps::install(lua, &app, sink, origin)?;
    Ok(app)
}

/// Line of the Lua code that called the current Rust function.
fn caller_line(lua: &Lua) -> Option<u32> {
    lua.inspect_stack(1, |d| d.current_line())
        .flatten()
        .and_then(|l| u32::try_from(l).ok())
}

/// Records a diagnostic at the caller's line.
pub(crate) fn report(lua: &Lua, sink: &Sink, origin: &Origin, message: String) {
    lock(sink).diagnostics.push(PluginDiagnostic {
        source: origin.file.clone(),
        message,
        line: caller_line(lua),
    });
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::{HookKind, KeyAction};
    use serde_json::json;

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::sandbox;

    fn lua() -> Lua {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        lua
    }

    fn run(lua: &Lua, phase: &Phase, code: &str) {
        lua.load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .exec()
            .unwrap();
    }

    #[test]
    fn require_app_returns_the_phase_table() {
        let lua = lua();
        let phase = Phase::new(&lua, "init.lua", "config").unwrap();
        run(
            &lua,
            &phase,
            "local a = require('app'); assert(a == app); a.opt.theme = 'x'",
        );
        let theme: String = phase.app.get::<Table>("opt").unwrap().get("theme").unwrap();
        assert_eq!(theme, "x");
    }

    #[test]
    fn phases_do_not_share_app_or_globals() {
        let lua = lua();
        let one = Phase::new(&lua, "a.lua", "a").unwrap();
        let two = Phase::new(&lua, "b.lua", "b").unwrap();
        run(&lua, &one, "app.opt.leader = ','; mine = 1");
        run(
            &lua,
            &two,
            "assert(app.opt.leader == nil); assert(mine == nil)",
        );
    }

    #[test]
    fn keymaps_take_builtins_functions_and_the_old_mode_form() {
        let lua = lua();
        let phase = Phase::new(&lua, "init.lua", "config").unwrap();
        run(
            &lua,
            &phase,
            r#"app.keymap.set("<leader>r", "picker.files", { desc = "Files" })
               app.keymap.set("<C-x>", function() end)
               app.keymap.set({ "n", "i" }, "<C-y>", "help.open")
               app.keymap.set("n", "<C-z>", function() end, { desc = "Old" })"#,
        );
        let got = phase.take();
        assert!(got.diagnostics.is_empty(), "{:?}", got.diagnostics);
        assert_eq!(got.keymaps.len(), 4);
        assert_eq!(
            got.keymaps[0],
            KeymapSpec {
                lhs: "<leader>r".into(),
                action: KeyAction::Builtin("picker.files".into()),
                description: Some("Files".into()),
                plugin: "config".into(),
            }
        );
        assert_eq!(got.keymaps[1].description, None);
        assert!(matches!(got.keymaps[1].action, KeyAction::Callback(_)));
        assert_eq!(got.keymaps[2].lhs, "<C-y>");
        assert_eq!(got.keymaps[3].lhs, "<C-z>");
        assert_eq!(got.keymaps[3].description.as_deref(), Some("Old"));
    }

    #[test]
    fn commands_and_keymaps_take_a_desc_option() {
        let lua = lua();
        let phase = Phase::new(&lua, "plugins/jira.lua", "jira").unwrap();
        run(
            &lua,
            &phase,
            r#"app.commands.register("review", function() end, { desc = "Review the changes" })
               app.commands.register("plain", function() end)
               app.commands.register("bad", function() end, { desc = 3 })
               app.keymap.set("<C-x>", "help.open", "oops")"#,
        );
        let got = phase.take();
        let commands: Vec<_> = got
            .commands
            .iter()
            .map(|c| (c.name.as_str(), c.description.as_deref(), c.plugin.as_str()))
            .collect();
        assert_eq!(
            commands,
            [
                ("review", Some("Review the changes"), "jira"),
                ("plain", None, "jira")
            ]
        );
        assert!(got.keymaps.is_empty());
        let messages: Vec<_> = got.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(
            messages[0].contains("desc must be a string"),
            "{messages:?}"
        );
        assert!(
            messages[1].contains("options must be a table"),
            "{messages:?}"
        );
    }

    #[test]
    fn bad_arguments_become_diagnostics_with_the_line() {
        let lua = lua();
        let phase = Phase::new(&lua, "init.lua", "config").unwrap();
        run(
            &lua,
            &phase,
            "app.keymap.set(3, 'b')\n\napp.on('whenever', function() end)\napp.tools.register({ name = 't' })",
        );
        let got = phase.take();
        assert!(got.keymaps.is_empty() && got.hooks.is_empty() && got.tools.is_empty());
        let lines: Vec<_> = got.diagnostics.iter().map(|d| d.line).collect();
        assert_eq!(lines, [Some(1), Some(3), Some(4)]);
        assert!(
            got.diagnostics[0]
                .message
                .contains("key sequence must be a string")
        );
        assert_eq!(got.diagnostics[0].source, "init.lua");
    }

    #[test]
    fn commands_tools_and_hooks_are_recorded() {
        let lua = lua();
        let phase = Phase::new(&lua, "init.lua", "todo").unwrap();
        run(
            &lua,
            &phase,
            r#"app.commands.register("review", function(args) end)
               app.tools.register({
                 name = "todo_list",
                 description = "Muestra tareas",
                 schema = { type = "object", properties = { items = { type = "array" } } },
                 handler = function(input) return input end,
               })
               app.on("before_tool", function(ev) end)"#,
        );
        let got = phase.take();
        assert_eq!(got.commands.len(), 1);
        assert_eq!(got.commands[0].name, "review");
        assert_eq!(got.tools.len(), 1);
        let tool = &got.tools[0].spec;
        assert_eq!(tool.name, "todo_list");
        assert_eq!(tool.plugin, "todo");
        assert_eq!(
            tool.input_schema,
            json!({ "type": "object", "properties": { "items": { "type": "array" } } })
        );
        assert_eq!(got.hooks.len(), 1);
        assert_eq!(got.hooks[0].kind, HookKind::BeforeTool);
        let ids = [
            got.commands[0].callback,
            got.tools[0].handler,
            got.hooks[0].callback,
        ];
        for id in ids {
            assert!(callbacks::get(&lua, id).unwrap().is_some());
        }
    }
}
