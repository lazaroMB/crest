//! What the help modal shows about the [`App`]: the commands of the binary
//! and of the plugins, the effective keymap with the descriptions of
//! `init.lua`, the agent's tools, the plugins with their trust state, the
//! MCP servers, the models, the command-line flags and the paths in use.

use std::collections::BTreeMap;
use std::path::Path;

use application::keymap::{Action, KeymapEntry, parse_keys};
use application::ports::mcp::{McpServerState, McpServerStatus, parse_qualified_name};
use application::ports::plugin::{
    ConfigOrigin, HookKind, KeyAction, KeymapSpec, McpServerSpec, McpTransport,
};
use clap::CommandFactory as _;
use domain::model::{ModelRole, ResolveRequest};
use domain::permission::ToolEffect;
use ui::commands::CommandInfo;
use ui::help::{
    BUILTIN_SOURCE, CliRow, CommandRow, HelpSnapshot, McpRow, ModelRow, PathRow, PluginRow,
    ToolRow, ToolSource, key_rows,
};

use crate::args::Args;
use crate::plugin_trust::PluginTrust;
use crate::root::App;

/// The commands the binary runs ([`UiController::run_command`]).
///
/// [`UiController::run_command`]: crate::gui::UiController
pub const BIN_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "!",
        usage: "/! <command>",
        description: "Run a shell command in the workspace; its output is shown and goes to \
                      the model with the next prompt (also /!cmd)",
    },
    CommandInfo {
        name: "implement",
        usage: "/implement <spec>",
        description: "Run the plan of a Spec task by task, one commit per task, with gates you \
                      answer from the input",
    },
    CommandInfo {
        name: "implement",
        usage: "/implement resume",
        description: "Continue the last implementation where it stopped (also after an error)",
    },
    CommandInfo {
        name: "research",
        usage: "/research <problem>",
        description: "Start the research flow on a problem; it ends in a Spec",
    },
    CommandInfo {
        name: "research",
        usage: "/research",
        description: "Where the research flow is",
    },
    CommandInfo {
        name: "research",
        usage: "/research resume [n]",
        description: "Continue the last research flow (or number n of /research list), \
                      running a failed or interrupted stage again",
    },
    CommandInfo {
        name: "research",
        usage: "/research list",
        description: "The research flows of this workspace, newest first",
    },
    CommandInfo {
        name: "retry",
        usage: "/retry",
        description: "Run the failed stage of the research flow again",
    },
    CommandInfo {
        name: "stage",
        usage: "/stage",
        description: "Where the active flow is (research or implementation)",
    },
    CommandInfo {
        name: "back",
        usage: "/back <stage>",
        description: "Take the research flow back to a stage, by number or name",
    },
    CommandInfo {
        name: "skip",
        usage: "/skip",
        description: "Skip an optional stage of the research flow",
    },
    CommandInfo {
        name: "abort",
        usage: "/abort",
        description: "Stop the active flow for good (its journal is kept)",
    },
    CommandInfo {
        name: "open",
        usage: "/open @<path>",
        description: "Show a workspace file in a modal, 3D models included (type @ to complete the path)",
    },
    CommandInfo {
        name: "simulate",
        usage: "/simulate <spec>",
        description: "Simulate the solution of a Spec and open the 3D board with every scenario",
    },
    CommandInfo {
        name: "mcp",
        usage: "/mcp trust <name>",
        description: "Approve an MCP server of the workspace config (remembered for this \
                      workspace)",
    },
    CommandInfo {
        name: "plugin",
        usage: "/plugin list",
        description: "List the plugins that declare capabilities and whether they are approved",
    },
    CommandInfo {
        name: "plugin",
        usage: "/plugin trust <name>",
        description: "Approve the capabilities (network, shell) a plugin declares",
    },
];

/// Name the Lua host gives the global `init.lua`.
const GLOBAL_CONFIG: &str = "config";
/// Name the Lua host gives the workspace `init.lua`.
const WORKSPACE_CONFIG: &str = "workspace";

fn effect_name(effect: ToolEffect) -> &'static str {
    match effect {
        ToolEffect::Read => "read",
        ToolEffect::Write => "write",
        ToolEffect::Execute => "execute",
        ToolEffect::Network => "network",
    }
}

/// The effect of a built-in tool, which does not depend on its input
/// except for `run_command`.
fn builtin_effect(name: &str) -> &'static str {
    match name {
        "run_command" => "depends on input",
        "edit_file" | "write_file" | "simulate_solution" => "write",
        "list_files" | "find_file" | "grep" | "tree" | "read_file" | "task" | "show_chart"
        | "show_table" | "show_mermaid" | "show_markdown" => "read",
        "web_search" | "web_fetch" => "network (no approval)",
        _ => "",
    }
}

fn origin_name(origin: ConfigOrigin) -> &'static str {
    match origin {
        ConfigOrigin::Global => "global",
        ConfigOrigin::Workspace => "workspace",
    }
}

fn transport_text(transport: &McpTransport) -> String {
    match transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut line = format!("stdio: {command}");
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            line
        }
        McpTransport::Http { url, .. } => format!("http: {url}"),
    }
}

fn path_text(path: Option<&Path>) -> String {
    path.map_or_else(|| "none".to_owned(), |p| p.display().to_string())
}

/// The `init.lua` mapping behind `entry`, the last one that set its keys.
fn mapping_of<'a>(
    entry: &KeymapEntry<Action>,
    specs: &'a [KeymapSpec],
    leader: &str,
) -> Option<&'a KeymapSpec> {
    specs.iter().rev().find(|spec| {
        let same_action = match (&spec.action, &entry.action) {
            (KeyAction::Builtin(a), Action::Builtin(b)) => a == b,
            (KeyAction::Callback(a), Action::Callback(b)) => a.0 == *b,
            _ => false,
        };
        same_action && parse_keys(&spec.lhs, leader).is_ok_and(|keys| keys == entry.keys)
    })
}

/// The flags of `crest`, from clap.
#[must_use]
pub fn cli_rows() -> Vec<CliRow> {
    let command = Args::command();
    command
        .get_arguments()
        .map(|arg| {
            let value = arg
                .get_value_names()
                .map(|names| {
                    names
                        .iter()
                        .map(|n| format!("<{n}>"))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let mut flag = Vec::new();
            if let Some(short) = arg.get_short() {
                flag.push(format!("-{short}"));
            }
            if let Some(long) = arg.get_long() {
                flag.push(format!("--{long}"));
            }
            let flag = if flag.is_empty() {
                format!("[{}]", value.trim_matches(['<', '>']))
            } else if value.is_empty() || !arg.get_action().takes_values() {
                flag.join(", ")
            } else {
                format!("{} {value}", flag.join(", "))
            };
            let description = arg.get_help().map(ToString::to_string).unwrap_or_default();
            CliRow { flag, description }
        })
        .collect()
}

/// The row of plugin `name`, created empty.
fn entry<'a>(rows: &'a mut BTreeMap<String, PluginRow>, name: &str) -> &'a mut PluginRow {
    rows.entry(name.to_owned()).or_insert_with(|| PluginRow {
        name: name.to_owned(),
        ..PluginRow::default()
    })
}

/// The plugins of `app`: every name that registered something or declared
/// capabilities, the global config first and the workspace config last.
fn plugin_rows(app: &App, trust: &PluginTrust) -> Vec<PluginRow> {
    let mut rows: BTreeMap<String, PluginRow> = BTreeMap::new();
    for tool in &app.plugin_tools {
        entry(&mut rows, &tool.plugin)
            .adds
            .push(format!("tool {}", tool.name));
    }
    for command in &app.commands {
        let plugin = if command.plugin.is_empty() {
            GLOBAL_CONFIG
        } else {
            &command.plugin
        };
        entry(&mut rows, plugin)
            .adds
            .push(format!("command :{}", command.name));
    }
    for keymap in &app.keymaps {
        let plugin = if keymap.plugin.is_empty() {
            GLOBAL_CONFIG
        } else {
            &keymap.plugin
        };
        entry(&mut rows, plugin)
            .adds
            .push(format!("keymap {}", keymap.lhs));
    }
    for hook in &app.hooks {
        let kind = match hook.kind {
            HookKind::BeforeTool => "before_tool",
            HookKind::AfterTool => "after_tool",
        };
        entry(&mut rows, &hook.plugin)
            .adds
            .push(format!("hook {kind}"));
    }
    for decl in &app.plugin_capabilities {
        let granted = trust.is_granted(decl);
        let r = entry(&mut rows, &decl.plugin);
        r.path = decl.source.display().to_string();
        r.layer = origin_name(decl.origin).to_owned();
        r.capabilities
            .extend(decl.capabilities.iter().map(ToString::to_string));
        if granted {
            r.trust = "approved".into();
        } else {
            r.trust = "pending".into();
            r.approve = Some(format!("/plugin trust {}", decl.plugin));
        }
    }
    let sources = &app.config_sources;
    let plugins_dir = sources
        .global
        .as_deref()
        .and_then(Path::parent)
        .map(|dir| dir.join("plugins"));
    let mut out: Vec<PluginRow> = rows
        .into_values()
        .map(|mut r| {
            if r.path.is_empty() {
                let path = match r.name.as_str() {
                    GLOBAL_CONFIG => sources.global.clone(),
                    WORKSPACE_CONFIG => sources.workspace.clone(),
                    name => plugins_dir
                        .as_ref()
                        .map(|dir| dir.join(name).join("init.lua"))
                        .filter(|p| p.is_file()),
                };
                r.path = path.map(|p| p.display().to_string()).unwrap_or_default();
            }
            if r.layer.is_empty() {
                r.layer = if r.name == WORKSPACE_CONFIG {
                    "workspace"
                } else {
                    "global"
                }
                .to_owned();
            }
            if r.trust.is_empty() {
                r.trust = "no capabilities".into();
            }
            if !r.path.is_empty() {
                r.diagnostics = app
                    .config_errors
                    .iter()
                    .filter(|e| e.starts_with(&r.path))
                    .cloned()
                    .collect();
            }
            r
        })
        .collect();
    let rank = |r: &PluginRow| match r.name.as_str() {
        GLOBAL_CONFIG => 0,
        WORKSPACE_CONFIG => 2,
        _ => 1,
    };
    out.sort_by_key(|r| rank(r));
    out
}

/// The MCP servers declared in the config, with their state now.
fn mcp_rows(specs: &[McpServerSpec], statuses: &[McpServerStatus]) -> Vec<McpRow> {
    specs
        .iter()
        .map(|spec| {
            let status = statuses.iter().find(|s| s.name == spec.name);
            let (state, trust_command) = match status.map(|s| &s.state) {
                Some(McpServerState::Ready { tool_count }) => {
                    let s = if *tool_count == 1 { "" } else { "s" };
                    (format!("ready ({tool_count} tool{s})"), None)
                }
                Some(McpServerState::Failed { message }) => (format!("failed: {message}"), None),
                Some(McpServerState::Starting) => ("starting".to_owned(), None),
                Some(McpServerState::Stopped) => ("stopped".to_owned(), None),
                None if spec.origin == ConfigOrigin::Workspace => (
                    "held back: the workspace config is not trusted".to_owned(),
                    Some(format!("/mcp trust {}", spec.name)),
                ),
                None => ("not started".to_owned(), None),
            };
            McpRow {
                name: spec.name.clone(),
                transport: transport_text(&spec.transport),
                origin: origin_name(spec.origin).to_owned(),
                state,
                trust_command,
            }
        })
        .collect()
}

/// The help's snapshot of `app`; `statuses` are the MCP servers' states
/// now.
#[must_use]
pub fn snapshot(app: &App, trust: &PluginTrust, statuses: &[McpServerStatus]) -> HelpSnapshot {
    let mut commands: Vec<CommandRow> = BIN_COMMANDS
        .iter()
        .map(|info| CommandRow {
            name: info.name.into(),
            usage: info.usage.into(),
            description: info.description.into(),
            source: BUILTIN_SOURCE.into(),
        })
        .collect();
    commands.extend(app.commands.iter().map(|c| CommandRow {
        name: c.name.clone(),
        usage: format!("/{} [args]", c.name),
        description: c.description.clone().unwrap_or_default(),
        source: if c.plugin.is_empty() {
            GLOBAL_CONFIG.to_owned()
        } else {
            c.plugin.clone()
        },
    }));

    let (keymap, _) = crate::keys::keymap(&app.keymaps, &app.options);
    let leader = keymap.leader().to_owned();
    let keys = key_rows(&keymap, |entry| {
        mapping_of(entry, &app.keymaps, &leader).map(|spec| {
            let plugin = if spec.plugin.is_empty() {
                GLOBAL_CONFIG.to_owned()
            } else {
                spec.plugin.clone()
            };
            (spec.description.clone(), plugin)
        })
    });

    let registry = app.service.tools();
    let mut tools: Vec<ToolRow> = registry
        .definitions()
        .into_iter()
        .map(|def| {
            let plugin = app.plugin_tools.iter().find(|t| t.name == def.name);
            let (source, effect) = if let Some(spec) = plugin {
                (
                    ToolSource::Plugin(spec.plugin.clone()),
                    effect_name(spec.effect).to_owned(),
                )
            } else if let Some((server, _)) = parse_qualified_name(&def.name) {
                let effect = registry
                    .get(&def.name)
                    .map(|tool| effect_name(tool.effect(&serde_json::Value::Null)))
                    .unwrap_or_default();
                (ToolSource::Mcp(server.to_owned()), effect.to_owned())
            } else {
                (ToolSource::Builtin, builtin_effect(&def.name).to_owned())
            };
            ToolRow {
                schema: serde_json::to_string_pretty(&def.input_schema).unwrap_or_default(),
                name: def.name,
                description: def.description,
                source,
                effect,
            }
        })
        .collect();
    let rank = |source: &ToolSource| match source {
        ToolSource::Builtin => 0,
        ToolSource::Plugin(_) => 1,
        ToolSource::Mcp(_) => 2,
    };
    tools.sort_by_key(|t| rank(&t.source));

    let policy = app.model_policy();
    let mut models: Vec<ModelRow> = ModelRole::ALL
        .iter()
        .map(|role| ModelRow {
            kind: "role".into(),
            name: role.to_string(),
            value: policy
                .resolve(&ResolveRequest::new(*role))
                .spec
                .id
                .to_string(),
        })
        .collect();
    models.extend(policy.aliases().into_iter().map(|(alias, id)| ModelRow {
        kind: "alias".into(),
        name: alias,
        value: id.to_string(),
    }));
    if let Some(model) = &app.turn_options.turn_override {
        models.push(ModelRow {
            kind: "setting".into(),
            name: "--model".into(),
            value: model.clone(),
        });
    }

    let sources = &app.config_sources;
    let plugins_dir = sources
        .global
        .as_deref()
        .and_then(Path::parent)
        .map(|dir| dir.join("plugins"));
    let path = |label: &str, value: String| PathRow {
        label: label.into(),
        value,
    };
    let paths = vec![
        path("workspace", app.workspace.as_path().display().to_string()),
        path("global config", path_text(sources.global.as_deref())),
        path("workspace config", path_text(sources.workspace.as_deref())),
        path("plugins", path_text(plugins_dir.as_deref())),
        path("sessions and flows", path_text(app.db_path.as_deref())),
        path(
            "approved MCP servers",
            path_text(app.mcp_trust_path.as_deref()),
        ),
        path(
            "approved plugins",
            path_text(app.plugin_trust_path.as_deref()),
        ),
        path(
            "documentation",
            "docs/usage.md, docs/configuration.md, docs/plugins.md (crest repository)".into(),
        ),
    ];

    HelpSnapshot {
        commands,
        keys,
        tools,
        plugins: plugin_rows(app, trust),
        mcp: mcp_rows(&app.mcp_specs, statuses),
        models,
        cli: cli_rows(),
        paths,
        diagnostics: app
            .config_errors
            .iter()
            .chain(&app.warnings)
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_rows_come_from_clap() {
        let rows = cli_rows();
        let find = |flag: &str| rows.iter().find(|r| r.flag == flag);
        assert!(find("--model <ALIAS|ID>").is_some(), "{rows:?}");
        assert!(find("[DIR]").is_some(), "{rows:?}");
        let new = find("--new").unwrap();
        assert!(new.description.contains("new session"), "{new:?}");
        assert!(rows.iter().any(|r| r.flag.starts_with("--trust-plugin")));
    }

    #[test]
    fn builtin_effects_are_known_and_run_command_depends_on_input() {
        assert_eq!(builtin_effect("run_command"), "depends on input");
        assert_eq!(builtin_effect("edit_file"), "write");
        assert_eq!(builtin_effect("grep"), "read");
        assert_eq!(builtin_effect("web_fetch"), "network (no approval)");
        assert_eq!(builtin_effect("nope"), "");
    }

    #[test]
    fn held_back_servers_say_how_to_trust_them() {
        let mut ws = McpServerSpec::new(
            "git",
            McpTransport::Stdio {
                command: "mcp-git".into(),
                args: vec!["--x".into()],
                env: BTreeMap::new(),
            },
        );
        ws.origin = ConfigOrigin::Workspace;
        let ready = McpServerSpec::new(
            "echo",
            McpTransport::Http {
                url: "http://localhost:1".into(),
                headers: BTreeMap::new(),
            },
        );
        let statuses = [McpServerStatus {
            name: "echo".into(),
            state: McpServerState::Ready { tool_count: 2 },
        }];
        let rows = mcp_rows(&[ws, ready], &statuses);
        assert_eq!(rows[0].trust_command.as_deref(), Some("/mcp trust git"));
        assert_eq!(rows[0].transport, "stdio: mcp-git --x");
        assert_eq!(rows[0].origin, "workspace");
        assert_eq!(rows[1].state, "ready (2 tools)");
        assert_eq!(rows[1].transport, "http: http://localhost:1");
    }
}
