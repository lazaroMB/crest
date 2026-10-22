//! `app.mcp`: MCP servers declared in a config file.
//!
//! ```lua
//! app.mcp = {
//!   git  = { command = "mcp-git", args = { "--repo", "." }, env = { A = "1" } },
//!   docs = { url = "https://example.test/mcp", headers = { Authorization = "..." } },
//! }
//! ```
//!
//! or, one at a time, `app.mcp.add("git", { command = "mcp-git" })`.
//!
//! Each server has either `command` (a child process speaking over stdio,
//! with optional `args`, `env` and `cwd`, which defaults to the workspace)
//! or `url` (streamable HTTP, with optional `headers` and `oauth`, a
//! pre-registered client `{ client_id, client_secret, scopes,
//! callback_port }` for servers without dynamic registration). `${VAR}`
//! and `${VAR:-default}` in `args`, `env`, `url` and `headers` are read
//! from the environment when the server starts, so secrets stay out of
//! the config. Both take:
//!
//! - `effect`: what calling its tools does, for the permission policy:
//!   `"read"`, `"write"`, `"execute"` (the default, so every call asks like
//!   a command) or `"network"`;
//! - `timeout`: limit for one tool call, in seconds;
//! - `startup_timeout`: limit for starting it, in seconds (the first
//!   `npx -y` downloads the package);
//! - `disabled`: `true` declares it without starting it (`/mcp enable`);
//! - `type`: `"stdio"` or `"http"`, optional, checked against the rest.
//!
//! Server names become part of tool names (`mcp__<server>__<tool>`), so
//! they may only have letters, digits, `-` and single `_`.

use std::collections::BTreeMap;

use std::sync::Arc;

use application::ports::plugin::{McpOAuthClient, McpServerSpec, McpTransport};
use domain::permission::ToolEffect;
use mlua::{Function, Lua, Table};
use serde_json::{Map, Value};

use crate::config::api::{Origin, Sink, lock, report};
use crate::convert;

fn as_map(value: &Value) -> Option<&Map<String, Value>> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
    match value {
        Value::Object(map) => Some(map),
        // Lua has a single table type, so `{}` may come out as a list.
        Value::Array(items) if items.is_empty() => Some(&EMPTY),
        _ => None,
    }
}

fn strings(value: &Value, what: &str, problems: &mut Vec<String>) -> Vec<String> {
    let items = match value {
        Value::Array(items) => items.as_slice(),
        Value::Object(map) if map.is_empty() => &[],
        _ => {
            problems.push(format!("{what} must be a list of strings"));
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(s) => out.push(s.to_owned()),
            None => problems.push(format!("{what}: ignoring non-string entry {item}")),
        }
    }
    out
}

fn string_map(value: &Value, what: &str, problems: &mut Vec<String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(map) = as_map(value) else {
        problems.push(format!("{what} must be a table of strings"));
        return out;
    };
    for (k, v) in map {
        match v.as_str() {
            Some(s) => {
                out.insert(k.clone(), s.to_owned());
            }
            None => problems.push(format!("{what}.{k} must be a string")),
        }
    }
    out
}

/// Fields a server definition may have.
const FIELDS: &[&str] = &[
    "command",
    "args",
    "env",
    "cwd",
    "url",
    "headers",
    "oauth",
    "effect",
    "timeout",
    "startup_timeout",
    "disabled",
    "type",
];

/// Fields of `oauth`.
const OAUTH_FIELDS: &[&str] = &["client_id", "client_secret", "scopes", "callback_port"];

/// The pre-registered OAuth client of `def.oauth`, if any.
fn oauth(value: &Value, what: &str, problems: &mut Vec<String>) -> Option<McpOAuthClient> {
    let what = format!("{what}.oauth");
    let Some(map) = as_map(value) else {
        problems.push(format!("{what} must be a table"));
        return None;
    };
    for key in map.keys().filter(|k| !OAUTH_FIELDS.contains(&k.as_str())) {
        problems.push(format!("{what}: unknown field `{key}`"));
    }
    let client_id = match text(value, "client_id", &what) {
        Ok(Some(id)) => id.to_owned(),
        Ok(None) => {
            problems.push(format!("{what}.client_id is required"));
            return None;
        }
        Err(e) => {
            problems.push(e);
            return None;
        }
    };
    let client_secret = text(value, "client_secret", &what)
        .unwrap_or_else(|e| {
            problems.push(e);
            None
        })
        .map(str::to_owned);
    let scopes = value
        .get("scopes")
        .map(|s| strings(s, &format!("{what}.scopes"), problems))
        .unwrap_or_default();
    let callback_port = match value.get("callback_port") {
        None | Some(Value::Null) => None,
        Some(port) => match port.as_u64().and_then(|p| u16::try_from(p).ok()) {
            Some(port) if port > 0 => Some(port),
            _ => {
                problems.push(format!("{what}.callback_port must be a port number"));
                None
            }
        },
    };
    Some(McpOAuthClient {
        client_id,
        client_secret,
        scopes,
        callback_port,
    })
}

/// Why `name` cannot name a server, if it cannot. Tools are offered as
/// `mcp__<server>__<tool>`, so `__` would make the name ambiguous.
fn invalid_name(name: &str) -> Option<String> {
    let valid_chars = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (name.is_empty() || !valid_chars || name.contains("__")).then(|| {
        format!("invalid MCP server name `{name}` (use letters, digits, `-` and single `_`)")
    })
}

/// A non-empty string field; `Ok(None)` when absent.
fn text<'a>(def: &'a Value, key: &str, what: &str) -> Result<Option<&'a str>, String> {
    match def.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s)),
        Some(_) => Err(format!("{what}.{key} must be a non-empty string")),
    }
}

fn effect(value: &Value, what: &str) -> Result<ToolEffect, String> {
    match value.as_str() {
        Some("read") => Ok(ToolEffect::Read),
        Some("write") => Ok(ToolEffect::Write),
        Some("execute") => Ok(ToolEffect::Execute),
        Some("network") => Ok(ToolEffect::Network),
        _ => Err(format!(
            "{what}.effect: unknown effect {value} (use \"read\", \"write\", \"execute\" or \"network\")"
        )),
    }
}

/// A timeout field in seconds, as milliseconds.
fn timeout_ms(value: &Value, what: &str, field: &str) -> Result<u64, String> {
    match value.as_f64() {
        Some(secs) if secs > 0.0 && secs.is_finite() => {
            // Saturates for absurd values, which is what we want.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ms = (secs * 1000.0).ceil() as u64;
            Ok(ms)
        }
        _ => Err(format!(
            "{what}.{field} must be a positive number of seconds, got {value}"
        )),
    }
}

/// Reads the definition of server `name`. Problems go to `problems`; a
/// definition that cannot be used gives `None`, a bad optional field is
/// dropped with its problem.
pub(crate) fn server(name: &str, def: &Value, problems: &mut Vec<String>) -> Option<McpServerSpec> {
    let what = format!("app.mcp.{name}");
    if let Some(problem) = invalid_name(name) {
        problems.push(problem);
        return None;
    }
    let Some(fields) = def.as_object() else {
        problems.push(format!("{what} must be a table with `command` or `url`"));
        return None;
    };
    for key in fields.keys().filter(|k| !FIELDS.contains(&k.as_str())) {
        problems.push(format!("{what}: unknown field `{key}`"));
    }
    let (command, url) = match (text(def, "command", &what), text(def, "url", &what)) {
        (Ok(command), Ok(url)) => (command, url),
        (Err(e), _) | (_, Err(e)) => {
            problems.push(e);
            return None;
        }
    };
    if let Some(kind) = def.get("type").filter(|v| !v.is_null()) {
        let expected = if command.is_some() { "stdio" } else { "http" };
        match kind.as_str() {
            Some(kind) if kind == expected => {}
            Some("stdio" | "local") if expected == "stdio" => {}
            Some("http" | "streamable-http" | "remote") if expected == "http" => {}
            _ => problems.push(format!(
                "{what}.type is {kind} but the server has {}",
                if command.is_some() {
                    "`command` (stdio)"
                } else {
                    "`url` (http)"
                }
            )),
        }
    }
    let transport = match (command, url) {
        (Some(command), None) => McpTransport::Stdio {
            command: command.to_owned(),
            args: def
                .get("args")
                .map(|a| strings(a, &format!("{what}.args"), problems))
                .unwrap_or_default(),
            env: def
                .get("env")
                .map(|e| string_map(e, &format!("{what}.env"), problems))
                .unwrap_or_default(),
            cwd: text(def, "cwd", &what)
                .unwrap_or_else(|e| {
                    problems.push(e);
                    None
                })
                .map(str::to_owned),
        },
        (None, Some(url)) => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                problems.push(format!("{what}.url must start with http:// or https://"));
                return None;
            }
            McpTransport::Http {
                url: url.to_owned(),
                headers: def
                    .get("headers")
                    .map(|h| string_map(h, &format!("{what}.headers"), problems))
                    .unwrap_or_default(),
                oauth: def
                    .get("oauth")
                    .filter(|o| !o.is_null())
                    .and_then(|o| oauth(o, &what, problems)),
            }
        }
        (Some(_), Some(_)) => {
            problems.push(format!("{what} cannot have both `command` and `url`"));
            return None;
        }
        (None, None) => {
            problems.push(format!("{what} needs either `command` or `url`"));
            return None;
        }
    };
    let mut spec = McpServerSpec::new(name, transport);
    if let Some(value) = def.get("effect") {
        match effect(value, &what) {
            Ok(effect) => spec.effect = effect,
            Err(e) => problems.push(e),
        }
    }
    if let Some(value) = def.get("timeout") {
        match timeout_ms(value, &what, "timeout") {
            Ok(ms) => spec.timeout_ms = Some(ms),
            Err(e) => problems.push(e),
        }
    }
    if let Some(value) = def.get("startup_timeout") {
        match timeout_ms(value, &what, "startup_timeout") {
            Ok(ms) => spec.startup_timeout_ms = Some(ms),
            Err(e) => problems.push(e),
        }
    }
    match def.get("disabled") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(disabled)) => spec.disabled = *disabled,
        Some(_) => problems.push(format!("{what}.disabled must be true or false")),
    }
    Some(spec)
}

/// Reads the table form, `app.mcp = { <name> = <definition>, ... }`.
pub(crate) fn servers(value: &Value, problems: &mut Vec<String>) -> Vec<McpServerSpec> {
    let Some(map) = as_map(value) else {
        if !value.is_null() {
            problems.push("app.mcp must be a table of servers by name".into());
        }
        return Vec::new();
    };
    map.iter()
        .filter_map(|(name, def)| server(name, def, problems))
        .collect()
}

/// Builds the `app.mcp` table of one phase, with its `add` function.
pub(crate) fn table(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let mcp = lua.create_table()?;
    mcp.set("add", add(lua, sink, origin)?)?;
    Ok(mcp)
}

/// `app.mcp.add(name, definition)`.
fn add(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, (name, def): (mlua::Value, mlua::Value)| {
        let (mlua::Value::String(name), mlua::Value::Table(_)) = (&name, &def) else {
            report(
                lua,
                &sink,
                &origin,
                "mcp.add expects a name and a table with `command` or `url`".into(),
            );
            return Ok(());
        };
        let name = name.to_str()?.to_owned();
        let def = convert::to_json(lua, def)?;
        let mut problems = Vec::new();
        let spec = server(&name, &def, &mut problems);
        for message in problems {
            report(lua, &sink, &origin, message);
        }
        if let Some(spec) = spec {
            lock(&sink).mcp_servers.push(spec);
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read(name: &str, def: Value) -> (Option<McpServerSpec>, Vec<String>) {
        let mut problems = Vec::new();
        let spec = server(name, &def, &mut problems);
        (spec, problems)
    }

    #[test]
    fn reads_effect_and_timeout() {
        let (spec, problems) = read(
            "echo",
            json!({ "command": "echo-server", "effect": "read", "timeout": 2.5 }),
        );
        assert!(problems.is_empty(), "{problems:?}");
        let spec = spec.unwrap();
        assert_eq!(spec.effect, ToolEffect::Read);
        assert_eq!(spec.timeout_ms, Some(2500));
    }

    #[test]
    fn defaults_to_execute_without_timeout() {
        let (spec, _) = read("docs", json!({ "url": "https://example.test/mcp" }));
        let spec = spec.unwrap();
        assert_eq!(spec.effect, ToolEffect::Execute);
        assert_eq!(spec.timeout_ms, None);
    }

    #[test]
    fn rejects_unusable_definitions() {
        for (name, def, needle) in [
            ("a__b", json!({ "command": "x" }), "invalid MCP server name"),
            ("a b", json!({ "command": "x" }), "invalid MCP server name"),
            ("", json!({ "command": "x" }), "invalid MCP server name"),
            (
                "both",
                json!({ "command": "x", "url": "https://h" }),
                "both",
            ),
            ("none", json!({ "args": [] }), "either"),
            ("num", json!({ "command": 3 }), "non-empty string"),
            ("blank", json!({ "command": " " }), "non-empty string"),
            ("ftp", json!({ "url": "ftp://h" }), "http://"),
            ("str", json!("mcp-git"), "must be a table"),
        ] {
            let (spec, problems) = read(name, def);
            assert!(spec.is_none(), "{name}");
            assert!(
                problems.iter().any(|p| p.contains(needle)),
                "{name}: {problems:?}"
            );
        }
    }

    #[test]
    fn bad_optional_fields_are_dropped_with_a_problem() {
        let (spec, problems) = read(
            "echo",
            json!({ "command": "x", "effect": "maybe", "timeout": -1, "workdir": "/" }),
        );
        let spec = spec.unwrap();
        assert_eq!(spec.effect, ToolEffect::Execute);
        assert_eq!(spec.timeout_ms, None);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("unknown field `workdir`"))
        );
    }

    #[test]
    fn reads_cwd_startup_timeout_disabled_and_oauth() {
        let (spec, problems) = read(
            "fs",
            json!({ "type": "stdio", "command": "npx", "cwd": "/tmp", "startup_timeout": 90, "disabled": true }),
        );
        assert!(problems.is_empty(), "{problems:?}");
        let spec = spec.unwrap();
        assert!(
            matches!(&spec.transport, McpTransport::Stdio { cwd: Some(cwd), .. } if cwd == "/tmp")
        );
        assert_eq!(spec.startup_timeout_ms, Some(90_000));
        assert!(spec.disabled);

        let (spec, problems) = read(
            "gh",
            json!({
                "type": "http",
                "url": "https://api.githubcopilot.com/mcp/",
                "oauth": { "client_id": "Iv1.abc", "scopes": ["repo"], "callback_port": 33418 },
            }),
        );
        assert!(problems.is_empty(), "{problems:?}");
        let McpTransport::Http {
            oauth: Some(oauth), ..
        } = spec.unwrap().transport
        else {
            panic!("expected an OAuth client")
        };
        assert_eq!(oauth.client_id, "Iv1.abc");
        assert_eq!(oauth.scopes, ["repo"]);
        assert_eq!(oauth.callback_port, Some(33418));

        let (_, problems) = read("t", json!({ "type": "http", "command": "x" }));
        assert!(problems.iter().any(|p| p.contains(".type")), "{problems:?}");
        let (_, problems) = read(
            "o",
            json!({ "url": "https://h", "oauth": { "scopes": [] } }),
        );
        assert!(
            problems.iter().any(|p| p.contains("client_id is required")),
            "{problems:?}"
        );
    }
}
