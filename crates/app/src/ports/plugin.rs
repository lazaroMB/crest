//! What the core needs from the Lua host: configuration, keymaps, commands,
//! tools and hooks defined in `init.lua` and plugins.
//!
//! The Lua VM lives on its own thread and never touches the UI; the core
//! talks to it through this port. Lua functions stay inside the VM and are
//! referred to by [`CallbackId`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use domain::model::{ModelPolicy, RoleConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Handle to a Lua function kept alive by the host (keymap, command or hook).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallbackId(pub u64);

impl fmt::Display for CallbackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Configuration files to load; absent files are skipped.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConfigSources {
    /// Global config, usually `~/.config/crest/init.lua`. Plugins are looked
    /// up in the `plugins/` directory next to it.
    pub global: Option<PathBuf>,
    /// Workspace config, usually `<workspace>/.crest/init.lua`.
    pub workspace: Option<PathBuf>,
}

/// Options set with `app.opt.*`. `None` means "use the built-in default".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppOptions {
    /// Key used as `<leader>` in keymaps.
    pub leader: Option<String>,
    /// UI theme name.
    pub theme: Option<String>,
    /// Time to wait for the next key of a sequence, in milliseconds.
    pub timeoutlen_ms: Option<u64>,
    /// Any other option, for plugins and future settings.
    pub extra: Map<String, Value>,
}

/// Mode a keymap applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMode {
    /// Typing the prompt (`"i"` in Lua).
    Insert,
    /// Navigating the timeline (`"n"` in Lua).
    Normal,
    /// The `:` command line (`"c"` in Lua).
    Command,
}

/// What a keymap does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum KeyAction {
    /// A built-in action by name (e.g. `session.resume`).
    Builtin(String),
    /// A Lua function, run through [`PluginHostPort::invoke_callback`].
    Callback(CallbackId),
}

/// A keymap from `app.keymap.set(mode, lhs, action)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeymapSpec {
    /// Mode it applies to.
    pub mode: KeyMode,
    /// Key sequence in Neovim notation (e.g. `<leader>ff`, `gg`).
    pub lhs: String,
    /// Action to run.
    pub action: KeyAction,
}

/// A chat command from `app.commands.register(name, fn)`, run as `/name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Command name, without the slash.
    pub name: String,
    /// Lua function receiving the arguments.
    pub callback: CallbackId,
}

/// A tool from `app.tools.register`, offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginToolSpec {
    /// Tool name, unique among all tools.
    pub name: String,
    /// Description shown to the model.
    pub description: String,
    /// JSON schema of the input.
    pub input_schema: Value,
    /// Plugin (or config file) that registered it.
    pub plugin: String,
}

/// Point of the tool loop where a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    /// Before a tool call; may deny it or rewrite its input.
    BeforeTool,
    /// After a tool call, with its output.
    AfterTool,
}

/// A hook from `app.on(event, fn)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSpec {
    /// When it runs.
    pub kind: HookKind,
    /// Lua function to call.
    pub callback: CallbackId,
    /// Plugin (or config file) that registered it.
    pub plugin: String,
}

/// How to reach an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// A child process speaking over stdin/stdout.
    Stdio {
        /// Program to run.
        command: String,
        /// Arguments.
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment variables.
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// A remote server over streamable HTTP.
    Http {
        /// Endpoint URL.
        url: String,
        /// Extra request headers (e.g. authorization).
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

/// An MCP server declared in the config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerSpec {
    /// Server name, used to prefix its tools.
    pub name: String,
    /// How to connect.
    pub transport: McpTransport,
}

/// A problem found while loading a config file or plugin. Loading goes on
/// with the rest; the UI shows each diagnostic as an `Error` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// File or plugin where it happened.
    pub source: String,
    /// What went wrong.
    pub message: String,
    /// Line in `source`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Result of loading the global and workspace configs and the plugins.
///
/// Scalar options, keymaps and commands are already merged, the workspace
/// overriding the global config field by field. Model roles are kept per
/// layer so the model policy can report where each spec came from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadedConfig {
    /// Model roles and aliases from the global config.
    pub global_roles: RoleConfig,
    /// Model roles and aliases from the workspace config.
    pub workspace_roles: RoleConfig,
    /// Options from `app.opt`.
    pub options: AppOptions,
    /// Keymaps, in registration order (later ones win).
    pub keymaps: Vec<KeymapSpec>,
    /// Chat commands.
    pub commands: Vec<CommandSpec>,
    /// Tools offered to the model.
    pub tools: Vec<PluginToolSpec>,
    /// Hooks, in the order they run.
    pub hooks: Vec<HookSpec>,
    /// MCP servers to start.
    pub mcp_servers: Vec<McpServerSpec>,
    /// Sensitive path patterns; `None` keeps the built-in defaults.
    pub sensitive_paths: Option<Vec<String>>,
    /// Problems found while loading; empty when everything loaded.
    pub errors: Vec<PluginDiagnostic>,
}

impl LoadedConfig {
    /// Builds the model policy from both role layers.
    #[must_use]
    pub fn model_policy(&self) -> ModelPolicy {
        ModelPolicy::new(self.global_roles.clone(), self.workspace_roles.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_empty() {
        let config = LoadedConfig::default();
        assert_eq!(config.options, AppOptions::default());
        assert!(config.keymaps.is_empty());
        assert!(config.tools.is_empty());
        assert!(config.sensitive_paths.is_none());
        assert!(config.errors.is_empty());
        assert_eq!(config.model_policy(), ModelPolicy::default());
    }

    #[test]
    fn missing_fields_deserialize_to_defaults() {
        let config: LoadedConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, LoadedConfig::default());
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = LoadedConfig {
            options: AppOptions {
                leader: Some(" ".into()),
                theme: Some("tokyonight".into()),
                timeoutlen_ms: Some(500),
                extra: Map::new(),
            },
            keymaps: vec![
                KeymapSpec {
                    mode: KeyMode::Normal,
                    lhs: "<leader>r".into(),
                    action: KeyAction::Builtin("session.resume".into()),
                },
                KeymapSpec {
                    mode: KeyMode::Normal,
                    lhs: "<leader>x".into(),
                    action: KeyAction::Callback(CallbackId(3)),
                },
            ],
            mcp_servers: vec![McpServerSpec {
                name: "git".into(),
                transport: McpTransport::Stdio {
                    command: "mcp-git".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            }],
            errors: vec![PluginDiagnostic {
                source: "init.lua".into(),
                message: "unexpected symbol".into(),
                line: Some(4),
            }],
            ..LoadedConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: LoadedConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }
}
