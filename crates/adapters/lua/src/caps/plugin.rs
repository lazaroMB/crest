//! `app.plugin{ name = ..., capabilities = { ... } }`: who a file is and
//! what it may do beyond the sandbox.
//!
//! ```lua
//! app.plugin{
//!   name = "jira",
//!   capabilities = { "network:jira.example.com", "shell:jq" },
//! }
//! ```
//!
//! Called at most once per file. Malformed entries become diagnostics and
//! are left out; the rest is kept, sorted and without duplicates.

use std::sync::Arc;

use application::ports::plugin::Capability;
use mlua::{Function, Lua, Value};

use super::Declaration;
use crate::config::api::{Origin, Sink, report};

/// Longest plugin name accepted.
const MAX_NAME: usize = 64;

/// Whether `name` can name a plugin: letters, digits, `-` and `_`.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Builds `app.plugin` for one phase.
pub(crate) fn declare(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let (sink, origin) = (Arc::clone(sink), Arc::clone(origin));
    lua.create_function(move |lua, def: Value| {
        let say = |message: String| report(lua, &sink, &origin, message);
        let Value::Table(def) = def else {
            say("app.plugin expects a table: app.plugin{ name = ..., capabilities = { ... } }".into());
            return Ok(());
        };
        if origin.grant.declared().is_some() {
            say("app.plugin may only be called once per file; this call is ignored".into());
            return Ok(());
        }
        let name = match def.get::<Value>("name")? {
            Value::Nil => None,
            Value::String(s) => {
                let name = s.to_str()?.to_owned();
                if valid_name(&name) {
                    Some(name)
                } else {
                    say(format!(
                        "app.plugin: invalid name `{name}`: use letters, digits, `-` and `_` \
                         (at most {MAX_NAME})"
                    ));
                    return Ok(());
                }
            }
            other => {
                say(format!(
                    "app.plugin: `name` must be a string, got {}",
                    other.type_name()
                ));
                return Ok(());
            }
        };
        let mut capabilities = Vec::new();
        match def.get::<Value>("capabilities")? {
            Value::Nil => {}
            Value::Table(list) => {
                for entry in list.sequence_values::<Value>() {
                    match entry? {
                        Value::String(s) => match Capability::parse(&s.to_str()?) {
                            Ok(capability) => capabilities.push(capability),
                            Err(message) => say(format!("app.plugin: {message}")),
                        },
                        other => say(format!(
                            "app.plugin: each capability must be a string, got {}",
                            other.type_name()
                        )),
                    }
                }
            }
            other => {
                say(format!(
                    "app.plugin: `capabilities` must be a list of strings, got {}",
                    other.type_name()
                ));
            }
        }
        for key in def.pairs::<Value, Value>() {
            let (key, _) = key?;
            let known = matches!(&key, Value::String(s)
                if matches!(s.to_str().as_deref(), Ok("name" | "capabilities")));
            if !known {
                say(format!(
                    "app.plugin: unknown field `{}` (use `name` and `capabilities`)",
                    key.to_string().unwrap_or_else(|_| key.type_name().to_owned())
                ));
            }
        }
        capabilities.sort();
        capabilities.dedup();
        origin.grant.declare(Declaration { name, capabilities });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run(code: &str) -> (Phase, Vec<String>) {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        let phase = Phase::new(&lua, "init.lua", "jira").unwrap();
        lua.load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .exec()
            .unwrap();
        let messages = phase
            .take()
            .diagnostics
            .into_iter()
            .map(|d| d.message)
            .collect();
        (phase, messages)
    }

    #[test]
    fn records_valid_capabilities_sorted() {
        let (phase, errors) = run(
            r#"app.plugin{ name = "jira", capabilities = {
                 "shell:jq", "network:jira.example.com", "shell:jq" } }"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let declared = phase.origin().grant.declared().unwrap();
        assert_eq!(declared.name.as_deref(), Some("jira"));
        let caps: Vec<String> = declared
            .capabilities
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(caps, ["network:jira.example.com", "shell:jq"]);
    }

    #[test]
    fn malformed_entries_are_diagnostics() {
        let (phase, errors) = run(
            r#"app.plugin{ capabilities = { "disk:/", 3, "shell:/bin/sh", "shell:git" }, extra = 1 }
               app.plugin{ capabilities = { "shell:rm" } }"#,
        );
        assert_eq!(errors.len(), 5, "{errors:?}");
        assert!(errors[0].contains("unknown capability"), "{errors:?}");
        assert!(errors[3].contains("unknown field `extra`"), "{errors:?}");
        assert!(errors[4].contains("only be called once"), "{errors:?}");
        let declared = phase.origin().grant.declared().unwrap();
        assert_eq!(declared.name, None);
        assert_eq!(declared.capabilities, [Capability::Shell("git".into())]);
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_name("jira-cloud_2"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(""));
        let (phase, errors) = run(r#"app.plugin{ name = "a/b" }"#);
        assert!(errors[0].contains("invalid name"), "{errors:?}");
        assert!(phase.origin().grant.declared().is_none());
    }
}
