//! `app.mcp`: MCP servers declared in a config file.
//!
//! ```lua
//! app.mcp = {
//!   git  = { command = "mcp-git", args = { "--repo", "." }, env = { A = "1" } },
//!   docs = { url = "https://example.test/mcp", headers = { Authorization = "..." } },
//! }
//! ```
//!
//! Each server has either `command` (a child process speaking over stdio,
//! with optional `args` and `env`) or `url` (streamable HTTP, with optional
//! `headers`).

use std::collections::BTreeMap;

use application::ports::plugin::{McpServerSpec, McpTransport};
use serde_json::{Map, Value};

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

/// Reads the definition of server `name`. Problems go to `problems`; a
/// definition that cannot be used gives `None`.
pub(crate) fn server(name: &str, def: &Value, problems: &mut Vec<String>) -> Option<McpServerSpec> {
    let what = format!("app.mcp.{name}");
    let command = def.get("command").and_then(Value::as_str);
    let url = def.get("url").and_then(Value::as_str);
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
        },
        (None, Some(url)) => McpTransport::Http {
            url: url.to_owned(),
            headers: def
                .get("headers")
                .map(|h| string_map(h, &format!("{what}.headers"), problems))
                .unwrap_or_default(),
        },
        _ => {
            problems.push(format!("{what} needs either `command` or `url`"));
            return None;
        }
    };
    Some(McpServerSpec::new(name, transport))
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
