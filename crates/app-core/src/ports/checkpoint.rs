//! Saving a turn in progress.
//!
//! The session is saved when a turn ends. A tool that waits on the user (a
//! form) may still wait when the process ends: the agent then saves the
//! partial turn, so the next start finds the question and asks it again
//! ([`crate::usecases::run_turn::AgentService::resume_pending_tools`]).

use async_trait::async_trait;
use domain::conversation::{Session, Turn};

/// Saves a turn that has not ended.
#[async_trait]
pub trait TurnCheckpointPort: Send + Sync {
    /// Saves `session` with `turn` (unfinished, `stop_reason` unset)
    /// appended, as the session will be once the turn ends.
    async fn checkpoint(&self, session: &Session, turn: &Turn);
}
