//! Ports of the discovery engine: the event log of a discovery with the
//! texts it produced, and the store of the sources it read.
//!
//! The log is append-only and judged by the domain: [`judge`] replays the
//! stored events, asks [`DiscoveryState::check`] whether the new event is
//! legal now, and says what to store. Every adapter calls it, so no event
//! the state rejects is ever written ([`Appended::Refused`] stores a
//! `Refused` event instead while the discovery is open; a frozen log takes
//! nothing).

use std::fmt;

use async_trait::async_trait;
use domain::discovery::{
    DiscoveryEvent, DiscoveryState, Illegal, Level, Logged, ReplayError, Section, SourceId,
    SourceKind, Status, replay,
};
use serde::{Deserialize, Serialize};

/// Id of a discovery: the name of its folder under `.crest/specs/`
/// (`0007-session-cache`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiscoveryId(String);

impl DiscoveryId {
    /// Wraps a folder name.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The folder name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DiscoveryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A stored discovery, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySummary {
    /// Its id.
    pub id: DiscoveryId,
    /// The workspace it belongs to.
    pub workspace_root: String,
    /// The user's request.
    pub problem: String,
    /// The level.
    pub level: Level,
    /// Open, accepted or aborted.
    pub status: Status,
    /// The open section, while open.
    pub open: Option<Section>,
    /// Events in the log.
    pub events: u64,
    /// When it started, Unix epoch milliseconds.
    pub started_at: u64,
    /// The time of the last event.
    pub updated_at: u64,
}

impl DiscoverySummary {
    /// The summary of `id` from its replayed events.
    ///
    /// # Errors
    ///
    /// The replay error of a corrupt log.
    pub fn from_events(
        id: DiscoveryId,
        workspace_root: &str,
        events: &[Logged],
    ) -> Result<Self, ReplayError> {
        let state = replay(events)?;
        let started_at = events.first().map_or(0, |e| e.at);
        let updated_at = events.last().map_or(0, |e| e.at);
        Ok(Self {
            id,
            workspace_root: workspace_root.to_owned(),
            problem: state.problem.clone(),
            level: state.level,
            status: state.status,
            open: state.open(),
            events: u64::try_from(events.len()).unwrap_or(u64::MAX),
            started_at,
            updated_at,
        })
    }
}

/// Why a log operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryLogError {
    /// No discovery with that id.
    #[error("discovery {0} not found")]
    NotFound(DiscoveryId),
    /// `start` on an id that already has a log.
    #[error("discovery {0} already exists")]
    AlreadyExists(DiscoveryId),
    /// The state rejected the event. `logged` says whether a `Refused`
    /// event was appended (while open) or nothing was (frozen).
    #[error("{id}: {reason}")]
    Illegal {
        /// The discovery.
        id: DiscoveryId,
        /// Why.
        reason: Illegal,
        /// Whether the refusal is in the log.
        logged: bool,
    },
    /// The stored log does not replay (corrupt, or from a newer version).
    #[error("the log of {0} is corrupt: {1}")]
    Corrupt(DiscoveryId, String),
    /// The backend failed (I/O, locked database, ...).
    #[error("discovery store failed: {0}")]
    Backend(String),
}

/// What [`judge`] says to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Appended {
    /// The event is legal: store it.
    Accepted(Logged),
    /// The event is illegal and the discovery is open: store `refusal`
    /// instead and report `reason`.
    Refused {
        /// The `Refused` event to store.
        refusal: Logged,
        /// Why the event was illegal.
        reason: Illegal,
    },
    /// The event is illegal and the log is frozen: store nothing.
    Frozen(Illegal),
}

/// Judges `event` against the replayed `events`, as every adapter must
/// before writing. `at` is the time of the new event.
///
/// # Errors
///
/// The replay error of a corrupt log.
pub fn judge(events: &[Logged], event: DiscoveryEvent, at: u64) -> Result<Appended, ReplayError> {
    let state: DiscoveryState = replay(events)?;
    let seq = state.last_seq + 1;
    match state.check(&event) {
        Ok(()) => Ok(Appended::Accepted(Logged::new(seq, at, event))),
        Err(reason) if state.status == Status::Open => Ok(Appended::Refused {
            refusal: Logged::new(
                seq,
                at,
                DiscoveryEvent::Refused {
                    what: describe(&event),
                    why: reason.to_string(),
                },
            ),
            reason,
        }),
        Err(reason) => Ok(Appended::Frozen(reason)),
    }
}

/// A short name of `event` for the `Refused` trail.
#[must_use]
pub fn describe(event: &DiscoveryEvent) -> String {
    match event {
        DiscoveryEvent::Started { level, .. } => format!("start ({level:?})"),
        DiscoveryEvent::SourceFetched { source, .. } => format!("fetch {source}"),
        DiscoveryEvent::SectionWritten { section, .. } => format!("write {section}"),
        DiscoveryEvent::Audited { section, .. } => format!("audit {section}"),
        DiscoveryEvent::Verdict {
            section,
            choice,
            by,
            ..
        } => format!("verdict {choice:?} on {section} by {by}"),
        DiscoveryEvent::RecordChecked {
            record, verdict, ..
        } => format!("check {record} -> {verdict:?}"),
        DiscoveryEvent::SpikeStarted { spike, .. } => format!("start spike {spike}"),
        DiscoveryEvent::SpikeScored { spike, score } => format!("score spike {spike} -> {score:?}"),
        DiscoveryEvent::Reopened { section, .. } => format!("reopen {section}"),
        DiscoveryEvent::Accepted { by } => format!("accept by {by}"),
        DiscoveryEvent::Aborted => "abort".to_owned(),
        DiscoveryEvent::Refused { what, .. } => format!("refuse {what}"),
    }
}

/// The append-only log of each discovery of a workspace, with the texts
/// it produced (sections, records, spikes, drafts) as named artifacts.
#[async_trait]
pub trait DiscoveryLogPort: Send + Sync {
    /// Creates the log of `id` with its `Started` event.
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::AlreadyExists`], or the backend's.
    async fn start(
        &self,
        id: &DiscoveryId,
        workspace_root: &str,
        problem: &str,
        level: Level,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError>;

    /// Appends `event` if the replayed state accepts it (see [`judge`]).
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::Illegal`] when the state rejects it (a
    /// `Refused` event is stored while the discovery is open),
    /// [`DiscoveryLogError::NotFound`], or the backend's.
    async fn append(
        &self,
        id: &DiscoveryId,
        event: DiscoveryEvent,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError>;

    /// Every event of `id`, in sequence order.
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::NotFound`], or the backend's.
    async fn load(&self, id: &DiscoveryId) -> Result<Vec<Logged>, DiscoveryLogError>;

    /// The discoveries of `workspace_root`, newest first. A log that does
    /// not replay is left out, so one corrupt discovery does not block the
    /// workspace; [`DiscoveryLogPort::load`] reports it as
    /// [`DiscoveryLogError::Corrupt`].
    ///
    /// # Errors
    ///
    /// The backend's.
    async fn list(&self, workspace_root: &str) -> Result<Vec<DiscoverySummary>, DiscoveryLogError>;

    /// Stores (or replaces) the artifact `name` of `id`, e.g.
    /// `sections/goal.md`, `records/claims.json`, `spikes/SP1.md`.
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::NotFound`], or the backend's.
    async fn put_artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
        text: &str,
    ) -> Result<(), DiscoveryLogError>;

    /// The artifact `name` of `id`, if stored.
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::NotFound`], or the backend's.
    async fn artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
    ) -> Result<Option<String>, DiscoveryLogError>;

    /// The names of the artifacts of `id`, sorted.
    ///
    /// # Errors
    ///
    /// [`DiscoveryLogError::NotFound`], or the backend's.
    async fn artifacts(&self, id: &DiscoveryId) -> Result<Vec<String>, DiscoveryLogError>;
}

/// A source's text as it is stored: the explorer read it from the
/// repository or the web and the claims' spans are checked against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceText {
    /// Final URL or workspace path.
    pub location: String,
    /// Title, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What kind of source it is.
    #[serde(default)]
    pub kind: SourceKind,
    /// The text that was read.
    pub text: String,
    /// When it was read, Unix epoch milliseconds.
    pub fetched_at: u64,
}

/// A stored source with its id and hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSource {
    /// Its id within the discovery.
    pub id: SourceId,
    /// BLAKE3 of the text, hex.
    pub hash: String,
    /// The source.
    #[serde(flatten)]
    pub source: SourceText,
}

/// Why a source store operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceStoreError {
    /// No such source in that discovery.
    #[error("source {1} of discovery {0} not found")]
    NotFound(DiscoveryId, SourceId),
    /// The backend failed.
    #[error("source store failed: {0}")]
    Backend(String),
}

/// BLAKE3 of `text`, hex: the hash the log records for a source.
#[must_use]
pub fn text_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// Keeps the text of every source a discovery read.
#[async_trait]
pub trait SourceStorePort: Send + Sync {
    /// Stores `source` under the next id of `discovery` and returns it
    /// with its hash.
    ///
    /// # Errors
    ///
    /// The backend's.
    async fn put(
        &self,
        discovery: &DiscoveryId,
        source: SourceText,
    ) -> Result<StoredSource, SourceStoreError>;

    /// The source `id` of `discovery`.
    ///
    /// # Errors
    ///
    /// [`SourceStoreError::NotFound`], or the backend's.
    async fn get(
        &self,
        discovery: &DiscoveryId,
        id: SourceId,
    ) -> Result<StoredSource, SourceStoreError>;

    /// Every source of `discovery`, by id.
    ///
    /// # Errors
    ///
    /// The backend's.
    async fn sources(&self, discovery: &DiscoveryId)
    -> Result<Vec<StoredSource>, SourceStoreError>;
}

#[cfg(test)]
mod tests {
    use domain::discovery::{Actor, Verdict};

    use super::*;

    fn started() -> Vec<Logged> {
        vec![Logged::new(
            1,
            1,
            DiscoveryEvent::Started {
                problem: "p".into(),
                level: Level::Research,
            },
        )]
    }

    #[test]
    fn a_legal_event_gets_the_next_seq() {
        let events = started();
        let event = DiscoveryEvent::SectionWritten {
            section: Section::Goal,
            hash: "h".into(),
        };
        match judge(&events, event.clone(), 5).unwrap() {
            Appended::Accepted(logged) => {
                assert_eq!((logged.seq, logged.at), (2, 5));
                assert_eq!(logged.event, event);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_illegal_event_while_open_becomes_a_refusal() {
        let events = started();
        let event = DiscoveryEvent::Verdict {
            section: Section::Goal,
            choice: Verdict::ApproveAsIs,
            by: Actor::Human("alex".into()),
            note: None,
        };
        match judge(&events, event, 5).unwrap() {
            Appended::Refused { refusal, reason } => {
                assert_eq!(refusal.seq, 2);
                assert!(matches!(reason, Illegal::Missing { .. }));
                assert!(matches!(
                    refusal.event,
                    DiscoveryEvent::Refused { ref what, .. } if what.starts_with("verdict ApproveAsIs on Goal")
                ));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_frozen_log_stores_nothing() {
        let mut events = started();
        events.push(Logged::new(2, 2, DiscoveryEvent::Aborted));
        let event = DiscoveryEvent::SectionWritten {
            section: Section::Goal,
            hash: "h".into(),
        };
        assert!(matches!(
            judge(&events, event, 5).unwrap(),
            Appended::Frozen(Illegal::Frozen(Status::Aborted))
        ));
    }

    #[test]
    fn summary_reads_the_state() {
        let mut events = started();
        events.push(Logged::new(
            2,
            9,
            DiscoveryEvent::SectionWritten {
                section: Section::Goal,
                hash: "h".into(),
            },
        ));
        let s = DiscoverySummary::from_events(DiscoveryId::new("0001-p"), "/ws", &events).unwrap();
        assert_eq!(s.problem, "p");
        assert_eq!(s.status, Status::Open);
        assert_eq!(s.open, Some(Section::Goal));
        assert_eq!((s.events, s.started_at, s.updated_at), (2, 1, 9));
    }

    #[test]
    fn hash_is_stable_hex() {
        assert_eq!(text_hash("a"), text_hash("a"));
        assert_ne!(text_hash("a"), text_hash("b"));
        assert_eq!(text_hash("").len(), 64);
    }

    #[test]
    fn ports_are_object_safe() {
        fn log(_: Option<std::sync::Arc<dyn DiscoveryLogPort>>) {}
        fn sources(_: Option<std::sync::Arc<dyn SourceStorePort>>) {}
        log(None);
        sources(None);
    }
}
