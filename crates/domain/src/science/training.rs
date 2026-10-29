//! Machine-learning training curves (`show_training_curves`).
//!
//! The adapter reads one or more runs (CSV or JSON lines logs) into
//! series per metric, with the step, epoch and time of every logged row,
//! smooths each series with TensorBoard's debiased exponential moving
//! average and summarises it (count, minimum, maximum, last value and the
//! best by the metric's goal, each with where it happened) in a
//! [`Computed`] envelope. The UI plots the chosen metric for every run on
//! the plot painter: the smoothed line, the raw one faint under it, the
//! best values marked, on a linear or logarithmic y axis.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;
use crate::computed::Computed;
use crate::plot::{AxisScale, PlotSpec, SeriesLook, SeriesStyle, XySeries};

/// Most values (all series together) kept.
pub const MAX_VALUES: usize = 2_000_000;
/// Most runs.
pub const MAX_RUNS: usize = 50;
/// Points per series drawn (every n-th point beyond; the summaries use
/// all of them).
pub const DRAW_POINTS: usize = 4_000;

/// What the x axis counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrainingAxis {
    /// Optimizer steps (or the row number when the log has none).
    #[default]
    Step,
    /// Epochs.
    Epoch,
    /// Seconds since the run's first logged row.
    Time,
}

impl TrainingAxis {
    /// Every axis.
    pub const ALL: [Self; 3] = [Self::Step, Self::Epoch, Self::Time];

    /// Its name (`step`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Epoch => "epoch",
            Self::Time => "time (s)",
        }
    }
}

/// Whether lower or higher is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// Lower is better (losses, errors).
    Minimize,
    /// Higher is better (accuracy, F1, AUC).
    Maximize,
}

impl Goal {
    /// The goal a metric's name suggests: higher for accuracy-like names,
    /// lower otherwise (losses, errors and anything unknown).
    #[must_use]
    pub fn guess(metric: &str) -> Self {
        let m = metric.to_ascii_lowercase();
        let lower = [
            "loss",
            "error",
            "err",
            "perplexity",
            "ppl",
            "mse",
            "mae",
            "rmse",
            "wer",
            "cer",
        ];
        if lower.iter().any(|w| m.contains(w)) {
            return Self::Minimize;
        }
        let higher = [
            "acc",
            "f1",
            "auc",
            "precision",
            "recall",
            "bleu",
            "rouge",
            "map",
            "iou",
            "dice",
            "r2",
            "score",
            "reward",
            "return",
        ];
        if higher.iter().any(|w| m.contains(w)) {
            Self::Maximize
        } else {
            Self::Minimize
        }
    }
}

/// The x values of a run's rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunAxes {
    /// Step per row.
    pub step: Vec<f64>,
    /// Epoch per row, when logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<Vec<f64>>,
    /// Seconds since the first row, when a time was logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<Vec<f64>>,
}

impl RunAxes {
    /// The values of `axis`, when the run has it.
    #[must_use]
    pub fn get(&self, axis: TrainingAxis) -> Option<&[f64]> {
        match axis {
            TrainingAxis::Step => Some(&self.step),
            TrainingAxis::Epoch => self.epoch.as_deref(),
            TrainingAxis::Time => self.time.as_deref(),
        }
    }
}

/// One metric of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricSeries {
    /// The metric's name.
    pub metric: String,
    /// Rows of the run where it was logged.
    pub rows: Vec<usize>,
    /// The values logged, one per row.
    pub values: Vec<f64>,
    /// The engine's EMA of the values.
    pub smoothed: Vec<f64>,
}

/// One run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainingRun {
    /// Its name (the file, or the `run` column).
    pub name: String,
    /// The x values of its rows.
    pub axes: RunAxes,
    /// Its metrics.
    pub series: Vec<MetricSeries>,
}

impl TrainingRun {
    /// The series of `metric`.
    #[must_use]
    pub fn series(&self, metric: &str) -> Option<&MetricSeries> {
        self.series.iter().find(|s| s.metric == metric)
    }
}

/// A value with the row it was logged at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AtRow {
    /// The value.
    pub value: f64,
    /// Its row in the run.
    pub row: usize,
}

/// The engine's summary of one series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SeriesSummary {
    /// Run name.
    pub run: String,
    /// Metric name.
    pub metric: String,
    /// Values logged.
    pub n: usize,
    /// Lowest value.
    pub min: AtRow,
    /// Highest value.
    pub max: AtRow,
    /// Last value.
    pub last: AtRow,
    /// Lower or higher is better.
    pub goal: Goal,
    /// Whether the goal was given rather than guessed from the name.
    #[serde(default)]
    pub goal_given: bool,
}

impl SeriesSummary {
    /// The best value: the minimum or the maximum by the goal.
    #[must_use]
    pub fn best(&self) -> AtRow {
        match self.goal {
            Goal::Minimize => self.min,
            Goal::Maximize => self.max,
        }
    }
}

/// Training runs with their curves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainingSpec {
    /// Title shown above the plot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace files read.
    pub paths: Vec<String>,
    /// The runs.
    pub runs: Vec<TrainingRun>,
    /// Metric names in first-seen order.
    pub metrics: Vec<String>,
    /// The metric shown at first.
    pub metric: String,
    /// The x axis at first.
    #[serde(default)]
    pub axis: TrainingAxis,
    /// EMA weight from 0 (none) to below 1.
    pub smoothing: f64,
    /// Whether the y axis starts logarithmic.
    #[serde(default)]
    pub log_y: bool,
    /// The summaries, run by run, metric by metric.
    pub summaries: Vec<SeriesSummary>,
    /// The summaries as an envelope.
    pub stats: Computed,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "training_curves",
        reason: reason.into(),
    }
}

/// TensorBoard's smoothing: an exponential moving average with weight
/// `weight` (0 ≤ weight < 1), debiased so the first values are not pulled
/// towards zero.
#[must_use]
pub fn ema(values: &[f64], weight: f64) -> Vec<f64> {
    let mut last = 0.0;
    let mut steps = 0;
    values
        .iter()
        .map(|&v| {
            last = last * weight + (1.0 - weight) * v;
            steps += 1;
            let debias = 1.0 - weight.powi(steps);
            if debias > 0.0 { last / debias } else { v }
        })
        .collect()
}

/// `v` with up to 5 significant digits.
#[must_use]
pub fn value_text(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return v.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e12 {
        return format!("{v:.0}");
    }
    let digits = (4 - v.abs().log10().floor() as i32).clamp(0, 12) as usize;
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

impl TrainingSpec {
    /// Checks sizes, lengths and that every value is finite.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.runs.is_empty() {
            return Err(invalid("there are no runs"));
        }
        if self.runs.len() > MAX_RUNS {
            return Err(invalid(format!(
                "{} runs; at most {MAX_RUNS}",
                self.runs.len()
            )));
        }
        if !(0.0..1.0).contains(&self.smoothing) {
            return Err(invalid("smoothing is from 0 to below 1"));
        }
        let mut total = 0;
        for run in &self.runs {
            let rows = run.axes.step.len();
            let other = [run.axes.epoch.as_ref(), run.axes.time.as_ref()];
            if other.iter().flatten().any(|a| a.len() != rows) {
                return Err(invalid(format!(
                    "run {:?} has axes of different lengths",
                    run.name
                )));
            }
            for s in &run.series {
                total += s.values.len();
                if s.rows.len() != s.values.len() || s.smoothed.len() != s.values.len() {
                    return Err(invalid(format!(
                        "series {}/{} has mismatched lengths",
                        run.name, s.metric
                    )));
                }
                if s.rows.iter().any(|r| *r >= rows) {
                    return Err(invalid(format!(
                        "series {}/{} points past its run",
                        run.name, s.metric
                    )));
                }
                if s.values.iter().chain(&s.smoothed).any(|v| !v.is_finite()) {
                    return Err(invalid(format!(
                        "series {}/{} has a non-finite value",
                        run.name, s.metric
                    )));
                }
            }
        }
        if total > MAX_VALUES {
            return Err(invalid(format!("{total} values; at most {MAX_VALUES}")));
        }
        self.stats
            .validate()
            .map_err(|e| invalid(format!("summary envelope: {e}")))
    }

    /// The title, the single file's name or `Training curves`.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        match self.paths.as_slice() {
            [one] => one.rsplit('/').next().unwrap_or(one).to_owned(),
            _ => "Training curves".to_owned(),
        }
    }

    /// The axes every run has (step always).
    #[must_use]
    pub fn axes(&self) -> Vec<TrainingAxis> {
        TrainingAxis::ALL
            .into_iter()
            .filter(|a| self.runs.iter().all(|r| r.axes.get(*a).is_some()))
            .collect()
    }

    /// The summary of `run`'s `metric`.
    #[must_use]
    pub fn summary(&self, run: &str, metric: &str) -> Option<&SeriesSummary> {
        self.summaries
            .iter()
            .find(|s| s.run == run && s.metric == metric)
    }

    /// The x of `run`'s row `row` on `axis` (the step when it has none).
    #[must_use]
    pub fn x_at(&self, run: &str, row: usize, axis: TrainingAxis) -> Option<f64> {
        let r = self.runs.iter().find(|r| r.name == run)?;
        r.axes
            .get(axis)
            .or_else(|| r.axes.get(TrainingAxis::Step))
            .and_then(|xs| xs.get(row))
            .copied()
    }

    /// The plot of `metric` on `axis`: per run the smoothed line in the
    /// run's colour over the raw values faint (or the raw line alone
    /// without smoothing), and the best value as a point.
    #[must_use]
    pub fn plot(&self, metric: &str, axis: TrainingAxis, log_y: bool, raw: bool) -> PlotSpec {
        let mut series = Vec::new();
        let mut looks = Vec::new();
        for (slot, run) in self.runs.iter().enumerate() {
            let Some(s) = run.series(metric) else {
                continue;
            };
            let Some(xs) = run
                .axes
                .get(axis)
                .or_else(|| run.axes.get(TrainingAxis::Step))
            else {
                continue;
            };
            let keep = |v: f64| !log_y || v > 0.0;
            let stride = s.values.len().div_ceil(DRAW_POINTS).max(1);
            let points = |ys: &[f64]| -> (Vec<f64>, Vec<f64>) {
                s.rows
                    .iter()
                    .zip(ys)
                    .enumerate()
                    .filter(|(i, _)| i % stride == 0 || *i + 1 == ys.len())
                    .filter(|(_, (_, y))| keep(**y))
                    .map(|(_, (r, y))| (xs[*r], *y))
                    .unzip()
            };
            let smoothing = self.smoothing > 0.0;
            if raw || !smoothing {
                let (x, y) = points(&s.values);
                series.push(XySeries {
                    name: if smoothing {
                        format!("{} (raw)", run.name)
                    } else {
                        run.name.clone()
                    },
                    x,
                    y,
                    style: SeriesStyle::Line,
                });
                looks.push(SeriesLook {
                    color: Some(slot),
                    faint: smoothing,
                });
            }
            if smoothing {
                let (x, y) = points(&s.smoothed);
                series.push(XySeries {
                    name: run.name.clone(),
                    x,
                    y,
                    style: SeriesStyle::Line,
                });
                looks.push(SeriesLook {
                    color: Some(slot),
                    faint: false,
                });
            }
            if let Some(best) = self.summary(&run.name, metric).map(SeriesSummary::best)
                && keep(best.value)
                && let Some(x) = xs.get(best.row)
            {
                series.push(XySeries {
                    name: format!("{} best", run.name),
                    x: vec![*x],
                    y: vec![best.value],
                    style: SeriesStyle::Scatter,
                });
                looks.push(SeriesLook {
                    color: Some(slot),
                    faint: false,
                });
            }
        }
        PlotSpec {
            title: None,
            x_label: Some(axis.name().to_owned()),
            y_label: Some(metric.to_owned()),
            series,
            y_scale: if log_y {
                AxisScale::Log
            } else {
                AxisScale::Linear
            },
            looks,
            ..PlotSpec::default()
        }
    }

    /// The summaries as CSV: run, metric, n, min, max, last, best and
    /// their steps.
    #[must_use]
    pub fn summary_csv(&self) -> String {
        let mut out = String::from(
            "run,metric,n,min,min_step,max,max_step,last,last_step,goal,best,best_step\n",
        );
        for s in &self.summaries {
            let step = |row: usize| {
                self.x_at(&s.run, row, TrainingAxis::Step)
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            };
            let best = s.best();
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{},{}",
                csv_field(&s.run),
                csv_field(&s.metric),
                s.n,
                s.min.value,
                step(s.min.row),
                s.max.value,
                step(s.max.row),
                s.last.value,
                step(s.last.row),
                match s.goal {
                    Goal::Minimize => "min",
                    Goal::Maximize => "max",
                },
                best.value,
                step(best.row)
            );
        }
        out
    }
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn spec() -> TrainingSpec {
        let values = vec![1.0, 0.5, 0.25, 0.4];
        TrainingSpec {
            title: None,
            paths: vec!["runs/a.csv".into()],
            runs: vec![TrainingRun {
                name: "a".into(),
                axes: RunAxes {
                    step: vec![10.0, 20.0, 30.0, 40.0],
                    epoch: Some(vec![1.0, 1.0, 2.0, 2.0]),
                    time: None,
                },
                series: vec![MetricSeries {
                    metric: "loss".into(),
                    rows: vec![0, 1, 2, 3],
                    smoothed: ema(&values, 0.5),
                    values,
                }],
            }],
            metrics: vec!["loss".into()],
            metric: "loss".into(),
            axis: TrainingAxis::Step,
            smoothing: 0.5,
            log_y: false,
            summaries: vec![SeriesSummary {
                run: "a".into(),
                metric: "loss".into(),
                n: 4,
                min: AtRow {
                    value: 0.25,
                    row: 2,
                },
                max: AtRow { value: 1.0, row: 0 },
                last: AtRow { value: 0.4, row: 3 },
                goal: Goal::Minimize,
                goal_given: false,
            }],
            stats: Computed::new(
                "ml_1",
                "ml.training_curves",
                "s",
                Engine {
                    name: "crest-curves".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn ema_is_debiased() {
        let s = ema(&[1.0, 1.0, 1.0], 0.9);
        assert!(s.iter().all(|v| (v - 1.0).abs() < 1e-12), "{s:?}");
        let s = ema(&[0.0, 1.0], 0.5);
        // last = 0.5 after two steps; debias 1 - 0.25 = 0.75.
        assert!((s[1] - 0.5 / 0.75).abs() < 1e-12);
        assert_eq!(ema(&[3.0, 4.0], 0.0), vec![3.0, 4.0]);
    }

    #[test]
    fn goals_plot_and_csv() {
        assert_eq!(Goal::guess("val_loss"), Goal::Minimize);
        assert_eq!(Goal::guess("val_accuracy"), Goal::Maximize);
        assert_eq!(Goal::guess("lr"), Goal::Minimize);
        let s = spec();
        s.validate().unwrap();
        assert_eq!(s.axes(), vec![TrainingAxis::Step, TrainingAxis::Epoch]);
        let plot = s.plot("loss", TrainingAxis::Step, false, true);
        plot.validate().unwrap();
        assert_eq!(plot.series.len(), 3, "raw, smoothed, best");
        assert!(plot.looks[0].faint && !plot.looks[1].faint);
        assert_eq!(plot.series[2].x, vec![30.0]);
        let on_epochs = s.plot("loss", TrainingAxis::Epoch, true, false);
        assert_eq!(on_epochs.series.len(), 2, "smoothed and best");
        assert_eq!(on_epochs.y_scale, AxisScale::Log);
        assert_eq!(
            s.summary_csv(),
            "run,metric,n,min,min_step,max,max_step,last,last_step,goal,best,best_step\n\
             a,loss,4,0.25,30,1,10,0.4,40,min,0.25,30\n"
        );
        assert_eq!(value_text(0.123_456_7), "0.12346");
        assert_eq!(value_text(1500.0), "1500");
    }
}
