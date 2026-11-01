//! `app.formats`: the file-format blocks of the `show_dataset`,
//! `show_molecule`, `show_trace`, `show_flamegraph`, `show_logs`,
//! `show_query_plan`, `show_har`, `show_terraform_plan`, `show_dot`,
//! `show_er`, `show_benchmarks`, `show_waveform`, `show_netlist`,
//! `show_step`, `show_dxf`, `show_ifc_schedule`, `show_sequence`,
//! `show_reaction`, `show_molecule_grid`, `show_spectrum`,
//! `show_molecule_3d`, `show_ghs`, `show_map`, `show_gantt`,
//! `show_array_file`, `show_network` and `show_training_curves` tools
//! from Lua. Each returns a block (wrapped like
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
//! return app.formats.molecule_grid{ molecules = { { id = 'A1', smiles = 'CCO', activity = 12 } } }
//! return app.formats.spectrum{ path = 'spectra/ir.jdx' }
//! return app.formats.molecule_3d{ smiles = 'CCO' }
//! return app.formats.ghs{ query = 'acetone' }
//! return app.formats.map{ path = 'geo/parks.geojson', color_by = 'kind' }
//! return app.formats.array_file{ path = 'data/run.h5', variable = '/sim/field' }
//! return app.formats.training_curves{ paths = { 'runs/a.csv', 'runs/b.csv' }, metric = 'val_loss' }
//! return app.formats.network{ edges = { { source = 'a', target = 'b' } }, size_by = 'pagerank' }
//! return app.formats.gantt{ tasks = { { id = 'a', duration = 3 }, { id = 'b', duration = 2, depends_on = { 'a' } } } }
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
//! | `molecule_grid{ molecules or path, activity_column, activity_label, activity_unit, direction, title }` | a SAR grid with computed descriptors (`molecules`: `{ id, smiles, name, activity }` tables; `path`: CSV/TSV with a `smiles` column; `direction`: `higher` or `lower`) |
//! | `spectrum{ path, block, threshold, max_peaks, title }` | a JCAMP-DX (IR, Raman, UV-Vis, NMR), mzML or MGF spectrum with its picked peaks (`block` 1-based) |
//! | `molecule_3d{ smiles or path, name, title }` | ball and stick in the 3D view: a SMILES embedded by the engine, or a MOL/SDF, XYZ or PDB file's coordinates |
//! | `ghs{ query or cid }` | the GHS hazard card PubChem gives for a name, CAS number or CID, with its source and date (needs the web on; a compound PubChem does not know is a card that says so) |
//! | `map{ path or geojson, crs, color_by, title }` | GeoJSON on a map with geodesic area, length and box (`geojson`: text or a table; `crs`: an EPSG code or PROJ string over the file's) |
//! | `gantt{ path or tasks, start, calendar, title }` | a project scheduled by the critical path method on a Gantt chart (`tasks`: `{ id, name, group, duration, start, finish, depends_on, milestone }`; `depends_on` like `'a'`, `'a SS+2'`; `calendar`: `all_days` or `weekdays`); raises on a cycle |
//! | `array_file{ path, variable, title }` | an HDF5, NetCDF-4 or NetCDF-3 file's tree with bounded previews and their statistics (of the preview only) |
//! | `network{ path or edges, directed, source, target, weight, size_by, seed, title }` | a network (edge-list CSV or node-link JSON, or `{ source, target, weight }` tables) with degree, betweenness, closeness, PageRank and communities; `size_by` picks the metric nodes are sized by |
//! | `training_curves{ path or paths, metric, x, smoothing, log_y, goal, title }` | ML training logs (CSV or JSON lines) as smoothed curves per run with the engine's min/max/last/best (`x`: `step`, `epoch`, `time`; `goal`: `minimize`, `maximize`) |
//!
//! The path is read through the same guard as the model's `read_file`.
//! The block is in `result["$crest_block"]` (e.g. its `formula`). A file
//! that cannot be read or parsed, or a SMILES that does not parse,
//! raises a Lua error with the reason. The calls block the VM while they
//! read and parse.
//!
//! The arguments are the tool's input (its JSON schema, read from the
//! table): an unknown key or a wrong type raises, and the checks, limits
//! and messages are the tool's own, from the shared `normalize_*` /
//! `build_*` of `application::tools::formats`. Only `waveform{ signals }`
//! and `reaction{ names }` also take a single string for their list.

use std::sync::Arc;

use application::ports::formats::FormatsPort;
use application::ports::fs::FileSystemPort;
use application::tools::formats::{
    FormatToolPorts, arrays, bench, build_dataset, build_molecule, devops, diagram, dxf, gantt,
    geomap, har, ifc, molecule_grid, molecule3d, netlist, network, normalize_dataset,
    normalize_molecule, query_plan, reaction, sequence, spectrum, step, terraform, training,
    waveform,
};
use application::tools::ghs::{ShowGhs, lookup, target};
use application::usecases::compute::inline_offload;
use domain::block::BlockKind;
use mlua::{Lua, Table, Value};
use serde::de::DeserializeOwned;
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
    /// The web, when `app.opt.web` has it on (PubChem lookups).
    pub web: Option<Arc<dyn application::ports::web::WebPort>>,
}

impl std::fmt::Debug for FormatsServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormatsServices")
    }
}

impl FormatsServices {
    /// The ports the format builders run on. The Lua thread already
    /// blocks while a call runs, so the parsers run inline on it.
    fn ports(&self) -> FormatToolPorts {
        FormatToolPorts {
            formats: Arc::clone(&self.formats),
            fs: Arc::clone(&self.fs),
            offload: inline_offload(),
        }
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

/// The table of arguments of `function` as the tool's input `I`: unknown
/// keys, missing fields and wrong types fail as they do for the tool,
/// with the field's path (`signals[1]: invalid type …`, counted from 0).
fn input<I: DeserializeOwned>(function: &str, args: Value) -> mlua::Result<I> {
    if !args.is_table() {
        return Err(error(function, "expected a table of arguments"));
    }
    serde_path_to_error::deserialize(mlua::serde::Deserializer::new(args)).map_err(|e| {
        let path = e.path().to_string();
        let message = match e.into_inner() {
            mlua::Error::DeserializeError(message) => message,
            other => other.to_string(),
        };
        if path == "." {
            error(function, message)
        } else {
            error(function, format!("{path}: {message}"))
        }
    })
}

/// Runs `function` the way its tool does: `args` as the tool's input,
/// checked by `normalize`, built by `build` (reading and parsing while
/// the VM waits) and returned as the block `block` makes of the spec.
/// Messages are the tool's, prefixed with `app.formats.{function}`.
fn show<I, C, S>(
    lua: &Lua,
    function: &str,
    args: Value,
    normalize: fn(I) -> Result<C, String>,
    build: impl AsyncFnOnce(&FormatToolPorts, C) -> Result<S, String>,
    block: fn(S) -> BlockKind,
) -> mlua::Result<Value>
where
    I: DeserializeOwned,
{
    let ports = services(lua, function)?.ports();
    let call = normalize(input(function, args)?).map_err(|e| error(function, e))?;
    let spec = context(lua)?
        .block_on(build(&ports, call))?
        .map_err(|e| error(function, e))?;
    block_value(lua, &block(spec))
}

/// Adds `function` to `formats`, run by [`show`].
fn register<I, C, S>(
    lua: &Lua,
    formats: &Table,
    function: &'static str,
    normalize: fn(I) -> Result<C, String>,
    build: impl AsyncFn(&FormatToolPorts, C) -> Result<S, String> + Send + 'static,
    block: fn(S) -> BlockKind,
) -> mlua::Result<()>
where
    I: DeserializeOwned + 'static,
    C: 'static,
    S: 'static,
{
    let run = lua.create_function(move |lua, args: Value| {
        show(lua, function, args, normalize, &build, block)
    })?;
    formats.set(function, run)
}

/// `block` as the table `app.ui.block` returns.
pub(crate) fn block_value(lua: &Lua, block: &BlockKind) -> mlua::Result<Value> {
    let json = serde_json::to_value(block).map_err(mlua::Error::external)?;
    convert::to_lua(lua, &json!({ BLOCK_KEY: json }))
}

/// `args` with `key` as a list: Lua also takes one item as a plain
/// string (`signals = 'tb.*'`). A copy, so the caller's table is left
/// as it was.
fn one_or_many(lua: &Lua, args: Value, key: &str) -> mlua::Result<Value> {
    let Value::Table(table) = &args else {
        return Ok(args);
    };
    let item = table.get::<Value>(key)?;
    if !item.is_string() {
        return Ok(args);
    }
    let copy = lua.create_table()?;
    for pair in table.pairs::<Value, Value>() {
        let (k, v) = pair?;
        copy.set(k, v)?;
    }
    copy.set(key, lua.create_sequence_from([item])?)?;
    Ok(Value::Table(copy))
}

fn waveform(lua: &Lua, args: Value) -> mlua::Result<Value> {
    let args = one_or_many(lua, args, "signals")?;
    show(
        lua,
        "waveform",
        args,
        waveform::normalize_waveform,
        waveform::build_waveform,
        |spec| BlockKind::Waveform(Box::new(spec)),
    )
}

fn reaction(lua: &Lua, args: Value) -> mlua::Result<Value> {
    let args = one_or_many(lua, args, "names")?;
    // A reaction that does not parse raises here; the tool draws it with
    // its error for the model.
    show(
        lua,
        "reaction",
        args,
        reaction::normalize_reaction,
        async |ports, input| {
            let spec = reaction::build_reaction(ports, input).await?;
            match spec.error {
                Some(e) => Err(e),
                None => Ok(spec),
            }
        },
        |spec| BlockKind::Reaction(Box::new(spec)),
    )
}

fn ghs(lua: &Lua, args: Value) -> mlua::Result<Value> {
    const F: &str = "ghs";
    let web = services(lua, F)?
        .web
        .ok_or_else(|| error(F, "the web is off (app.opt.web.enabled)"))?;
    let input: ShowGhs = input(F, args)?;
    let (query, cid) = target(input.query, input.cid).map_err(|e| error(F, e))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let spec = context(lua)?
        .block_on(lookup(web.as_ref(), &query, cid, now))?
        .map_err(|e| error(F, e))?;
    spec.validate().map_err(|e| error(F, e))?;
    block_value(lua, &BlockKind::Ghs(Box::new(spec)))
}

/// `register`s each `"name" => Variant(normalize, build)`: the tool's
/// builder, its spec boxed in `BlockKind::Variant`.
macro_rules! boxed {
    (
        $lua:expr, $formats:expr;
        $($name:literal => $variant:ident($normalize:path, $build:path)),* $(,)?
    ) => {
        $(register($lua, $formats, $name, $normalize, $build, |spec| {
            BlockKind::$variant(Box::new(spec))
        })?;)*
    };
}

/// The `app.formats` table.
pub(crate) fn table(lua: &Lua) -> mlua::Result<Table> {
    let formats = lua.create_table()?;
    register(
        lua,
        &formats,
        "dataset",
        normalize_dataset,
        build_dataset,
        BlockKind::Dataset,
    )?;
    register(
        lua,
        &formats,
        "molecule",
        normalize_molecule,
        // A SMILES that does not parse raises here; the tool draws it
        // with its error for the model.
        async |ports, input| {
            let spec = build_molecule(ports, input).await?;
            match &spec.error {
                Some(e) => Err(format!("`{}`: {e}", spec.smiles)),
                None => Ok(spec),
            }
        },
        BlockKind::Molecule,
    )?;
    boxed!(lua, &formats;
        "trace" => Trace(devops::normalize_trace, devops::build_trace),
        "flamegraph" => FlameGraph(devops::normalize_flamegraph, devops::build_flamegraph),
        "logs" => Logs(devops::normalize_logs, devops::build_logs),
        "query_plan" => QueryPlan(
            query_plan::normalize_query_plan,
            query_plan::build_query_plan
        ),
        "har" => Har(har::normalize_har, har::build_har),
        "terraform_plan" => TerraformPlan(
            terraform::normalize_terraform_plan,
            terraform::build_terraform_plan
        ),
        "dot" => Diagram(diagram::normalize_dot, diagram::build_dot),
        "er" => Diagram(diagram::normalize_er, diagram::build_er),
        "benchmarks" => Benchmarks(bench::normalize_benchmarks, bench::build_benchmarks),
        "netlist" => Netlist(netlist::normalize_netlist, netlist::build_netlist),
        "step" => CadModel(step::normalize_step, step::build_step),
        "dxf" => Drawing(dxf::normalize_dxf, dxf::build_dxf),
        "ifc_schedule" => IfcSchedule(ifc::normalize_ifc_schedule, ifc::build_ifc_schedule),
        "sequence" => Sequence(sequence::normalize_sequence, sequence::build_sequence),
        "molecule_grid" => MoleculeGrid(
            molecule_grid::normalize_molecule_grid,
            molecule_grid::build_molecule_grid
        ),
        "spectrum" => Spectrum(spectrum::normalize_spectrum, spectrum::build_spectrum),
        "molecule_3d" => Molecule3d(
            molecule3d::normalize_molecule_3d,
            molecule3d::build_molecule_3d
        ),
        "map" => GeoMap(geomap::normalize_map, geomap::build_map),
        "gantt" => Gantt(gantt::normalize_gantt, gantt::build_gantt),
        "array_file" => ArrayFile(arrays::normalize_array_file, arrays::build_array_file),
        "network" => Network(network::normalize_network, network::build_network),
        "training_curves" => TrainingCurves(
            training::normalize_training_curves,
            training::build_training_curves
        ),
    );
    formats.set("waveform", lua.create_function(waveform)?)?;
    formats.set("reaction", lua.create_function(reaction)?)?;
    formats.set("ghs", lua.create_function(ghs)?)?;
    Ok(formats)
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::PluginOutput;
    use application_testing::formats::FakeFormats;
    use application_testing::fs::{InMemoryFs, test_root};
    use domain::devops::logs::LogFormat;

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
            .with_file("spectra/ir.jdx", "##TITLE=x\n")
            .with_file("geo/parks.geojson", "{}")
            .with_file("data/run.h5", "x")
            .with_file("runs/a.csv", "step,loss\n")
            .with_file(".env", "a\n1\n");
        let ctx = Context::new(None, false, Some(Vec::new()));
        if let Some(formats) = formats {
            ctx.provide_formats(FormatsServices {
                formats,
                fs: Arc::new(fs),
                web: Some(Arc::new(application_testing::web::ScriptedWeb::new())),
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
        assert!(bad.contains("radix: unknown variant `oct`"), "{bad}");
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
        let err = run_code(
            "return app.formats.sequence{ sequence = 'ACGT', record = 0 }",
            formats(),
        )
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
        let out = run_code(
            "return app.formats.molecule_grid{ molecules = { { id = 'A1', smiles = 'CCO', \
             activity = 12 } }, direction = 'lower' }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::MoleculeGrid(spec)) = out else {
            panic!("not a grid: {out:?}");
        };
        assert_eq!(spec.molecules[0].activity, Some(12.0));
        assert!(run_code("return app.formats.molecule_grid{}", formats()).is_err());
        let out = run_code(
            "return app.formats.spectrum{ path = 'spectra/ir.jdx', max_peaks = 5 }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Spectrum(spec)) = out else {
            panic!("not a spectrum: {out:?}");
        };
        assert_eq!(spec.peaks.len(), 1);
        let out = run_code(
            "return app.formats.molecule_3d{ smiles = 'O', name = 'w' }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Molecule3d(spec)) = out else {
            panic!("not a 3D molecule: {out:?}");
        };
        assert_eq!((spec.atoms.len(), spec.name.as_deref()), (3, Some("w")));
        // The scripted web knows nothing: the card says so.
        let out = run_code("return app.formats.ghs{ query = 'unobtainium' }", formats()).unwrap();
        let PluginOutput::Block(BlockKind::Ghs(spec)) = out else {
            panic!("not a GHS card: {out:?}");
        };
        assert!(!spec.found());
        assert!(run_code("return app.formats.ghs{}", formats()).is_err());
        sciences_b(formats);
    }

    /// The science B functions, each returning its block.
    fn sciences_b(formats: impl Fn() -> Option<Arc<FakeFormats>>) {
        let out = run_code(
            "return app.formats.map{ path = 'geo/parks.geojson', color_by = 'kind' }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::GeoMap(spec)) = out else {
            panic!("not a map: {out:?}");
        };
        assert_eq!(spec.color_by.as_deref(), Some("kind"));
        let out = run_code(
            "return app.formats.map{ geojson = { type = 'Point', coordinates = { 1, 2 } } }",
            formats(),
        )
        .unwrap();
        assert!(matches!(out, PluginOutput::Block(BlockKind::GeoMap(_))));
        assert!(run_code("return app.formats.map{}", formats()).is_err());
        let out = run_code(
            "return app.formats.gantt{ tasks = { { id = 'a', duration = 3 } }, \
             calendar = 'weekdays', start = '2026-11-02' }",
            formats(),
        )
        .unwrap();
        let PluginOutput::Block(BlockKind::Gantt(spec)) = out else {
            panic!("not a Gantt chart: {out:?}");
        };
        assert_eq!(spec.critical_path, vec!["design", "build"]);
        assert!(
            run_code(
                "return app.formats.gantt{ tasks = {}, calendar = 'x' }",
                formats()
            )
            .is_err()
        );
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
        assert!(
            bad.contains("format: unknown variant `xml`, expected one of `json`, `logfmt`, `text`"),
            "{bad}"
        );
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
        assert!(err("return app.formats.dataset{}").contains("missing field `path`"));
        assert!(err("return app.formats.dataset{ path = ' ' }").contains("`path` is empty"));
        assert!(
            err("return app.formats.dataset{ path = 'data/cars.csv', rows = 0 }")
                .contains("`rows` must be at least 1")
        );
        assert!(
            err("return app.formats.dataset{ path = 'data/cars.csv', row = 2 }")
                .contains("unknown field `row`")
        );
        assert!(
            err("return app.formats.benchmarks{ baseline = '../x' }")
                .contains("app.formats.benchmarks: `baseline` is a baseline name")
        );
        assert!(
            err("return app.formats.waveform{ path = 'sim/tb.vcd', signals = { 'a', 2 } }")
                .contains("signals[1]: invalid type")
        );
        assert!(err("return app.formats.molecule(1)").contains("expected a table"));
        assert!(err("return app.formats.molecule{ smiles = 'bad' }").contains("invalid SMILES"));
        let failing = run_code(
            "return app.formats.dataset{ path = 'data/cars.csv' }",
            Some(Arc::new(FakeFormats::failing("broken row"))),
        )
        .unwrap_err()
        .to_string();
        assert!(failing.contains("broken row"), "{failing}");
        // One pattern may be a plain string, as before the shared builders.
        let one = run_code(
            "return app.formats.waveform{ path = 'sim/tb.vcd', signals = 'tb.*' }",
            Some(Arc::new(FakeFormats::new())),
        );
        assert!(one.is_ok(), "{one:?}");
        let many = format!(
            "return app.formats.waveform{{ path = 'sim/tb.vcd', signals = {{ {} }} }}",
            vec!["'a'"; 65].join(", ")
        );
        assert!(err(&many).contains("at most 64 signal patterns"));
        let none = run_code("return app.formats.molecule{ smiles = 'C' }", None)
            .unwrap_err()
            .to_string();
        assert!(none.contains("not available"), "{none}");
    }
}
