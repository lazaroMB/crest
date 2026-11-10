//! The chat half of organizing the sidebar in SQLite: rename, pin, archive
//! and branch. None of them changes `updated_at` of the chat they act on,
//! except a branch, which is a new chat.

use application_core::ports::store::{StoreError, StoredSession};
use domain::conversation::{Session, SessionId};
use rusqlite::{Connection, TransactionBehavior, params};

use crate::db::sql_error;
use crate::projects::now_ms;
use crate::session::{from_json, load_in, save_in, to_json};

/// Sets the title of chat `id` in its column and in its JSON; an empty title
/// clears it.
pub(crate) fn rename(conn: &mut Connection, id: SessionId, title: &str) -> Result<(), StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    let key = id.to_string();
    let json: Option<String> = rusqlite::OptionalExtension::optional(tx.query_row(
        "SELECT data_json FROM sessions WHERE id = ?1",
        [&key],
        |row| row.get(0),
    ))
    .map_err(sql_error)?;
    let mut session: Session = from_json(&json.ok_or(StoreError::NotFound(id))?, "session")?;
    session.title = Some(title.trim().to_owned()).filter(|t| !t.is_empty());
    tx.execute(
        "UPDATE sessions SET title = ?2, data_json = ?3 WHERE id = ?1",
        params![key, session.title, to_json(&session)?],
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)
}

/// Sets the 0/1 column `flag` (`pinned` or `archived`, never user text) of
/// chat `id`.
pub(crate) fn set_flag(
    conn: &Connection,
    id: SessionId,
    flag: &'static str,
    on: bool,
) -> Result<(), StoreError> {
    let changed = conn
        .execute(
            &format!("UPDATE sessions SET {flag} = ?2 WHERE id = ?1"),
            params![id.to_string(), on],
        )
        .map_err(sql_error)?;
    if changed == 0 {
        return Err(StoreError::NotFound(id));
    }
    Ok(())
}

/// Copies chat `id` into a new one, in one transaction (`Session::branch`).
/// Permission rules granted to the original are not copied.
pub(crate) fn branch(conn: &mut Connection, id: SessionId) -> Result<SessionId, StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql_error)?;
    let source = load_in(&tx, id)?;
    let now = u64::try_from(now_ms()).unwrap_or_default();
    let copy = StoredSession {
        session: source.session.branch(source.timeline.len(), now),
        timeline: source.timeline,
        permission_rules: Vec::new(),
        prompt_history: source.prompt_history,
    };
    save_in(&tx, &copy)?;
    tx.commit().map_err(sql_error)?;
    Ok(copy.session.id)
}

#[cfg(test)]
mod tests {
    mod menu {
        use application_core::ports::store::{SessionStorePort, StoreError, StoredSession};
        use domain::conversation::{ProjectId, SessionId};

        use crate::seed::tag;
        use crate::session::{SqliteSessionStore, tests::sample};

        fn outcome<T>(result: Result<T, StoreError>) -> String {
            result.err().map_or("ok".into(), |e| tag(&e))
        }

        /// The chat menu's steps over one store, with a transcript that has no
        /// ids or clocks in it, so the SQLite store and the fake can be compared.
        struct Scenario<'a> {
            store: &'a dyn SessionStorePort,
            project: ProjectId,
            a: SessionId,
            b: SessionId,
            stale_b: StoredSession,
            log: Vec<String>,
        }

        impl<'a> Scenario<'a> {
            /// A project with chat `a` (8 blocks) and chat `b` (3 blocks).
            async fn new(store: &'a dyn SessionStorePort) -> Self {
                let api = ["/src/api".to_owned()];
                let project = store.create_project("api", &api).await.unwrap();
                let (a, b) = (sample("", 10, 8), sample("", 20, 3));
                for s in [&a, &b] {
                    store.save(s.clone()).await.unwrap();
                    let id = s.session.id;
                    store.move_session(id, Some(project.id)).await.unwrap();
                }
                Self {
                    store,
                    project: project.id,
                    a: a.session.id,
                    b: b.session.id,
                    stale_b: b,
                    log: Vec::new(),
                }
            }

            /// One chat as the store reports it, with the listing checked
            /// against it.
            async fn note(&mut self, label: &str, id: SessionId) {
                let stored = self.store.load(id).await.unwrap();
                let s = &stored.session;
                let listed = self.store.list_all().await.unwrap();
                let row = listed.iter().find(|row| row.id == id).unwrap();
                assert_eq!(
                    (row.pinned, row.archived, row.branched_at),
                    (s.pinned, s.archived, s.branched_at),
                    "the listing and the loaded chat agree"
                );
                let line = format!(
                    "{label}{:?} pinned={} archived={} root={:?} project={} blocks={} from={} at={:?} updated={}",
                    s.title,
                    s.pinned,
                    s.archived,
                    s.workspace_root,
                    s.project_id.is_some(),
                    stored.timeline.len(),
                    s.branched_from.is_some(),
                    s.branched_at,
                    if s.updated_at > 1_000_000 {
                        1
                    } else {
                        s.updated_at
                    },
                );
                self.log.push(line);
            }

            /// Names, pins and archive flags, and that a stale save keeps them.
            async fn flags(&mut self) {
                let (store, a, b) = (self.store, self.a, self.b);
                for title in ["  Fix bug ", "   "] {
                    self.log.push(outcome(store.rename_session(a, title).await));
                    self.note("", a).await;
                }
                self.log
                    .push(outcome(store.rename_session(SessionId::new(), "x").await));
                store.rename_session(a, "Fix bug").await.unwrap();

                store.pin_session(b, true).await.unwrap();
                store.set_session_archived(b, true).await.unwrap();
                // A copy loaded before and saved after must not undo either flag.
                store.save(self.stale_b.clone()).await.unwrap();
                self.note("b: ", b).await;
                let live = store.list("/src/api").await.unwrap().len();
                let latest = store.latest_for_workspace("/src/api").await.unwrap();
                let all = store.list_all().await.unwrap().len();
                let first = latest.is_some_and(|l| l.session.id == a);
                self.log
                    .push(format!("live={live} all={all} latest_is_a={first}"));
                store.set_session_archived(b, false).await.unwrap();
                store.pin_session(b, false).await.unwrap();
                self.note("b restored: ", b).await;
                let ghost = SessionId::new();
                self.log.push(outcome(store.pin_session(ghost, true).await));
                self.log
                    .push(outcome(store.set_session_archived(ghost, true).await));
            }

            /// Branches, and what deleting the original does to them.
            async fn branches(&mut self) -> SessionId {
                let store = self.store;
                let branch = store.branch_session(self.a).await.unwrap();
                self.note("branch: ", branch).await;
                self.note("source: ", self.a).await;
                let again = store.branch_session(branch).await.unwrap();
                self.note("branch of branch: ", again).await;
                let ghost = store.branch_session(SessionId::new()).await;
                self.log.push(outcome(ghost));
                store.delete(self.a).await.unwrap();
                self.note("source deleted: ", branch).await;
                branch
            }

            /// Archiving a project, then deleting it: its chats stay.
            async fn project_end(&mut self, branch: SessionId) {
                let (store, project) = (self.store, self.project);
                store.set_project_archived(project, true).await.unwrap();
                let archived = store.projects().await.unwrap()[0].archived;
                self.log.push(format!("project archived={archived}"));
                store.set_project_archived(project, false).await.unwrap();
                let ghost = ProjectId::new();
                self.log
                    .push(outcome(store.set_project_archived(ghost, true).await));

                store.set_session_archived(self.b, true).await.unwrap();
                let freed = store.delete_project(project).await.map_err(|e| tag(&e));
                self.log.push(format!("freed={freed:?}"));
                self.note("b: ", self.b).await;
                self.note("branch: ", branch).await;
                self.log.push(outcome(store.delete_project(project).await));
                let left = store.projects().await.unwrap().len();
                self.log.push(format!("{left} projects"));
            }

            async fn run(store: &'a dyn SessionStorePort) -> Vec<String> {
                let mut run = Self::new(store).await;
                run.flags().await;
                let branch = run.branches().await;
                run.project_end(branch).await;
                run.log
            }
        }

        #[tokio::test]
        async fn the_chat_menu_works_alike_in_sqlite_and_in_the_fake() {
            let real = Scenario::run(&SqliteSessionStore::open_in_memory().unwrap()).await;
            let fake =
                Scenario::run(&application_testing::system::InMemorySessionStore::new()).await;
            let expected = [
                r#"ok"#,
                r#"Some("Fix bug") pinned=false archived=false root="/src/api" project=true blocks=8 from=false at=None updated=10"#,
                r#"ok"#,
                r#"None pinned=false archived=false root="/src/api" project=true blocks=8 from=false at=None updated=10"#,
                r#"no such session"#,
                r#"b: Some("session at 20") pinned=true archived=true root="/src/api" project=true blocks=3 from=false at=None updated=20"#,
                r#"live=1 all=2 latest_is_a=true"#,
                r#"b restored: Some("session at 20") pinned=false archived=false root="/src/api" project=true blocks=3 from=false at=None updated=20"#,
                r#"no such session"#,
                r#"no such session"#,
                r#"branch: Some("Fix bug (branch)") pinned=false archived=false root="/src/api" project=true blocks=8 from=true at=Some(8) updated=1"#,
                r#"source: Some("Fix bug") pinned=false archived=false root="/src/api" project=true blocks=8 from=false at=None updated=10"#,
                r#"branch of branch: Some("Fix bug (branch) (branch)") pinned=false archived=false root="/src/api" project=true blocks=8 from=true at=Some(8) updated=1"#,
                r#"no such session"#,
                r#"source deleted: Some("Fix bug (branch)") pinned=false archived=false root="/src/api" project=true blocks=8 from=false at=Some(8) updated=1"#,
                r#"project archived=true"#,
                r#"no such project"#,
                r#"freed=Ok(3)"#,
                r#"b: Some("session at 20") pinned=false archived=true root="" project=false blocks=3 from=false at=None updated=20"#,
                r#"branch: Some("Fix bug (branch)") pinned=false archived=false root="" project=false blocks=8 from=false at=Some(8) updated=1"#,
                r#"no such project"#,
                r#"0 projects"#,
            ];
            assert_eq!(real, expected);
            assert_eq!(fake, expected);
        }
    }

    mod upgrade {
        use application_core::ports::store::{OrganizeStorePort, SessionStorePort};

        use crate::migrations::{current_version, latest_version};
        use crate::seed::seed_at_version;
        use crate::session::SqliteSessionStore;

        #[tokio::test]
        async fn a_database_at_version_7_upgrades_and_keeps_its_chats() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sessions.db");
            let ids = seed_at_version(&path, 7, &[("/work/api", 4), ("/work/api", 0)]);
            // Opening migrates; opening again must change nothing.
            for round in 0..2 {
                let store = SqliteSessionStore::open(&path).unwrap();
                let conn = rusqlite::Connection::open(&path).unwrap();
                let version = current_version(&conn).unwrap();
                assert_eq!((version, version >= 8), (latest_version(), true));
                let listed = store.list_all().await.unwrap();
                let archived = listed.iter().filter(|c| c.archived).count();
                assert_eq!((listed.len(), archived), (2, round), "round {round}");
                assert!(
                    listed
                        .iter()
                        .all(|c| !c.pinned && c.branched_from.is_none())
                );
                let branch = store.branch_session(ids[0]).await.unwrap();
                assert_eq!(store.load(branch).await.unwrap().timeline.len(), 4);
                store.delete(branch).await.unwrap();
                store
                    .set_session_archived(ids[1], round == 0)
                    .await
                    .unwrap();
                assert_eq!(store.load(ids[0]).await.unwrap().timeline.len(), 4);
            }
        }
    }
}
