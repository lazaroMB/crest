//! Opening the gates of a deep discovery: producing the records the state
//! asks for before `Findings` and before `Solution proposal`.

use domain::discovery::Section;

use super::engine::{DiscoveryEngine, Loaded, StepOutcome};
use super::error::DiscoveryError;
use crate::ports::discovery::DiscoveryId;

/// Tries to produce what `missing` lists for `section`; reports the gate
/// closed when it cannot.
pub(crate) async fn open_gate(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    loaded: &Loaded,
    section: Section,
    missing: Vec<String>,
) -> Result<StepOutcome, DiscoveryError> {
    match section {
        Section::Findings | Section::CurrentSystem => {
            super::explore::open_problem_gate(engine, id, loaded, section, missing).await
        }
        Section::SolutionProposal => {
            super::spikes::open_solution_gate(engine, id, loaded, section).await
        }
        _ => Ok(StepOutcome::GateClosed {
            section,
            missing,
            produced: Vec::new(),
        }),
    }
}
