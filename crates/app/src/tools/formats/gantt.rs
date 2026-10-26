//! `show_gantt`: a project scheduled by the critical path method on a
//! Gantt chart.
//!
//! The model gets the task and milestone counts, the project duration
//! (and finish date), the critical path, the tasks with the most slack,
//! the milestones' dates and the envelope id with its keys, never the
//! whole schedule.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::gantt::{Calendar, GanttSpec, MAX_TASKS, SLACK_EPSILON, days};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line, read};
use crate::ports::formats::{GanttRequest, GanttSource, GanttTaskInput};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Tasks with slack listed to the model.
const LISTED: usize = 8;

/// Input of `show_gantt`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowGantt {
    /// A task file of the workspace: CSV/TSV with a header (`id`, `name`,
    /// `duration`, `depends_on`, `start`, `finish`, `group`, `milestone`)
    /// or JSON (an array of tasks, or `{"tasks": [...]}`).
    #[serde(default)]
    pub path: Option<String>,
    /// The tasks inline instead of `path`.
    #[serde(default)]
    pub tasks: Option<Vec<GanttTaskInput>>,
    /// Project start as `YYYY-MM-DD`; the earliest task `start` when
    /// absent, else the chart counts days from the start.
    #[serde(default)]
    pub start: Option<String>,
    /// `all_days` (default) or `weekdays` (Monday to Friday count).
    #[serde(default)]
    pub calendar: Option<Calendar>,
    /// Title shown above the chart.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_gantt`: a CPM-scheduled Gantt chart.
#[derive(Clone)]
pub struct ShowGanttTool {
    ports: FormatToolPorts,
}

impl ShowGanttTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_gantt`.
pub const SHOW_GANTT: &str = "show_gantt";

#[async_trait]
impl TypedTool for ShowGanttTool {
    type Input = ShowGantt;

    fn name(&self) -> &str {
        SHOW_GANTT
    }

    fn description(&self) -> &str {
        "Schedules a project with the critical path method and shows it in \
         the user's timeline as a Gantt chart (critical tasks marked, slack \
         drawn after each task, milestones as diamonds) with a schedule \
         table. Tasks come from a CSV/JSON file (`path`) or `tasks`: each \
         with an id, a duration in working days (or start and finish \
         dates), and `depends_on` such as `A`, `A SS`, `A FS+2`, `A FF-1`. \
         The engine computes early/late start and finish, total and free \
         slack and the critical path, and rejects dependency cycles. Cite \
         its dates, durations and slack; do not schedule by hand."
    }

    fn effect(&self, _input: &ShowGantt) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowGantt,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input.path.filter(|p| !p.trim().is_empty());
        let tasks = input.tasks.filter(|t| !t.is_empty());
        match (&path, &tasks) {
            (Some(_), Some(_)) => {
                return Err(invalid(SHOW_GANTT, "give `path` or `tasks`, not both"));
            }
            (None, None) => return Err(invalid(SHOW_GANTT, "give `path` or `tasks`")),
            (None, Some(t)) if t.len() > MAX_TASKS => {
                return Err(invalid(SHOW_GANTT, format!("at most {MAX_TASKS} tasks")));
            }
            _ => {}
        }
        let ports = self.ports.clone();
        let subject = path.clone().unwrap_or_else(|| "inline tasks".to_owned());
        let start = input.start.filter(|s| !s.trim().is_empty());
        let calendar = input.calendar.unwrap_or_default();
        let title = input.title.filter(|t| !t.trim().is_empty());
        let run = async move {
            let source = match (&path, tasks) {
                (Some(path), _) => match read(ports.fs.as_ref(), path).await {
                    Ok(bytes) => GanttSource::File(bytes),
                    Err(message) => return Ok(ToolOutput::error(message)),
                },
                (None, tasks) => GanttSource::Tasks(tasks.unwrap_or_default()),
            };
            let request = GanttRequest {
                path: path.clone(),
                source,
                start,
                calendar,
            };
            let formats = Arc::clone(&ports.formats);
            let scheduled =
                offloaded(&ports.offload, move || formats.schedule_gantt(&request)).await;
            let name = path.as_deref().unwrap_or("the tasks");
            let mut spec = match scheduled {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(format!("{name}: {err}"))),
                None => return Ok(ToolOutput::error("the scheduler did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("{name}: {err}")));
            }
            let summary = gantt_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Gantt(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_gantt`.
#[must_use]
pub fn gantt_summary(spec: &GanttSpec) -> String {
    let milestones = spec.milestones().count();
    let mut out = format!(
        "Gantt chart `{}` shown to the user: {} tasks ({milestones} milestones), project \
         duration {} working days",
        one_line(&spec.display_title(), 80),
        spec.tasks.len(),
        days(spec.project_duration)
    );
    match &spec.start {
        Some(start) => {
            let _ = write!(
                out,
                ", {start} to {} ({} calendar)",
                spec.finish_text(spec.project_duration),
                if spec.calendar == Calendar::Weekdays {
                    "Monday–Friday"
                } else {
                    "every-day"
                }
            );
        }
        None => out.push_str(" (no start date: days from the start)"),
    }
    let label = |id: &str| {
        spec.tasks
            .iter()
            .find(|t| t.id == id)
            .map_or(id.to_owned(), |t| one_line(t.label(), 40))
    };
    let path: Vec<String> = spec.critical_path.iter().map(|id| label(id)).collect();
    let _ = write!(out, ".\nCritical path: {}.", path.join(" → "));
    let _ = write!(
        out,
        "\nSchedule envelope `{}` (keys `project_duration`, `<task id>.es|ef|ls|lf|total_slack|\
         free_slack`, in working days).",
        spec.schedule.id
    );
    let mut slack: Vec<_> = spec
        .tasks
        .iter()
        .filter(|t| t.total_slack > SLACK_EPSILON)
        .collect();
    slack.sort_by(|a, b| b.total_slack.total_cmp(&a.total_slack));
    if !slack.is_empty() {
        let items: Vec<String> = slack
            .iter()
            .take(LISTED)
            .map(|t| {
                format!(
                    "{} {} d total / {} d free",
                    one_line(t.label(), 40),
                    days(t.total_slack),
                    days(t.free_slack)
                )
            })
            .collect();
        let _ = write!(out, "\nMost slack: {}.", items.join("; "));
    }
    if milestones > 0 {
        let items: Vec<String> = spec
            .milestones()
            .take(LISTED)
            .map(|t| {
                format!(
                    "{} {}",
                    one_line(t.label(), 40),
                    spec.start_text(t.early_start)
                )
            })
            .collect();
        let _ = write!(out, "\nMilestones: {}.", items.join("; "));
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 200));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("plan/tasks.csv", "id,duration\n");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowGanttTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_gantt_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_the_critical_path_and_slack() {
        let tool = Typed(ShowGanttTool::new(ports()));
        let out = call(
            &tool,
            json!({ "path": "plan/tasks.csv", "start": "2026-11-02", "calendar": "weekdays" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Gantt chart `tasks.csv` shown to the user: 3 tasks (0 milestones), project \
             duration 5 working days, 2026-11-02 to 2026-11-06 (Monday–Friday calendar).\n\
             Critical path: design → build.\n\
             Schedule envelope `pl_fake` (keys `project_duration`, `<task id>.es|ef|ls|lf|\
             total_slack|free_slack`, in working days).\n\
             Most slack: docs 1 d total / 1 d free."
        );
        assert!(matches!(out.block_update, Some(BlockKind::Gantt(_))));
        let inline = call(
            &tool,
            json!({ "tasks": [{ "id": "a", "duration": 2 }, { "id": "b", "duration": 1, "depends_on": ["a"] }] }),
        )
        .await
        .unwrap();
        assert!(inline.text_content().contains("no start date"));
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "tasks": [{ "id": "a", "bogus": 1 }] }))
                .await
                .is_err()
        );
        let missing = call(&tool, json!({ "path": "plan/none.csv" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
