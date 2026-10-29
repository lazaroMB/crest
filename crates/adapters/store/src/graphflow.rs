//! graph-flow's [`SessionStorage`] over SQLite: the checkpoints of the
//! investigative flow.
//!
//! Each [`Session`] is one row of `flow_sessions` (migration 0003): the whole
//! session as JSON, its [`Context`](graph_flow::Context) included, plus
//! copies of the graph, the current task, the status message and the
//! optimistic-locking version. Every save is one transaction, so a crash
//! leaves the previous checkpoint intact; reopening the database and running
//! the same graph again resumes at `current_task_id`.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use application::ports::store::StoreError;
use async_trait::async_trait;
use graph_flow::{GraphError, Session, SessionStorage};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::db;
use crate::path::default_db_path;

/// [`SessionStorage`] over the crest database.
///
/// It lives in the same file as the chat sessions
/// ([`SqliteSessionStore`](crate::SqliteSessionStore)) but opens its own
/// connection to it: the two stores have no rows in common, WAL lets them
/// read and write side by side and the busy timeout covers the rare moment
/// both write at once. Opening runs the (idempotent) migrations, so either
/// store can be opened first. Like the chat store, calls run on tokio's
/// blocking pool over a single connection behind a `Mutex`.
///
/// Saves follow graph-flow's optimistic locking: a save is accepted only if
/// the session is new or its `version` equals the stored one, and the stored
/// version is then incremented; otherwise it fails with
/// [`GraphError::SessionConflict`].
///
/// Cloning is cheap and shares the connection.
#[derive(Debug, Clone)]
pub struct SqliteFlowStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteFlowStorage {
    /// Opens (creating and migrating if needed) the database at `path`.
    /// Blocks while opening; meant for start-up.
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] if the file is not a database this build can
    /// read, [`StoreError::Backend`] on I/O or SQLite failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self::from_connection(db::open(path.as_ref())?))
    }

    /// Opens the database at the default path, the same file the chat
    /// sessions use (see [`default_db_path`]).
    ///
    /// # Errors
    /// As [`open`](Self::open), or [`StoreError::Backend`] if there is no
    /// home directory to put it in.
    pub fn open_default() -> Result<Self, StoreError> {
        let path = default_db_path()
            .ok_or_else(|| StoreError::Backend("no home directory for the flow store".into()))?;
        Self::open(path)
    }

    /// A private in-memory database, lost on drop; for tests.
    ///
    /// # Errors
    /// [`StoreError::Backend`] if SQLite fails.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Ok(Self::from_connection(db::open_in_memory()?))
    }

    fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// The shared connection, for the discovery store to reuse.
    pub(crate) fn connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Runs `f` with the connection on the blocking pool.
    async fn run<T, F>(&self, f: F) -> graph_flow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> graph_flow::Result<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            // An open transaction is rolled back when dropped, so a panic
            // mid-call leaves the connection usable.
            let mut conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut conn)
        })
        .await
        .map_err(|err| storage_error(format!("flow store task failed: {err}")))?
    }
}

#[async_trait]
impl SessionStorage for SqliteFlowStorage {
    async fn save(&self, session: Session) -> graph_flow::Result<()> {
        self.run(move |conn| save(conn, session)).await
    }

    async fn get(&self, id: &str) -> graph_flow::Result<Option<Session>> {
        let id = id.to_owned();
        self.run(move |conn| get(conn, &id)).await
    }

    async fn delete(&self, id: &str) -> graph_flow::Result<()> {
        let id = id.to_owned();
        self.run(move |conn| delete(conn, &id)).await
    }
}

/// Inserts or replaces `session` if its version matches the stored one.
fn save(conn: &mut Connection, mut session: Session) -> graph_flow::Result<()> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    let stored: Option<i64> = tx
        .query_row(
            "SELECT version FROM flow_sessions WHERE id = ?1",
            [&session.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if let Some(stored) = stored
        && u64::try_from(stored).ok() != Some(session.version)
    {
        return Err(GraphError::SessionConflict(format!(
            "flow session '{}' was modified concurrently (stored version {stored}, \
             attempted save from version {})",
            session.id, session.version
        )));
    }
    session.version = session
        .version
        .checked_add(1)
        .ok_or_else(|| storage_error("flow session version overflow"))?;
    let version = i64::try_from(session.version)
        .map_err(|_| storage_error("flow session version overflow"))?;
    let data_json = serde_json::to_string(&session)
        .map_err(|err| storage_error(format!("cannot encode flow session: {err}")))?;
    tx.execute(
        "INSERT INTO flow_sessions
             (id, graph_id, current_task_id, status_message, version, updated_at, data_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
             graph_id = excluded.graph_id,
             current_task_id = excluded.current_task_id,
             status_message = excluded.status_message,
             version = excluded.version,
             updated_at = excluded.updated_at,
             data_json = excluded.data_json",
        params![
            session.id,
            session.graph_id,
            session.current_task_id,
            session.status_message,
            version,
            now_millis(),
            data_json,
        ],
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

/// The session `id`, or `None` if there is none.
fn get(conn: &Connection, id: &str) -> graph_flow::Result<Option<Session>> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT version, data_json FROM flow_sessions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let Some((version, data_json)) = row else {
        return Ok(None);
    };
    let mut session: Session = serde_json::from_str(&data_json)
        .map_err(|err| storage_error(format!("flow session '{id}' is corrupt: {err}")))?;
    session.version = u64::try_from(version)
        .map_err(|_| storage_error(format!("flow session '{id}' has version {version}")))?;
    Ok(Some(session))
}

/// Removes the session `id`; removing a missing one is not an error.
fn delete(conn: &Connection, id: &str) -> graph_flow::Result<()> {
    conn.execute("DELETE FROM flow_sessions WHERE id = ?1", [id])
        .map_err(sql_error)?;
    Ok(())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn storage_error(message: impl Into<String>) -> GraphError {
    GraphError::StorageError(message.into())
}

fn sql_error(err: rusqlite::Error) -> GraphError {
    storage_error(format!("flow store: {}", db::sql_error(err)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SqliteFlowStorage {
        SqliteFlowStorage::open_in_memory().unwrap()
    }

    fn session(id: &str) -> Session {
        let session = Session::new_from_task(id.to_owned(), "start").with_graph_id("toy");
        session.context.set("answer", 42).unwrap();
        session.context.add_user_message("hello".to_owned());
        session
    }

    #[tokio::test]
    async fn save_then_get_round_trips() {
        let store = store();
        let mut s = session("s1");
        s.status_message = Some("waiting".into());
        store.save(s).await.unwrap();

        let got = store.get("s1").await.unwrap().unwrap();
        assert_eq!(got.id, "s1");
        assert_eq!(got.graph_id, "toy");
        assert_eq!(got.current_task_id, "start");
        assert_eq!(got.status_message.as_deref(), Some("waiting"));
        assert_eq!(got.version, 1);
        assert_eq!(got.context.get::<i32>("answer"), Some(42));
        assert_eq!(got.context.chat_history_len(), 1);
    }

    #[tokio::test]
    async fn get_of_a_missing_id_is_none() {
        assert!(store().get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_removes_and_tolerates_missing() {
        let store = store();
        store.save(session("s1")).await.unwrap();
        store.delete("s1").await.unwrap();
        assert!(store.get("s1").await.unwrap().is_none());
        store.delete("s1").await.unwrap();
    }

    #[tokio::test]
    async fn a_stale_save_is_a_conflict() {
        let store = store();
        store.save(session("s1")).await.unwrap();
        let a = store.get("s1").await.unwrap().unwrap();
        let b = store.get("s1").await.unwrap().unwrap();
        store.save(a).await.unwrap();
        let err = store.save(b).await.unwrap_err();
        assert!(matches!(err, GraphError::SessionConflict(_)), "{err}");
        assert_eq!(store.get("s1").await.unwrap().unwrap().version, 2);
    }

    #[tokio::test]
    async fn a_large_context_round_trips() {
        let store = store();
        let s = session("big");
        let blob = "x".repeat(4 * 1024 * 1024);
        s.context.set("blob", &blob).unwrap();
        let items: Vec<u32> = (0..50_000).collect();
        s.context.set("items", &items).unwrap();
        store.save(s).await.unwrap();

        let got = store.get("big").await.unwrap().unwrap();
        assert_eq!(got.context.get::<String>("blob"), Some(blob));
        assert_eq!(got.context.get::<Vec<u32>>("items"), Some(items));
    }

    #[tokio::test]
    async fn corrupt_json_is_a_storage_error() {
        let store = store();
        store.save(session("s1")).await.unwrap();
        store
            .run(|conn| {
                conn.execute("UPDATE flow_sessions SET data_json = 'not json'", [])
                    .map_err(sql_error)
            })
            .await
            .unwrap();
        let err = store.get("s1").await.unwrap_err();
        assert!(matches!(err, GraphError::StorageError(_)), "{err}");
    }

    #[tokio::test]
    async fn sessions_survive_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crest.db");
        SqliteFlowStorage::open(&path)
            .unwrap()
            .save(session("s1"))
            .await
            .unwrap();
        let reopened = SqliteFlowStorage::open(&path).unwrap();
        let got = reopened.get("s1").await.unwrap().unwrap();
        assert_eq!(got.context.get::<i32>("answer"), Some(42));
    }
}
