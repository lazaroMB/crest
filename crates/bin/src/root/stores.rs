//! The session and flow stores, and the checkpoint that saves a turn waiting
//! on the user.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use adapter_store::{SqliteFlowStorage, SqliteSessionStore};
use application::ports::checkpoint::TurnCheckpointPort;
use application::ports::store::{SessionStorePort, StoreError, StoredSession};
use application::usecases::approval::lock_permissions;
use application::usecases::run_turn::SharedPermissions;
use domain::conversation::Session;
use ui::timeline::TimelineState;

use super::{SharedPromptHistory, session_title};

/// Saves the turn in progress when a tool waits on the user (a form), so a
/// restart finds the question and asks it again. The session is written
/// as the end of the turn would write it, with that turn unfinished.
pub(super) struct Checkpoint {
    pub(super) store: Arc<dyn SessionStorePort>,
    pub(super) timeline: Arc<Mutex<TimelineState>>,
    pub(super) permissions: SharedPermissions,
    pub(super) prompt_history: SharedPromptHistory,
}

#[async_trait::async_trait]
impl TurnCheckpointPort for Checkpoint {
    async fn checkpoint(&self, session: &Session, turn: &domain::conversation::Turn) {
        let prompt_history = self
            .prompt_history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut session = session.clone();
        session.title = session_title(session.title.as_deref(), &prompt_history);
        session.turns.push(turn.clone());
        let timeline = self
            .timeline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries()
            .iter()
            .map(|entry| entry.block.clone())
            .collect();
        let stored = StoredSession {
            session,
            timeline,
            permission_rules: lock_permissions(&self.permissions).rules.clone(),
            prompt_history,
        };
        if let Err(err) = self.store.save(stored).await {
            eprintln!("warning: could not save the turn in progress: {err}");
        }
    }
}

pub(super) fn open_store(path: Option<&Path>) -> Result<SqliteSessionStore, StoreError> {
    match path {
        Some(path) => SqliteSessionStore::open(path),
        None => SqliteSessionStore::open_default(),
    }
}

pub(super) fn open_flow_storage(path: Option<&Path>) -> Result<SqliteFlowStorage, StoreError> {
    match path {
        Some(path) => SqliteFlowStorage::open(path),
        None => SqliteFlowStorage::open_default(),
    }
}
