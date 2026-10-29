//! [`SessionStorePort`] over SQLite.
//!
//! A session is one row of `sessions` (the [`Session`] as JSON plus copies of
//! the fields used to list and order), one row per timeline block and one row
//! per permission rule. A save replaces all of them in one transaction.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::projects::RecentWorkspace;
use application::ports::store::{SessionStorePort, SessionSummary, StoreError, StoredSession};
use async_trait::async_trait;
use domain::block::Block;
use domain::conversation::{Session, SessionId};
use domain::permission::{PermissionRule, RuleScope};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::de::DeserializeOwned;

use crate::db::{self, sql_error};
use crate::path::default_db_path;

/// [`SessionStorePort`] over one SQLite database.
///
/// rusqlite is synchronous, so every call runs on tokio's blocking pool
/// (`spawn_blocking`) over a single connection behind a `Mutex`. One
/// connection is enough: SQLite serializes writers anyway, saves are small
/// and happen once per turn, and it keeps the connection settings and the
/// migration in one place (opening per call would redo both). Other
/// processes using the same file are handled by WAL and the busy timeout.
///
/// Cloning is cheap and shares the connection.
#[derive(Debug, Clone)]
pub struct SqliteSessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteSessionStore {
    /// Opens (creating and migrating if needed) the database at `path`.
    /// Blocks while opening; meant for start-up.
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] if the file is not a database this build can
    /// read, [`StoreError::Backend`] on I/O or SQLite failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Ok(Self::from_connection(db::open(path.as_ref())?))
    }

    /// Opens the database at the default path (see
    /// [`default_db_path`](crate::path::default_db_path)).
    ///
    /// # Errors
    /// As [`open`](Self::open), or [`StoreError::Backend`] if there is no
    /// home directory to put it in.
    pub fn open_default() -> Result<Self, StoreError> {
        let path = default_db_path()
            .ok_or_else(|| StoreError::Backend("no home directory for the session store".into()))?;
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

    /// Runs `f` with the connection on the blocking pool.
    async fn run<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            // A panic mid-call cannot leave a half-applied save: the open
            // transaction is rolled back when dropped, so the connection is
            // still usable after poisoning.
            let mut conn = conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut conn)
        })
        .await
        .map_err(|err| StoreError::Backend(format!("session store task failed: {err}")))?
    }
}

#[async_trait]
impl SessionStorePort for SqliteSessionStore {
    async fn save(&self, session: &StoredSession) -> Result<(), StoreError> {
        let session = session.clone();
        self.run(move |conn| save(conn, &session)).await
    }

    async fn load(&self, id: SessionId) -> Result<StoredSession, StoreError> {
        self.run(move |conn| load(conn, id)).await
    }

    async fn latest_for_workspace(&self, root: &str) -> Result<Option<StoredSession>, StoreError> {
        let root = root.to_owned();
        self.run(move |conn| latest_for_workspace(conn, &root))
            .await
    }

    async fn list(&self, root: &str) -> Result<Vec<SessionSummary>, StoreError> {
        let root = root.to_owned();
        self.run(move |conn| list(conn, &root)).await
    }

    async fn delete(&self, id: SessionId) -> Result<(), StoreError> {
        self.run(move |conn| delete(conn, id)).await
    }

    async fn recent_workspaces(&self, limit: usize) -> Result<Vec<RecentWorkspace>, StoreError> {
        self.run(move |conn| recent_workspaces(conn, limit)).await
    }
}

/// Inserts or replaces `stored`, its timeline and its rules, atomically.
pub(crate) fn save(conn: &mut Connection, stored: &StoredSession) -> Result<(), StoreError> {
    let session = &stored.session;
    let id = session.id.to_string();
    let data_json = to_json(session)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    tx.execute(
        "INSERT INTO sessions
             (id, workspace_root, title, created_at, updated_at, turn_count, data_json,
              prompt_history_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
             workspace_root = excluded.workspace_root,
             title = excluded.title,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at,
             turn_count = excluded.turn_count,
             data_json = excluded.data_json,
             prompt_history_json = excluded.prompt_history_json",
        params![
            id,
            session.workspace_root,
            session.title,
            to_i64(session.created_at, "created_at")?,
            to_i64(session.updated_at, "updated_at")?,
            to_i64(session.turns.len(), "turn_count")?,
            data_json,
            to_json(&stored.prompt_history)?,
        ],
    )
    .map_err(sql_error)?;
    replace_blocks(&tx, &id, &stored.timeline)?;
    replace_rules(&tx, &id, &stored.permission_rules)?;
    tx.commit().map_err(sql_error)
}

fn replace_blocks(tx: &Transaction<'_>, id: &str, blocks: &[Block]) -> Result<(), StoreError> {
    tx.execute("DELETE FROM timeline_blocks WHERE session_id = ?1", [id])
        .map_err(sql_error)?;
    let mut insert = tx
        .prepare(
            "INSERT INTO timeline_blocks (session_id, seq, block_id, kind, data_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(sql_error)?;
    for (seq, block) in (0_i64..).zip(blocks) {
        let value = serde_json::to_value(block).map_err(encode_error)?;
        // The serde tag of the kind (`text`, `tool_call`, ...), for queries
        // and debugging; the block itself is read back from `data_json`.
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        insert
            .execute(params![
                id,
                seq,
                block.id.to_string(),
                kind,
                value.to_string()
            ])
            .map_err(sql_error)?;
    }
    Ok(())
}

/// Replaces the rules of session `id` with its session rules: project
/// rules live in the workspace's permissions file, not with a session.
fn replace_rules(
    tx: &Transaction<'_>,
    id: &str,
    rules: &[PermissionRule],
) -> Result<(), StoreError> {
    tx.execute("DELETE FROM permission_rules WHERE session_id = ?1", [id])
        .map_err(sql_error)?;
    let mut insert = tx
        .prepare("INSERT INTO permission_rules (session_id, seq, rule_json) VALUES (?1, ?2, ?3)")
        .map_err(sql_error)?;
    let session_rules = rules.iter().filter(|rule| rule.scope == RuleScope::Session);
    for (seq, rule) in (0_i64..).zip(session_rules) {
        insert
            .execute(params![id, seq, to_json(rule)?])
            .map_err(sql_error)?;
    }
    Ok(())
}

/// Loads the session `id` with its timeline and rules.
pub(crate) fn load(conn: &mut Connection, id: SessionId) -> Result<StoredSession, StoreError> {
    // A read transaction so the three reads see the same save.
    let tx = conn.transaction().map_err(sql_error)?;
    let key = id.to_string();
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT data_json, prompt_history_json FROM sessions WHERE id = ?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    let (data_json, history_json) = row.ok_or(StoreError::NotFound(id))?;
    let session: Session = from_json(&data_json, "session")?;
    let prompt_history = from_json(&history_json, "prompt history")?;
    if session.id != id {
        return Err(StoreError::Corrupt(format!(
            "row {id} holds session {}",
            session.id
        )));
    }
    let timeline = load_column(
        &tx,
        "SELECT data_json FROM timeline_blocks WHERE session_id = ?1 ORDER BY seq",
        &key,
        "timeline block",
    )?;
    let permission_rules = load_column(
        &tx,
        "SELECT rule_json FROM permission_rules WHERE session_id = ?1 ORDER BY seq",
        &key,
        "permission rule",
    )?;
    tx.commit().map_err(sql_error)?;
    Ok(StoredSession {
        session,
        timeline,
        permission_rules,
        prompt_history,
    })
}

/// Runs `sql` (one JSON text column, filtered by session id) and decodes
/// every row.
fn load_column<T: DeserializeOwned>(
    tx: &Transaction<'_>,
    sql: &str,
    id: &str,
    what: &str,
) -> Result<Vec<T>, StoreError> {
    let mut stmt = tx.prepare(sql).map_err(sql_error)?;
    let rows = stmt
        .query_map([id], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    rows.map(|json| from_json(&json.map_err(sql_error)?, what))
        .collect()
}

/// Order of a workspace's sessions, the same as the in-memory fake: most
/// recently updated first, then most recently created, then by id. Ids are
/// lowercase hyphenated UUIDs, whose text order matches `SessionId`'s order.
const NEWEST_FIRST: &str = "ORDER BY updated_at DESC, created_at DESC, id DESC";

/// Loads the most recent session of `root`, if any.
pub(crate) fn latest_for_workspace(
    conn: &mut Connection,
    root: &str,
) -> Result<Option<StoredSession>, StoreError> {
    let sql = format!("SELECT id FROM sessions WHERE workspace_root = ?1 {NEWEST_FIRST} LIMIT 1");
    let id: Option<String> = conn
        .query_row(&sql, [root], |row| row.get(0))
        .optional()
        .map_err(sql_error)?;
    id.map(|id| load(conn, parse_id(&id)?)).transpose()
}

/// Lists the sessions of `root`, newest first, without decoding them.
pub(crate) fn list(conn: &Connection, root: &str) -> Result<Vec<SessionSummary>, StoreError> {
    let sql = format!(
        "SELECT id, title, workspace_root, updated_at, turn_count
         FROM sessions WHERE workspace_root = ?1 {NEWEST_FIRST}"
    );
    let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
    let rows = stmt
        .query_map([root], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(sql_error)?;
    rows.map(|row| {
        let (id, title, workspace_root, updated_at, turns) = row.map_err(sql_error)?;
        Ok(SessionSummary {
            id: parse_id(&id)?,
            title,
            workspace_root,
            updated_at: from_i64(updated_at, "updated_at")?,
            turns: from_i64(turns, "turn_count")?,
        })
    })
    .collect()
}

/// The workspaces with sessions, by their last save, newest first.
pub(crate) fn recent_workspaces(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<RecentWorkspace>, StoreError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut stmt = conn
        .prepare(
            "SELECT workspace_root, MAX(updated_at) AS last
             FROM sessions GROUP BY workspace_root
             ORDER BY last DESC, workspace_root ASC LIMIT ?1",
        )
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(sql_error)?;
    rows.map(|row| {
        let (root, last) = row.map_err(sql_error)?;
        Ok(RecentWorkspace {
            root,
            updated_at: from_i64(last, "updated_at")?,
        })
    })
    .collect()
}

/// Deletes the session `id`; its blocks and rules go with it (cascade).
pub(crate) fn delete(conn: &Connection, id: SessionId) -> Result<(), StoreError> {
    let deleted = conn
        .execute("DELETE FROM sessions WHERE id = ?1", [id.to_string()])
        .map_err(sql_error)?;
    if deleted == 0 {
        return Err(StoreError::NotFound(id));
    }
    Ok(())
}

/// Parses an id column back into a [`SessionId`] (serde, since the id is
/// a transparent UUID).
fn parse_id(id: &str) -> Result<SessionId, StoreError> {
    serde_json::from_value(serde_json::Value::String(id.to_owned()))
        .map_err(|err| StoreError::Corrupt(format!("session id {id:?}: {err}")))
}

fn from_i64<T: TryFrom<i64>>(value: i64, field: &str) -> Result<T, StoreError> {
    T::try_from(value).map_err(|_| StoreError::Corrupt(format!("{field} out of range: {value}")))
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(encode_error)
}

fn from_json<T: DeserializeOwned>(json: &str, what: &str) -> Result<T, StoreError> {
    serde_json::from_str(json).map_err(|err| StoreError::Corrupt(format!("{what}: {err}")))
}

fn encode_error(err: serde_json::Error) -> StoreError {
    StoreError::Backend(format!("cannot encode session: {err}"))
}

/// SQLite integers are signed; values past `i64::MAX` cannot be stored.
fn to_i64<T: TryInto<i64>>(value: T, field: &str) -> Result<i64, StoreError> {
    value
        .try_into()
        .map_err(|_| StoreError::Backend(format!("{field} does not fit in SQLite")))
}

#[cfg(test)]
pub(crate) mod tests {
    use domain::block::{BlockKind, ToolStatus};
    use domain::conversation::{Message, Turn};
    use domain::permission::{RuleDecision, RuleMatcher};

    use super::*;

    /// A session with two turns, `blocks` varied blocks and one rule.
    pub(crate) fn sample(root: &str, updated_at: u64, blocks: usize) -> StoredSession {
        let mut session = Session::new(root, updated_at);
        session.title = Some(format!("session at {updated_at}"));
        session.push_turn(Turn::new(Message::user_text("hola")), updated_at);
        session.push_turn(Turn::new(Message::user_text("¿y ahora?")), updated_at);
        let timeline = (0..blocks)
            .map(|i| {
                Block::new(match i % 4 {
                    0 => BlockKind::Text {
                        markdown: format!("respuesta **{i}**"),
                    },
                    1 => BlockKind::Thinking {
                        text: format!("pensando {i}"),
                        collapsed: true,
                    },
                    2 => BlockKind::ToolCall {
                        tool_use_id: format!("toolu_{i}"),
                        name: "bash".into(),
                        summary: "cargo test".into(),
                        input: serde_json::json!({ "command": "cargo test", "n": i }),
                        status: ToolStatus::Succeeded,
                        output: Some("ok".into()),
                    },
                    _ => BlockKind::Error {
                        message: format!("fallo {i}"),
                        hint: None,
                    },
                })
            })
            .collect();
        StoredSession {
            session,
            timeline,
            permission_rules: vec![PermissionRule::new(
                "bash",
                RuleMatcher::Prefix("cargo test".into()),
                RuleDecision::AllowAlways,
            )],
            prompt_history: vec!["hola".into(), "/model opus".into()],
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut conn = db::open_in_memory().unwrap();
        let stored = sample("/ws", 10, 100);
        save(&mut conn, &stored).unwrap();
        assert_eq!(load(&mut conn, stored.session.id).unwrap(), stored);
    }

    #[test]
    fn only_session_rules_are_saved() {
        let mut conn = db::open_in_memory().unwrap();
        let mut stored = sample("/ws", 1, 1);
        let session_rule = stored.permission_rules[0].clone();
        stored.permission_rules.insert(
            0,
            PermissionRule::new("run_command", RuleMatcher::Any, RuleDecision::Deny)
                .with_scope(RuleScope::Project),
        );
        save(&mut conn, &stored).unwrap();
        assert_eq!(
            load(&mut conn, stored.session.id).unwrap().permission_rules,
            [session_rule]
        );
    }

    #[test]
    fn saving_again_replaces_the_timeline() {
        let mut conn = db::open_in_memory().unwrap();
        let mut stored = sample("/ws", 10, 5);
        save(&mut conn, &stored).unwrap();
        stored.timeline.truncate(2);
        stored.permission_rules.clear();
        stored.session.title = Some("renamed".into());
        save(&mut conn, &stored).unwrap();
        assert_eq!(load(&mut conn, stored.session.id).unwrap(), stored);
        let blocks: i64 = conn
            .query_row("SELECT count(*) FROM timeline_blocks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(blocks, 2);
    }

    #[test]
    fn records_the_block_kind() {
        let mut conn = db::open_in_memory().unwrap();
        save(&mut conn, &sample("/ws", 1, 3)).unwrap();
        let mut stmt = conn
            .prepare("SELECT kind FROM timeline_blocks ORDER BY seq")
            .unwrap();
        let kinds: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kinds, ["text", "thinking", "tool_call"]);
    }

    #[test]
    fn loading_a_missing_session_is_not_found() {
        let mut conn = db::open_in_memory().unwrap();
        let id = SessionId::new();
        assert_eq!(load(&mut conn, id), Err(StoreError::NotFound(id)));
    }

    #[test]
    fn corrupt_session_json_is_reported() {
        let mut conn = db::open_in_memory().unwrap();
        let stored = sample("/ws", 1, 1);
        save(&mut conn, &stored).unwrap();
        conn.execute("UPDATE sessions SET data_json = '{oops'", [])
            .unwrap();
        let err = load(&mut conn, stored.session.id).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
    }

    #[test]
    fn corrupt_block_json_is_reported() {
        let mut conn = db::open_in_memory().unwrap();
        let stored = sample("/ws", 1, 3);
        save(&mut conn, &stored).unwrap();
        conn.execute(
            "UPDATE timeline_blocks SET data_json = '{\"type\":\"nope\"}' WHERE seq = 1",
            [],
        )
        .unwrap();
        let err = load(&mut conn, stored.session.id).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
    }

    #[test]
    fn a_failed_save_keeps_the_previous_version() {
        let mut conn = db::open_in_memory().unwrap();
        let stored = sample("/ws", 1, 3);
        save(&mut conn, &stored).unwrap();
        // Make the rules insert fail half-way through the second save.
        conn.execute_batch(
            "CREATE TRIGGER no_rules BEFORE INSERT ON permission_rules
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
        let mut changed = stored.clone();
        changed.timeline.clear();
        changed.session.title = Some("changed".into());
        assert!(matches!(
            save(&mut conn, &changed),
            Err(StoreError::Backend(_))
        ));
        conn.execute_batch("DROP TRIGGER no_rules").unwrap();
        assert_eq!(load(&mut conn, stored.session.id).unwrap(), stored);
    }

    // --- through the port ------------------------------------------------

    use application::testing::system::InMemorySessionStore;

    fn port(store: SqliteSessionStore) -> Arc<dyn SessionStorePort> {
        Arc::new(store)
    }

    #[tokio::test]
    async fn a_session_with_100_blocks_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let stored = sample("/ws", 42, 100);
        port(SqliteSessionStore::open(&path).unwrap())
            .save(&stored)
            .await
            .unwrap();

        let store = port(SqliteSessionStore::open(&path).unwrap());
        let loaded = store.load(stored.session.id).await.unwrap();
        assert_eq!(loaded.timeline.len(), 100);
        assert_eq!(loaded, stored);
    }

    #[tokio::test]
    async fn recent_workspaces_match_the_fake() {
        let sqlite = port(SqliteSessionStore::open_in_memory().unwrap());
        let fake: Arc<dyn SessionStorePort> = Arc::new(InMemorySessionStore::new());
        for (root, at) in [("/a", 5), ("/b", 9), ("/a", 12), ("/c", 1), ("/d", 9)] {
            let s = sample(root, at, 0);
            sqlite.save(&s).await.unwrap();
            fake.save(&s).await.unwrap();
        }
        let recent = sqlite.recent_workspaces(3).await.unwrap();
        let roots: Vec<&str> = recent.iter().map(|r| r.root.as_str()).collect();
        assert_eq!(roots, ["/a", "/b", "/d"]);
        assert_eq!(recent[0].updated_at, 12);
        assert_eq!(recent, fake.recent_workspaces(3).await.unwrap());
    }

    #[tokio::test]
    async fn list_matches_the_fake_order_with_ties() {
        let sqlite = port(SqliteSessionStore::open_in_memory().unwrap());
        let fake: Arc<dyn SessionStorePort> = Arc::new(InMemorySessionStore::new());
        let mut sessions = Vec::new();
        for (updated_at, created_at) in [(5, 1), (9, 2), (5, 3), (5, 3), (1, 1), (9, 2)] {
            let mut s = sample("/ws", updated_at, 0);
            s.session.created_at = created_at;
            sessions.push(s);
        }
        sessions.push(sample("/other", 100, 0));
        for s in &sessions {
            sqlite.save(s).await.unwrap();
            fake.save(s).await.unwrap();
        }

        let listed = sqlite.list("/ws").await.unwrap();
        assert_eq!(listed, fake.list("/ws").await.unwrap());
        let updated: Vec<u64> = listed.iter().map(|s| s.updated_at).collect();
        assert_eq!(updated, [9, 9, 5, 5, 5, 1]);
        assert_eq!(listed[0].turns, 2);
        assert!(sqlite.list("/none").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn latest_is_per_workspace() {
        let store = port(SqliteSessionStore::open_in_memory().unwrap());
        assert_eq!(store.latest_for_workspace("/a").await.unwrap(), None);
        let old = sample("/a", 1, 2);
        let new = sample("/a", 2, 3);
        let other = sample("/b", 99, 1);
        for s in [&old, &new, &other] {
            store.save(s).await.unwrap();
        }
        assert_eq!(store.latest_for_workspace("/a").await.unwrap(), Some(new));
        assert_eq!(store.latest_for_workspace("/b").await.unwrap(), Some(other));
    }

    #[tokio::test]
    async fn delete_removes_the_session_and_its_rows() {
        let store = SqliteSessionStore::open_in_memory().unwrap();
        let stored = sample("/ws", 1, 10);
        let id = stored.session.id;
        let dyn_store = port(store.clone());
        dyn_store.save(&stored).await.unwrap();
        dyn_store.delete(id).await.unwrap();

        assert_eq!(dyn_store.load(id).await, Err(StoreError::NotFound(id)));
        assert_eq!(dyn_store.delete(id).await, Err(StoreError::NotFound(id)));
        let orphans: i64 = store
            .run(|conn| {
                conn.query_row(
                    "SELECT (SELECT count(*) FROM timeline_blocks)
                          + (SELECT count(*) FROM permission_rules)",
                    [],
                    |r| r.get(0),
                )
                .map_err(sql_error)
            })
            .await
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[tokio::test]
    async fn corrupt_rows_surface_as_corrupt_through_the_port() {
        let store = SqliteSessionStore::open_in_memory().unwrap();
        let stored = sample("/ws", 1, 1);
        port(store.clone()).save(&stored).await.unwrap();
        store
            .run(|conn| {
                conn.execute("UPDATE permission_rules SET rule_json = 'null'", [])
                    .map_err(sql_error)
            })
            .await
            .unwrap();
        let err = port(store).latest_for_workspace("/ws").await.unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_saves_all_land() {
        let store = port(SqliteSessionStore::open_in_memory().unwrap());
        let tasks: Vec<_> = (0..16)
            .map(|i| {
                let store = Arc::clone(&store);
                tokio::spawn(async move { store.save(&sample("/ws", i, 5)).await })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(store.list("/ws").await.unwrap().len(), 16);
    }
}
