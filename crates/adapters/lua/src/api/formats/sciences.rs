//! `app.formats` functions of the science B viewers: `map`, `gantt`.

use application::ports::formats::{GanttRequest, GanttSource, GanttTaskInput, MapRequest};
use application::tools::formats::{read_map, read_report};
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

fn gantt(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "gantt";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?;
    let tasks: Option<Vec<GanttTaskInput>> = match args.get::<Value>("tasks")? {
        Value::Nil => None,
        value @ Value::Table(_) => {
            let json = convert::to_json(lua, value)?;
            Some(serde_json::from_value(json).map_err(|e| error(F, format!("`tasks`: {e}")))?)
        }
        other => {
            return Err(error(
                F,
                format!("`tasks` must be a list, got {}", other.type_name()),
            ));
        }
    };
    let source = match (&path, tasks) {
        (Some(_), Some(_)) => return Err(error(F, "give `path` or `tasks`, not both")),
        (None, None) => return Err(error(F, "`path` or `tasks` is required")),
        (Some(path), None) => GanttSource::File(
            context(lua)?
                .block_on(read_report(services.fs.as_ref(), path))?
                .map_err(|e| error(F, e))?,
        ),
        (None, Some(tasks)) => GanttSource::Tasks(tasks),
    };
    let calendar = match text(args, F, "calendar")? {
        None => domain::gantt::Calendar::AllDays,
        Some(name) => {
            serde_json::from_value(serde_json::Value::String(name.clone())).map_err(|_| {
                error(
                    F,
                    format!("`calendar` is `all_days` or `weekdays`, not {name:?}"),
                )
            })?
        }
    };
    let request = GanttRequest {
        path: path.clone(),
        source,
        start: text(args, F, "start")?,
        calendar,
    };
    let mut spec = services
        .formats
        .schedule_gantt(&request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Gantt(Box::new(spec)))
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
    formats.set(
        "gantt",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => gantt(lua, &args),
            _ => Err(error("gantt", "expected a table of arguments")),
        })?,
    )?;
    Ok(())
}
