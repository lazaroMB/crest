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
