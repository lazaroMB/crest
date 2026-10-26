//! Requests of the science B viewers (Gantt charts, array files,
//! networks, training curves), re-exported from [`super`].

use domain::gantt::Calendar;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One task of a [`GanttRequest`], as the model or a file gives it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GanttTaskInput {
    /// Unique id, used by `depends_on` (`design`, `T3`).
    pub id: String,
    /// Name shown; the id when absent.
    #[serde(default)]
    pub name: Option<String>,
    /// Phase or work package (rows of a group share a color).
    #[serde(default)]
    pub group: Option<String>,
    /// Duration in working days; 0 (or `milestone`) for a milestone.
    /// From `start` and `finish` when absent.
    #[serde(default)]
    pub duration: Option<f64>,
    /// Earliest start as `YYYY-MM-DD` (a "start no earlier than"
    /// constraint).
    #[serde(default)]
    pub start: Option<String>,
    /// Finish as `YYYY-MM-DD`, with `start`, when there is no `duration`.
    #[serde(default)]
    pub finish: Option<String>,
    /// Predecessors: `A` (finish-to-start), `A SS`, `A FS+2`, `A FF-1`,
    /// `A SF` (lags in working days).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// A milestone (zero duration).
    #[serde(default)]
    pub milestone: bool,
}

/// Where a [`GanttRequest`]'s tasks come from.
#[derive(Debug, Clone, PartialEq)]
pub enum GanttSource {
    /// A CSV/TSV (header with `id`, `duration`, `depends_on`...) or JSON
    /// (an array of tasks, or `{"tasks": [...]}`) file's bytes.
    File(Vec<u8>),
    /// Tasks given inline.
    Tasks(Vec<GanttTaskInput>),
}

/// What [`super::FormatsPort::schedule_gantt`] schedules.
#[derive(Debug, Clone, PartialEq)]
pub struct GanttRequest {
    /// Workspace path of the file, for the block, messages and the CSV
    /// delimiter.
    pub path: Option<String>,
    /// The tasks.
    pub source: GanttSource,
    /// Project start as `YYYY-MM-DD`; the earliest task `start` when
    /// absent, else offsets only.
    pub start: Option<String>,
    /// Which days are working days.
    pub calendar: Calendar,
}

/// What [`super::FormatsPort::inspect_array_file`] reads besides the
/// bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArrayRequest {
    /// Workspace path of the file, for the block and messages.
    pub path: String,
    /// The variable to preview (`/sim/field`, or a NetCDF name); the first
    /// few numeric variables when absent.
    pub variable: Option<String>,
}

/// What [`super::FormatsPort::analyse_network`] reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkRequest {
    /// Workspace path of the file, for the block, messages and the CSV
    /// delimiter.
    pub path: Option<String>,
    /// An edge list (CSV/TSV with a header) or node-link JSON (`nodes`
    /// and `links` or `edges`, NetworkX style).
    pub bytes: Vec<u8>,
    /// Whether edges have a direction; the JSON's `directed`, else false.
    pub directed: Option<bool>,
    /// Column of the source node (`source`, `from`, `src`... or the first).
    pub source: Option<String>,
    /// Column of the target node (`target`, `to`, `dst`... or the second).
    pub target: Option<String>,
    /// Column of the weight (`weight` when present).
    pub weight: Option<String>,
    /// Metric the nodes are sized by at first.
    pub size_by: domain::science::network::NetworkMetric,
    /// Seed of the community search, sampling and layout.
    pub seed: Option<u64>,
}

/// One log of a [`TrainingRequest`]: a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrainingFile {
    /// Workspace path; its file stem names the run (unless the log has a
    /// `run` column).
    pub path: String,
    /// CSV/TSV with a header, or JSON lines.
    pub bytes: Vec<u8>,
}

/// What [`super::FormatsPort::training_curves`] reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainingRequest {
    /// The logs, one run each (or several through a `run` column).
    pub files: Vec<TrainingFile>,
    /// The metric shown first; the first loss-like one when absent.
    pub metric: Option<String>,
    /// The x axis shown first (step by default).
    pub axis: Option<domain::science::training::TrainingAxis>,
    /// EMA weight, 0 to below 1 (0.6 when absent).
    pub smoothing: Option<f64>,
    /// Start with a logarithmic y axis.
    pub log_y: bool,
    /// Whether lower or higher is better for every metric; guessed from
    /// each name when absent.
    pub goal: Option<domain::science::training::Goal>,
}
