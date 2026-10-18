//! `app.secret(name)`: a credential of the calling plugin, or `nil`.
//!
//! Looked up first in the system keyring (service `crest`, entry
//! `plugin:<plugin>:<name>`), then in the environment variable
//! `CREST_PLUGIN_<PLUGIN>_<NAME>` (uppercase, every character other than a
//! letter or digit turned into `_`). Only a plugin whose capabilities were
//! approved can read its secrets, and only its own. Values are never
//! logged.

use std::sync::Arc;

use mlua::{Function, Lua};

use super::{Context, authorize, context};
use crate::config::api::Origin;

/// Keyring service of crest's secrets.
pub(crate) const KEYRING_SERVICE: &str = "crest";

const API: &str = "app.secret";

/// Keyring entry of `name` for `plugin`.
pub(crate) fn keyring_user(plugin: &str, name: &str) -> String {
    format!("plugin:{plugin}:{name}")
}

/// Environment variable of `name` for `plugin`.
pub(crate) fn env_var(plugin: &str, name: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("CREST_PLUGIN_{}_{}", clean(plugin), clean(name))
}

fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Builds `app.secret` for one phase.
pub(crate) fn function(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let origin = Arc::clone(origin);
    lua.create_function(move |lua, name: String| {
        if !valid(&name) {
            return Err(mlua::Error::runtime(format!(
                "{API}: invalid name `{name}`: use letters, digits, `-`, `_` and `.`"
            )));
        }
        let identity = authorize(lua, &origin, API, None)?;
        let ctx: Arc<Context> = context(lua)?;
        Ok(lookup(&ctx, &identity.plugin, &name))
    })
}

fn lookup(ctx: &Context, plugin: &str, name: &str) -> Option<String> {
    if ctx.keyring {
        match keyring::Entry::new(KEYRING_SERVICE, &keyring_user(plugin, name))
            .and_then(|entry| entry.get_password())
        {
            Ok(value) => return Some(value),
            Err(err) => tracing::debug!(plugin, name, error = %err, "plugin secret not in the keyring"),
        }
    }
    ctx.var(&env_var(plugin, name))
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map_to_keyring_entries_and_env_vars() {
        assert_eq!(keyring_user("jira", "token"), "plugin:jira:token");
        assert_eq!(env_var("jira-cloud", "api.token"), "CREST_PLUGIN_JIRA_CLOUD_API_TOKEN");
        assert!(valid("api.token-2"));
        assert!(!valid("a b") && !valid(""));
    }
}
