//! [`NodeTask`]: the graph-flow [`Task`] that runs a [`ResearchNode`].
//!
//! It translates the [`NodeOutcome`] into a [`NextAction`]:
//!
//! | Outcome                  | Next action    | Flow status       |
//! | ------------------------ | -------------- | ----------------- |
//! | `Done`, no gate          | `Continue`     | `Running` (next)  |
//! | `Done`, last node        | `End`          | `Completed`       |
//! | `Done`, gated node       | `WaitForInput` | `WaitingForInput` |
//! | `NeedsInput`             | `WaitForInput` | `WaitingForInput` |
//! | `Back { to }`            | `GoTo(to)`     | `Running` (`to`)  |
//!
//! `Continue` (never `ContinueAndExecute`) makes each finished node a
//! checkpoint: the runner saves the session after every node.
//!
//! On entering a stage it emits a `StageHeader`; at a gate it shows the
//! prompt as a `Text` block. Gate answers, returns and the end of the flow
//! are written to the journal.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::research::{ResearchState, Stage};
use graph_flow::{Context, GraphError, NextAction, Task, TaskResult};
use serde::Serialize;

use super::gate::{
    FlowStatus, GateKind, GateReply, KEY_BACKS, KEY_ENTERED, KEY_GATE, KEY_PROBLEM, KEY_REPLY,
    KEY_STATE, KEY_STATUS, PendingGate, approves, task_id,
};
use super::node::{NodeContext, NodeDeps, NodeError, NodeOutcome, ResearchNode};
use crate::ports::events::NoticeLevel;

/// Times a stage may send the flow back before the frame asks the user.
pub const MAX_BACKS_PER_STAGE: u32 = 3;

/// Times a node that failed with a transient error (see
/// [`NodeError::is_transient`]) runs again before the flow fails.
pub const MAX_NODE_RETRIES: u32 = 2;

/// What the error block of a failed node suggests.
pub const FAILED_NODE_HINT: &str = "The flow is saved at this stage. `/research retry` runs it \
     again, `/back <stage>` redoes an earlier one and `/abort` drops the flow.";

/// Reads the research state of the flow (empty if missing).
pub(crate) fn load_state(ctx: &Context) -> ResearchState {
    ctx.get(KEY_STATE).unwrap_or_default()
}

/// Writes `value` under a frame key.
pub(crate) fn put(ctx: &Context, key: &str, value: impl Serialize) -> Result<(), NodeError> {
    ctx.set(key, value)
        .map_err(|e| NodeError::Context(e.to_string()))
}

/// Appends a journal entry for `stage` directly in the flow context.
pub(crate) fn log(ctx: &Context, at: u64, stage: Stage, text: &str) -> Result<(), NodeError> {
    let mut state = load_state(ctx);
    state.log(at, stage, text);
    put(ctx, KEY_STATE, &state)
}

/// The confirmation prompt of a gated stage.
fn confirm_prompt(stage: Stage, next: Option<Stage>) -> String {
    let then = next.map_or_else(
        || "finish the flow".to_owned(),
        |n| format!("go on to {} ({}/{})", n.name(), n.index(), n.total()),
    );
    format!(
        "**{}** is done. Answer \"yes\" to {then}, write corrections to redo this \
         stage or use `/back <stage>` to go back.",
        stage.name()
    )
}

/// Runs one [`ResearchNode`] as a graph-flow task.
pub struct NodeTask {
    id: String,
    node: Arc<dyn ResearchNode>,
    deps: NodeDeps,
    /// Stages of the graph, in order; `Back` must target one of them.
    stages: Arc<[Stage]>,
}

impl std::fmt::Debug for NodeTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeTask").field("id", &self.id).finish()
    }
}

impl NodeTask {
    /// Wraps `node`, part of a graph with `stages` (in order).
    #[must_use]
    pub fn new(node: Arc<dyn ResearchNode>, deps: NodeDeps, stages: Arc<[Stage]>) -> Self {
        Self {
            id: task_id(node.stage()),
            node,
            deps,
            stages,
        }
    }

    fn stage(&self) -> Stage {
        self.node.stage()
    }

    fn next_stage(&self) -> Option<Stage> {
        let stage = self.stage();
        let pos = self.stages.iter().position(|s| *s == stage)?;
        self.stages.get(pos + 1).copied()
    }

    /// Moves on after the node finished (and its gate, if any, approved).
    fn advance(&self, ctx: &Context) -> Result<TaskResult, NodeError> {
        if let Some(next) = self.next_stage() {
            put(ctx, KEY_STATUS, FlowStatus::Running { next })?;
            return Ok(TaskResult::new(None, NextAction::Continue));
        }
        log(ctx, self.deps.now(), self.stage(), "Flow finished.")?;
        put(ctx, KEY_STATUS, FlowStatus::Completed)?;
        Ok(TaskResult::new(None, NextAction::End))
    }

    /// Stops at a gate and shows its prompt.
    fn wait(&self, ctx: &Context, kind: GateKind, prompt: String) -> Result<TaskResult, NodeError> {
        let stage = self.stage();
        self.deps.emit(BlockKind::Text {
            markdown: prompt.clone(),
        })?;
        put(
            ctx,
            KEY_GATE,
            PendingGate {
                stage,
                kind,
                prompt: prompt.clone(),
            },
        )?;
        put(
            ctx,
            KEY_STATUS,
            FlowStatus::WaitingForInput {
                stage,
                prompt: prompt.clone(),
            },
        )?;
        Ok(TaskResult::new(Some(prompt), NextAction::WaitForInput))
    }

    /// Emits the `StageHeader` when the flow enters a new stage.
    fn enter(&self, ctx: &Context) -> Result<(), NodeError> {
        let stage = self.stage();
        if ctx.get::<Stage>(KEY_ENTERED) == Some(stage) {
            return Ok(());
        }
        self.deps.emit(BlockKind::StageHeader {
            stage: stage.name().to_owned(),
            index: stage.index(),
            total: stage.total(),
        })?;
        put(ctx, KEY_ENTERED, stage)
    }

    /// Goes back to `to`, or asks the user once the stage did it too often.
    fn back(&self, ctx: &Context, to: Stage, reason: &str) -> Result<TaskResult, NodeError> {
        let stage = self.stage();
        if !self.stages.contains(&to) || to >= stage {
            return Err(NodeError::Failed(format!(
                "{} asked to go back to {}, which is not an earlier stage of the graph",
                stage.name(),
                to.name()
            )));
        }
        let mut backs: BTreeMap<Stage, u32> = ctx.get(KEY_BACKS).unwrap_or_default();
        let count = backs.entry(stage).or_default();
        *count += 1;
        let count = *count;
        put(ctx, KEY_BACKS, &backs)?;
        if count > MAX_BACKS_PER_STAGE {
            let prompt = format!(
                "**{}** asked to go back to **{}** {count} times ({reason}). Tell me what is \
                 missing to redo this stage, or use `/back <stage>`.",
                stage.name(),
                to.name()
            );
            return self.wait(ctx, GateKind::Question, prompt);
        }
        log(
            ctx,
            self.deps.now(),
            stage,
            &format!("Back to {}: {reason}", to.name()),
        )?;
        self.deps.notice(
            NoticeLevel::Info,
            format!("{} goes back to {}: {reason}", stage.name(), to.name()),
        );
        put(ctx, KEY_STATUS, FlowStatus::Running { next: to })?;
        Ok(TaskResult::new(None, NextAction::GoTo(task_id(to))))
    }

    /// The user answered: the stage may send the flow back again.
    fn reset_backs(&self, ctx: &Context) -> Result<(), NodeError> {
        let mut backs: BTreeMap<Stage, u32> = ctx.get(KEY_BACKS).unwrap_or_default();
        if backs.remove(&self.stage()).is_some() {
            put(ctx, KEY_BACKS, &backs)?;
        }
        Ok(())
    }

    async fn run_node(&self, ctx: Context) -> Result<TaskResult, NodeError> {
        let stage = self.stage();
        let gate = ctx
            .get::<PendingGate>(KEY_GATE)
            .filter(|g| g.stage == stage);
        let reply = ctx.get::<GateReply>(KEY_REPLY).filter(|r| r.stage == stage);
        ctx.remove(KEY_REPLY);

        let mut feedback = None;
        if let Some(gate) = gate {
            let Some(reply) = reply else {
                // Still waiting: run without an answer changes nothing.
                return Ok(TaskResult::new(Some(gate.prompt), NextAction::WaitForInput));
            };
            ctx.remove(KEY_GATE);
            self.reset_backs(&ctx)?;
            if !reply.text.trim().is_empty() {
                log(
                    &ctx,
                    self.deps.now(),
                    stage,
                    &format!("User answer: {}", reply.text.trim()),
                )?;
            }
            if gate.kind == GateKind::Confirm && approves(&reply.text, self.deps.decider()).await {
                return self.advance(&ctx);
            }
            feedback = Some(reply.text);
        }

        self.enter(&ctx)?;
        let problem: String = ctx.get(KEY_PROBLEM).unwrap_or_default();
        let mut node_ctx = NodeContext::in_flow(
            stage,
            self.deps.clone(),
            load_state(&ctx),
            problem,
            feedback,
            ctx.clone(),
        )
        .with_stages(Arc::clone(&self.stages));
        let outcome = self.node.run(&mut node_ctx).await?;
        put(&ctx, KEY_STATE, node_ctx.into_state())?;

        match outcome {
            NodeOutcome::Done if self.node.has_gate() => {
                let prompt = confirm_prompt(stage, self.next_stage());
                self.wait(&ctx, GateKind::Confirm, prompt)
            }
            NodeOutcome::Done => self.advance(&ctx),
            NodeOutcome::NeedsInput { prompt } => self.wait(&ctx, GateKind::Question, prompt),
            NodeOutcome::Back { to, reason } => self.back(&ctx, to, &reason),
        }
    }
}

#[async_trait]
impl Task for NodeTask {
    fn id(&self) -> &str {
        &self.id
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // A rerun must see the gate and the answer the first attempt took.
        let gate = context.get::<PendingGate>(KEY_GATE);
        let reply = context.get::<GateReply>(KEY_REPLY);
        let mut attempt = 0;
        loop {
            let error = match self.run_node(context.clone()).await {
                Ok(result) => return Ok(result),
                Err(error) => error,
            };
            let stage = self.stage().name();
            if attempt < MAX_NODE_RETRIES && error.is_transient() {
                attempt += 1;
                self.deps.notice(
                    NoticeLevel::Warning,
                    format!("{stage}: {error}; retrying ({attempt}/{MAX_NODE_RETRIES})"),
                );
                if let Some(gate) = &gate {
                    context.set(KEY_GATE, gate)?;
                }
                if let Some(reply) = &reply {
                    context.set(KEY_REPLY, reply)?;
                }
                continue;
            }
            let message = format!("{stage}: {error}");
            // The error block is best effort: it only fails on bad payloads.
            let _ = self.deps.emit(BlockKind::Error {
                message: message.clone(),
                hint: Some(FAILED_NODE_HINT.to_owned()),
            });
            return Err(GraphError::TaskExecutionFailed(message));
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;
    use graph_flow::{ExecutionStatus, Graph, GraphBuilder, Session};

    use super::*;
    use crate::flows::research::fake::FakeNode;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    /// A linear graph of `nodes`, as the builder makes it.
    fn graph(nodes: Vec<Arc<FakeNode>>, sink: &Arc<RecordingSink>) -> Graph {
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::clone(sink) as _,
        );
        let stages: Arc<[Stage]> = nodes.iter().map(|n| n.stage()).collect();
        let mut builder = GraphBuilder::new("research");
        for node in nodes {
            let task = NodeTask::new(node, deps.clone(), Arc::clone(&stages));
            builder = builder.add_task(Arc::new(task));
        }
        for pair in stages.windows(2) {
            builder = builder.add_edge(task_id(pair[0]), task_id(pair[1]));
        }
        builder.build().unwrap()
    }

    fn session(stage: Stage) -> Session {
        Session::new_from_task("s".into(), &task_id(stage))
    }

    fn status(session: &Session) -> FlowStatus {
        session.context.get(KEY_STATUS).unwrap()
    }

    fn reply(session: &Session, stage: Stage, text: &str) {
        let reply = GateReply {
            stage,
            text: text.into(),
        };
        session.context.set(KEY_REPLY, reply).unwrap();
    }

    #[tokio::test]
    async fn done_continues_to_the_next_node_after_a_header() {
        let sink = Arc::new(RecordingSink::new());
        let a = Arc::new(FakeNode::new(Stage::Observation));
        let b = Arc::new(FakeNode::new(Stage::Analysis));
        let g = graph(vec![Arc::clone(&a), b], &sink);
        let mut s = session(Stage::Observation);
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::Paused { .. }));
        assert_eq!(s.current_task_id, task_id(Stage::Analysis));
        assert_eq!(
            status(&s),
            FlowStatus::Running {
                next: Stage::Analysis
            }
        );
        assert_eq!(a.runs(), 1);
        let header = sink.events().into_iter().find_map(|e| match e {
            DomainEvent::BlockAdded { block } => Some(block.kind),
            _ => None,
        });
        assert_eq!(
            header,
            Some(BlockKind::StageHeader {
                stage: "Observation".into(),
                index: 2,
                total: 14,
            })
        );
        assert_eq!(load_state(&s.context).journal[0].stage, Stage::Observation);
    }

    #[tokio::test]
    async fn gated_nodes_wait_and_rerun_only_on_feedback() {
        let sink = Arc::new(RecordingSink::new());
        let h = Arc::new(FakeNode::new(Stage::Hypotheses));
        let g = graph(
            vec![Arc::clone(&h), Arc::new(FakeNode::new(Stage::Definition))],
            &sink,
        );
        let mut s = session(Stage::Hypotheses);
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::WaitingForInput));
        assert!(matches!(
            status(&s),
            FlowStatus::WaitingForInput {
                stage: Stage::Hypotheses,
                ..
            }
        ));

        // No answer yet: the node does not run again.
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(h.runs(), 1);

        // Feedback reruns the node with it and asks again.
        reply(&s, Stage::Hypotheses, "the network hypothesis is missing");
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::WaitingForInput));
        assert_eq!(
            h.replies(),
            [None, Some("the network hypothesis is missing".into())]
        );

        // Approval moves on without running it again.
        reply(&s, Stage::Hypotheses, "yes");
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(h.runs(), 2);
        assert_eq!(s.current_task_id, task_id(Stage::Definition));
    }

    #[tokio::test]
    async fn back_must_target_an_earlier_stage_of_the_graph() {
        let sink = Arc::new(RecordingSink::new());
        let a = FakeNode::new(Stage::Observation).then(NodeOutcome::Back {
            to: Stage::Analysis,
            reason: "x".into(),
        });
        let g = graph(
            vec![Arc::new(a), Arc::new(FakeNode::new(Stage::Analysis))],
            &sink,
        );
        let mut s = session(Stage::Observation);
        let err = g.execute_session(&mut s).await.unwrap_err();
        assert!(err.to_string().contains("not an earlier stage"), "{err}");
        assert!(sink.kinds().contains(&"block_added".to_owned()));
    }

    /// Goes back to the first of Data and Observation the graph has.
    struct NeedsData;

    #[async_trait]
    impl ResearchNode for NeedsData {
        fn stage(&self) -> Stage {
            Stage::Analysis
        }

        async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
            Ok(NodeOutcome::Back {
                to: ctx.first_stage_of(&[Stage::Data, Stage::Observation]),
                reason: format!("{} stages", ctx.stages().len()),
            })
        }
    }

    #[tokio::test]
    async fn nodes_see_the_stages_of_their_graph() {
        let sink = Arc::new(RecordingSink::new());
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::clone(&sink) as _,
        );
        let stages: Arc<[Stage]> = [Stage::Observation, Stage::Analysis].into();
        let g = GraphBuilder::new("research")
            .add_task(Arc::new(NodeTask::new(
                Arc::new(FakeNode::new(Stage::Observation)),
                deps.clone(),
                Arc::clone(&stages),
            )))
            .add_task(Arc::new(NodeTask::new(
                Arc::new(NeedsData),
                deps,
                Arc::clone(&stages),
            )))
            .add_edge(task_id(Stage::Observation), task_id(Stage::Analysis))
            .build()
            .unwrap();
        let mut s = session(Stage::Analysis);
        g.execute_session(&mut s).await.unwrap();
        assert_eq!(s.current_task_id, task_id(Stage::Observation));
        let journal = load_state(&s.context).journal;
        assert!(
            journal.last().unwrap().text.contains("2 stages"),
            "{journal:?}"
        );
    }

    #[tokio::test]
    async fn repeated_backs_turn_into_a_question() {
        let sink = Arc::new(RecordingSink::new());
        let back = NodeOutcome::Back {
            to: Stage::Observation,
            reason: "data is missing".into(),
        };
        let b = (0..=MAX_BACKS_PER_STAGE)
            .fold(FakeNode::new(Stage::Analysis), |n, _| n.then(back.clone()));
        let g = graph(
            vec![Arc::new(FakeNode::new(Stage::Observation)), Arc::new(b)],
            &sink,
        );
        let mut s = session(Stage::Analysis);
        for _ in 0..MAX_BACKS_PER_STAGE {
            g.execute_session(&mut s).await.unwrap();
            assert_eq!(s.current_task_id, task_id(Stage::Observation));
            s.current_task_id = task_id(Stage::Analysis);
        }
        let out = g.execute_session(&mut s).await.unwrap();
        assert!(matches!(out.status, ExecutionStatus::WaitingForInput));
        assert!(out.response.unwrap().contains("4 times"));
    }
}
