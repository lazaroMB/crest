//! Conversions between JSON, Lua values and port errors.

use application::ports::plugin::PluginError;
use mlua::{Lua, LuaSerdeExt};
use serde_json::Value;

/// Converts JSON into a Lua value. JSON `null` becomes `nil`, so Lua code can
/// test fields with `if x == nil`.
pub(crate) fn to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    let options = mlua::serde::ser::Options::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false);
    lua.to_value_with(value, options)
}

/// Converts a Lua value into JSON. Functions and other values JSON cannot
/// represent are skipped; `nil` becomes `null`. Keys are sorted, so the
/// result does not depend on Lua's table order.
pub(crate) fn to_json(lua: &Lua, value: mlua::Value) -> mlua::Result<Value> {
    if value.is_nil() {
        return Ok(Value::Null);
    }
    let options = mlua::serde::de::Options::new()
        .deny_unsupported_types(false)
        .sort_keys(true);
    lua.from_value_with(value, options)
}

/// Maps an error raised while running Lua code.
pub(crate) fn runtime_error(err: &mlua::Error) -> PluginError {
    match err {
        mlua::Error::MemoryError(message) => {
            PluginError::Runtime(format!("out of memory: {message}"))
        }
        other => PluginError::Runtime(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_round_trips_through_lua() {
        let lua = Lua::new();
        let value = json!({ "name": "t", "items": [1, 2, 3], "nested": { "ok": true } });
        let back = to_json(&lua, to_lua(&lua, &value).unwrap()).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn json_null_is_nil_in_lua() {
        let lua = Lua::new();
        lua.globals()
            .set("v", to_lua(&lua, &json!({ "a": null })).unwrap())
            .unwrap();
        let is_nil: bool = lua.load("return v.a == nil").eval().unwrap();
        assert!(is_nil);
    }

    #[test]
    fn functions_are_skipped_and_nil_is_null() {
        let lua = Lua::new();
        let v: mlua::Value = lua
            .load("return { a = 1, f = function() end }")
            .eval()
            .unwrap();
        assert_eq!(to_json(&lua, v).unwrap(), json!({ "a": 1 }));
        assert_eq!(to_json(&lua, mlua::Value::Nil).unwrap(), Value::Null);
    }
}
