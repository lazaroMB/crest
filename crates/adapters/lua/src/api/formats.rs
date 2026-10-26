//! `app.formats`: the file-format blocks of the `show_dataset`,
//! `show_molecule`, `show_trace`, `show_flamegraph`, `show_logs`,
//! `show_query_plan`, `show_har`, `show_terraform_plan`, `show_dot`,
//! `show_er`, `show_benchmarks`, `show_waveform`, `show_netlist`,
//! `show_step`, `show_dxf`, `show_ifc_schedule`, `show_sequence` and
//! `show_reaction` tools from Lua. Each returns a block (wrapped like
//! `app.ui.block`), so a tool or command handler that returns it shows it.
//!
//! ```lua
//! return app.formats.dataset{ path = 'data/cars.csv', rows = 10 }
//! return app.formats.molecule{ smiles = 'CCO', name = 'ethanol' }
//! return app.formats.trace{ path = 'traces/checkout.json' }
//! return app.formats.flamegraph{ path = 'cpu.pb.gz', sample_type = 'cpu' }
//! return app.formats.logs{ path = 'logs/app.log', format = 'logfmt' }
//! return app.formats.query_plan{ path = 'plans/slow.json' }
//! return app.formats.har{ path = 'net/checkout.har' }
//! return app.formats.terraform_plan{ path = 'infra/plan.json' }
//! return app.formats.dot{ source = 'digraph { a -> b }' }
//! return app.formats.er{ path = 'db/schema.sql', dialect = 'postgresql' }
//! return app.formats.benchmarks{ path = 'target/criterion', baseline = 'base' }
//! return app.formats.waveform{ path = 'sim/tb.vcd', signals = { 'tb.dut.*' } }
//! return app.formats.netlist{ path = 'hw/amp.cir' }
//! return app.formats.step{ path = 'cad/bracket.step' }
//! return app.formats.dxf{ path = 'dwg/plate.dxf' }
//! return app.formats.ifc_schedule{ path = 'bim/house.ifc' }
//! return app.formats.sequence{ path = 'dna/puc19.gb' }
//! return app.formats.reaction{ smiles = 'C.O=O>>O=C=O.O' }
//! ```
//!
//! | Function | Block |
//! | --- | --- |
//! | `dataset{ path, rows }` | a data file's shape, column statistics and first rows (CSV, TSV, JSON lines, Parquet) |
//! | `molecule{ smiles, name, claimed_formula }` | a 2D structure with its computed formula, mass and charge |
//! | `trace{ path, title }` | a trace waterfall (OTLP, Jaeger or Zipkin JSON) |
//! | `flamegraph{ path, sample_type, title }` | a flame graph (folded stacks or pprof) |
//! | `logs{ path, format }` | a log file on a filterable list (`json`, `logfmt` or `text`; guessed when absent) |
//! | `query_plan{ path or plan, title }` | a PostgreSQL `EXPLAIN (FORMAT JSON)` plan on a tree table |
//! | `har{ path, title }` | an HTTP archive as a waterfall with a request inspector |
//! | `terraform_plan{ path, title }` | a Terraform plan (`terraform show -json`) as a tree of resource changes |
//! | `dot{ path or source, title }` | a Graphviz DOT graph on the graph widget |
//! | `er{ path or sql, dialect, title }` | an ER diagram from SQL DDL (tables, foreign keys) |
//! | `benchmarks{ path, baseline, filter, title }` | criterion results, baseline against new (`target/criterion`, `base` by default) |
//! | `waveform{ path, signals, radix, title }` | a VCD, FST or GHW dump as digital waves (`signals`: paths or `*` patterns; `radix`: `hex`, `bin`, `dec`) |
//! | `netlist{ path or source, format, title }` | a SPICE deck or KiCad netlist as a component–net graph and table (`format`: `spice` or `kicad`, guessed when absent) |
//! | `step{ path, tolerance, title }` | a STEP part tessellated into the 3D view with its solids (`tolerance` in the file's unit) |
//! | `dxf{ path, title }` | a 2D DXF drawing with its layers, zoom and pan |
//! | `ifc_schedule{ path, title }` | an IFC model's spaces and areas (only the file's) and wall/door/window counts per storey |
//! | `sequence{ path or sequence, record, alphabet, circular, enzymes, title }` | a GenBank, FASTA or plain sequence with features, frames and (circular DNA) a plasmid map; `record` 1-based, `alphabet` `dna`/`rna`/`protein`, `enzymes` (default true) searches the bundled restriction enzymes |
//! | `reaction{ smiles, coefficients, conditions, names, title }` | a reaction scheme with its atom and charge balance and, when unbalanced, suggested coefficients (raises on a SMILES that does not parse) |
//!
//! The path is read through the same guard as the model's `read_file`.
//! The block is in `result["$crest_block"]` (e.g. its `formula`). A file
//! that cannot be read or parsed, or a SMILES that does not parse,
//! raises a Lua error with the reason. The calls block the VM while they
//! read and parse.

use std::sync::Arc;

use application::ports::formats::{
    DatasetRequest, FormatsPort, LogRequest, ProfileRequest, SequenceRequest, StepRequest,
    WaveRequest,
};
use application::ports::fs::FileSystemPort;
use application::tools::formats::{
    DEFAULT_BASELINE, DEFAULT_CRITERION_DIR, DEFAULT_PREVIEW_ROWS, MAX_PREVIEW_ROWS,
    collect_benchmarks, dataset_format, read_dataset, read_har, read_ifc, read_log, read_report,
    read_step, read_wave,
};
use domain::block::BlockKind;
use domain::devops::logs::LogFormat;
use domain::engineering::waveform::Radix;
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

fn query_plan(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "query_plan";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?;
    let bytes = match (&path, text(args, F, "plan")?) {
        (Some(_), Some(_)) => return Err(error(F, "give `path` or `plan`, not both")),
        (None, None) => return Err(error(F, "`path` or `plan` is required")),
        (Some(path), None) => context(lua)?
            .block_on(read_report(services.fs.as_ref(), path))?
            .map_err(|e| error(F, e))?,
        (None, Some(plan)) => plan.into_bytes(),
    };
    let mut spec = services
        .formats
        .parse_query_plan(&bytes, path.as_deref())
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::QueryPlan(Box::new(spec)))
}

fn har(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "har";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_har(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_har(&bytes, &path)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Har(Box::new(spec)))
}

fn terraform_plan(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "terraform_plan";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_report(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_terraform_plan(&bytes, &path)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::TerraformPlan(Box::new(spec)))
}

/// The text of `path` (through the guard) or of the inline `inline`
/// argument; exactly one must be given.
fn path_or_inline(
    lua: &Lua,
    args: &Table,
    function: &str,
    inline: &str,
) -> mlua::Result<(Option<String>, String)> {
    let services = services(lua, function)?;
    match (text(args, function, "path")?, text(args, function, inline)?) {
        (Some(_), Some(_)) => Err(error(
            function,
            format!("give `path` or `{inline}`, not both"),
        )),
        (None, None) => Err(error(function, format!("`path` or `{inline}` is required"))),
        (Some(path), None) => {
            let bytes = context(lua)?
                .block_on(read_report(services.fs.as_ref(), &path))?
                .map_err(|e| error(function, e))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| error(function, format!("`{path}` is not UTF-8 text")))?;
            Ok((Some(path), text))
        }
        (None, Some(source)) => Ok((None, source)),
    }
}

fn dot(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "dot";
    let services = services(lua, F)?;
    let (path, source) = path_or_inline(lua, args, F, "source")?;
    let mut spec = services
        .formats
        .parse_dot(&source, path.as_deref())
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Diagram(Box::new(spec)))
}

fn er(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "er";
    let services = services(lua, F)?;
    let (path, sql) = path_or_inline(lua, args, F, "sql")?;
    let dialect = text(args, F, "dialect")?;
    let mut spec = services
        .formats
        .parse_ddl(&sql, dialect.as_deref(), path.as_deref())
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Diagram(Box::new(spec)))
}

fn benchmarks(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "benchmarks";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.unwrap_or_else(|| DEFAULT_CRITERION_DIR.to_owned());
    let baseline = text(args, F, "baseline")?.unwrap_or_else(|| DEFAULT_BASELINE.to_owned());
    let filter = text(args, F, "filter")?;
    let request = context(lua)?
        .block_on(collect_benchmarks(
            services.fs.as_ref(),
            &path,
            &baseline,
            filter.as_deref(),
        ))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_benchmarks(&request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Benchmarks(Box::new(spec)))
}

/// A list of strings (or one string).
fn strings(args: &Table, function: &str, key: &str) -> mlua::Result<Vec<String>> {
    match args.get::<Value>(key)? {
        Value::Nil => Ok(Vec::new()),
        Value::String(s) => Ok(vec![s.to_str()?.to_owned()]),
        Value::Table(list) => list
            .sequence_values::<Value>()
            .map(|v| match v? {
                Value::String(s) => Ok(s.to_str()?.to_owned()),
                other => Err(error(
                    function,
                    format!("`{key}` must hold strings, got {}", other.type_name()),
                )),
            })
            .collect(),
        other => Err(error(
            function,
            format!(
                "`{key}` must be a list of strings, got {}",
                other.type_name()
            ),
        )),
    }
}

/// The radix field (`hex`, `bin`, `dec`; `hex` when absent).
fn radix(args: &Table, function: &str) -> mlua::Result<Radix> {
    match text(args, function, "radix")? {
        Some(name) => serde_json::from_value(json!(name))
            .map_err(|_| error(function, format!("unknown `radix` `{name}`"))),
        None => Ok(Radix::default()),
    }
}

fn waveform(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "waveform";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_wave(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let request = WaveRequest {
        path: path.clone(),
        signals: strings(args, F, "signals")?,
        radix: radix(args, F)?,
    };
    let mut spec = services
        .formats
        .parse_waveform(&bytes, &request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Waveform(Box::new(spec)))
}

fn step(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "step";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let tolerance = match args.get::<Value>("tolerance")? {
        Value::Nil => None,
        Value::Integer(n) => Some(n as f64),
        Value::Number(n) if n.is_finite() && n > 0.0 => Some(n),
        _ => return Err(error(F, "`tolerance` must be a positive number")),
    };
    if tolerance.is_some_and(|t| t <= 0.0) {
        return Err(error(F, "`tolerance` must be a positive number"));
    }
    let bytes = context(lua)?
        .block_on(read_step(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let request = StepRequest {
        path: path.clone(),
        tolerance,
    };
    let mut spec = services
        .formats
        .parse_step(&bytes, &request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::CadModel(Box::new(spec)))
}

fn dxf(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "dxf";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_report(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_dxf(&bytes, &path)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::Drawing(Box::new(spec)))
}

fn ifc_schedule(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "ifc_schedule";
    let services = services(lua, F)?;
    let path = text(args, F, "path")?.ok_or_else(|| error(F, "`path` is required"))?;
    let bytes = context(lua)?
        .block_on(read_ifc(services.fs.as_ref(), &path))?
        .map_err(|e| error(F, e))?;
    let mut spec = services
        .formats
        .parse_ifc_schedule(&bytes, &path)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate()
        .map_err(|e| error(F, format!("`{path}`: {e}")))?;
    block_value(lua, &BlockKind::IfcSchedule(Box::new(spec)))
}

fn netlist(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "netlist";
    let services = services(lua, F)?;
    let (path, source) = path_or_inline(lua, args, F, "source")?;
    let format = match text(args, F, "format")? {
        Some(name) => Some(
            serde_json::from_value(json!(name))
                .map_err(|_| error(F, format!("unknown `format` `{name}`")))?,
        ),
        None => None,
    };
    let mut spec = services
        .formats
        .parse_netlist(&source, format, path.as_deref())
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Netlist(Box::new(spec)))
}

fn sequence(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "sequence";
    let services = services(lua, F)?;
    let (path, source) = path_or_inline(lua, args, F, "sequence")?;
    let record = match args.get::<Option<i64>>("record")? {
        Some(n) if n >= 1 => usize::try_from(n - 1).unwrap_or(0),
        Some(_) => return Err(error(F, "`record` counts from 1")),
        None => 0,
    };
    let alphabet = match text(args, F, "alphabet")? {
        Some(name) => Some(
            serde_json::from_value(json!(name.to_ascii_lowercase()))
                .map_err(|_| error(F, format!("unknown `alphabet` `{name}`")))?,
        ),
        None => None,
    };
    let topology = args.get::<Option<bool>>("circular")?.map(|c| {
        if c {
            domain::science::sequence::Topology::Circular
        } else {
            domain::science::sequence::Topology::Linear
        }
    });
    let request = SequenceRequest {
        path,
        record,
        alphabet,
        topology,
        enzymes: args.get::<Option<bool>>("enzymes")?.unwrap_or(true),
    };
    let mut spec = services
        .formats
        .parse_sequence(source.as_bytes(), &request)
        .map_err(|e| error(F, e))?;
    spec.title = text(args, F, "title")?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Sequence(Box::new(spec)))
}

fn reaction(lua: &Lua, args: &Table) -> mlua::Result<Value> {
    const F: &str = "reaction";
    let services = services(lua, F)?;
    let smiles = text(args, F, "smiles")?.ok_or_else(|| error(F, "`smiles` is required"))?;
    let coefficients = match args.get::<Value>("coefficients")? {
        Value::Nil => None,
        Value::Table(t) => Some(
            t.sequence_values::<u32>()
                .collect::<mlua::Result<Vec<_>>>()
                .map_err(|_| error(F, "`coefficients` must be a list of positive integers"))?,
        ),
        other => {
            return Err(error(
                F,
                format!("`coefficients` must be a list, got {}", other.type_name()),
            ));
        }
    };
    let mut spec = services
        .formats
        .depict_reaction(&smiles, coefficients.as_deref());
    if let Some(e) = &spec.error {
        return Err(error(F, e));
    }
    spec.title = text(args, F, "title")?;
    spec.conditions = text(args, F, "conditions")?;
    let mut names = strings(args, F, "names")?.into_iter();
    for c in &mut spec.components {
        c.molecule.name = names.next().filter(|n| !n.trim().is_empty());
    }
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Reaction(Box::new(spec)))
}

/// The `app.formats` table.
pub(crate) fn table(lua: &Lua) -> mlua::Result<Table> {
    let formats = lua.create_table()?;
    formats.set(
        "reaction",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => reaction(lua, &args),
            _ => Err(error("reaction", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "sequence",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => sequence(lua, &args),
            _ => Err(error("sequence", "expected a table of arguments")),
        })?,
    )?;
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
        "benchmarks",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => benchmarks(lua, &args),
            _ => Err(error("benchmarks", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "waveform",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => waveform(lua, &args),
            _ => Err(error("waveform", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "step",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => step(lua, &args),
            _ => Err(error("step", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "dxf",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => dxf(lua, &args),
            _ => Err(error("dxf", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "ifc_schedule",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => ifc_schedule(lua, &args),
            _ => Err(error("ifc_schedule", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "netlist",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => netlist(lua, &args),
            _ => Err(error("netlist", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "dot",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => dot(lua, &args),
            _ => Err(error("dot", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "er",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => er(lua, &args),
            _ => Err(error("er", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "terraform_plan",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => terraform_plan(lua, &args),
            _ => Err(error("terraform_plan", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "har",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => har(lua, &args),
            _ => Err(error("har", "expected a table of arguments")),
        })?,
    )?;
    formats.set(
        "query_plan",
        lua.create_function(|lua, args: Value| match args {
            Value::Table(args) => query_plan(lua, &args),
            _ => Err(error("query_plan", "expected a table of arguments")),
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
            .with_file("plans/q.json", "[]")
            .with_file("net/app.har", "{}")
            .with_file("infra/plan.json", "{}")
            .with_file("db/schema.sql", "create table a (id int);")
            .with_file("target/criterion/fib/new/estimates.json", "{}")
            .with_file("sim/tb.vcd", "$enddefinitions $end")
            .with_file("cad/block.step", "ISO-10303-21;\n")
            .with_file("dwg/plate.dxf", "0\nEOF\n")
            .with_file("bim/house.ifc", "ISO-10303-21;\n")
            .with_file("dna/p.gb", "LOCUS x\n")
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
    fn query_plans_come_from_a_file_or_inline() {
        let formats = Arc::new(FakeFormats::new());
        let out = run_code(
            "return app.formats.query_plan{ path = 'plans/q.json', title = 'slow' }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::QueryPlan(spec)) = out else {
            panic!("not a plan: {out:?}");
        };
        assert_eq!(spec.title.as_deref(), Some("slow"));
        let out = run_code(
            "return app.formats.query_plan{ plan = '[]' }",
            Some(Arc::clone(&formats)),
        )
        .unwrap();
        assert!(
            matches!(out, PluginOutput::Block(BlockKind::QueryPlan(_))),
            "{out:?}"
        );
        let both = run_code(
            "return app.formats.query_plan{ plan = '[]', path = 'x' }",
            Some(formats),
        )
        .unwrap_err()
        .to_string();
        assert!(both.contains("not both"), "{both}");
        let out = run_code(
            "return app.formats.har{ path = 'net/app.har' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Har(spec)) = out else {
            panic!("not a HAR: {out:?}");
        };
        assert_eq!(spec.entries.len(), 2);
        let out = run_code(
            "return app.formats.terraform_plan{ path = 'infra/plan.json' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::TerraformPlan(spec)) = out else {
            panic!("not a plan: {out:?}");
        };
        assert_eq!(spec.resources.len(), 3);
        let out = run_code(
            "return app.formats.dot{ source = 'digraph { a -> b }', title = 'g' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Diagram(spec)) = out else {
            panic!("not a diagram: {out:?}");
        };
        assert_eq!(spec.title.as_deref(), Some("g"));
        let out = run_code(
            "return app.formats.er{ path = 'db/schema.sql', dialect = 'postgresql' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Diagram(spec)) = out else {
            panic!("not a diagram: {out:?}");
        };
        assert_eq!(spec.tables.len(), 2);
        let bad = run_code(
            "return app.formats.er{}",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap_err()
        .to_string();
        assert!(bad.contains("`path` or `sql`"), "{bad}");
        let out = run_code(
            "return app.formats.benchmarks{}",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Benchmarks(spec)) = out else {
            panic!("not benchmarks: {out:?}");
        };
        assert_eq!(
            (spec.path.as_str(), spec.baseline.as_str()),
            ("target/criterion", "base")
        );
        let out = run_code(
            "return app.formats.waveform{ path = 'sim/tb.vcd', signals = { 'tb.*' }, radix = 'dec' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Waveform(spec)) = out else {
            panic!("not a waveform: {out:?}");
        };
        assert_eq!(spec.radix, domain::engineering::waveform::Radix::Dec);
        let bad = run_code(
            "return app.formats.waveform{ path = 'sim/tb.vcd', radix = 'oct' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap_err()
        .to_string();
        assert!(bad.contains("unknown `radix`"), "{bad}");
        let out = run_code(
            "return app.formats.netlist{ source = 'div\\nR1 a 0 1k', format = 'spice' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Netlist(spec)) = out else {
            panic!("not a netlist: {out:?}");
        };
        assert_eq!(spec.components.len(), 4);
        let out = run_code(
            "return app.formats.step{ path = 'cad/block.step', tolerance = 0.5, title = 'b' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::CadModel(spec)) = out else {
            panic!("not a part: {out:?}");
        };
        assert_eq!((spec.tolerance, spec.title.as_deref()), (0.5, Some("b")));
        let bad = run_code(
            "return app.formats.step{ path = 'cad/block.step', tolerance = -1 }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap_err()
        .to_string();
        assert!(bad.contains("`tolerance`"), "{bad}");
        let out = run_code(
            "return app.formats.dxf{ path = 'dwg/plate.dxf' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Drawing(spec)) = out else {
            panic!("not a drawing: {out:?}");
        };
        assert_eq!(spec.layers.len(), 2);
        let out = run_code(
            "return app.formats.ifc_schedule{ path = 'bim/house.ifc', title = 'h' }",
            Some(Arc::new(FakeFormats::new())),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::IfcSchedule(spec)) = out else {
            panic!("not a schedule: {out:?}");
        };
        assert_eq!(
            (spec.missing_areas(), spec.title.as_deref()),
            (1, Some("h"))
        );
    }

    #[test]
    fn science_viewers_return_their_blocks() {
        let formats = || Some(Arc::new(FakeFormats::new()));
        let out = run_code(
            "return app.formats.sequence{ path = 'dna/p.gb', record = 1, circular = false }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Sequence(spec)) = out else {
            panic!("not a sequence: {out:?}");
        };
        assert_eq!((spec.len(), spec.is_circular()), (30, false));
        let err = run_code("return app.formats.sequence{ sequence = 'ACGT', record = 0 }", formats())
            .unwrap_err();
        assert!(err.to_string().contains("counts from 1"), "{err}");
        let out = run_code(
            "return app.formats.reaction{ smiles = '[H][H].O=O>>O', names = { 'h2' } }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Reaction(spec)) = out else {
            panic!("not a reaction: {out:?}");
        };
        assert_eq!(spec.components[0].molecule.name.as_deref(), Some("h2"));
        let err = run_code("return app.formats.reaction{ smiles = 'bad' }", formats()).unwrap_err();
        assert!(err.to_string().contains("invalid reaction"), "{err}");
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
