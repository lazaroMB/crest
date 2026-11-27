//! The agent's tools for the chat's documents (R03, spec 0003 section 4):
//! `doc_list`, `doc_read`, `doc_write` and `doc_edit`.
//!
//! Every tool is scoped by the session of the call: a document of another
//! chat is refused. The two write tools are [`ToolEffect::Write`], so the
//! permission modes apply as for `edit_file`: ask shows the diff of the
//! markdown and waits, accept-edits applies. The write goes through the
//! [`DocService`], which announces it so the panel reloads the document
//! and names the agent as the author.

use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, DiffSpec};
use domain::doc::{Doc, DocId, file_name, find_span, replace_span};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use similar::TextDiff;

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolRun,
    TypedTool,
};
use crate::ports::approval::ApprovalPreview;
use crate::usecases::docs::{DocAuthor, DocError, DocService};

/// What the system prompt says about the document tools.
pub const DOC_TOOLS_PROMPT: &str = "\
Documents: the chat can have markdown documents the person reads and edits beside the conversation. \
`doc_list` shows this chat's documents, `doc_read` returns one (by id or title), `doc_write` creates a \
document or replaces all of one with `markdown`, and `doc_edit` replaces one passage (`from` to `to`). \
Prefer `doc_edit` for a change to an existing document. When the message names a document to change, \
change that one. A diagram is a fenced ```mermaid block in the markdown. The person sees each change \
as a diff and may have to approve it.";

const DIFF_CONTEXT: usize = 3;

/// The scope of a call: a document of another chat is refused.
async fn resolve(docs: &DocService, ctx: &ToolContext, query: &str) -> Result<Doc, ToolError> {
    if let Some(id) = DocId::parse(query.trim()) {
        let doc = docs.doc(id).await.map_err(tool_error)?;
        if doc.session_id != ctx.session_id {
            return Err(tool_error(DocError::OtherChat));
        }
        return Ok(doc);
    }
    docs.one_of(ctx.session_id, query).await.map_err(tool_error)
}

fn tool_error(err: DocError) -> ToolError {
    ToolError::Execution(err.to_string())
}

/// Unified diff from `old` (`None` for a new document) to `new`.
fn unified(path: &str, old: Option<&str>, new: &str) -> String {
    let old_name = old.map_or_else(|| "/dev/null".to_owned(), |_| format!("a/{path}"));
    TextDiff::from_lines(old.unwrap_or(""), new)
        .unified_diff()
        .context_radius(DIFF_CONTEXT)
        .header(&old_name, &format!("b/{path}"))
        .to_string()
}

fn diff_spec(path: &str, old: Option<&str>, new: &str) -> DiffSpec {
    DiffSpec {
        path: path.to_owned(),
        old_text: old.map(str::to_owned),
        new_text: new.to_owned(),
        unified: unified(path, old, new),
        approval: None,
    }
}

/// A call that writes through `run`, previewed as the document's diff.
fn write_call(spec: DiffSpec, run: ToolRun) -> PreparedCall {
    PreparedCall::new(spec.path.clone(), run)
        .with_preview(ApprovalPreview::Diff(spec.clone()))
        .with_block(BlockKind::Diff(spec))
}

/// Input of `doc_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocListRequest {}

/// `doc_list`: the documents of this chat.
pub struct DocListTool {
    docs: Arc<DocService>,
}

#[async_trait]
impl TypedTool for DocListTool {
    type Input = DocListRequest;

    fn name(&self) -> &str {
        "doc_list"
    }

    fn description(&self) -> &str {
        "Lists the documents of this chat: id, title and size. Use the id or the title with `doc_read`, `doc_edit` or `doc_write`."
    }

    fn effect(&self, _input: &DocListRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        _input: DocListRequest,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let docs = Arc::clone(&self.docs);
        let session = ctx.session_id;
        let run = async move {
            let all = docs.docs_of(session).await.map_err(tool_error)?;
            if all.is_empty() {
                return Ok(ToolOutput::text("This chat has no documents."));
            }
            let mut lines = Vec::new();
            for doc in all {
                let size = match docs.read(doc.id).await {
                    Ok((_, text)) => format!("{} bytes", text.len()),
                    Err(err) => format!("unreadable: {err}"),
                };
                lines.push(format!("{}  {}  ({size})", doc.id, doc.title));
            }
            Ok(ToolOutput::text(lines.join("\n")))
        };
        Ok(PreparedCall::new("documents", run.boxed()))
    }
}

/// Input of `doc_read`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocReadRequest {
    /// The document: its id, or its title (or part of it) when that names
    /// one document of this chat.
    pub doc: String,
}

/// `doc_read`: the markdown of a document of this chat.
pub struct DocReadTool {
    docs: Arc<DocService>,
}

#[async_trait]
impl TypedTool for DocReadTool {
    type Input = DocReadRequest;

    fn name(&self) -> &str {
        "doc_read"
    }

    fn description(&self) -> &str {
        "Returns the markdown of a document of this chat, by id or title. Read it before editing it."
    }

    fn effect(&self, _input: &DocReadRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: DocReadRequest,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let doc = resolve(&self.docs, ctx, &input.doc).await?;
        let docs = Arc::clone(&self.docs);
        let run = async move {
            let (_, text) = docs.read(doc.id).await.map_err(tool_error)?;
            Ok(ToolOutput::text(text))
        };
        Ok(PreparedCall::new(input.doc, run.boxed()))
    }
}

/// Input of `doc_write`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocWriteRequest {
    /// Title of the document. A document of this chat with this title is
    /// replaced; otherwise a new one is created.
    pub title: String,
    /// The whole markdown of the document.
    pub markdown: String,
}

/// `doc_write`: creates a document or replaces all of one.
pub struct DocWriteTool {
    docs: Arc<DocService>,
}

#[async_trait]
impl TypedTool for DocWriteTool {
    type Input = DocWriteRequest;

    fn name(&self) -> &str {
        "doc_write"
    }

    fn description(&self) -> &str {
        "Creates a document in this chat, or replaces all of the one with the same title, with `markdown` as its full text. Prefer `doc_edit` for a change to an existing document. The user sees the diff and may need to approve it."
    }

    fn effect(&self, _input: &DocWriteRequest) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: DocWriteRequest,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let session = ctx.session_id;
        let existing = self
            .docs
            .docs_of(session)
            .await
            .map_err(tool_error)?
            .into_iter()
            .find(|d| d.title.trim().eq_ignore_ascii_case(input.title.trim()));
        let old = match &existing {
            Some(doc) => Some(self.docs.read(doc.id).await.map_err(tool_error)?.1),
            None => None,
        };
        let path = existing
            .as_ref()
            .map_or_else(|| file_name(&input.title), Doc::path);
        let spec = diff_spec(&path, old.as_deref(), &input.markdown);
        let docs = Arc::clone(&self.docs);
        let lines = input.markdown.lines().count();
        let run = async move {
            let done = match existing {
                Some(doc) => docs
                    .save_by(doc.id, &input.markdown, DocAuthor::Agent)
                    .await
                    .map(|doc| {
                        format!(
                            "Updated document “{}” ({}, {lines} lines).",
                            doc.title, doc.id
                        )
                    }),
                None => docs
                    .create_with(session, &input.title, &input.markdown, DocAuthor::Agent)
                    .await
                    .map(|doc| {
                        format!(
                            "Created document “{}” ({}, {lines} lines).",
                            doc.title, doc.id
                        )
                    }),
            };
            Ok(done.map_or_else(|err| ToolOutput::error(err.to_string()), ToolOutput::text))
        };
        Ok(write_call(spec, run.boxed()))
    }
}

/// Input of `doc_edit`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocEditRequest {
    /// The document: its id, or its title.
    pub doc: String,
    /// The passage to replace, as it reads in the document. Marks such as
    /// `**` or a list bullet may be left out.
    pub from: String,
    /// The text that replaces it.
    pub to: String,
}

/// `doc_edit`: replaces one passage of a document of this chat.
pub struct DocEditTool {
    docs: Arc<DocService>,
}

#[async_trait]
impl TypedTool for DocEditTool {
    type Input = DocEditRequest;

    fn name(&self) -> &str {
        "doc_edit"
    }

    fn description(&self) -> &str {
        "Edits a document of this chat by replacing one passage, `from`, with `to`. `from` must be in the document (read it first); include enough surrounding text to be exact. The user sees the diff and may need to approve it."
    }

    fn effect(&self, _input: &DocEditRequest) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: DocEditRequest,
        ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let doc = resolve(&self.docs, ctx, &input.doc).await?;
        let (_, old) = self.docs.read(doc.id).await.map_err(tool_error)?;
        if find_span(&old, &input.from).is_none() {
            return Err(tool_error(DocError::PassageNotFound));
        }
        let new = replace_span(&old, &input.from, &input.to);
        let spec = diff_spec(&doc.path(), Some(&old), &new);
        let docs = Arc::clone(&self.docs);
        let run = async move {
            let done = docs
                .replace_passage(doc.id, &input.from, &input.to, DocAuthor::Agent)
                .await;
            Ok(match done {
                Ok(doc) => ToolOutput::text(format!("Edited document “{}”.", doc.title)),
                Err(err) => ToolOutput::error(err.to_string()),
            })
        };
        Ok(write_call(spec, run.boxed()))
    }
}

/// Registers `doc_list`, `doc_read`, `doc_write` and `doc_edit` over `docs`.
///
/// # Errors
///
/// [`RegisterError::Duplicate`] when one of them is already registered.
pub fn register_doc_tools(
    registry: &mut ToolRegistry,
    docs: Arc<DocService>,
) -> Result<(), RegisterError> {
    registry.register_typed(DocListTool {
        docs: Arc::clone(&docs),
    })?;
    registry.register_typed(DocReadTool {
        docs: Arc::clone(&docs),
    })?;
    registry.register_typed(DocWriteTool {
        docs: Arc::clone(&docs),
    })?;
    registry.register_typed(DocEditTool { docs })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use domain::conversation::{Session, SessionId, ToolUseId, TurnId};
    use domain::permission::{ActionRequest, Decision, PermissionMode, PermissionPolicy};
    use futures::StreamExt as _;
    use serde_json::{Value, json};

    use super::*;
    use crate::ports::docs::DocBases;
    use crate::ports::store::{SessionStorePort as _, StoredSession};
    use crate::testing::docs::InMemoryDocFiles;
    use crate::testing::system::InMemorySessionStore;
    use crate::tools::registry::{Tool, Typed};

    struct Rig {
        docs: Arc<DocService>,
        files: Arc<InMemoryDocFiles>,
        session: SessionId,
        store: Arc<InMemorySessionStore>,
    }

    async fn saved(store: &InMemorySessionStore) -> SessionId {
        let mut session = Session::new("/w/api", 1);
        session.title = Some("Release".into());
        let id = session.id;
        store.save(StoredSession::new(session)).await.unwrap();
        id
    }

    async fn rig() -> Rig {
        let store = Arc::new(InMemorySessionStore::new());
        let files = Arc::new(InMemoryDocFiles::new(DocBases {
            roots: vec!["/w/api".into()],
            data_dir: Some("/data".into()),
        }));
        let docs = Arc::new(DocService::new(
            store.clone(),
            files.clone(),
            Some("/data".into()),
            || 7,
        ));
        let session = saved(&store).await;
        Rig {
            docs,
            files,
            session,
            store,
        }
    }

    fn ctx(session: SessionId) -> ToolContext {
        ToolContext::new(session, TurnId::new(), ToolUseId::new("toolu_1"))
    }

    async fn prepare(tool: &dyn Tool, session: SessionId, input: Value) -> PreparedCall {
        tool.prepare(input, &ctx(session)).await.unwrap()
    }

    async fn plan(tool: &dyn Tool, session: SessionId, input: Value) -> ToolError {
        match tool.prepare(input, &ctx(session)).await {
            Err(err) => err,
            Ok(_) => panic!("expected the call to be refused"),
        }
    }

    fn tools(r: &Rig) -> (impl Tool, impl Tool, impl Tool, impl Tool) {
        let docs = || Arc::clone(&r.docs);
        (
            Typed(DocListTool { docs: docs() }),
            Typed(DocReadTool { docs: docs() }),
            Typed(DocWriteTool { docs: docs() }),
            Typed(DocEditTool { docs: docs() }),
        )
    }

    #[test]
    fn definitions_snapshot() {
        let store = Arc::new(InMemorySessionStore::new());
        let docs = Arc::new(DocService::new(
            store,
            Arc::new(InMemoryDocFiles::default()),
            None,
            || 0,
        ));
        let mut registry = ToolRegistry::new();
        register_doc_tools(&mut registry, docs).unwrap();
        let defs = registry.definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["doc_edit", "doc_list", "doc_read", "doc_write"]);
        for def in defs.iter() {
            insta::assert_json_snapshot!(def.name.clone(), def);
        }
        assert!(DOC_TOOLS_PROMPT.contains("doc_edit"));
    }

    #[tokio::test]
    async fn list_and_read_are_reads_and_see_only_this_chat() {
        let r = rig().await;
        let (list, read, ..) = tools(&r);
        let mine = r
            .docs
            .create_for(r.session, "/w/api", Some("Notes".into()))
            .await
            .unwrap();
        r.docs.save(mine.id, "# Notes\n").await.unwrap();
        let other = saved(&r.store).await;
        let theirs = r
            .docs
            .create_for(other, "/w/api", Some("Secret".into()))
            .await
            .unwrap();

        assert_eq!(list.effect(&json!({})), ToolEffect::Read);
        assert_eq!(read.effect(&json!({"doc": "x"})), ToolEffect::Read);
        let out = prepare(&list, r.session, json!({}))
            .await
            .run
            .await
            .unwrap();
        assert_eq!(out.text_content(), format!("{}  Notes  (8 bytes)", mine.id));

        let out = prepare(&read, r.session, json!({"doc": "notes"}))
            .await
            .run
            .await
            .unwrap();
        assert_eq!(out.text_content(), "# Notes\n");
        let by_id = json!({"doc": mine.id.to_string()});
        let out = prepare(&read, r.session, by_id).await.run.await.unwrap();
        assert_eq!(out.text_content(), "# Notes\n");

        let refused = plan(&read, r.session, json!({"doc": theirs.id.to_string()})).await;
        assert_eq!(
            refused,
            ToolError::Execution("that document belongs to another chat".into())
        );
        let by_title = plan(&read, r.session, json!({"doc": "Secret"})).await;
        assert!(matches!(by_title, ToolError::Execution(m) if m.contains("no document matches")));
    }

    #[tokio::test]
    async fn doc_write_previews_a_diff_and_creates_or_replaces_through_the_service() {
        let r = rig().await;
        let (.., write, _) = tools(&r);
        let mut feed = r.docs.changes();
        assert_eq!(
            write.effect(&json!({"title": "t", "markdown": "m"})),
            ToolEffect::Write
        );

        let call = prepare(
            &write,
            r.session,
            json!({"title": "Plan", "markdown": "# Plan\n"}),
        )
        .await;
        let Some(ApprovalPreview::Diff(spec)) = call.preview.clone() else {
            panic!("a diff preview");
        };
        assert_eq!(
            (spec.path.as_str(), spec.old_text.clone()),
            ("plan.md", None)
        );
        assert!(spec.unified.contains("+# Plan"), "{}", spec.unified);
        assert!(matches!(call.block, Some(BlockKind::Diff(_))));
        assert!(
            r.files.files().is_empty(),
            "nothing is written before the run"
        );
        let out = call.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(r.files.files().len(), 1);
        let change = feed.next().await.unwrap();
        assert_eq!(change.author, DocAuthor::Agent);

        let call = prepare(
            &write,
            r.session,
            json!({"title": "plan", "markdown": "# Plan\nmore\n"}),
        )
        .await;
        let Some(ApprovalPreview::Diff(spec)) = call.preview.clone() else {
            panic!("a diff preview");
        };
        assert_eq!(spec.old_text.as_deref(), Some("# Plan\n"));
        assert!(spec.unified.contains("+more"), "{}", spec.unified);
        call.run.await.unwrap();
        let docs = r.docs.docs_of(r.session).await.unwrap();
        assert_eq!(docs.len(), 1, "same title replaces");
        assert_eq!(r.docs.read(docs[0].id).await.unwrap().1, "# Plan\nmore\n");
    }

    #[tokio::test]
    async fn doc_edit_replaces_a_passage_and_refuses_a_missing_one_and_another_chat() {
        let r = rig().await;
        let (.., edit) = tools(&r);
        let doc = r
            .docs
            .create_for(r.session, "/w/api", Some("Plan".into()))
            .await
            .unwrap();
        r.docs.save(doc.id, "one **two** three\n").await.unwrap();
        assert_eq!(
            edit.effect(&json!({"doc": "d", "from": "a", "to": "b"})),
            ToolEffect::Write
        );

        let input = json!({"doc": "Plan", "from": "two", "to": "2"});
        let call = prepare(&edit, r.session, input).await;
        assert_eq!(call.subject, doc.path());
        let Some(ApprovalPreview::Diff(spec)) = call.preview.clone() else {
            panic!("a diff preview");
        };
        assert!(
            spec.unified.contains("-one **two** three"),
            "{}",
            spec.unified
        );
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "one **two** three\n");
        call.run.await.unwrap();
        assert_eq!(r.docs.read(doc.id).await.unwrap().1, "one **2** three\n");

        let missing = json!({"doc": "Plan", "from": "absent", "to": "x"});
        assert_eq!(
            plan(&edit, r.session, missing).await,
            ToolError::Execution("that passage is not in the document".into())
        );
        let other = saved(&r.store).await;
        let foreign = json!({"doc": doc.id.to_string(), "from": "one", "to": "x"});
        assert_eq!(
            plan(&edit, other, foreign).await,
            ToolError::Execution("that document belongs to another chat".into())
        );
    }

    #[tokio::test]
    async fn ask_mode_waits_on_writes_and_accept_edits_applies_them_and_reads_never_ask() {
        let r = rig().await;
        let (list, read, write, edit) = tools(&r);
        let doc = r
            .docs
            .create_for(r.session, "/w/api", Some("Plan".into()))
            .await
            .unwrap();
        let decide = |mode, tool: &dyn Tool, name: &str, input: Value, subject: String| {
            let effect = tool.effect(&input);
            (
                PermissionPolicy::new(mode).decide(&ActionRequest::new(name, effect, subject)),
                name.to_owned(),
            )
        };
        let write_in = json!({"title": "Plan", "markdown": "x"});
        let edit_in = json!({"doc": "Plan", "from": "a", "to": "b"});
        for (mode, writes) in [
            (PermissionMode::Ask, Decision::Ask),
            (PermissionMode::AutoEdit, Decision::Allow),
        ] {
            let got = [
                decide(mode, &write, "doc_write", write_in.clone(), doc.path()).0,
                decide(mode, &edit, "doc_edit", edit_in.clone(), doc.path()).0,
            ];
            assert_eq!(got, [writes.clone(), writes], "{mode:?}");
            for (tool, name, input) in [
                (&list as &dyn Tool, "doc_list", json!({})),
                (&read as &dyn Tool, "doc_read", json!({"doc": "Plan"})),
            ] {
                let (decision, _) = decide(mode, tool, name, input, "Plan".into());
                assert_eq!(decision, Decision::Allow, "{name} in {mode:?}");
            }
        }
    }
}
