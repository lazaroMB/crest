//! The restricted Lua environment and the per-call instruction limit.
//!
//! Config files and plugins run with `string`, `table`, `math`, `utf8`,
//! `coroutine` and a reduced `os` (`time`, `clock`, `date`, `difftime`).
//! There is no `io`, no `debug`, no `package`, no file loading (`dofile`,
//! `loadfile`, `require` of paths), no bytecode (`load` only takes text and
//! `string.dump` is gone) and no `os.execute`/`remove`/`rename`/`exit`/
//! `getenv`/`tmpname`/`setlocale`. `os.getenv` is removed because the
//! environment holds secrets such as the API key. `print` goes to the log.
//!
//! The global `require` refuses every module; each config file gets its own
//! `require` that only returns its `app` table.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use application::ports::plugin::PluginError;
use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Table, Variadic, VmState};

use crate::convert;

/// Instructions between two checks of the budget.
const CHECK_EVERY: u32 = 10_000;

/// `os` functions kept in the sandbox.
const OS_KEEP: [&str; 4] = ["time", "clock", "date", "difftime"];

/// Base functions removed from the sandbox.
const BASE_REMOVE: [&str; 2] = ["dofile", "loadfile"];

/// Replaces `load` with a version that only accepts text chunks.
const TEXT_ONLY_LOAD: &str = r##"
local raw_load = load
return function(chunk, name, _mode, ...)
  if select("#", ...) == 0 then
    return raw_load(chunk, name, "t")
  end
  return raw_load(chunk, name, "t", ...)
end
"##;

/// Creates a sandboxed Lua state.
pub(crate) fn new_lua(memory_limit: Option<usize>) -> mlua::Result<Lua> {
    let libs = StdLib::COROUTINE
        | StdLib::TABLE
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::MATH
        | StdLib::OS;
    let lua = Lua::new_with(libs, LuaOptions::default())?;
    restrict(&lua)?;
    if let Some(limit) = memory_limit {
        lua.set_memory_limit(limit)?;
    }
    Ok(lua)
}

fn restrict(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    for name in BASE_REMOVE {
        globals.raw_set(name, mlua::Nil)?;
    }
    let load: mlua::Function = lua.load(TEXT_ONLY_LOAD).set_name("=sandbox").eval()?;
    globals.raw_set("load", load)?;

    let os: Table = globals.get("os")?;
    let safe_os = lua.create_table()?;
    for name in OS_KEEP {
        safe_os.raw_set(name, os.raw_get::<mlua::Value>(name)?)?;
    }
    globals.raw_set("os", safe_os)?;

    let string: Table = globals.get("string")?;
    string.raw_set("dump", mlua::Nil)?;

    globals.raw_set(
        "print",
        lua.create_function(|_, args: Variadic<mlua::Value>| {
            let line: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| v.type_name().to_owned()))
                .collect();
            tracing::info!(target: "lua", "{}", line.join("\t"));
            Ok(())
        })?,
    )?;

    globals.raw_set("require", lua.create_function(require)?)?;
    Ok(())
}

fn require(_: &Lua, name: String) -> mlua::Result<()> {
    Err(mlua::Error::runtime(format!(
        "module '{name}' not found (only require(\"app\") is available)"
    )))
}

/// Wraps the functions that catch errors so they re-raise the limit error.
const RETHROW_LIMIT: &str = r#"
local raw_pcall, raw_xpcall, raw_resume, exceeded = ...
local function check(...)
  if exceeded() then error("instruction limit exceeded", 0) end
  return ...
end
pcall = function(...) return check(raw_pcall(...)) end
xpcall = function(...) return check(raw_xpcall(...)) end
coroutine.resume = function(...) return check(raw_resume(...)) end
"#;

/// Why a guarded call failed.
#[derive(Debug)]
pub(crate) enum Failure {
    /// It ran out of instructions.
    InstructionLimit,
    /// Lua raised an error.
    Lua(mlua::Error),
}

/// Stops a call that runs more than a fixed number of instructions.
///
/// The budget is refilled at the start of every [`Limiter::guard`]. Once it
/// runs out, the hook raises an error at every check until the call
/// returns, and `pcall`, `xpcall` and `coroutine.resume` re-raise it, so a
/// plugin cannot catch it and keep looping. The call is reported as
/// [`PluginError::InstructionLimit`] even if Lua swallowed the error.
#[derive(Debug, Clone)]
pub(crate) struct Limiter {
    limit: i64,
    state: Arc<LimiterState>,
}

#[derive(Debug, Default)]
struct LimiterState {
    remaining: AtomicI64,
    exceeded: AtomicBool,
}

impl Limiter {
    /// Installs the instruction hook on `lua`.
    pub(crate) fn install(lua: &Lua, limit: u64) -> mlua::Result<Self> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX).max(1);
        let step = u32::try_from(limit)
            .unwrap_or(CHECK_EVERY)
            .clamp(1, CHECK_EVERY);
        let state = Arc::new(LimiterState::default());
        state.remaining.store(limit, Ordering::Relaxed);
        let hook_state = Arc::clone(&state);
        // A global hook, unlike `set_hook`, also runs inside coroutines
        // created from Lua, which inherit it.
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(step),
            move |_, _| {
                let left = hook_state
                    .remaining
                    .fetch_sub(i64::from(step), Ordering::Relaxed)
                    - i64::from(step);
                if left <= 0 {
                    hook_state.exceeded.store(true, Ordering::Relaxed);
                    return Err(mlua::Error::runtime("instruction limit exceeded"));
                }
                Ok(VmState::Continue)
            },
        )?;
        let flag = Arc::clone(&state);
        let exceeded =
            lua.create_function(move |_, ()| Ok(flag.exceeded.load(Ordering::Relaxed)))?;
        let globals = lua.globals();
        let coroutine: Table = globals.get("coroutine")?;
        lua.load(RETHROW_LIMIT).set_name("=sandbox").call::<()>((
            globals.get::<mlua::Function>("pcall")?,
            globals.get::<mlua::Function>("xpcall")?,
            coroutine.get::<mlua::Function>("resume")?,
            exceeded,
        ))?;
        Ok(Self { limit, state })
    }

    /// Runs `f` with a full budget, keeping the Lua error as is.
    pub(crate) fn run<R>(&self, f: impl FnOnce() -> mlua::Result<R>) -> Result<R, Failure> {
        self.state.remaining.store(self.limit, Ordering::Relaxed);
        self.state.exceeded.store(false, Ordering::Relaxed);
        let result = f();
        let exceeded = self.state.exceeded.swap(false, Ordering::Relaxed);
        self.state.remaining.store(self.limit, Ordering::Relaxed);
        if exceeded {
            return Err(Failure::InstructionLimit);
        }
        result.map_err(Failure::Lua)
    }

    /// Runs `f` with a full budget and maps its errors.
    pub(crate) fn guard<R>(&self, f: impl FnOnce() -> mlua::Result<R>) -> Result<R, PluginError> {
        self.run(f).map_err(|failure| match failure {
            Failure::InstructionLimit => PluginError::InstructionLimit,
            Failure::Lua(e) => convert::runtime_error(&e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        new_lua(None).unwrap()
    }

    fn is_nil(lua: &Lua, expr: &str) -> bool {
        lua.load(format!("return {expr} == nil")).eval().unwrap()
    }

    #[test]
    fn dangerous_libraries_are_absent() {
        let lua = lua();
        for expr in [
            "io",
            "debug",
            "package",
            "dofile",
            "loadfile",
            "os.execute",
            "os.remove",
            "os.rename",
            "os.exit",
            "os.getenv",
            "os.tmpname",
            "string.dump",
        ] {
            assert!(is_nil(&lua, expr), "{expr} is reachable");
        }
    }

    #[test]
    fn safe_libraries_are_present() {
        let lua = lua();
        let ok: bool = lua
            .load(
                r#"return string.upper("a") == "A" and table.concat({1, 2}, ",") == "1,2"
                  and math.floor(1.5) == 1 and utf8.char(72) == "H"
                  and type(os.time()) == "number" and type(os.clock()) == "number"
                  and type(os.date("%Y")) == "string" and coroutine ~= nil"#,
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn load_accepts_text_but_not_bytecode() {
        let lua = lua();
        let n: i64 = lua.load("return load('return 7')()").eval().unwrap();
        assert_eq!(n, 7);
        let (f, err): (mlua::Value, String) = lua
            .load(r#"return load("\27Lua\84\0", "x", "b")"#)
            .eval()
            .unwrap();
        assert!(f.is_nil());
        assert!(err.contains("binary"), "{err}");
    }

    #[test]
    fn load_with_an_env_uses_it() {
        let lua = lua();
        let n: i64 = lua
            .load("return load('return x', 'c', 't', { x = 3 })()")
            .eval()
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn require_cannot_load_files_or_modules() {
        let lua = lua();
        let err = lua
            .load("return require('/etc/passwd')")
            .exec()
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    fn limited(limit: u64) -> (Lua, Limiter) {
        let lua = lua();
        let limiter = Limiter::install(&lua, limit).unwrap();
        (lua, limiter)
    }

    #[test]
    fn an_infinite_loop_is_cut() {
        let (lua, limiter) = limited(1_000_000);
        let result = limiter.guard(|| lua.load("while true do end").exec());
        assert_eq!(result, Err(PluginError::InstructionLimit));
    }

    #[test]
    fn the_vm_is_usable_after_hitting_the_limit() {
        let (lua, limiter) = limited(1_000_000);
        let cut = limiter.guard(|| lua.load("while true do end").exec());
        assert_eq!(cut, Err(PluginError::InstructionLimit));
        let n = limiter.guard(|| {
            lua.load("local s = 0 for i = 1, 100 do s = s + i end return s")
                .eval::<i64>()
        });
        assert_eq!(n, Ok(5050));
    }

    #[test]
    fn the_budget_is_per_call() {
        let (lua, limiter) = limited(1_000_000);
        let work = "local s = 0 for i = 1, 20000 do s = s + i end return s";
        for _ in 0..20 {
            assert!(limiter.guard(|| lua.load(work).eval::<i64>()).is_ok());
        }
    }

    #[test]
    fn pcall_cannot_swallow_the_limit() {
        let (lua, limiter) = limited(1_000_000);
        let result = limiter.guard(|| {
            lua.load("while true do pcall(function() while true do end end) end")
                .exec()
        });
        assert_eq!(result, Err(PluginError::InstructionLimit));
        let caught = limiter.guard(|| {
            lua.load("pcall(function() while true do end end) return 1")
                .eval::<i64>()
        });
        assert_eq!(caught, Err(PluginError::InstructionLimit));
    }

    #[test]
    fn xpcall_and_resume_cannot_swallow_the_limit() {
        let (lua, limiter) = limited(1_000_000);
        for code in [
            "while true do xpcall(function() while true do end end, function(e) return e end) end",
            "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
        ] {
            assert_eq!(
                limiter.guard(|| lua.load(code).exec()),
                Err(PluginError::InstructionLimit),
                "{code}"
            );
        }
    }

    #[test]
    fn pcall_still_catches_ordinary_errors() {
        let (lua, limiter) = limited(1_000_000);
        let (ok, msg): (bool, String) = limiter
            .guard(|| lua.load("return pcall(error, 'x', 0)").eval())
            .unwrap();
        assert!(!ok);
        assert_eq!(msg, "x");
    }

    #[test]
    fn loops_inside_coroutines_are_cut() {
        let (lua, limiter) = limited(1_000_000);
        let result = limiter.guard(|| {
            lua.load("coroutine.wrap(function() while true do end end)()")
                .exec()
        });
        assert_eq!(result, Err(PluginError::InstructionLimit));
    }

    #[test]
    fn memory_limit_stops_runaway_allocation() {
        let lua = new_lua(Some(8 * 1024 * 1024)).unwrap();
        let limiter = Limiter::install(&lua, u64::MAX).unwrap();
        let result = limiter.guard(|| {
            lua.load("local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end")
                .exec()
        });
        assert!(
            matches!(&result, Err(PluginError::Runtime(m)) if m.contains("memory")),
            "{result:?}"
        );
        let n = limiter.guard(|| lua.load("return 1 + 1").eval::<i64>());
        assert_eq!(n, Ok(2));
    }
}
