//! One file per research node, registered by stage.
//!
//! # Adding a node (T5.5–T5.9)
//!
//! 1. Write `nodes/<file>.rs` with a type that implements
//!    [`ResearchNode`] for its stage (see the example in
//!    [`crate::flows::research`]).
//! 2. Uncomment its `pub mod` line below.
//! 3. In [`node_for`], replace `None` in its arm with
//!    `Some(Arc::new(<file>::<Type>::new()))`.
//!
//! Every slot is pre-written and separated from the next by a blank line,
//! so tasks editing different slots rebase without conflicts. Do not
//! reorder or reformat other slots. Ports a node needs beyond
//! [`NodeDeps`](super::node::NodeDeps) are requested from the orchestrator
//! as a new `with_*` method there, not as constructor arguments here.

use std::sync::Arc;

use domain::research::Stage;

use super::node::ResearchNode;

// T5.5a: Intake
pub mod intake;

// T5.5b: Observation
pub mod observation;

// T5.5c: Data
pub mod data;

// T5.5d: Analysis
pub mod analysis;

// T5.5e: Hypotheses
pub mod hypotheses;

// T5.5f: Experiments
pub mod experiments;

// T5.6: Definition
pub mod define;

// T5.7a: Research
// pub mod research;

// T5.7b: Brainstorm
// pub mod brainstorm;

// T5.7c: Modeling
// pub mod modeling;

// T5.7d: Prototype
// pub mod prototype;

// T5.8: Spec
pub mod spec;

// T5.8: Plan
pub mod plan;

// T5.9: CriticalReview
// pub mod review;

/// The node registered for `stage`, if its task already landed.
#[must_use]
pub fn node_for(stage: Stage) -> Option<Arc<dyn ResearchNode>> {
    match stage {
        // T5.5a: Some(Arc::new(intake::<Type>::new()))
        Stage::Intake => Some(Arc::new(intake::Intake::new())),

        // T5.5b: Some(Arc::new(observation::<Type>::new()))
        Stage::Observation => Some(Arc::new(observation::Observation::new())),

        // T5.5c: Some(Arc::new(data::<Type>::new()))
        Stage::Data => Some(Arc::new(data::Data::new())),

        // T5.5d: Some(Arc::new(analysis::<Type>::new()))
        Stage::Analysis => Some(Arc::new(analysis::Analysis::new())),

        // T5.5e: Some(Arc::new(hypotheses::<Type>::new()))
        Stage::Hypotheses => Some(Arc::new(hypotheses::Hypotheses::new())),

        // T5.5f: Some(Arc::new(experiments::<Type>::new()))
        Stage::Experiments => Some(Arc::new(experiments::Experiments::new())),

        // T5.6: Some(Arc::new(define::<Type>::new()))
        Stage::Definition => Some(Arc::new(define::Definition::new())),

        // T5.7a: Some(Arc::new(research::<Type>::new()))
        Stage::Research => None,

        // T5.7b: Some(Arc::new(brainstorm::<Type>::new()))
        Stage::Brainstorm => None,

        // T5.7c: Some(Arc::new(modeling::<Type>::new()))
        Stage::Modeling => None,

        // T5.7d: Some(Arc::new(prototype::<Type>::new()))
        Stage::Prototype => None,

        // T5.8: Some(Arc::new(spec::<Type>::new()))
        Stage::Spec => Some(Arc::new(spec::Spec::new())),

        // T5.8: Some(Arc::new(plan::<Type>::new()))
        Stage::Plan => Some(Arc::new(plan::Planner::new())),

        // T5.9: Some(Arc::new(review::<Type>::new()))
        Stage::CriticalReview => None,
    }
}

/// Every registered node, in workflow order.
#[must_use]
pub fn registered() -> Vec<Arc<dyn ResearchNode>> {
    Stage::ALL.into_iter().filter_map(node_for).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_slot_holds_the_node_of_its_stage() {
        for stage in Stage::ALL {
            if let Some(node) = node_for(stage) {
                assert_eq!(node.stage(), stage);
            }
        }
        let stages: Vec<_> = registered().iter().map(|n| n.stage()).collect();
        assert!(stages.windows(2).all(|w| w[0] < w[1]));
    }
}
