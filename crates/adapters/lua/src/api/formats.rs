//! `app.formats`: the file-format blocks of the `show_dataset`,
//! `show_molecule`, `show_trace`, `show_flamegraph` and `show_logs` tools
//! from Lua. Each returns a block (wrapped like
//! `app.ui.block`), so a tool or command handler that returns it shows it.
//!
//! ```lua
//! return app.formats.dataset{ path = 'data/cars.csv', rows = 10 }
//! return app.formats.molecule{ smiles = 'CCO', name = 'ethanol' }
//! return app.formats.trace{ path = 'traces/checkout.json' }
//! return app.formats.flamegraph{ path = 'cpu.pb.gz', sample_type = 'cpu' }
//! return app.formats.logs{ path = 'logs/app.log', format = 'logfmt' }
//! ```
//!
//! | Function | Block |
//! | --- | --- |
//! | `dataset{ path, rows }` | a data file's shape, column statistics and first rows (CSV, TSV, JSON lines, Parquet) |
//! | `molecule{ smiles, name, claimed_formula }` | a 2D structure with its computed formula, mass and charge |
//! | `trace{ path, title }` | a trace waterfall (OTLP, Jaeger or Zipkin JSON) |
//! | `flamegraph{ path, sample_type, title }` | a flame graph (folded stacks or pprof) |
//! | `logs{ path, format }` | a log file on a filterable list (`json`, `logfmt` or `text`; guessed when absent) |
//!
//! The path is read through the same guard as the model's `read_file`.
//! The block is in `result["$crest_block"]` (e.g. its `formula`). A file
//! that cannot be read or parsed, or a SMILES that does not parse,
//! raises a Lua error with the reason. The calls block the VM while they
//! read and parse.

use std::sync::Arc;

use application::ports::formats::{DatasetRequest, FormatsPort, LogRequest, ProfileRequest};
use application::ports::fs::FileSystemPort;
use application::tools::formats::{
    DEFAULT_PREVIEW_ROWS, MAX_PREVIEW_ROWS, dataset_format, read_dataset, read_log, read_report,
};
use domain::block::BlockKind;
use domain::devops::logs::LogFormat;
use mlua::{Lua, Table, Value};
use serde_json::json;

use super::ui::BLOCK_KEY;
use crate::caps::context;
use crate::convert;

/// What `app.formats` runs on, handed to the host by the binary.
#[derive(Clone)]
pub struct FormatsServices {
    /// The parsers.
    pub formats: Arc<dyn FormatsPort>,
    /// Reads data files of the workspace.
    pub fs: Arc<dyn FileSystemPort>,
}

impl std::fmt::Debug for FormatsServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormatsServices")
    }
}

fn error(function: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.formats.{function}: {message}"))
}

fn services(lua: &Lua, function: &str) -> mlua::Result<FormatsServices> {
    context(lua)?
        .formats()
        .ok_or_else(|| error(function, "the format parsers are not available"))
}

/// `block` as the table `app.ui.block` returns.
pub(crate) fn block_value(lua: &Lua, block: &BlockKind) -> mlua::Result<Value> {
    let json = serde_json::to_value(block).map_err(mlua::Error::external)?;
    convert::to_lua(lua, &json!({ BLOCK_KEY: json }))
}

/// A non-empty string field.
fn text(args: &Table, function: &str, key: &str) -> mlua::Result<Option<String>> {
    match args.get::<Value>(key)? {
        Value::Nil => Ok(None),
        Value::String(s) => {
            let s = s.to_str()?.trim().to_owned();
            Ok((!s.is_empty()).then_some(s))
        }
        other => Err(error(
            function,
            format!("`{key}` must be a string, got {}", other.type_name()),
        )),
    }
}

fn dataset(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "dataset";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let format = dataset_format(&path).ok_or_else(|| {
        error(
            F,
            format!("`{path}` is not a dataset file: use .csv, .tsv, .jsonl/.ndjson or .parquet"),
        )
    })?;
    let rows = match args.get::<Option<i64>>("rows")? {
        Some(n) if n >= 1 => usize::try_from(n).unwrap_or(MAX_PREVIEW_ROWS),
        Some(_) => return Err(error(F, "`rows` must be at least 1")),
        None => DEFAULT_PREVIEW_ROWS,
    };
    let ctx = context(lua)?;
    let (bytes, truncated) = ctx
        .block_on(read_dataset(services.fs.as_ref(), &path, format))?
        .map_err(|e| error(F, e))?;
    let request = DatasetRequest {
        path: path.clone(),
        format,
        preview_rows: rows.min(MAX_PREVIEW_ROWS),
        truncated,
    };
    let spec = services
        .formats
        .preview_dataset(&bytes, &request)
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Dataset(spec))
}

fn molecule(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "molecule";
    let services = services(lua, F)?;
    let smiles = text(args, F, "smiles")?.ok_or_else(|| error(F, "`smiles` is required"))?;
    let claimed = text(args, F, "claimed_formula")?;
    let mut spec = services
        .formats
        .depict_molecule(&smiles, claimed.as_deref());
    if let Some(err) = &spec.error {
        return Err(error(F, format!("`{smiles}`: {err}")));
    }
    spec.name = text(args, F, "name")?;
    block_value(lua, &BlockKind::Molecule(spec))
}

fn trace(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "trace";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_report(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_trace(&bytes, &path)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Trace(Box::new(spec)))
}

fn flamegraph(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "flamegraph";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_report(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let request = ProfileRequest {
        path: path.clone(),
        sample_type: text(args, F, "sample_type")?,
    };
    let mut spec = services
        .formats
        .parse_profile(&bytes, &request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::FlameGraph(Box::new(spec)))
}

fn logs(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "logs";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let format = match text(args, F, "format")?.as_deref() {
        None => None,
        Some("json") => Some(LogFormat::Json),
        Some("logfmt") => Some(LogFormat::Logfmt),
        Some("text") => Some(LogFormat::Text),
        Some(other) => {
            return Err(error(
                F,
                format!("`format` must be json, logfmt or text, got {other:?}"),
            ));
        }
    };
    let (bytes, truncated) = context(lua)?
        .block_on(read_log(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let request = LogRequest {
        path: path.clone(),
        format,
        truncated,
    };
    let spec = services
        .formats
        .parse_logs(&bytes, &request)
        .map_err(|e| error(F, e))?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Logs(Box::new(spec)))
}

/// The `app.formats` table.
pub(crate) fn table(lua: &Lua) -> mlua::Result<Table> {
    let formats = lua.create_table()?;
    formats.set(
        "dataset",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => dataset(lua, &args),
            _ => Err(error("dataset", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "molecule",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => molecule(lua, &args),
            _ => Err(error("molecule", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "trace",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => trace(lua, &args),
            _ => Err(error("trace", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "flamegraph",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => flamegraph(lua, &args),
            _ => Err(error("flamegraph", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "logs",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => logs(lua, &args),
            _ => Err(error("logs", "expected a table of arguments")),
        })?,
    )?;
    Ok(formats)
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::PluginOutput;
    use application::testing::formats::FakeFormats;
    use application::testing::fs::{InMemoryFs, test_root};

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::caps::Context;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run_code(code: &str, formats: Option<Arc<FakeFormats>>) -> mlua::Result<PluginOutput> {
        let lua = sandbox::new_lua(None)?;
        callbacks::install(&lua, CallbackIds::default())?;
        let fs = InMemoryFs::new(test_root())
            .with_file("data/cars.csv", "mpg,am\n21,m\n22.8,a\n")
            .with_file("traces/t.json", "{}")
            .with_file("cpu.folded", "main 1\n")
            .with_file("logs/app.log", "info hi\n")
            .with_file(".env", "a\n1\n");
        let ctx = Context::new(None, false, Some(Vec::new()));
        if let Some(formats) = formats {
            ctx.provide_formats(FormatsServices {
                formats,
                fs: Arc::new(fs),
            });
        }
        lua.set_app_data(Arc::new(ctx));
        let phase = Phase::new(&lua, "init.lua", "lab")?;
        let value: Value = lua
            .load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .eval()?;
        Ok(crate::api::ui::output(convert::to_json(&lua, value)?))
    }

    #[test]
    fn dataset_and_molecule_return_their_blocks() {
        let formats = Arc::new(FakeFormats::new());
        let out = run_code(
            "return app.formats.dataset{ path = 'data/cars.csv', rows = 5 }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        assert!(
            matches!(out, PluginOutput::Block(BlockKind::Dataset(_))),
            "{out:?}"
        );
        let out = run_code(
            "return app.formats.molecule{ smiles = 'CCO', name = 'ethanol' }",
            Some(formats),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Molecule(spec)) = out else {
            panic!("not a molecule: {out:?}");
        };
        assert_eq!(spec.name.as_deref(), Some("ethanol"));
    }

    #[test]
    fn traces_profiles_and_logs_return_their_blocks() {
        let formats = Arc::new(FakeFormats::new());
        let out = run_code(
            "return app.formats.trace{ path = 'traces/t.json', title = 'checkout' }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Trace(spec)) = out else {
            panic!("not a trace: {out:?}");
        };
        assert_eq!(spec.title.as_deref(), Some("checkout"));
        let out = run_code(
            "return app.formats.flamegraph{ path = 'cpu.folded' }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        assert!(
            matches!(out, PluginOutput::Block(BlockKind::FlameGraph(_))),
            "{out:?}"
        );
        let out = run_code(
            "return app.formats.logs{ path = 'logs/app.log', format = 'text' }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Logs(spec)) = out else {
            panic!("not logs: {out:?}");
        };
        assert_eq!(spec.format, LogFormat::Text);
        let bad = run_code(
            "return app.formats.logs{ path = 'logs/app.log', format = 'xml' }",
            Some(formats),
        )
        .unwrap_err()
        .to_string();
        assert!(bad.contains("json, logfmt or text"), "{bad}");
    }

    #[test]
    fn bad_calls_raise_lua_errors() {
        let err = |code: &str| {
            run_code(code, Some(Arc::new(FakeFormats::new())))
                .unwrap_err()
                .to_string()
        };
        assert!(err("return app.formats.dataset{ path = '.env' }").contains("not a dataset"));
        assert!(
            err("return app.formats.dataset{ path = 'x/../../a.csv' }")
                .contains("app.formats.dataset")
        );
        assert!(err("return app.formats.dataset{}").contains("`path` is required"));
        assert!(err("return app.formats.molecule(1)").contains("expected a table"));
        assert!(err("return app.formats.molecule{ smiles = 'bad' }").contains("invalid SMILES"));
        let failing = run_code(
            "return app.formats.dataset{ path = 'data/cars.csv' }",
            Some(Arc::new(FakeFormats::failing("broken row"))),
        )
        .unwrap_err()
        .to_string();
        assert!(failing.contains("broken row"), "{failing}");
        let none = run_code("return app.formats.molecule{ smiles = 'C' }", None)
            .unwrap_err()
            .to_string();
        assert!(none.contains("not available"), "{none}");
    }
}
