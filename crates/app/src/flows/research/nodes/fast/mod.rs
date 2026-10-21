//! The five stages of the fast pipeline, registered by stage.
//!
//! Each one reuses the nodes of the deep pipeline: Triage runs Intake,
//! Discovery runs the gathering half of Observation, Data and Research at
//! once, Framing runs Hypotheses and Definition, Solution runs Brainstorm
//! and Modeling, and Delivery writes, plans and reviews each Spec at once.
//! Model calls run concurrently with `futures` combinators, never
//! `tokio::spawn`, so cancelling the flow cancels them all; blocks are
//! emitted afterwards, in a fixed order.

use std::sync::Arc;

use domain::research::Stage;

use super::super::node::ResearchNode;

pub mod discovery;
pub mod triage;

/// The fast node registered for `stage`, if it is a fast stage that
/// needs no ports beyond [`NodeDeps`](super::super::node::NodeDeps).
#[must_use]
pub fn fast_node_for(stage: Stage) -> Option<Arc<dyn ResearchNode>> {
    match stage {
        Stage::Triage => Some(Arc::new(triage::Triage::new())),
        Stage::Discovery => Some(Arc::new(discovery::Discovery::new())),
        _ => None,
    }
}
