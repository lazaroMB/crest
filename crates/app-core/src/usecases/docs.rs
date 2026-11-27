//! Chat documents: create, read, save, append, rename, delete, and moving
//! with the chat (R03, spec 0003 sections 3 and 4).
//!
//! [`DocService`] is the one handle the UI, the agent's `doc_*` tools and
//! the organize use cases hold. The rows are in the session store (which is
//! also the [`DocStorePort`]) and the text in the files of a
//! [`DocFilesPort`]. Every change is announced on [`DocService::changes`]
//! so a panel showing the document can reload it and say who wrote.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use domain::conversation::{ProjectId, SessionId};
use domain::doc::{
    Doc, DocId, append_markdown, default_title, doc_dir, match_docs, replace_span, unique_file_name,
};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::stream::BoxStream;

use crate::ports::docs::{DocFilesError, DocFilesPort};
use crate::ports::store::{SessionStorePort, SessionSummary, StoreError};

/// Who wrote a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocAuthor {
    /// The person, from the panel.
    Person,
    /// The agent, through a `doc_*` tool.
    Agent,
}

/// One change of a document, sent to every subscriber of
/// [`DocService::changes`]: a write, create, rename, delete or relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocChange {
    /// The document.
    pub id: DocId,
    /// Its chat.
    pub session_id: SessionId,
    /// Who made the change.
    pub author: DocAuthor,
}

/// Why a query names no single document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocLookup {
    /// No document matches.
    None,
    /// This many documents match; the person must pick one.
    Several(usize),
}

/// Why a document operation failed; the messages are fit to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The files failed.
    #[error(transparent)]
    Files(#[from] DocFilesError),
    /// No document matches the query.
    #[error("no document matches “{0}”")]
    NotFound(String),
    /// Several documents match the query.
    #[error("{0} documents match; pick one")]
    Ambiguous(usize),
    /// The document belongs to another chat.
    #[error("that document belongs to another chat")]
    OtherChat,
    /// The passage to replace is not in the document.
    #[error("that passage is not in the document")]
    PassageNotFound,
    /// A chat outside a project needs crest's data folder, which is unknown.
    #[error("crest's data folder is not known, so this chat cannot keep documents")]
    NoDataDir,
}

/// The documents of every chat.
pub struct DocService {
    store: Arc<dyn SessionStorePort>,
    files: Arc<dyn DocFilesPort>,
    data_dir: Option<String>,
    now: fn() -> u64,
    subscribers: Mutex<Vec<UnboundedSender<DocChange>>>,
}

impl std::fmt::Debug for DocService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocService")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The single document `query` names among `docs`: its id, else the one
/// whose title equals it (ignoring case), else the only title that
/// contains it.
///
/// # Errors
///
/// [`DocLookup::None`] or [`DocLookup::Several`].
pub fn pick(docs: &[Doc], query: &str) -> Result<Doc, DocLookup> {
    let query = query.trim();
    if let Some(id) = DocId::parse(query)
        && let Some(doc) = docs.iter().find(|d| d.id == id)
    {
        return Ok(doc.clone());
    }
    let exact: Vec<&Doc> = docs
        .iter()
        .filter(|d| d.title.trim().eq_ignore_ascii_case(query))
        .collect();
    if let [only] = exact.as_slice() {
        return Ok((*only).clone());
    }
    match match_docs(docs, query).as_slice() {
        [] => Err(DocLookup::None),
        [only] => Ok((*only).clone()),
        many => Err(DocLookup::Several(many.len())),
    }
}

impl DocService {
    /// A service over the session store (also the doc store) and the files.
    /// `data_dir` is where chats outside a project keep their documents.
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStorePort>,
        files: Arc<dyn DocFilesPort>,
        data_dir: Option<String>,
        now: fn() -> u64,
    ) -> Self {
        Self {
            store,
            files,
            data_dir,
            now,
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// The session store this service moves chats with.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn SessionStorePort> {
        &self.store
    }

    /// Every document, newest first.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn list(&self) -> Result<Vec<Doc>, DocError> {
        Ok(self.store.docs().await?)
    }

    /// The documents of a chat, newest first.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn docs_of(&self, session: SessionId) -> Result<Vec<Doc>, DocError> {
        Ok(self.store.docs_of(session).await?)
    }

    /// The document `id`.
    ///
    /// # Errors
    ///
    /// [`StoreError::DocNotFound`] as a [`DocError::Store`].
    pub async fn doc(&self, id: DocId) -> Result<Doc, DocError> {
        Ok(self.store.doc(id).await?)
    }

    /// The documents whose title contains `query`, newest first.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn find(&self, query: &str) -> Result<Vec<Doc>, DocError> {
        let all = self.store.docs().await?;
        Ok(match_docs(&all, query).into_iter().cloned().collect())
    }

    /// The one document `query` names (see [`pick`]), among every chat's.
    ///
    /// # Errors
    ///
    /// [`DocError::NotFound`], [`DocError::Ambiguous`] or the store's error.
    pub async fn one(&self, query: &str) -> Result<Doc, DocError> {
        pick(&self.store.docs().await?, query).map_err(|lookup| lookup_error(lookup, query))
    }

    /// The one document of chat `session` that `query` names.
    ///
    /// # Errors
    ///
    /// As [`DocService::one`].
    pub async fn one_of(&self, session: SessionId, query: &str) -> Result<Doc, DocError> {
        pick(&self.store.docs_of(session).await?, query)
            .map_err(|lookup| lookup_error(lookup, query))
    }

    /// Subscribes to every change made through this service, from now on.
    pub fn changes(&self) -> BoxStream<'static, DocChange> {
        let (tx, rx) = unbounded();
        lock(&self.subscribers).push(tx);
        rx.boxed()
    }

    fn announce(&self, doc: &Doc, author: DocAuthor) {
        let change = DocChange {
            id: doc.id,
            session_id: doc.session_id,
            author,
        };
        lock(&self.subscribers).retain(|tx| tx.unbounded_send(change).is_ok());
    }

    fn dir_for(&self, session: SessionId, workspace_root: &str) -> Result<String, DocError> {
        let root = Some(workspace_root).filter(|r| !r.trim().is_empty());
        if root.is_none() && self.data_dir.is_none() {
            return Err(DocError::NoDataDir);
        }
        let data = self.data_dir.as_deref().unwrap_or_default();
        Ok(doc_dir(root, data, session))
    }

    /// A new empty document in chat `session.id`; `title` defaults to the
    /// chat's. The file exists from creation.
    ///
    /// # Errors
    ///
    /// The files' or the store's error; the file is removed again when the
    /// row cannot be written.
    pub async fn create(
        &self,
        session: &SessionSummary,
        title: Option<String>,
    ) -> Result<Doc, DocError> {
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default_title(session.title.as_deref()));
        self.create_in(
            session.id,
            &session.workspace_root,
            title,
            "",
            DocAuthor::Person,
        )
        .await
    }

    /// [`DocService::create`] for the chat `session_id` working in
    /// `workspace_root` (empty for a chat outside a project), looking the
    /// chat's title up in the store for the default title. The chat must
    /// have been saved.
    ///
    /// # Errors
    ///
    /// As [`DocService::create`].
    pub async fn create_for(
        &self,
        session_id: SessionId,
        workspace_root: &str,
        title: Option<String>,
    ) -> Result<Doc, DocError> {
        let chat_title = self
            .store
            .load(session_id)
            .await
            .ok()
            .and_then(|stored| stored.session.title);
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| default_title(chat_title.as_deref()));
        self.create_in(session_id, workspace_root, title, "", DocAuthor::Person)
            .await
    }

    /// Creates a document of chat `session_id` with `text`, as `author`: what
    /// the agent's `doc_write` does. The chat's directory comes from the
    /// saved chat.
    ///
    /// # Errors
    ///
    /// As [`DocService::create`]; [`StoreError::NotFound`] when the chat is
    /// not saved.
    pub async fn create_with(
        &self,
        session_id: SessionId,
        title: &str,
        text: &str,
        author: DocAuthor,
    ) -> Result<Doc, DocError> {
        let root = self.store.load(session_id).await?.session.workspace_root;
        let title = Some(title.trim()).filter(|t| !t.is_empty());
        let title = title.map_or_else(|| default_title(None), str::to_owned);
        self.create_in(session_id, &root, title, text, author).await
    }

    async fn create_in(
        &self,
        session_id: SessionId,
        workspace_root: &str,
        title: String,
        text: &str,
        author: DocAuthor,
    ) -> Result<Doc, DocError> {
        let dir = self.dir_for(session_id, workspace_root)?;
        let taken: Vec<String> = self
            .store
            .docs_of(session_id)
            .await?
            .into_iter()
            .map(|d| d.file_name)
            .collect();
        let file_name = unique_file_name(&title, &taken);
        let now = (self.now)();
        let doc = Doc {
            id: DocId::new(),
            session_id,
            title,
            file_name,
            dir,
            created_at: now,
            updated_at: now,
        };
        self.files.write(&doc.dir, &doc.file_name, text).await?;
        if let Err(err) = self.store.create_doc(doc.clone()).await {
            // Best effort: the row failed, so the empty file is an orphan.
            let _ = self.files.remove(&doc.dir, &doc.file_name).await;
            return Err(err.into());
        }
        self.announce(&doc, author);
        Ok(doc)
    }

    /// The document and its markdown.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn read(&self, id: DocId) -> Result<(Doc, String), DocError> {
        let doc = self.store.doc(id).await?;
        let text = self.files.read(&doc.dir, &doc.file_name).await?;
        Ok((doc, text))
    }

    /// Replaces the text of `id` as the person.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn save(&self, id: DocId, text: &str) -> Result<Doc, DocError> {
        self.save_by(id, text, DocAuthor::Person).await
    }

    /// Replaces the text of `id`, recording `author` on the change feed.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn save_by(&self, id: DocId, text: &str, author: DocAuthor) -> Result<Doc, DocError> {
        let mut doc = self.store.doc(id).await?;
        self.files.write(&doc.dir, &doc.file_name, text).await?;
        let now = (self.now)();
        self.store.touch_doc(id, now).await?;
        doc.updated_at = now;
        self.announce(&doc, author);
        Ok(doc)
    }

    /// Appends `addition` to the document as its own section.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn append(&self, id: DocId, addition: &str) -> Result<Doc, DocError> {
        let (_, text) = self.read(id).await?;
        self.save(id, &append_markdown(&text, addition)).await
    }

    /// Replaces the passage `from` of the document with `to`, as `author`.
    ///
    /// # Errors
    ///
    /// [`DocError::PassageNotFound`], the store's or the files' error.
    pub async fn replace_passage(
        &self,
        id: DocId,
        from: &str,
        to: &str,
        author: DocAuthor,
    ) -> Result<Doc, DocError> {
        let (_, text) = self.read(id).await?;
        if domain::doc::find_span(&text, from).is_none() {
            return Err(DocError::PassageNotFound);
        }
        self.save_by(id, &replace_span(&text, from, to), author)
            .await
    }

    /// Renames the document and its file (unique among the chat's docs).
    ///
    /// # Errors
    ///
    /// The store's or the files' error; the old file stays when the new one
    /// cannot be written.
    pub async fn rename(&self, id: DocId, title: &str) -> Result<Doc, DocError> {
        let title = title.trim();
        let mut doc = self.store.doc(id).await?;
        let taken: Vec<String> = self
            .store
            .docs_of(doc.session_id)
            .await?
            .into_iter()
            .filter(|d| d.id != id)
            .map(|d| d.file_name)
            .collect();
        let file_name = unique_file_name(title, &taken);
        let now = (self.now)();
        if file_name != doc.file_name {
            let text = self.files.read(&doc.dir, &doc.file_name).await?;
            self.files.write(&doc.dir, &file_name, &text).await?;
            self.store.rename_doc(id, title, &file_name, now).await?;
            // The row points at the new file; a leftover old one is harmless.
            let _ = self.files.remove(&doc.dir, &doc.file_name).await;
        } else {
            self.store.rename_doc(id, title, &file_name, now).await?;
        }
        title.clone_into(&mut doc.title);
        doc.file_name = file_name;
        doc.updated_at = now;
        self.announce(&doc, DocAuthor::Person);
        Ok(doc)
    }

    /// Deletes the row of the document; its files stay.
    ///
    /// # Errors
    ///
    /// The store's error.
    pub async fn delete(&self, id: DocId) -> Result<(), DocError> {
        let doc = self.store.doc(id).await?;
        self.store.delete_doc(id).await?;
        self.announce(&doc, DocAuthor::Person);
        Ok(())
    }

    /// Stores an image in the document's `assets/` and returns the name it
    /// got: `name`, or `name-2.ext`, `name-3.ext`... when taken.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn write_asset(
        &self,
        id: DocId,
        name: &str,
        bytes: &[u8],
    ) -> Result<String, DocError> {
        let doc = self.store.doc(id).await?;
        let (stem, ext) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
            _ => (name, String::new()),
        };
        let mut stored = name.to_owned();
        let mut n = 2;
        // A zero limit tells a missing asset (NotFound) from a present one.
        while !matches!(
            self.files.read_asset(&doc.dir, &stored, 0).await,
            Err(DocFilesError::NotFound(_) | DocFilesError::BadName(_))
        ) {
            stored = format!("{stem}-{n}{ext}");
            n += 1;
        }
        self.files.write_asset(&doc.dir, &stored, bytes).await?;
        Ok(stored)
    }

    /// The bytes of an image of the document, at most `max`.
    ///
    /// # Errors
    ///
    /// The store's or the files' error.
    pub async fn read_asset(&self, id: DocId, name: &str, max: usize) -> Result<Vec<u8>, DocError> {
        let doc = self.store.doc(id).await?;
        Ok(self.files.read_asset(&doc.dir, name, max).await?)
    }

    /// Moves the files of every document of `session_id` to `new_dir`, then
    /// updates their `dir`. A failure leaves rows and files as they were.
    ///
    /// # Errors
    ///
    /// The files' or the store's error.
    pub async fn relocate_session_docs(
        &self,
        session_id: SessionId,
        new_dir: &str,
    ) -> Result<(), DocError> {
        let docs = self.store.docs_of(session_id).await?;
        let new = new_dir.trim_end_matches('/');
        let mut moved: Vec<&str> = Vec::new();
        for doc in &docs {
            let old = doc.dir.trim_end_matches('/');
            if old == new || moved.contains(&old) {
                continue;
            }
            if let Err(err) = self.files.relocate(old, new).await {
                for back in moved {
                    // Undo what moved; a failure here cannot be reported twice.
                    let _ = self.files.relocate(new, back).await;
                }
                return Err(err.into());
            }
            moved.push(old);
        }
        for doc in docs.iter().filter(|d| d.dir.trim_end_matches('/') != new) {
            self.store.set_doc_dir(doc.id, new).await?;
            self.announce(doc, DocAuthor::Person);
        }
        Ok(())
    }

    /// Moves chat `id` into `project` (or out of every project) and its
    /// documents to the folder of the new first directory.
    ///
    /// # Errors
    ///
    /// The store's error, or the relocation's; the chat has moved when only
    /// the relocation failed, and its documents stay where they were.
    pub async fn move_session(
        &self,
        id: SessionId,
        project: Option<ProjectId>,
    ) -> Result<(), DocError> {
        self.store.move_session(id, project).await?;
        self.follow(id).await
    }

    /// Removes directory `root` from `project`; when it was the first, the
    /// documents of the project's chats move to the new first directory.
    ///
    /// # Errors
    ///
    /// The store's error, or the relocation's.
    pub async fn remove_root(
        &self,
        project: ProjectId,
        root: &str,
    ) -> Result<domain::project::Project, DocError> {
        let updated = self.store.remove_root(project, root).await?;
        let chats = self.store.list_all().await?;
        for chat in chats.iter().filter(|c| c.project_id == Some(project)) {
            self.follow(chat.id).await?;
        }
        Ok(updated)
    }

    /// Moves the documents of `id` to where its workspace root says.
    async fn follow(&self, id: SessionId) -> Result<(), DocError> {
        if self.store.docs_of(id).await?.is_empty() {
            return Ok(());
        }
        let root = self.store.load(id).await?.session.workspace_root;
        let dir = self.dir_for(id, &root)?;
        self.relocate_session_docs(id, &dir).await
    }
}

fn lookup_error(lookup: DocLookup, query: &str) -> DocError {
    match lookup {
        DocLookup::None => DocError::NotFound(query.trim().to_owned()),
        DocLookup::Several(n) => DocError::Ambiguous(n),
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::Session;

    use super::*;
    use crate::ports::docs::DocBases;
    use crate::ports::store::{OrganizeStorePort as _, ProjectStorePort as _, StoredSession};
    use crate::testing::docs::InMemoryDocFiles;
    use crate::testing::system::InMemorySessionStore;

    fn now() -> u64 {
        1_000
    }

    struct Rig {
        store: Arc<InMemorySessionStore>,
        files: Arc<InMemoryDocFiles>,
        docs: DocService,
    }

    fn rig() -> Rig {
        let store = Arc::new(InMemorySessionStore::new());
        let files = Arc::new(InMemoryDocFiles::new(DocBases {
            roots: vec!["/w/api".into(), "/w/web".into(), "/w/lib".into()],
            data_dir: Some("/data".into()),
        }));
        let docs = DocService::new(store.clone(), files.clone(), Some("/data".into()), now);
        Rig { store, files, docs }
    }

    async fn chat(store: &InMemorySessionStore, root: &str) -> SessionSummary {
        let mut session = Session::new(root, 1);
        session.title = Some("Release".into());
        let stored = StoredSession::new(session);
        store.save(stored.clone()).await.unwrap();
        stored.summary()
    }

    #[tokio::test]
    async fn a_project_chat_keeps_its_docs_in_the_root_and_a_standalone_one_in_the_data_dir() {
        let r = rig();
        let project = chat(&r.store, "/w/api").await;
        let alone = chat(&r.store, "").await;
        let a = r.docs.create(&project, Some("Notes".into())).await.unwrap();
        let b = r.docs.create(&alone, Some("Notes".into())).await.unwrap();
        assert_eq!(
            a.path(),
            format!("/w/api/.crest/docs/{}/notes.md", project.id)
        );
        assert_eq!(b.path(), format!("/data/docs/{}/notes.md", alone.id));
        assert_eq!(r.docs.read(a.id).await.unwrap().1, "");
        assert_eq!(r.files.files().len(), 2, "the files exist from creation");
        let default = r.docs.create(&project, None).await.unwrap();
        assert_eq!(default.title, "Release · notes");
    }

    #[tokio::test]
    async fn two_docs_with_one_title_get_distinct_files() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let first = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        let second = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        assert_eq!(first.file_name, "plan.md");
        assert_eq!(second.file_name, "plan-2.md");
        assert_eq!(r.docs.docs_of(chat.id).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn create_for_reads_the_chat_title_and_a_standalone_chat_needs_the_data_dir() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let doc = r.docs.create_for(chat.id, "/w/api", None).await.unwrap();
        assert_eq!(doc.title, "Release · notes");
        let no_data = DocService::new(r.store.clone(), r.files.clone(), None, now);
        let alone = self::chat(&r.store, "").await;
        assert_eq!(
            no_data.create(&alone, None).await.unwrap_err(),
            DocError::NoDataDir
        );
    }

    #[tokio::test]
    async fn save_append_and_replace_write_the_file_and_touch_the_row() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "one two").await.unwrap();
        r.docs.append(doc.id, "three").await.unwrap();
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "one two\n\nthree\n");
        r.docs
            .replace_passage(doc.id, "two", "2", DocAuthor::Agent)
            .await
            .unwrap();
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "one 2\n\nthree\n");
        let missing = r
            .docs
            .replace_passage(doc.id, "absent", "x", DocAuthor::Agent)
            .await;
        assert_eq!(missing.unwrap_err(), DocError::PassageNotFound);
        assert_eq!(r.docs.doc(doc.id).await.unwrap().updated_at, 1_000);
    }

    #[tokio::test]
    async fn rename_renames_the_file_and_keeps_the_text() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "text").await.unwrap();
        let other = r.docs.create(&chat, Some("Final".into())).await.unwrap();
        let renamed = r.docs.rename(doc.id, "Final").await.unwrap();
        assert_eq!(
            (renamed.title.as_str(), renamed.file_name.as_str()),
            ("Final", "final-2.md")
        );
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "text");
        let names: Vec<String> = r.files.files().into_iter().map(|f| f.1).collect();
        assert_eq!(names, ["final-2.md", "final.md"]);
        assert_eq!(other.file_name, "final.md");
    }

    #[tokio::test]
    async fn changes_name_the_author() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let mut feed = r.docs.changes();
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "x").await.unwrap();
        r.docs.save_by(doc.id, "y", DocAuthor::Agent).await.unwrap();
        let mut got = Vec::new();
        for _ in 0..3 {
            let change = feed.next().await.unwrap();
            assert_eq!((change.id, change.session_id), (doc.id, chat.id));
            got.push(change.author);
        }
        assert_eq!(
            got,
            [DocAuthor::Person, DocAuthor::Person, DocAuthor::Agent]
        );
    }

    #[tokio::test]
    async fn assets_get_unique_names() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        assert_eq!(
            r.docs.write_asset(doc.id, "p.png", &[1]).await.unwrap(),
            "p.png"
        );
        assert_eq!(
            r.docs.write_asset(doc.id, "p.png", &[2]).await.unwrap(),
            "p-2.png"
        );
        assert_eq!(r.docs.read_asset(doc.id, "p-2.png", 10).await.unwrap(), [2]);
    }

    async fn project_and_chat(
        r: &Rig,
        roots: &[&str],
    ) -> (domain::project::Project, SessionSummary) {
        let roots: Vec<String> = roots.iter().map(|s| (*s).to_owned()).collect();
        let project = r.store.create_project("p", &roots).await.unwrap();
        let chat = chat(&r.store, "").await;
        r.docs
            .move_session(chat.id, Some(project.id))
            .await
            .unwrap();
        (project, chat)
    }

    #[tokio::test]
    async fn moving_a_chat_moves_its_docs_and_dir() {
        let r = rig();
        let chat = chat(&r.store, "").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "text").await.unwrap();
        let project = r
            .store
            .create_project("p", &["/w/web".into()])
            .await
            .unwrap();
        r.docs
            .move_session(chat.id, Some(project.id))
            .await
            .unwrap();
        let moved = r.docs.doc(doc.id).await.unwrap();
        assert_eq!(moved.dir, format!("/w/web/.crest/docs/{}", chat.id));
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "text");
        assert_eq!(r.files.files().len(), 1);
        r.docs.move_session(chat.id, None).await.unwrap();
        assert_eq!(
            r.docs.doc(doc.id).await.unwrap().dir,
            format!("/data/docs/{}", chat.id)
        );
    }

    #[tokio::test]
    async fn removing_the_first_root_moves_docs_and_a_later_root_does_nothing() {
        let r = rig();
        let (project, chat) = project_and_chat(&r, &["/w/api", "/w/web", "/w/lib"]).await;
        let doc = r
            .docs
            .create_for(chat.id, "/w/api", Some("Plan".into()))
            .await
            .unwrap();
        r.docs.save(doc.id, "text").await.unwrap();
        r.docs.remove_root(project.id, "/w/lib").await.unwrap();
        assert_eq!(
            r.docs.doc(doc.id).await.unwrap().dir,
            format!("/w/api/.crest/docs/{}", chat.id)
        );
        r.docs.remove_root(project.id, "/w/api").await.unwrap();
        let moved = r.docs.doc(doc.id).await.unwrap();
        assert_eq!(moved.dir, format!("/w/web/.crest/docs/{}", chat.id));
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "text");
    }

    #[tokio::test]
    async fn a_failed_relocation_changes_neither_dir_nor_files() {
        let r = rig();
        let chat = chat(&r.store, "").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "text").await.unwrap();
        let project = r
            .store
            .create_project("p", &["/w/web".into()])
            .await
            .unwrap();
        r.files.fail_relocate(true);
        let before = r.files.files();
        let err = r
            .docs
            .move_session(chat.id, Some(project.id))
            .await
            .unwrap_err();
        assert!(matches!(err, DocError::Files(_)), "{err:?}");
        assert_eq!(r.docs.doc(doc.id).await.unwrap().dir, doc.dir);
        assert_eq!(r.files.files(), before);
    }

    #[tokio::test]
    async fn deleting_the_chat_drops_the_rows_and_keeps_the_files_and_a_branch_has_none() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let doc = r.docs.create(&chat, Some("Plan".into())).await.unwrap();
        r.docs.save(doc.id, "text").await.unwrap();
        let branch = r.store.branch_session(chat.id).await.unwrap();
        assert!(r.docs.docs_of(branch).await.unwrap().is_empty());
        r.store.delete(chat.id).await.unwrap();
        assert!(r.docs.list().await.unwrap().is_empty());
        assert_eq!(r.files.files().len(), 1);
    }

    #[tokio::test]
    async fn a_query_names_one_doc_or_says_why_not() {
        let r = rig();
        let chat = chat(&r.store, "/w/api").await;
        let notes = r
            .docs
            .create(&chat, Some("Release notes".into()))
            .await
            .unwrap();
        r.docs
            .create(&chat, Some("Release plan".into()))
            .await
            .unwrap();
        assert_eq!(r.docs.one("notes").await.unwrap().id, notes.id);
        assert_eq!(
            r.docs.one(&notes.id.to_string()).await.unwrap().id,
            notes.id
        );
        assert_eq!(
            r.docs.one("release plan").await.unwrap().title,
            "Release plan"
        );
        assert_eq!(
            r.docs.one("release").await.unwrap_err(),
            DocError::Ambiguous(2)
        );
        assert_eq!(
            r.docs.one("zzz").await.unwrap_err(),
            DocError::NotFound("zzz".into())
        );
        assert_eq!(pick(&[], "x"), Err(DocLookup::None));
        assert_eq!(r.docs.find("rel").await.unwrap().len(), 2);
    }
}
