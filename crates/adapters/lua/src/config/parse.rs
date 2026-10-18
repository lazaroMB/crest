//! Reading the declarative fields of an `app` table after its file ran.
//!
//! Each field is read on its own: a malformed one is skipped with a
//! diagnostic and the rest still apply.

use std::collections::BTreeMap;
use std::str::FromStr;

use application::cost;
use application::ports::plugin::{AppOptions, McpServerSpec, PluginDiagnostic};
use domain::model::{ModelId, ModelRole, ModelSpecPatch, RoleConfig, ThinkingConfig};
use mlua::{Lua, Table};
use serde_json::{Map, Value};

use crate::convert;

/// Declarative settings of one config file.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Declared {
    pub(crate) options: AppOptions,
    pub(crate) roles: RoleConfig,
    pub(crate) mcp_servers: Vec<McpServerSpec>,
    pub(crate) sensitive_paths: Option<Vec<String>>,
    pub(crate) plugins: Vec<String>,
}

/// Collects problems of one file.
struct Report<'a> {
    file: &'a str,
    diagnostics: Vec<PluginDiagnostic>,
}

impl Report<'_> {
    fn push(&mut self, message: impl Into<String>) {
        self.diagnostics.push(PluginDiagnostic {
            source: self.file.to_owned(),
            message: message.into(),
            line: None,
        });
    }
}

/// Reads `app.opt`, `app.models`, `app.mcp`, `app.permissions` and
/// `app.plugins`.
pub(crate) fn read(lua: &Lua, app: &Table, file: &str) -> (Declared, Vec<PluginDiagnostic>) {
    let mut report = Report {
        file,
        diagnostics: Vec::new(),
    };
    let json = |path: &[&str], report: &mut Report<'_>| -> Value {
        match lookup(lua, app, path) {
            Ok(v) => v,
            Err(e) => {
                report.push(format!("app.{}: {e}", path.join(".")));
                Value::Null
            }
        }
    };
    let declared = Declared {
        options: options(json(&["opt"], &mut report), &mut report),
        roles: RoleConfig {
            roles: roles(json(&["models", "roles"], &mut report), &mut report),
            aliases: aliases(json(&["models", "aliases"], &mut report), &mut report),
        },
        mcp_servers: mcp(&json(&["mcp"], &mut report), &mut report),
        sensitive_paths: match json(&["permissions", "sensitive_paths"], &mut report) {
            Value::Null => None,
            v => strings(&v, "app.permissions.sensitive_paths", &mut report),
        },
        plugins: strings(&json(&["plugins"], &mut report), "app.plugins", &mut report)
            .unwrap_or_default(),
    };
    (declared, report.diagnostics)
}

/// `app.<path>` as JSON; `null` when any step is missing.
fn lookup(lua: &Lua, app: &Table, path: &[&str]) -> mlua::Result<Value> {
    let mut value = mlua::Value::Table(app.clone());
    for key in path {
        value = match value {
            mlua::Value::Table(t) => t.get(*key)?,
            _ => return Ok(Value::Null),
        };
    }
    convert::to_json(lua, value)
}

/// Lua has a single table type, so `{}` comes out as an empty object.
fn as_list(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(items) => Some(items),
        Value::Object(map) if map.is_empty() => Some(&[]),
        _ => None,
    }
}

fn as_map(value: &Value) -> Option<&Map<String, Value>> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
    match value {
        Value::Object(map) => Some(map),
        Value::Array(items) if items.is_empty() => Some(&EMPTY),
        _ => None,
    }
}

fn strings(value: &Value, what: &str, report: &mut Report<'_>) -> Option<Vec<String>> {
    let Some(items) = as_list(value) else {
        if !value.is_null() {
            report.push(format!("{what} must be a list of strings"));
        }
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(s) => out.push(s.to_owned()),
            None => report.push(format!("{what}: ignoring non-string entry {item}")),
        }
    }
    Some(out)
}

fn string_map(value: &Value, what: &str, report: &mut Report<'_>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(map) = as_map(value) else {
        if !value.is_null() {
            report.push(format!("{what} must be a table of strings"));
        }
        return out;
    };
    for (k, v) in map {
        match v.as_str() {
            Some(s) => {
                out.insert(k.clone(), s.to_owned());
            }
            None => report.push(format!("{what}.{k} must be a string")),
        }
    }
    out
}

fn options(value: Value, report: &mut Report<'_>) -> AppOptions {
    let mut out = AppOptions::default();
    let Some(map) = as_map(&value) else {
        if !value.is_null() {
            report.push("app.opt must be a table");
        }
        return out;
    };
    for (key, v) in map {
        match key.as_str() {
            "leader" => match v.as_str() {
                Some(s) => out.leader = Some(s.to_owned()),
                None => report.push("app.opt.leader must be a string"),
            },
            "theme" => match v.as_str() {
                Some(s) => out.theme = Some(s.to_owned()),
                None => report.push("app.opt.theme must be a string"),
            },
            "timeoutlen" => match v.as_u64() {
                Some(ms) => out.timeoutlen_ms = Some(ms),
                None => report.push("app.opt.timeoutlen must be a whole number of milliseconds"),
            },
            // Kept in `extra` for `CostTracker::from_options`, once valid.
            cost::PRICING_OPTION | cost::CAP_OPTION => {
                let checked = if key == cost::CAP_OPTION {
                    cost::cap_from_json(v).map(drop)
                } else {
                    cost::PricingTable::from_json(v).map(drop)
                };
                match checked {
                    Ok(()) => {
                        out.extra.insert(key.clone(), v.clone());
                    }
                    Err(e) => report.push(e.to_string()),
                }
            }
            _ => {
                out.extra.insert(key.clone(), v.clone());
            }
        }
    }
    out
}

fn aliases(value: Value, report: &mut Report<'_>) -> BTreeMap<String, ModelId> {
    string_map(&value, "app.models.aliases", report)
        .into_iter()
        .map(|(k, v)| (k, ModelId::new(v)))
        .collect()
}

fn roles(value: Value, report: &mut Report<'_>) -> BTreeMap<ModelRole, ModelSpecPatch> {
    let mut out = BTreeMap::new();
    let Some(map) = as_map(&value) else {
        if !value.is_null() {
            report.push("app.models.roles must be a table");
        }
        return out;
    };
    for (name, spec) in map {
        let Ok(role) = ModelRole::from_str(name) else {
            report.push(format!("app.models.roles: unknown role `{name}`"));
            continue;
        };
        if let Some(patch) = role_patch(name, spec, report) {
            out.insert(role, patch);
        }
    }
    out
}

fn role_patch(role: &str, spec: &Value, report: &mut Report<'_>) -> Option<ModelSpecPatch> {
    let what = format!("app.models.roles.{role}");
    // `main = "opus"` is short for `main = { id = "opus" }`.
    if let Some(id) = spec.as_str() {
        return Some(ModelSpecPatch {
            id: Some(ModelId::new(id)),
            ..ModelSpecPatch::default()
        });
    }
    let Some(map) = as_map(spec) else {
        report.push(format!("{what} must be a table or a model id"));
        return None;
    };
    let mut patch = ModelSpecPatch::default();
    for (key, v) in map {
        match key.as_str() {
            "id" => match v.as_str() {
                Some(id) => patch.id = Some(ModelId::new(id)),
                None => report.push(format!("{what}.id must be a string")),
            },
            "max_tokens" => match v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                Some(n) => patch.max_tokens = Some(n),
                None => report.push(format!("{what}.max_tokens must be a positive whole number")),
            },
            "thinking" => match thinking(v) {
                Some(t) => patch.thinking = Some(t),
                None => report.push(format!(
                    "{what}.thinking must be false, \"adaptive\", a budget or {{ budget = N }}"
                )),
            },
            "fallbacks" => {
                if let Some(list) = strings(v, &format!("{what}.fallbacks"), report) {
                    patch.fallbacks = Some(list.into_iter().map(ModelId::new).collect());
                }
            }
            other => report.push(format!("{what}: unknown field `{other}`")),
        }
    }
    Some(patch)
}

fn budget(value: &Value) -> Option<ThinkingConfig> {
    let budget_tokens = u32::try_from(value.as_u64()?).ok()?;
    Some(ThinkingConfig::Enabled { budget_tokens })
}

fn thinking(value: &Value) -> Option<ThinkingConfig> {
    match value {
        Value::Bool(false) => Some(ThinkingConfig::Disabled),
        Value::Number(_) => budget(value),
        Value::String(s) => match s.as_str() {
            "disabled" | "off" => Some(ThinkingConfig::Disabled),
            "adaptive" => Some(ThinkingConfig::Adaptive),
            _ => None,
        },
        Value::Object(map) => {
            if let Some(b) = map.get("budget").or_else(|| map.get("budget_tokens")) {
                return budget(b);
            }
            match map.get("type").and_then(Value::as_str) {
                Some("disabled") => Some(ThinkingConfig::Disabled),
                Some("adaptive") => Some(ThinkingConfig::Adaptive),
                _ => None,
            }
        }
        _ => None,
    }
}

fn mcp(value: &Value, report: &mut Report<'_>) -> Vec<McpServerSpec> {
    let mut problems = Vec::new();
    let servers = crate::api::mcp::servers(value, &mut problems);
    for message in problems {
        report.push(message);
    }
    servers
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::McpTransport;
    use serde_json::json;

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::config::api::Phase;
    use crate::sandbox;

    fn read_code(code: &str) -> (Declared, Vec<PluginDiagnostic>) {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        let phase = Phase::new(&lua, "init.lua", "config").unwrap();
        lua.load(code)
            .set_environment(phase.env.clone())
            .exec()
            .unwrap();
        read(&lua, &phase.app, "init.lua")
    }

    #[test]
    fn an_untouched_app_declares_nothing() {
        let (declared, diags) = read_code("");
        assert_eq!(declared, Declared::default());
        assert!(diags.is_empty());
    }

    #[test]
    fn reads_options_into_known_fields_and_extra() {
        let (declared, diags) = read_code(
            r#"app.opt.leader = " "; app.opt.theme = "tokyonight"
               app.opt.timeoutlen = 300; app.opt.wrap = true"#,
        );
        assert!(diags.is_empty(), "{diags:?}");
        let o = declared.options;
        assert_eq!(o.leader.as_deref(), Some(" "));
        assert_eq!(o.theme.as_deref(), Some("tokyonight"));
        assert_eq!(o.timeoutlen_ms, Some(300));
        assert_eq!(o.extra.get("wrap"), Some(&json!(true)));
    }

    #[test]
    fn checks_pricing_and_the_spending_cap() {
        let (declared, diags) = read_code(
            r#"app.opt.pricing = {
                 ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
               }
               app.opt.max_session_cost = 2.5"#,
        );
        assert!(diags.is_empty(), "{diags:?}");
        let o = declared.options;
        assert_eq!(o.extra.get("max_session_cost"), Some(&json!(2.5)));
        assert_eq!(
            o.extra["pricing"]["claude-sonnet-5"]["cache_write"],
            json!(3.75)
        );

        let (declared, diags) = read_code(
            r#"app.opt.pricing = { m = { input = 1 } }
               app.opt.max_session_cost = "mucho""#,
        );
        assert!(declared.options.extra.is_empty());
        let messages: Vec<_> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "app.opt.max_session_cost must be a positive number of USD",
                "app.opt.pricing[\"m\"]: `output` must be a number (USD per million tokens)",
            ]
        );
    }

    #[test]
    fn reads_the_role_example_of_the_plan() {
        let (declared, diags) = read_code(
            r#"app.models.roles = {
                 main    = { id = "claude-sonnet-5-5", max_tokens = 16000,
                             thinking = { budget = 8000 }, fallbacks = { "claude-haiku-5-5" } },
                 plan    = { id = "claude-opus-5-5", max_tokens = 32000, thinking = { budget = 16000 } },
                 explore = { id = "claude-haiku-5-5", max_tokens = 8000 },
                 compact = { id = "claude-haiku-5-5", max_tokens = 4000 },
               }
               app.models.aliases = { opus = "claude-opus-5-5", sonnet = "claude-sonnet-5-5", haiku = "claude-haiku-5-5" }"#,
        );
        assert!(diags.is_empty(), "{diags:?}");
        let roles = &declared.roles;
        assert_eq!(roles.roles.len(), 4);
        assert_eq!(
            roles.role(ModelRole::Main),
            Some(&ModelSpecPatch {
                id: Some("claude-sonnet-5-5".into()),
                max_tokens: Some(16000),
                thinking: Some(ThinkingConfig::Enabled {
                    budget_tokens: 8000
                }),
                fallbacks: Some(vec!["claude-haiku-5-5".into()]),
            })
        );
        assert_eq!(
            roles.role(ModelRole::Explore),
            Some(&ModelSpecPatch {
                id: Some("claude-haiku-5-5".into()),
                max_tokens: Some(8000),
                ..ModelSpecPatch::default()
            })
        );
        assert_eq!(roles.alias("opus"), Some(&ModelId::new("claude-opus-5-5")));
    }

    #[test]
    fn thinking_accepts_several_spellings() {
        for (lua, want) in [
            ("false", ThinkingConfig::Disabled),
            ("'adaptive'", ThinkingConfig::Adaptive),
            (
                "4000",
                ThinkingConfig::Enabled {
                    budget_tokens: 4000,
                },
            ),
            (
                "{ budget_tokens = 10 }",
                ThinkingConfig::Enabled { budget_tokens: 10 },
            ),
            ("{ type = 'adaptive' }", ThinkingConfig::Adaptive),
        ] {
            let (declared, diags) =
                read_code(&format!("app.models.roles.deep = {{ thinking = {lua} }}"));
            assert!(diags.is_empty(), "{lua}: {diags:?}");
            assert_eq!(
                declared
                    .roles
                    .role(ModelRole::Deep)
                    .and_then(|p| p.thinking),
                Some(want),
                "{lua}"
            );
        }
    }

    #[test]
    fn a_string_role_is_just_an_id() {
        let (declared, _) = read_code("app.models.roles.title = 'haiku'");
        assert_eq!(
            declared
                .roles
                .role(ModelRole::Title)
                .and_then(|p| p.id.clone()),
            Some(ModelId::new("haiku"))
        );
    }

    #[test]
    fn bad_fields_are_reported_and_the_rest_applies() {
        let (declared, diags) = read_code(
            r#"app.models.roles = { boss = { id = "x" }, main = { max_tokens = "lots", id = "m" } }
               app.opt.leader = 5; app.opt.theme = "ok""#,
        );
        assert_eq!(declared.options.theme.as_deref(), Some("ok"));
        assert_eq!(declared.options.leader, None);
        assert_eq!(
            declared
                .roles
                .role(ModelRole::Main)
                .and_then(|p| p.id.clone()),
            Some(ModelId::new("m"))
        );
        let messages: Vec<_> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("unknown role `boss`")));
        assert!(messages.iter().any(|m| m.contains("max_tokens")));
        assert!(messages.iter().any(|m| m.contains("leader")));
    }

    #[test]
    fn reads_mcp_servers_permissions_and_plugins() {
        let (declared, diags) = read_code(
            r#"app.mcp = {
                 git = { command = "mcp-git", args = { "--repo", "." }, env = { A = "1" } },
                 docs = { url = "https://example.test/mcp", headers = {} },
                 broken = { args = {} },
               }
               app.permissions.sensitive_paths = { ".env*", "secrets/" }
               app.plugins = { "~/src/my-plugin" }"#,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("app.mcp.broken"));
        assert_eq!(
            declared.mcp_servers,
            vec![
                McpServerSpec::new(
                    "docs",
                    McpTransport::Http {
                        url: "https://example.test/mcp".into(),
                        headers: BTreeMap::new(),
                    },
                ),
                McpServerSpec::new(
                    "git",
                    McpTransport::Stdio {
                        command: "mcp-git".into(),
                        args: vec!["--repo".into(), ".".into()],
                        env: BTreeMap::from([("A".into(), "1".into())]),
                    },
                ),
            ]
        );
        assert_eq!(
            declared.sensitive_paths,
            Some(vec![".env*".to_owned(), "secrets/".to_owned()])
        );
        assert_eq!(declared.plugins, ["~/src/my-plugin"]);
    }

    #[test]
    fn an_empty_sensitive_paths_list_is_kept() {
        let (declared, _) = read_code("app.permissions.sensitive_paths = {}");
        assert_eq!(declared.sensitive_paths, Some(Vec::new()));
    }
}
