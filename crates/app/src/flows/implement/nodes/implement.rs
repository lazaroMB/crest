//! Step 2, Implementar tarea: the agent implements the current task with
//! the task's role, restricted to its files (see [`run_agent`]).
//!
//! When the agent answers `STATUS: blocked`, the plan may not be feasible:
//! the step stops and asks (retry, go on, skip, abort or back to the Spec)
//! instead of improvising. With no task left it goes to Verificar.

use async_trait::async_trait;
use domain::plan::Task;
use domain::research::TestKind;

use super::{AgentStatus, run_agent, skip_task};
use crate::flows::implement::gate::{GateChoice, PROBLEM_OPTIONS, parse_choice};
use crate::flows::implement::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use crate::flows::implement::state::{EntryKind, Step, TaskStatus};

/// The prompt that asks the agent to implement `task` of `spec`.
#[must_use]
pub fn task_prompt(spec: &str, task: &Task, extra_files: &[String], hint: Option<&str>) -> String {
    let mut prompt = format!(
        "Implement task {} of the plan of spec {spec}: {}\n\n{}\n\nFiles you may change:\n",
        task.id,
        task.title,
        task.description.trim()
    );
    for file in task.files.iter().chain(extra_files) {
        prompt.push_str(&format!("- {file}\n"));
    }
    prompt.push_str(
        "Changing any other file asks the user and is recorded as a deviation from the plan.\n",
    );
    if !task.acceptance.is_empty() {
        prompt.push_str("\nAcceptance checks (they run when you finish):\n");
        for check in &task.acceptance {
            let kind = match check.kind {
                TestKind::Test => "test",
                TestKind::Command => "command",
            };
            prompt.push_str(&format!("- `{}` ({kind})\n", check.reference));
        }
    }
    if let Some(hint) = hint.map(str::trim).filter(|h| !h.is_empty()) {
        prompt.push_str(&format!("\nInstructions from the user: {hint}\n"));
    }
    prompt.push_str("\nEnd with `STATUS: done` or `STATUS: blocked: <reason>`.");
    prompt
}

/// Step 2, Implementar tarea.
#[derive(Debug, Default)]
pub struct ImplementTask;

impl ImplementTask {
    /// The step.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ImplementNode for ImplementTask {
    fn step(&self) -> Step {
        Step::Implement
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        let Some(task) = ctx.state().current_task().cloned() else {
            return Ok(StepOutcome::GoTo(Step::Verify));
        };
        let mut hint = None;
        if let Some(reply) = ctx.user_reply() {
            match parse_choice(reply) {
                GateChoice::Retry => {}
                GateChoice::Other(text) => hint = Some(text),
                GateChoice::Continue => {
                    ctx.log(
                        EntryKind::Deviation,
                        format!("Tarea {} bloqueada; el usuario pidió seguir así.", task.id),
                    );
                    ctx.state_mut().record_mut(&task.id).status = TaskStatus::Implemented;
                    return Ok(StepOutcome::Next);
                }
                GateChoice::Skip => return Ok(skip_task(ctx)),
                GateChoice::Abort => {
                    return Ok(StepOutcome::Abort {
                        reason: format!("tarea {} bloqueada", task.id),
                    });
                }
                GateChoice::BackToSpec => {
                    let why = ctx
                        .state()
                        .record(&task.id)
                        .and_then(|r| r.last_failure.clone())
                        .unwrap_or_else(|| "el plan resultó inviable".to_owned());
                    return Ok(StepOutcome::BackToSpec {
                        reason: format!("tarea {}: {why}", task.id),
                    });
                }
            }
        }

        let spec = ctx
            .state()
            .plan
            .as_ref()
            .map(|p| p.spec.clone())
            .unwrap_or_default();
        let extra = ctx
            .state()
            .record(&task.id)
            .map(|r| r.extra_files.clone())
            .unwrap_or_default();
        let prompt = task_prompt(&spec, &task, &extra, hint.as_deref());
        let report = run_agent(ctx, &task, prompt).await?;
        let record = ctx.state_mut().record_mut(&task.id);
        record.summary = Some(report.summary.clone()).filter(|s| !s.is_empty());
        match report.status {
            AgentStatus::Done => {
                record.status = TaskStatus::Implemented;
                record.attempts = 0;
                ctx.log(EntryKind::Note, format!("Tarea {} implementada.", task.id));
                Ok(StepOutcome::Next)
            }
            AgentStatus::Blocked(reason) => {
                record.last_failure = Some(reason.clone());
                ctx.log(
                    EntryKind::Failure,
                    format!("Tarea {} bloqueada: {reason}", task.id),
                );
                Ok(StepOutcome::Ask {
                    prompt: format!(
                        "La tarea **{}** ({}) no se puede hacer como dice el plan: {reason}. \
                         {PROBLEM_OPTIONS}",
                        task.id, task.title
                    ),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::flows::implement::nodes::parse_report;
    use crate::flows::implement::nodes::tests::{Harness, loaded_state};
    use crate::testing::llm::ScriptedResponse;

    #[test]
    fn parses_status_and_ideas() {
        let report = parse_report("Listo.\nIDEA: cachear X\n**STATUS: done**");
        assert_eq!(report.status, AgentStatus::Done);
        assert_eq!(report.ideas, ["cachear X"]);
        assert_eq!(report.summary, "Listo.");
        let blocked = parse_report("No.\nstatus: blocked: no existe la API");
        assert_eq!(
            blocked.status,
            AgentStatus::Blocked("no existe la API".into())
        );
        assert_eq!(parse_report("sin estado").status, AgentStatus::Done);
    }

    #[test]
    fn the_prompt_lists_files_and_checks() {
        let state = loaded_state(&[("T1", &[])]);
        let task = state.current_task().unwrap();
        let prompt = task_prompt("0007", task, &["extra.rs".into()], Some("usa Mutex"));
        assert!(prompt.contains("- src/t1.rs\n- extra.rs\n"), "{prompt}");
        assert!(prompt.contains("`cargo test t1` (test)"), "{prompt}");
        assert!(prompt.contains("Instructions from the user: usa Mutex"));
    }

    #[tokio::test]
    async fn a_file_outside_the_task_asks_and_goes_to_the_journal() {
        let h = Harness::without_shell([
            ScriptedResponse::tool_use(
                "toolu_1",
                "write_file",
                json!({ "path": "Cargo.toml", "content": "[workspace]\n" }),
            ),
            ScriptedResponse::tool_use(
                "toolu_2",
                "write_file",
                json!({ "path": "src/t1.rs", "content": "pub fn t1() {}\n" }),
            ),
            ScriptedResponse::text("Hecho.\nIDEA: medir el arranque\nSTATUS: done"),
        ]);
        h.approve_next();
        let mut ctx = h.ctx(Step::Implement, loaded_state(&[("T1", &[])]));
        let out = ImplementTask::new().run(&mut ctx).await.unwrap();
        assert_eq!(out, StepOutcome::Next);

        // Only the file outside the task asked (auto-edit mode).
        let requests = h.approval.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].subject, "Cargo.toml");
        assert_eq!(h.fs.file("src/t1.rs").as_deref(), Some("pub fn t1() {}\n"));

        let state = ctx.into_state();
        let deviation = state.entries(EntryKind::Deviation).next().unwrap();
        assert!(deviation.text.contains("Cargo.toml"), "{}", deviation.text);
        assert_eq!(deviation.task.as_ref().map(|t| t.as_str()), Some("T1"));
        let record = state.record(&state.order[0]).unwrap();
        assert_eq!(record.status, TaskStatus::Implemented);
        assert_eq!(record.extra_files, ["Cargo.toml"]);
        assert_eq!(state.entries(EntryKind::Idea).count(), 1);
        // The task's role picks the model.
        let req = h.llm.last_request().unwrap();
        assert_eq!(req.model.role, domain::model::ModelRole::Main);
    }

    #[tokio::test]
    async fn a_blocked_task_stops_and_can_go_back_to_the_spec() {
        let h = Harness::without_shell([ScriptedResponse::text(
            "STATUS: blocked: el puerto no tiene ese método",
        )]);
        let mut ctx = h.ctx(Step::Implement, loaded_state(&[("T1", &[])]));
        let out = ImplementTask::new().run(&mut ctx).await.unwrap();
        let StepOutcome::Ask { prompt } = out else {
            panic!("{out:?}")
        };
        assert!(prompt.contains("volver al spec"), "{prompt}");

        let state = ctx.into_state();
        let mut again = h
            .ctx(Step::Implement, state.clone())
            .with_reply("volver al spec");
        let out = ImplementTask::new().run(&mut again).await.unwrap();
        assert!(
            matches!(&out, StepOutcome::BackToSpec { reason } if reason.contains("ese método")),
            "{out:?}"
        );

        let mut skip = h.ctx(Step::Implement, state).with_reply("saltar");
        let out = ImplementTask::new().run(&mut skip).await.unwrap();
        assert_eq!(out, StepOutcome::GoTo(Step::Implement));
        assert_eq!(skip.state().current, 1);
        // No task left: on to Verificar.
        let out = ImplementTask::new().run(&mut skip).await.unwrap();
        assert_eq!(out, StepOutcome::GoTo(Step::Verify));
    }
}
