//! What the implementation flow keeps in its graph-flow session: the steps
//! of the graph, the [`ImplementState`] (plan, order, progress, checks and
//! journal), the flow status and the pending gate.
//!
//! Every key the frame owns starts with [`RESERVED_PREFIX`].

use std::collections::BTreeMap;

use domain::plan::{Plan, Task, TaskId};
use domain::research::{ConstraintId, TestKind};
use serde::{Deserialize, Serialize};

/// Prefix of the context keys owned by the implementation frame.
pub const RESERVED_PREFIX: &str = "implement.";

/// Prefix of the graph-flow task ids of the steps.
const TASK_PREFIX: &str = "implement/";

/// A step of the implementation flow, in graph order.
///
/// `Implement`, `Test` and `Commit` repeat once per task of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Validates `plan.json`, orders the tasks and creates the branch.
    Load,
    /// The agent implements the current task, restricted to its files.
    Implement,
    /// Runs the acceptance checks of the task; asks for fixes.
    Test,
    /// Commits the task with its id in the message.
    Commit,
    /// Runs the hypothesis and constraint tests.
    Verify,
    /// Writes `report.md` in the Spec folder.
    Report,
}

impl Step {
    /// Every step, in graph order.
    pub const ALL: [Self; 6] = [
        Self::Load,
        Self::Implement,
        Self::Test,
        Self::Commit,
        Self::Verify,
        Self::Report,
    ];

    /// Spanish name shown in the timeline.
    #[must_use]
    pub const fn name_es(self) -> &'static str {
        match self {
            Self::Load => "Cargar plan",
            Self::Implement => "Implementar tarea",
            Self::Test => "Pruebas y corrección",
            Self::Commit => "Commit",
            Self::Verify => "Verificar hipótesis",
            Self::Report => "Reporte y futuro",
        }
    }

    /// 1-based position among the steps.
    #[must_use]
    pub fn index(self) -> u32 {
        Self::ALL
            .iter()
            .position(|s| *s == self)
            .map_or(0, |i| u32::try_from(i + 1).unwrap_or(u32::MAX))
    }

    /// Number of steps (6).
    #[must_use]
    pub const fn total() -> u32 {
        6
    }

    /// The graph-flow task id of the step, e.g. `implement/test`.
    #[must_use]
    pub fn task_id(self) -> String {
        let name = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{self:?}"));
        format!("{TASK_PREFIX}{name}")
    }

    /// The step of a task id built by [`Step::task_id`].
    #[must_use]
    pub fn of_task(id: &str) -> Option<Self> {
        let name = id.strip_prefix(TASK_PREFIX)?;
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }

    /// Whether the step repeats for every task of the plan.
    #[must_use]
    pub const fn is_per_task(self) -> bool {
        matches!(self, Self::Implement | Self::Test | Self::Commit)
    }
}

/// Where the flow is, as stored in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FlowStatus {
    /// Between steps: the next `step` call runs `next`.
    Running {
        /// Step that runs next.
        next: Step,
    },
    /// A step stopped at a gate and waits for the user.
    WaitingForInput {
        /// Step that waits.
        step: Step,
        /// What it asks.
        prompt: String,
    },
    /// The report was written and the user closed the flow.
    Completed,
    /// The user aborted the flow.
    Aborted,
    /// The plan turned out not to be feasible: the flow stopped and offers
    /// to go back to the Spec node of the research flow.
    BackToSpec {
        /// Why the plan cannot be followed.
        reason: String,
    },
}

impl FlowStatus {
    /// Whether `step` would run a node.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Whether the flow stopped for good.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Aborted | Self::BackToSpec { .. }
        )
    }
}

/// A gate waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingGate {
    /// Step that waits.
    pub step: Step,
    /// What the user is asked.
    pub prompt: String,
}

/// The user's answer to a [`PendingGate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReply {
    /// Step the answer is for.
    pub step: Step,
    /// What the user wrote.
    pub text: String,
}

/// Progress of one task of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not started.
    #[default]
    Pending,
    /// The agent finished implementing it; its checks did not run yet.
    Implemented,
    /// Its acceptance checks pass.
    Passed,
    /// Its checks kept failing and the user chose to go on anyway.
    Failed,
    /// The user skipped it.
    Skipped,
    /// Committed.
    Committed,
}

/// What the flow knows about one task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Progress.
    pub status: TaskStatus,
    /// Fix attempts made after a failing check.
    #[serde(default)]
    pub attempts: u32,
    /// Files outside the task that the user allowed it to touch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_files: Vec<String>,
    /// Output of the last failing check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    /// Hash of the task's commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// What the agent said it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// What a journal entry records; the report groups entries by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Something that happened as planned.
    Note,
    /// Something that worked out: a task done, a check passed.
    Achievement,
    /// Something that failed.
    Failure,
    /// A departure from the plan: a file outside the task, a skipped task.
    Deviation,
    /// An idea for a later Spec.
    Idea,
    /// What the user answered at a gate.
    User,
}

/// An entry of the implementation journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// When it was written, in Unix milliseconds.
    pub at: u64,
    /// Step that wrote it.
    pub step: Step,
    /// Task it is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskId>,
    /// What it records.
    pub kind: EntryKind,
    /// Markdown text.
    pub text: String,
}

/// Which kind of final check a [`CheckResult`] is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckSubject {
    /// A test of the hypothesis of the Spec.
    Hypothesis,
    /// A test of a constraint.
    Constraint {
        /// The constraint it proves.
        id: ConstraintId,
    },
}

/// The result of a hypothesis or constraint test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// What it checks.
    pub subject: CheckSubject,
    /// How it ran.
    pub kind: TestKind,
    /// The command line.
    pub command: String,
    /// What was expected, as written in the plan (or "pasa").
    pub expected: String,
    /// What was obtained, in words (a measurement or the exit status).
    pub obtained: String,
    /// The measurement parsed from the output, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<f64>,
    /// The threshold parsed from the expectation, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Unit of `measured` and `threshold`, e.g. `ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Whether the obtained result meets the expectation.
    pub passed: bool,
}

/// Everything the implementation flow knows: the plan, the order of its
/// tasks, where it is and what happened.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ImplementState {
    /// Workspace path of `plan.json`.
    pub plan_path: String,
    /// The validated plan, once loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    /// Task ids in dependency order.
    #[serde(default)]
    pub order: Vec<TaskId>,
    /// Branch the tasks are committed to, once created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Index in `order` of the task being worked on.
    #[serde(default)]
    pub current: usize,
    /// Progress of each task.
    #[serde(default)]
    pub tasks: BTreeMap<TaskId, TaskRecord>,
    /// Results of the hypothesis and constraint tests.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    /// Workspace path of `report.md`, once written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    /// The implementation journal.
    #[serde(default)]
    pub journal: Vec<JournalEntry>,
}

impl ImplementState {
    /// The state of a flow about to load `plan_path`.
    #[must_use]
    pub fn new(plan_path: impl Into<String>) -> Self {
        Self {
            plan_path: plan_path.into(),
            ..Self::default()
        }
    }

    /// Folder of the Spec: the directory holding `plan.json`.
    #[must_use]
    pub fn spec_dir(&self) -> String {
        match self.plan_path.rsplit_once('/') {
            Some((dir, _)) => dir.to_owned(),
            None => ".".to_owned(),
        }
    }

    /// The task with id `id`.
    #[must_use]
    pub fn task(&self, id: &TaskId) -> Option<&Task> {
        self.plan.as_ref()?.tasks.iter().find(|t| &t.id == id)
    }

    /// The task being worked on, or `None` once all are done.
    #[must_use]
    pub fn current_task(&self) -> Option<&Task> {
        self.task(self.order.get(self.current)?)
    }

    /// The record of `id`, created when missing.
    pub fn record_mut(&mut self, id: &TaskId) -> &mut TaskRecord {
        self.tasks.entry(id.clone()).or_default()
    }

    /// The record of `id`, if any.
    #[must_use]
    pub fn record(&self, id: &TaskId) -> Option<&TaskRecord> {
        self.tasks.get(id)
    }

    /// Moves on to the next task.
    pub fn advance(&mut self) {
        self.current += 1;
    }

    /// Appends a journal entry.
    pub fn log(
        &mut self,
        at: u64,
        step: Step,
        task: Option<TaskId>,
        kind: EntryKind,
        text: impl Into<String>,
    ) {
        self.journal.push(JournalEntry {
            at,
            step,
            task,
            kind,
            text: text.into(),
        });
    }

    /// Journal entries of `kind`, in order.
    pub fn entries(&self, kind: EntryKind) -> impl Iterator<Item = &JournalEntry> {
        self.journal.iter().filter(move |e| e.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids_round_trip_for_every_step() {
        for step in Step::ALL {
            let id = step.task_id();
            assert!(id.starts_with("implement/"), "{id}");
            assert_eq!(Step::of_task(&id), Some(step));
        }
        assert_eq!(Step::Verify.task_id(), "implement/verify");
        assert_eq!(Step::of_task("research/spec"), None);
        assert_eq!(
            Step::ALL.map(Step::index),
            [1, 2, 3, 4, 5, 6],
            "indexes follow graph order"
        );
    }

    #[test]
    fn spec_dir_is_the_folder_of_the_plan() {
        let state = ImplementState::new(".crest/specs/0007-cache/plan.json");
        assert_eq!(state.spec_dir(), ".crest/specs/0007-cache");
        assert_eq!(ImplementState::new("plan.json").spec_dir(), ".");
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut state = ImplementState::new("s/plan.json");
        state.order = vec![TaskId::new("T2"), TaskId::new("T10")];
        state.record_mut(&TaskId::new("T10")).status = TaskStatus::Committed;
        state.log(
            5,
            Step::Commit,
            Some(TaskId::new("T10")),
            EntryKind::Note,
            "ok",
        );
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["tasks"]["T10"]["status"], "committed");
        let back: ImplementState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
        assert_eq!(back.entries(EntryKind::Note).count(), 1);
    }

    #[test]
    fn status_round_trips_through_json() {
        let status = FlowStatus::BackToSpec {
            reason: "la API no existe".into(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "back_to_spec");
        assert_eq!(serde_json::from_value::<FlowStatus>(json).unwrap(), status);
        assert!(status.is_finished() && !status.is_running());
    }
}
