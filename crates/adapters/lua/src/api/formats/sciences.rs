//! `app.formats` functions of the science B viewers: `map`, `gantt`,
//! `array_file`, `network`, `training_curves`.

use application::ports::formats::{
    ArrayRequest, GanttRequest, GanttSource, GanttTaskInput, MapRequest, NetworkRequest,
    TrainingFile, TrainingRequest,
};
use application::tools::formats::{EdgeInput, edges_json, read_array, read_map, read_report};
use domain::block::BlockKind;
use domain::science::training::{Goal, TrainingAxis};
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

fn array_file(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "array_file";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_array(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let request = ArrayRequest {
        path: path.clone(),
        variable: text(args, F, "variable")?,
    };
    let mut spec = services
        .formats
        .inspect_array_file(&bytes, &request)
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::ArrayFile(Box::new(spec)))
}

fn network(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "network";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?;
    let edges: Option<Vec<EdgeInput>> = match args.get::<Value>("edges")? {
        Value::Nil => None,
        value @ Value::Table(_) => {
            let json = convert::to_json(lua, value)?;
            Some(serde_json::from_value(json).map_err(|e| error(F, format!("`edges`: {e}")))?)
        }
        other => {
            return Err(error(
                F,
                format!("`edges` must be a list, got {}", other.type_name()),
            ));
        }
    };
    let bytes = match (&path, edges) {
        (Some(_), Some(_)) => return Err(error(F, "give `path` or `edges`, not both")),
        (None, None) => return Err(error(F, "`path` or `edges` is required")),
        (Some(path), None) => context(lua)?
            .block_on(read_report(services.fs.as_ref(), path))?
            .map_err(|e| error(F, e))?,
        (None, Some(edges)) => edges_json(&edges),
    };
    let size_by = match text(args, F, "size_by")? {
        None => domain::science::network::NetworkMetric::Degree,
        Some(name) => {
            serde_json::from_value(serde_json::Value::String(name.clone())).map_err(|_| {
                error(
                    F,
                    format!(
                        "`size_by` is degree, betweenness, closeness or pagerank, not {name:?}"
                    ),
                )
            })?
        }
    };
    let seed = match args.get::<Option<i64>>("seed")? {
        Some(s) => Some(u64::try_from(s).map_err(|_| error(F, "`seed` must not be negative"))?),
        None => None,
    };
    let request = NetworkRequest {
        path: path.clone(),
        bytes,
        directed: args.get::<Option<bool>>("directed")?,
        source: text(args, F, "source")?,
        target: text(args, F, "target")?,
        weight: text(args, F, "weight")?,
        size_by,
        seed,
    };
    let mut spec = services
        .formats
        .analyse_network(&request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Network(Box::new(spec)))
}

fn training_curves(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "training_curves";
    let services = services(lua, F)?;
    let mut paths: Vec<String> = text(args, F, "path")?.into_iter().collect();
    if let Some(list) = args.get::<Option<Vec<String>>>("paths")? {
        paths.extend(list);
    }
    if paths.is_empty() {
        return Err(error(F, "`path` or `paths` is required"));
    }
    let mut files = Vec::new();
    for path in paths {
        let bytes = context(lua)?
            .block_on(read_report(services.fs.as_ref(), &path))?
            .map_err(|e| error(F, e))?;
        files.push(TrainingFile { path, bytes });
    }
    let request = TrainingRequest {
        files,
        metric: text(args, F, "metric")?,
        axis: match text(args, F, "x")?.as_deref() {
            None => None,
            Some("step") => Some(TrainingAxis::Step),
            Some("epoch") => Some(TrainingAxis::Epoch),
            Some("time") => Some(TrainingAxis::Time),
            Some(other) => {
                return Err(error(
                    F,
                    format!("`x` is step, epoch or time, not {other:?}"),
                ));
            }
        },
        smoothing: args.get::<Option<f64>>("smoothing")?,
        log_y: args.get::<Option<bool>>("log_y")?.unwrap_or(false),
        goal: match text(args, F, "goal")?.as_deref() {
            None => None,
            Some("minimize") => Some(Goal::Minimize),
            Some("maximize") => Some(Goal::Maximize),
            Some(other) => {
                return Err(error(
                    F,
                    format!("`goal` is minimize or maximize, not {other:?}"),
                ));
            }
        },
    };
    let mut spec = services
        .formats
        .training_curves(&request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::TrainingCurves(Box::new(spec)))
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
    formats.set(
        "array_file",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => array_file(lua, &args),
            _ => Err(error("array_file", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "network",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => network(lua, &args),
            _ => Err(error("network", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "training_curves",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => training_curves(lua, &args),
            _ => Err(error("training_curves", "expected a table of arguments")),
        })?,
    )?;
    Ok(())
}
