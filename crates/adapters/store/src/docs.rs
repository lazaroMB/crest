//! The document half of the SQLite store: where each chat's docs are.
//!
//! One row of `docs` per document. The text is a file; the row keeps the
//! folder so a doc stays where it was written until it is moved on purpose.
//! Deleting a chat cascades to its rows (foreign keys are on) and touches no
//! file; a branch copies no row.

use application_core::ports::store::{DocStorePort, StoreError};
use async_trait::async_trait;
use domain::conversation::SessionId;
use domain::doc::{Doc, DocId};
use rusqlite::{Connection, Row, params};

use crate::db::sql_error;
use crate::session::{SqliteSessionStore, from_i64, parse_id};

const COLUMNS: &str = "id, session_id, title, file_name, dir, created_at, updated_at";

fn parse_doc_id(text: &str) -> Result<DocId, StoreError> {
    DocId::parse(text).ok_or_else(|| StoreError::Corrupt(format!("doc id {text:?}")))
}

/// Raw columns of one row, converted outside the rusqlite closure so a bad
/// value is `Corrupt` rather than a SQLite error.
type RawDoc = (String, String, String, String, String, i64, i64);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawDoc> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn build(
    (id, session, title, file_name, dir, created, updated): RawDoc,
) -> Result<Doc, StoreError> {
    Ok(Doc {
        id: parse_doc_id(&id)?,
        session_id: parse_id(&session)?,
        title,
        file_name,
        dir,
        created_at: from_i64(created, "doc created_at")?,
        updated_at: from_i64(updated, "doc updated_at")?,
    })
}

fn query(conn: &Connection, filter: &str, args: &[&str]) -> Result<Vec<Doc>, StoreError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM docs {filter} ORDER BY updated_at DESC, created_at DESC, id DESC"
        ))
        .map_err(sql_error)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(args), raw)
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    rows.into_iter().map(build).collect()
}

/// Every doc, newest first.
pub(crate) fn list(conn: &Connection) -> Result<Vec<Doc>, StoreError> {
    query(conn, "", &[])
}

/// The docs of chat `session`, newest first.
pub(crate) fn list_of(conn: &Connection, session: SessionId) -> Result<Vec<Doc>, StoreError> {
    query(conn, "WHERE session_id = ?1", &[&session.to_string()])
}

/// The doc `id`.
pub(crate) fn get(conn: &Connection, id: DocId) -> Result<Doc, StoreError> {
    query(conn, "WHERE id = ?1", &[&id.to_string()])?
        .into_iter()
        .next()
        .ok_or(StoreError::DocNotFound(id))
}

/// Inserts `doc`; a second doc with the same `(dir, file_name)` or a chat
/// that does not exist is a backend error.
pub(crate) fn create(conn: &Connection, doc: &Doc) -> Result<(), StoreError> {
    let times = (
        i64::try_from(doc.created_at).unwrap_or(i64::MAX),
        i64::try_from(doc.updated_at).unwrap_or(i64::MAX),
    );
    conn.execute(
        "INSERT INTO docs (id, session_id, title, file_name, dir, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            doc.id.to_string(),
            doc.session_id.to_string(),
            doc.title,
            doc.file_name,
            doc.dir,
            times.0,
            times.1
        ],
    )
    .map_err(sql_error)?;
    Ok(())
}

fn changed(
    conn: &Connection,
    id: DocId,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<(), StoreError> {
    let n = conn.execute(sql, args).map_err(sql_error)?;
    if n == 0 {
        return Err(StoreError::DocNotFound(id));
    }
    Ok(())
}

fn ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[async_trait]
impl DocStorePort for SqliteSessionStore {
    async fn docs(&self) -> Result<Vec<Doc>, StoreError> {
        self.run(|conn| list(conn)).await
    }

    async fn docs_of(&self, session: SessionId) -> Result<Vec<Doc>, StoreError> {
        self.run(move |conn| list_of(conn, session)).await
    }

    async fn doc(&self, id: DocId) -> Result<Doc, StoreError> {
        self.run(move |conn| get(conn, id)).await
    }

    async fn create_doc(&self, doc: Doc) -> Result<(), StoreError> {
        self.run(move |conn| create(conn, &doc)).await
    }

    async fn rename_doc(
        &self,
        id: DocId,
        title: &str,
        file_name: &str,
        now: u64,
    ) -> Result<(), StoreError> {
        let (title, file_name) = (title.to_owned(), file_name.to_owned());
        self.run(move |conn| {
            changed(
                conn,
                id,
                "UPDATE docs SET title = ?2, file_name = ?3, updated_at = ?4 WHERE id = ?1",
                &[&id.to_string(), &title, &file_name, &ms(now)],
            )
        })
        .await
    }

    async fn touch_doc(&self, id: DocId, now: u64) -> Result<(), StoreError> {
        self.run(move |conn| {
            changed(
                conn,
                id,
                "UPDATE docs SET updated_at = ?2 WHERE id = ?1",
                &[&id.to_string(), &ms(now)],
            )
        })
        .await
    }

    async fn delete_doc(&self, id: DocId) -> Result<(), StoreError> {
        self.run(move |conn| {
            changed(
                conn,
                id,
                "DELETE FROM docs WHERE id = ?1",
                &[&id.to_string()],
            )
        })
        .await
    }

    async fn set_doc_dir(&self, id: DocId, dir: &str) -> Result<(), StoreError> {
        let dir = dir.to_owned();
        self.run(move |conn| {
            changed(
                conn,
                id,
                "UPDATE docs SET dir = ?2 WHERE id = ?1",
                &[&id.to_string(), &dir],
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use application_core::ports::store::{OrganizeStorePort, SessionStorePort};

    use super::*;
    use crate::seed::seed_at_version;
    use crate::session::tests::sample;

    fn doc(session: SessionId, title: &str, file: &str, at: u64) -> Doc {
        Doc {
            id: DocId::new(),
            session_id: session,
            title: title.into(),
            file_name: file.into(),
            dir: format!("/ws/.crest/docs/{session}"),
            created_at: at,
            updated_at: at,
        }
    }

    async fn store_with_chat() -> (SqliteSessionStore, SessionId) {
        let store = SqliteSessionStore::open_in_memory().unwrap();
        let chat = sample("/ws", 1, 2);
        store.save(chat.clone()).await.unwrap();
        (store, chat.session.id)
    }

    #[tokio::test]
    async fn migration_over_saved_sessions_changes_none_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let ids = seed_at_version(&path, 8, &[("/work/api", 4), ("/work/web", 2)]);
        let store = SqliteSessionStore::open(&path).unwrap();
        let first: Vec<_> = {
            let mut v = Vec::new();
            for id in &ids {
                v.push(store.load(*id).await.unwrap());
            }
            v
        };
        assert!(store.docs().await.unwrap().is_empty());
        drop(store);
        let again = SqliteSessionStore::open(&path).unwrap();
        for (id, loaded) in ids.iter().zip(&first) {
            assert_eq!(&again.load(*id).await.unwrap(), loaded);
        }
        assert_eq!(first[0].timeline.len(), 4);
        assert_eq!(first[1].timeline.len(), 2);
        assert!(again.docs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_list_get_rename_touch_dir_and_delete() {
        let (store, chat) = store_with_chat().await;
        let other = sample("/ws", 2, 1);
        store.save(other.clone()).await.unwrap();
        let a = doc(chat, "A", "a.md", 10);
        let b = doc(chat, "B", "b.md", 20);
        let c = doc(other.session.id, "C", "c.md", 15);
        for d in [&a, &b, &c] {
            store.create_doc(d.clone()).await.unwrap();
        }
        let ids = |docs: Vec<Doc>| docs.into_iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids(store.docs().await.unwrap()), [b.id, c.id, a.id]);
        assert_eq!(ids(store.docs_of(chat).await.unwrap()), [b.id, a.id]);
        assert_eq!(store.doc(a.id).await.unwrap(), a);

        store.rename_doc(a.id, "A2", "a2.md", 30).await.unwrap();
        let renamed = store.doc(a.id).await.unwrap();
        assert_eq!(
            (
                renamed.title.as_str(),
                renamed.file_name.as_str(),
                renamed.updated_at
            ),
            ("A2", "a2.md", 30)
        );
        assert_eq!(renamed.created_at, 10);
        assert_eq!(ids(store.docs_of(chat).await.unwrap()), [a.id, b.id]);

        store.touch_doc(b.id, 40).await.unwrap();
        assert_eq!(store.doc(b.id).await.unwrap().updated_at, 40);
        store
            .set_doc_dir(b.id, "/other/.crest/docs/x")
            .await
            .unwrap();
        assert_eq!(store.doc(b.id).await.unwrap().dir, "/other/.crest/docs/x");

        store.delete_doc(c.id).await.unwrap();
        assert_eq!(store.doc(c.id).await, Err(StoreError::DocNotFound(c.id)));
        assert_eq!(
            store.delete_doc(c.id).await,
            Err(StoreError::DocNotFound(c.id))
        );
        assert_eq!(
            store.touch_doc(c.id, 1).await,
            Err(StoreError::DocNotFound(c.id))
        );
        assert_eq!(
            store.rename_doc(c.id, "x", "x.md", 1).await,
            Err(StoreError::DocNotFound(c.id))
        );
        assert_eq!(
            store.set_doc_dir(c.id, "/x").await,
            Err(StoreError::DocNotFound(c.id))
        );
    }

    #[tokio::test]
    async fn a_second_doc_with_the_same_dir_and_file_is_refused() {
        let (store, chat) = store_with_chat().await;
        store.create_doc(doc(chat, "A", "a.md", 1)).await.unwrap();
        let err = store.create_doc(doc(chat, "A again", "a.md", 2)).await;
        assert!(matches!(err, Err(StoreError::Backend(_))), "{err:?}");
        assert_eq!(store.docs().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deleting_the_chat_drops_its_rows_only() {
        let (store, chat) = store_with_chat().await;
        let other = sample("/ws", 2, 1);
        store.save(other.clone()).await.unwrap();
        store.create_doc(doc(chat, "A", "a.md", 1)).await.unwrap();
        store.create_doc(doc(chat, "B", "b.md", 2)).await.unwrap();
        let kept = doc(other.session.id, "C", "c.md", 3);
        store.create_doc(kept.clone()).await.unwrap();
        store.delete(chat).await.unwrap();
        assert_eq!(store.docs().await.unwrap(), [kept]);
        assert!(store.docs_of(chat).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_doc_for_a_missing_chat_is_refused() {
        let store = SqliteSessionStore::open_in_memory().unwrap();
        let err = store
            .create_doc(doc(SessionId::new(), "A", "a.md", 1))
            .await;
        assert!(matches!(err, Err(StoreError::Backend(_))), "{err:?}");
    }

    #[tokio::test]
    async fn a_branch_has_no_docs() {
        let (store, chat) = store_with_chat().await;
        store.create_doc(doc(chat, "A", "a.md", 1)).await.unwrap();
        let branch = store.branch_session(chat).await.unwrap();
        assert!(store.docs_of(branch).await.unwrap().is_empty());
        assert_eq!(store.docs_of(chat).await.unwrap().len(), 1);
        assert_eq!(store.docs().await.unwrap().len(), 1);
    }
}
