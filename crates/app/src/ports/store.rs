//! Saving and resuming sessions.
//!
//! A stored session is the conversation history plus everything needed to
//! redraw it: the timeline of blocks and the "allow always" rules granted
//! during it. The adapter is SQLite; the graph-flow checkpoints of the
//! investigative flow use their own storage trait, not this port.

use async_trait::async_trait;
use domain::block::Block;
use domain::conversation::{Session, SessionId};
use domain::permission::PermissionRule;
use serde::{Deserialize, Serialize};

/// Everything persisted for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredSession {
    /// Conversation history, sent back to the model on resume.
    pub session: Session,
    /// Blocks of the timeline, in display order.
    pub timeline: Vec<Block>,
    /// "Allow always" rules granted in the workspace.
    #[serde(default)]
    pub permission_rules: Vec<PermissionRule>,
}

impl StoredSession {
    /// Builds the listing entry for this session.
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.session.id,
            title: self.session.title.clone(),
            workspace_root: self.session.workspace_root.clone(),
            updated_at: self.session.updated_at,
            turns: self.session.turns.len(),
        }
    }
}

/// Listing entry of a stored session, without its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Id of the session.
    pub id: SessionId,
    /// Title, once generated.
    pub title: Option<String>,
    /// Workspace the session belongs to.
    pub workspace_root: String,
    /// Last update, in milliseconds since the Unix epoch.
    pub updated_at: u64,
    /// Number of turns.
    pub turns: usize,
}

/// Why a store operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No session with that id.
    #[error("session {0} not found")]
    NotFound(SessionId),
    /// The stored data could not be decoded (corrupt or from a newer version).
    #[error("stored session is corrupt: {0}")]
    Corrupt(String),
    /// The backend failed (I/O, locked database, ...).
    #[error("session store failed: {0}")]
    Backend(String),
}

/// Persists sessions by workspace.
#[async_trait]
pub trait SessionStorePort: Send + Sync {
    /// Inserts or replaces the session with the same id, updating its
    /// listing entry. Must be atomic: a crash leaves the old or new version.
    async fn save(&self, session: &StoredSession) -> Result<(), StoreError>;

    /// Loads the session `id`, or fails with [`StoreError::NotFound`].
    async fn load(&self, id: SessionId) -> Result<StoredSession, StoreError>;

    /// Loads the most recently updated session of the workspace `root`, if
    /// any; used to resume on start-up.
    async fn latest_for_workspace(&self, root: &str) -> Result<Option<StoredSession>, StoreError>;

    /// Lists the sessions of the workspace `root`, most recent first.
    async fn list(&self, root: &str) -> Result<Vec<SessionSummary>, StoreError>;

    /// Deletes the session `id`, or fails with [`StoreError::NotFound`].
    async fn delete(&self, id: SessionId) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use domain::block::BlockKind;

    use super::*;

    #[test]
    fn stored_session_round_trips_through_json() {
        let stored = StoredSession {
            session: Session::new("/tmp/ws", 10),
            timeline: vec![Block::new(BlockKind::Text {
                markdown: "hola".into(),
            })],
            permission_rules: Vec::new(),
        };
        let json = serde_json::to_string(&stored).unwrap();
        let back: StoredSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stored);
    }

    #[test]
    fn summary_reflects_the_session() {
        let stored = StoredSession {
            session: Session::new("/tmp/ws", 10),
            timeline: Vec::new(),
            permission_rules: Vec::new(),
        };
        let summary = stored.summary();
        assert_eq!(summary.id, stored.session.id);
        assert_eq!(summary.workspace_root, "/tmp/ws");
        assert_eq!(summary.updated_at, 10);
        assert_eq!(summary.turns, 0);
    }

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn SessionStorePort>>) {}
        assert_dyn(None);
    }
}
