//! Schema migrations, embedded in the binary.
//!
//! The applied version lives in `PRAGMA user_version`: migration `n` (1-based
//! position in [`MIGRATIONS`]) moves the database from version `n - 1` to `n`.
//! Each one runs in its own transaction together with the version bump, so a
//! crash leaves the database at the previous version, and running the runner
//! again is a no-op once everything is applied.
//!
//! Adding a migration: write `migrations/NNNN_name.sql` and append it here;
//! never edit an applied one.

use rusqlite::{Connection, Transaction, TransactionBehavior};

/// One schema step.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// File name, for logs and errors.
    pub name: &'static str,
    /// SQL run as a batch.
    pub sql: &'static str,
    /// Rust run in the same transaction after the SQL, for data a script
    /// cannot derive (backfilling from replayed logs).
    pub after: Option<fn(&Transaction<'_>) -> rusqlite::Result<()>>,
}

/// Every migration, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init",
        sql: include_str!("../migrations/0001_init.sql"),
        after: None,
    },
    Migration {
        name: "0002_turn_count",
        sql: include_str!("../migrations/0002_turn_count.sql"),
        after: None,
    },
    Migration {
        name: "0003_flow_sessions",
        sql: include_str!("../migrations/0003_flow_sessions.sql"),
        after: None,
    },
    Migration {
        name: "0004_prompt_history",
        sql: include_str!("../migrations/0004_prompt_history.sql"),
        after: None,
    },
    Migration {
        name: "0005_discovery",
        sql: include_str!("../migrations/0005_discovery.sql"),
        after: None,
    },
    Migration {
        name: "0006_discovery_summary",
        sql: include_str!("../migrations/0006_discovery_summary.sql"),
        after: Some(crate::discovery::backfill_summaries),
    },
];

/// Version a fully migrated database has.
#[must_use]
pub fn latest_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

/// Why migrating failed.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// The database was written by a newer crest.
    #[error("database schema version {found} is newer than the supported {supported}")]
    TooNew {
        /// Version found in the file.
        found: u32,
        /// Latest version this build knows.
        supported: u32,
    },
    /// SQLite failed while running a migration.
    #[error("migration {name} failed: {source}")]
    Sql {
        /// Migration that failed (or `user_version` when reading it).
        name: &'static str,
        /// Underlying error.
        source: rusqlite::Error,
    },
}

/// Current `user_version` of `conn`.
///
/// # Errors
/// If SQLite cannot read the pragma.
pub fn current_version(conn: &Connection) -> Result<u32, rusqlite::Error> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Applies the pending migrations of [`MIGRATIONS`] to `conn`; returns the
/// version reached.
///
/// # Errors
/// [`MigrationError::TooNew`] if the database is ahead of this build, or
/// [`MigrationError::Sql`] if a step fails (that step is rolled back).
pub fn migrate(conn: &mut Connection) -> Result<u32, MigrationError> {
    migrate_with(conn, MIGRATIONS)
}

/// [`migrate`] over an explicit list, so tests can stop at an older version.
///
/// # Errors
/// As [`migrate`].
pub fn migrate_with(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<u32, MigrationError> {
    let sql_err = |name| move |source| MigrationError::Sql { name, source };
    let supported = u32::try_from(migrations.len()).unwrap_or(u32::MAX);
    let found = current_version(conn).map_err(sql_err("user_version"))?;
    if found > supported {
        return Err(MigrationError::TooNew { found, supported });
    }
    for (version, migration) in (1..).zip(migrations).skip(found as usize) {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err(migration.name))?;
        tx.execute_batch(migration.sql)
            .and_then(|()| migration.after.map_or(Ok(()), |after| after(&tx)))
            .and_then(|()| tx.pragma_update(None, "user_version", version))
            .and_then(|()| tx.commit())
            .map_err(sql_err(migration.name))?;
        tracing::info!(
            migration = migration.name,
            version,
            "applied store migration"
        );
    }
    Ok(supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn migrates_an_empty_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        assert_eq!(current_version(&conn).unwrap(), latest_version());
        assert_eq!(
            tables(&conn),
            [
                "discoveries",
                "discovery_artifacts",
                "discovery_events",
                "discovery_sources",
                "flow_sessions",
                "permission_rules",
                "sessions",
                "timeline_blocks"
            ]
        );
    }

    #[test]
    fn running_twice_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        assert_eq!(current_version(&conn).unwrap(), latest_version());
    }

    #[test]
    fn refuses_a_newer_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", latest_version() + 1)
            .unwrap();
        let err = migrate(&mut conn).unwrap_err();
        assert!(matches!(err, MigrationError::TooNew { .. }), "{err}");
    }

    #[test]
    fn a_failing_step_is_rolled_back() {
        let broken = [
            MIGRATIONS[0],
            Migration {
                name: "broken",
                sql: "CREATE TABLE half_done (x); SELECT * FROM missing_table;",
                after: None,
            },
        ];
        let mut conn = Connection::open_in_memory().unwrap();
        let err = migrate_with(&mut conn, &broken).unwrap_err();
        assert!(
            matches!(err, MigrationError::Sql { name: "broken", .. }),
            "{err}"
        );
        assert_eq!(current_version(&conn).unwrap(), 1);
        assert!(!tables(&conn).contains(&"half_done".to_owned()));
    }

    #[test]
    fn migrates_a_version_1_database_with_data() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        assert_eq!(migrate_with(&mut conn, &MIGRATIONS[..1]).unwrap(), 1);
        conn.execute_batch(
            r#"
            INSERT INTO sessions VALUES
                ('a', '/ws', 'two turns', 1, 2, '{"id":"a","turns":[{},{}]}'),
                ('b', '/ws', NULL, 1, 3, '{"id":"b"}'),
                ('c', '/ws', NULL, 1, 4, 'not json');
            INSERT INTO timeline_blocks VALUES ('a', 0, 'blk', 'text', '{"type":"text"}');
            INSERT INTO permission_rules VALUES ('a', 0, '{"tool":"bash"}');
            "#,
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), latest_version());

        let mut stmt = conn
            .prepare("SELECT id, title, updated_at, turn_count FROM sessions ORDER BY id")
            .unwrap();
        let rows: Vec<(String, Option<String>, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                ("a".into(), Some("two turns".into()), 2, 2),
                ("b".into(), None, 3, 0),
                ("c".into(), None, 4, 0),
            ]
        );
        let blocks: i64 = conn
            .query_row("SELECT count(*) FROM timeline_blocks", [], |r| r.get(0))
            .unwrap();
        let rules: i64 = conn
            .query_row("SELECT count(*) FROM permission_rules", [], |r| r.get(0))
            .unwrap();
        assert_eq!((blocks, rules), (1, 1));
    }

    #[test]
    fn migrates_a_version_2_database_with_data() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        assert_eq!(migrate_with(&mut conn, &MIGRATIONS[..2]).unwrap(), 2);
        conn.execute_batch(
            r#"
            INSERT INTO sessions VALUES ('a', '/ws', 'kept', 1, 2, '{"id":"a","turns":[{}]}', 1);
            INSERT INTO timeline_blocks VALUES ('a', 0, 'blk', 'text', '{"type":"text"}');
            "#,
        )
        .unwrap();

        assert_eq!(migrate(&mut conn).unwrap(), latest_version());

        let row: (String, i64, i64) = conn
            .query_row(
                "SELECT title, turn_count, updated_at FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("kept".into(), 1, 2));
        let blocks: i64 = conn
            .query_row("SELECT count(*) FROM timeline_blocks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(blocks, 1);
        let flows: i64 = conn
            .query_row("SELECT count(*) FROM flow_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flows, 0);
    }
}
