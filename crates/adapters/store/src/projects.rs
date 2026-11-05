//! Projects in SQLite: the `projects` and `project_roots` tables, the data
//! step of migration 0007 and the project half of the session store.
//!
//! The rules (names, directories, the last directory) are
//! `domain::project`; this module reads a project, lets the domain change
//! it and writes the result, in one transaction.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use application_core::ports::store::StoreError;
use domain::conversation::{ProjectId, SessionId};
use domain::project::{Project, ProjectError, name_from_root, normalize_root};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::db::sql_error;

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

/// The directories one project is made of, while migrating.
struct Found {
    stored_as: Vec<String>,
    created_at: i64,
    updated_at: i64,
}

/// Data step of migration 0007: one project per directory that has sessions.
///
/// Directories are grouped by their normalized form (`/a/` and `/a` are one
/// project); a session with an empty directory stays standalone. A project
/// takes its timestamps from its oldest and newest session. Only
/// `projects`, `project_roots` and `sessions.project_id` are written.
pub(crate) fn backfill(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let mut stmt = tx.prepare(
        "SELECT workspace_root, MIN(created_at), MAX(updated_at)
         FROM sessions WHERE project_id IS NULL GROUP BY workspace_root",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (stored_as, created_at, updated_at) = row?;
        let Some(root) = normalize_root(&stored_as) else {
            continue;
        };
        let entry = found.entry(root).or_insert(Found {
            stored_as: Vec::new(),
            created_at,
            updated_at,
        });
        entry.stored_as.push(stored_as);
        entry.created_at = entry.created_at.min(created_at);
        entry.updated_at = entry.updated_at.max(updated_at);
    }
    drop(stmt);

    for (root, group) in found {
        let project = Project {
            id: ProjectId::new(),
            name: name_from_root(&root),
            pinned: false,
            roots: vec![root],
        };
        write(tx, &project, group.created_at, group.updated_at)?;
        for stored_as in &group.stored_as {
            tx.execute(
                "UPDATE sessions SET project_id = ?1
                 WHERE workspace_root = ?2 AND project_id IS NULL",
                params![project.id.to_string(), stored_as],
            )?;
        }
    }
    Ok(())
}

/// Inserts or updates the project row and makes its directories exactly
/// `project.roots`, in that order.
fn write(
    tx: &Transaction<'_>,
    project: &Project,
    created_at: i64,
    updated_at: i64,
) -> rusqlite::Result<()> {
    let id = project.id.to_string();
    tx.execute(
        "INSERT INTO projects (id, name, pinned, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET
             name = excluded.name, pinned = excluded.pinned, updated_at = excluded.updated_at",
        params![id, project.name, project.pinned, created_at, updated_at],
    )?;
    let mut keep = tx.prepare(
        "INSERT INTO project_roots (project_id, root, position) VALUES (?1, ?2, ?3)
         ON CONFLICT (project_id, root) DO UPDATE SET position = excluded.position",
    )?;
    for (position, root) in (0_i64..).zip(&project.roots) {
        keep.execute(params![id, root, position])?;
    }
    let placeholders = vec!["?"; project.roots.len()].join(", ");
    let sql =
        format!("DELETE FROM project_roots WHERE project_id = ?1 AND root NOT IN ({placeholders})");
    let args = std::iter::once(&id).chain(&project.roots);
    tx.execute(&sql, rusqlite::params_from_iter(args))?;
    Ok(())
}

/// Parses a project id column.
pub(crate) fn parse_project_id(id: &str) -> Result<ProjectId, StoreError> {
    ProjectId::parse(id).ok_or_else(|| StoreError::Corrupt(format!("project id {id:?}")))
}

/// Every project, in the order they were created.
pub(crate) fn list(conn: &Connection) -> Result<Vec<Project>, StoreError> {
    let mut roots: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT project_id, root FROM project_roots ORDER BY position, root")
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sql_error)?;
    for row in rows {
        let (id, root): (String, String) = row.map_err(sql_error)?;
        roots.entry(id).or_default().push(root);
    }
    let mut stmt = conn
        .prepare("SELECT id, name, pinned FROM projects ORDER BY rowid")
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(sql_error)?;
    rows.map(|row| {
        let (id, name, pinned) = row.map_err(sql_error)?;
        Ok(Project {
            id: parse_project_id(&id)?,
            name,
            pinned,
            roots: roots.remove(&id).unwrap_or_default(),
        })
    })
    .collect()
}

fn find(conn: &Connection, id: ProjectId) -> Result<Project, StoreError> {
    list(conn)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(StoreError::ProjectNotFound(id))
}

/// A write transaction, taking the lock now so nothing changes under it.
fn begin(conn: &mut Connection) -> Result<Transaction<'_>, StoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)
}

/// Creates a project.
pub(crate) fn create(
    conn: &mut Connection,
    name: &str,
    roots: &[String],
) -> Result<Project, StoreError> {
    let project = Project::new(name, roots)?;
    let tx = begin(conn)?;
    let now = now_ms();
    write(&tx, &project, now, now).map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(project)
}

/// Lets `change` edit project `id` and stores the result; nothing is stored
/// when the rules refuse the change.
pub(crate) fn edit(
    conn: &mut Connection,
    id: ProjectId,
    change: impl FnOnce(&mut Project) -> Result<(), ProjectError>,
) -> Result<Project, StoreError> {
    let tx = begin(conn)?;
    let mut project = find(&tx, id)?;
    let first_before = project.roots.first().cloned();
    change(&mut project)?;
    let created_at = tx
        .query_row(
            "SELECT created_at FROM projects WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    write(&tx, &project, created_at, now_ms()).map_err(sql_error)?;
    if project.roots.first() != first_before.as_ref() {
        // The first directory changed (the old one was removed): the
        // sessions in the project follow it, as `move_session` sets.
        tx.execute(
            "UPDATE sessions SET workspace_root = ?2 WHERE project_id = ?1",
            params![id.to_string(), first_root(&project)],
        )
        .map_err(sql_error)?;
    }
    tx.commit().map_err(sql_error)?;
    Ok(project)
}

/// The `workspace_root` of a session in `project`: its first directory.
fn first_root(project: &Project) -> String {
    project.roots.first().cloned().unwrap_or_default()
}

/// Sets the project of session `id`, and its `workspace_root` to the project's
/// first directory (empty without a project, as a standalone chat has). The
/// row is otherwise untouched, so its blocks and history stay.
pub(crate) fn move_session(
    conn: &mut Connection,
    id: SessionId,
    project: Option<ProjectId>,
) -> Result<(), StoreError> {
    let tx = begin(conn)?;
    let workspace_root = match project {
        Some(project) => first_root(&find(&tx, project)?),
        None => String::new(),
    };
    let moved = tx
        .execute(
            "UPDATE sessions SET project_id = ?2, workspace_root = ?3 WHERE id = ?1",
            params![
                id.to_string(),
                project.map(|p| p.to_string()),
                workspace_root
            ],
        )
        .map_err(sql_error)?;
    if moved == 0 {
        return Err(StoreError::NotFound(id));
    }
    tx.commit().map_err(sql_error)
}

#[cfg(test)]
mod tests {
    mod migration {
        use std::collections::BTreeMap;

        use super::super::*;
        use crate::db;
        use crate::migrations::{MIGRATIONS, migrate_with};
        use crate::session::load;
        use crate::session::tests::sample;

        /// Sessions of a store at the previous schema version: directory and
        /// number of blocks each.
        const SEEDED: [(&str, usize); 8] = [
            ("/work/api", 5),
            ("/work/api/", 2),
            ("/work/web", 0),
            ("/home/me/api", 7),
            ("/", 1),
            ("", 3),
            ("", 0),
            ("/work/api", 4),
        ];

        /// A database file at version 6 holding [`SEEDED`]; returns the ids.
        fn seed_previous_version(path: &std::path::Path) -> Vec<SessionId> {
            let mut conn = Connection::open(path).unwrap();
            conn.pragma_update(None, "foreign_keys", true).unwrap();
            migrate_with(&mut conn, &MIGRATIONS[..6]).unwrap();
            let mut ids = Vec::new();
            for (n, (root, blocks)) in (1_i64..).zip(SEEDED) {
                let stored = sample(root, n.unsigned_abs(), blocks);
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

        fn block_counts(conn: &Connection) -> BTreeMap<String, i64> {
            let mut stmt = conn
                .prepare(
                    "SELECT s.id, count(b.seq) FROM sessions s
                 LEFT JOIN timeline_blocks b ON b.session_id = s.id GROUP BY s.id",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        }

        /// Every seeded session still loads, keeps its directory, and is in the
        /// project of that directory (or in none when it had no directory).
        fn assert_sessions_follow_their_directory(
            conn: &mut Connection,
            ids: &[SessionId],
            all: &[Project],
        ) {
            for (id, (root, _)) in ids.iter().zip(SEEDED) {
                let session = load(conn, *id).unwrap().session;
                let project = session
                    .project_id
                    .map(|p| all.iter().find(|a| a.id == p).unwrap());
                let expected = normalize_root(root);
                assert_eq!(project.map(|p| p.roots[0].clone()), expected, "{root:?}");
                assert_eq!(session.workspace_root, root, "workspace_root is untouched");
            }
        }

        #[test]
        fn migrating_chats_makes_one_project_per_directory_and_loses_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sessions.db");
            let ids = seed_previous_version(&path);
            let before = block_counts(&Connection::open(&path).unwrap());
            assert_eq!(before.len(), SEEDED.len());

            // Opening migrates; opening again must change nothing.
            for _ in 0..2 {
                let mut conn = db::open(&path).unwrap();
                let all = list(&conn).unwrap();
                let named: Vec<(&str, &[String])> = all
                    .iter()
                    .map(|p| (p.name.as_str(), p.roots.as_slice()))
                    .collect();
                assert_eq!(
                    named,
                    [
                        ("/", &["/".to_owned()][..]),
                        ("api", &["/home/me/api".to_owned()]),
                        ("api", &["/work/api".to_owned()]),
                        ("web", &["/work/web".to_owned()]),
                    ]
                );
                assert!(all.iter().all(|p| !p.pinned));
                assert_eq!(block_counts(&conn), before);

                assert_sessions_follow_their_directory(&mut conn, &ids, &all);
                // Running the data step by hand again adds nothing.
                let tx = conn.transaction().unwrap();
                backfill(&tx).unwrap();
                tx.commit().unwrap();
                assert_eq!(list(&conn).unwrap(), all);
            }
        }
    }

    mod rules {
        use application_core::ports::store::{SessionStorePort, StoredSession};
        use application_testing::system::InMemorySessionStore;

        use super::super::*;
        use crate::session::SqliteSessionStore;
        use crate::session::tests::sample;

        fn tag(error: &StoreError) -> String {
            match error {
                StoreError::ProjectNotFound(_) => "no such project".into(),
                StoreError::NotFound(_) => "no such session".into(),
                other => other.to_string(),
            }
        }

        fn show(result: Result<Project, StoreError>) -> String {
            match result {
                Ok(p) => format!("{} {:?} pinned={}", p.name, p.roots, p.pinned),
                Err(error) => format!("refused: {}", tag(&error)),
            }
        }

        /// One session's project, root and block count as the store reports them.
        async fn where_is(store: &dyn SessionStorePort, session: &StoredSession) -> String {
            let loaded = store.load(session.session.id).await.unwrap();
            let projects = store.projects().await.unwrap();
            let name = loaded
                .session
                .project_id
                .and_then(|id| projects.iter().find(|p| p.id == id))
                .map(|p| p.name.as_str());
            format!(
                "{name:?} root={:?} blocks={}",
                loaded.session.workspace_root,
                loaded.timeline.len()
            )
        }

        /// Everything the project API does, as a transcript that does not depend
        /// on ids, so the SQLite store and the fake can be compared.
        async fn scenario(store: &dyn SessionStorePort) -> Vec<String> {
            let mut log = Vec::new();
            let api = edit_projects(store, &mut log).await;
            move_sessions(store, api, &mut log).await;
            log
        }

        fn roots(paths: &[&str]) -> Vec<String> {
            paths.iter().map(|p| (*p).to_owned()).collect()
        }

        /// Creates, edits and refuses; returns the project it ends with.
        async fn edit_projects(store: &dyn SessionStorePort, log: &mut Vec<String>) -> ProjectId {
            for (name, dirs) in [
                ("", &["/a"][..]),
                ("  ", &["/a"]),
                ("x", &[]),
                ("x", &[" "]),
            ] {
                log.push(show(store.create_project(name, &roots(dirs)).await));
            }
            let api = store
                .create_project(" api ", &roots(&["/src/api/", "/src/api"]))
                .await
                .unwrap();
            log.push(show(Ok(api.clone())));
            for root in ["/other/lib", "/third/docs/", "/other/lib/", " "] {
                log.push(show(store.add_root(api.id, root).await));
            }
            for root in ["/nowhere", "/src/api/", "/other/lib", "/third/docs"] {
                log.push(show(store.remove_root(api.id, root).await));
            }
            log.push(show(store.rename_project(api.id, " ").await));
            log.push(show(store.rename_project(api.id, "core").await));
            log.push(show(store.pin_project(api.id, true).await));
            log.push(show(store.pin_project(api.id, true).await));
            log.push(show(store.add_root(ProjectId::new(), "/x").await));
            api.id
        }

        async fn move_sessions(
            store: &dyn SessionStorePort,
            api: ProjectId,
            log: &mut Vec<String>,
        ) {
            let missing = ProjectId::new();
            let with_dir = sample("/src/api", 10, 8);
            let standalone = sample("", 11, 3);
            for s in [&with_dir, &standalone] {
                store.save(s.clone()).await.unwrap();
            }
            let docs = store
                .create_project("docs", &roots(&["/docs"]))
                .await
                .unwrap();
            // A new session naming a project that does not exist is standalone.
            let mut orphan = sample("/ws", 12, 1);
            orphan.session.project_id = Some(missing);
            store.save(orphan.clone()).await.unwrap();
            log.push(format!("orphan: {}", where_is(store, &orphan).await));
            for (session, target) in [
                (&standalone, Some(api)),
                (&with_dir, None),
                (&with_dir, Some(docs.id)),
                (&standalone, Some(missing)),
                (&standalone, None),
            ] {
                let moved = store.move_session(session.session.id, target).await;
                let outcome = moved.err().map_or("moved".into(), |e| tag(&e));
                log.push(format!("{outcome}: {}", where_is(store, session).await));
            }
            let moved = store.move_session(SessionId::new(), Some(docs.id)).await;
            log.push(tag(&moved.unwrap_err()));
            // A copy loaded before the move and saved after must not undo it.
            store.save(with_dir.clone()).await.unwrap();
            log.push(format!("saved stale: {}", where_is(store, &with_dir).await));
            // A project that loses its first directory moves its sessions to the
            // next one: `workspace_root` stays the project's first directory.
            store.add_root(docs.id, "/docs2").await.unwrap();
            store.remove_root(docs.id, "/docs").await.unwrap();
            let moved = where_is(store, &with_dir).await;
            log.push(format!("first directory removed: {moved}"));
            log.push(format!(
                "{} projects",
                store.projects().await.unwrap().len()
            ));
        }

        #[tokio::test]
        async fn projects_follow_their_rules_and_the_fake_does_too() {
            let sqlite = SqliteSessionStore::open_in_memory().unwrap();
            let real = scenario(&sqlite).await;
            let fake = scenario(&InMemorySessionStore::new()).await;
            let expected = [
                "refused: a project needs a name",
                "refused: a project needs a name",
                "refused: a project needs at least one directory",
                "refused: a project needs at least one directory",
                r#"api ["/src/api"] pinned=false"#,
                r#"api ["/src/api", "/other/lib"] pinned=false"#,
                r#"api ["/src/api", "/other/lib", "/third/docs"] pinned=false"#,
                r#"api ["/src/api", "/other/lib", "/third/docs"] pinned=false"#,
                "refused: a directory path cannot be empty",
                r#"api ["/src/api", "/other/lib", "/third/docs"] pinned=false"#,
                r#"api ["/other/lib", "/third/docs"] pinned=false"#,
                r#"api ["/third/docs"] pinned=false"#,
                "refused: a project keeps at least one directory",
                "refused: a project needs a name",
                r#"core ["/third/docs"] pinned=false"#,
                r#"core ["/third/docs"] pinned=true"#,
                r#"core ["/third/docs"] pinned=true"#,
                "refused: no such project",
                r#"orphan: None root="/ws" blocks=1"#,
                r#"moved: Some("core") root="/third/docs" blocks=3"#,
                r#"moved: None root="" blocks=8"#,
                r#"moved: Some("docs") root="/docs" blocks=8"#,
                r#"no such project: Some("core") root="/third/docs" blocks=3"#,
                r#"moved: None root="" blocks=3"#,
                "no such session",
                r#"saved stale: Some("docs") root="/docs" blocks=8"#,
                r#"first directory removed: Some("docs") root="/docs2" blocks=8"#,
                "2 projects",
            ];
            assert_eq!(real, expected);
            assert_eq!(fake, expected);
        }
    }
}
