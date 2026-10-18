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
//! `require(name)` only resolves modules the host registers with
//! [`set_module`], such as `app`.

use mlua::{Lua, LuaOptions, StdLib, Table, Variadic};

/// Registry table with the modules `require` can return.
const MODULES: &str = "crest.modules";

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

    lua.set_named_registry_value(MODULES, lua.create_table()?)?;
    globals.raw_set("require", lua.create_function(require)?)?;
    Ok(())
}

fn require(lua: &Lua, name: String) -> mlua::Result<mlua::Value> {
    let modules: Table = lua.named_registry_value(MODULES)?;
    match modules.raw_get::<mlua::Value>(name.as_str())? {
        mlua::Value::Nil => Err(mlua::Error::runtime(format!(
            "module '{name}' not found (only host modules such as 'app' can be required)"
        ))),
        module => Ok(module),
    }
}

/// Makes `require(name)` return `module`.
pub(crate) fn set_module(lua: &Lua, name: &str, module: impl mlua::IntoLua) -> mlua::Result<()> {
    let modules: Table = lua.named_registry_value(MODULES)?;
    modules.raw_set(name, module)
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
    fn require_only_returns_host_modules() {
        let lua = lua();
        let app = lua.create_table().unwrap();
        app.set("name", "crest").unwrap();
        set_module(&lua, "app", app).unwrap();
        let name: String = lua.load("return require('app').name").eval().unwrap();
        assert_eq!(name, "crest");
        let err = lua
            .load("return require('/etc/passwd')")
            .exec()
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn memory_limit_stops_runaway_allocation() {
        let lua = new_lua(Some(8 * 1024 * 1024)).unwrap();
        let result = lua
            .load("local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end")
            .exec();
        assert!(
            matches!(&result, Err(mlua::Error::MemoryError(_))),
            "{result:?}"
        );
        let n: i64 = lua.load("return 1 + 1").eval().unwrap();
        assert_eq!(n, 2);
    }
}
