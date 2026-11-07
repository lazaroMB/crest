//! Saving and resuming sessions.
//!
//! A stored session is the conversation history plus everything needed to
//! redraw it: the timeline of blocks and the "allow always" rules granted
//! during it. The adapter is SQLite; the graph-flow checkpoints of the
//! investigative flow use their own storage trait, not this port.

use async_trait::async_trait;
use domain::block::Block;
use domain::conversation::{ProjectId, Session, SessionId};
use domain::permission::PermissionRule;
use domain::project::{Project, ProjectError};
use serde::{Deserialize, Serialize};

use super::projects::RecentWorkspace;

/// Everything persisted for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredSession {
    /// Conversation history, sent back to the model on resume.
    pub session: Session,
    /// Blocks of the timeline, in display order.
    pub timeline: Vec<Block>,
    /// Rules granted during the session. Only those with
    /// [`RuleScope::Session`](domain::permission::RuleScope::Session) are
    /// stored (adapters drop the others): project rules live in the
    /// workspace's permissions file.
    #[serde(default)]
    pub permission_rules: Vec<PermissionRule>,
    /// Lines sent from the prompt, oldest first, which `Up` brings back.
    #[serde(default)]
    pub prompt_history: Vec<String>,
}

impl StoredSession {
    /// A session with an empty timeline, no rules and no prompt history.
    #[must_use]
    pub fn new(session: Session) -> Self {
        Self {
            session,
            timeline: Vec::new(),
            permission_rules: Vec::new(),
            prompt_history: Vec::new(),
        }
    }

    /// Builds the listing entry for this session.
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.session.id,
            title: self.session.title.clone(),
            workspace_root: self.session.workspace_root.clone(),
            project_id: self.session.project_id,
            pinned: self.session.pinned,
            archived: self.session.archived,
            branched_from: self.session.branched_from,
            branched_at: self.session.branched_at,
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
    /// The project it is in; `None` for a chat without a project.
    pub project_id: Option<ProjectId>,
    /// Whether it is listed first in its section.
    pub pinned: bool,
    /// Whether it is archived: out of the normal listing.
    pub archived: bool,
    /// The chat it was branched from, while that chat exists.
    pub branched_from: Option<SessionId>,
    /// How many blocks the branch copied.
    pub branched_at: Option<usize>,
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
    /// No project with that id.
    #[error("project {0} not found")]
    ProjectNotFound(ProjectId),
    /// A change to a project that its rules refuse; the message is fit to
    /// show.
    #[error(transparent)]
    Project(#[from] ProjectError),
    /// The stored data could not be decoded (corrupt or from a newer version).
    #[error("stored session is corrupt: {0}")]
    Corrupt(String),
    /// The backend failed (I/O, locked database, ...).
    #[error("session store failed: {0}")]
    Backend(String),
}

/// Persists sessions by workspace.
#[async_trait]
pub trait SessionStorePort: ProjectStorePort + OrganizeStorePort {
    /// Inserts or replaces the session with the same id, updating its
    /// listing entry. Must be atomic: a crash leaves the old or new version.
    /// Takes the session by value: callers build it for the save, and an
    /// adapter that writes on another thread can move it there instead of
    /// copying the whole timeline again.
    async fn save(&self, session: StoredSession) -> Result<(), StoreError>;

    /// Loads the session `id`, or fails with [`StoreError::NotFound`].
    async fn load(&self, id: SessionId) -> Result<StoredSession, StoreError>;

    /// Loads the most recently updated session of the workspace `root`, if
    /// any; archived ones are skipped; used to resume on start-up.
    async fn latest_for_workspace(&self, root: &str) -> Result<Option<StoredSession>, StoreError>;

    /// Lists the sessions of the workspace `root`, most recent first, leaving
    /// out archived ones.
    async fn list(&self, root: &str) -> Result<Vec<SessionSummary>, StoreError>;

    /// Lists every session, archived ones included (flagged), most recent
    /// first: what the sidebar groups by project.
    async fn list_all(&self) -> Result<Vec<SessionSummary>, StoreError>;

    /// Deletes the session `id`, or fails with [`StoreError::NotFound`].
    async fn delete(&self, id: SessionId) -> Result<(), StoreError>;

    /// The workspaces with saved sessions, the most recently saved first,
    /// at most `limit`.
    async fn recent_workspaces(&self, limit: usize) -> Result<Vec<RecentWorkspace>, StoreError>;
}

/// Projects: named sets of directories that sessions can belong to. Every
/// session store is a project store, so one handle serves both.
#[async_trait]
pub trait ProjectStorePort: Send + Sync {
    /// Every project, in the order they were created.
    async fn projects(&self) -> Result<Vec<Project>, StoreError>;

    /// Creates a project with `roots` (normalized, repeats dropped).
    /// Refused without a name or without a directory.
    async fn create_project(&self, name: &str, roots: &[String]) -> Result<Project, StoreError>;

    /// Adds a directory to a project; one it already has changes nothing.
    async fn add_root(&self, id: ProjectId, root: &str) -> Result<Project, StoreError>;

    /// Removes a directory from a project; refused for the last one.
    async fn remove_root(&self, id: ProjectId, root: &str) -> Result<Project, StoreError>;

    /// Renames a project; refused for an empty name.
    async fn rename_project(&self, id: ProjectId, name: &str) -> Result<Project, StoreError>;

    /// Pins or unpins a project.
    async fn pin_project(&self, id: ProjectId, pinned: bool) -> Result<Project, StoreError>;

    /// Puts a session in a project, or out of any with `None`, keeping its
    /// id and its whole history. Sets the session's `workspace_root` to the
    /// project's first directory, or to empty without a project; saving the
    /// session later does not undo it. A project that loses its first
    /// directory moves its sessions to the next one the same way.
    async fn move_session(
        &self,
        id: SessionId,
        project: Option<ProjectId>,
    ) -> Result<(), StoreError>;
}

/// The sidebar menus' changes to chats and projects that the other ports do
/// not cover: names, pins, archive, branch and deleting a project. The
/// session store is also this, so one handle serves all three.
#[async_trait]
pub trait OrganizeStorePort: Send + Sync {
    /// Renames the chat; an empty title clears it. Does not change the
    /// order of the listing (`updated_at` stays).
    async fn rename_session(&self, id: SessionId, title: &str) -> Result<(), StoreError>;

    /// Pins or unpins a chat. A later `save` does not undo it.
    async fn pin_session(&self, id: SessionId, pinned: bool) -> Result<(), StoreError>;

    /// Archives or restores a chat. Touches neither its blocks nor its
    /// `updated_at`; a later `save` does not undo it.
    async fn set_session_archived(&self, id: SessionId, archived: bool) -> Result<(), StoreError>;

    /// Starts a new chat as a copy of `id` (see `Session::branch`): its
    /// timeline and prompt history are copied, `branched_from` and
    /// `branched_at` set. The source is only read; one transaction. Returns
    /// the new id.
    async fn branch_session(&self, id: SessionId) -> Result<SessionId, StoreError>;

    /// Archives or restores a project; its chats keep their own flag and
    /// stay in it, hidden together. Nothing is deleted.
    async fn set_project_archived(
        &self,
        id: ProjectId,
        archived: bool,
    ) -> Result<Project, StoreError>;

    /// Deletes a project but never its chats: they become standalone (no
    /// project, empty `workspace_root`), keeping their history and flags.
    /// Returns how many chats were set free.
    async fn delete_project(&self, id: ProjectId) -> Result<usize, StoreError>;
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
            prompt_history: Vec::new(),
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
            prompt_history: Vec::new(),
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
