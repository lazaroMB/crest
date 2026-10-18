//! Loading `init.lua` files and plugins into a [`LoadedConfig`].
//!
//! The global config (`$XDG_CONFIG_HOME/crest/init.lua`, by default
//! `~/.config/crest/init.lua`) runs first, then the plugins in
//! `<config dir>/plugins/<name>/init.lua` in alphabetical order, then the
//! workspace config (`<workspace>/.crest/init.lua`). Each file runs in its
//! own environment with its own `app` table.
//!
//! [`LoadedConfig`]: application::ports::plugin::LoadedConfig

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use application::ports::plugin::{ConfigSources, LoadedConfig, PluginDiagnostic, PluginError};
use mlua::prelude::LuaChunkMode;

use crate::vm::Vm;
use api::Phase;
use merge::Layer;

pub(crate) mod api;
mod diag;
mod merge;
mod parse;

/// Directory under the config home that holds crest's files.
const APP_DIR: &str = "crest";
/// Name of every config file.
const INIT_FILE: &str = "init.lua";
/// Workspace directory holding the workspace config.
const WORKSPACE_DIR: &str = ".crest";
/// Directory next to the global config holding the plugins.
const PLUGINS_DIR: &str = "plugins";
/// Name tools and hooks of the global config are attributed to.
const GLOBAL_NAME: &str = "config";
/// Name tools and hooks of the workspace config are attributed to.
const WORKSPACE_NAME: &str = "workspace";

/// Config files for `workspace`, following `XDG_CONFIG_HOME` and falling
/// back to `~/.config` on every platform.
#[must_use]
pub fn default_sources(workspace: &Path) -> ConfigSources {
    sources_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        dirs::home_dir(),
        workspace,
    )
}

fn sources_from(xdg: Option<OsString>, home: Option<PathBuf>, workspace: &Path) -> ConfigSources {
    let config_home = xdg
        .map(PathBuf::from)
        // The XDG spec says relative values must be ignored.
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")));
    ConfigSources {
        global: config_home.map(|dir| dir.join(APP_DIR).join(INIT_FILE)),
        workspace: Some(workspace.join(WORKSPACE_DIR).join(INIT_FILE)),
    }
}

/// Loads `sources` into a fresh VM: the global config, the plugins, then the
/// workspace config. Problems in single files end up in
/// [`LoadedConfig::errors`]; a file that fails contributes nothing.
pub(crate) fn load(vm: &mut Vm, sources: &ConfigSources) -> Result<LoadedConfig, PluginError> {
    vm.reset()?;
    let mut errors = Vec::new();

    let global = sources
        .global
        .as_deref()
        .and_then(|path| run_file(vm, path, GLOBAL_NAME, false, &mut errors));

    let mut plugins = Vec::new();
    if let Some(config_dir) = sources.global.as_deref().and_then(Path::parent) {
        let listed = global
            .as_ref()
            .map(|g| g.declared.plugins.clone())
            .unwrap_or_default();
        for (name, init) in plugin_files(config_dir, &listed, &mut errors) {
            plugins.extend(run_file(vm, &init, &name, true, &mut errors));
        }
    }

    let workspace = sources
        .workspace
        .as_deref()
        .and_then(|path| run_file(vm, path, WORKSPACE_NAME, false, &mut errors));
    if let (Some(ws), Some(path)) = (&workspace, &sources.workspace)
        && !ws.declared.plugins.is_empty()
    {
        errors.push(PluginDiagnostic {
            source: path.display().to_string(),
            message:
                "app.plugins is ignored in a workspace config; list plugins in the global config"
                    .into(),
            line: None,
        });
    }

    let merged = merge::merge(global, plugins, workspace);
    vm.tools = merged.tools;
    vm.hooks = merged.hooks;
    let mut config = merged.config;
    config.errors = errors;
    Ok(config)
}

/// Runs one file in its own phase. Returns `None` when the file does not
/// exist (and `required` is false) or fails.
fn run_file(
    vm: &Vm,
    path: &Path,
    name: &str,
    required: bool,
    errors: &mut Vec<PluginDiagnostic>,
) -> Option<Layer> {
    let file = path.display().to_string();
    let diagnostic = |message: String| PluginDiagnostic {
        source: file.clone(),
        message,
        line: None,
    };
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => return None,
        Err(e) => {
            errors.push(diagnostic(format!("cannot read the file: {e}")));
            return None;
        }
    };
    let phase = match Phase::new(&vm.lua, &file, name) {
        Ok(phase) => phase,
        Err(e) => {
            errors.push(diagnostic(format!("cannot prepare the environment: {e}")));
            return None;
        }
    };
    let ran = vm.limiter.run(|| {
        vm.lua
            .load(source.as_str())
            .set_name(format!("@{file}"))
            .set_mode(LuaChunkMode::Text)
            .set_environment(phase.env.clone())
            .exec()
    });
    let mut collected = phase.take();
    errors.append(&mut collected.diagnostics);
    if let Err(failure) = ran {
        errors.push(diag::from_failure(&file, &failure));
        return None;
    }
    let (declared, mut problems) = parse::read(&vm.lua, &phase.app, &file);
    errors.append(&mut problems);
    Some(Layer {
        declared,
        collected,
    })
}

/// Plugin `init.lua` files: every `<config dir>/plugins/<name>/init.lua` in
/// alphabetical order, then the directories listed in `app.plugins` that
/// were not already found. Listed paths may start with `~/` or be relative
/// to the config directory.
fn plugin_files(
    config_dir: &Path,
    listed: &[String],
    errors: &mut Vec<PluginDiagnostic>,
) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(config_dir.join(PLUGINS_DIR))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| dir.join(INIT_FILE).is_file())
        .filter_map(|dir| {
            let name = dir.file_name()?.to_string_lossy().into_owned();
            Some((name, dir.join(INIT_FILE)))
        })
        .collect();
    found.sort();

    for entry in listed {
        let dir = match entry.strip_prefix("~/") {
            Some(rest) => match dirs::home_dir() {
                Some(home) => home.join(rest),
                None => {
                    errors.push(PluginDiagnostic {
                        source: config_dir.join(INIT_FILE).display().to_string(),
                        message: format!("app.plugins: cannot expand `{entry}`: no home directory"),
                        line: None,
                    });
                    continue;
                }
            },
            None => config_dir.join(entry),
        };
        let init = dir.join(INIT_FILE);
        let same = |p: &PathBuf| {
            p == &init
                || matches!((p.canonicalize(), init.canonicalize()), (Ok(a), Ok(b)) if a == b)
        };
        if found.iter().any(|(_, p)| same(p)) {
            continue;
        }
        let name = dir
            .file_name()
            .map_or_else(|| entry.clone(), |n| n.to_string_lossy().into_owned());
        found.push((name, init));
    }
    found
}

#[cfg(test)]
mod tests {
    use application::cost::CostTracker;
    use application::ports::plugin::{
        HookEvent, HookKind, HookOutcome, KeyAction, KeyMode, KeymapSpec, PluginOutput,
    };
    use domain::conversation::Usage;
    use domain::model::{ModelId, ModelRole, ResolveRequest, ThinkingConfig};
    use serde_json::{Value, json};

    use super::*;
    use crate::callbacks::CallbackIds;
    use crate::host::HostOptions;

    /// A temporary config home and workspace.
    struct Fixture {
        _dir: tempfile::TempDir,
        config_dir: PathBuf,
        workspace: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config_dir = dir.path().join("config").join(APP_DIR);
            let workspace = dir.path().join("ws");
            std::fs::create_dir_all(&config_dir).unwrap();
            std::fs::create_dir_all(workspace.join(WORKSPACE_DIR)).unwrap();
            Self {
                _dir: dir,
                config_dir,
                workspace,
            }
        }

        fn global(&self, code: &str) -> &Self {
            std::fs::write(self.config_dir.join(INIT_FILE), code).unwrap();
            self
        }

        fn workspace(&self, code: &str) -> &Self {
            std::fs::write(self.workspace.join(WORKSPACE_DIR).join(INIT_FILE), code).unwrap();
            self
        }

        fn plugin(&self, name: &str, code: &str) -> &Self {
            let dir = self.config_dir.join(PLUGINS_DIR).join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(INIT_FILE), code).unwrap();
            self
        }

        fn sources(&self) -> ConfigSources {
            ConfigSources {
                global: Some(self.config_dir.join(INIT_FILE)),
                workspace: Some(self.workspace.join(WORKSPACE_DIR).join(INIT_FILE)),
            }
        }

        fn load(&self) -> (Vm, LoadedConfig) {
            let options = HostOptions {
                instruction_limit: 1_000_000,
                ..HostOptions::default()
            };
            let mut vm = Vm::new(options, CallbackIds::default()).unwrap();
            let config = load(&mut vm, &self.sources()).unwrap();
            (vm, config)
        }
    }

    const PLAN_EXAMPLE: &str = r#"
local app = require("app")

app.opt.leader = " "
app.opt.theme = "tokyonight"

app.keymap.set("n", "<leader>r", "session.resume")
app.keymap.set("n", "<leader>x", function() app.cmd("clear") end)

app.commands.register("review", function(args)
  app.send("Revisa los cambios sin commitear y lista riesgos", { role = "plan" })
end)

app.tools.register({
  name = "todo_list",
  description = "Muestra tareas pendientes del proyecto",
  schema = { type = "object", properties = { items = { type = "array" } } },
  handler = function(input)
    return app.ui.block("todo", app.ui.list(input.items))
  end,
})

app.on("before_tool", function(ev)
  if ev.name == "run_command" and ev.input.command == "rm" then return { deny = true } end
end)
"#;

    #[test]
    fn loads_the_lua_api_example_of_the_plan() {
        let fx = Fixture::new();
        fx.global(PLAN_EXAMPLE);
        let (mut vm, config) = fx.load();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        assert_eq!(config.options.leader.as_deref(), Some(" "));
        assert_eq!(config.options.theme.as_deref(), Some("tokyonight"));
        assert_eq!(config.keymaps.len(), 2);
        assert_eq!(
            config.keymaps[0],
            KeymapSpec {
                mode: KeyMode::Normal,
                lhs: "<leader>r".into(),
                action: KeyAction::Builtin("session.resume".into()),
            }
        );
        assert!(matches!(config.keymaps[1].action, KeyAction::Callback(_)));
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].name, "review");
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "todo_list");
        assert_eq!(config.tools[0].plugin, GLOBAL_NAME);
        assert_eq!(config.hooks.len(), 1);
        assert_eq!(config.hooks[0].kind, HookKind::BeforeTool);

        let rm = HookEvent::BeforeTool {
            name: "run_command".into(),
            input: json!({ "command": "rm" }),
        };
        assert_eq!(
            vm.run_hooks(rm).unwrap(),
            HookOutcome::Deny { reason: None }
        );
        let ls = HookEvent::BeforeTool {
            name: "run_command".into(),
            input: json!({ "command": "ls" }),
        };
        assert_eq!(vm.run_hooks(ls).unwrap(), HookOutcome::Continue);
    }

    #[test]
    fn workspace_overrides_global_field_by_field() {
        let fx = Fixture::new();
        fx.global(
            r#"app.opt.leader = " "
               app.opt.theme = "tokyonight"
               app.opt.timeoutlen = 500
               app.models.roles = {
                 main = { id = "claude-sonnet-5-5", max_tokens = 16000,
                          thinking = { budget = 8000 }, fallbacks = { "claude-haiku-5-5" } },
                 plan = { id = "opus" },
               }
               app.models.aliases = { opus = "claude-opus-5-5", fast = "claude-haiku-5-5" }
               app.keymap.set("n", "<leader>r", "session.resume")
               app.keymap.set("n", "<leader>m", "model.pick")
               app.commands.register("review", function() return "global" end)
               app.tools.register({ name = "t", handler = function() return "global" end })"#,
        )
        .workspace(
            r#"app.opt.theme = "light"
               app.models.roles = { main = { max_tokens = 4000 } }
               app.models.aliases = { fast = "claude-sonnet-5-5" }
               app.keymap.set("n", "<leader>r", "session.new")
               app.commands.register("review", function() return "workspace" end)"#,
        );
        let (mut vm, config) = fx.load();
        assert!(config.errors.is_empty(), "{:?}", config.errors);

        let o = &config.options;
        assert_eq!(o.leader.as_deref(), Some(" "));
        assert_eq!(o.theme.as_deref(), Some("light"));
        assert_eq!(o.timeoutlen_ms, Some(500));

        let policy = config.model_policy();
        let main = policy.resolve(&ResolveRequest::new(ModelRole::Main));
        assert_eq!(main.spec.id, ModelId::new("claude-sonnet-5-5"));
        assert_eq!(main.spec.max_tokens, 4000);
        assert_eq!(
            main.spec.thinking,
            ThinkingConfig::Enabled {
                budget_tokens: 8000
            }
        );
        assert_eq!(main.spec.fallbacks, [ModelId::new("claude-haiku-5-5")]);
        let plan = policy.resolve(&ResolveRequest::new(ModelRole::Plan));
        assert_eq!(plan.spec.id, ModelId::new("claude-opus-5-5"));
        assert_eq!(policy.expand("fast"), ModelId::new("claude-sonnet-5-5"));

        let keys: Vec<_> = config
            .keymaps
            .iter()
            .map(|k| (k.lhs.as_str(), k.action.clone()))
            .collect();
        assert_eq!(
            keys,
            [
                ("<leader>m", KeyAction::Builtin("model.pick".into())),
                ("<leader>r", KeyAction::Builtin("session.new".into())),
            ]
        );
        assert_eq!(config.commands.len(), 1);
        let review = vm
            .invoke(config.commands[0].callback, &Value::Null)
            .unwrap();
        assert_eq!(review, PluginOutput::Value(json!("workspace")));
        assert_eq!(
            vm.call_tool("t", &Value::Null).unwrap(),
            PluginOutput::Value(json!("global"))
        );
    }

    #[test]
    fn pricing_and_spending_cap_reach_the_cost_tracker() {
        let fx = Fixture::new();
        fx.global(
            r#"app.opt.pricing = {
                 ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
               }
               app.opt.max_session_cost = 5"#,
        )
        .workspace("app.opt.max_session_cost = 1.5");
        let (_, config) = fx.load();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        let tracker = CostTracker::from_options(&config.options).unwrap();
        assert_eq!(tracker.cap(), Some(1.5));
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Usage::default()
        };
        let cost = tracker
            .pricing()
            .cost(&ModelId::new("claude-sonnet-5-5"), &usage);
        assert_eq!(cost, Some(18.0));
    }

    #[test]
    fn a_syntax_error_is_reported_and_the_rest_still_loads() {
        let fx = Fixture::new();
        fx.global("app.opt.theme = 'dark'\n\napp.opt.leader = = ','")
            .workspace("app.opt.timeoutlen = 200");
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1, "{:?}", config.errors);
        let err = &config.errors[0];
        assert_eq!(
            err.source,
            fx.config_dir.join(INIT_FILE).display().to_string()
        );
        assert_eq!(err.line, Some(3));
        assert!(err.message.starts_with("syntax error"), "{}", err.message);
        // The broken file contributes nothing; the workspace still applies.
        assert_eq!(config.options.theme, None);
        assert_eq!(config.options.timeoutlen_ms, Some(200));
    }

    #[test]
    fn a_runtime_error_drops_that_file_with_its_line() {
        let fx = Fixture::new();
        fx.global("app.opt.theme = 'dark'\nlocal x = nil\nprint(x.y)");
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1);
        assert_eq!(config.errors[0].line, Some(3));
        assert_eq!(config.options, Default::default());
    }

    #[test]
    fn plugins_load_alphabetically_between_global_and_workspace() {
        let fx = Fixture::new();
        fx.global("app.on('after_tool', function() end)")
            .plugin(
                "zeta",
                "app.on('after_tool', function() end); app.opt.theme = 'zeta'",
            )
            .plugin(
                "alpha",
                "app.on('after_tool', function() end); app.opt.theme = 'alpha'",
            )
            .workspace("app.on('after_tool', function() end)");
        let (_, config) = fx.load();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        let order: Vec<_> = config.hooks.iter().map(|h| h.plugin.as_str()).collect();
        assert_eq!(order, [GLOBAL_NAME, "alpha", "zeta", WORKSPACE_NAME]);
        assert_eq!(config.options.theme.as_deref(), Some("zeta"));
    }

    #[test]
    fn the_global_config_wins_over_its_plugins() {
        let fx = Fixture::new();
        fx.global("app.opt.theme = 'mine'")
            .plugin("p", "app.opt.theme = 'plugin'; app.opt.leader = ','");
        let (_, config) = fx.load();
        assert_eq!(config.options.theme.as_deref(), Some("mine"));
        assert_eq!(config.options.leader.as_deref(), Some(","));
    }

    #[test]
    fn a_runaway_plugin_is_cut_and_the_others_load() {
        let fx = Fixture::new();
        fx.plugin("hang", "while true do end")
            .plugin("ok", "app.opt.theme = 'fine'");
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1);
        assert!(config.errors[0].source.contains("hang"));
        assert_eq!(config.errors[0].message, "instruction limit exceeded");
        assert_eq!(config.options.theme.as_deref(), Some("fine"));
    }

    #[test]
    fn listed_plugins_load_after_the_discovered_ones() {
        let fx = Fixture::new();
        let extra = fx.config_dir.join("elsewhere").join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(
            extra.join(INIT_FILE),
            "app.on('after_tool', function() end)",
        )
        .unwrap();
        fx.global("app.plugins = { 'elsewhere/extra', 'plugins/b' }")
            .plugin("b", "app.on('after_tool', function() end)");
        let (_, config) = fx.load();
        assert!(config.errors.is_empty(), "{:?}", config.errors);
        let order: Vec<_> = config.hooks.iter().map(|h| h.plugin.as_str()).collect();
        assert_eq!(order, ["b", "extra"]);
    }

    #[test]
    fn a_missing_listed_plugin_is_reported() {
        let fx = Fixture::new();
        fx.global("app.plugins = { 'nowhere' }");
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1);
        assert!(config.errors[0].source.contains("nowhere"));
    }

    #[test]
    fn the_workspace_cannot_load_plugins() {
        let fx = Fixture::new();
        fx.workspace("app.plugins = { '/tmp/evil' }");
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1);
        assert!(config.errors[0].message.contains("ignored"));
        assert!(config.hooks.is_empty());
    }

    #[test]
    fn missing_files_give_the_default_config() {
        let fx = Fixture::new();
        let (_, config) = fx.load();
        assert_eq!(config, LoadedConfig::default());
    }

    #[test]
    fn reloading_forgets_the_previous_config() {
        let fx = Fixture::new();
        fx.global("app.tools.register({ name = 'old', handler = function() end })");
        let (mut vm, first) = fx.load();
        assert_eq!(first.tools.len(), 1);
        fx.global("app.opt.theme = 'new'");
        let second = load(&mut vm, &fx.sources()).unwrap();
        assert!(second.tools.is_empty());
        assert!(matches!(
            vm.call_tool("old", &Value::Null),
            Err(PluginError::NotFound(_))
        ));
    }

    #[test]
    fn config_files_cannot_be_bytecode() {
        let fx = Fixture::new();
        std::fs::write(fx.config_dir.join(INIT_FILE), b"\x1bLua\x54\x00garbage").unwrap();
        let (_, config) = fx.load();
        assert_eq!(config.errors.len(), 1);
        assert!(
            config.errors[0].message.contains("binary"),
            "{:?}",
            config.errors
        );
    }

    #[test]
    fn uses_xdg_config_home_when_set() {
        let s = sources_from(
            Some("/xdg".into()),
            Some("/home/u".into()),
            Path::new("/ws"),
        );
        assert_eq!(s.global, Some(PathBuf::from("/xdg/crest/init.lua")));
        assert_eq!(s.workspace, Some(PathBuf::from("/ws/.crest/init.lua")));
    }

    #[test]
    fn falls_back_to_dot_config_in_home() {
        let s = sources_from(None, Some("/home/u".into()), Path::new("/ws"));
        assert_eq!(
            s.global,
            Some(PathBuf::from("/home/u/.config/crest/init.lua"))
        );
    }

    #[test]
    fn ignores_a_relative_xdg_config_home() {
        let s = sources_from(Some("rel".into()), Some("/home/u".into()), Path::new("/ws"));
        assert_eq!(
            s.global,
            Some(PathBuf::from("/home/u/.config/crest/init.lua"))
        );
    }

    #[test]
    fn no_home_means_no_global_config() {
        let s = sources_from(None, None, Path::new("/ws"));
        assert_eq!(s.global, None);
        assert!(s.workspace.is_some());
    }
}
