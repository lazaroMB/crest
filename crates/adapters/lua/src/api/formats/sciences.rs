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

use application::tools::formats::arrays::{build_array_file, normalize_array_file};
use application::tools::formats::geomap::{build_map, normalize_map};
use application::tools::formats::network::{build_network, normalize_network};

use super::{block_value, error, register, services, text};
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
pub(super) fn add(lua: &Lua, formats: &Table) -> mlua::Result<()> {
    register(lua, formats, "map", normalize_map, build_map, |spec| {
        BlockKind::GeoMap(Box::new(spec))
    })?;
    formats.set(
        "gantt",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => gantt(lua, &args),
            _ => Err(error("gantt", "expected a table of arguments")),
        })?,
    )?;
    register(
        lua,
        formats,
        "array_file",
        normalize_array_file,
        build_array_file,
        |spec| BlockKind::ArrayFile(Box::new(spec)),
    )?;
    register(
        lua,
        formats,
        "network",
        normalize_network,
        build_network,
        |spec| BlockKind::Network(Box::new(spec)),
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
