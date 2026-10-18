//! Capabilities: `app.plugin{ capabilities = ... }` and the APIs they
//! unlock, `app.http` ([`http`]), `app.shell.run` ([`shell`]) and
//! `app.secret` ([`secret`]).
//!
//! Each config file (phase) has a [`Grant`]: what its `app.plugin` call
//! declared and whether the user approved it. Nothing is approved until
//! the app calls `grant_capabilities` after loading, so calls made while a
//! file loads, or by a file whose declaration was not approved, fail with
//! a Lua error that says how to approve it.
//!
//! The APIs are bound to the file that declared them: before doing
//! anything they check that the nearest Lua function on the call stack
//! was defined in that file (`load` cannot fake a file name, see the
//! sandbox). So a plugin cannot borrow another plugin's capabilities by
//! getting hold of its `app` table or its functions.
//!
//! The calls block the VM thread until they finish; their timeouts (the
//! instruction limit does not count blocked time) bound how long the rest
//! of the app waits for the VM.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use application::ports::plugin::{Capability, PluginCapabilities};
use mlua::{Lua, Table};

use crate::config::api::Origin;

pub(crate) mod http;
pub(crate) mod plugin;
pub(crate) mod secret;
pub(crate) mod shell;

/// What `app.plugin` received in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declaration {
    /// `name`, when given.
    pub(crate) name: Option<String>,
    /// Valid capabilities, sorted and without duplicates.
    pub(crate) capabilities: Vec<Capability>,
}

#[derive(Debug, Default)]
struct GrantState {
    declared: Option<Declaration>,
    /// Who the file is, set once it loaded.
    identity: Option<PluginCapabilities>,
    granted: bool,
}

/// The capabilities of one file and whether they are active.
#[derive(Debug, Default)]
pub(crate) struct Grant {
    state: Mutex<GrantState>,
}

impl Grant {
    fn lock(&self) -> std::sync::MutexGuard<'_, GrantState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `app.plugin` was already called in this file.
    pub(crate) fn declared(&self) -> Option<Declaration> {
        self.lock().declared.clone()
    }

    pub(crate) fn declare(&self, declaration: Declaration) {
        self.lock().declared = Some(declaration);
    }

    /// Records who declared the capabilities, once the file loaded.
    pub(crate) fn set_identity(&self, identity: PluginCapabilities) {
        self.lock().identity = Some(identity);
    }

    /// Activates the capabilities when `granted` holds this declaration.
    pub(crate) fn apply(&self, granted: &[PluginCapabilities]) {
        let mut state = self.lock();
        state.granted = state
            .identity
            .as_ref()
            .is_some_and(|identity| granted.contains(identity));
    }
}

/// What the capability APIs need from the host.
#[derive(Debug, Default)]
pub(crate) struct Context {
    /// Workspace root: the working directory of `app.shell.run`.
    pub(crate) workspace: Option<PathBuf>,
    /// Whether `app.secret` reads the system keyring.
    pub(crate) keyring: bool,
    /// Environment to use instead of the process's.
    env: Option<Vec<(OsString, OsString)>>,
    runtime: OnceLock<Result<tokio::runtime::Runtime, String>>,
}

impl Context {
    pub(crate) fn new(
        workspace: Option<PathBuf>,
        keyring: bool,
        env: Option<Vec<(String, String)>>,
    ) -> Self {
        Self {
            workspace,
            keyring,
            env: env.map(|vars| {
                vars.into_iter()
                    .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                    .collect()
            }),
            runtime: OnceLock::new(),
        }
    }

    /// The environment: the configured one or the process's.
    pub(crate) fn vars(&self) -> Vec<(OsString, OsString)> {
        match &self.env {
            Some(vars) => vars.clone(),
            None => std::env::vars_os().collect(),
        }
    }

    /// One variable of [`Context::vars`].
    pub(crate) fn var(&self, name: &str) -> Option<OsString> {
        match &self.env {
            Some(vars) => vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()),
            None => std::env::var_os(name),
        }
    }

    /// A single-threaded runtime for the I/O of the blocking calls, built
    /// on first use. The VM thread is not inside any other runtime.
    pub(crate) fn block_on<F: std::future::Future>(&self, work: F) -> mlua::Result<F::Output> {
        let runtime = self.runtime.get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("cannot start the I/O runtime: {e}"))
        });
        match runtime {
            Ok(runtime) => Ok(runtime.block_on(work)),
            Err(message) => Err(mlua::Error::runtime(message.clone())),
        }
    }
}

/// The host's [`Context`], stored as the Lua state's app data.
pub(crate) fn context(lua: &Lua) -> mlua::Result<Arc<Context>> {
    lua.app_data_ref::<Arc<Context>>()
        .map(|ctx| Arc::clone(&ctx))
        .ok_or_else(|| mlua::Error::runtime("capabilities are unavailable in this Lua state"))
}

/// Adds `app.plugin`, `app.http`, `app.shell` and `app.secret` to a phase's
/// `app` table.
pub(crate) fn install(
    lua: &Lua,
    app: &Table,
    sink: &crate::config::api::Sink,
    origin: &Arc<Origin>,
) -> mlua::Result<()> {
    app.set("plugin", plugin::declare(lua, sink, origin)?)?;
    app.set("http", http::table(lua, origin)?)?;
    app.set("shell", shell::table(lua, origin)?)?;
    app.set("secret", secret::function(lua, origin)?)?;
    Ok(())
}

/// The source of the nearest Lua function on the stack above the running
/// Rust function, skipping C functions and the sandbox's own wrappers
/// (`pcall`, `xpcall`, `coroutine.resume`).
fn calling_source(lua: &Lua) -> Option<String> {
    (1..)
        .map_while(|level| {
            lua.inspect_stack(level, |debug| {
                let source = debug.source();
                (source.what, source.source.map(|s| s.into_owned()))
            })
        })
        .find(|(what, source)| *what != "C" && source.as_deref() != Some("=sandbox"))
        .and_then(|(_, source)| source)
}

/// What a call needs: its capability as shown to the user, and a test
/// that a declared capability allows it.
pub(crate) type Wanted<'a> = (&'a str, &'a dyn Fn(&Capability) -> bool);

/// Checks that the code calling `api` belongs to the file of `origin` and
/// that its declaration was approved and includes what `wanted` looks for.
/// Returns the approved declaration.
pub(crate) fn authorize(
    lua: &Lua,
    origin: &Origin,
    api: &str,
    wanted: Option<Wanted<'_>>,
) -> mlua::Result<PluginCapabilities> {
    let expected = format!("@{}", origin.file);
    if calling_source(lua).as_deref() != Some(expected.as_str()) {
        return Err(mlua::Error::runtime(format!(
            "{api}: only code written in {} can use its capabilities",
            origin.file
        )));
    }
    let state = origin.grant.lock();
    let Some(identity) = state.identity.clone() else {
        let message = if state.declared.is_some() {
            format!(
                "{api}: capabilities are not active while init.lua loads; use them inside \
                 tool handlers, hooks and callbacks"
            )
        } else {
            format!(
                "{api}: `{}` declares no capabilities; add \
                 app.plugin{{ capabilities = {{ ... }} }} to {}",
                origin.plugin, origin.file
            )
        };
        return Err(mlua::Error::runtime(message));
    };
    let name = &identity.plugin;
    if let Some((what, matches)) = wanted
        && !identity.capabilities.iter().any(matches)
    {
        return Err(mlua::Error::runtime(format!(
            "{api}: plugin `{name}` did not declare {what}; add it to its app.plugin{{ \
             capabilities }} (a changed list must be approved again)"
        )));
    }
    if !state.granted {
        let summary = identity.summary();
        let what = if summary.is_empty() {
            "nothing".to_owned()
        } else {
            summary
        };
        return Err(mlua::Error::runtime(format!(
            "{api}: the capabilities of plugin `{name}` ({what}) are not approved; approve \
             them with `crest --trust-plugin {name}` or `:plugin trust {name}`"
        )));
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::config::api::Phase;
    use crate::sandbox;

    fn lua() -> Lua {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        lua
    }

    #[test]
    fn the_caller_is_the_nearest_lua_function() {
        let lua = lua();
        // Its `pcall` is a Lua wrapper, which must not count as the caller.
        sandbox::Limiter::install(&lua, u64::MAX).unwrap();
        let probe = lua
            .create_function(|lua, ()| Ok(calling_source(lua)))
            .unwrap();
        lua.globals().set("probe", probe).unwrap();
        let direct: Option<String> = lua
            .load("return probe()")
            .set_name("@a.lua")
            .eval()
            .unwrap();
        assert_eq!(direct.as_deref(), Some("@a.lua"));
        let through_pcall: Option<String> = lua
            .load("local ok, s = pcall(probe) return s")
            .set_name("@b.lua")
            .eval()
            .unwrap();
        assert_eq!(through_pcall.as_deref(), Some("@b.lua"));
        let spoofed: Option<String> = lua
            .load("return load('return probe()', '@a.lua')()")
            .set_name("@c.lua")
            .eval()
            .unwrap();
        assert_ne!(spoofed.as_deref(), Some("@a.lua"));
    }

    #[test]
    fn another_file_cannot_use_a_phase_s_functions() {
        let lua = lua();
        let phase = Phase::new(&lua, "a.lua", "a").unwrap();
        lua.globals().set("stolen", phase.app.clone()).unwrap();
        let err = lua
            .load("return stolen.secret('token')")
            .set_name("@b.lua")
            .exec()
            .unwrap_err();
        assert!(
            err.to_string().contains("only code written in a.lua"),
            "{err}"
        );
    }
}
