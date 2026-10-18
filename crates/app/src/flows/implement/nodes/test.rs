//! Step 3, Test and fix: runs the acceptance checks of the task;
//! when they fail, asks the agent to fix them up to [`MAX_FIX_ATTEMPTS`]
//! times and then stops at a gate: retry (with instructions), go on with
//! the failures, skip the task, abort or go back to the Spec.

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, TableSpec};
use domain::plan::Task;

use super::{AgentStatus, run_agent, skip_task};
use crate::flows::implement::checks::{CheckRun, run_check};
use crate::flows::implement::gate::{GateChoice, PROBLEM_OPTIONS, parse_choice};
use crate::flows::implement::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use crate::flows::implement::state::{EntryKind, Step, TaskStatus};

/// Fix attempts per task before the step stops and asks.
pub const MAX_FIX_ATTEMPTS: u32 = 3;

/// The prompt that asks the agent to fix the failing checks of `task`.
#[must_use]
pub fn fix_prompt(task: &Task, failures: &str, attempt: u32, hint: Option<&str>) -> String {
    let mut prompt = format!(
        "The acceptance checks of task {} ({}) fail. Fix the implementation \
         (attempt {attempt} of {MAX_FIX_ATTEMPTS}); change only the task's files: {}.\n\n\
         Failing checks:\n{failures}\n",
        task.id,
        task.title,
        task.files.join(", ")
    );
    if let Some(hint) = hint.map(str::trim).filter(|h| !h.is_empty()) {
        prompt.push_str(&format!("\nInstructions from the user: {hint}\n"));
    }
    prompt.push_str("\nEnd with `STATUS: done` or `STATUS: blocked: <reason>`.");
    prompt
}

/// Step 3, Test and fix.
#[derive(Debug, Default)]
pub struct TestTask;

impl TestTask {
    /// The step.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn show(ctx: &StepContext, runs: &[CheckRun]) -> Result<(), NodeError> {
        if runs.is_empty() {
            return Ok(());
        }
        let rows = runs
            .iter()
            .map(|r| vec![Cell::Text(r.command.clone()), Cell::Text(r.verdict())])
            .collect();
        ctx.emit(BlockKind::Table(TableSpec {
            columns: vec![Column::new("Check"), Column::new("Result")],
            rows,
        }))?;
        Ok(())
    }

    fn stop(task: &Task, failures: &str, why: &str) -> StepOutcome {
        StepOutcome::Ask {
            prompt: format!(
                "Task **{}** ({}) {why}:\n\n{failures}\n\n{PROBLEM_OPTIONS}",
                task.id, task.title
            ),
        }
    }
}

#[async_trait]
impl ImplementNode for TestTask {
    fn step(&self) -> Step {
        Step::Test
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        let Some(task) = ctx.state().current_task().cloned() else {
            return Ok(StepOutcome::GoTo(Step::Verify));
        };
        let mut hint = None;
        let mut fix_first = false;
        if let Some(reply) = ctx.user_reply() {
            match parse_choice(reply) {
                GateChoice::Retry => fix_first = true,
                GateChoice::Other(text) => {
                    fix_first = true;
                    hint = Some(text);
                }
                GateChoice::Continue => {
                    ctx.log(
                        EntryKind::Deviation,
                        format!(
                            "Task {} keeps failing checks, by the user's decision.",
                            task.id
                        ),
                    );
                    ctx.state_mut().record_mut(&task.id).status = TaskStatus::Failed;
                    return Ok(StepOutcome::Next);
                }
                GateChoice::Skip => return Ok(skip_task(ctx)),
                GateChoice::Abort => {
                    return Ok(StepOutcome::Abort {
                        reason: format!("the checks of {} do not pass", task.id),
                    });
                }
                GateChoice::BackToSpec => {
                    return Ok(StepOutcome::BackToSpec {
                        reason: format!("the acceptance checks of {} cannot be met", task.id),
                    });
                }
            }
            if fix_first {
                ctx.state_mut().record_mut(&task.id).attempts = 0;
            }
        }

        loop {
            if !fix_first {
                let mut runs = Vec::with_capacity(task.acceptance.len());
                for check in &task.acceptance {
                    runs.push(run_check(ctx.shell().as_ref(), &check.reference).await);
                }
                Self::show(ctx, &runs)?;
                let failed: Vec<_> = runs.iter().filter(|r| !r.passed()).collect();
                if failed.is_empty() {
                    ctx.state_mut().record_mut(&task.id).status = TaskStatus::Passed;
                    ctx.log(
                        EntryKind::Achievement,
                        format!(
                            "Task {}: its {} acceptance checks pass.",
                            task.id,
                            runs.len()
                        ),
                    );
                    return Ok(StepOutcome::Next);
                }
                let failures = failed
                    .iter()
                    .map(|r| r.describe())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let record = ctx.state_mut().record_mut(&task.id);
                record.last_failure = Some(failures.clone());
                let attempts = record.attempts;
                if attempts >= MAX_FIX_ATTEMPTS {
                    ctx.log(
                        EntryKind::Failure,
                        format!(
                            "Task {}: the checks still fail after {attempts} \
                             fix attempts.\n\n{failures}",
                            task.id
                        ),
                    );
                    return Ok(Self::stop(
                        &task,
                        &failures,
                        &format!("used up its {MAX_FIX_ATTEMPTS} fix attempts"),
                    ));
                }
            }
            fix_first = false;

            let record = ctx.state_mut().record_mut(&task.id);
            record.attempts += 1;
            let attempt = record.attempts;
            let failures = record.last_failure.clone().unwrap_or_default();
            ctx.log(
                EntryKind::Note,
                format!(
                    "Task {}: fix attempt {attempt}/{MAX_FIX_ATTEMPTS}.",
                    task.id
                ),
            );
            let prompt = fix_prompt(&task, &failures, attempt, hint.take().as_deref());
            let report = run_agent(ctx, &task, prompt).await?;
            if let AgentStatus::Blocked(reason) = report.status {
                ctx.log(
                    EntryKind::Failure,
                    format!("Task {} blocked while fixing: {reason}", task.id),
                );
                return Ok(Self::stop(
                    &task,
                    &failures,
                    &format!("cannot be fixed as the plan says ({reason})"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::flows::implement::nodes::tests::{Harness, SequenceShell, exit, loaded_state};
    use crate::testing::fs::ScriptedShell;
    use crate::testing::llm::ScriptedResponse;

    fn fixed() -> ScriptedResponse {
        ScriptedResponse::text("Fixed.\nSTATUS: done")
    }

    #[tokio::test]
    async fn passing_checks_go_on_to_the_commit() {
        let shell = ScriptedShell::new().on("cargo", ["test", "t1"], exit(0, "ok"));
        let h = Harness::new([], Arc::new(shell));
        let mut ctx = h.ctx(Step::Test, loaded_state(&[("T1", &[])]));
        assert_eq!(
            TestTask::new().run(&mut ctx).await.unwrap(),
            StepOutcome::Next
        );
        let state = ctx.into_state();
        assert_eq!(
            state.record(&state.order[0]).unwrap().status,
            TaskStatus::Passed
        );
        assert_eq!(state.entries(EntryKind::Achievement).count(), 1);
    }

    #[tokio::test]
    async fn a_fix_that_works_passes_on_the_next_run() {
        let shell = Arc::new(SequenceShell::new([exit(101, "FAILED"), exit(0, "ok")]));
        let h = Harness::new([fixed()], Arc::clone(&shell) as _);
        let mut ctx = h.ctx(Step::Test, loaded_state(&[("T1", &[])]));
        assert_eq!(
            TestTask::new().run(&mut ctx).await.unwrap(),
            StepOutcome::Next
        );
        let req = h.llm.last_request().unwrap();
        let prompt = format!("{:?}", req.messages);
        assert!(
            prompt.contains("attempt 1 of 3") && prompt.contains("FAILED"),
            "{prompt}"
        );
        assert_eq!(
            ctx.state().record(&ctx.state().order[0]).unwrap().attempts,
            1
        );
        assert_eq!(shell.calls().len(), 2);
    }

    #[tokio::test]
    async fn exhausted_retries_stop_and_ask() {
        let shell = Arc::new(ScriptedShell::new().with_default(exit(101, "test t1 ... FAILED")));
        let h = Harness::new([fixed(), fixed(), fixed()], Arc::clone(&shell) as _);
        let mut ctx = h.ctx(Step::Test, loaded_state(&[("T1", &[]), ("T2", &[])]));
        let out = TestTask::new().run(&mut ctx).await.unwrap();
        let StepOutcome::Ask { prompt } = out else {
            panic!("{out:?}")
        };
        assert!(prompt.contains("used up its 3 fix attempts"), "{prompt}");
        assert!(prompt.contains("\"skip\"") && prompt.contains("\"back to spec\""));
        assert_eq!(shell.calls().len(), 4, "one run and three after fixes");
        assert_eq!(h.llm.requests().len(), 3);
        let state = ctx.into_state();
        assert_eq!(state.record(&state.order[0]).unwrap().attempts, 3);
        assert_eq!(state.entries(EntryKind::Failure).count(), 1);

        // Answers at the gate.
        let mut skip = h.ctx(Step::Test, state.clone()).with_reply("skip");
        assert_eq!(
            TestTask::new().run(&mut skip).await.unwrap(),
            StepOutcome::GoTo(Step::Implement)
        );
        assert_eq!(skip.state().current, 1);
        let mut go_on = h.ctx(Step::Test, state.clone()).with_reply("continue");
        assert_eq!(
            TestTask::new().run(&mut go_on).await.unwrap(),
            StepOutcome::Next
        );
        let record = go_on.state().record(&state.order[0]).unwrap().clone();
        assert_eq!(record.status, TaskStatus::Failed);
        let mut back = h.ctx(Step::Test, state).with_reply("back to spec");
        assert!(matches!(
            TestTask::new().run(&mut back).await.unwrap(),
            StepOutcome::BackToSpec { .. }
        ));
    }

    #[tokio::test]
    async fn instructions_at_the_gate_reset_the_attempts() {
        let shell = SequenceShell::new([exit(0, "ok")]);
        let h = Harness::new([fixed()], Arc::new(shell));
        let mut state = loaded_state(&[("T1", &[])]);
        let id = state.order[0].clone();
        state.record_mut(&id).attempts = 3;
        state.record_mut(&id).last_failure = Some("`cargo test t1` → fails".into());
        let mut ctx = h.ctx(Step::Test, state).with_reply("use a Mutex");
        assert_eq!(
            TestTask::new().run(&mut ctx).await.unwrap(),
            StepOutcome::Next
        );
        let prompt = format!("{:?}", h.llm.last_request().unwrap().messages);
        assert!(
            prompt.contains("Instructions from the user: use a Mutex"),
            "{prompt}"
        );
        assert_eq!(ctx.state().record(&id).unwrap().attempts, 1);
    }
}
