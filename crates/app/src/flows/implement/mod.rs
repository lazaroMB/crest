//! The implementation flow: a second graph-flow graph that takes an
//! approved `plan.json` and runs it task by task.
//!
//! - [`gate`]: the answers every gate accepts.
//! - [`load`]: reading, validating and ordering `plan.json`; the branch.
//! - [`node`]: the step contract ([`ImplementNode`](node::ImplementNode)).
//! - [`task`]: [`StepTask`](task::StepTask), the graph-flow task of a step.
//! - [`state`]: the steps and what the session keeps
//!   ([`ImplementState`](state::ImplementState)).

#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod gate;
pub mod load;
pub mod node;
pub mod state;
pub mod task;
