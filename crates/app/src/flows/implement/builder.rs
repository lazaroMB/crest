//! [`ImplementGraphBuilder`]: assembles implementation steps into a
//! graph-flow [`Graph`], one [`StepTask`] per step, chained in step order.
//! Loops (the next task, a retry) are `GoTo`s of the steps themselves.

use std::sync::Arc;
use std::time::Duration;

use graph_flow::{Graph, GraphBuilder};

use super::flow::FlowError;
use super::node::{ImplementDeps, ImplementNode};
use super::state::Step;
use super::task::StepTask;

/// Id of the implementation graph, stored in its sessions.
pub const GRAPH_ID: &str = "implement";

/// Default limit for one step run (agent turns with tools can be long).
pub const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// A built implementation graph and its steps, in order.
#[derive(Clone)]
pub struct ImplementGraph {
    graph: Arc<Graph>,
    steps: Arc<[Step]>,
    deps: ImplementDeps,
}

impl std::fmt::Debug for ImplementGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImplementGraph")
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

impl ImplementGraph {
    /// The graph-flow graph.
    #[must_use]
    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    /// Steps of the graph, in order.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The first step.
    #[must_use]
    pub fn first(&self) -> Step {
        // `build` rejects empty graphs.
        self.steps.first().copied().unwrap_or(Step::Load)
    }

    /// Whether `step` has a node in this graph.
    #[must_use]
    pub fn contains(&self, step: Step) -> bool {
        self.steps.contains(&step)
    }

    pub(crate) fn deps(&self) -> &ImplementDeps {
        &self.deps
    }
}

/// Builds an [`ImplementGraph`] from steps given in order.
///
/// ```ignore
/// let graph = ImplementGraphBuilder::new(deps)
///     .node(LoadNode::new())
///     .node(ImplementTask::new())
///     .build()?;
/// ```
pub struct ImplementGraphBuilder {
    deps: ImplementDeps,
    nodes: Vec<Arc<dyn ImplementNode>>,
    timeout: Duration,
}

impl std::fmt::Debug for ImplementGraphBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImplementGraphBuilder")
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl ImplementGraphBuilder {
    /// An empty builder; every step gets `deps`.
    #[must_use]
    pub fn new(deps: ImplementDeps) -> Self {
        Self {
            deps,
            nodes: Vec::new(),
            timeout: DEFAULT_STEP_TIMEOUT,
        }
    }

    /// Appends a step.
    #[must_use]
    pub fn node(self, node: impl ImplementNode + 'static) -> Self {
        self.shared_node(Arc::new(node))
    }

    /// Appends a shared step.
    #[must_use]
    pub fn shared_node(mut self, node: Arc<dyn ImplementNode>) -> Self {
        self.nodes.push(node);
        self
    }

    /// Limit for one step run.
    #[must_use]
    pub fn with_step_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Chains the steps in the order given.
    ///
    /// # Errors
    ///
    /// [`FlowError::InvalidGraph`] when there are no steps or they are not
    /// strictly in step order (no repeats).
    pub fn build(self) -> Result<ImplementGraph, FlowError> {
        let steps: Arc<[Step]> = self.nodes.iter().map(|n| n.step()).collect();
        if steps.is_empty() {
            return Err(FlowError::InvalidGraph("no steps".to_owned()));
        }
        if let Some(pair) = steps.windows(2).find(|w| w[0] >= w[1]) {
            return Err(FlowError::InvalidGraph(format!(
                "{} must come before {}, without repeats",
                pair[1].name(),
                pair[0].name()
            )));
        }
        let mut builder = GraphBuilder::new(GRAPH_ID).with_task_timeout(self.timeout);
        for node in self.nodes {
            let task = StepTask::new(node, self.deps.clone(), Arc::clone(&steps));
            builder = builder.add_task(Arc::new(task));
        }
        for pair in steps.windows(2) {
            builder = builder.add_edge(pair[0].task_id(), pair[1].task_id());
        }
        let graph = builder.build()?;
        Ok(ImplementGraph {
            graph: Arc::new(graph),
            steps,
            deps: self.deps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flows::implement::fake::FakeStep;
    use crate::flows::implement::node::tests::deps;
    use crate::testing::system::RecordingSink;

    fn builder() -> ImplementGraphBuilder {
        ImplementGraphBuilder::new(deps(&Arc::new(RecordingSink::new())))
    }

    #[test]
    fn chains_steps_in_order() {
        let g = builder()
            .node(FakeStep::new(Step::Load))
            .node(FakeStep::new(Step::Implement))
            .node(FakeStep::new(Step::Report))
            .build()
            .unwrap();
        assert_eq!(g.steps(), [Step::Load, Step::Implement, Step::Report]);
        assert_eq!(g.first(), Step::Load);
        assert!(g.contains(Step::Report) && !g.contains(Step::Test));
        assert_eq!(g.graph().start_task_id(), Some("implement/load"));
    }

    #[test]
    fn rejects_empty_and_unordered_graphs() {
        assert!(matches!(builder().build(), Err(FlowError::InvalidGraph(_))));
        let unordered = builder()
            .node(FakeStep::new(Step::Test))
            .node(FakeStep::new(Step::Load))
            .build();
        assert!(matches!(unordered, Err(FlowError::InvalidGraph(_))));
    }
}
