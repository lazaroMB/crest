//! The implementation flow: a second graph-flow graph that takes an
//! approved `plan.json` and runs it task by task.
//!
//! - [`state`]: the steps and what the session keeps
//!   ([`ImplementState`](state::ImplementState)).

pub mod state;
