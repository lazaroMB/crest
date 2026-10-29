//! Errors of the discovery engine.

use domain::discovery::{Finding, Illegal, Section, Status};

use crate::ports::discovery::{DiscoveryId, DiscoveryLogError, SourceStoreError};
use crate::ports::llm::LlmError;
use crate::usecases::run_turn::TurnError;

/// Why the engine could not go on.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The log refused or failed.
    #[error(transparent)]
    Log(#[from] DiscoveryLogError),
    /// The source store failed.
    #[error(transparent)]
    Sources(#[from] SourceStoreError),
    /// The model failed.
    #[error(transparent)]
    Llm(#[from] LlmError),
    /// An agent turn failed.
    #[error(transparent)]
    Turn(#[from] TurnError),
    /// The model answered something the engine cannot use, twice.
    #[error("the model's answer could not be used: {0}")]
    Model(String),
    /// The writer could not meet the contract of `section` in three
    /// attempts; the last findings are kept.
    #[error("the writer could not meet the contract of {section} in three attempts")]
    Contract {
        /// The section.
        section: Section,
        /// The findings of the last attempt.
        findings: Vec<Finding>,
    },
    /// No discovery is active.
    #[error("no discovery is active; `/research <problem>` starts one")]
    NotActive,
    /// The discovery does not wait for an answer.
    #[error("the discovery does not wait for an answer (next step: {0})")]
    NotWaiting(String),
    /// The state rejected an event.
    #[error(transparent)]
    Illegal(#[from] Illegal),
    /// The log is frozen.
    #[error("the discovery is {0:?}; `/research <problem>` starts another")]
    Frozen(Status),
    /// A piece of the engine was not configured.
    #[error("{0} is not available in this build")]
    Unavailable(&'static str),
    /// Writing the document or the derived files failed.
    #[error("could not write the discovery files: {0}")]
    Files(String),
    /// A gate is closed and nothing can open it here.
    #[error("the gate of {section} is closed: {}", missing.join("; "))]
    GateClosed {
        /// The section.
        section: Section,
        /// What is missing.
        missing: Vec<String>,
    },
    /// An unknown discovery id.
    #[error("no discovery {0} in this workspace")]
    Unknown(DiscoveryId),
}
