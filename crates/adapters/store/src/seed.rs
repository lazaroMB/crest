//! Test support: databases as an older build left them.

use std::path::Path;

use application_core::ports::store::StoreError;
use domain::conversation::SessionId;
use rusqlite::{Connection, params};

use crate::migrations::{MIGRATIONS, migrate_with};
use crate::session::tests::sample;

/// A database file migrated only to `version`, holding one session per
/// `(root, blocks)` row, written the way that version did (no project);
/// the first has `updated_at` 1. Returns the ids.
pub(crate) fn seed_at_version(
    path: &Path,
    version: usize,
    rows: &[(&str, usize)],
) -> Vec<SessionId> {
    let mut conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    migrate_with(&mut conn, &MIGRATIONS[..version]).unwrap();
    let mut ids = Vec::new();
    for (n, (root, blocks)) in (1_i64..).zip(rows) {
        let stored = sample(root, n.unsigned_abs(), *blocks);
        let session = &stored.session;
        conn.execute(
            "INSERT INTO sessions (id, workspace_root, title, created_at, updated_at,
                                   turn_count, data_json)
             VALUES (?1, ?2, ?3, ?4, ?4, 2, ?5)",
            params![
                session.id.to_string(),
                root,
                session.title,
                n,
                serde_json::to_string(session).unwrap()
            ],
        )
        .unwrap();
        for (seq, block) in (0_i64..).zip(&stored.timeline) {
            conn.execute(
                "INSERT INTO timeline_blocks VALUES (?1, ?2, ?3, 'k', ?4)",
                params![
                    session.id.to_string(),
                    seq,
                    block.id.to_string(),
                    serde_json::to_string(block).unwrap()
                ],
            )
            .unwrap();
        }
        ids.push(session.id);
    }
    ids
}

/// A store error as a short line with no ids, for transcripts.
pub(crate) fn tag(error: &StoreError) -> String {
    match error {
        StoreError::ProjectNotFound(_) => "no such project".into(),
        StoreError::NotFound(_) => "no such session".into(),
        other => other.to_string(),
    }
}
