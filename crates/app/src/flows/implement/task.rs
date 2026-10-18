//! [`StepTask`]: the graph-flow [`Task`] that runs an [`ImplementNode`].
//!
//! | Outcome          | Next action    | Flow status       |
//! | ---------------- | -------------- | ----------------- |
//! | `Next`           | `Continue`     | `Running` (next)  |
//! | `Next`, last     | `End`          | `Completed`       |
//! | `GoTo(step)`     | `GoTo(step)`   | `Running` (step)  |
//! | `Ask`            | `WaitForInput` | `WaitingForInput` |
//! | `Finish`         | `End`          | `Completed`       |
//! | `BackToSpec`     | `End`          | `BackToSpec`      |
//! | `Abort`          | `End`          | `Aborted`         |
//!
//! `Continue` and `GoTo` both pause the runner, so every finished step is a
//! checkpoint. On entering a step (or the same step for another task) it
//! emits a `StageHeader`; at a gate it shows the prompt as a `Text` block
//! and writes the user's answer to the journal.

use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use graph_flow::{Context, GraphError, NextAction, Task, TaskResult};
use serde::Serialize;

use super::node::{ImplementDeps, ImplementNode, NodeError, StepContext, StepOutcome};
use super::state::{
    EntryKind, FlowStatus, GateReply, ImplementState, KEY_ENTERED, KEY_GATE, KEY_REPLY, KEY_STATE,
    KEY_STATUS, PendingGate, Step,
};

/// Reads the state of the flow (empty if missing).
pub(crate) fn load_state(ctx: &Context) -> ImplementState {
    ctx.get(KEY_STATE).unwrap_or_default()
}

/// Writes `value` under a frame key.
pub(crate) fn put(ctx: &Context, key: &str, value: impl Serialize) -> Result<(), NodeError> {
    ctx.set(key, value)
        .map_err(|e| NodeError::Context(e.to_string()))
}

/// Runs one [`ImplementNode`] as a graph-flow task.
pub struct StepTask {
    id: String,
    node: Arc<dyn ImplementNode>,
    deps: ImplementDeps,
    /// Steps of the graph, in order; `GoTo` must target one of them.
    steps: Arc<[Step]>,
}

impl std::fmt::Debug for StepTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepTask").field("id", &self.id).finish()
    }
}

impl StepTask {
    /// Wraps `node`, part of a graph with `steps` (in order).
    #[must_use]
    pub fn new(node: Arc<dyn ImplementNode>, deps: ImplementDeps, steps: Arc<[Step]>) -> Self {
        Self {
            id: node.step().task_id(),
            node,
            deps,
            steps,
        }
    }

    fn step(&self) -> Step {
        self.node.step()
    }

    fn next_step(&self) -> Option<Step> {
        let pos = self.steps.iter().position(|s| *s == self.step())?;
        self.steps.get(pos + 1).copied()
    }

    /// Appends a journal entry directly in the flow context.
    fn log(&self, ctx: &Context, kind: EntryKind, text: &str) -> Result<(), NodeError> {
        let mut state = load_state(ctx);
        let task = if self.step().is_per_task() {
            state.current_task().map(|t| t.id.clone())
        } else {
            None
        };
        state.log(self.deps.now(), self.step(), task, kind, text);
        put(ctx, KEY_STATE, &state)
    }

    fn end(&self, ctx: &Context, status: FlowStatus) -> Result<TaskResult, NodeError> {
        put(ctx, KEY_STATUS, status)?;
        Ok(TaskResult::new(None, NextAction::End))
    }

    /// Stops at a gate and shows its prompt.
    fn wait(&self, ctx: &Context, prompt: String) -> Result<TaskResult, NodeError> {
        let step = self.step();
        self.deps.emit(BlockKind::Text {
            markdown: prompt.clone(),
        })?;
        put(
            ctx,
            KEY_GATE,
            PendingGate {
                step,
                prompt: prompt.clone(),
            },
        )?;
        put(
            ctx,
            KEY_STATUS,
            FlowStatus::WaitingForInput {
                step,
                prompt: prompt.clone(),
            },
        )?;
        Ok(TaskResult::new(Some(prompt), NextAction::WaitForInput))
    }

    /// Emits the `StageHeader` when the flow enters the step, or the same
    /// step for another task.
    fn enter(&self, ctx: &Context, state: &ImplementState) -> Result<(), NodeError> {
        let step = self.step();
        let mark = (step, state.current);
        if ctx.get::<(Step, usize)>(KEY_ENTERED) == Some(mark) {
            return Ok(());
        }
        let title = match state.current_task().filter(|_| step.is_per_task()) {
            Some(task) => format!(
                "{} · {} ({}/{})",
                step.name(),
                task.id,
                state.current + 1,
                state.order.len()
            ),
            None => step.name().to_owned(),
        };
        self.deps.emit(BlockKind::StageHeader {
            stage: title,
            index: step.index(),
            total: Step::total(),
        })?;
        put(ctx, KEY_ENTERED, mark)
    }

    async fn run_step(&self, ctx: Context) -> Result<TaskResult, NodeError> {
        let step = self.step();
        let gate = ctx.get::<PendingGate>(KEY_GATE).filter(|g| g.step == step);
        let reply = ctx.get::<GateReply>(KEY_REPLY).filter(|r| r.step == step);
        ctx.remove(KEY_REPLY);

        let mut feedback = None;
        if let Some(gate) = gate {
            let Some(reply) = reply else {
                // Still waiting: running without an answer changes nothing.
                return Ok(TaskResult::new(Some(gate.prompt), NextAction::WaitForInput));
            };
            ctx.remove(KEY_GATE);
            let text = reply.text.trim();
            let shown = if text.is_empty() { "(empty)" } else { text };
            self.log(&ctx, EntryKind::User, &format!("User reply: {shown}"))?;
            feedback = Some(reply.text);
        }

        let state = load_state(&ctx);
        self.enter(&ctx, &state)?;
        let mut step_ctx = StepContext::in_flow(step, self.deps.clone(), state, feedback);
        let outcome = self.node.run(&mut step_ctx).await?;
        put(&ctx, KEY_STATE, step_ctx.into_state())?;

        match outcome {
            StepOutcome::Next => match self.next_step() {
                Some(next) => {
                    put(&ctx, KEY_STATUS, FlowStatus::Running { next })?;
                    Ok(TaskResult::new(None, NextAction::Continue))
                }
                None => {
                    self.log(&ctx, EntryKind::Note, "Flow finished.")?;
                    self.end(&ctx, FlowStatus::Completed)
                }
            },
            StepOutcome::GoTo(to) => {
                if !self.steps.contains(&to) {
                    return Err(NodeError::Failed(format!(
                        "{} asked to go to {}, which is not a step of the graph",
                        step.name(),
                        to.name()
                    )));
                }
                put(&ctx, KEY_STATUS, FlowStatus::Running { next: to })?;
                Ok(TaskResult::new(None, NextAction::GoTo(to.task_id())))
            }
            StepOutcome::Ask { prompt } => self.wait(&ctx, prompt),
            StepOutcome::Finish => {
                self.log(&ctx, EntryKind::Note, "Flow finished.")?;
                self.end(&ctx, FlowStatus::Completed)
            }
            StepOutcome::BackToSpec { reason } => {
                self.log(
                    &ctx,
                    EntryKind::Deviation,
                    &format!(
                        "The plan turned out to be unworkable; going back to the Spec: {reason}"
                    ),
                )?;
                self.deps.emit(BlockKind::Text {
                    markdown: format!(
                        "**The plan turned out to be unworkable**: {reason}\n\nThe implementation \
                         flow stopped. Go back to the Spec node of the research flow \
                         (`/back spec`) to fix it."
                    ),
                })?;
                self.end(&ctx, FlowStatus::BackToSpec { reason })
            }
            StepOutcome::Abort { reason } => {
                self.log(&ctx, EntryKind::Note, &format!("Flow aborted: {reason}"))?;
                self.end(&ctx, FlowStatus::Aborted)
            }
        }
    }
}

#[async_trait]
impl Task for StepTask {
    fn id(&self) -> &str {
        &self.id
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        self.run_step(context).await.map_err(|error| {
            let message = format!("{}: {error}", self.step().name());
            // The error block is best effort: it only fails on bad payloads.
            let _ = self.deps.emit(BlockKind::Error {
                message: message.clone(),
                hint: Some("Fix the problem and try again.".to_owned()),
            });
            GraphError::TaskExecutionFailed(message)
        })
    }
}

#[cfg(test)]
mod tests {
    use graph_flow::{ExecutionStatus, Graph, GraphBuilder, Session};

    use super::*;
    use crate::flows::implement::fake::FakeStep;
    use crate::flows::implement::node::tests::deps;
    use crate::ports::events::DomainEvent;
    use crate::testing::system::RecordingSink;

    fn graph(nodes: Vec<Arc<FakeStep>>, sink: &Arc<RecordingSink>) -> Graph {
        let deps = deps(sink);
        let steps: Arc<[Step]> = nodes.iter().map(|n| n.step()).collect();
        let mut builder = GraphBuilder::new("implement");
        for node in nodes {
            builder = builder.add_task(Arc::new(StepTask::new(
                node,
                deps.clone(),
                Arc::clone(&steps),
            )));
        }
        for pair in steps.windows(2) {
            builder = builder.add_edge(pair[0].task_id(), pair[1].task_id());
        }
        builder.build().unwrap()
    }

    fn session(step: Step) -> Session {
        Session::new_from_task("s".into(), &step.task_id())
    }

    fn status(session: &Session) -> FlowStatus {
        session.context.get(KEY_STATUS).unwrap()
    }

    fn headers(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => match block.kind {
                    BlockKind::StageHeader { stage, .. } => Some(stage),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn next_pauses_at_the_next_step_after_a_header() {
        let sink = Arc::new(RecordingSink::new());
        let g = graph(
            vec![
                Arc::new(FakeStep::new(Step::Load)),
                Arc::new(FakeStep::new(Step::Implement)),
            ],
            &sink,
        );
        let mut s = session(Step::Load);
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::Paused { .. }));
        assert_eq!(s.current_task_id, Step::Implement.task_id());
        assert_eq!(
            status(&s),
            FlowStatus::Running {
                next: Step::Implement
            }
        );
        assert_eq!(headers(&sink), ["Load plan"]);

        g.execute_session(&mut s).await.unwrap();
        assert_eq!(status(&s), FlowStatus::Completed);
        let state = load_state(&s.context);
        assert_eq!(state.journal.last().unwrap().text, "Flow finished.");
    }

    #[tokio::test]
    async fn ask_waits_and_reruns_with_the_answer() {
        let sink = Arc::new(RecordingSink::new());
        let load = Arc::new(FakeStep::new(Step::Load).then(StepOutcome::Ask {
            prompt: "Shall we start?".into(),
        }));
        let g = graph(
            vec![Arc::clone(&load), Arc::new(FakeStep::new(Step::Implement))],
            &sink,
        );
        let mut s = session(Step::Load);
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::WaitingForInput));
        assert_eq!(
            status(&s),
            FlowStatus::WaitingForInput {
                step: Step::Load,
                prompt: "Shall we start?".into()
            }
        );
        // No answer yet: nothing runs.
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(load.runs(), 1);

        let reply = GateReply {
            step: Step::Load,
            text: "yes".into(),
        };
        s.context.set(KEY_REPLY, reply).unwrap();
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(load.replies(), [None, Some("yes".into())]);
        assert_eq!(s.current_task_id, Step::Implement.task_id());
        let state = load_state(&s.context);
        assert!(state.journal.iter().any(|e| e.kind == EntryKind::User));
        // One header: the rerun is the same step.
        assert_eq!(headers(&sink), ["Load plan"]);
    }

    #[tokio::test]
    async fn goto_jumps_and_back_to_spec_ends() {
        let sink = Arc::new(RecordingSink::new());
        let commit = FakeStep::new(Step::Commit).then(StepOutcome::GoTo(Step::Implement));
        let implement = FakeStep::new(Step::Implement).then(StepOutcome::BackToSpec {
            reason: "the API does not exist".into(),
        });
        let g = graph(vec![Arc::new(implement), Arc::new(commit)], &sink);
        let mut s = session(Step::Commit);
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(s.current_task_id, Step::Implement.task_id());
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::Completed));
        assert_eq!(
            status(&s),
            FlowStatus::BackToSpec {
                reason: "the API does not exist".into()
            }
        );
        let state = load_state(&s.context);
        assert!(state.entries(EntryKind::Deviation).count() == 1);
    }

    #[tokio::test]
    async fn goto_outside_the_graph_and_failures_are_errors() {
        let sink = Arc::new(RecordingSink::new());
        let load = FakeStep::new(Step::Load)
            .then(StepOutcome::GoTo(Step::Report))
            .then_fail("no plan");
        let g = graph(vec![Arc::new(load)], &sink);
        let mut s = session(Step::Load);
        let err = g.execute_session(&mut s).await.unwrap_err();
        assert!(err.to_string().contains("not a step"), "{err}");
        let err = g.execute_session(&mut s).await.unwrap_err();
        assert!(err.to_string().contains("no plan"), "{err}");
        assert!(sink.events().iter().any(|e| matches!(
            e,
            DomainEvent::BlockAdded { block } if matches!(block.kind, BlockKind::Error { .. })
        )));
    }
}
