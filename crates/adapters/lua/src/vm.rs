//! State owned by the VM thread and the calls it serves.

use std::collections::BTreeMap;
use std::sync::Arc;

use application::ports::plugin::{
    CallbackId, HookEvent, HookKind, HookOutcome, PluginCapabilities, PluginError, PluginOutput,
};
use mlua::Lua;
use serde_json::Value;

use crate::api::{hooks, keymap};
use crate::callbacks::{self, CallbackIds};
use crate::caps::{Context, Grant};
use crate::convert;
use crate::host::HostOptions;
use crate::sandbox::{self, Limiter};

/// A hook registered with `app.on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hook {
    pub(crate) kind: HookKind,
    pub(crate) callback: CallbackId,
}

/// The Lua state plus what the loaded config registered in it.
pub(crate) struct Vm {
    pub(crate) lua: Lua,
    pub(crate) limiter: Limiter,
    pub(crate) options: HostOptions,
    ids: CallbackIds,
    /// Handler of each plugin tool.
    pub(crate) tools: BTreeMap<String, CallbackId>,
    /// Hooks, in the order they run.
    pub(crate) hooks: Vec<Hook>,
    /// What the capability APIs need; survives [`Vm::reset`].
    ctx: Arc<Context>,
    /// The capability declarations of the loaded files.
    pub(crate) grants: Vec<Arc<Grant>>,
}

impl Vm {
    pub(crate) fn new(options: HostOptions, ids: CallbackIds) -> Result<Self, PluginError> {
        let ctx = Arc::new(Context::new(
            options.workspace.clone(),
            options.keyring,
            options.environment.clone(),
        ));
        let (lua, limiter) = new_lua(&options, &ids, &ctx)?;
        Ok(Self {
            lua,
            limiter,
            options,
            ids,
            tools: BTreeMap::new(),
            hooks: Vec::new(),
            ctx,
            grants: Vec::new(),
        })
    }

    /// Replaces the Lua state with a fresh one and forgets every tool and
    /// hook. Callback ids keep increasing, so old ids stop resolving.
    pub(crate) fn reset(&mut self) -> Result<(), PluginError> {
        (self.lua, self.limiter) = new_lua(&self.options, &self.ids, &self.ctx)?;
        self.tools.clear();
        self.hooks.clear();
        self.grants.clear();
        Ok(())
    }

    /// Activates the declarations in `granted` and deactivates the rest.
    pub(crate) fn grant(&mut self, granted: &[PluginCapabilities]) {
        for grant in &self.grants {
            grant.apply(granted);
        }
    }

    /// Calls the callback `id` with `args` and returns what it returned.
    pub(crate) fn call(&mut self, id: CallbackId, args: &Value) -> Result<Value, PluginError> {
        let lua = &self.lua;
        let f = callbacks::get(lua, id)
            .map_err(|e| convert::runtime_error(&e))?
            .ok_or_else(|| PluginError::NotFound(format!("callback {id}")))?;
        let run = || -> mlua::Result<Value> {
            let ret: mlua::Value = f.call(convert::to_lua(lua, args)?)?;
            convert::to_json(lua, ret)
        };
        self.limiter.guard(run)
    }

    /// Runs a keymap or command callback. When it called `app.cmd` or
    /// `app.send` the result is those [`PluginOutput::Actions`]; otherwise
    /// a table built by `app.ui.block` comes back as
    /// [`PluginOutput::Block`] and anything else as a value.
    pub(crate) fn invoke(
        &mut self,
        id: CallbackId,
        args: &Value,
    ) -> Result<PluginOutput, PluginError> {
        keymap::take_actions(&self.lua);
        let ret = self.call(id, args);
        let actions = keymap::take_actions(&self.lua);
        let ret = ret?;
        if actions.is_empty() {
            Ok(crate::api::ui::output(ret))
        } else {
            Ok(PluginOutput::Actions(actions))
        }
    }

    /// Runs the handler of the plugin tool `name`.
    pub(crate) fn call_tool(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Result<PluginOutput, PluginError> {
        let id = *self
            .tools
            .get(name)
            .ok_or_else(|| PluginError::NotFound(format!("tool {name}")))?;
        // A tool answers the model; app.cmd/app.send do nothing there.
        let ret = self.call(id, input);
        keymap::take_actions(&self.lua);
        ret.map(crate::api::ui::output)
    }

    /// Runs the hooks for `event` in order. A hook returns `nil` to go on,
    /// `{ deny = true, reason = "..." }` to stop the tool, or
    /// `{ input = {...} }` (before a tool) to rewrite its input.
    pub(crate) fn run_hooks(&mut self, mut event: HookEvent) -> Result<HookOutcome, PluginError> {
        let kind = event.kind();
        let ids: Vec<CallbackId> = self
            .hooks
            .iter()
            .filter(|h| h.kind == kind)
            .map(|h| h.callback)
            .collect();
        let mut replaced = None;
        for id in ids {
            let ev = serde_json::to_value(&event)
                .map_err(|e| PluginError::Runtime(format!("cannot encode hook event: {e}")))?;
            let ret = self.call(id, &ev)?;
            match hooks::outcome(&ret) {
                HookOutcome::Continue => {}
                deny @ HookOutcome::Deny { .. } => return Ok(deny),
                HookOutcome::ReplaceInput(input) => {
                    if let HookEvent::BeforeTool { input: current, .. } = &mut event {
                        current.clone_from(&input);
                        replaced = Some(input);
                    }
                }
            }
        }
        Ok(replaced.map_or(HookOutcome::Continue, HookOutcome::ReplaceInput))
    }
}

fn new_lua(
    options: &HostOptions,
    ids: &CallbackIds,
    ctx: &Arc<Context>,
) -> Result<(Lua, Limiter), PluginError> {
    let setup = || -> mlua::Result<(Lua, Limiter)> {
        let lua = sandbox::new_lua(options.memory_limit)?;
        lua.set_app_data(Arc::clone(ctx));
        callbacks::install(&lua, ids.clone())?;
        let limiter = Limiter::install(&lua, options.instruction_limit)?;
        Ok((lua, limiter))
    };
    setup().map_err(|e| convert::runtime_error(&e))
}

/// Test helper: evaluates `code`, which must return a function, and
/// registers it.
#[cfg(test)]
pub(crate) fn register_code(vm: &Vm, code: &str) -> CallbackId {
    let f: mlua::Function = vm.lua.load(code).eval().unwrap();
    callbacks::register(&vm.lua, f).unwrap()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn vm() -> Vm {
        Vm::new(HostOptions::default(), CallbackIds::default()).unwrap()
    }

    fn before(name: &str, input: Value) -> HookEvent {
        HookEvent::BeforeTool {
            name: name.into(),
            input,
        }
    }

    #[test]
    fn invokes_a_callback_with_json_args() {
        let mut vm = vm();
        let id = register_code(&vm, "return function(a) return { sum = a.x + a.y } end");
        let out = vm.invoke(id, &json!({ "x": 1, "y": 2 })).unwrap();
        assert_eq!(out, PluginOutput::Value(json!({ "sum": 3 })));
    }

    #[test]
    fn callbacks_that_ask_for_actions_return_them() {
        use application::ports::plugin::PluginAction;
        let mut vm = vm();
        let phase = crate::config::api::Phase::new(&vm.lua, "init.lua", "config").unwrap();
        let f: mlua::Function = vm
            .lua
            .load("return function(args) app.send('Revisa ' .. args, { role = 'plan' }) end")
            .set_environment(phase.env.clone())
            .eval()
            .unwrap();
        let id = callbacks::register(&vm.lua, f).unwrap();
        let out = vm.invoke(id, &json!("todo")).unwrap();
        assert_eq!(
            out,
            PluginOutput::Actions(vec![PluginAction::Send {
                text: "Revisa todo".into(),
                role: Some(domain::model::ModelRole::Plan),
            }])
        );
        vm.tools.insert("t".into(), id);
        let out = vm.call_tool("t", &json!("x")).unwrap();
        assert_eq!(
            out,
            PluginOutput::Value(Value::Null),
            "tools ignore actions"
        );
        assert_eq!(
            vm.invoke(id, &json!("y")).unwrap(),
            PluginOutput::Actions(vec![PluginAction::Send {
                text: "Revisa y".into(),
                role: Some(domain::model::ModelRole::Plan),
            }])
        );
    }

    #[test]
    fn unknown_callbacks_and_tools_are_not_found() {
        let mut vm = vm();
        assert!(matches!(
            vm.invoke(CallbackId(99), &Value::Null),
            Err(PluginError::NotFound(_))
        ));
        assert!(matches!(
            vm.call_tool("nope", &Value::Null),
            Err(PluginError::NotFound(_))
        ));
    }

    #[test]
    fn lua_errors_are_runtime_errors_and_the_vm_survives() {
        let mut vm = vm();
        let bad = register_code(&vm, "return function() error('boom') end");
        let good = register_code(&vm, "return function() return 1 end");
        let err = vm.invoke(bad, &Value::Null).unwrap_err();
        assert!(
            matches!(&err, PluginError::Runtime(m) if m.contains("boom")),
            "{err:?}"
        );
        assert_eq!(
            vm.invoke(good, &Value::Null).unwrap(),
            PluginOutput::Value(json!(1))
        );
    }

    #[test]
    fn a_runaway_callback_hits_the_instruction_limit() {
        let options = HostOptions {
            instruction_limit: 1_000_000,
            ..HostOptions::default()
        };
        let mut vm = Vm::new(options, CallbackIds::default()).unwrap();
        let spin = register_code(&vm, "return function() while true do end end");
        let ok = register_code(&vm, "return function() return 'alive' end");
        assert_eq!(
            vm.invoke(spin, &Value::Null),
            Err(PluginError::InstructionLimit)
        );
        assert_eq!(
            vm.invoke(ok, &Value::Null),
            Ok(PluginOutput::Value(json!("alive")))
        );
    }

    #[test]
    fn tools_run_their_handler() {
        let mut vm = vm();
        let id = register_code(&vm, "return function(input) return #input.items end");
        vm.tools.insert("count".into(), id);
        let out = vm
            .call_tool("count", &json!({ "items": [1, 2, 3] }))
            .unwrap();
        assert_eq!(out, PluginOutput::Value(json!(3)));
    }

    #[test]
    fn hooks_can_deny_and_stop_the_chain() {
        let mut vm = vm();
        let deny = register_code(
            &vm,
            r#"return function(ev)
                if ev.name == "run_command" and ev.input.command == "rm" then
                  return { deny = true, reason = "no rm" }
                end
              end"#,
        );
        let never = register_code(&vm, "return function() error('must not run') end");
        vm.hooks.push(Hook {
            kind: HookKind::BeforeTool,
            callback: deny,
        });
        vm.hooks.push(Hook {
            kind: HookKind::BeforeTool,
            callback: never,
        });

        let out = vm
            .run_hooks(before("run_command", json!({ "command": "rm" })))
            .unwrap();
        assert_eq!(
            out,
            HookOutcome::Deny {
                reason: Some("no rm".into())
            }
        );
    }

    #[test]
    fn replaced_input_feeds_the_next_hook() {
        let mut vm = vm();
        let first = register_code(
            &vm,
            "return function(ev) return { input = { n = ev.input.n + 1 } } end",
        );
        let second = register_code(
            &vm,
            "return function(ev) return { input = { n = ev.input.n * 10 } } end",
        );
        let after = register_code(&vm, "return function() error('wrong kind') end");
        vm.hooks.push(Hook {
            kind: HookKind::BeforeTool,
            callback: first,
        });
        vm.hooks.push(Hook {
            kind: HookKind::AfterTool,
            callback: after,
        });
        vm.hooks.push(Hook {
            kind: HookKind::BeforeTool,
            callback: second,
        });

        let out = vm.run_hooks(before("t", json!({ "n": 1 }))).unwrap();
        assert_eq!(out, HookOutcome::ReplaceInput(json!({ "n": 20 })));
    }

    #[test]
    fn hooks_returning_nothing_continue() {
        let mut vm = vm();
        let id = register_code(&vm, "return function() end");
        vm.hooks.push(Hook {
            kind: HookKind::AfterTool,
            callback: id,
        });
        let event = HookEvent::AfterTool {
            name: "t".into(),
            input: json!({}),
            output: json!("ok"),
        };
        assert_eq!(vm.run_hooks(event).unwrap(), HookOutcome::Continue);
    }

    #[test]
    fn reset_forgets_tools_hooks_and_callbacks() {
        let mut vm = vm();
        let id = register_code(&vm, "return function() return 1 end");
        vm.tools.insert("t".into(), id);
        vm.hooks.push(Hook {
            kind: HookKind::AfterTool,
            callback: id,
        });
        vm.reset().unwrap();
        assert!(vm.tools.is_empty() && vm.hooks.is_empty());
        assert!(matches!(
            vm.invoke(id, &Value::Null),
            Err(PluginError::NotFound(_))
        ));
    }
}
