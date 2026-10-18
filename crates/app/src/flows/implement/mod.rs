//! The implementation flow: a second graph-flow graph that takes an
//! approved `plan.json` and runs it task by task.
//!
//! - [`builder`]: chains steps into an [`ImplementGraph`](builder::ImplementGraph).
//! - [`flow`]: [`ImplementFlow`](flow::ImplementFlow), one session of the graph.
//! - [`gate`]: the answers every gate accepts.
//! - [`load`]: reading, validating and ordering `plan.json`; the branch.
//! - [`node`]: the step contract ([`ImplementNode`](node::ImplementNode)).
//! - [`task`]: [`StepTask`](task::StepTask), the graph-flow task of a step.
//! - [`nodes`]: the steps after Cargar plan, one file each.
//! - [`scope`]: file tools restricted to the files of a task.
//! - [`state`]: the steps and what the session keeps
//!   ([`ImplementState`](state::ImplementState)).

pub mod builder;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod flow;
pub mod gate;
pub mod load;
pub mod node;
pub mod nodes;
pub mod scope;
pub mod state;
pub mod task;
