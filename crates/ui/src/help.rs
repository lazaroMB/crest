//! The help modal: everything the user can use, in one place.
//!
//! [`HelpSnapshot`] is plain data: the `/` commands, the effective keymap,
//! the agent's tools, the plugins, the MCP servers, the models, the
//! command-line flags and the paths in use. The session asks its
//! controller for one ([`SessionController::help`]), completes it with
//! what the UI knows by itself ([`HelpSnapshot::complete`]) and shows it in
//! a [`HelpView`] inside a modal (`F1`, `<leader>h`,
//! `/help [section|search]`, the builtin action `help.open`).
//!
//! [`filter`] is the search box: a pure function over the snapshot.
//!
//! The Keys section lists the keymap, then the keys each panel handles by
//! itself ([`panel_keys::PANEL_KEYS`], a static table). Each keymap row
//! says where it comes from ([`source_label`]) and carries a remap snippet
//! ([`remap_snippet`]) that `c` copies; the panel keys are fixed.
//!
//! [`SessionController::help`]: crate::session::SessionController::help

use std::fmt;

use application::keymap::layers::{DEFAULT_SOURCE, GLOBAL_CONFIG, source_label};
use application::keymap::{
    Action, Key, KeyCode, Keymap, KeymapEntry, Platform, action_description, default_keymap_with,
    parse_keys,
};
use domain::modal::ModalId;
use domain::permission::PermissionMode;

use crate::commands::BUILTIN_COMMANDS;
use crate::session::ModelChoice;

pub mod panel_keys;
mod view;

pub use panel_keys::panel_key_rows;
pub use view::HelpView;

/// Builtin action that opens the help.
pub const HELP_ACTION: &str = "help.open";

/// Id of the help modal.
pub fn help_modal_id() -> ModalId {
    ModalId::new("crest/help")
}

/// Who provides a command, a mapping or a tool when it is not crest itself.
pub const BUILTIN_SOURCE: &str = "crest";

/// A `/` command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandRow {
    /// Name after the slash.
    pub name: String,
    /// How to type it.
    pub usage: String,
    /// What it does.
    pub description: String,
    /// `crest`, or the plugin that registered it.
    pub source: String,
}

/// One mapping of the effective keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    /// Keys in Neovim notation, with the leader resolved (`<Space>ff`).
    pub keys: String,
    /// The builtin action, or `Lua function`.
    pub action: String,
    /// What it does.
    pub description: String,
    /// Where the mapping comes from: `default`, `global`, `workspace`,
    /// `global keymap.toml`, `workspace keymap.toml` or `plugin <name>`
    /// (see [`source_label`]).
    pub source: String,
    /// Whether it replaces a builtin mapping of the same keys.
    pub overridden: bool,
    /// The panel that handles the keys by itself; `None` for the keymap.
    pub panel: Option<String>,
    /// What `c` copies to remap the keys ([`remap_snippet`]); `None` for
    /// the fixed keys of a panel.
    pub snippet: Option<String>,
}

/// Where a tool comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Built into crest.
    Builtin,
    /// Registered by a plugin with `app.tools.register`.
    Plugin(String),
    /// Offered by an MCP server.
    Mcp(String),
}

impl fmt::Display for ToolSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => f.write_str("Built-in"),
            Self::Plugin(name) => write!(f, "Plugin · {name}"),
            Self::Mcp(name) => write!(f, "MCP · {name}"),
        }
    }
}

/// A tool the agent can call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    /// Name the model calls it by.
    pub name: String,
    /// Description given to the model.
    pub description: String,
    /// Where it comes from.
    pub source: ToolSource,
    /// Its side effect for the permission policy (`read`, `write`,
    /// `execute`, `depends on input`...).
    pub effect: String,
    /// JSON schema of its input, pretty-printed.
    pub schema: String,
}

/// A loaded plugin (or config file).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginRow {
    /// Plugin name.
    pub name: String,
    /// File it was loaded from, when known.
    pub path: String,
    /// `global` or `workspace`.
    pub layer: String,
    /// What it adds (`tool todo_list`, `command /review`, `keymap <Space>x`,
    /// `hook before_tool`).
    pub adds: Vec<String>,
    /// Capabilities it declares (`network:jira.example.com`).
    pub capabilities: Vec<String>,
    /// `approved`, `pending` or `no capabilities`.
    pub trust: String,
    /// How to approve it, while pending (`/plugin trust jira`).
    pub approve: Option<String>,
    /// Problems found while loading it.
    pub diagnostics: Vec<String>,
}

/// A configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpRow {
    /// Server name (prefix of its tools).
    pub name: String,
    /// `stdio: <command>` or `http: <url>`.
    pub transport: String,
    /// `global` or `workspace`.
    pub origin: String,
    /// `ready (3 tools)`, `failed: ...`, `starting`, `untrusted`...
    pub state: String,
    /// How to trust it, while held back (`/mcp trust git`).
    pub trust_command: Option<String>,
}

/// A role, an alias or a setting of the models.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRow {
    /// `role`, `alias` or `setting`.
    pub kind: String,
    /// Role, alias or setting name.
    pub name: String,
    /// The model id or the value.
    pub value: String,
}

/// A command-line flag of `crest`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliRow {
    /// `--model <ALIAS|ID>`.
    pub flag: String,
    /// Its help text.
    pub description: String,
}

/// A path in use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PathRow {
    /// What it is.
    pub label: String,
    /// The path or pointer.
    pub value: String,
}

/// Everything the help shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HelpSnapshot {
    /// `/` commands: builtin first, then the plugins'.
    pub commands: Vec<CommandRow>,
    /// The effective keymap.
    pub keys: Vec<KeyRow>,
    /// The agent's tools.
    pub tools: Vec<ToolRow>,
    /// Loaded plugins.
    pub plugins: Vec<PluginRow>,
    /// Configured MCP servers.
    pub mcp: Vec<McpRow>,
    /// Model roles, aliases and settings.
    pub models: Vec<ModelRow>,
    /// Flags of the command line.
    pub cli: Vec<CliRow>,
    /// Paths and pointers.
    pub paths: Vec<PathRow>,
    /// Startup problems (config errors and warnings).
    pub diagnostics: Vec<String>,
}

/// What the session itself knows, used by [`HelpSnapshot::complete`].
pub struct LocalHelp<'a> {
    /// The input's keymap.
    pub keymap: &'a Keymap<Action>,
    /// The models of `<leader>m`.
    pub models: &'a [ModelChoice],
    /// The current permission mode.
    pub permission_mode: PermissionMode,
    /// The override of the next turn, if any.
    pub turn_override: Option<&'a str>,
}

impl HelpSnapshot {
    /// Adds what the UI knows by itself: the builtin commands (always, in
    /// front), the keymap and the model aliases when the controller gave
    /// none, and the settings of the next turn.
    pub fn complete(mut self, local: &LocalHelp<'_>) -> Self {
        let mut commands = builtin_command_rows();
        commands.append(&mut self.commands);
        // A command that runs a builtin action (`/picker.files`) says
        // which keys run it too.
        for row in &mut commands {
            let action = crate::commands::COMMAND_ACTIONS
                .iter()
                .find(|(command, _)| *command == row.name)
                .map_or(row.name.as_str(), |(_, action)| action);
            if let Some(keys) = action_keys(local.keymap, action) {
                row.description = format!("{} ({keys})", row.description);
            }
        }
        self.commands = commands;
        if self.keys.is_empty() {
            self.keys = key_rows(local.keymap, |_| None);
        }
        if !self.keys.iter().any(|row| row.panel.is_some()) {
            self.keys.extend(panel_key_rows());
        }
        if self.models.is_empty() {
            self.models = local
                .models
                .iter()
                .map(|choice| ModelRow {
                    kind: "alias".into(),
                    name: choice.alias.clone(),
                    value: choice.model.to_string(),
                })
                .collect();
        }
        self.models.push(ModelRow {
            kind: "setting".into(),
            name: "next turn".into(),
            value: local
                .turn_override
                .map_or_else(|| "default (main role)".to_owned(), str::to_owned),
        });
        self.models.push(ModelRow {
            kind: "setting".into(),
            name: "permission mode".into(),
            value: local.permission_mode.to_string(),
        });
        // Where the rules are seen and changed, with the keys of this keymap.
        let keys = |action: &str, fallback: &str| {
            action_keys(local.keymap, action).unwrap_or_else(|| fallback.to_owned())
        };
        self.models.push(ModelRow {
            kind: "setting".into(),
            name: "permission rules".into(),
            value: format!(
                "/permissions ({}) lists, removes and promotes them; {} cycles the mode",
                keys("permissions.open", "no key"),
                keys("permission.cycle", "/mode"),
            ),
        });
        self
    }

    /// Rows of `section`.
    pub fn len(&self, section: Section) -> usize {
        match section {
            Section::Commands => self.commands.len(),
            Section::Keys => self.keys.len(),
            Section::Tools => self.tools.len(),
            Section::Plugins => self.plugins.len(),
            Section::Mcp => self.mcp.len(),
            Section::Models => self.models.len(),
            Section::Cli => self.cli.len(),
            Section::Files => self.paths.len() + self.diagnostics.len(),
        }
    }

    /// Whether every section is empty.
    pub fn is_empty(&self) -> bool {
        Section::ALL.iter().all(|section| self.len(*section) == 0)
    }
}

/// The builtin `/` commands as rows.
pub fn builtin_command_rows() -> Vec<CommandRow> {
    BUILTIN_COMMANDS
        .iter()
        .map(|info| CommandRow {
            name: info.name.into(),
            usage: info.usage.into(),
            description: info.description.into(),
            source: BUILTIN_SOURCE.into(),
        })
        .collect()
}

/// The keys that run the builtin action `name` in `keymap`, for a hint
/// (`Ctrl+G ff`): the shortest mapping, if any.
pub fn action_keys(keymap: &Keymap<Action>, name: &str) -> Option<String> {
    keymap
        .entries()
        .into_iter()
        .filter(|entry| matches!(&entry.action, Action::Builtin(action) if action == name))
        .min_by_key(|entry| entry.keys.len())
        .map(|entry| keys_label(&entry.keys))
}

/// A key that can follow the pending ones, for the which-key popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextKey {
    /// The key as read (`f`, `Ctrl+C`, `Enter`).
    pub key: String,
    /// What it runs, and how many mappings go on from it.
    pub what: String,
}

/// The keys that can follow `pending` in `keymap`, with what they do: the
/// description of a builtin action (else its name), `Lua function`, and
/// `+N more` when longer mappings start there (`f` before `ff`/`fg`).
pub fn next_keys(keymap: &Keymap<Action>, pending: &[Key]) -> Vec<NextKey> {
    keymap
        .continuations(pending)
        .into_iter()
        .map(|next| {
            let what = match next.action {
                Some(Action::Builtin(name)) => {
                    action_description(name).map_or_else(|| name.clone(), str::to_owned)
                }
                Some(Action::Callback(_)) => "Lua function".to_owned(),
                None => String::new(),
            };
            let more = next.mappings - usize::from(next.action.is_some());
            let what = match (what.is_empty(), more) {
                (_, 0) => what,
                (true, n) => format!("+{n} more"),
                (false, n) => format!("{what} (+{n} more)"),
            };
            NextKey {
                key: keys_label(&[next.key]),
                what,
            }
        })
        .collect()
}

/// `keys` as a person reads them: `Ctrl+G ff`, `Alt+P`, `F1`. A key with
/// modifiers stands alone; plain characters that follow each other are
/// written together.
pub fn keys_label(keys: &[Key]) -> String {
    let mut out = String::new();
    let mut after_plain_char = false;
    for key in keys {
        let m = key.mods;
        let plain_char = matches!(key.code, KeyCode::Char(c) if c != ' ')
            && !(m.ctrl || m.alt || m.cmd || m.shift);
        if !out.is_empty() && !(plain_char && after_plain_char) {
            out.push(' ');
        }
        after_plain_char = plain_char;
        if plain_char {
            if let KeyCode::Char(c) = key.code {
                out.push(c);
            }
            continue;
        }
        for (on, name) in [
            (m.ctrl, "Ctrl+"),
            (m.alt, "Alt+"),
            (m.shift, "Shift+"),
            (m.cmd, "Cmd+"),
        ] {
            if on {
                out.push_str(name);
            }
        }
        out.push_str(&match key.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char(c) => c.to_uppercase().collect(),
            KeyCode::Enter => "Enter".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::Backspace => "Backspace".to_owned(),
            KeyCode::Delete => "Delete".to_owned(),
            KeyCode::Insert => "Insert".to_owned(),
            KeyCode::Up => "Up".to_owned(),
            KeyCode::Down => "Down".to_owned(),
            KeyCode::Left => "Left".to_owned(),
            KeyCode::Right => "Right".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PageUp".to_owned(),
            KeyCode::PageDown => "PageDown".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
        });
    }
    out
}

/// `text` as a string literal of Lua and of TOML (both take `\\` and
/// `\"` escapes).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The lines that map `keys` (notation, `<leader>ff`) to the builtin
/// `action` again, for `init.lua` and for `keymap.toml`: edit the keys or
/// the action and paste one of them. Without an action (a Lua function)
/// they remove the mapping instead.
pub fn remap_snippet(keys: &str, action: Option<&str>) -> String {
    let keys = quoted(keys);
    match action {
        Some(action) => {
            let action = quoted(action);
            format!(
                "-- init.lua\napp.keymap.set({keys}, {action})\n\n# keymap.toml\n[[bind]]\n\
                 keys = {keys}\naction = {action}\n"
            )
        }
        None => format!(
            "-- init.lua\napp.keymap.del({keys})\n\n# keymap.toml\n[[unbind]]\nkeys = {keys}\n"
        ),
    }
}

/// `keys` in notation, with `<leader>` in front when they start with the
/// leader's keys.
fn leader_notation(keys: &[Key], leader: &[Key]) -> String {
    let notation = |keys: &[Key]| keys.iter().map(ToString::to_string).collect::<String>();
    match keys.strip_prefix(leader) {
        Some(rest) if !leader.is_empty() && !rest.is_empty() => {
            format!("<leader>{}", notation(rest))
        }
        _ => notation(keys),
    }
}

/// The rows of `keymap`. `describe` gives the description and the source
/// ([`source_label`]) of a mapping it knows (one set from Lua or
/// `keymap.toml`); the others get the description of their builtin
/// action. A mapping whose keys have another action in the builtin keymap
/// (with the same leader) is marked as overridden.
pub fn key_rows(
    keymap: &Keymap<Action>,
    describe: impl Fn(&KeymapEntry<Action>) -> Option<(Option<String>, String)>,
) -> Vec<KeyRow> {
    let defaults = default_keymap_with(keymap.leader(), Platform::current());
    let leader = parse_keys("<leader>", keymap.leader()).unwrap_or_default();
    keymap
        .entries()
        .into_iter()
        .map(|entry| {
            let default = defaults.lookup(&entry.keys).exact;
            let overridden = default.is_some_and(|action| *action != entry.action);
            let (action, builtin) = match &entry.action {
                Action::Builtin(name) => (name.clone(), action_description(name)),
                Action::Callback(_) => ("Lua function".to_owned(), None),
            };
            let known = describe(&entry);
            let source = match &known {
                Some((_, source)) => source.clone(),
                None if default == Some(&entry.action) => DEFAULT_SOURCE.to_owned(),
                None => source_label(GLOBAL_CONFIG),
            };
            let snippet = remap_snippet(
                &leader_notation(&entry.keys, &leader),
                match &entry.action {
                    Action::Builtin(name) => Some(name.as_str()),
                    Action::Callback(_) => None,
                },
            );
            let description = known
                .and_then(|(description, _)| description)
                .or_else(|| builtin.map(str::to_owned))
                .unwrap_or_else(|| match entry.action {
                    Action::Builtin(_) => "unknown builtin action".to_owned(),
                    Action::Callback(_) => String::new(),
                });
            KeyRow {
                keys: entry.notation(),
                action,
                description,
                source,
                overridden,
                panel: None,
                snippet: Some(snippet),
            }
        })
        .collect()
}

/// A tab of the help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// `/` commands.
    Commands,
    /// The keymap.
    Keys,
    /// The agent's tools.
    Tools,
    /// Loaded plugins.
    Plugins,
    /// MCP servers.
    Mcp,
    /// Models and settings.
    Models,
    /// Command-line flags.
    Cli,
    /// Files and config.
    Files,
}

impl Section {
    /// Every section, in tab order (keys `1`–`8`).
    pub const ALL: [Section; 8] = [
        Section::Commands,
        Section::Keys,
        Section::Tools,
        Section::Plugins,
        Section::Mcp,
        Section::Models,
        Section::Cli,
        Section::Files,
    ];

    /// Its tab title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Commands => "Commands",
            Self::Keys => "Keys",
            Self::Tools => "Tools",
            Self::Plugins => "Plugins",
            Self::Mcp => "MCP servers",
            Self::Models => "Models",
            Self::Cli => "CLI",
            Self::Files => "Files & config",
        }
    }

    /// Its position in [`Section::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The section a `/help` argument names: its title or a short name
    /// (`keys`, `mcp`, `cli`, `files`...), in any case, or its number.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        if let Ok(n) = name.parse::<usize>() {
            return n.checked_sub(1).and_then(|ix| Self::ALL.get(ix).copied());
        }
        let section = match name.as_str() {
            "commands" | "command" | "cmd" | "cmds" => Self::Commands,
            "keys" | "key" | "keymap" | "keymaps" | "mappings" => Self::Keys,
            "tools" | "tool" => Self::Tools,
            "plugins" | "plugin" | "lua" => Self::Plugins,
            "mcp" | "servers" | "mcp servers" => Self::Mcp,
            "models" | "model" => Self::Models,
            "cli" | "flags" | "args" => Self::Cli,
            "files" | "config" | "paths" | "files & config" => Self::Files,
            _ => return None,
        };
        Some(section)
    }
}

/// What `/help <arg>` opens: a section, or the first section with a match
/// for a search.
pub fn parse_help_arg(arg: &str) -> (Option<Section>, String) {
    let arg = arg.trim();
    if arg.is_empty() {
        return (None, String::new());
    }
    match Section::parse(arg) {
        Some(section) => (Some(section), String::new()),
        None => (None, arg.to_owned()),
    }
}

/// Whether every word of `query` (lowercase) appears in one of `fields`.
fn matches(terms: &[String], fields: &[&str]) -> bool {
    let haystack = fields.join("\u{1f}").to_lowercase();
    terms.iter().all(|term| haystack.contains(term.as_str()))
}

/// The snapshot with only the rows that match `query` in every section:
/// each whitespace-separated word must appear (ignoring case) in some field
/// of the row. An empty query keeps everything.
pub fn filter(snapshot: &HelpSnapshot, query: &str) -> HelpSnapshot {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return snapshot.clone();
    }
    let keep = |fields: &[&str]| matches(&terms, fields);
    HelpSnapshot {
        commands: snapshot
            .commands
            .iter()
            .filter(|r| keep(&[&r.name, &r.usage, &r.description, &r.source]))
            .cloned()
            .collect(),
        keys: snapshot
            .keys
            .iter()
            .filter(|r| {
                let panel = r.panel.as_deref().unwrap_or_default();
                keep(&[&r.keys, &r.action, &r.description, &r.source, panel])
            })
            .cloned()
            .collect(),
        tools: snapshot
            .tools
            .iter()
            .filter(|r| {
                let source = r.source.to_string();
                keep(&[&r.name, &r.description, &source, &r.effect])
            })
            .cloned()
            .collect(),
        plugins: snapshot
            .plugins
            .iter()
            .filter(|r| {
                let lists = [
                    r.adds.join(" "),
                    r.capabilities.join(" "),
                    r.diagnostics.join(" "),
                ];
                keep(&[
                    &r.name, &r.path, &r.layer, &r.trust, &lists[0], &lists[1], &lists[2],
                ])
            })
            .cloned()
            .collect(),
        mcp: snapshot
            .mcp
            .iter()
            .filter(|r| keep(&[&r.name, &r.transport, &r.origin, &r.state]))
            .cloned()
            .collect(),
        models: snapshot
            .models
            .iter()
            .filter(|r| keep(&[&r.kind, &r.name, &r.value]))
            .cloned()
            .collect(),
        cli: snapshot
            .cli
            .iter()
            .filter(|r| keep(&[&r.flag, &r.description]))
            .cloned()
            .collect(),
        paths: snapshot
            .paths
            .iter()
            .filter(|r| keep(&[&r.label, &r.value]))
            .cloned()
            .collect(),
        diagnostics: snapshot
            .diagnostics
            .iter()
            .filter(|d| keep(&[d.as_str()]))
            .cloned()
            .collect(),
    }
}

/// How one row is shown and what `y` copies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowView {
    /// Heading of the group the row starts, if it starts one.
    pub group: Option<String>,
    /// Main column (name, keys, flag).
    pub primary: String,
    /// Description.
    pub secondary: String,
    /// Short tags (source, effect, trust...).
    pub tags: Vec<String>,
    /// Extra lines shown when the row is expanded (schema, details).
    pub details: Vec<String>,
    /// What `y` copies.
    pub copy: String,
    /// What `c` copies: a remap snippet (Keys section, keymap rows only).
    pub snippet: Option<String>,
}

fn group_of<T>(rows: &[T], ix: usize, key: impl Fn(&T) -> String) -> Option<String> {
    let this = key(&rows[ix]);
    match ix.checked_sub(1).map(|prev| key(&rows[prev])) {
        Some(prev) if prev == this => None,
        _ => Some(this),
    }
}

/// The rows of `section` as shown.
pub fn section_rows(snapshot: &HelpSnapshot, section: Section) -> Vec<RowView> {
    match section {
        Section::Commands => {
            let rows = &snapshot.commands;
            (0..rows.len())
                .map(|ix| {
                    let r = &rows[ix];
                    RowView {
                        group: group_of(rows, ix, |r| {
                            if r.source == BUILTIN_SOURCE {
                                "Built-in".to_owned()
                            } else {
                                "Plugins".to_owned()
                            }
                        }),
                        primary: r.usage.clone(),
                        secondary: r.description.clone(),
                        tags: (r.source != BUILTIN_SOURCE)
                            .then(|| r.source.clone())
                            .into_iter()
                            .collect(),
                        details: Vec::new(),
                        copy: format!("/{}", r.name),
                        snippet: None,
                    }
                })
                .collect()
        }
        Section::Keys => {
            let rows = &snapshot.keys;
            (0..rows.len())
                .map(|ix| {
                    let r = &rows[ix];
                    // Keymap rows: the action and where it comes from;
                    // panel rows: where the keys work, and that they are
                    // fixed.
                    let mut tags = match &r.panel {
                        Some(panel) => vec![panel.clone(), panel_keys::FIXED_ACTION.to_owned()],
                        None => vec![r.action.clone(), r.source.clone()],
                    };
                    if r.overridden {
                        tags.push("overrides default".into());
                    }
                    let details = match (&r.snippet, &r.panel) {
                        (Some(snippet), _) => std::iter::once(
                            "c copies this remap snippet (init.lua or keymap.toml):".to_owned(),
                        )
                        .chain(snippet.lines().map(str::to_owned))
                        .collect(),
                        (None, Some(panel)) => vec![format!(
                            "fixed: {panel} handles these keys itself; they cannot be remapped"
                        )],
                        (None, None) => Vec::new(),
                    };
                    RowView {
                        group: group_of(rows, ix, |r| match &r.panel {
                            Some(panel) => format!("{panel} (fixed keys)"),
                            None => "Shortcuts (the keymap; dialogs, pickers and forms keep \
                                     their own keys)"
                                .to_owned(),
                        }),
                        primary: r.keys.clone(),
                        secondary: r.description.clone(),
                        tags,
                        details,
                        copy: r.keys.clone(),
                        snippet: r.snippet.clone(),
                    }
                })
                .collect()
        }
        Section::Tools => {
            let rows = &snapshot.tools;
            (0..rows.len())
                .map(|ix| {
                    let r = &rows[ix];
                    RowView {
                        group: group_of(rows, ix, |r| match &r.source {
                            ToolSource::Builtin => "Built-in".to_owned(),
                            ToolSource::Plugin(_) => "Plugin".to_owned(),
                            ToolSource::Mcp(_) => "MCP".to_owned(),
                        }),
                        primary: r.name.clone(),
                        secondary: r.description.clone(),
                        tags: match &r.source {
                            ToolSource::Builtin => vec![r.effect.clone()],
                            ToolSource::Plugin(name) | ToolSource::Mcp(name) => {
                                vec![r.effect.clone(), name.clone()]
                            }
                        },
                        details: std::iter::once("input schema:".to_owned())
                            .chain(r.schema.lines().map(str::to_owned))
                            .collect(),
                        copy: r.name.clone(),
                        snippet: None,
                    }
                })
                .collect()
        }
        Section::Plugins => snapshot
            .plugins
            .iter()
            .map(|r| {
                let mut details = Vec::new();
                if !r.path.is_empty() {
                    details.push(format!("file: {}", r.path));
                }
                if !r.adds.is_empty() {
                    details.push(format!("adds: {}", r.adds.join(", ")));
                }
                if !r.capabilities.is_empty() {
                    details.push(format!("capabilities: {}", r.capabilities.join(", ")));
                }
                if let Some(approve) = &r.approve {
                    details.push(format!(
                        "approve with {approve} or --trust-plugin {}",
                        r.name
                    ));
                }
                details.extend(r.diagnostics.iter().map(|d| format!("problem: {d}")));
                let summary = if r.adds.is_empty() {
                    "adds nothing".to_owned()
                } else {
                    r.adds.join(", ")
                };
                RowView {
                    group: None,
                    primary: r.name.clone(),
                    secondary: summary,
                    tags: vec![r.layer.clone(), r.trust.clone()],
                    details,
                    copy: r.approve.clone().unwrap_or_else(|| r.name.clone()),
                    snippet: None,
                }
            })
            .collect(),
        Section::Mcp => snapshot
            .mcp
            .iter()
            .map(|r| RowView {
                group: None,
                primary: r.name.clone(),
                secondary: r.state.clone(),
                tags: vec![r.origin.clone()],
                details: std::iter::once(r.transport.clone())
                    .chain(
                        r.trust_command
                            .iter()
                            .map(|c| format!("trust with {c} or --trust-mcp {}", r.name)),
                    )
                    .collect(),
                copy: r.trust_command.clone().unwrap_or_else(|| r.name.clone()),
                snippet: None,
            })
            .collect(),
        Section::Models => {
            let rows = &snapshot.models;
            (0..rows.len())
                .map(|ix| {
                    let r = &rows[ix];
                    RowView {
                        group: group_of(rows, ix, |r| match r.kind.as_str() {
                            "role" => "Roles".to_owned(),
                            "alias" => "Aliases".to_owned(),
                            _ => "Settings".to_owned(),
                        }),
                        primary: r.name.clone(),
                        secondary: r.value.clone(),
                        tags: Vec::new(),
                        details: Vec::new(),
                        copy: r.value.clone(),
                        snippet: None,
                    }
                })
                .collect()
        }
        Section::Cli => snapshot
            .cli
            .iter()
            .map(|r| RowView {
                group: None,
                primary: r.flag.clone(),
                secondary: r.description.clone(),
                tags: Vec::new(),
                details: Vec::new(),
                copy: r
                    .flag
                    .split([' ', ','])
                    .find(|w| w.starts_with("--"))
                    .unwrap_or(&r.flag)
                    .to_owned(),
                snippet: None,
            })
            .collect(),
        Section::Files => {
            let mut rows: Vec<RowView> = snapshot
                .paths
                .iter()
                .enumerate()
                .map(|(ix, r)| RowView {
                    group: (ix == 0).then(|| "Paths".to_owned()),
                    primary: r.label.clone(),
                    secondary: r.value.clone(),
                    copy: r.value.clone(),
                    ..RowView::default()
                })
                .collect();
            rows.extend(
                snapshot
                    .diagnostics
                    .iter()
                    .enumerate()
                    .map(|(ix, d)| RowView {
                        group: (ix == 0).then(|| "Problems at startup".to_owned()),
                        primary: d.clone(),
                        copy: d.clone(),
                        ..RowView::default()
                    }),
            );
            rows
        }
    }
}

#[cfg(test)]
mod tests;
