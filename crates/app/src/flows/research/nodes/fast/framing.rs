//! Fast 3, Framing: the hypotheses, then the measurable problem
//! statements and their constraints.
//!
//! Runs [`Hypotheses`] and then [`Definition`] inside the stage (see
//! [`run_steps`]). When Definition asks for a measurement, the answer
//! resumes at Definition without redoing the hypotheses. At quick depth
//! the hypotheses use the main model. The stage ends at a confirmation
//! gate: corrections rerun both steps with them.

use std::sync::Arc;

use async_trait::async_trait;
use domain::research::Stage;

use super::{Step, run_steps};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::flows::research::nodes::define::Definition;
use crate::flows::research::nodes::hypotheses::Hypotheses;

/// Prefix of the flow data of the stage (`framing.resume_at`).
pub const KEY: &str = "framing";

/// The Framing node.
pub struct Framing {
    steps: Vec<Step>,
}

impl std::fmt::Debug for Framing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Framing").finish_non_exhaustive()
    }
}

impl Default for Framing {
    fn default() -> Self {
        Self::new()
    }
}

impl Framing {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            steps: vec![
                Step::main_when_quick(Arc::new(Hypotheses::new())),
                Step::new(Arc::new(Definition::new())),
            ],
        }
    }

    /// The stage over other nodes, for tests.
    #[cfg(test)]
    pub(crate) fn with_steps(steps: Vec<Step>) -> Self {
        Self { steps }
    }
}

#[async_trait]
impl ResearchNode for Framing {
    fn stage(&self) -> Stage {
        Stage::Framing
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        run_steps(ctx, &self.steps, KEY).await
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;
    use domain::research::ResearchState;

    use super::*;
    use crate::flows::research::fake::FakeNode;
    use crate::flows::research::node::NodeDeps;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    #[tokio::test]
    async fn ok_after_a_question_of_definition_reruns_only_definition() {
        let hypotheses = Arc::new(FakeNode::new(Stage::Hypotheses));
        let definition = Arc::new(
            FakeNode::new(Stage::Definition).then(NodeOutcome::NeedsInput {
                prompt: "How slow is it?".into(),
            }),
        );
        let framing = Framing::with_steps(vec![
            Step::main_when_quick(hypotheses.clone() as _),
            Step::new(definition.clone() as _),
        ]);
        assert!(framing.has_gate());
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        );
        let mut ctx = NodeContext::new(Stage::Framing, deps, "p", ResearchState::default());

        let outcome = framing.run(&mut ctx).await.unwrap();
        assert!(matches!(outcome, NodeOutcome::NeedsInput { .. }));
        let mut ctx = ctx.with_reply("ok");
        assert_eq!(framing.run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert_eq!((hypotheses.runs(), definition.runs()), (1, 2));

        // Corrections at the gate redo both steps.
        let mut ctx = ctx.with_reply("add a hypothesis about the cache");
        framing.run(&mut ctx).await.unwrap();
        assert_eq!((hypotheses.runs(), definition.runs()), (2, 3));
        let stages: Vec<Stage> = ctx.state().journal.iter().map(|e| e.stage).collect();
        assert!(stages.contains(&Stage::Hypotheses) && stages.contains(&Stage::Definition));
    }
}
