//! The full research graph: the 14 nodes of the X-Workflow in order.
//!
//! [`research_graph`] takes the [`NodeDeps`] (model, events, agents) and
//! the workspace where the Spec folders go. Spec, Plan and the critical
//! review share one [`FsSpecSink`] over it, so the review reads and
//! annotates the folders the other two wrote, and the review checks the
//! cited files through the same file system. Without a workspace the three
//! write to one shared [`MemorySpecSink`] (tests).

use std::sync::Arc;

use domain::research::Stage;

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
    }
}
