//! Training curves for `show_training_curves`: CSV/TSV or JSON lines
//! logs into series per run and metric.
//!
//! Columns named like a step (`step`, `global_step`, `iteration`...), an
//! epoch, a time (`time`, `wall_time`, `timestamp`...) or a run (`run`,
//! `run_id`...) are axes or the run; every other column with numbers is
//! a metric (Keras' and Lightning's CSV loggers, wandb exports). A
//! TensorBoard scalar export (`Wall time`, `Step`, `Value`) names its
//! metric after the file. Without a step column the row number is the
//! step; a missing epoch or time repeats the last one seen. Each series
//! is smoothed with TensorBoard's debiased EMA and summarised (count,
//! minimum, maximum, last and best by the goal, each with its row) into
//! an `ml.training_curves` envelope. TensorBoard event files are not read.

use std::collections::BTreeMap;

use application::csv::{delimiter_for, number, records};
use application::ports::formats::TrainingRequest;
use domain::computed::{Computed, ComputedValue};
use domain::science::molecule_grid::value_key;
use domain::science::training::{
    AtRow, Goal, MAX_RUNS, MAX_VALUES, MetricSeries, RunAxes, SeriesSummary, TrainingAxis,
    TrainingRun, TrainingSpec, ema,
};
use serde_json::Value;

use crate::{chem, sci};

/// Envelope kind of the summaries.
pub const KIND: &str = "ml.training_curves";
/// Engine name.
pub const CURVES_ENGINE_NAME: &str = "crest-curves";
/// TensorBoard's default smoothing.
pub const DEFAULT_SMOOTHING: f64 = 0.6;

const STEP: &[&str] = &[
    "step",
    "steps",
    "global_step",
    "iteration",
    "iter",
    "batch",
    "trainer/global_step",
    "_step",
];
const EPOCH: &[&str] = &["epoch", "epochs"];
const TIME: &[&str] = &[
    "time",
    "wall_time",
    "wall time",
    "timestamp",
    "_timestamp",
    "elapsed",
    "relative time",
    "_runtime",
];
const RUN: &[&str] = &["run", "run_id", "run_name", "experiment"];

/// A cell: a number or text.
#[derive(Clone)]
enum Cell {
    Num(f64),
    Text(String),
}

impl Cell {
    fn num(&self) -> Option<f64> {
        match self {
            Self::Num(v) => Some(*v).filter(|v| v.is_finite()),
            Self::Text(t) => number(t),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Num(v) => v.to_string(),
            Self::Text(t) => t.trim().to_owned(),
        }
    }
}

/// A log as column names and rows of optional cells.
struct Table {
    columns: Vec<String>,
    rows: Vec<Vec<Option<Cell>>>,
}

fn read_table(bytes: &[u8], path: &str) -> Result<Table, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8"))?;
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('{') {
        let mut columns: Vec<String> = Vec::new();
        let mut maps = Vec::new();
        for (i, line) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let value: Value = serde_json::from_str(line)
                .map_err(|e| format!("`{path}` line {}: not JSON: {e}", i + 1))?;
            let Value::Object(obj) = value else {
                return Err(format!("`{path}` line {}: not an object", i + 1));
            };
            for k in obj.keys() {
                if !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
            maps.push(obj);
        }
        let rows = maps
            .into_iter()
            .map(|obj| {
                columns
                    .iter()
                    .map(|c| match obj.get(c) {
                        Some(Value::Number(n)) => n.as_f64().map(Cell::Num),
                        Some(Value::String(s)) => Some(Cell::Text(s.clone())),
                        Some(Value::Bool(b)) => Some(Cell::Text(b.to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        return Ok(Table { columns, rows });
    }
    let mut recs = records(text, delimiter_for(path))?.into_iter();
    let columns: Vec<String> = recs
        .next()
        .ok_or_else(|| format!("`{path}` is empty"))?
        .into_iter()
        .map(|c| c.trim().to_owned())
        .collect();
    let rows = recs
        .map(|r| {
            (0..columns.len())
                .map(|i| {
                    r.get(i)
                        .filter(|c| !c.trim().is_empty())
                        .map(|c| Cell::Text(c.clone()))
                })
                .collect()
        })
        .collect();
    Ok(Table { columns, rows })
}

fn find(columns: &[String], names: &[&str]) -> Option<usize> {
    columns
        .iter()
        .position(|c| names.contains(&c.to_ascii_lowercase().as_str()))
}

fn stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned()
}

/// Runs of one log.
fn runs_of(table: &Table, path: &str, warnings: &mut Vec<String>) -> Vec<TrainingRun> {
    let step = find(&table.columns, STEP);
    let epoch = find(&table.columns, EPOCH);
    let time = find(&table.columns, TIME);
    let run = find(&table.columns, RUN);
    let axes: Vec<Option<usize>> = vec![step, epoch, time, run];
    let mut metrics: Vec<(usize, String)> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(i, _)| !axes.contains(&Some(*i)))
        .filter(|(i, _)| {
            table
                .rows
                .iter()
                .any(|r| r[*i].as_ref().and_then(Cell::num).is_some())
        })
        .map(|(i, c)| (i, c.clone()))
        .collect();
    // A TensorBoard scalar export: its one value is the file's metric.
    if let [(_, name)] = metrics.as_mut_slice()
        && name.eq_ignore_ascii_case("value")
    {
        *name = stem(path);
    }
    let skipped: Vec<&str> = table
        .columns
        .iter()
        .enumerate()
        .filter(|(i, _)| !axes.contains(&Some(*i)) && !metrics.iter().any(|(m, _)| m == i))
        .map(|(_, c)| c.as_str())
        .collect();
    if !skipped.is_empty() {
        warnings.push(format!(
            "`{path}`: columns without numbers skipped: {}",
            skipped.join(", ")
        ));
    }
    // Rows by run, in first-seen order.
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (ix, row) in table.rows.iter().enumerate() {
        let name = match run.and_then(|r| row[r].as_ref()) {
            Some(cell) => cell.text(),
            None => stem(path),
        };
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, rows)) => rows.push(ix),
            None => groups.push((name, vec![ix])),
        }
    }
    groups
        .into_iter()
        .map(|(name, rows)| {
            let column = |col: usize, relative: bool| -> Vec<f64> {
                let mut last = None;
                let mut out: Vec<f64> = rows
                    .iter()
                    .map(|r| {
                        let v = table.rows[*r][col].as_ref().and_then(Cell::num).or(last);
                        last = v;
                        v.unwrap_or(f64::NAN)
                    })
                    .collect();
                // Rows before the first value take it.
                let first = out.iter().copied().find(|v| v.is_finite()).unwrap_or(0.0);
                out.iter_mut()
                    .filter(|v| !v.is_finite())
                    .for_each(|v| *v = first);
                if relative {
                    out.iter_mut().for_each(|v| *v -= first);
                }
                out
            };
            #[allow(clippy::cast_precision_loss)]
            let step_values = match step {
                Some(c) => column(c, false),
                None => (0..rows.len()).map(|i| i as f64).collect(),
            };
            let axes = RunAxes {
                step: step_values,
                epoch: epoch.map(|c| column(c, false)),
                time: time.map(|c| column(c, true)),
            };
            let series = metrics
                .iter()
                .filter_map(|(col, metric)| {
                    let (idx, values): (Vec<usize>, Vec<f64>) = rows
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| {
                            table.rows[*r][*col]
                                .as_ref()
                                .and_then(Cell::num)
                                .map(|v| (i, v))
                        })
                        .unzip();
                    (!values.is_empty()).then(|| MetricSeries {
                        metric: metric.clone(),
                        rows: idx,
                        smoothed: Vec::new(),
                        values,
                    })
                })
                .collect();
            TrainingRun { name, axes, series }
        })
        .collect()
}

fn summarise(run: &str, s: &MetricSeries, goal: Option<Goal>) -> SeriesSummary {
    let mut min = AtRow {
        value: f64::INFINITY,
        row: 0,
    };
    let mut max = AtRow {
        value: f64::NEG_INFINITY,
        row: 0,
    };
    for (r, v) in s.rows.iter().zip(&s.values) {
        // The first occurrence of the extreme wins.
        if *v < min.value {
            min = AtRow { value: *v, row: *r };
        }
        if *v > max.value {
            max = AtRow { value: *v, row: *r };
        }
    }
    let last = AtRow {
        value: s.values.last().copied().unwrap_or(0.0),
        row: s.rows.last().copied().unwrap_or(0),
    };
    SeriesSummary {
        run: run.to_owned(),
        metric: s.metric.clone(),
        n: s.values.len(),
        min,
        max,
        last,
        goal: goal.unwrap_or_else(|| Goal::guess(&s.metric)),
        goal_given: goal.is_some(),
    }
}

/// The training curves of `request`.
///
/// # Errors
///
/// A message for unreadable logs, no metric or an unknown `metric`.
pub fn curves(request: &TrainingRequest) -> Result<TrainingSpec, String> {
    if request.files.is_empty() {
        return Err("no log files".into());
    }
    let smoothing = request.smoothing.unwrap_or(DEFAULT_SMOOTHING);
    if !(0.0..1.0).contains(&smoothing) {
        return Err("`smoothing` is from 0 to below 1".into());
    }
    let mut warnings = Vec::new();
    let mut runs: Vec<TrainingRun> = Vec::new();
    for f in &request.files {
        let table = read_table(&f.bytes, &f.path)?;
        for mut run in runs_of(&table, &f.path, &mut warnings) {
            if runs.iter().any(|r| r.name == run.name) {
                let n = runs.len() + 1;
                run.name = format!("{} ({n})", run.name);
            }
            runs.push(run);
        }
    }
    runs.retain(|r| !r.series.is_empty());
    if runs.is_empty() {
        return Err("the logs have no numeric metric columns".into());
    }
    if runs.len() > MAX_RUNS {
        return Err(format!("{} runs; at most {MAX_RUNS}", runs.len()));
    }
    let total: usize = runs
        .iter()
        .flat_map(|r| &r.series)
        .map(|s| s.values.len())
        .sum();
    if total > MAX_VALUES {
        return Err(format!("{total} values; at most {MAX_VALUES}"));
    }
    let mut metrics: Vec<String> = Vec::new();
    for s in runs.iter().flat_map(|r| &r.series) {
        if !metrics.contains(&s.metric) {
            metrics.push(s.metric.clone());
        }
    }
    let metric = match request
        .metric
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(m) => metrics
            .iter()
            .find(|x| x.as_str() == m)
            .cloned()
            .ok_or_else(|| format!("no metric {m:?}; the logs have {}", metrics.join(", ")))?,
        None => metrics
            .iter()
            .find(|m| m.to_ascii_lowercase().contains("loss"))
            .unwrap_or(&metrics[0])
            .clone(),
    };
    let mut summaries = Vec::new();
    for run in &mut runs {
        for s in &mut run.series {
            s.smoothed = ema(&s.values, smoothing);
            summaries.push(summarise(&run.name, s, request.goal));
        }
    }
    let mut axis = request.axis.unwrap_or_default();
    if runs.iter().any(|r| r.axes.get(axis).is_none()) {
        warnings.push(format!(
            "not every run logs {}; the x axis is the step",
            axis.name()
        ));
        axis = TrainingAxis::Step;
    }
    let mut spec = TrainingSpec {
        title: None,
        paths: request.files.iter().map(|f| f.path.clone()).collect(),
        runs,
        metrics,
        metric,
        axis,
        smoothing,
        log_y: request.log_y,
        summaries,
        stats: Computed::new(
            "ml_pending",
            KIND,
            "Training curve summaries",
            sci::engine(CURVES_ENGINE_NAME, "crest"),
        ),
        warnings,
    };
    spec.stats = envelope(request, &spec);
    Ok(spec)
}

fn envelope(request: &TrainingRequest, spec: &TrainingSpec) -> Computed {
    let hashes: Vec<String> = request.files.iter().map(|f| chem::hash(&f.bytes)).collect();
    let joined = hashes.join(",");
    let mut computed = Computed::new(
        sci::derive_id(
            "ml",
            &[
                KIND.as_bytes(),
                joined.as_bytes(),
                &spec.smoothing.to_le_bytes(),
            ],
        ),
        KIND,
        "Training curve summaries",
        sci::engine(CURVES_ENGINE_NAME, "crest"),
    );
    computed.method.why = Some(format!(
        "Per run and metric over every logged value: count, minimum, maximum and last value \
         with their steps, best = minimum or maximum by the goal (guessed from the name unless \
         given); smoothing is TensorBoard's debiased EMA with weight {} (display only, not \
         used by the summaries)",
        spec.smoothing
    ));
    let mut params = BTreeMap::new();
    params.insert("smoothing".into(), Value::from(spec.smoothing));
    computed.method.params = params;
    computed.inputs = request
        .files
        .iter()
        .map(|f| chem::input("log", "data", &f.bytes, Some(&f.path)))
        .collect();
    let v = &mut computed.values;
    #[allow(clippy::cast_precision_loss)]
    {
        v.insert("runs".into(), ComputedValue::new(spec.runs.len() as f64));
        v.insert(
            "metrics".into(),
            ComputedValue::new(spec.metrics.len() as f64),
        );
    }
    for s in &spec.summaries {
        let key = format!("{}.{}", value_key(&s.run), value_key(&s.metric));
        let step = |row: usize| {
            spec.x_at(&s.run, row, TrainingAxis::Step)
                .unwrap_or(f64::NAN)
        };
        let best = s.best();
        #[allow(clippy::cast_precision_loss)]
        for (name, x) in [
            ("n", s.n as f64),
            ("min", s.min.value),
            ("max", s.max.value),
            ("last", s.last.value),
            ("best", best.value),
            ("best_step", step(best.row)),
            ("last_step", step(s.last.row)),
        ] {
            if x.is_finite() {
                v.insert(format!("{key}.{name}"), ComputedValue::new(x));
            }
        }
    }
    computed
}
