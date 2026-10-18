//! Schema migrations, embedded in the binary.
//!
//! The applied version lives in `PRAGMA user_version`: migration `n` (1-based
//! position in [`MIGRATIONS`]) moves the database from version `n - 1` to `n`.
//! Each one runs in its own transaction together with the version bump, so a
//! crash leaves the database at the previous version, and running the runner
//! again is a no-op once everything is applied.
//!
//! Adding a migration: write `migrations/NNNN_name.sql` and append it here;
//! never edit an applied one. The graph-flow checkpoints of the investigative
//! flow (T5.1) are expected to come as the next migration, with their own
//! tables (e.g. `flow_sessions(id, session_id, data_json, updated_at)`) so
//! they do not touch the session tables.

use rusqlite::{Connection, TransactionBehavior};

/// One schema step.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// File name, for logs and errors.
    pub name: &'static str,
    /// SQL run as a batch.
    pub sql: &'static str,
}

/// Every migration, in order.
pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "0001_init",
    sql: include_str!("../migrations/0001_init.sql"),
}];

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
            ["permission_rules", "sessions", "timeline_blocks"]
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
}
