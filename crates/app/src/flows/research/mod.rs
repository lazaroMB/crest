//! Frame of the research flow (X-Workflow): the node contract, its
//! graph-flow adapter, gates in the chat and conditional returns.

#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod gate;
pub mod node;
