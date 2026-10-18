//! [`ResearchGraphBuilder`]: assembles research nodes into a graph-flow
//! [`Graph`], one [`NodeTask`] per node, chained in workflow order.

use std::sync::Arc;
use std::time::Duration;

use domain::research::Stage;
use graph_flow::{Graph, GraphBuilder};

use super::error::FlowError;
use super::gate::task_id;
use super::node::{NodeDeps, ResearchNode};
use super::task::NodeTask;

/// Id of the research graph, stored in its sessions.
pub const GRAPH_ID: &str = "research";

/// Default limit for one node run (agent turns with tools can be long).
pub const DEFAULT_NODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A built research graph and its stages, in order.
#[derive(Clone)]
pub struct ResearchGraph {
    graph: Arc<Graph>,
    stages: Arc<[Stage]>,
}

impl std::fmt::Debug for ResearchGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchGraph")
            .field("stages", &self.stages)
            .finish_non_exhaustive()
    }
}

impl ResearchGraph {
    /// The graph-flow graph.
    #[must_use]
    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    /// Stages of the graph, in workflow order.
    #[must_use]
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// The first stage.
    #[must_use]
    pub fn first(&self) -> Stage {
        // `build` rejects empty graphs.
        self.stages.first().copied().unwrap_or(Stage::Intake)
    }

    /// The stage after `stage` in this graph.
    #[must_use]
    pub fn next_after(&self, stage: Stage) -> Option<Stage> {
        self.stages.iter().copied().find(|s| *s > stage)
    }

    /// Whether `stage` has a node in this graph.
    #[must_use]
    pub fn contains(&self, stage: Stage) -> bool {
        self.stages.contains(&stage)
    }
}

/// Builds a [`ResearchGraph`] from nodes given in workflow order.
///
/// ```ignore
/// let graph = ResearchGraphBuilder::new(deps)
///     .node(Intake::default())
///     .node(Observation::default())
///     .build()?;
/// ```
pub struct ResearchGraphBuilder {
    deps: NodeDeps,
    nodes: Vec<Arc<dyn ResearchNode>>,
    timeout: Duration,
}

impl std::fmt::Debug for ResearchGraphBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchGraphBuilder")
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl ResearchGraphBuilder {
    /// An empty builder; every node gets `deps`.
    #[must_use]
    pub fn new(deps: NodeDeps) -> Self {
        Self {
            deps,
            nodes: Vec::new(),
            timeout: DEFAULT_NODE_TIMEOUT,
        }
    }

    /// Appends a node.
    #[must_use]
    pub fn node(self, node: impl ResearchNode + 'static) -> Self {
        self.shared_node(Arc::new(node))
    }

    /// Appends a shared node (e.g. from `nodes::node_for`).
    #[must_use]
    pub fn shared_node(mut self, node: Arc<dyn ResearchNode>) -> Self {
        self.nodes.push(node);
        self
    }

    /// Limit for one node run.
    #[must_use]
    pub fn with_node_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Chains the nodes in the order given.
    ///
    /// # Errors
    ///
    /// [`FlowError::InvalidGraph`] when there are no nodes or their stages
    /// are not strictly in workflow order (no repeats).
    pub fn build(self) -> Result<ResearchGraph, FlowError> {
        let stages: Arc<[Stage]> = self.nodes.iter().map(|n| n.stage()).collect();
        if stages.is_empty() {
            return Err(FlowError::InvalidGraph("no nodes".to_owned()));
        }
        if let Some(pair) = stages.windows(2).find(|w| w[0] >= w[1]) {
            return Err(FlowError::InvalidGraph(format!(
                "{} must come before {}, without repeats",
                pair[1].name_es(),
                pair[0].name_es()
            )));
        }
        let mut builder = GraphBuilder::new(GRAPH_ID).with_task_timeout(self.timeout);
        for node in self.nodes {
            let task = NodeTask::new(node, self.deps.clone(), Arc::clone(&stages));
            builder = builder.add_task(Arc::new(task));
        }
        for pair in stages.windows(2) {
            builder = builder.add_edge(task_id(pair[0]), task_id(pair[1]));
        }
        let graph = builder.build()?;
        Ok(ResearchGraph {
            graph: Arc::new(graph),
            stages,
        })
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;
    use graph_flow::Context;

    use super::*;
    use crate::flows::research::fake::FakeNode;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    fn builder() -> ResearchGraphBuilder {
        ResearchGraphBuilder::new(NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        ))
    }

    #[test]
    fn chains_nodes_in_order() {
        let g = builder()
            .node(FakeNode::new(Stage::Intake))
            .node(FakeNode::new(Stage::Hypotheses))
            .node(FakeNode::new(Stage::Spec))
            .build()
            .unwrap();
        assert_eq!(g.stages(), [Stage::Intake, Stage::Hypotheses, Stage::Spec]);
        assert_eq!(g.first(), Stage::Intake);
        assert_eq!(g.next_after(Stage::Hypotheses), Some(Stage::Spec));
        assert_eq!(g.next_after(Stage::Spec), None);
        assert!(g.contains(Stage::Spec) && !g.contains(Stage::Plan));
        let graph = g.graph();
        assert_eq!(graph.start_task_id(), Some("research/intake"));
        let next = graph.find_next_task("research/intake", &Context::new());
        assert_eq!(next.as_deref(), Some("research/hypotheses"));
    }

    #[test]
    fn rejects_empty_and_unordered_graphs() {
        assert!(matches!(builder().build(), Err(FlowError::InvalidGraph(_))));
        let unordered = builder()
            .node(FakeNode::new(Stage::Spec))
            .node(FakeNode::new(Stage::Intake))
            .build();
        assert!(matches!(unordered, Err(FlowError::InvalidGraph(_))));
        let repeated = builder()
            .node(FakeNode::new(Stage::Intake))
            .node(FakeNode::new(Stage::Intake))
            .build();
        assert!(matches!(repeated, Err(FlowError::InvalidGraph(_))));
    }
}
