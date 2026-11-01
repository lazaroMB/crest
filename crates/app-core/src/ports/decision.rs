//! A decision model: answers typed questions about a state with
//! probabilities (see [`domain::decision`]). Cheaper and faster than a
//! model call, for judging, classifying and routing. Optional: every use
//! has a fallback for when there is none or it fails.

use async_trait::async_trait;
use domain::decision::{DecisionRequest, DecisionRequestError, DecisionResponse};

/// Why a decision failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    /// The request breaks the model's limits.
    #[error(transparent)]
    Invalid(#[from] DecisionRequestError),
    /// The credentials were rejected.
    #[error("the decision model rejected the credentials: {0}")]
    Auth(String),
    /// The provider answered with an error.
    #[error("the decision model failed with HTTP {status}: {message}")]
    Http {
        /// HTTP status.
        status: u16,
        /// What it said.
        message: String,
    },
    /// The provider could not be reached in time.
    #[error("the decision model did not answer: {0}")]
    Network(String),
    /// The answer could not be read.
    #[error("the decision model's answer could not be read: {0}")]
    Decode(String),
}

/// Answers decision requests.
#[async_trait]
pub trait DecisionPort: Send + Sync {
    /// The model id, for telemetry and prices.
    fn model(&self) -> &str;

    /// Answers every question of `request`.
    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError>;
}
