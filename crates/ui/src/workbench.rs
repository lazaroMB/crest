//! The window's content: the [project picker](crate::project_picker)
//! while no project is open, then the session of the chosen one.
//!
//! The UI does not know how to build a session: the binary passes a
//! [`ProjectOpener`] that turns a directory into [`SessionParts`]. While it
//! works the window says so; when it fails the picker comes back with the
//! reason. `/project` dispatches [`SwitchProject`], which drops the open
//! session (and with it everything the binary started for it) and shows
//! the picker again. [`SessionEvent::Reopen`] (after `/login` or
//! `/logout`) builds the session of the same directory again, so the agent
//! starts with the new API key.
//!
//! Switching chats while a turn runs (a form waiting for an answer, a long
//! reply) parks the busy session instead of dropping it: it keeps running
//! out of sight, without the keyboard or the focus, and comes back as it
//! was when its chat is opened again. A parked session is dropped once its
//! turn ends. Idle sessions are not parked: each one carries its own agent
//! and processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use application::ports::events::NoticeLevel;
use application::ports::projects::{ProjectBrowserPort, RecentWorkspace};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, actions, div,
};
use gpui_component::{ActiveTheme as _, h_flex};

use crate::app::{RootView, set_theme, window_title};
use crate::chats::{ChatsEvent, ChatsPanel, ProjectChats, SessionOpen};
use crate::project_picker::{ProjectPickerEvent, ProjectPickerView};
use crate::session::{Session, SessionEvent, SessionParts};

actions!(crest, [SwitchProject]);

/// Opens projects for the window; implemented by the binary.
pub trait ProjectOpener: Send + Sync {
    /// The session parts of the project at `dir`, or why it cannot open.
    fn open(&self, dir: PathBuf) -> BoxFuture<'static, Result<SessionParts, String>>;

    /// The projects with saved sessions, newest first.
    fn recent(&self) -> BoxFuture<'static, Vec<RecentWorkspace>>;

    /// Lists directories for the picker.
    fn browser(&self) -> Arc<dyn ProjectBrowserPort>;

    /// [`Self::open`] for a chosen session. The default opens `dir` and
    /// ignores `which`.
    fn open_as(
        &self,
        dir: PathBuf,
        _which: SessionOpen,
    ) -> BoxFuture<'static, Result<SessionParts, String>> {
        self.open(dir)
    }

    /// Projects and their chats, newest first. The default is an empty list.
    fn catalog(&self) -> BoxFuture<'static, Vec<ProjectChats>> {
        futures::future::ready(Vec::new()).boxed()
    }

    /// Stores `title` on the chat `id`. The default fails.
    fn rename_chat(&self, id: String, title: String) -> BoxFuture<'static, Result<(), String>> {
        let _ = (id, title);
        futures::future::ready(Err("this window cannot rename chats".into())).boxed()
    }

    /// Removes the chat `id`. The default fails.
    fn delete_chat(&self, id: String) -> BoxFuture<'static, Result<(), String>> {
        let _ = id;
        futures::future::ready(Err("this window cannot delete chats".into())).boxed()
    }
}

/// A session with its root view, shown or parked.
struct Live {
    root: Entity<RootView>,
    session: Entity<Session>,
    dir: PathBuf,
    session_id: Option<String>,
    /// Workspace label, for the window title.
    label: String,
    _subscription: Subscription,
}

/// What the window shows.
enum Content {
    Picker(Entity<ProjectPickerView>),
    Opening(PathBuf),
    Session(Live),
}

/// The window's root view.
pub struct Workbench {
    opener: Arc<dyn ProjectOpener>,
    content: Content,
    /// Busy sessions whose chat was left, by session id.
    parked: HashMap<String, Live>,
    chats: Entity<ChatsPanel>,
    focus_handle: FocusHandle,
    _chats: Subscription,
}

impl Workbench {
    fn build(
        opener: Arc<dyn ProjectOpener>,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let chats = cx.new(|cx| ChatsPanel::new(String::new(), None, cx));
        let subscription = cx.subscribe_in(&chats, window, Self::on_chats);
        Self {
            opener,
            content: Content::Opening(dir),
            parked: HashMap::new(),
            chats,
            focus_handle: cx.focus_handle(),
            _chats: subscription,
        }
    }

    /// A window that starts in the picker, showing `notice` (why the last
    /// project did not open), if any.
    pub fn picker(
        opener: Arc<dyn ProjectOpener>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(opener, PathBuf::new(), window, cx);
        this.show_picker(None, notice, window, cx);
        this
    }

    /// A window that starts with the session of `dir`, already built.
    pub fn with_session(
        opener: Arc<dyn ProjectOpener>,
        parts: SessionParts,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(opener, dir.clone(), window, cx);
        this.show_session(parts, dir, window, cx);
        this
    }

    /// A new window root in the picker ([`Self::picker`]).
    pub fn new_picker(
        opener: Arc<dyn ProjectOpener>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::picker(opener, None, window, cx))
    }

    /// A new window root with the session of `dir` ([`Self::with_session`]).
    pub fn new_with_session(
        opener: Arc<dyn ProjectOpener>,
        parts: SessionParts,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::with_session(opener, parts, dir, window, cx))
    }

    /// The open session, if a project is open.
    pub fn session(&self) -> Option<&Entity<Session>> {
        match &self.content {
            Content::Session(live) => Some(&live.session),
            _ => None,
        }
    }

    /// The picker, while it shows.
    pub fn project_picker(&self) -> Option<&Entity<ProjectPickerView>> {
        match &self.content {
            Content::Picker(picker) => Some(picker),
            _ => None,
        }
    }

    /// The directory being opened, while it opens.
    pub fn opening(&self) -> Option<&Path> {
        match &self.content {
            Content::Opening(dir) => Some(dir),
            _ => None,
        }
    }

    /// How many busy sessions run out of sight.
    pub fn parked(&self) -> usize {
        self.parked.len()
    }

    fn show_picker(
        &mut self,
        at: Option<&Path>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let browser = self.opener.browser();
        let picker = cx.new(|cx| ProjectPickerView::new(browser, window, cx));
        picker.update(cx, |picker, cx| {
            if let Some(dir) = at {
                picker.show_dir(dir, window, cx);
            }
            picker.set_notice(notice, cx);
            picker.focus(window, cx);
        });
        cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &ProjectPickerEvent, window, cx| {
                let ProjectPickerEvent::Chosen(dir) = event;
                this.open(dir.clone(), window, cx);
            },
        )
        .detach();
        let recent = self.opener.recent();
        let weak = picker.downgrade();
        cx.spawn(async move |_, cx| {
            let recent = recent.await;
            weak.update(cx, |picker, cx| picker.set_recent(recent, cx))
                .ok();
        })
        .detach();
        window.set_window_title(&window_title(""));
        self.content = Content::Picker(picker);
        cx.notify();
    }

    /// Opens the project at `dir`; on failure the picker comes back.
    pub fn open(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_with_notice(dir, SessionOpen::Latest, None, window, cx);
    }

    /// [`Self::open`], showing `notice` in the new session.
    fn open_with_notice(
        &mut self,
        dir: PathBuf,
        which: SessionOpen,
        notice: Option<(NoticeLevel, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opening = self.opener.open_as(dir.clone(), which);
        self.content = Content::Opening(dir.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = opening.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(parts) => {
                    this.show_session(parts, dir, window, cx);
                    if let (Some((level, message)), Some(session)) = (notice, this.session()) {
                        session.update(cx, |session, cx| session.show_notice(level, message, cx));
                    }
                }
                Err(err) => {
                    let notice = format!("cannot open {}: {err}", dir.display());
                    this.show_picker(Some(&dir), Some(notice), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_session(
        &mut self,
        parts: SessionParts,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        set_theme(&parts.theme, cx);
        let label = parts.workspace_label.clone();
        let session_id = parts.session_id.clone();
        let dir = parts.workspace_dir.clone().unwrap_or(dir);
        let (root, session) = crate::session::open(parts, window, cx);
        let subscription = cx.subscribe_in(&session, window, Self::on_session);
        self.show_live(
            Live {
                root,
                session,
                dir,
                session_id,
                label,
                _subscription: subscription,
            },
            window,
            cx,
        );
        self.reload_chats(cx);
    }

    /// Puts `live` on screen: new, or back from the parked ones.
    fn show_live(&mut self, live: Live, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(&window_title(&live.label));
        live.session
            .update(cx, |session, cx| session.set_shown(true, window, cx));
        let (root, id) = (live.dir.display().to_string(), live.session_id.clone());
        self.chats
            .update(cx, |chats, cx| chats.set_current(root, id, cx));
        self.content = Content::Session(live);
        cx.notify();
    }

    fn on_session(
        &mut self,
        session: &Entity<Session>,
        event: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Reopen { notice } => {
                // Only the shown session reopens; a parked one is idle by
                // the time it could ask.
                let Content::Session(live) = &self.content else {
                    return;
                };
                if &live.session != session {
                    return;
                }
                let which = match &live.session_id {
                    Some(id) => SessionOpen::Id(id.clone()),
                    None => SessionOpen::Latest,
                };
                let dir = live.dir.clone();
                // Dropping the session drops its controller and what it started.
                self.open_with_notice(dir, which, notice.clone(), window, cx);
            }
            SessionEvent::Activity { busy, waiting } => {
                let shown = self.session() == Some(session);
                let id = if shown {
                    self.open_id()
                } else {
                    self.parked
                        .iter()
                        .find(|(_, live)| &live.session == session)
                        .map(|(id, _)| id.clone())
                };
                if let Some(id) = &id {
                    self.chats
                        .update(cx, |chats, cx| chats.set_activity(id, *busy, *waiting, cx));
                }
                if *busy {
                    return;
                }
                // Idle: the turn has ended and saved. A parked session has
                // nothing left to do; the list may show a new title.
                match id {
                    Some(id) if !shown => {
                        // After this event: the session is still in use.
                        let this = cx.entity().downgrade();
                        cx.defer(move |cx| {
                            this.update(cx, |this, cx| {
                                this.parked.remove(&id);
                                this.reload_chats(cx);
                            })
                            .ok();
                        });
                    }
                    _ => self.reload_chats(cx),
                }
            }
        }
    }

    fn on_chats(
        &mut self,
        _: &Entity<ChatsPanel>,
        event: &ChatsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Content::Session(live) = &self.content else {
            return;
        };
        let (dir, open_id) = (live.dir.clone(), live.session_id.clone());
        match event.clone() {
            ChatsEvent::New => self.switch_chat(dir, SessionOpen::New, window, cx),
            ChatsEvent::Open { root, id } => {
                if open_id.as_deref() == Some(id.as_str()) {
                    return;
                }
                self.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
            }
            ChatsEvent::Rename { id, title } => self.rename_chat(id, title, window, cx),
            ChatsEvent::Delete { root, id } => self.delete_chat(root, id, window, cx),
        }
    }

    /// Opens `which` in `dir`. A shown session with a turn running is
    /// parked (it must have an id: a chat not saved yet stays); a parked
    /// target comes back as it was.
    fn switch_chat(
        &mut self,
        dir: PathBuf,
        which: SessionOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let SessionOpen::Id(id) = &which
            && let Some(live) = self.parked.remove(id)
        {
            if !self.leave_current(window, cx) {
                self.parked.insert(id.clone(), live);
                return;
            }
            self.show_live(live, window, cx);
            return;
        }
        if !self.leave_current(window, cx) {
            return;
        }
        self.open_with_notice(dir, which, None, window, cx);
    }

    /// Prepares the shown session to be replaced: a busy one with an id is
    /// parked, an idle one is left for the replacement to drop. Returns
    /// false when the session cannot be left (busy and not saved yet).
    fn leave_current(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Content::Session(live) = &self.content else {
            return true;
        };
        if !live.session.read(cx).is_busy() {
            return true;
        }
        let Some(id) = live.session_id.clone() else {
            let message = if live.session.read(cx).is_running() {
                "this chat is running and not saved yet; Ctrl+C to interrupt it before switching"
            } else {
                "a command is running; switch chats once it stops"
            };
            live.session.update(cx, |session, cx| {
                session.show_notice(NoticeLevel::Warning, message.into(), cx);
            });
            return false;
        };
        let dir = live.dir.clone();
        let Content::Session(live) = std::mem::replace(&mut self.content, Content::Opening(dir))
        else {
            return true;
        };
        live.session
            .update(cx, |session, cx| session.set_shown(false, window, cx));
        self.parked.insert(id, live);
        true
    }

    /// Saves `title` on the chat `id` and refreshes the list.
    fn rename_chat(
        &mut self,
        id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session().cloned() else {
            return;
        };
        let current = self.open_id().as_deref() == Some(id.as_str());
        let opener = Arc::clone(&self.opener);
        // The session of the chat, shown or parked, saves the title itself:
        // a store written behind its back would be overwritten on its next
        // save.
        let primary = if current {
            Some(session.read(cx).retitle(title.clone()))
        } else {
            self.parked
                .get(&id)
                .map(|live| live.session.read(cx).retitle(title.clone()))
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = if let Some(primary) = primary {
                match primary.await {
                    Ok(()) => Ok(()),
                    Err(_) => opener.rename_chat(id, title).await,
                }
            } else {
                opener.rename_chat(id, title).await
            };
            this.update_in(cx, |this, window, cx| {
                this.finish_rename(result, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn finish_rename(
        &mut self,
        result: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = &result {
            self.notify(format!("could not rename the chat: {err}"), cx);
        }
        self.reload_chats(cx);
        if result.is_ok()
            && let Some(session) = self.session().cloned()
        {
            session.update(cx, |session, cx| session.focus_input(window, cx));
        }
    }

    /// Shows `message` as a warning in the shown session, if any.
    fn notify(&self, message: String, cx: &mut Context<Self>) {
        if let Some(session) = self.session().cloned() {
            session.update(cx, |session, cx| {
                session.show_notice(NoticeLevel::Warning, message, cx);
            });
        }
    }

    /// Removes the chat `id`. The open one is left for another chat of
    /// `root`, or a new chat when it was the last.
    fn delete_chat(
        &mut self,
        root: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.parked.contains_key(&id) {
            // Interrupting would not help: the turn saves when it ends and
            // the chat would come back.
            self.notify(
                "that chat is still running; open it and Ctrl+C before deleting it".into(),
                cx,
            );
            return;
        }
        let current = self.open_id().as_deref() == Some(id.as_str());
        if current
            && let Some(session) = self.session().cloned()
            && session.read(cx).is_busy()
        {
            session.update(cx, |session, cx| {
                let message = if session.is_running() {
                    "a turn is running; Ctrl+C to interrupt it before deleting this chat"
                } else {
                    "a command is running; delete this chat once it stops"
                };
                session.show_notice(NoticeLevel::Warning, message.into(), cx);
            });
            return;
        }
        let next = self.chats.read(cx).next_chat(&root, &id);
        let delete = self.opener.delete_chat(id);
        cx.spawn_in(window, async move |this, cx| {
            let result = delete.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) if current => {
                    let which = next.map_or(SessionOpen::New, SessionOpen::Id);
                    this.switch_chat(PathBuf::from(root), which, window, cx);
                }
                Ok(()) => this.reload_chats(cx),
                Err(err) => {
                    this.notify(format!("could not delete the chat: {err}"), cx);
                    this.reload_chats(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn reload_chats(&mut self, cx: &mut Context<Self>) {
        let chats = self.chats.clone();
        let catalog = self.opener.catalog();
        cx.spawn(async move |_, cx| {
            let projects = catalog.await;
            chats.update(cx, |panel, cx| panel.set_projects(projects, cx));
        })
        .detach();
    }

    fn open_id(&self) -> Option<String> {
        match &self.content {
            Content::Session(live) => live.session_id.clone(),
            _ => None,
        }
    }

    /// `/project`: back to the picker, at the parent of the open project.
    /// Not while chats run out of sight: the picker could open one of them
    /// a second time.
    fn switch_project(&mut self, _: &SwitchProject, window: &mut Window, cx: &mut Context<Self>) {
        let Content::Session(live) = &self.content else {
            return;
        };
        if !self.parked.is_empty() {
            let n = self.parked.len();
            let message = if n == 1 {
                "a chat is still running; open it and Ctrl+C before /project".to_owned()
            } else {
                format!("{n} chats are still running; open each and Ctrl+C before /project")
            };
            self.notify(message, cx);
            return;
        }
        let at = live.dir.parent().map(Path::to_path_buf);
        // Dropping the session drops its controller and what it started.
        self.show_picker(at.as_deref(), None, window, cx);
    }
}

impl Focusable for Workbench {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.content {
            Content::Picker(picker) => picker.clone().into_any_element(),
            Content::Session(live) => h_flex()
                .size_full()
                .child(self.chats.clone())
                .child(div().flex_1().h_full().min_w_0().child(live.root.clone()))
                .into_any_element(),
            Content::Opening(dir) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(format!("Opening {}…", dir.display())))
                .into_any_element(),
        };
        div()
            .id("workbench")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(Self::switch_project))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use application::ports::events::DomainEvent;
    use application::testing::fs::{InMemoryFs, test_root};
    use application::testing::projects::InMemoryDirs;
    use domain::conversation::StopReason;
    use domain::permission::PermissionMode;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::approval::UiApproval;
    use crate::session::{ModelChoice, SessionController, TurnEnd, TurnRequest};

    /// A controller whose turns end only when interrupted, so a prompt
    /// keeps it busy.
    struct Idle;

    impl SessionController for Idle {
        fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
            async move {
                request.cancel.cancelled().await;
                TurnEnd::Finished(StopReason::Interrupted)
            }
            .boxed()
        }

        fn permission_mode(&self) -> PermissionMode {
            PermissionMode::Ask
        }

        fn set_permission_mode(&self, _mode: PermissionMode) {}

        fn models(&self) -> Vec<ModelChoice> {
            Vec::new()
        }

        fn new_session(&self) -> BoxFuture<'static, ()> {
            futures::future::ready(()).boxed()
        }
    }

    /// Opens every directory but `/home/u/bad`; records what it opened.
    /// The session id is the one asked for, `latest` or `new-N`.
    struct Opener {
        opened: Mutex<Vec<PathBuf>>,
        news: Mutex<usize>,
    }

    impl ProjectOpener for Opener {
        fn open(&self, dir: PathBuf) -> BoxFuture<'static, Result<SessionParts, String>> {
            self.opened
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(dir.clone());
            if dir.ends_with("bad") {
                return futures::future::ready(Err("the config is broken".into())).boxed();
            }
            let fs = Arc::new(InMemoryFs::new(test_root()));
            let (_tx, events) = futures::channel::mpsc::unbounded::<DomainEvent>();
            let (_approval, approvals) = UiApproval::channel();
            let mut parts = SessionParts::new(Arc::new(Idle), events, approvals, fs.clone(), fs);
            parts.workspace_label = "proj".into();
            futures::future::ready(Ok(parts)).boxed()
        }

        fn open_as(
            &self,
            dir: PathBuf,
            which: SessionOpen,
        ) -> BoxFuture<'static, Result<SessionParts, String>> {
            let id = match which {
                SessionOpen::Latest => "latest".to_owned(),
                SessionOpen::Id(id) => id,
                SessionOpen::New => {
                    let mut news = self.news.lock().unwrap_or_else(PoisonError::into_inner);
                    *news += 1;
                    format!("new-{}", *news)
                }
            };
            self.open(dir)
                .map(move |parts| {
                    parts.map(|mut parts| {
                        parts.session_id = Some(id);
                        parts
                    })
                })
                .boxed()
        }

        fn recent(&self) -> BoxFuture<'static, Vec<RecentWorkspace>> {
            futures::future::ready(Vec::new()).boxed()
        }

        fn catalog(&self) -> BoxFuture<'static, Vec<ProjectChats>> {
            let chat = |id: &str| crate::chats::ChatEntry {
                id: id.to_owned(),
                title: id.to_owned(),
            };
            futures::future::ready(vec![ProjectChats {
                name: "proj".into(),
                root: "/home/u/proj".into(),
                chats: vec![chat("latest"), chat("second")],
            }])
            .boxed()
        }

        fn browser(&self) -> Arc<dyn ProjectBrowserPort> {
            Arc::new(
                InMemoryDirs::new("/home/u")
                    .with_project("/home/u/proj")
                    .with_dir("/home/u/bad"),
            )
        }
    }

    fn workbench(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let opener = Arc::new(Opener {
            opened: Mutex::new(Vec::new()),
            news: Mutex::new(0),
        });
        let (view, cx) =
            cx.add_window_view(|window, cx| Workbench::picker(opener, None, window, cx));
        cx.run_until_parked();
        (view, cx)
    }

    fn open(view: &Entity<Workbench>, dir: &str, cx: &mut VisualTestContext) {
        view.update_in(cx, |view, window, cx| {
            view.open(PathBuf::from(dir), window, cx)
        });
        cx.run_until_parked();
    }

    fn switch(view: &Entity<Workbench>, which: SessionOpen, cx: &mut VisualTestContext) {
        view.update_in(cx, |view, window, cx| {
            view.switch_chat(PathBuf::from("/home/u/proj"), which, window, cx);
        });
        cx.run_until_parked();
    }

    fn shown(view: &Entity<Workbench>, cx: &mut VisualTestContext) -> Entity<Session> {
        view.read_with(cx, |view, _| view.session().expect("a session").clone())
    }

    fn prompt_text(session: &Entity<Session>, cx: &mut VisualTestContext) -> String {
        session.read_with(cx, |s, cx| s.input().read(cx).text(cx))
    }

    fn draw(view: &Entity<Workbench>, cx: &mut VisualTestContext) {
        let view = view.clone();
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(1200.), gpui::px(800.)),
            move |_, _| gpui::div().size_full().child(view.clone()),
        );
    }

    #[gpui::test]
    fn switching_chats_parks_a_busy_session_and_brings_it_back(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/proj", cx);
        let first = shown(&view, cx);
        first.update(cx, |session, cx| session.submit("hola".into(), cx));
        cx.run_until_parked();

        // Busy: parked, not dropped. The new chat opens.
        switch(&view, SessionOpen::Id("second".into()), cx);
        let second = shown(&view, cx);
        assert!(second != first);
        view.read_with(cx, |view, _| assert_eq!(view.parked(), 1));
        first.read_with(cx, |s, _| assert!(s.is_running() && !s.is_shown()));
        draw(&view, cx);
        assert!(
            cx.debug_bounds("chats-activity").is_some(),
            "the parked chat shows it runs"
        );

        // Keys go to the shown session only.
        cx.simulate_input("x");
        assert_eq!(prompt_text(&second, cx), "x");
        assert_eq!(prompt_text(&first, cx), "");

        // Back: the same session, still running; the idle one is dropped.
        let weak_second = second.downgrade();
        drop(second);
        switch(&view, SessionOpen::Id("latest".into()), cx);
        let back = shown(&view, cx);
        assert!(back == first);
        back.read_with(cx, |s, _| assert!(s.is_running() && s.is_shown()));
        view.read_with(cx, |view, _| assert_eq!(view.parked(), 0));
        drop(back);
        cx.update(|_, _| {});
        assert!(
            weak_second.upgrade().is_none(),
            "the idle session was dropped"
        );
        cx.simulate_input("y");
        assert_eq!(prompt_text(&first, cx), "y");

        // Parked again and interrupted: dropped once its turn ends.
        switch(&view, SessionOpen::New, cx);
        view.read_with(cx, |view, _| assert_eq!(view.parked(), 1));
        first.update(cx, |session, cx| {
            session.interrupt(cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.parked(), 0));
        let weak_first = first.downgrade();
        drop(first);
        cx.update(|_, _| {});
        assert!(
            weak_first.upgrade().is_none(),
            "the finished session was dropped"
        );
    }

    #[gpui::test]
    fn a_running_chat_cannot_be_deleted_and_blocks_slash_project(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/proj", cx);
        let first = shown(&view, cx);
        first.update(cx, |session, cx| session.submit("hola".into(), cx));
        switch(&view, SessionOpen::Id("second".into()), cx);
        let second = shown(&view, cx);
        view.update_in(cx, |view, window, cx| {
            view.delete_chat("/home/u/proj".into(), "latest".into(), window, cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.parked(), 1));
        second.update_in(cx, |session, window, cx| {
            session.command("project", window, cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.session().is_some(), "/project refused: a chat runs");
        });
    }

    #[gpui::test]
    fn a_chosen_project_replaces_the_picker(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        view.read_with(cx, |view, _| assert!(view.project_picker().is_some()));
        // Enter in the picker opens the chosen row (the project).
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.session().is_some(), "the session is open");
            assert!(view.project_picker().is_none());
        });
    }

    #[gpui::test]
    fn a_project_that_fails_brings_the_picker_back(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/bad", cx);
        view.read_with(cx, |view, cx| {
            let picker = view.project_picker().expect("picker again");
            assert_eq!(picker.read(cx).text(cx), "/home/u/bad/");
        });
    }

    #[gpui::test]
    fn slash_project_closes_the_session_once_it_is_idle(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/proj", cx);
        let session = view.read_with(cx, |view, _| view.session().unwrap().clone());
        let weak = session.downgrade();
        // Busy: refused.
        session.update(cx, |session, cx| session.submit("hola".into(), cx));
        session.update_in(cx, |session, window, cx| {
            session.command("project", window, cx)
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.session().is_some()));
        drop(session);
        // Idle: back to the picker, at the project's parent, and the
        // session is gone.
        view.update_in(cx, |view, window, cx| {
            view.open(PathBuf::from("/home/u/proj"), window, cx)
        });
        cx.run_until_parked();
        let session = view.read_with(cx, |view, _| view.session().unwrap().clone());
        session.update_in(cx, |session, window, cx| {
            session.command("project", window, cx)
        });
        cx.run_until_parked();
        drop(session);
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let picker = view.project_picker().expect("picker again");
            assert_eq!(picker.read(cx).text(cx), "/home/u/");
        });
        cx.update(|_, _| {});
        assert!(weak.upgrade().is_none(), "the first session was dropped");
    }
}
