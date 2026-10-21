//! The full research graph: the 14 nodes of the X-Workflow in order.
//!
//! [`research_graph`] takes the [`NodeDeps`] (model, events, agents) and
//! the workspace where the Spec folders go. Spec, Plan and the critical
//! review share one [`FsSpecSink`] over it, so the review reads and
//! annotates the folders the other two wrote, and the review checks the
//! cited files through the same file system. Without a workspace the three
//! write to one shared [`MemorySpecSink`] (tests).

use std::sync::Arc;

use domain::research::{Pipeline, Stage};

use super::builder::{ResearchGraph, ResearchGraphBuilder};
use super::error::FlowError;
use super::node::{NodeDeps, ResearchNode};
use super::nodes::node_for;
use super::nodes::plan::Planner;
use super::nodes::review::Review;
use super::nodes::spec::Spec;
use super::nodes::spec::sink::{FsSpecSink, MemorySpecSink, SpecSink};
use crate::ports::fs::FileSystemPort;
use crate::ports::search::FileSearchPort;

/// Where the research flow writes its Specs.
#[derive(Clone)]
pub struct ResearchWorkspace {
    /// The workspace files (Specs are written under `.crest/specs`).
    pub fs: Arc<dyn FileSystemPort>,
    /// Listing of the workspace, to number the Spec folders.
    pub search: Arc<dyn FileSearchPort>,
}

impl std::fmt::Debug for ResearchWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchWorkspace").finish_non_exhaustive()
    }
}

/// The research graphs of a workspace, one per pipeline.
///
/// Each stored session names its graph ([`ResearchGraph::id`]), so a
/// session is always resumed with the graph that started it. A pipeline
/// may be missing (e.g. a test or a build with only the deep graph).
#[derive(Debug, Clone, Default)]
pub struct ResearchGraphs {
    fast: Option<ResearchGraph>,
    deep: Option<ResearchGraph>,
}

impl ResearchGraphs {
    /// Both graphs.
    #[must_use]
    pub fn new(fast: ResearchGraph, deep: ResearchGraph) -> Self {
        Self::default().with(fast).with(deep)
    }

    /// Adds `graph` in the slot of its pipeline, replacing what was there.
    #[must_use]
    pub fn with(mut self, graph: ResearchGraph) -> Self {
        match graph.pipeline() {
            Pipeline::Fast => self.fast = Some(graph),
            Pipeline::Deep => self.deep = Some(graph),
        }
        self
    }

    /// The graph of `pipeline`, if built.
    #[must_use]
    pub fn get(&self, pipeline: Pipeline) -> Option<&ResearchGraph> {
        match pipeline {
            Pipeline::Fast => self.fast.as_ref(),
            Pipeline::Deep => self.deep.as_ref(),
        }
    }

    /// The graph whose id a stored session carries, if built.
    #[must_use]
    pub fn for_id(&self, graph_id: &str) -> Option<&ResearchGraph> {
        self.iter().find(|g| g.id() == graph_id)
    }

    /// The built graphs, fast first.
    pub fn iter(&self) -> impl Iterator<Item = &ResearchGraph> {
        self.fast.iter().chain(self.deep.iter())
    }
}

impl From<ResearchGraph> for ResearchGraphs {
    fn from(graph: ResearchGraph) -> Self {
        Self::default().with(graph)
    }
}

/// The 14 nodes of the research flow over `deps`, writing the Specs to
/// `workspace` (in memory when `None`).
///
/// # Errors
///
/// [`FlowError::InvalidGraph`] if a node is missing from the registry.
pub fn research_graph(
    deps: NodeDeps,
    workspace: Option<ResearchWorkspace>,
) -> Result<ResearchGraph, FlowError> {
    let sink: Arc<dyn SpecSink> = match &workspace {
        Some(ws) => Arc::new(FsSpecSink::new(Arc::clone(&ws.fs), Arc::clone(&ws.search))),
        None => Arc::new(MemorySpecSink::new()),
    };
    let mut builder = ResearchGraphBuilder::new(deps);
    for stage in Stage::ALL {
        let node: Arc<dyn ResearchNode> = match stage {
            Stage::Spec => Arc::new(Spec::with_sink(Arc::clone(&sink))),
            Stage::Plan => Arc::new(Planner::with_sink(Arc::clone(&sink))),
            Stage::CriticalReview => {
                let review = Review::new().with_sink(Arc::clone(&sink));
                Arc::new(match &workspace {
                    Some(ws) => review.with_workspace(Arc::clone(&ws.fs)),
                    None => review,
                })
            }
            other => node_for(other)
                .ok_or_else(|| FlowError::InvalidGraph(format!("{} has no node", other.name())))?,
        };
        builder = builder.shared_node(node);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPolicy;

    use super::*;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::RecordingSink;

    #[test]
    fn has_the_fourteen_stages_in_order() {
        let deps = NodeDeps::new(
            Arc::new(ScriptedLlm::new([])),
            ModelPolicy::default(),
            Arc::new(RecordingSink::new()),
        );
        let graph = research_graph(deps, None).unwrap();
        assert_eq!(graph.stages(), Stage::ALL);
        assert_eq!(graph.first(), Stage::Intake);
        let graphs = ResearchGraphs::from(graph);
        assert!(graphs.get(Pipeline::Fast).is_none());
        assert_eq!(
            graphs.for_id("research").unwrap().pipeline(),
            Pipeline::Deep
        );
        assert!(graphs.for_id("research-fast").is_none());
    }
}
