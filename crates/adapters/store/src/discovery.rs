//! The discovery ports over SQLite: the event log with its artifacts
//! ([`DiscoveryLogPort`]) and the source texts ([`SourceStorePort`]), in the
//! tables of migration 0005.
//!
//! Every append is one transaction: the stored events are replayed, the
//! domain judges the new one ([`judge`]), and only then a row is written:
//! the event itself, or a `Refused` entry while the discovery is open,
//! nothing once it is frozen. So no event the state rejects is ever stored,
//! and a crash leaves the previous log intact.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::discovery::{
    Appended, DiscoveryId, DiscoveryLogError, DiscoveryLogPort, DiscoverySummary, SourceStoreError,
    SourceStorePort, SourceText, StoredSource, judge, text_hash,
};
use application::ports::store::StoreError;
use async_trait::async_trait;
use domain::discovery::{DiscoveryEvent, Level, Logged, SourceId, SourceKind};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SqliteFlowStorage;
use crate::db;
use crate::path::default_db_path;

/// The discovery log and source store over the crest database.
///
/// Like the other stores, calls run on tokio's blocking pool over a single
/// connection behind a `Mutex`; [`SqliteDiscoveryStore::sharing`] reuses the
/// flow store's connection so the two open the file once. Cloning is cheap
/// and shares the connection.
#[derive(Debug, Clone)]
pub struct SqliteDiscoveryStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteDiscoveryStore {
    /// Opens (creating and migrating if needed) the database at `path`.
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] if the file is not a database this build can
    /// read, [`StoreError::Backend`] on I/O or SQLite failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self::from_connection(db::open(path.as_ref())?))
    }

    /// Opens the database at the default path.
    ///
    /// # Errors
    /// As [`open`](Self::open), or [`StoreError::Backend`] if there is no
    /// home directory to put it in.
    pub fn open_default() -> Result<Self, StoreError> {
        let path = default_db_path().ok_or_else(|| {
            StoreError::Backend("no home directory for the discovery store".into())
        })?;
        Self::open(path)
    }

    /// A private in-memory database, lost on drop; for tests.
    ///
    /// # Errors
    /// [`StoreError::Backend`] if SQLite fails.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Ok(Self::from_connection(db::open_in_memory()?))
    }

    /// A store over the connection of `flow`.
    #[must_use]
    pub fn sharing(flow: &SqliteFlowStorage) -> Self {
        Self {
            conn: flow.connection(),
        }
    }

    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Runs `f` with the connection on the blocking pool; `wrap` turns a
    /// pool failure into the port's error.
    async fn run<T, E, F>(&self, wrap: fn(String) -> E, f: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, E> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut conn)
        })
        .await
        .map_err(|err| wrap(format!("discovery store task failed: {err}")))?
    }
}

fn backend(err: rusqlite::Error) -> DiscoveryLogError {
    DiscoveryLogError::Backend(db::sql_error(err).to_string())
}

fn source_backend(err: rusqlite::Error) -> SourceStoreError {
    SourceStoreError::Backend(db::sql_error(err).to_string())
}

fn level_name(level: Level) -> String {
    serde_json::to_value(level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{level:?}").to_lowercase())
}

fn kind_name(event: &DiscoveryEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|v| v.get("event").and_then(|k| k.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

fn exists(conn: &Connection, id: &DiscoveryId) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT 1 FROM discoveries WHERE id = ?1",
        params![id.as_str()],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}

fn insert_event(
    conn: &Connection,
    id: &DiscoveryId,
    logged: &Logged,
) -> Result<(), DiscoveryLogError> {
    let json = serde_json::to_string(logged)
        .map_err(|e| DiscoveryLogError::Backend(format!("cannot encode event: {e}")))?;
    conn.execute(
        "INSERT INTO discovery_events (discovery_id, seq, at, kind, event_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id.as_str(),
            i64::try_from(logged.seq).unwrap_or(i64::MAX),
            i64::try_from(logged.at).unwrap_or(i64::MAX),
            kind_name(&logged.event),
            json
        ],
    )
    .map_err(backend)?;
    conn.execute(
        "UPDATE discoveries SET updated_at = ?2 WHERE id = ?1",
        params![id.as_str(), i64::try_from(logged.at).unwrap_or(i64::MAX)],
    )
    .map_err(backend)?;
    Ok(())
}

fn load_events(conn: &Connection, id: &DiscoveryId) -> Result<Vec<Logged>, DiscoveryLogError> {
    if !exists(conn, id).map_err(backend)? {
        return Err(DiscoveryLogError::NotFound(id.clone()));
    }
    let mut stmt = conn
        .prepare("SELECT event_json FROM discovery_events WHERE discovery_id = ?1 ORDER BY seq")
        .map_err(backend)?;
    let rows = stmt
        .query_map(params![id.as_str()], |row| row.get::<_, String>(0))
        .map_err(backend)?;
    let mut events = Vec::new();
    for row in rows {
        let json = row.map_err(backend)?;
        let logged: Logged = serde_json::from_str(&json)
            .map_err(|e| DiscoveryLogError::Corrupt(id.clone(), e.to_string()))?;
        events.push(logged);
    }
    Ok(events)
}

fn start(
    conn: &mut Connection,
    id: &DiscoveryId,
    workspace_root: &str,
    problem: &str,
    level: Level,
    at: u64,
) -> Result<Logged, DiscoveryLogError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(backend)?;
    if exists(&tx, id).map_err(backend)? {
        return Err(DiscoveryLogError::AlreadyExists(id.clone()));
    }
    let at_i = i64::try_from(at).unwrap_or(i64::MAX);
    tx.execute(
        "INSERT INTO discoveries (id, workspace_root, problem, level, started_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            id.as_str(),
            workspace_root,
            problem,
            level_name(level),
            at_i
        ],
    )
    .map_err(backend)?;
    let logged = Logged::new(
        1,
        at,
        DiscoveryEvent::Started {
            problem: problem.to_owned(),
            level,
        },
    );
    insert_event(&tx, id, &logged)?;
    write_summary(&tx, id, std::slice::from_ref(&logged))?;
    tx.commit().map_err(backend)?;
    Ok(logged)
}

fn append(
    conn: &mut Connection,
    id: &DiscoveryId,
    event: DiscoveryEvent,
    at: u64,
) -> Result<Logged, DiscoveryLogError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(backend)?;
    let mut events = load_events(&tx, id)?;
    let judged = judge(&events, event, at)
        .map_err(|e| DiscoveryLogError::Corrupt(id.clone(), e.to_string()))?;
    let result = match judged {
        Appended::Accepted(logged) => {
            insert_event(&tx, id, &logged)?;
            events.push(logged.clone());
            write_summary(&tx, id, &events)?;
            Ok(logged)
        }
        Appended::Refused { refusal, reason } => {
            insert_event(&tx, id, &refusal)?;
            events.push(refusal);
            write_summary(&tx, id, &events)?;
            Err(DiscoveryLogError::Illegal {
                id: id.clone(),
                reason,
                logged: true,
            })
        }
        Appended::Frozen(reason) => Err(DiscoveryLogError::Illegal {
            id: id.clone(),
            reason,
            logged: false,
        }),
    };
    tx.commit().map_err(backend)?;
    result
}

/// Writes the replayed summary of `events` onto the row of `id`.
fn write_summary(
    conn: &Connection,
    id: &DiscoveryId,
    events: &[Logged],
) -> Result<(), DiscoveryLogError> {
    let summary = DiscoverySummary::from_events(id.clone(), "", events)
        .map_err(|e| DiscoveryLogError::Corrupt(id.clone(), e.to_string()))?;
    store_summary(conn, id, &summary).map_err(backend)
}

fn store_summary(
    conn: &Connection,
    id: &DiscoveryId,
    summary: &DiscoverySummary,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE discoveries SET status = ?2, open_section = ?3, event_count = ?4 WHERE id = ?1",
        params![
            id.as_str(),
            snake_name(&summary.status),
            summary.open.as_ref().map(snake_name),
            i64::try_from(summary.events).unwrap_or(i64::MAX)
        ],
    )
    .map(drop)
}

/// The snake_case serde name of a unit enum variant.
fn snake_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_name<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// Migration 0006: fills the summary columns of the discoveries that exist.
/// A log that does not replay keeps `status` NULL and is left out of
/// listings, as before.
pub(crate) fn backfill_summaries(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let ids: Vec<String> = tx
        .prepare("SELECT id FROM discoveries")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        let id = DiscoveryId::new(id);
        if let Ok(events) = load_events(tx, &id)
            && let Ok(summary) = DiscoverySummary::from_events(id.clone(), "", &events)
        {
            store_summary(tx, &id, &summary)?;
        }
    }
    Ok(())
}

/// Lists the replayable discoveries of a workspace, newest first, from the
/// summary columns: one query, no event is read.
fn list(
    conn: &Connection,
    workspace_root: &str,
) -> Result<Vec<DiscoverySummary>, DiscoveryLogError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, problem, level, status, open_section, event_count, started_at, updated_at \
             FROM discoveries WHERE workspace_root = ?1 AND status IS NOT NULL \
             ORDER BY started_at DESC, id DESC",
        )
        .map_err(backend)?;
    let rows = stmt
        .query_map(params![workspace_root], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(backend)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, problem, level, status, open, events, started_at, updated_at) =
            row.map_err(backend)?;
        let id = DiscoveryId::new(id);
        let (Some(level), Some(status)) = (from_name(&level), from_name(&status)) else {
            continue;
        };
        out.push(DiscoverySummary {
            id,
            workspace_root: workspace_root.to_owned(),
            problem,
            level,
            status,
            open: open.as_deref().and_then(from_name),
            events: u64::try_from(events).unwrap_or(0),
            started_at: u64::try_from(started_at).unwrap_or(0),
            updated_at: u64::try_from(updated_at).unwrap_or(0),
        });
    }
    Ok(out)
}

fn put_artifact(
    conn: &Connection,
    id: &DiscoveryId,
    name: &str,
    text: &str,
    at: u64,
) -> Result<(), DiscoveryLogError> {
    if !exists(conn, id).map_err(backend)? {
        return Err(DiscoveryLogError::NotFound(id.clone()));
    }
    conn.execute(
        "INSERT INTO discovery_artifacts (discovery_id, name, text, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (discovery_id, name) DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at",
        params![id.as_str(), name, text, i64::try_from(at).unwrap_or(i64::MAX)],
    )
    .map_err(backend)?;
    Ok(())
}

fn artifact(
    conn: &Connection,
    id: &DiscoveryId,
    name: &str,
) -> Result<Option<String>, DiscoveryLogError> {
    if !exists(conn, id).map_err(backend)? {
        return Err(DiscoveryLogError::NotFound(id.clone()));
    }
    conn.query_row(
        "SELECT text FROM discovery_artifacts WHERE discovery_id = ?1 AND name = ?2",
        params![id.as_str(), name],
        |row| row.get(0),
    )
    .optional()
    .map_err(backend)
}

fn artifacts(conn: &Connection, id: &DiscoveryId) -> Result<Vec<String>, DiscoveryLogError> {
    if !exists(conn, id).map_err(backend)? {
        return Err(DiscoveryLogError::NotFound(id.clone()));
    }
    let mut stmt = conn
        .prepare("SELECT name FROM discovery_artifacts WHERE discovery_id = ?1 ORDER BY name")
        .map_err(backend)?;
    let names = stmt
        .query_map(params![id.as_str()], |row| row.get(0))
        .map_err(backend)?
        .collect::<Result<_, _>>()
        .map_err(backend)?;
    Ok(names)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[async_trait]
impl DiscoveryLogPort for SqliteDiscoveryStore {
    async fn start(
        &self,
        id: &DiscoveryId,
        workspace_root: &str,
        problem: &str,
        level: Level,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError> {
        let (id, ws, problem) = (id.clone(), workspace_root.to_owned(), problem.to_owned());
        self.run(DiscoveryLogError::Backend, move |conn| {
            start(conn, &id, &ws, &problem, level, at)
        })
        .await
    }

    async fn append(
        &self,
        id: &DiscoveryId,
        event: DiscoveryEvent,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError> {
        let id = id.clone();
        self.run(DiscoveryLogError::Backend, move |conn| {
            append(conn, &id, event, at)
        })
        .await
    }

    async fn load(&self, id: &DiscoveryId) -> Result<Vec<Logged>, DiscoveryLogError> {
        let id = id.clone();
        self.run(DiscoveryLogError::Backend, move |conn| {
            load_events(conn, &id)
        })
        .await
    }

    async fn list(&self, workspace_root: &str) -> Result<Vec<DiscoverySummary>, DiscoveryLogError> {
        let ws = workspace_root.to_owned();
        self.run(DiscoveryLogError::Backend, move |conn| list(conn, &ws))
            .await
    }

    async fn put_artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
        text: &str,
    ) -> Result<(), DiscoveryLogError> {
        let (id, name, text) = (id.clone(), name.to_owned(), text.to_owned());
        self.run(DiscoveryLogError::Backend, move |conn| {
            put_artifact(conn, &id, &name, &text, now_ms())
        })
        .await
    }

    async fn artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
    ) -> Result<Option<String>, DiscoveryLogError> {
        let (id, name) = (id.clone(), name.to_owned());
        self.run(DiscoveryLogError::Backend, move |conn| {
            artifact(conn, &id, &name)
        })
        .await
    }

    async fn artifacts(&self, id: &DiscoveryId) -> Result<Vec<String>, DiscoveryLogError> {
        let id = id.clone();
        self.run(DiscoveryLogError::Backend, move |conn| artifacts(conn, &id))
            .await
    }
}

fn kind_json(kind: SourceKind) -> String {
    serde_json::to_string(&kind).unwrap_or_else(|_| "\"other\"".to_owned())
}

fn kind_from_json(json: &str) -> SourceKind {
    serde_json::from_str(json).unwrap_or_default()
}

fn row_source(row: &rusqlite::Row<'_>) -> Result<StoredSource, rusqlite::Error> {
    Ok(StoredSource {
        id: SourceId(row.get::<_, i64>(0)?.try_into().unwrap_or(0)),
        hash: row.get(1)?,
        source: SourceText {
            location: row.get(2)?,
            title: row.get(3)?,
            kind: kind_from_json(&row.get::<_, String>(4)?),
            fetched_at: row.get::<_, i64>(5)?.try_into().unwrap_or(0),
            text: row.get(6)?,
        },
    })
}

const SOURCE_COLUMNS: &str = "source_id, hash, location, title, kind, fetched_at, text";

fn put_source(
    conn: &mut Connection,
    discovery: &DiscoveryId,
    source: SourceText,
) -> Result<StoredSource, SourceStoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(source_backend)?;
    let next: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(source_id), 0) + 1 FROM discovery_sources WHERE discovery_id = ?1",
            params![discovery.as_str()],
            |row| row.get(0),
        )
        .map_err(source_backend)?;
    let hash = text_hash(&source.text);
    tx.execute(
        "INSERT INTO discovery_sources (discovery_id, source_id, hash, location, title, kind, fetched_at, text) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            discovery.as_str(),
            next,
            hash,
            source.location,
            source.title,
            kind_json(source.kind),
            i64::try_from(source.fetched_at).unwrap_or(i64::MAX),
            source.text
        ],
    )
    .map_err(source_backend)?;
    tx.commit().map_err(source_backend)?;
    Ok(StoredSource {
        id: SourceId(u32::try_from(next).unwrap_or(u32::MAX)),
        hash,
        source,
    })
}

fn get_source(
    conn: &Connection,
    discovery: &DiscoveryId,
    id: SourceId,
) -> Result<StoredSource, SourceStoreError> {
    conn.query_row(
        &format!("SELECT {SOURCE_COLUMNS} FROM discovery_sources WHERE discovery_id = ?1 AND source_id = ?2"),
        params![discovery.as_str(), i64::from(id.0)],
        row_source,
    )
    .optional()
    .map_err(source_backend)?
    .ok_or_else(|| SourceStoreError::NotFound(discovery.clone(), id))
}

fn list_sources(
    conn: &Connection,
    discovery: &DiscoveryId,
) -> Result<Vec<StoredSource>, SourceStoreError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SOURCE_COLUMNS} FROM discovery_sources WHERE discovery_id = ?1 ORDER BY source_id"
        ))
        .map_err(source_backend)?;
    let rows = stmt
        .query_map(params![discovery.as_str()], row_source)
        .map_err(source_backend)?;
    rows.collect::<Result<_, _>>().map_err(source_backend)
}

#[async_trait]
impl SourceStorePort for SqliteDiscoveryStore {
    async fn put(
        &self,
        discovery: &DiscoveryId,
        source: SourceText,
    ) -> Result<StoredSource, SourceStoreError> {
        let discovery = discovery.clone();
        self.run(SourceStoreError::Backend, move |conn| {
            put_source(conn, &discovery, source)
        })
        .await
    }

    async fn get(
        &self,
        discovery: &DiscoveryId,
        id: SourceId,
    ) -> Result<StoredSource, SourceStoreError> {
        let discovery = discovery.clone();
        self.run(SourceStoreError::Backend, move |conn| {
            get_source(conn, &discovery, id)
        })
        .await
    }

    async fn sources(
        &self,
        discovery: &DiscoveryId,
    ) -> Result<Vec<StoredSource>, SourceStoreError> {
        let discovery = discovery.clone();
        self.run(SourceStoreError::Backend, move |conn| {
            list_sources(conn, &discovery)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use domain::discovery::{Actor, Illegal, Section, Status, Verdict};

    use super::*;

    fn id() -> DiscoveryId {
        DiscoveryId::new("0001-slow")
    }

    fn write(section: Section) -> DiscoveryEvent {
        DiscoveryEvent::SectionWritten {
            section,
            hash: "h".into(),
        }
    }

    fn count(store: &SqliteDiscoveryStore, table: &str) -> i64 {
        let conn = store.conn.lock().unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn events_round_trip_and_illegal_ones_are_refused_or_dropped() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        let started = store
            .start(&id(), "/ws", "slow sessions", Level::Research, 10)
            .await
            .unwrap();
        assert_eq!(started.seq, 1);
        assert_eq!(
            store
                .start(&id(), "/ws", "again", Level::Research, 11)
                .await
                .unwrap_err(),
            DiscoveryLogError::AlreadyExists(id())
        );
        let written = store.append(&id(), write(Section::Goal), 20).await.unwrap();
        assert_eq!((written.seq, written.at), (2, 20));
        // A verdict without an audit: illegal while open, so a Refused row.
        let err = store
            .append(
                &id(),
                DiscoveryEvent::Verdict {
                    section: Section::Goal,
                    choice: Verdict::ApproveAsIs,
                    by: Actor::Human("alex".into()),
                    note: None,
                },
                30,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DiscoveryLogError::Illegal {
                reason: Illegal::Missing { .. },
                logged: true,
                ..
            }
        ));
        let events = store.load(&id()).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], started);
        assert_eq!(events[1], written);
        assert!(matches!(events[2].event, DiscoveryEvent::Refused { .. }));
        assert_eq!(count(&store, "discovery_events"), 3);
        // Frozen: the table is unchanged.
        store
            .append(&id(), DiscoveryEvent::Aborted, 40)
            .await
            .unwrap();
        let err = store
            .append(&id(), write(Section::Goal), 50)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DiscoveryLogError::Illegal { logged: false, .. }
        ));
        assert_eq!(count(&store, "discovery_events"), 4);
        let list = store.list("/ws").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, Status::Aborted);
        assert_eq!((list[0].started_at, list[0].updated_at), (10, 40));
        assert!(store.list("/elsewhere").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_corrupt_log_is_reported_on_load_and_refuses_appends() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        let bad = DiscoveryId::new("0001-bad");
        let good = DiscoveryId::new("0002-good");
        store
            .start(&bad, "/ws", "p", Level::Research, 1)
            .await
            .unwrap();
        store
            .start(&good, "/ws", "q", Level::Research, 2)
            .await
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE discovery_events SET event_json = '{\"seq\": 1, \"at\": 1, \"event\": \"from_the_future\"}' WHERE discovery_id = ?1",
                params![bad.as_str()],
            )
            .unwrap();
        }
        // The listing reads the summary kept on the row, not the log: the
        // damaged one stays listed until its next append, which replays and
        // fails. (A log that is corrupt at migration time is left out; see
        // the backfill test.) `load` reports it.
        let list = store.list("/ws").await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|s| s.id == good));
        assert!(matches!(
            store
                .append(&bad, DiscoveryEvent::Aborted, 5)
                .await
                .unwrap_err(),
            DiscoveryLogError::Corrupt(..)
        ));
        assert!(matches!(
            store.load(&bad).await.unwrap_err(),
            DiscoveryLogError::Corrupt(id, _) if id == bad
        ));
    }

    #[tokio::test]
    async fn list_matches_the_replayed_summaries_and_follows_appends() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        let a = DiscoveryId::new("0001-a");
        let b = DiscoveryId::new("0002-b");
        store
            .start(&a, "/ws", "first", Level::Research, 10)
            .await
            .unwrap();
        store
            .start(&b, "/ws", "second", Level::Research, 20)
            .await
            .unwrap();
        store
            .start(
                &DiscoveryId::new("0003-c"),
                "/other",
                "x",
                Level::Research,
                30,
            )
            .await
            .unwrap();
        store.append(&a, write(Section::Goal), 40).await.unwrap();
        // A refused event is a row too, and counts.
        let refused = store
            .append(
                &a,
                DiscoveryEvent::Verdict {
                    section: Section::Goal,
                    choice: Verdict::ApproveAsIs,
                    by: Actor::Human("alex".into()),
                    note: None,
                },
                50,
            )
            .await;
        assert!(refused.is_err());
        let list = store.list("/ws").await.unwrap();
        assert_eq!(
            list.iter().map(|s| s.status).collect::<Vec<_>>(),
            [Status::Open, Status::Open]
        );
        store.append(&b, DiscoveryEvent::Aborted, 60).await.unwrap();
        let list = store.list("/ws").await.unwrap();
        assert_eq!(
            list.iter()
                .map(|s| (s.id.as_str(), s.status))
                .collect::<Vec<_>>(),
            [("0002-b", Status::Aborted), ("0001-a", Status::Open)]
        );
        assert_eq!(list[1].events, 3);
        for summary in &list {
            let events = store.load(&summary.id).await.unwrap();
            let replayed =
                DiscoverySummary::from_events(summary.id.clone(), "/ws", &events).unwrap();
            assert_eq!(summary, &replayed);
        }
    }

    #[test]
    fn migration_0006_backfills_existing_discoveries() {
        use crate::migrations::{
            MIGRATIONS, current_version, latest_version, migrate, migrate_with,
        };
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_with(&mut conn, &MIGRATIONS[..5]).unwrap();
        let insert = |id: &str, at: i64, events: &[Logged]| {
            conn.execute(
                "INSERT INTO discoveries (id, workspace_root, problem, level, started_at, updated_at) \
                 VALUES (?1, '/ws', 'p', 'research', ?2, ?2)",
                params![id, at],
            )
            .unwrap();
            for e in events {
                conn.execute(
                    "INSERT INTO discovery_events VALUES (?1, ?2, ?3, 'k', ?4)",
                    params![
                        id,
                        i64::try_from(e.seq).unwrap(),
                        i64::try_from(e.at).unwrap(),
                        serde_json::to_string(e).unwrap()
                    ],
                )
                .unwrap();
                conn.execute(
                    "UPDATE discoveries SET updated_at = ?2 WHERE id = ?1",
                    params![id, i64::try_from(e.at).unwrap()],
                )
                .unwrap();
            }
        };
        let started = |at| {
            Logged::new(
                1,
                at,
                DiscoveryEvent::Started {
                    problem: "p".into(),
                    level: Level::Research,
                },
            )
        };
        let open = vec![started(1), Logged::new(2, 2, write(Section::Goal))];
        let aborted = vec![started(3), Logged::new(2, 4, DiscoveryEvent::Aborted)];
        insert("0001-open", 1, &open);
        insert("0002-aborted", 3, &aborted);
        insert("0003-bad", 5, &[started(5)]);
        conn.execute(
            "UPDATE discovery_events SET event_json = '{}' WHERE discovery_id = '0003-bad'",
            [],
        )
        .unwrap();
        insert("0004-empty", 6, &[]);

        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        assert_eq!(current_version(&conn).unwrap(), latest_version());
        let list = list(&conn, "/ws").unwrap();
        // Logs that do not replay (corrupt, empty) are left out.
        assert_eq!(
            list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["0002-aborted", "0001-open"]
        );
        let want = |id: &str, events: &[Logged]| {
            DiscoverySummary::from_events(DiscoveryId::new(id), "/ws", events).unwrap()
        };
        assert_eq!(list[0], want("0002-aborted", &aborted));
        assert_eq!(list[1], want("0001-open", &open));
        assert_eq!(list[1].status, Status::Open);
    }

    #[tokio::test]
    async fn missing_discoveries_are_reported() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        let nope = DiscoveryId::new("nope");
        assert_eq!(
            store.load(&nope).await.unwrap_err(),
            DiscoveryLogError::NotFound(nope.clone())
        );
        assert_eq!(
            store
                .append(&nope, DiscoveryEvent::Aborted, 1)
                .await
                .unwrap_err(),
            DiscoveryLogError::NotFound(nope.clone())
        );
        assert_eq!(
            store.put_artifact(&nope, "a", "b").await.unwrap_err(),
            DiscoveryLogError::NotFound(nope)
        );
    }

    #[tokio::test]
    async fn artifacts_replace_by_name() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        store
            .start(&id(), "/ws", "p", Level::Deep, 1)
            .await
            .unwrap();
        store
            .put_artifact(&id(), "sections/goal.md", "v1")
            .await
            .unwrap();
        store
            .put_artifact(&id(), "sections/goal.md", "v2")
            .await
            .unwrap();
        store
            .put_artifact(&id(), "records/claims.json", "[]")
            .await
            .unwrap();
        assert_eq!(
            store
                .artifact(&id(), "sections/goal.md")
                .await
                .unwrap()
                .as_deref(),
            Some("v2")
        );
        assert_eq!(store.artifact(&id(), "missing").await.unwrap(), None);
        assert_eq!(
            store.artifacts(&id()).await.unwrap(),
            vec!["records/claims.json", "sections/goal.md"]
        );
    }

    #[tokio::test]
    async fn sources_round_trip_with_their_hash() {
        let store = SqliteDiscoveryStore::open_in_memory().unwrap();
        store
            .start(&id(), "/ws", "p", Level::Deep, 1)
            .await
            .unwrap();
        let source = SourceText {
            location: "https://example.org/paper".into(),
            title: Some("A paper".into()),
            kind: SourceKind::Study,
            text: "habituation grows with repetition".into(),
            fetched_at: 5,
        };
        let stored = store.put(&id(), source.clone()).await.unwrap();
        assert_eq!(stored.id, SourceId(1));
        assert_eq!(stored.hash, text_hash(&source.text));
        let second = store
            .put(
                &id(),
                SourceText {
                    title: None,
                    kind: SourceKind::Vendor,
                    ..source.clone()
                },
            )
            .await
            .unwrap();
        assert_eq!(second.id, SourceId(2));
        let back = store.get(&id(), SourceId(1)).await.unwrap();
        assert_eq!(back, stored);
        let all = store.sources(&id()).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].source.kind, SourceKind::Vendor);
        assert_eq!(
            store.get(&id(), SourceId(9)).await.unwrap_err(),
            SourceStoreError::NotFound(id(), SourceId(9))
        );
    }

    #[tokio::test]
    async fn shares_the_flow_store_connection() {
        let flow = SqliteFlowStorage::open_in_memory().unwrap();
        let store = SqliteDiscoveryStore::sharing(&flow);
        store
            .start(&id(), "/ws", "p", Level::Research, 1)
            .await
            .unwrap();
        assert_eq!(store.load(&id()).await.unwrap().len(), 1);
    }
}
