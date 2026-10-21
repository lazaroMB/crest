//! Fast 4, Solution: options, then the decision matrix.
//!
//! Runs [`Brainstorm`] (over the prior solutions Discovery stored) and
//! then [`Modeling`] inside the stage (see [`run_steps`]). At quick depth
//! the brainstorm uses the main model. A return Modeling asks for (e.g.
//! to the definition) goes to the fast stage that owns it (Framing). The
//! stage ends at a confirmation gate.

use std::sync::Arc;

use async_trait::async_trait;
use domain::research::Stage;

use super::{Step, run_steps};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::flows::research::nodes::brainstorm::Brainstorm;
use crate::flows::research::nodes::modeling::Modeling;

/// Prefix of the flow data of the stage (`solution.resume_at`).
pub const KEY: &str = "solution";

/// The Solution node.
pub struct Solution {
    steps: Vec<Step>,
}

impl std::fmt::Debug for Solution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Solution").finish_non_exhaustive()
    }
}

impl Default for Solution {
    fn default() -> Self {
        Self::new()
    }
}

impl Solution {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            steps: vec![
                Step::main_when_quick(Arc::new(Brainstorm::new())),
                Step::new(Arc::new(Modeling::new())),
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
impl ResearchNode for Solution {
    fn stage(&self) -> Stage {
        Stage::Solution
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        run_steps(ctx, &self.steps, KEY).await
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;
    use domain::research::Pipeline;
    use graph_flow::{InMemorySessionStorage, SessionStorage};

    use super::*;
    use crate::flows::research::builder::ResearchGraphBuilder;
    use crate::flows::research::fake::FakeNode;
    use crate::flows::research::flow::ResearchFlow;
    use crate::flows::research::gate::FlowStatus;
    use crate::flows::research::node::NodeDeps;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    #[tokio::test]
    async fn modeling_going_back_to_definition_goes_back_to_framing() {
        let framing = Arc::new(FakeNode::new(Stage::Framing));
        let modeling = Arc::new(FakeNode::new(Stage::Modeling).then(NodeOutcome::Back {
            to: Stage::Definition,
            reason: "the constraints are missing".into(),
        }));
        let solution = Solution::with_steps(vec![
            Step::main_when_quick(Arc::new(FakeNode::new(Stage::Brainstorm))),
            Step::new(modeling.clone() as _),
        ]);
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        );
        let graph = ResearchGraphBuilder::new(deps)
            .pipeline(Pipeline::Fast)
            .shared_node(framing.clone() as _)
            .node(solution)
            .build()
            .unwrap();
        let storage: Arc<dyn SessionStorage> = Arc::new(InMemorySessionStorage::new());
        let flow = ResearchFlow::start(graph, storage, "s", "p").await.unwrap();

        let status = flow.run().await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::WaitingForInput {
                stage: Stage::Framing,
                ..
            }
        ));
        // Approving Framing runs Solution, whose Modeling goes back.
        let status = flow.provide_input("yes").await.unwrap();
        assert!(matches!(
            status,
            FlowStatus::WaitingForInput {
                stage: Stage::Framing,
                ..
            }
        ));
        assert_eq!((framing.runs(), modeling.runs()), (2, 1));
        let journal = flow.state().await.unwrap().journal;
        assert!(
            journal
                .iter()
                .any(|e| e.text.contains("Back to Problem definition"))
        );
    }
}
