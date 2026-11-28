//! Chat documents in the session: the panel beside (or in place of) the
//! chat, the `/doc` commands, "Add to doc", the references on the prompt
//! and the sidebar's way in.
//!
//! The text lives in files behind the [`DocService`]; the session reads a
//! document into the [`DocPanel`] and writes what the panel asks for. Each
//! operation is one task on the foreground executor that awaits the
//! service (its adapters hand the blocking work to their own runtime) and
//! then applies the result here; a failure is a warning notice.
//!
//! The panel shows the chat's document, which is remembered while the
//! session lives (a parked session keeps it) and is the latest one when a
//! chat opens. A change the agent writes ([`DocService::changes`]) reloads
//! the shown document and says "agent" as the author, and a document the
//! agent creates opens by itself.

use std::sync::Arc;

use application_core::keymap::Action;
use application_core::ports::events::NoticeLevel;
use application_core::usecases::docs::{DocAuthor, DocChange, DocError, DocService};
use domain::block::BlockKind;
use domain::conversation::SessionId;
use domain::doc::{Doc, DocCommand, DocId, ExportFormat};
use futures::{FutureExt as _, StreamExt as _};
use gpui::{AppContext as _, ClipboardItem, Context, Entity, Subscription, Task, Window};

use super::{Session, SessionController, SessionEvent};
use crate::app::DocViewChanged;
use crate::docs::export::{export_file_name, file_url, html_export, markdown_export, pdf_export};
use crate::docs::{DocPanel, DocPanelEvent, DocView, SOURCE_ACTION, SavedState};
use crate::input::{DocChip, DocChoice};
use crate::timeline::{DocRef, DocTarget, TimelineEntry, TimelineEvent};

/// The largest picture of a document the page reads, in bytes.
const MAX_ASSET: usize = 16 * 1024 * 1024;

/// The most characters of the agent's last line in the strip under a
/// document shown alone.
const STRIP_CHARS: usize = 160;

/// What the session keeps about documents.
pub(super) struct DocState {
    /// The documents; `None` without a store.
    pub service: Option<Arc<DocService>>,
    /// The open chat.
    pub session_id: Option<SessionId>,
    /// The chat's first directory, empty for a chat outside a project.
    pub workspace_root: String,
    pub panel: Entity<DocPanel>,
    /// The document the panel shows.
    pub current: Option<DocId>,
    /// Every document, newest first, for the prompt's palette.
    pub list: Vec<Doc>,
    /// The view the window had for the previous chat: applied to the
    /// document that opens with this one.
    initial_view: Option<DocView>,
    changes: Option<Task<()>>,
    timeline_events: Option<Subscription>,
    pub subscriptions: Vec<Subscription>,
}

impl DocState {
    pub fn new(
        service: Option<Arc<DocService>>,
        session_id: Option<&str>,
        workspace_root: String,
        panel: Entity<DocPanel>,
    ) -> Self {
        Self {
            service,
            session_id: session_id.and_then(SessionId::parse),
            workspace_root,
            panel,
            current: None,
            list: Vec::new(),
            initial_view: None,
            changes: None,
            timeline_events: None,
            subscriptions: Vec::new(),
        }
    }
}

/// Runs `work` where the controller's adapters can run: the store and the
/// files hand blocking work to their own runtime, which GPUI's executors do
/// not have. `None` when that runtime went away.
async fn on_runtime<T: Send + 'static>(
    controller: Arc<dyn SessionController>,
    work: impl std::future::Future<Output = T> + Send + 'static,
) -> Option<T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let job = async move {
        let _ = tx.send(work.await);
    }
    .boxed();
    if let Some(job) = controller.run_on_runtime(job) {
        job.await;
    }
    rx.await.ok()
}

/// A document read with everything the panel shows of it.
struct Loaded {
    doc: Doc,
    text: String,
    assets: Vec<(String, Vec<u8>)>,
    siblings: Vec<Doc>,
    chat_title: Option<String>,
}

/// Reads document `id`, the pictures its text links and the chat's other
/// documents.
async fn load(service: &DocService, id: DocId) -> Result<Loaded, DocError> {
    let (doc, text) = service.read(id).await?;
    let mut assets = Vec::new();
    for name in asset_names(&text) {
        if let Ok(bytes) = service.read_asset(id, &name, MAX_ASSET).await {
            assets.push((name, bytes));
        }
    }
    let siblings = service.docs_of(doc.session_id).await?;
    let chat_title = service
        .store()
        .list_all()
        .await
        .ok()
        .and_then(|all| all.into_iter().find(|s| s.id == doc.session_id))
        .and_then(|s| s.title);
    Ok(Loaded {
        doc,
        text,
        assets,
        siblings,
        chat_title,
    })
}

/// The names of the pictures `markdown` links as `](assets/<name>)`.
fn asset_names(markdown: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in markdown.split("](assets/").skip(1) {
        if let Some((name, _)) = part.split_once(')')
            && !name.is_empty()
            && !names.iter().any(|known| known == name)
        {
            names.push(name.to_owned());
        }
    }
    names
}

/// The first line of the agent's last message, plain: no heading or list
/// marks, cut to [`STRIP_CHARS`].
fn agent_line(entries: &[TimelineEntry]) -> Option<String> {
    entries.iter().rev().find_map(|entry| {
        let BlockKind::Text { markdown } = &entry.block.kind else {
            return None;
        };
        // The person's own messages are quoted.
        if markdown.trim_start().starts_with('>') {
            return None;
        }
        let line = markdown
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())?;
        let line = plain_line(line);
        let line = line.as_str();
        if line.is_empty() {
            return None;
        }
        let mut shown: String = line.chars().take(STRIP_CHARS).collect();
        if line.chars().count() > STRIP_CHARS {
            shown.push('…');
        }
        Some(shown)
    })
}

/// `line` without its inline Markdown marks: a leading `#`, `>`, `-`, `*` or
/// `1.`, then `**`, `*`, `__` and backticks, and links reduced to their text.
fn plain_line(line: &str) -> String {
    let line = line.trim_start_matches(['#', '>', '-', '*', ' ']);
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let line = match line[digits..].strip_prefix(". ") {
        Some(rest) if digits > 0 => rest,
        _ => line,
    };
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(ch) = rest.chars().next() {
        if ch == '[' {
            // [text](url) -> text
            if let Some(close) = rest.find("](")
                && let Some(end) = rest[close..].find(')')
            {
                out.push_str(&rest[1..close]);
                rest = &rest[close + end + 1..];
                continue;
            }
        }
        if rest.starts_with("__") {
            rest = &rest[2..];
        } else {
            if ch != '*' && ch != '`' {
                out.push(ch);
            }
            rest = &rest[ch.len_utf8()..];
        }
    }
    out.trim().to_owned()
}

/// What to tell when `query` names no single document.
fn lookup_notice(error: &DocError, query: &str) -> String {
    match error {
        DocError::NotFound(_) => format!("no document matches “{query}”"),
        DocError::Ambiguous(n) => format!("{n} documents match “{query}”; pick one"),
        other => other.to_string(),
    }
}

/// Creates a document of chat `session`; a chat the store does not know yet
/// is saved first and the creation retried once.
async fn create_doc(
    service: &DocService,
    controller: &Arc<dyn SessionController>,
    session: SessionId,
    root: &str,
    title: Option<String>,
) -> Result<Doc, String> {
    let mut made = service.create_for(session, root, title.clone()).await;
    if matches!(made, Err(DocError::Store(_))) {
        controller.ensure_saved().await?;
        made = service.create_for(session, root, title).await;
    }
    made.map_err(|err| err.to_string())
}

impl Session {
    /// The documents service and the open chat, or the notice that says why
    /// there are none.
    fn doc_context(&mut self, cx: &mut Context<Self>) -> Option<(Arc<DocService>, SessionId)> {
        let Some(service) = self.docs.service.clone() else {
            self.notice(
                NoticeLevel::Warning,
                "documents are not available in this window",
                cx,
            );
            return None;
        };
        let Some(session) = self.docs.session_id else {
            self.notice(
                NoticeLevel::Warning,
                "this chat has no documents: it is not saved yet",
                cx,
            );
            return None;
        };
        Some((service, session))
    }

    /// The panel and the window follow the chat's documents: the panel's
    /// events, the "show chat" of the strip, the timeline's "Add to doc"
    /// and the changes the agent writes.
    pub(super) fn wire_docs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.docs.panel.clone();
        let mut subscriptions = vec![cx.subscribe_in(&panel, window, Self::on_doc_panel)];
        if let Some(root) = self.root.upgrade() {
            subscriptions.push(cx.subscribe(&root, |this, _, event: &DocViewChanged, cx| {
                this.docs
                    .panel
                    .update(cx, |panel, cx| panel.set_view(event.0, cx));
            }));
        }
        self.docs.subscriptions = subscriptions;
        self.watch_timeline(cx);
        if let Some(service) = self.docs.service.clone() {
            let mut changes = service.changes();
            self.docs.changes = Some(cx.spawn(async move |this, cx| {
                while let Some(change) = changes.next().await {
                    if this
                        .update(cx, |this, cx| this.on_doc_change(change, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            self.refresh_doc_list(cx);
            self.restore_docs(cx);
        }
    }

    /// Follows "Add to doc" of the timeline on screen (`/clear` replaces
    /// the timeline, so this runs again).
    pub(super) fn watch_timeline(&mut self, cx: &mut Context<Self>) {
        let timeline = self.timeline.clone();
        self.docs.timeline_events = Some(cx.subscribe(
            &timeline,
            |this, _, event: &TimelineEvent, cx| {
                let TimelineEvent::AddToDoc {
                    markdown,
                    target,
                    image,
                } = event;
                this.add_to_doc(markdown.clone(), *target, image.clone(), cx);
            },
        ));
        let docs = self.docs.list.clone();
        self.sync_timeline_docs(&docs, cx);
    }

    /// Tells the timeline which documents this chat has, for its block menu
    /// (`docs` may hold other chats' documents; they are skipped).
    fn sync_timeline_docs(&mut self, docs: &[Doc], cx: &mut Context<Self>) {
        let mine = docs
            .iter()
            .filter(|doc| Some(doc.session_id) == self.docs.session_id)
            .map(|doc| DocRef {
                id: doc.id,
                title: doc.title.clone(),
                updated_at: doc.updated_at,
            })
            .collect();
        self.timeline
            .update(cx, |timeline, cx| timeline.set_docs(mine, cx));
    }

    /// The view the window had for the previous chat: the document of this
    /// one opens in it.
    pub fn restore_doc_view(&mut self, view: DocView, cx: &mut Context<Self>) {
        if view == DocView::Hidden {
            return;
        }
        if self.docs.current.is_some() {
            if self.docs.panel.read(cx).view() == DocView::Hidden {
                self.set_doc_view(view, cx);
            }
        } else {
            self.docs.initial_view = Some(view);
        }
    }

    /// Opens the document `id` in `view`, over its chat, which must be this
    /// one (the window opens the chat first).
    pub fn show_doc(&mut self, id: &str, view: DocView, cx: &mut Context<Self>) {
        match DocId::parse(id) {
            Some(id) => self.open_doc(id, Some(view), cx),
            None => self.notice(NoticeLevel::Warning, "no such document", cx),
        }
    }

    /// The panel that shows the chat's documents.
    #[must_use]
    pub fn doc_panel(&self) -> &Entity<DocPanel> {
        &self.docs.panel
    }

    /// A new empty document in this chat, shown beside it (the "+" of the
    /// sidebar's Docs heading).
    pub fn new_doc_here(&mut self, cx: &mut Context<Self>) {
        self.new_doc(None, None, cx);
    }

    /// The id of the document the panel shows while the panel is on screen.
    #[must_use]
    pub fn shown_doc_id(&self, cx: &gpui::App) -> Option<String> {
        let panel = self.docs.panel.read(cx);
        if panel.view() == DocView::Hidden {
            return None;
        }
        panel.doc().map(|doc| doc.id.to_string())
    }

    /// The panel's view and the window's, in step.
    pub(super) fn set_doc_view(&mut self, view: DocView, cx: &mut Context<Self>) {
        self.docs
            .panel
            .update(cx, |panel, cx| panel.set_view(view, cx));
        self.root
            .update(cx, |root, cx| root.set_doc_view(view, cx))
            .ok();
        self.refresh_doc_strip(cx);
    }

    /// The view a document opens in when none is asked: the one it has, or
    /// beside the chat.
    fn default_doc_view(&self, cx: &Context<Self>) -> DocView {
        match self.docs.panel.read(cx).view() {
            DocView::Hidden => DocView::Split,
            shown => shown,
        }
    }

    /// Shows `loaded` in the panel, in `view` when one is given.
    fn show_loaded(&mut self, loaded: Loaded, view: Option<DocView>, cx: &mut Context<Self>) {
        let Loaded {
            doc,
            text,
            assets,
            siblings,
            chat_title,
        } = loaded;
        self.docs.current = Some(doc.id);
        self.sync_timeline_docs(&siblings, cx);
        self.docs.panel.update(cx, |panel, cx| {
            panel.show(doc, text, assets, cx);
            panel.set_docs_of_chat(siblings, cx);
            panel.set_chat_title(chat_title, cx);
        });
        if let Some(view) = view {
            self.set_doc_view(view, cx);
        }
    }

    /// Reads document `id` and shows it in `view` (`None` leaves the view).
    pub(super) fn open_doc(&mut self, id: DocId, view: Option<DocView>, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            self.notice(
                NoticeLevel::Warning,
                "documents are not available in this window",
                cx,
            );
            return;
        };
        let view = view.unwrap_or_else(|| self.default_doc_view(cx));
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let Some(loaded) =
                on_runtime(controller, async move { load(&service, id).await }).await
            else {
                return;
            };
            this.update(cx, |this, cx| match loaded {
                Ok(loaded) => this.show_loaded(loaded, Some(view), cx),
                Err(err) => this.notice(NoticeLevel::Warning, err.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// When the chat opens: its latest document goes in the panel, which
    /// shows in the view the window had (hidden stays hidden).
    fn restore_docs(&mut self, cx: &mut Context<Self>) {
        let (Some(service), Some(session)) = (self.docs.service.clone(), self.docs.session_id)
        else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                let latest = service
                    .docs_of(session)
                    .await
                    .ok()
                    .and_then(|docs| docs.into_iter().next())?;
                Some(load(&service, latest.id).await)
            };
            let Some(Some(loaded)) = on_runtime(controller, work).await else {
                return;
            };
            this.update(cx, |this, cx| {
                // Opened by hand meanwhile.
                if this.docs.current.is_some() {
                    return;
                }
                if let Ok(loaded) = loaded {
                    let view = this.docs.initial_view.take();
                    this.show_loaded(loaded, view, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads every document again for the prompt's palette.
    pub(super) fn refresh_doc_list(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                let docs = service.list().await.unwrap_or_default();
                let chats = service.store().list_all().await.unwrap_or_default();
                (docs, chats)
            };
            let Some((docs, chats)) = on_runtime(controller, work).await else {
                return;
            };
            let choices: Vec<DocChoice> = docs
                .iter()
                .map(|doc| {
                    let title = chats
                        .iter()
                        .find(|chat| chat.id == doc.session_id)
                        .and_then(|chat| chat.title.as_deref());
                    DocChoice {
                        id: doc.id.to_string(),
                        title: doc.title.clone(),
                        chat: crate::chats::chat_label(title),
                    }
                })
                .collect();
            this.update(cx, |this, cx| {
                this.sync_timeline_docs(&docs, cx);
                this.docs.list = docs;
                this.input
                    .update(cx, |input, cx| input.set_docs(choices, cx));
            })
            .ok();
        })
        .detach();
    }

    /// A document changed: the palette and the sidebar hear of it, and what
    /// the agent wrote shows in the panel.
    fn on_doc_change(&mut self, change: DocChange, cx: &mut Context<Self>) {
        self.refresh_doc_list(cx);
        cx.emit(SessionEvent::DocsChanged);
        if Some(change.session_id) != self.docs.session_id || change.author != DocAuthor::Agent {
            return;
        }
        if Some(change.id) == self.docs.current {
            self.reload_doc(change.id, cx);
        } else {
            // A document the agent made or edited: show it.
            self.open_doc(change.id, None, cx);
        }
    }

    /// Reads the shown document again after the agent wrote it.
    fn reload_doc(&mut self, id: DocId, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let Some(loaded) =
                on_runtime(controller, async move { load(&service, id).await }).await
            else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.docs.current != Some(id) {
                    return;
                }
                match loaded {
                    Ok(loaded) => {
                        this.show_loaded(loaded, None, cx);
                        this.docs.panel.update(cx, |panel, cx| {
                            panel.set_saved(SavedState::Saved, Some("agent"), cx);
                        });
                    }
                    Err(err) => this.notice(NoticeLevel::Warning, err.to_string(), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Puts the first line of the agent's last message in the strip under a
    /// document shown alone.
    pub(super) fn refresh_doc_strip(&mut self, cx: &mut Context<Self>) {
        if self.docs.panel.read(cx).view() == DocView::Hidden {
            return;
        }
        let line = agent_line(self.timeline.read(cx).state().entries());
        self.docs
            .panel
            .update(cx, |panel, cx| panel.set_last_agent_line(line.clone(), cx));
        self.root
            .update(cx, |root, cx| root.set_doc_strip(line, cx))
            .ok();
    }

    /// What the panel asks of the session.
    fn on_doc_panel(
        &mut self,
        _panel: &Entity<DocPanel>,
        event: &DocPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.clone() {
            DocPanelEvent::Save { id, text } => self.save_doc(id, text, cx),
            DocPanelEvent::Rename { id, title } => self.rename_doc(id, title, cx),
            DocPanelEvent::Delete(id) => self.delete_doc(id, cx),
            DocPanelEvent::New => self.new_doc(None, None, cx),
            DocPanelEvent::Switch(id) => self.open_doc(id, None, cx),
            DocPanelEvent::Export(format) => self.export_doc(format, cx),
            DocPanelEvent::AskAgent { id, passage } => self.ask_agent(id, passage, window, cx),
            DocPanelEvent::ViewChanged(view) => {
                self.root
                    .update(cx, |root, cx| root.set_doc_view(view, cx))
                    .ok();
                self.refresh_doc_strip(cx);
                cx.emit(SessionEvent::DocView(view));
                if view == DocView::Alone {
                    self.focus_input(window, cx);
                }
            }
            DocPanelEvent::ShowChat => self.set_doc_view(DocView::Split, cx),
            DocPanelEvent::FocusPrompt => self.focus_input(window, cx),
            DocPanelEvent::WriteAsset { name, bytes } => self.write_doc_asset(name, bytes, cx),
        }
    }

    /// `panel.doc` (`<leader>d`): hidden, beside the chat, alone, hidden.
    /// With no document open the chat's latest opens, or a new one.
    pub(super) fn cycle_doc_view(&mut self, cx: &mut Context<Self>) {
        let next = self.docs.panel.read(cx).view().cycle();
        if next == DocView::Hidden || self.docs.current.is_some() {
            self.set_doc_view(next, cx);
        } else {
            self.show_chat_doc(Some(next), cx);
        }
    }

    /// Shows the chat's document in `view` (or the usual one): the open one,
    /// else the latest, else a new one.
    fn show_chat_doc(&mut self, view: Option<DocView>, cx: &mut Context<Self>) {
        if self.docs.current.is_some() {
            let view = view.unwrap_or_else(|| self.default_doc_view(cx));
            self.set_doc_view(view, cx);
            return;
        }
        let Some((service, session)) = self.doc_context(cx) else {
            return;
        };
        let (controller, root) = (self.controller.clone(), self.docs.workspace_root.clone());
        let view = view.unwrap_or_else(|| self.default_doc_view(cx));
        let hop = controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                let latest = service
                    .docs_of(session)
                    .await
                    .ok()
                    .and_then(|docs| docs.into_iter().next());
                match latest {
                    Some(doc) => load(&service, doc.id).await.map_err(|err| err.to_string()),
                    None => match create_doc(&service, &controller, session, &root, None).await {
                        Ok(doc) => load(&service, doc.id).await.map_err(|err| err.to_string()),
                        Err(err) => Err(err),
                    },
                }
            };
            let Some(loaded) = on_runtime(hop, work).await else {
                return;
            };
            this.update(cx, |this, cx| match loaded {
                Ok(loaded) => this.show_loaded(loaded, Some(view), cx),
                Err(err) => this.notice(NoticeLevel::Warning, err, cx),
            })
            .ok();
        })
        .detach();
    }

    /// A new empty document in this chat, shown in `view` (or beside it).
    pub(super) fn new_doc(
        &mut self,
        title: Option<String>,
        view: Option<DocView>,
        cx: &mut Context<Self>,
    ) {
        let Some((service, session)) = self.doc_context(cx) else {
            return;
        };
        let (controller, root) = (self.controller.clone(), self.docs.workspace_root.clone());
        let view = view.unwrap_or_else(|| self.default_doc_view(cx));
        let hop = controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                match create_doc(&service, &controller, session, &root, title).await {
                    Ok(doc) => load(&service, doc.id).await.map_err(|err| err.to_string()),
                    Err(err) => Err(err),
                }
            };
            let Some(loaded) = on_runtime(hop, work).await else {
                return;
            };
            this.update(cx, |this, cx| match loaded {
                Ok(loaded) => this.show_loaded(loaded, Some(view), cx),
                Err(err) => this.notice(NoticeLevel::Warning, err, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Appends `markdown` to the document `target` names (by default the
    /// chat's current one, else the latest, else a new one) and shows the
    /// document, which lights up as changed.
    pub(super) fn add_to_doc(
        &mut self,
        markdown: String,
        target: DocTarget,
        image: Option<(String, Vec<u8>)>,
        cx: &mut Context<Self>,
    ) {
        if markdown.trim().is_empty() {
            return;
        }
        let Some((service, session)) = self.doc_context(cx) else {
            return;
        };
        let (controller, root) = (self.controller.clone(), self.docs.workspace_root.clone());
        let current = match target {
            DocTarget::Doc(id) => Some(id),
            DocTarget::New => None,
            DocTarget::Default => self.docs.current,
        };
        let view = self.default_doc_view(cx);
        let hop = controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                let target = match current {
                    Some(id) => Some(id),
                    None if target == DocTarget::New => None,
                    None => service
                        .docs_of(session)
                        .await
                        .ok()
                        .and_then(|docs| docs.into_iter().next())
                        .map(|doc| doc.id),
                };
                let id = match target {
                    Some(id) => id,
                    None => {
                        create_doc(&service, &controller, session, &root, None)
                            .await?
                            .id
                    }
                };
                let mut markdown = markdown;
                if let Some((name, bytes)) = image {
                    let stored = service
                        .write_asset(id, &name, &bytes)
                        .await
                        .map_err(|err| err.to_string())?;
                    markdown =
                        markdown.replace(&format!("assets/{name}"), &format!("assets/{stored}"));
                }
                service
                    .append(id, &markdown)
                    .await
                    .map_err(|err| err.to_string())?;
                let loaded = load(&service, id).await.map_err(|err| err.to_string())?;
                Ok::<_, String>((loaded, markdown.trim().to_owned()))
            };
            let Some(done) = on_runtime(hop, work).await else {
                return;
            };
            this.update(cx, |this, cx| match done {
                Ok((loaded, added)) => {
                    let title = loaded.doc.title.clone();
                    // Shown again after the change, and marked as saved.
                    this.show_loaded(loaded, Some(view), cx);
                    this.docs
                        .panel
                        .update(cx, |panel, cx| panel.set_saved(SavedState::Saved, None, cx));
                    this.flash_added_text(&added, cx);
                    this.notice(NoticeLevel::Info, format!("added to “{title}”"), cx);
                }
                Err(err) => this.notice(NoticeLevel::Warning, err, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Lights up `added`, the markdown just appended at the end of the shown
    /// doc, and brings it into view.
    fn flash_added_text(&mut self, added: &str, cx: &mut Context<Self>) {
        self.docs.panel.update(cx, |panel, cx| {
            let text = panel.text();
            let end = text.trim_end().len();
            if let Some(start) = end.checked_sub(added.len())
                && text.is_char_boundary(start)
                && text[start..end] == *added
            {
                panel.flash_added(start..end, cx);
            }
        });
    }

    /// Writes the text the panel asks to save.
    fn save_doc(&mut self, id: DocId, text: String, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let Some(saved) =
                on_runtime(controller, async move { service.save(id, &text).await }).await
            else {
                return;
            };
            this.update(cx, |this, cx| {
                let shown = this.docs.current == Some(id);
                this.docs.panel.update(cx, |panel, cx| match &saved {
                    // Typing since then keeps the doc dirty.
                    Ok(_) if shown && *panel.saved() == SavedState::Saving => {
                        panel.set_saved(SavedState::Saved, None, cx);
                    }
                    Err(err) if shown => {
                        panel.set_saved(SavedState::Failed(err.to_string()), None, cx);
                    }
                    _ => {}
                });
                if let Err(err) = saved {
                    this.notice(NoticeLevel::Warning, err.to_string(), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Renames the document (and its file), keeping what the person typed
    /// and has not saved yet.
    fn rename_doc(&mut self, id: DocId, title: String, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            return;
        };
        let panel = self.docs.panel.read(cx);
        let unsaved = (*panel.saved() != SavedState::Saved).then(|| panel.text());
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                if let Some(text) = unsaved {
                    service.save(id, &text).await?;
                }
                service.rename(id, &title).await?;
                load(&service, id).await
            };
            let Some(renamed) = on_runtime(controller, work).await else {
                return;
            };
            this.update(cx, |this, cx| match renamed {
                Ok(loaded) => this.show_loaded(loaded, None, cx),
                Err(err) => this.notice(NoticeLevel::Warning, err.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Deletes the document's row (its files stay), then shows the chat's
    /// latest document or hides the panel.
    fn delete_doc(&mut self, id: DocId, cx: &mut Context<Self>) {
        let Some(service) = self.docs.service.clone() else {
            return;
        };
        let session = self.docs.session_id;
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                service.delete(id).await?;
                match session {
                    Some(session) => service.docs_of(session).await,
                    None => Ok(Vec::new()),
                }
            };
            let Some(result) = on_runtime(controller, work).await else {
                return;
            };
            this.update(cx, |this, cx| match result {
                Ok(left) => {
                    if this.docs.current == Some(id) {
                        this.docs.current = None;
                        match left.first() {
                            Some(next) => this.open_doc(next.id, None, cx),
                            None => this.set_doc_view(DocView::Hidden, cx),
                        }
                    } else {
                        this.sync_timeline_docs(&left, cx);
                        this.docs
                            .panel
                            .update(cx, |panel, cx| panel.set_docs_of_chat(left, cx));
                    }
                    this.notice(NoticeLevel::Info, "doc deleted", cx);
                }
                Err(err) => this.notice(NoticeLevel::Warning, err.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Exports the shown document: a file the person names (Markdown with
    /// the pictures inside, or a web page), or the markdown on the
    /// clipboard.
    fn export_doc(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let panel = self.docs.panel.read(cx);
        let Some(doc) = panel.doc().cloned() else {
            self.notice(NoticeLevel::Warning, "no document open", cx);
            return;
        };
        let (text, assets) = (panel.text(), panel.assets().to_vec());
        if format == ExportFormat::Copy {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.notice(
                NoticeLevel::Info,
                format!("copied “{}” as markdown", doc.title),
                cx,
            );
            return;
        }
        if format == ExportFormat::Pdf {
            self.open_pdf_page(&doc.title, &text, &assets, cx);
            return;
        }
        let name = export_file_name(&doc.title, format);
        let start = if self.docs.workspace_root.is_empty() {
            std::path::PathBuf::from(&doc.dir)
        } else {
            std::path::PathBuf::from(&self.docs.workspace_root)
        };
        let chosen = cx.prompt_for_new_path(&start, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = chosen.await else {
                return;
            };
            let content = match format {
                ExportFormat::Html | ExportFormat::Pdf => html_export(&doc.title, &text, &assets),
                _ => markdown_export(&text, &assets),
            };
            let target = path.clone();
            let written = cx
                .background_spawn(async move { std::fs::write(&target, content) })
                .await;
            this.update(cx, |this, cx| match written {
                Ok(()) => this.notice(
                    NoticeLevel::Info,
                    format!("exported to {}", path.display()),
                    cx,
                ),
                Err(err) => this.notice(
                    NoticeLevel::Warning,
                    format!("could not write {}: {err}", path.display()),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// The PDF export: the page with its print script goes to a temporary
    /// file that opens in the browser, whose print dialog saves the PDF. No
    /// path is asked.
    fn open_pdf_page(
        &mut self,
        title: &str,
        text: &str,
        assets: &[(String, Vec<u8>)],
        cx: &mut Context<Self>,
    ) {
        let content = pdf_export(title, text, assets);
        let name = export_file_name(title, ExportFormat::Pdf);
        let stem = name.strip_suffix(".html").unwrap_or(&name);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("crest-{stem}-{nanos}.html"));
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let written = cx
                .background_spawn(async move { std::fs::write(&target, content) })
                .await;
            this.update(cx, |this, cx| match written {
                Ok(()) => {
                    cx.open_url(&file_url(&path));
                    this.notice(
                        NoticeLevel::Info,
                        "opened in the browser: use its print dialog to save the PDF",
                        cx,
                    );
                }
                Err(err) => this.notice(
                    NoticeLevel::Warning,
                    format!("could not write {}: {err}", path.display()),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Stores a picture the panel inserted in the document.
    fn write_doc_asset(&mut self, name: String, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let (Some(service), Some(id)) = (self.docs.service.clone(), self.docs.current) else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let asked = name.clone();
            let Some(stored) = on_runtime(controller, async move {
                service.write_asset(id, &asked, &bytes).await
            })
            .await
            else {
                return;
            };
            this.update(cx, |this, cx| match stored {
                Ok(stored) if stored != name => this.notice(
                    NoticeLevel::Warning,
                    format!("{name} was stored as {stored}; the link in the text is {name}"),
                    cx,
                ),
                Ok(_) => {}
                Err(err) => this.notice(NoticeLevel::Warning, err.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// "Ask agent" on a passage of the page: a chip on the prompt, which
    /// takes the focus.
    fn ask_agent(
        &mut self,
        id: DocId,
        passage: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self
            .docs
            .panel
            .read(cx)
            .doc()
            .map(|doc| doc.title.clone())
            .unwrap_or_default();
        self.input.update(cx, |input, cx| {
            input.add_doc_chip(
                DocChip::Selection {
                    doc_id: id,
                    title,
                    passage,
                },
                cx,
            );
        });
        self.focus_input(window, cx);
    }

    /// `/doc use`: the next message names the document as the one to change.
    /// With no `query` it is the open document, else the chat's latest.
    fn reference_doc(&mut self, query: Option<String>, cx: &mut Context<Self>) {
        let Some((service, session)) = self.doc_context(cx) else {
            return;
        };
        let current = self.docs.current;
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let work = async move {
                let found: Result<Doc, String> = match (&query, current) {
                    (Some(query), _) => service
                        .one(query)
                        .await
                        .map_err(|err| lookup_notice(&err, query)),
                    (None, Some(id)) => service.doc(id).await.map_err(|err| err.to_string()),
                    (None, None) => service
                        .docs_of(session)
                        .await
                        .ok()
                        .and_then(|docs| docs.into_iter().next())
                        .ok_or_else(|| "no document to reference".to_owned()),
                };
                found
            };
            let Some(found) = on_runtime(controller, work).await else {
                return;
            };
            this.update(cx, |this, cx| match found {
                Ok(doc) => {
                    this.input.update(cx, |input, cx| {
                        input.add_doc_chip(
                            DocChip::Reference {
                                doc_id: doc.id,
                                title: doc.title.clone(),
                            },
                            cx,
                        );
                    });
                    this.notice(
                        NoticeLevel::Info,
                        format!("referencing “{}”", doc.title),
                        cx,
                    );
                    if doc.session_id == session {
                        this.open_doc(doc.id, None, cx);
                    }
                }
                Err(message) => this.notice(NoticeLevel::Warning, message, cx),
            })
            .ok();
        })
        .detach();
    }

    /// `/doc find` and `/doc open`: the one document `query` names, in the
    /// split view, over its own chat (the window opens that chat first).
    fn open_doc_named(&mut self, query: String, cx: &mut Context<Self>) {
        let Some((service, session)) = self.doc_context(cx) else {
            return;
        };
        let controller = self.controller.clone();
        cx.spawn(async move |this, cx| {
            let asked = query.clone();
            let Some(found) =
                on_runtime(controller, async move { service.one(&asked).await }).await
            else {
                return;
            };
            this.update(cx, |this, cx| match found {
                Ok(doc) if doc.session_id == session => {
                    this.open_doc(doc.id, Some(DocView::Split), cx);
                }
                Ok(doc) => cx.emit(SessionEvent::OpenDoc {
                    session_id: doc.session_id.to_string(),
                    doc_id: doc.id.to_string(),
                    view: DocView::Split,
                }),
                Err(err) => this.notice(NoticeLevel::Warning, lookup_notice(&err, &query), cx),
            })
            .ok();
        })
        .detach();
    }

    /// `/doc ...`.
    pub(super) fn doc_command(
        &mut self,
        command: DocCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            DocCommand::Usage(text) => self.notice(NoticeLevel::Warning, text, cx),
            DocCommand::Open => self.show_chat_doc(None, cx),
            DocCommand::New(title) => self.new_doc(title, None, cx),
            DocCommand::Find(query) | DocCommand::OpenTitle(query) => {
                self.open_doc_named(query, cx);
            }
            DocCommand::Use(query) => self.reference_doc(query, cx),
            DocCommand::Add => match self.timeline.read(cx).selected_block_doc_content() {
                Some((markdown, image)) => {
                    self.add_to_doc(markdown, DocTarget::Default, image, cx);
                }
                None => self.notice(NoticeLevel::Warning, "select a block in the chat first", cx),
            },
            DocCommand::Split => self.show_chat_doc(Some(DocView::Split), cx),
            DocCommand::Only => self.show_chat_doc(Some(DocView::Alone), cx),
            DocCommand::Close => self.set_doc_view(DocView::Hidden, cx),
            DocCommand::Source => {
                if self.docs.current.is_some() {
                    self.docs.panel.update(cx, |panel, cx| {
                        panel.handle_action(&Action::builtin(SOURCE_ACTION), window, cx);
                    });
                } else {
                    self.notice(NoticeLevel::Warning, "no document open", cx);
                }
            }
            DocCommand::Export(None) => {
                self.notice(
                    NoticeLevel::Warning,
                    "usage: /doc export md|html|pdf|copy",
                    cx,
                );
            }
            DocCommand::Export(Some(format)) => {
                if self.docs.current.is_some() {
                    self.export_doc(format, cx);
                } else {
                    self.notice(NoticeLevel::Warning, "no document open", cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_shows_plain_text() {
        assert_eq!(
            plain_line("Found it.** `add` special-cased **negative** numbers"),
            "Found it. add special-cased negative numbers"
        );
        assert_eq!(
            plain_line("## *Heading* with [a link](http://x.y)"),
            "Heading with a link"
        );
        assert_eq!(plain_line("> quoted"), "quoted");
        assert_eq!(plain_line("- item __one__"), "item one");
        assert_eq!(plain_line("3. third"), "third");
    }

    #[test]
    fn a_text_names_the_pictures_it_links_once_each() {
        let text = "![a](assets/one.png) and ![b](assets/two.jpg) and ![c](assets/one.png)\n\
                    [not](assets/) ![x](other/three.png)";
        assert_eq!(asset_names(text), ["one.png", "two.jpg"]);
        assert!(asset_names("no pictures").is_empty());
    }

    #[test]
    fn a_failed_lookup_says_how_many_match_or_that_none_does() {
        assert_eq!(
            lookup_notice(&DocError::NotFound("zzz".into()), "zzz"),
            "no document matches “zzz”"
        );
        assert_eq!(
            lookup_notice(&DocError::Ambiguous(2), "re"),
            "2 documents match “re”; pick one"
        );
    }
}
