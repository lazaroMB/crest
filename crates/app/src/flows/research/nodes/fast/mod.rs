//! The five stages of the fast pipeline, registered by stage.
//!
//! Each one reuses the nodes of the deep pipeline: Triage runs Intake,
//! Discovery runs the gathering half of Observation, Data and Research at
//! once, Framing runs Hypotheses and Definition, Solution runs Brainstorm
//! and Modeling, and Delivery writes, plans and reviews each Spec at once.
//! Model calls run concurrently with `futures` combinators, never
//! `tokio::spawn`, so cancelling the flow cancels them all; blocks are
//! emitted afterwards, in a fixed order.

use std::sync::Arc;

use domain::model::ModelRole;
use domain::research::Stage;

use super::super::depth::Depth;
use super::super::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

pub mod delivery;
pub mod discovery;
pub mod framing;
pub mod solution;
pub mod triage;

/// The fast node registered for `stage`, if it is a fast stage.
///
/// Delivery writes its Specs to memory here; the graph gives it the
/// workspace sink (see [`fast_graph`](super::super::graph::fast_graph)).
#[must_use]
pub fn fast_node_for(stage: Stage) -> Option<Arc<dyn ResearchNode>> {
    match stage {
        Stage::Triage => Some(Arc::new(triage::Triage::new())),
        Stage::Discovery => Some(Arc::new(discovery::Discovery::new())),
        Stage::Framing => Some(Arc::new(framing::Framing::new())),
        Stage::Solution => Some(Arc::new(solution::Solution::new())),
        Stage::Delivery => Some(Arc::new(delivery::Delivery::new())),
        _ => None,
    }
}

/// A deep node run as one step of a fast stage.
pub(crate) struct Step {
    node: Arc<dyn ResearchNode>,
    /// Whether it runs with the main model at quick depth.
    main_when_quick: bool,
}

impl Step {
    /// `node` with its own role.
    pub(crate) fn new(node: Arc<dyn ResearchNode>) -> Self {
        Self {
            node,
            main_when_quick: false,
        }
    }

    /// `node`, with the main model at quick depth.
    pub(crate) fn main_when_quick(node: Arc<dyn ResearchNode>) -> Self {
        Self {
            node,
            main_when_quick: true,
        }
    }
}

/// Flow data key of the step a stage resumes at, under `key`.
pub(crate) fn resume_key(key: &str) -> String {
    format!("{key}.resume_at")
}

/// Runs `steps` in order inside the current stage (see
/// [`NodeContext::run_as`]) and returns the first outcome that is not
/// `Done`.
///
/// A step that asks the user (`NeedsInput`) is stored under
/// `<key>.resume_at`, so the run with the answer starts at that step
/// instead of redoing the earlier ones; it is cleared once the steps end
/// any other way. A run without an answer always starts from the first
/// step.
pub(crate) async fn run_steps(
    ctx: &mut NodeContext,
    steps: &[Step],
    key: &str,
) -> Result<NodeOutcome, NodeError> {
    let resume = resume_key(key);
    let start = ctx
        .data::<Option<usize>>(&resume)
        .flatten()
        .filter(|_| ctx.user_reply().is_some())
        .unwrap_or(0);
    let quick = ctx.depth() == Depth::Quick;
    for (i, step) in steps.iter().enumerate().skip(start) {
        if quick && step.main_when_quick {
            ctx.set_role_override(Some(ModelRole::Main));
        }
        let result = ctx.run_as(step.node.as_ref()).await;
        ctx.set_role_override(None);
        match result? {
            NodeOutcome::Done => {}
            asks @ NodeOutcome::NeedsInput { .. } => {
                ctx.set_data(&resume, Some(i))?;
                return Ok(asks);
            }
            other => {
                ctx.set_data(&resume, None::<usize>)?;
                return Ok(other);
            }
        }
    }
    ctx.set_data(&resume, None::<usize>)?;
    Ok(NodeOutcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flows::research::depth::DEPTH_KEY;
    use crate::flows::research::fake::FakeNode;
    use crate::flows::research::node::NodeDeps;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;
    use domain::model::ModelPolicy;
    use domain::research::ResearchState;

    fn context(llm: Arc<ScriptedLlm>) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), Arc::new(RecordingSink::new()));
        NodeContext::new(Stage::Framing, deps, "p", ResearchState::default())
    }

    /// Asks the model once and records the role it used.
    struct Asks(Stage);

    #[async_trait::async_trait]
    impl ResearchNode for Asks {
        fn stage(&self) -> Stage {
            self.0
        }

        async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
            ctx.ask("", "x").await?;
            Ok(NodeOutcome::Done)
        }
    }

    #[tokio::test]
    async fn an_answer_resumes_at_the_step_that_asked() {
        let first = Arc::new(FakeNode::new(Stage::Hypotheses));
        let second = Arc::new(
            FakeNode::new(Stage::Definition).then(NodeOutcome::NeedsInput {
                prompt: "how much?".into(),
            }),
        );
        let steps = [
            Step::new(first.clone() as _),
            Step::new(second.clone() as _),
        ];
        let mut ctx = context(Arc::new(ScriptedLlm::new([])));

        let outcome = run_steps(&mut ctx, &steps, "framing").await.unwrap();
        assert!(matches!(outcome, NodeOutcome::NeedsInput { .. }));
        assert_eq!(
            ctx.data::<Option<usize>>("framing.resume_at"),
            Some(Some(1))
        );

        let mut ctx = ctx.with_reply("ok");
        let outcome = run_steps(&mut ctx, &steps, "framing").await.unwrap();
        assert_eq!(outcome, NodeOutcome::Done);
        assert_eq!((first.runs(), second.runs()), (1, 2));
        assert_eq!(second.replies(), [None, Some("ok".into())]);
        assert_eq!(ctx.data::<Option<usize>>("framing.resume_at"), Some(None));
        assert_eq!(ctx.stage(), Stage::Framing);
    }

    #[tokio::test]
    async fn a_return_stops_the_steps() {
        let first = Arc::new(FakeNode::new(Stage::Brainstorm).then(NodeOutcome::Back {
            to: Stage::Research,
            reason: "no options".into(),
        }));
        let second = Arc::new(FakeNode::new(Stage::Modeling));
        let steps = [Step::new(first as _), Step::new(second.clone() as _)];
        let mut ctx = context(Arc::new(ScriptedLlm::new([])));
        let outcome = run_steps(&mut ctx, &steps, "solution").await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Research,
                ..
            }
        ));
        assert_eq!(second.runs(), 0);
    }

    #[tokio::test]
    async fn quick_depth_runs_the_marked_steps_with_the_main_model() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("a"),
            ScriptedResponse::text("b"),
        ]));
        let steps = [
            Step::main_when_quick(Arc::new(Asks(Stage::Hypotheses))),
            Step::new(Arc::new(Asks(Stage::Definition))),
        ];
        let mut ctx = context(llm.clone());
        ctx.set_data(DEPTH_KEY, Depth::Quick).unwrap();
        run_steps(&mut ctx, &steps, "framing").await.unwrap();
        let roles: Vec<ModelRole> = llm.requests().iter().map(|r| r.model.role).collect();
        assert_eq!(roles, [ModelRole::Main, ModelRole::Plan]);
    }
}
