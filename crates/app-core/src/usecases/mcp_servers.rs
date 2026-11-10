//! The MCP servers manager, minus the I/O: what `mcp.json` holds, how it
//! merges with the servers of `init.lua`, what the window shows for each
//! server and what its add/edit form sends back. The binary reads and
//! writes the files and talks to the running manager; the window only sees
//! the types here.
//!
//! `mcp.json` sits next to an `init.lua` (`~/.config/crest/mcp.json` for
//! every project, `<workspace>/.crest/mcp.json` for one) and holds the same
//! fields `app.mcp.add` takes, keyed by server name:
//!
//! ```json
//! { "servers": { "github": { "url": "https://api.githubcopilot.com/mcp/", "effect": "network" } } }
//! ```

use std::collections::BTreeMap;

use domain::permission::ToolEffect;
use serde::{Deserialize, Serialize};

use crate::ports::mcp::{McpLogEntry, McpServerState, McpTool, invalid_server_name};
use crate::ports::plugin::{ConfigOrigin, McpOAuthClient, McpServerSpec, McpSource, McpTransport};

/// Default per-call limit shown when a server sets none, in seconds.
pub const DEFAULT_TIMEOUT_S: u64 = 120;
/// Default startup limit shown when a server sets none, in seconds.
pub const DEFAULT_STARTUP_TIMEOUT_S: u64 = 120;

// ---------------------------------------------------------------- mcp.json

/// `type` of a server in `mcp.json` and in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpKind {
    /// A local program crest starts.
    #[default]
    Stdio,
    /// A remote server crest connects to.
    Http,
}

/// One server of `mcp.json`: the fields of `app.mcp.add`, flat.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct McpServerDef {
    /// `stdio` or `http`; optional, `command`/`url` tell them apart.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<McpKind>,
    /// Program of a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Its arguments.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Its extra environment.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Its working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Endpoint of an http server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Its extra headers.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Its pre-registered OAuth client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthClient>,
    /// Side effect of its tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<ToolEffect>,
    /// Per-call limit, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    /// Startup limit, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_timeout: Option<f64>,
    /// Declared but off.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl McpServerDef {
    /// The spec of this definition as `name`, declared in `origin`'s
    /// `mcp.json`; the first problem otherwise.
    pub fn into_spec(self, name: &str, origin: ConfigOrigin) -> Result<McpServerSpec, String> {
        let what = format!("mcp.json `{name}`");
        if let Some(problem) = invalid_server_name(name) {
            return Err(problem);
        }
        let command = self.command.filter(|c| !c.trim().is_empty());
        let url = self.url.filter(|u| !u.trim().is_empty());
        let transport = match (command, url) {
            (Some(_), Some(_)) => {
                return Err(format!("{what} cannot have both `command` and `url`"));
            }
            (None, None) => return Err(format!("{what} needs either `command` or `url`")),
            (Some(command), None) => {
                if self.kind == Some(McpKind::Http) {
                    return Err(format!("{what}: type is http but the server has `command`"));
                }
                McpTransport::Stdio {
                    command,
                    args: self.args,
                    env: self.env,
                    cwd: self.cwd.filter(|c| !c.trim().is_empty()),
                }
            }
            (None, Some(url)) => {
                if self.kind == Some(McpKind::Stdio) {
                    return Err(format!("{what}: type is stdio but the server has `url`"));
                }
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(format!("{what}: url must start with http:// or https://"));
                }
                McpTransport::Http {
                    url,
                    headers: self.headers,
                    oauth: self.oauth,
                }
            }
        };
        let mut spec = McpServerSpec::new(name, transport);
        spec.origin = origin;
        spec.source = McpSource::Json;
        spec.disabled = self.disabled;
        if let Some(effect) = self.effect {
            spec.effect = effect;
        }
        spec.timeout_ms = seconds_to_ms(self.timeout, &what, "timeout")?;
        spec.startup_timeout_ms = seconds_to_ms(self.startup_timeout, &what, "startup_timeout")?;
        Ok(spec)
    }
}

impl From<&McpServerSpec> for McpServerDef {
    fn from(spec: &McpServerSpec) -> Self {
        let mut def = Self {
            effect: Some(spec.effect),
            timeout: spec.timeout_ms.map(ms_to_seconds),
            startup_timeout: spec.startup_timeout_ms.map(ms_to_seconds),
            disabled: spec.disabled,
            ..Self::default()
        };
        match &spec.transport {
            McpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                def.kind = Some(McpKind::Stdio);
                def.command = Some(command.clone());
                def.args.clone_from(args);
                def.env.clone_from(env);
                def.cwd.clone_from(cwd);
            }
            McpTransport::Http {
                url,
                headers,
                oauth,
            } => {
                def.kind = Some(McpKind::Http);
                def.url = Some(url.clone());
                def.headers.clone_from(headers);
                def.oauth.clone_from(oauth);
            }
        }
        def
    }
}

/// A timeout given in seconds, as milliseconds; `None` keeps the default.
fn seconds_to_ms(secs: Option<f64>, what: &str, field: &str) -> Result<Option<u64>, String> {
    match secs {
        None => Ok(None),
        Some(secs) if secs > 0.0 && secs.is_finite() => {
            // Saturates for absurd values, which is what we want.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ms = (secs * 1000.0).ceil() as u64;
            Ok(Some(ms))
        }
        Some(secs) => Err(format!(
            "{what}: {field} must be a positive number of seconds, got {secs}"
        )),
    }
}

/// Milliseconds as the seconds `mcp.json` holds (whole when they are).
#[allow(clippy::cast_precision_loss)]
fn ms_to_seconds(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

/// The contents of one `mcp.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpFile {
    /// The servers, by name.
    pub servers: BTreeMap<String, McpServerDef>,
}

impl McpFile {
    /// Reads a file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// The text to write back: pretty JSON with a final newline.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned());
        text.push('\n');
        text
    }

    /// The specs of the file's servers, declared in `origin`, in name
    /// order, and the problems of the entries that could not be used.
    #[must_use]
    pub fn into_specs(self, origin: ConfigOrigin) -> (Vec<McpServerSpec>, Vec<String>) {
        let mut specs = Vec::new();
        let mut problems = Vec::new();
        for (name, def) in self.servers {
            match def.into_spec(&name, origin) {
                Ok(spec) => specs.push(spec),
                Err(problem) => problems.push(problem),
            }
        }
        (specs, problems)
    }

    /// Adds or replaces `spec` under its name.
    pub fn upsert(&mut self, spec: &McpServerSpec) {
        self.servers
            .insert(spec.name.clone(), McpServerDef::from(spec));
    }

    /// Drops `name`; whether it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        self.servers.remove(name).is_some()
    }
}

/// The servers of every layer as one list, in precedence order: global
/// `mcp.json`, then global `init.lua` (with its plugins), then the
/// workspace `mcp.json`, then the workspace `init.lua`. A later layer
/// replaces an earlier server of the same name, which keeps its position.
/// A `mcp.json` server replaced by the `init.lua` next to it gets a
/// warning: the manager cannot edit it there.
#[must_use]
pub fn merge_layers(
    lua: Vec<McpServerSpec>,
    global_json: Vec<McpServerSpec>,
    workspace_json: Vec<McpServerSpec>,
) -> (Vec<McpServerSpec>, Vec<String>) {
    let (workspace_lua, global_lua): (Vec<_>, Vec<_>) = lua
        .into_iter()
        .partition(|spec| spec.origin == ConfigOrigin::Workspace);
    let mut merged: Vec<McpServerSpec> = Vec::new();
    let mut warnings = Vec::new();
    let layers = [
        (global_json, false),
        (global_lua, true),
        (workspace_json, false),
        (workspace_lua, true),
    ];
    for (layer, is_lua) in layers {
        for spec in layer {
            match merged.iter().position(|s| s.name == spec.name) {
                Some(at) => {
                    let shadowed = &merged[at];
                    if is_lua
                        && shadowed.source == McpSource::Json
                        && shadowed.origin == spec.origin
                    {
                        warnings.push(format!(
                            "MCP server `{}` in {} is shadowed by `app.mcp` in init.lua; edit it there",
                            spec.name,
                            where_saved(shadowed.origin, McpSource::Json)
                        ));
                    }
                    merged[at] = spec;
                }
                None => merged.push(spec),
            }
        }
    }
    (merged, warnings)
}

/// The file a server of `origin` and `source` is declared in, as shown to
/// the user (`~/.config/crest/mcp.json`, `.crest/init.lua`...).
#[must_use]
pub fn where_saved(origin: ConfigOrigin, source: McpSource) -> String {
    let file = match source {
        McpSource::Lua => "init.lua",
        McpSource::Json => "mcp.json",
    };
    match origin {
        ConfigOrigin::Global => format!("~/.config/crest/{file}"),
        ConfigOrigin::Workspace => format!(".crest/{file}"),
    }
}

// ---------------------------------------------------------------- listing

/// Every server the window shows, with what the manager may do.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct McpListing {
    /// The servers, in config order.
    pub servers: Vec<McpServerInfo>,
    /// Whether servers can be added, edited and removed (a manager runs).
    pub can_edit: bool,
    /// The global `mcp.json`, for the form's "Available in".
    pub global_file: Option<String>,
    /// The workspace `mcp.json`; `None` without a workspace.
    pub workspace_file: Option<String>,
}

impl McpListing {
    /// The names in use, which a new server may not take.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.servers.iter().map(|s| s.spec.name.clone()).collect()
    }

    /// The server called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&McpServerInfo> {
        self.servers.iter().find(|s| s.spec.name == name)
    }
}

/// One server as the window shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpServerInfo {
    /// Its declaration: name, transport, effect, timeouts, origin, source.
    pub spec: McpServerSpec,
    /// Where it is in its lifecycle.
    pub state: McpServerState,
    /// Its tools, once ready.
    pub tools: Vec<McpToolInfo>,
    /// Whether an OAuth token is stored for it; `None` for stdio servers.
    pub logged_in: Option<bool>,
    /// The last lines it wrote on stderr (stdio servers).
    pub stderr: Vec<String>,
    /// What happened to it, newest first.
    pub log: Vec<McpLogEntry>,
    /// The file it is declared in, as shown to the user.
    pub saved_in: String,
}

impl McpServerInfo {
    /// A server of `spec` in `state`, with nothing else known yet.
    #[must_use]
    pub fn new(spec: McpServerSpec, state: McpServerState) -> Self {
        let saved_in = where_saved(spec.origin, spec.source);
        Self {
            spec,
            state,
            tools: Vec::new(),
            logged_in: None,
            stderr: Vec::new(),
            log: Vec::new(),
            saved_in,
        }
    }

    /// Whether the manager may edit or remove it (it lives in `mcp.json`).
    #[must_use]
    pub fn editable(&self) -> bool {
        self.spec.source == McpSource::Json
    }

    /// Whether it is on (anything but disabled).
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.state != McpServerState::Disabled
    }

    /// Whether it can log in through the browser (http with OAuth).
    #[must_use]
    pub fn has_oauth(&self) -> bool {
        matches!(
            &self.spec.transport,
            McpTransport::Http { oauth: Some(_), .. }
        )
    }

    /// The per-call limit in seconds, default included.
    #[must_use]
    pub fn timeout_s(&self) -> u64 {
        self.spec
            .timeout_ms
            .map_or(DEFAULT_TIMEOUT_S, |ms| ms.div_ceil(1000))
    }

    /// The startup limit in seconds, default included.
    #[must_use]
    pub fn startup_timeout_s(&self) -> u64 {
        self.spec
            .startup_timeout_ms
            .map_or(DEFAULT_STARTUP_TIMEOUT_S, |ms| ms.div_ceil(1000))
    }
}

/// A tool of a ready server, as the Tools tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolInfo {
    /// Its name within the server.
    pub name: String,
    /// The name the model uses (`mcp__<server>__<tool>`).
    pub qualified: String,
    /// What it does.
    pub description: String,
    /// Its arguments, from its input schema.
    pub params: Vec<McpToolParam>,
}

impl From<&McpTool> for McpToolInfo {
    fn from(tool: &McpTool) -> Self {
        Self {
            name: tool.name.clone(),
            qualified: tool.qualified_name(),
            description: tool.description.clone(),
            params: tool_params(&tool.input_schema),
        }
    }
}

/// One argument of a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolParam {
    /// Its name.
    pub name: String,
    /// Its type, as the schema says (`string`, `integer`, `a | b`...).
    pub kind: String,
    /// Whether the schema requires it.
    pub required: bool,
    /// What it is for.
    pub description: String,
}

/// The arguments a JSON Schema object declares, in schema order.
#[must_use]
pub fn tool_params(input_schema: &serde_json::Value) -> Vec<McpToolParam> {
    let required: Vec<&str> = input_schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let Some(properties) = input_schema.get("properties").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, schema)| McpToolParam {
            name: name.clone(),
            kind: schema_kind(schema),
            required: required.contains(&name.as_str()),
            description: schema
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_owned(),
        })
        .collect()
}

/// A short name for a property's type.
fn schema_kind(schema: &serde_json::Value) -> String {
    if let Some(values) = schema.get("enum").and_then(|e| e.as_array()) {
        let words: Vec<String> = values
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        return words.join(" | ");
    }
    match schema.get("type") {
        Some(serde_json::Value::String(kind)) => {
            if kind == "array" {
                let items = schema
                    .get("items")
                    .map_or_else(|| "any".to_owned(), schema_kind);
                format!("{items}[]")
            } else {
                kind.clone()
            }
        }
        Some(serde_json::Value::Array(kinds)) => kinds
            .iter()
            .filter_map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(" | "),
        _ if schema.get("anyOf").is_some()
            || schema.get("oneOf").is_some()
            || schema.get("$ref").is_some() =>
        {
            "object".to_owned()
        }
        _ => "any".to_owned(),
    }
}

/// What the window asks the manager to do with a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McpAction {
    /// Connect once more and report, without changing anything.
    Test,
    /// Stop it and start it again.
    Restart,
    /// Log in through the browser (http with OAuth).
    Login,
    /// Give up a login that waits for the browser.
    CancelLogin,
    /// Forget the stored token and reconnect.
    Logout,
    /// Approve a workspace server and start it.
    Trust,
    /// Turn it on.
    Enable,
    /// Turn it off.
    Disable,
}

impl McpAction {
    /// The verb, as `/mcp <verb> <name>` spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Restart => "restart",
            Self::Login => "login",
            Self::CancelLogin => "cancel-login",
            Self::Logout => "logout",
            Self::Trust => "trust",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
}

// ---------------------------------------------------------------- the form

/// The add/edit form of a server: every field as typed.
#[derive(Debug, Clone, PartialEq)]
pub struct McpDraft {
    /// Server name.
    pub name: String,
    /// Local command or remote URL.
    pub kind: McpKind,
    /// Which `mcp.json` keeps it.
    pub scope: ConfigOrigin,
    /// Program of a stdio server.
    pub command: String,
    /// Its arguments, one per row.
    pub args: Vec<String>,
    /// Its environment, `(name, value)` rows.
    pub env: Vec<(String, String)>,
    /// Its working directory; empty is the workspace.
    pub cwd: String,
    /// Endpoint of an http server.
    pub url: String,
    /// Its headers, `(name, value)` rows.
    pub headers: Vec<(String, String)>,
    /// Its OAuth client, when it logs in through the browser.
    pub oauth: Option<McpOAuthDraft>,
    /// Side effect of its tools.
    pub effect: ToolEffect,
    /// Per-call limit in seconds, as typed; empty keeps the default.
    pub timeout_s: String,
    /// Startup limit in seconds, as typed; empty keeps the default.
    pub startup_timeout_s: String,
    /// Kept off.
    pub disabled: bool,
}

/// The OAuth fields of the form, as typed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpOAuthDraft {
    /// Client id; empty registers crest dynamically.
    pub client_id: String,
    /// Client secret, for confidential clients.
    pub client_secret: String,
    /// Scopes, space separated.
    pub scopes: String,
    /// Loopback port of the redirect URI.
    pub callback_port: String,
}

impl Default for McpDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: McpKind::Stdio,
            scope: ConfigOrigin::Global,
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: String::new(),
            url: String::new(),
            headers: Vec::new(),
            oauth: None,
            effect: ToolEffect::Execute,
            timeout_s: String::new(),
            startup_timeout_s: String::new(),
            disabled: false,
        }
    }
}

impl McpDraft {
    /// An empty stdio server for every project.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The form filled with `spec`, to edit it.
    #[must_use]
    pub fn from_spec(spec: &McpServerSpec) -> Self {
        let mut draft = Self {
            name: spec.name.clone(),
            scope: spec.origin,
            effect: spec.effect,
            timeout_s: spec.timeout_ms.map(seconds_text).unwrap_or_default(),
            startup_timeout_s: spec
                .startup_timeout_ms
                .map(seconds_text)
                .unwrap_or_default(),
            disabled: spec.disabled,
            ..Self::default()
        };
        match &spec.transport {
            McpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                draft.kind = McpKind::Stdio;
                draft.command.clone_from(command);
                draft.args.clone_from(args);
                draft.env = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                draft.cwd = cwd.clone().unwrap_or_default();
            }
            McpTransport::Http {
                url,
                headers,
                oauth,
            } => {
                draft.kind = McpKind::Http;
                draft.url.clone_from(url);
                draft.headers = headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                draft.oauth = oauth.as_ref().map(|client| McpOAuthDraft {
                    client_id: client.client_id.clone(),
                    client_secret: client.client_secret.clone().unwrap_or_default(),
                    scopes: client.scopes.join(" "),
                    callback_port: client
                        .callback_port
                        .map(|p| p.to_string())
                        .unwrap_or_default(),
                });
            }
        }
        draft
    }

    /// The spec this form describes, or the first thing wrong with it.
    /// `taken` are the names of the other servers.
    pub fn validate(&self, taken: &[String]) -> Result<McpServerSpec, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("the name takes letters, digits, - and _".to_owned());
        }
        if let Some(problem) = invalid_server_name(name) {
            return Err(if name.contains("__") {
                "the name can't contain __ (tools are named mcp__<server>__<tool>)".to_owned()
            } else {
                problem
            });
        }
        if taken.iter().any(|t| t == name) {
            return Err(format!("there is already a server named {name}"));
        }
        let transport = match self.kind {
            McpKind::Stdio => {
                let command = self.command.trim();
                if command.is_empty() {
                    return Err("a stdio server needs a command".to_owned());
                }
                McpTransport::Stdio {
                    command: command.to_owned(),
                    args: self
                        .args
                        .iter()
                        .map(|a| a.trim().to_owned())
                        .filter(|a| !a.is_empty())
                        .collect(),
                    env: pairs(&self.env),
                    cwd: Some(self.cwd.trim().to_owned()).filter(|c| !c.is_empty()),
                }
            }
            McpKind::Http => {
                let url = self.url.trim();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err("an http server needs an http(s) url".to_owned());
                }
                let oauth = match &self.oauth {
                    None => None,
                    Some(draft) => {
                        let callback_port = match draft.callback_port.trim() {
                            "" => None,
                            port => Some(port.parse::<u16>().map_err(|_| {
                                "the callback port must be a number up to 65535".to_owned()
                            })?),
                        };
                        Some(McpOAuthClient {
                            client_id: draft.client_id.trim().to_owned(),
                            client_secret: Some(draft.client_secret.trim().to_owned())
                                .filter(|s| !s.is_empty()),
                            scopes: draft.scopes.split_whitespace().map(str::to_owned).collect(),
                            callback_port,
                        })
                    }
                };
                McpTransport::Http {
                    url: url.to_owned(),
                    headers: pairs(&self.headers),
                    oauth,
                }
            }
        };
        let mut spec = McpServerSpec::new(name, transport);
        spec.origin = self.scope;
        spec.source = McpSource::Json;
        spec.effect = self.effect;
        spec.disabled = self.disabled;
        spec.timeout_ms = seconds_field(&self.timeout_s, "the per-call timeout")?;
        spec.startup_timeout_ms = seconds_field(&self.startup_timeout_s, "the startup timeout")?;
        Ok(spec)
    }
}

/// `(name, value)` rows as a map, without the empty names.
fn pairs(rows: &[(String, String)]) -> BTreeMap<String, String> {
    rows.iter()
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// A seconds field of the form as milliseconds; empty keeps the default.
fn seconds_field(text: &str, what: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let secs: f64 = text
        .parse()
        .map_err(|_| format!("{what} must be a number of seconds"))?;
    seconds_to_ms(Some(secs), what, "")
        .map_err(|_| format!("{what} must be a positive number of seconds"))
}

/// Milliseconds as the seconds text of the form (`30`, `0.5`).
fn seconds_text(ms: u64) -> String {
    if ms.is_multiple_of(1000) {
        (ms / 1000).to_string()
    } else {
        ms_to_seconds(ms).to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stdio(name: &str) -> McpServerSpec {
        McpServerSpec::new(
            name,
            McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server".into()],
                env: BTreeMap::from([("TOKEN".to_owned(), "${TOKEN}".to_owned())]),
                cwd: None,
            },
        )
    }

    #[test]
    fn the_file_round_trips_through_its_text() {
        let mut file = McpFile::default();
        let mut git = stdio("git");
        git.timeout_ms = Some(30_000);
        git.effect = ToolEffect::Read;
        file.upsert(&git);
        let mut hub = McpServerSpec::new(
            "github",
            McpTransport::Http {
                url: "https://api.githubcopilot.com/mcp/".into(),
                headers: BTreeMap::new(),
                oauth: Some(McpOAuthClient {
                    client_id: String::new(),
                    client_secret: None,
                    scopes: vec!["repo".into()],
                    callback_port: Some(33418),
                }),
            },
        );
        hub.effect = ToolEffect::Network;
        hub.disabled = true;
        file.upsert(&hub);
        let text = file.render();
        assert!(
            text.contains("\"timeout\": 30.0") || text.contains("\"timeout\": 30"),
            "{text}"
        );
        assert!(text.ends_with('\n'));
        let back = McpFile::parse(&text).unwrap();
        assert_eq!(back, file);
        let (specs, problems) = back.into_specs(ConfigOrigin::Workspace);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(specs.len(), 2, "in name order: git, github");
        let github = &specs[1];
        assert_eq!(github.name, "github");
        assert_eq!(github.source, McpSource::Json);
        assert_eq!(github.origin, ConfigOrigin::Workspace);
        assert!(github.disabled);
        assert_eq!(specs[0].timeout_ms, Some(30_000));
        assert_eq!(specs[0].effect, ToolEffect::Read);
        assert_eq!(specs[0].transport, git.transport);
    }

    #[test]
    fn an_empty_text_is_an_empty_file_and_unknown_fields_are_refused() {
        assert_eq!(McpFile::parse("").unwrap(), McpFile::default());
        assert_eq!(McpFile::parse("{}\n").unwrap(), McpFile::default());
        let err = McpFile::parse(r#"{"servers":{"x":{"command":"a","color":"red"}}}"#).unwrap_err();
        assert!(err.contains("color"), "{err}");
        assert!(McpFile::parse("not json").is_err());
    }

    #[test]
    fn a_definition_is_checked_like_the_lua_one() {
        let def = |text: &str| serde_json::from_str::<McpServerDef>(text).unwrap();
        let bad = [
            ("a__b", r#"{"command":"x"}"#, "invalid MCP server name"),
            ("ok", r#"{}"#, "needs either"),
            (
                "ok",
                r#"{"command":"x","url":"https://a"}"#,
                "cannot have both",
            ),
            ("ok", r#"{"url":"ftp://a"}"#, "http:// or https://"),
            ("ok", r#"{"type":"http","command":"x"}"#, "type is http"),
            ("ok", r#"{"command":"x","timeout":0}"#, "positive number"),
        ];
        for (name, text, expected) in bad {
            let err = def(text).into_spec(name, ConfigOrigin::Global).unwrap_err();
            assert!(err.contains(expected), "{name} {text}: {err}");
        }
        let spec = def(r#"{"url":"https://a/mcp","startup_timeout":2.5}"#)
            .into_spec("remote", ConfigOrigin::Global)
            .unwrap();
        assert_eq!(spec.startup_timeout_ms, Some(2500));
        assert_eq!(spec.effect, ToolEffect::Execute);
    }

    #[test]
    fn layers_merge_in_precedence_order_and_a_shadowed_json_server_warns() {
        let mut lua_global = stdio("shared");
        lua_global.effect = ToolEffect::Write;
        let mut lua_ws = stdio("ws");
        lua_ws.origin = ConfigOrigin::Workspace;
        let mut json_global = stdio("shared");
        json_global.source = McpSource::Json;
        let mut json_only = stdio("only");
        json_only.source = McpSource::Json;
        let mut json_ws = stdio("ws");
        json_ws.source = McpSource::Json;
        json_ws.origin = ConfigOrigin::Workspace;
        json_ws.effect = ToolEffect::Read;
        let mut json_ws_over_global = stdio("only");
        json_ws_over_global.source = McpSource::Json;
        json_ws_over_global.origin = ConfigOrigin::Workspace;
        let (merged, warnings) = merge_layers(
            vec![lua_global, lua_ws],
            vec![json_global, json_only],
            vec![json_ws, json_ws_over_global],
        );
        let names: Vec<_> = merged.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["shared", "only", "ws"], "first position is kept");
        assert_eq!(
            merged[0].source,
            McpSource::Lua,
            "global lua wins over global json"
        );
        assert_eq!(merged[0].effect, ToolEffect::Write);
        assert_eq!(
            merged[1].origin,
            ConfigOrigin::Workspace,
            "workspace json wins over global json"
        );
        assert_eq!(
            merged[2].source,
            McpSource::Lua,
            "workspace lua wins over workspace json"
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("`shared` in ~/.config/crest/mcp.json"));
        assert!(warnings[1].contains("`ws` in .crest/mcp.json"));
    }

    #[test]
    fn the_draft_round_trips_a_spec_and_validates() {
        let mut spec = McpServerSpec::new(
            "hub",
            McpTransport::Http {
                url: "https://a/mcp".into(),
                headers: BTreeMap::from([("Authorization".to_owned(), "${T}".to_owned())]),
                oauth: Some(McpOAuthClient {
                    client_id: "id".into(),
                    client_secret: Some("s".into()),
                    scopes: vec!["a".into(), "b".into()],
                    callback_port: Some(4000),
                }),
            },
        );
        spec.origin = ConfigOrigin::Workspace;
        spec.source = McpSource::Json;
        spec.effect = ToolEffect::Network;
        spec.timeout_ms = Some(45_000);
        spec.startup_timeout_ms = Some(1500);
        let draft = McpDraft::from_spec(&spec);
        assert_eq!(draft.kind, McpKind::Http);
        assert_eq!(draft.oauth.as_ref().unwrap().scopes, "a b");
        assert_eq!(draft.timeout_s, "45");
        assert_eq!(draft.startup_timeout_s, "1.5");
        assert_eq!(draft.validate(&[]).unwrap(), spec);

        let stdio_spec = stdio("git");
        let back = McpDraft::from_spec(&stdio_spec).validate(&[]).unwrap();
        assert_eq!(back.transport, stdio_spec.transport);
        assert_eq!(back.source, McpSource::Json);
    }

    #[test]
    fn the_draft_refuses_what_the_mock_refuses() {
        let mut d = McpDraft::new();
        assert!(d.validate(&[]).unwrap_err().contains("letters, digits"));
        d.name = "a__b".into();
        assert!(d.validate(&[]).unwrap_err().contains("__"));
        d.name = "git".into();
        assert!(d.validate(&["git".into()]).unwrap_err().contains("already"));
        assert!(d.validate(&[]).unwrap_err().contains("needs a command"));
        d.kind = McpKind::Http;
        assert!(d.validate(&[]).unwrap_err().contains("http(s) url"));
        d.url = "https://a".into();
        d.oauth = Some(McpOAuthDraft {
            callback_port: "abc".into(),
            ..McpOAuthDraft::default()
        });
        assert!(d.validate(&[]).unwrap_err().contains("callback port"));
        d.oauth = None;
        d.timeout_s = "-1".into();
        assert!(d.validate(&[]).unwrap_err().contains("positive"));
        d.timeout_s = "x".into();
        assert!(d.validate(&[]).unwrap_err().contains("number of seconds"));
        d.timeout_s = " ".into();
        d.headers = vec![(" ".into(), "dropped".into()), ("A".into(), " 1 ".into())];
        let spec = d.validate(&[]).unwrap();
        assert_eq!(spec.timeout_ms, None);
        match spec.transport {
            McpTransport::Http { headers, .. } => {
                assert_eq!(headers, BTreeMap::from([("A".to_owned(), "1".to_owned())]));
            }
            McpTransport::Stdio { .. } => panic!("http"),
        }
    }

    #[test]
    fn params_come_from_the_input_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer", "description": "left" },
                "b": { "type": ["number", "null"] },
                "mode": { "enum": ["fast", "slow"] },
                "tags": { "type": "array", "items": { "type": "string" } },
                "opts": { "anyOf": [{ "type": "object" }, { "type": "null" }] }
            },
            "required": ["a"]
        });
        let params = tool_params(&schema);
        let rows: Vec<(&str, &str, bool)> = params
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str(), p.required))
            .collect();
        assert_eq!(
            rows,
            [
                ("a", "integer", true),
                ("b", "number | null", false),
                ("mode", "fast | slow", false),
                ("tags", "string[]", false),
                ("opts", "object", false),
            ]
        );
        assert_eq!(params[0].description, "left");
        assert!(tool_params(&json!({ "type": "object" })).is_empty());
    }

    #[test]
    fn the_listing_knows_what_each_server_allows() {
        let mut info = McpServerInfo::new(stdio("git"), McpServerState::Ready { tool_count: 1 });
        assert!(!info.editable());
        assert!(info.enabled());
        assert!(!info.has_oauth());
        assert_eq!(info.saved_in, "~/.config/crest/init.lua");
        assert_eq!(info.timeout_s(), DEFAULT_TIMEOUT_S);
        info.spec.source = McpSource::Json;
        info.spec.timeout_ms = Some(1500);
        info.state = McpServerState::Disabled;
        assert!(info.editable());
        assert!(!info.enabled());
        assert_eq!(info.timeout_s(), 2);
        assert_eq!(McpAction::CancelLogin.as_str(), "cancel-login");
        assert!(!McpListing::default().can_edit);
    }
}
