//! [`RecordingCheckpoint`]: a [`TurnCheckpointPort`] that keeps every
//! session saved mid-turn, for tests of the resume after a restart.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use domain::conversation::{Session, Turn};

use crate::ports::checkpoint::TurnCheckpointPort;

/// Keeps the sessions as they were saved, oldest first.
#[derive(Debug, Default)]
pub struct RecordingCheckpoint {
    saved: Mutex<Vec<Session>>,
}

impl RecordingCheckpoint {
    /// The last session saved, with its unfinished turn appended.
    pub fn last(&self) -> Option<Session> {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .cloned()
    }

    /// How many times a turn was saved.
    pub fn count(&self) -> usize {
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[async_trait]
impl TurnCheckpointPort for RecordingCheckpoint {
    async fn checkpoint(&self, session: &Session, turn: &Turn) {
        let mut session = session.clone();
        session.turns.push(turn.clone());
        self.saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(session);
    }
}
