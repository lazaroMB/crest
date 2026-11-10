//! The session and flow stores, and the checkpoint that saves a turn waiting
//! on the user.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use adapter_store::{SqliteFlowStorage, SqliteSessionStore};
use application::ports::checkpoint::TurnCheckpointPort;
use application::ports::events::DomainEvent;
use application::ports::store::{SessionStorePort, StoreError, StoredSession};
use application::usecases::approval::lock_permissions;
use application::usecases::resume::{
    restore_permission_rules, resume_by_id, resume_latest, settle_orphan_forms,
};
use application::usecases::run_turn::SharedPermissions;
use domain::conversation::{Session, SessionId};
use domain::workspace::WorkspaceRoot;
use ui::timeline::TimelineState;

use super::{Config, SharedPromptHistory, now_ms, session_title};

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

/// The session [`resume_session`] settled on.
pub(super) struct Resumed {
    pub(super) session: Session,
    /// A saved session was opened, not a new one made.
    pub(super) resumed: bool,
    /// Says that the latest session was resumed, and how to start anew.
    pub(super) notice: Option<String>,
    pub(super) prompt_history: Vec<String>,
}

/// The session to continue: the one `--session` names, else the latest of
/// the workspace with `--resume`, else a new one. A resumed session brings
/// back its timeline (into `timeline`), its permission rules and its
/// prompt history; what fails becomes a warning.
pub(super) async fn resume_session(
    store: Option<&Arc<dyn SessionStorePort>>,
    config: &Config,
    root: &WorkspaceRoot,
    timeline: &Mutex<TimelineState>,
    permissions: &SharedPermissions,
    warnings: &mut Vec<String>,
) -> Resumed {
    let directory = if config.is_standalone() {
        String::new()
    } else {
        root.as_path().display().to_string()
    };
    let mut opened = Resumed {
        session: Session::new(directory, now_ms()),
        resumed: false,
        notice: None,
        prompt_history: Vec::new(),
    };
    let Some(store) = store else {
        return opened;
    };
    let workspace = opened.session.workspace_root.clone();
    match find(store.as_ref(), config, &workspace).await {
        Ok(Some(mut stored)) => {
            bring_back(&mut stored, timeline, permissions, warnings);
            if chosen(config).is_none() {
                opened.notice = Some(format!(
                    "previous session resumed ({} turns); `crest --new` starts a new one",
                    stored.session.turns.len()
                ));
            }
            opened.resumed = true;
            opened.session = stored.session;
            opened.prompt_history = stored.prompt_history;
        }
        Ok(None) => {}
        Err(warning) => warnings.push(warning),
    }
    opened
}

/// The id `--session` names, if it is one.
fn chosen(config: &Config) -> Option<SessionId> {
    config.session_id.as_deref().and_then(SessionId::parse)
}

/// The session to continue, if any: the chosen one, else the latest of the
/// workspace when resuming.
async fn find(
    store: &dyn SessionStorePort,
    config: &Config,
    workspace: &str,
) -> Result<Option<StoredSession>, String> {
    if let Some(id) = chosen(config) {
        // A session that cannot be opened is a warning: a new one opens.
        let chosen = resume_by_id(store, id, workspace).await;
        return chosen
            .map(Some)
            .map_err(|err| format!("could not open the chosen session: {err}"));
    }
    if config.resume {
        let latest = resume_latest(store, workspace).await;
        return latest.map_err(|err| format!("could not resume the last session: {err}"));
    }
    Ok(None)
}

/// Puts a resumed session back: forms of calls that are over close (one
/// whose call still waits is asked again once the session opens), its rules
/// return and its blocks fill `timeline`.
fn bring_back(
    stored: &mut StoredSession,
    timeline: &Mutex<TimelineState>,
    permissions: &SharedPermissions,
    warnings: &mut Vec<String>,
) {
    if settle_orphan_forms(stored) > 0 {
        warnings.push("a form left unanswered by an earlier run was closed".into());
    }
    restore_permission_rules(permissions, &stored.permission_rules);
    let mut state = timeline.lock().unwrap_or_else(PoisonError::into_inner);
    for block in std::mem::take(&mut stored.timeline) {
        state.apply(&DomainEvent::BlockAdded { block });
    }
}
