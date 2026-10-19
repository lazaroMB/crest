//! Merging the layers (plugins, global config, workspace config) into one
//! [`LoadedConfig`].
//!
//! Precedence, lowest first: plugins in load order, the global config, the
//! workspace config. The user's own config thus wins over the plugins it
//! loads, and the workspace wins over both:
//!
//! - options key by key (`extra` too);
//! - model roles field by field, kept per layer (plugins fold into the
//!   global layer) so the policy can tell where a spec came from;
//! - keymaps by `lhs`, commands, tools and MCP servers by name;
//!   each server records whether the workspace declared it, so the app can
//!   hold back servers that come with a cloned repository;
//! - hooks are all kept, in the order the files ran (global, plugins,
//!   workspace);
//! - sensitive paths: the global layer may replace the built-in defaults,
//!   but the workspace can only add patterns, so a cloned repository cannot
//!   switch the protection off.
//! - privacy (`app.opt.privacy`): likewise, the workspace can only make it
//!   stricter (see [`application::privacy::tighten`]).

use std::collections::BTreeMap;

use application::ports::plugin::{AppOptions, CallbackId, ConfigOrigin, HookSpec, LoadedConfig};
use application::privacy::{self, PRIVACY_OPTION};
use domain::model::RoleConfig;
use domain::workspace::DEFAULT_SENSITIVE_PATTERNS;

use super::api::Collected;
use super::parse::Declared;
use crate::vm::Hook;

/// What one file contributed.
#[derive(Debug, Default)]
pub(crate) struct Layer {
    pub(crate) declared: Declared,
    pub(crate) collected: Collected,
}

/// The merged config plus what the VM needs to dispatch tools and hooks.
#[derive(Debug, Default)]
pub(crate) struct Merged {
    pub(crate) config: LoadedConfig,
    pub(crate) tools: BTreeMap<String, CallbackId>,
    pub(crate) hooks: Vec<Hook>,
}

/// Replaces the item with the same key, moving it to the end, or appends.
fn upsert<T, K: PartialEq>(list: &mut Vec<T>, item: T, key: impl Fn(&T) -> K) {
    let k = key(&item);
    list.retain(|x| key(x) != k);
    list.push(item);
}

fn overlay_options(low: &mut AppOptions, high: AppOptions) {
    if high.leader.is_some() {
        low.leader = high.leader;
    }
    if high.theme.is_some() {
        low.theme = high.theme;
    }
    if high.timeoutlen_ms.is_some() {
        low.timeoutlen_ms = high.timeoutlen_ms;
    }
    low.extra.extend(high.extra);
}

/// Merges the layers; `plugins` are in load order.
pub(crate) fn merge(
    global: Option<Layer>,
    plugins: Vec<Layer>,
    workspace: Option<Layer>,
) -> Merged {
    let mut config = LoadedConfig::default();
    let mut tools = Vec::new();

    // Hooks run in load order, not in precedence order.
    let mut hooks: Vec<HookSpec> = Vec::new();
    for layer in global.iter().chain(&plugins).chain(&workspace) {
        hooks.extend(layer.collected.hooks.iter().cloned());
    }

    let mut global_roles = RoleConfig::default();
    let mut base_sensitive: Option<Vec<String>> = None;
    let base = plugins.into_iter().chain(global);
    let workspace = workspace.unwrap_or_default();
    let ranked = base
        .map(|layer| (false, layer))
        .chain(std::iter::once((true, workspace)));
    for (
        is_workspace,
        Layer {
            declared,
            collected,
        },
    ) in ranked
    {
        let mut options = declared.options;
        if is_workspace && let Some(ws) = options.extra.remove(PRIVACY_OPTION) {
            let base = config.options.extra.get(PRIVACY_OPTION);
            let tightened = privacy::tighten(base, &ws);
            config
                .options
                .extra
                .insert(PRIVACY_OPTION.to_owned(), tightened);
        }
        overlay_options(&mut config.options, options);
        if is_workspace {
            config.workspace_roles = declared.roles;
            if let Some(extra) = declared.sensitive_paths {
                let mut paths = base_sensitive.clone().unwrap_or_else(|| {
                    DEFAULT_SENSITIVE_PATTERNS
                        .iter()
                        .map(|p| (*p).to_owned())
                        .collect()
                });
                for p in extra {
                    if !paths.contains(&p) {
                        paths.push(p);
                    }
                }
                base_sensitive = Some(paths);
            }
        } else {
            global_roles = global_roles.overlay(&declared.roles);
            if declared.sensitive_paths.is_some() {
                base_sensitive = declared.sensitive_paths;
            }
        }
        for km in collected.keymaps {
            upsert(&mut config.keymaps, km, |k| k.lhs.clone());
        }
        for cmd in collected.commands {
            upsert(&mut config.commands, cmd, |c| c.name.clone());
        }
        for tool in collected.tools {
            upsert(&mut tools, tool, |t| t.spec.name.clone());
        }
        // `app.mcp.add` runs while the file runs, the table form is read
        // after it, but an explicit `add` is the more deliberate one.
        let origin = if is_workspace {
            ConfigOrigin::Workspace
        } else {
            ConfigOrigin::Global
        };
        for mut server in declared
            .mcp_servers
            .into_iter()
            .chain(collected.mcp_servers)
        {
            server.origin = origin;
            upsert(&mut config.mcp_servers, server, |s| s.name.clone());
        }
    }
    config.global_roles = global_roles;
    config.sensitive_paths = base_sensitive;

    let handlers = tools
        .iter()
        .map(|t| (t.spec.name.clone(), t.handler))
        .collect();
    config.tools = tools.into_iter().map(|t| t.spec).collect();
    let vm_hooks = hooks
        .iter()
        .map(|h| Hook {
            kind: h.kind,
            callback: h.callback,
        })
        .collect();
    config.hooks = hooks;
    Merged {
        config,
        tools: handlers,
        hooks: vm_hooks,
    }
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::{
        CommandSpec, HookKind, KeyAction, KeymapSpec, McpServerSpec, McpTransport, PluginToolSpec,
    };
    use domain::model::{ModelId, ModelRole, ModelSpecPatch};
    use serde_json::json;

    use super::*;
    use crate::config::api::ToolReg;

    fn keymap(lhs: &str, action: &str) -> KeymapSpec {
        KeymapSpec {
            lhs: lhs.into(),
            action: KeyAction::Builtin(action.into()),
            description: None,
            plugin: "config".into(),
        }
    }

    fn tool(name: &str, plugin: &str, handler: u64) -> ToolReg {
        ToolReg {
            spec: PluginToolSpec {
                name: name.into(),
                description: String::new(),
                input_schema: json!({ "type": "object" }),
                plugin: plugin.into(),
                effect: domain::permission::ToolEffect::Execute,
            },
            handler: CallbackId(handler),
        }
    }

    fn hook(plugin: &str, id: u64) -> HookSpec {
        HookSpec {
            kind: HookKind::BeforeTool,
            callback: CallbackId(id),
            plugin: plugin.into(),
        }
    }

    fn layer(f: impl FnOnce(&mut Declared, &mut Collected)) -> Layer {
        let mut l = Layer::default();
        f(&mut l.declared, &mut l.collected);
        l
    }

    #[test]
    fn workspace_options_override_key_by_key() {
        let global = layer(|d, _| {
            d.options.leader = Some(" ".into());
            d.options.theme = Some("tokyonight".into());
            d.options.extra.insert("a".into(), json!(1));
            d.options.extra.insert("b".into(), json!(1));
        });
        let ws = layer(|d, _| {
            d.options.theme = Some("light".into());
            d.options.extra.insert("b".into(), json!(2));
        });
        let o = merge(Some(global), Vec::new(), Some(ws)).config.options;
        assert_eq!(o.leader.as_deref(), Some(" "));
        assert_eq!(o.theme.as_deref(), Some("light"));
        assert_eq!(o.extra.get("a"), Some(&json!(1)));
        assert_eq!(o.extra.get("b"), Some(&json!(2)));
    }

    #[test]
    fn roles_stay_per_layer_with_plugins_under_global() {
        let plugin = layer(|d, _| {
            d.roles.roles.insert(
                ModelRole::Main,
                ModelSpecPatch {
                    id: Some("p".into()),
                    max_tokens: Some(1),
                    ..ModelSpecPatch::default()
                },
            );
        });
        let global = layer(|d, _| {
            d.roles.roles.insert(
                ModelRole::Main,
                ModelSpecPatch {
                    id: Some("g".into()),
                    ..ModelSpecPatch::default()
                },
            );
        });
        let ws = layer(|d, _| {
            d.roles.roles.insert(
                ModelRole::Main,
                ModelSpecPatch {
                    max_tokens: Some(9),
                    ..ModelSpecPatch::default()
                },
            );
        });
        let config = merge(Some(global), vec![plugin], Some(ws)).config;
        let g = config.global_roles.role(ModelRole::Main).unwrap();
        assert_eq!(g.id, Some(ModelId::new("g")));
        assert_eq!(g.max_tokens, Some(1));
        assert_eq!(
            config
                .workspace_roles
                .role(ModelRole::Main)
                .unwrap()
                .max_tokens,
            Some(9)
        );
    }

    #[test]
    fn keymaps_commands_tools_and_servers_are_replaced_by_key() {
        let global = layer(|d, c| {
            c.keymaps.push(keymap("<leader>r", "session.resume"));
            c.keymaps.push(keymap("gg", "timeline.first"));
            c.commands.push(CommandSpec {
                name: "review".into(),
                callback: CallbackId(1),
                description: None,
                plugin: "config".into(),
            });
            c.tools.push(tool("todo", "config", 2));
            d.mcp_servers.push(McpServerSpec::new(
                "git",
                McpTransport::Http {
                    url: "a".into(),
                    headers: BTreeMap::new(),
                },
            ));
        });
        let ws = layer(|d, c| {
            c.keymaps.push(keymap("<leader>r", "session.new"));
            c.commands.push(CommandSpec {
                name: "review".into(),
                callback: CallbackId(3),
                description: None,
                plugin: "config".into(),
            });
            c.tools.push(tool("todo", "workspace", 4));
            d.mcp_servers.push(McpServerSpec::new(
                "git",
                McpTransport::Http {
                    url: "b".into(),
                    headers: BTreeMap::new(),
                },
            ));
        });
        let merged = merge(Some(global), Vec::new(), Some(ws));
        let c = &merged.config;
        assert_eq!(
            c.keymaps,
            [
                keymap("gg", "timeline.first"),
                keymap("<leader>r", "session.new")
            ]
        );
        assert_eq!(c.commands.len(), 1);
        assert_eq!(c.commands[0].callback, CallbackId(3));
        assert_eq!(c.tools.len(), 1);
        assert_eq!(c.tools[0].plugin, "workspace");
        assert_eq!(merged.tools.get("todo"), Some(&CallbackId(4)));
        assert_eq!(c.mcp_servers.len(), 1);
        assert!(
            matches!(&c.mcp_servers[0].transport, McpTransport::Http { url, .. } if url == "b")
        );
        assert_eq!(c.mcp_servers[0].origin, ConfigOrigin::Workspace);
    }

    #[test]
    fn hooks_keep_load_order() {
        let global = layer(|_, c| c.hooks.push(hook("config", 1)));
        let plugin = layer(|_, c| c.hooks.push(hook("p", 2)));
        let ws = layer(|_, c| c.hooks.push(hook("workspace", 3)));
        let merged = merge(Some(global), vec![plugin], Some(ws));
        let ids: Vec<_> = merged.config.hooks.iter().map(|h| h.callback.0).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(merged.hooks.len(), 3);
    }

    #[test]
    fn the_workspace_can_only_add_sensitive_paths() {
        let ws = || layer(|d, _| d.sensitive_paths = Some(vec!["secrets/".into()]));
        let only_ws = merge(None, Vec::new(), Some(ws()))
            .config
            .sensitive_paths
            .unwrap();
        assert!(only_ws.contains(&".env*".to_owned()));
        assert!(only_ws.contains(&"secrets/".to_owned()));

        let empty_ws = layer(|d, _| d.sensitive_paths = Some(Vec::new()));
        let still = merge(None, Vec::new(), Some(empty_ws))
            .config
            .sensitive_paths
            .unwrap();
        assert_eq!(still.len(), DEFAULT_SENSITIVE_PATTERNS.len());

        let global = layer(|d, _| d.sensitive_paths = Some(vec!["*.key".into()]));
        let both = merge(Some(global), Vec::new(), Some(ws()))
            .config
            .sensitive_paths;
        assert_eq!(both, Some(vec!["*.key".to_owned(), "secrets/".to_owned()]));
    }

    #[test]
    fn nothing_set_keeps_defaults() {
        let config = merge(None, Vec::new(), None).config;
        assert_eq!(config, LoadedConfig::default());
    }
}
