//! `app.compute`: the compute engine from Lua, the same calls the
//! `stats_*`, `calc`, `convert` and solver card tools make. Each returns a
//! computed block (a statistics card, a calc sheet, a conversion or a
//! solver card), so a tool or command handler that returns it shows the
//! card, and the `reproduce.lua` line of every card runs as written.
//!
//! ```lua
//! return app.compute.test{ data = 'data/cars.csv', y = 'mpg', group = 'am' }
//! return app.compute.describe{
//!   data = { columns = { { name = 'x', values = { 1.5, nil, 3 } } } },
//! }
//! ```
//!
//! | Function | Request |
//! | --- | --- |
//! | `describe{ data, variables, by }` | descriptive statistics |
//! | `test{ data, test, y, group, y2, paired, mu, table, alternative, ... }` | hypothesis test |
//! | `regress{ data, y, x, conf_level }` | OLS regression |
//! | `correlate{ data, variables, method, adjustment, conf_level }` | correlation matrix |
//! | `calc{ lines, significant_digits }` | calc sheet with units (`lines = { 'v = 100 km/h', … }`) |
//! | `convert{ value, from, to, uncertainty, significant_digits }` | unit conversion |
//! | `distribution{ data, variable, by, bin_rule, bins, bandwidth_rule, bandwidth, conf_level }` | histogram, density, box/violin, QQ, ECDF |
//! | `contingency{ data, row, column, table, row_labels, column_labels, test, correct, alternative, conf_level, seed, simulations }` | contingency table, χ² or Fisher |
//! | `power{ test, n, effect, p1, p2, alpha, power, groups, alternative, solve_for }` | power or sample size |
//! | `ab_test{ control = { name, visitors, conversions }, treatment = {...}, expected_split, conf_level, alternative, looks, planned_visitors, minimum_lift }` | A/B test |
//! | `bootstrap{ data, y, statistic, group, y2, levels, trim, resamples, seed, conf_level }` | seeded bootstrap |
//! | `forecast{ data, y, time, periods, horizon, holdout }` | MSTL decomposition and forecast |
//! | `optimize{ sense, objective, variables = { { name, lower, upper, integer, binary } }, constraints = { { name, expr } } }` | LP/MILP with duals |
//! | `solve{ equations, unknowns = { { name, guess } }, range, tolerance, max_iterations }` | equation solver |
//! | `simulate{ inputs = { { name, distribution = { type = 'normal', mean, sd }, source, unit } }, model, output, unit, samples, seed }` | Monte Carlo, tornado, Fermi |
//! | `cash_flow{ flows, rate, finance_rate, reinvest_rate, currency, period }` | NPV, IRR, MIRR, payback |
//! | `decide{ criteria = { { name, weight, direction } }, options = { { name, scores } }, normalization }` | decision matrix |
//! | `bode{ numerator, denominator, zeros, poles, gain, delay, w_min, w_max, points, unit }` | Bode / Nyquist, margins |
//! | `karnaugh{ variables, minterms, maxterms, dont_cares, expression }` | Karnaugh map, minimal SOP and POS |
//! | `beam{ length, length_unit, force_unit, supports = { { at, kind } }, loads = { { kind, at, end, value, end_value } }, e, e_unit, i, i_unit, segments }` | beam reactions, shear, moment, deflection |
//! | `tolerance{ dimensions = { { name, nominal, tolerance, plus, minus, direction, distribution } }, lower_limit, upper_limit, unit, sigmas, samples, seed }` | tolerance stack-up |
//! | `bom{ data, items = { { part, quantity, unit_cost, category, reference, mpn } }, part, quantity, unit_cost, category, reference, mpn, builds, currency }` | BOM costing (`data` may be a `.json` array of records) |
//!
//! `data` is a CSV/TSV path of the workspace (read through the same guard
//! as the model's `read_file`; `delimiter` picks the separator) or a table
//! `{ columns = { { name, values, unit }, ... } }` whose `values` may have
//! `nil` holes (missing values). Columns shorter than the longest are
//! padded with missing values, so a trailing `nil` is not lost when
//! another column has that row. The other fields are those of the tools.
//! `calc`, `convert`, `power`, `ab_test`, `optimize`, `solve`, `simulate`,
//! `cash_flow`, `decide`, `bode`, `karnaugh`, `beam` and `tolerance` read
//! no data.
//! The envelope is in `result["$crest_block"].envelope` (its `values`,
//! `method`, `warnings`...). A request the engine rejects raises a Lua
//! error with the reason. The calls block the VM while the engine runs.

use std::sync::Arc;

use application::ports::compute::{ComputePort, ComputeRequest};
use application::ports::fs::FileSystemPort;
use application::usecases::compute::{DELIMITER_PARAM, load_file, set_provenance};
use domain::block::BlockKind;
use mlua::{Lua, Table, Value};
use serde_json::{Map, Value as Json, json};

use super::ui::BLOCK_KEY;
use crate::caps::context;
use crate::config::api::Origin;
use crate::convert;

/// What `app.compute` runs on, handed to the host by the binary.
#[derive(Clone)]
pub struct ComputeServices {
    /// The engine.
    pub engine: Arc<dyn ComputePort>,
    /// Reads data files of the workspace.
    pub fs: Arc<dyn FileSystemPort>,
}

impl std::fmt::Debug for ComputeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComputeServices")
    }
}

/// Who `app.compute` records as having asked.
const REQUESTED_BY: &str = "plugin";

fn error(function: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.compute.{function}: {message}"))
}

/// The values of a column table: by integer key, so `nil` holes count,
/// up to the largest index.
fn column_values(function: &str, values: &Table) -> mlua::Result<Vec<Json>> {
    let mut cells: Vec<(usize, Json)> = Vec::new();
    for pair in values.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let index = match key {
            Value::Integer(i) if i >= 1 => usize::try_from(i).unwrap_or(usize::MAX),
            Value::Number(n) if n >= 1.0 && n.fract() == 0.0 => {
                // A whole positive float index; Lua tables may store them.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let i = n as usize;
                i
            }
            other => {
                return Err(error(
                    function,
                    format!("`values` must be a list, found key {}", other.type_name()),
                ));
            }
        };
        let cell = match value {
            Value::Integer(i) => json!(i),
            Value::Number(n) => json!(n),
            Value::String(s) => Json::String(s.to_str()?.to_owned()),
            Value::Boolean(b) => Json::String(b.to_string()),
            other => {
                return Err(error(
                    function,
                    format!(
                        "values must be numbers or strings, got {}",
                        other.type_name()
                    ),
                ));
            }
        };
        cells.push((index, cell));
    }
    let len = cells.iter().map(|(i, _)| *i).max().unwrap_or_default();
    if len > 10_000_000 {
        return Err(error(function, "`values` is too long"));
    }
    let mut out = vec![Json::Null; len];
    for (index, cell) in cells {
        out[index - 1] = cell;
    }
    // A column with any text is categorical: its numbers become text.
    if out.iter().any(Json::is_string) {
        for cell in &mut out {
            if let Json::Number(n) = cell {
                *cell = Json::String(n.to_string());
            }
        }
    }
    Ok(out)
}

/// `data = { columns = { ... } }` as the request's JSON.
fn inline_data(function: &str, data: &Table) -> mlua::Result<Json> {
    let columns: Table = match data.get::<Value>("columns")? {
        Value::Table(columns) => columns,
        _ => return Err(error(function, "`data.columns` must be a list of columns")),
    };
    let mut parsed = Vec::new();
    for (ix, column) in columns.sequence_values::<Value>().enumerate() {
        let Value::Table(column) = column? else {
            return Err(error(
                function,
                format!("column {} must be a table", ix + 1),
            ));
        };
        let name: String = column
            .get::<Option<String>>("name")?
            .ok_or_else(|| error(function, format!("column {} has no `name`", ix + 1)))?;
        let values = match column.get::<Value>("values")? {
            Value::Table(values) => column_values(function, &values)?,
            _ => {
                return Err(error(
                    function,
                    format!("column `{name}` has no `values` list"),
                ));
            }
        };
        let mut entry = Map::new();
        entry.insert("name".into(), Json::String(name));
        entry.insert("values".into(), Json::Array(values));
        if let Some(unit) = column.get::<Option<String>>("unit")? {
            entry.insert("unit".into(), Json::String(unit));
        }
        parsed.push(entry);
    }
    let len = parsed
        .iter()
        .filter_map(|c| c.get("values").and_then(Json::as_array).map(Vec::len))
        .max()
        .unwrap_or_default();
    let columns: Vec<Json> = parsed
        .into_iter()
        .map(|mut c| {
            if let Some(Json::Array(values)) = c.get_mut("values") {
                values.resize(len, Json::Null);
            }
            Json::Object(c)
        })
        .collect();
    Ok(json!({ "columns": columns }))
}

/// Runs `kind` (the request's tag) on the arguments in `args`.
fn run(lua: &Lua, origin: &Origin, kind: &str, args: &Table) -> mlua::Result<Value> {
    let ctx = context(lua)?;
    let services = ctx
        .compute()
        .ok_or_else(|| error(kind, "the compute engine is not available"))?;
    // Every field but `data` converts as is; `data` keeps its nil holes.
    let rest = lua.create_table()?;
    for pair in args.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let is_data = matches!(&key, Value::String(s) if s.to_str().is_ok_and(|s| &*s == "data"));
        if !is_data {
            rest.raw_set(key, value)?;
        }
    }
    let mut request = match convert::to_json(lua, Value::Table(rest))? {
        Json::Object(map) => map,
        // An empty table converts as an empty list.
        _ => Map::new(),
    };
    let delimiter: Option<char> = match request.remove("delimiter") {
        Some(Json::String(d)) if d.chars().count() == 1 => d.chars().next(),
        None | Some(Json::Null) => None,
        Some(other) => {
            return Err(error(
                kind,
                format!("`delimiter` must be one character, got {other}"),
            ));
        }
    };
    let reads_data = !matches!(
        kind,
        "calc"
            | "convert"
            | "power"
            | "ab_test"
            | "optimize"
            | "solve"
            | "simulate"
            | "cash_flow"
            | "decide"
            | "bode"
            | "karnaugh"
            | "beam"
            | "tolerance"
    );
    let data = match args.get::<Value>("data")? {
        _ if !reads_data => Json::Null,
        Value::String(path) => {
            let path = path.to_str()?.to_owned();
            let loaded = ctx
                .block_on(load_file(services.fs.as_ref(), &path, delimiter))?
                .map_err(|e| error(kind, e))?;
            serde_json::to_value(loaded).map_err(|e| error(kind, e))?
        }
        Value::Table(data) => inline_data(kind, &data)?,
        Value::Nil => json!({ "columns": [] }),
        other => {
            return Err(error(
                kind,
                format!(
                    "`data` must be a file path or a table of columns, got {}",
                    other.type_name()
                ),
            ));
        }
    };
    if reads_data {
        request.insert("data".into(), data);
    }
    request.insert("kind".into(), Json::String(kind.to_owned()));
    let request: ComputeRequest =
        serde_json::from_value(Json::Object(request)).map_err(|e| error(kind, e))?;
    let (mut computed, body) = services.engine.run(&request).map_err(|e| error(kind, e))?;
    set_provenance(
        &mut computed,
        &format!("plugin:{}", origin.plugin),
        REQUESTED_BY,
    );
    if let Some(delimiter) = delimiter {
        computed
            .method
            .params
            .insert(DELIMITER_PARAM.into(), Json::String(delimiter.to_string()));
    }
    let block = BlockKind::Computed {
        envelope: Box::new(computed),
        body: Box::new(body),
    };
    let json = serde_json::to_value(&block).map_err(mlua::Error::external)?;
    convert::to_lua(lua, &json!({ BLOCK_KEY: json }))
}

/// Builds the `app.compute` table of one phase.
pub(crate) fn table(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let compute = lua.create_table()?;
    for kind in [
        "describe",
        "test",
        "regress",
        "correlate",
        "calc",
        "convert",
        "distribution",
        "contingency",
        "power",
        "ab_test",
        "bootstrap",
        "forecast",
        "optimize",
        "solve",
        "simulate",
        "cash_flow",
        "decide",
        "bode",
        "karnaugh",
        "beam",
        "tolerance",
        "bom",
    ] {
        let origin = Arc::clone(origin);
        compute.set(
            kind,
            lua.create_function(move |lua, args: Value| {
                let Value::Table(args) = args else {
                    return Err(error(kind, "expected a table of arguments"));
                };
                run(lua, &origin, kind, &args)
            })?,
        )?;
    }
    Ok(compute)
}

#[cfg(test)]
mod tests {
    use application::ports::compute::{ColumnValues, ComputeError};
    use application::ports::plugin::PluginOutput;
    use application::testing::compute::FakeCompute;
    use application::testing::fs::{InMemoryFs, test_root};

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::caps::Context;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run_code(code: &str, engine: Arc<FakeCompute>) -> mlua::Result<PluginOutput> {
        let lua = sandbox::new_lua(None)?;
        callbacks::install(&lua, CallbackIds::default())?;
        let fs = InMemoryFs::new(test_root())
            .with_file("cars.tsv", "mpg\tam\n21\tm\n22.8\ta\n")
            .with_file(".env", "a\n1\n");
        let ctx = Context::new(None, false, Some(Vec::new()));
        ctx.provide_compute(ComputeServices {
            engine,
            fs: Arc::new(fs),
        });
        lua.set_app_data(Arc::new(ctx));
        let phase = Phase::new(&lua, "init.lua", "stats")?;
        let value: Value = lua
            .load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .eval()?;
        Ok(crate::api::ui::output(convert::to_json(&lua, value)?))
    }

    fn envelope(out: PluginOutput) -> Box<domain::computed::Computed> {
        match out {
            PluginOutput::Block(BlockKind::Computed { envelope, .. }) => envelope,
            other => panic!("not a computed block: {other:?}"),
        }
    }

    #[test]
    fn a_file_path_reads_the_workspace_and_returns_a_card() {
        let engine = Arc::new(FakeCompute::new());
        let out = run_code(
            "return app.compute.test{ data = 'cars.tsv', y = 'mpg', group = 'am' }",
            Arc::clone(&engine),
        )
        .unwrap();
        let envelope = envelope(out);
        assert_eq!(envelope.provenance.agent.as_deref(), Some("plugin:stats"));
        assert_eq!(envelope.provenance.requested_by.as_deref(), Some("plugin"));
        let ComputeRequest::Test(request) = &engine.requests()[0] else {
            panic!("not a test request");
        };
        assert_eq!(request.y.as_deref(), Some("mpg"));
        assert_eq!(request.data.source.path.as_deref(), Some("cars.tsv"));
    }

    #[test]
    fn inline_values_keep_their_nil_holes() {
        let engine = Arc::new(FakeCompute::new());
        run_code(
            "return app.compute.describe{ data = { columns = {
               { name = 'x', values = { 1.5, nil, 3 } },
               { name = 'g', values = { 'a', 'b', nil, 'c' } },
             } }, variables = { 'x' } }",
            Arc::clone(&engine),
        )
        .unwrap();
        let request = engine.requests()[0].clone();
        let data = request.data().unwrap_or_else(|| panic!("no data"));
        assert_eq!(
            data.column("x").map(|c| &c.values),
            Some(&ColumnValues::Numeric(vec![
                Some(1.5),
                None,
                Some(3.0),
                None
            ]))
        );
        assert_eq!(
            data.column("g").map(|c| &c.values),
            Some(&ColumnValues::Categorical(vec![
                Some("a".into()),
                Some("b".into()),
                None,
                Some("c".into())
            ]))
        );
    }

    #[test]
    fn calc_and_convert_take_their_requests_as_tables() {
        let engine = Arc::new(FakeCompute::new());
        let out = run_code(
            "return app.compute.calc{ lines = { 'v = 100 km/h', 't_1 = 2 h' } }",
            Arc::clone(&engine),
        )
        .unwrap();
        assert_eq!(envelope(out).kind, "calc.sheet");
        let out = run_code(
            "return app.compute.convert{ value = 5, from = 'in', to = 'cm' }",
            Arc::clone(&engine),
        )
        .unwrap();
        assert_eq!(envelope(out).kind, "calc.convert");
        let requests = engine.requests();
        let ComputeRequest::Calc(calc) = &requests[0] else {
            panic!("not a calc request");
        };
        assert_eq!(calc.lines, ["v = 100 km/h", "t_1 = 2 h"]);
        let ComputeRequest::Convert(convert) = &requests[1] else {
            panic!("not a convert request");
        };
        assert_eq!(convert.value, Some(5.0));
        assert_eq!(convert.to, "cm");
    }

    #[test]
    fn the_card_requests_take_their_tables() {
        let engine = Arc::new(FakeCompute::new());
        let out = run_code(
            "return app.compute.power{ test = 'two_sample_t', effect = 0.8, power = 0.8 }",
            Arc::clone(&engine),
        )
        .unwrap();
        assert_eq!(envelope(out).kind, "stats.power");
        run_code(
            "return app.compute.optimize{ objective = '3*x + 5*y',
               variables = { { name = 'x' }, { name = 'y', upper = 6 } },
               constraints = { { name = 'plant', expr = '3*x + 2*y <= 18' } } }",
            Arc::clone(&engine),
        )
        .unwrap();
        run_code(
            "return app.compute.distribution{ data = 'cars.tsv', variable = 'mpg' }",
            Arc::clone(&engine),
        )
        .unwrap();
        let requests = engine.requests();
        let ComputeRequest::Power(power) = &requests[0] else {
            panic!("not a power request");
        };
        assert_eq!(power.effect, Some(0.8));
        let ComputeRequest::Optimize(lp) = &requests[1] else {
            panic!("not an optimisation");
        };
        assert_eq!(lp.variables[1].upper, Some(6.0));
        assert_eq!(lp.variables[0].lower, Some(0.0));
        let ComputeRequest::Distribution(d) = &requests[2] else {
            panic!("not a distribution");
        };
        assert_eq!(d.data.source.path.as_deref(), Some("cars.tsv"));
    }

    #[test]
    fn the_engineering_card_requests_take_their_tables() {
        let engine = Arc::new(FakeCompute::new());
        let out = run_code(
            "return app.compute.bode{ numerator = { 1 }, denominator = { 1, 1, 0 }, delay = 0.1 }",
            Arc::clone(&engine),
        )
        .unwrap();
        assert_eq!(envelope(out).kind, "solve.bode");
        let requests = engine.requests();
        let ComputeRequest::Bode(bode) = &requests[0] else {
            panic!("not a Bode request");
        };
        assert_eq!(bode.denominator, [1.0, 1.0, 0.0]);
        assert_eq!(bode.points, 400);
        run_code(
            "return app.compute.karnaugh{ expression = \"AB' + C\", dont_cares = { 0 } }",
            Arc::clone(&engine),
        )
        .unwrap();
        let ComputeRequest::Karnaugh(k) = &engine.requests()[1] else {
            panic!("not a Karnaugh request");
        };
        assert_eq!(k.dont_cares, [0]);
        run_code(
            "return app.compute.beam{ length = 6, supports = { { at = 0 }, { at = 6, kind = 'roller' } },
               loads = { { kind = 'distributed', at = 0, value = 10 } }, force_unit = 'kN' }",
            Arc::clone(&engine),
        )
        .unwrap();
        let ComputeRequest::Beam(b) = &engine.requests()[2] else {
            panic!("not a beam request");
        };
        assert_eq!(b.supports[1].kind, domain::computed::SupportKind::Roller);
        assert_eq!(b.segments, 200);
        run_code(
            "return app.compute.tolerance{ dimensions = { { name = 'a', nominal = 10, tolerance = 0.1 },
               { name = 'b', nominal = 9.8, tolerance = 0.05, direction = 'subtract' } }, lower_limit = 0 }",
            Arc::clone(&engine),
        )
        .unwrap();
        let ComputeRequest::Tolerance(t) = &engine.requests()[3] else {
            panic!("not a tolerance request");
        };
        assert_eq!(t.samples, 100_000);
        assert_eq!(
            t.dimensions[1].direction,
            domain::computed::StackDirection::Subtract
        );
        run_code(
            "return app.compute.bom{ items = { { part = 'case', quantity = 1, unit_cost = 2.2 } }, builds = 10 }",
            Arc::clone(&engine),
        )
        .unwrap();
        let ComputeRequest::Bom(b) = &engine.requests()[4] else {
            panic!("not a BOM request");
        };
        assert_eq!((b.items.len(), b.builds), (1, 10.0));
    }

    #[test]
    fn bad_calls_raise_lua_errors() {
        let engine = || Arc::new(FakeCompute::new());
        let err = |code: &str| run_code(code, engine()).unwrap_err().to_string();
        assert!(
            err("return app.compute.describe{ data = '.env' }").contains("app.compute.describe")
        );
        assert!(err("return app.compute.describe{ data = '../x.csv' }").contains("data path"));
        assert!(err("return app.compute.test(3)").contains("expected a table"));
        assert!(
            err("return app.compute.regress{ data = 'cars.tsv' }").contains("app.compute.regress")
        );
        let failing = Arc::new(FakeCompute::new().failing(ComputeError::Invalid("no y".into())));
        let message = run_code("return app.compute.correlate{ data = 'cars.tsv' }", failing)
            .unwrap_err()
            .to_string();
        assert!(message.contains("no y"), "{message}");
    }

    #[test]
    fn without_an_engine_the_call_says_so() {
        let lua = sandbox::new_lua(None).unwrap();
        callbacks::install(&lua, CallbackIds::default()).unwrap();
        lua.set_app_data(Arc::new(Context::new(None, false, Some(Vec::new()))));
        let phase = Phase::new(&lua, "init.lua", "stats").unwrap();
        let err = lua
            .load("return app.compute.describe{ data = 'x.csv' }")
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .eval::<Value>()
            .unwrap_err();
        assert!(err.to_string().contains("not available"), "{err}");
    }
}
