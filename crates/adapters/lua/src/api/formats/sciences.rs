//! `app.formats` functions of the science B viewers: `map`.

use application::ports::formats::MapRequest;
use application::tools::formats::read_map;
use domain::block::BlockKind;
use mlua::{Lua, Table, Value};

use super::{block_value, error, services, text};
use crate::caps::context;
use crate::convert;

/// Inline data given as a string or a table (turned into JSON text).
fn inline_json(lua: &Lua, args: &Table, function: &str, key: &str) -> mlua::Result<Option<String>> {
    match args.get::<Value>(key)? {
        Value::Nil => Ok(None),
        Value::String(s) => Ok(Some(s.to_str()?.to_owned()).filter(|s| !s.trim().is_empty())),
        value @ Value::Table(_) => Ok(Some(convert::to_json(lua, value)?.to_string())),
        other => Err(error(
            function,
            format!(
                "`{key}` must be a string or a table, got {}",
                other.type_name()
            ),
        )),
    }
}

fn map(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "map";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?;
    let inline = inline_json(lua, args, F, "geojson")?;
    let bytes = match (&path, inline) {
        (Some(_), Some(_)) => return Err(error(F, "give `path` or `geojson`, not both")),
        (None, None) => return Err(error(F, "`path` or `geojson` is required")),
        (Some(path), None) => context(lua)?
            .block_on(read_map(services.fs.as_ref(), path))?
            .map_err(|e| error(F, e))?,
        (None, Some(source)) => source.into_bytes(),
    };
    let request = MapRequest {
        path: path.clone(),
        crs: text(args, F, "crs")?,
        color_by: text(args, F, "color_by")?,
    };
    let name = path.unwrap_or_else(|| "inline GeoJSON".to_owned());
    let mut spec = services
        .formats
        .parse_geojson(&bytes, &request)
        .map_err(|e| error(F, format!("`{name}`: {e}")))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::GeoMap(Box::new(spec)))
}

/// Adds the science B functions to the `app.formats` table.
pub(super) fn register(lua: &Lua, formats: &Table) -> mlua::Result<()> {
    formats.set(
        "map",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => map(lua, &args),
            _ => Err(error("map", "expected a table of arguments")),
        })?,
    )?;
    Ok(())
}
