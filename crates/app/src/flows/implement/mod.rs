//! The implementation flow: a second graph-flow graph that takes an
//! approved `plan.json` and runs it task by task.
//!
//! - [`load`]: reading, validating and ordering `plan.json`; the branch.
//! - [`state`]: the steps and what the session keeps
//!   ([`ImplementState`](state::ImplementState)).

pub mod load;
pub mod state;
