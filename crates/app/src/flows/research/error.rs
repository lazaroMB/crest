//! Errors of the research flow runner.

use domain::research::Stage;
use graph_flow::GraphError;

/// Why a flow operation failed.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    /// The nodes given to the builder do not form a valid graph.
    #[error("invalid research graph: {0}")]
    InvalidGraph(String),
    /// No stored session has that id.
    #[error("research session {0} not found")]
    SessionNotFound(String),
    /// `start` was called with the id of an existing session.
    #[error("research session {0} already exists")]
    SessionExists(String),
    /// The stored session points at a task this graph does not have.
    #[error("research session is at {0:?}, which is not a node of this graph")]
    UnknownTask(String),
    /// Input was given while no node waits for it.
    #[error("the research flow is not waiting for input")]
    NotWaiting,
    /// The flow was aborted with `/abort`.
    #[error("the research flow was aborted")]
    Aborted,
    /// Input was given while the flow is stopped by a failed node.
    #[error("the research flow failed at {}: retry it before answering", .0.name())]
    Failed(Stage),
    /// The flow already finished.
    #[error("the research flow already finished")]
    Completed,
    /// `/back` to a stage that is not in the graph or not behind.
    #[error("cannot go back to {} ({}/{}): {reason}", .stage.name(), .stage.index(), Stage::total())]
    InvalidBack {
        /// Requested stage.
        stage: Stage,
        /// Why it is not allowed.
        reason: &'static str,
    },
    /// `/skip` on a stage that is not optional.
    #[error(
        "{} ({}/{}) is not optional; only {} can be skipped",
        .0.name(), .0.index(), Stage::total(), optional_list()
    )]
    NotOptional(Stage),
    /// A chat command could not be parsed.
    #[error(transparent)]
    Command(#[from] super::commands::CommandError),
    /// graph-flow or its storage failed (a node error included).
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// "Data (3) and Prototype (11)".
fn optional_list() -> String {
    super::node::OPTIONAL_STAGES
        .iter()
        .map(|s| format!("{} ({})", s.name(), s.index()))
        .collect::<Vec<_>>()
        .join(" and ")
}
