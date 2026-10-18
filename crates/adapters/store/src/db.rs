//! Opening the database: connection settings and migrations.

use std::path::Path;
use std::time::Duration;

use application::ports::store::StoreError;
use rusqlite::Connection;

use crate::migrations::{self, MigrationError};

/// How long a statement waits for a lock held by another connection (for
/// example a second crest window on the same workspace) before failing.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens (creating it and its parent directories if needed) the database at
/// `path`, applies the connection settings and migrates it to the latest
/// schema.
///
/// # Errors
/// [`StoreError::Corrupt`] if the file is not a database this build can
/// read (garbage, or a newer schema), [`StoreError::Backend`] otherwise.
pub fn open(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| {
            StoreError::Backend(format!("cannot create {}: {err}", parent.display()))
        })?;
    }
    let mut conn = Connection::open(path).map_err(sql_error)?;
    // WAL lets readers proceed while a save is writing; NORMAL sync is safe
    // under WAL (a crash may lose the last commit, never corrupt the file).
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
        .map_err(sql_error)?;
    prepare(&mut conn)?;
    Ok(conn)
}

/// Opens a private in-memory database, migrated; for tests.
///
/// # Errors
/// As [`open`].
pub fn open_in_memory() -> Result<Connection, StoreError> {
    let mut conn = Connection::open_in_memory().map_err(sql_error)?;
    prepare(&mut conn)?;
    Ok(conn)
}

/// Settings every connection needs, then migrations.
fn prepare(conn: &mut Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT).map_err(sql_error)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sql_error)?;
    migrations::migrate(conn).map_err(|err| match err {
        MigrationError::TooNew { .. } => StoreError::Corrupt(err.to_string()),
        MigrationError::Sql { source, .. } => sql_error(source),
    })?;
    Ok(())
}

/// Maps a SQLite error: a file that is not a database (or is damaged) is
/// [`StoreError::Corrupt`]; anything else is [`StoreError::Backend`].
pub(crate) fn sql_error(err: rusqlite::Error) -> StoreError {
    use rusqlite::ffi::ErrorCode;
    match err.sqlite_error_code() {
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => {
            StoreError::Corrupt(err.to_string())
        }
        _ => StoreError::Backend(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma<T: rusqlite::types::FromSql>(conn: &Connection, name: &str) -> T {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn opens_a_new_file_with_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/dir/sessions.db");
        let conn = open(&path).unwrap();
        assert!(path.exists());
        assert_eq!(pragma::<String>(&conn, "journal_mode"), "wal");
        assert!(pragma::<bool>(&conn, "foreign_keys"));
        assert_eq!(pragma::<i64>(&conn, "busy_timeout"), 5000);
        assert_eq!(
            migrations::current_version(&conn).unwrap(),
            migrations::latest_version()
        );
    }

    #[test]
    fn reopening_keeps_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();
        assert_eq!(
            migrations::current_version(&conn).unwrap(),
            migrations::latest_version()
        );
    }

    #[test]
    fn a_file_that_is_not_a_database_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        std::fs::write(&path, vec![0x5a_u8; 4096]).unwrap();
        let err = open(&path).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
    }

    #[test]
    fn a_newer_schema_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", migrations::latest_version() + 1)
            .unwrap();
        drop(conn);
        let err = open(&path).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt(_)), "{err}");
    }
}
