//! The window's content: the [project picker](crate::project_picker)
//! while no project is open, then the session of the chosen one.
//!
//! The UI does not know how to build a session: the binary passes a
//! [`ProjectOpener`] that turns a directory into [`SessionParts`]. While it
//! works the window says so; when it fails the picker comes back with the
//! reason. `/project` dispatches [`SwitchProject`], which shows the picker
//! again; the open session (and with it everything the binary started for
//! it) is dropped once another project is chosen. Until then it is kept out
//! of sight, and `Esc` or **Back** in the picker returns to it.
//! [`SessionEvent::Reopen`] (after `/login` or `/logout`) builds the
//! session of the same directory again, so the agent starts with the new
//! API key.
//!
//! The chats sidebar shows beside the session; `panel.chats` hides it.
//! The session's [`SessionEvent::Chats`] (the keymap's `chat.*` actions,
//! `focus.next` into the list) are run here. With no session open (the
//! picker), the keys of `app.quit` and `window.fullscreen` still work.
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
    Subscription, Window, actions, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex};

use crate::app::{RootView, set_theme, window_title};
use crate::chats::{ChatsCommand, ChatsEvent, ChatsPanel, ProjectChats, SessionOpen};
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
    /// The session `/project` left, until a project is chosen.
    back: Option<Live>,
    chats: Entity<ChatsPanel>,
    /// The chats sidebar is on screen (`panel.chats`).
    chats_shown: bool,
    focus_handle: FocusHandle,
    _chats: Subscription,
    _keys: Subscription,
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
        let weak = cx.entity().downgrade();
        let keys = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.key_without_session(event, window, cx))
                .ok();
        });
        Self {
            opener,
            content: Content::Opening(dir),
            parked: HashMap::new(),
            back: None,
            chats,
            chats_shown: true,
            focus_handle: cx.focus_handle(),
            _chats: subscription,
            _keys: keys,
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

    /// Whether the chats sidebar is on screen.
    pub fn chats_shown(&self) -> bool {
        self.chats_shown
    }

    /// The chats sidebar.
    pub fn chats(&self) -> &Entity<ChatsPanel> {
        &self.chats
    }

    /// With no session on screen (the picker, a project opening), the keys
    /// the builtin keymap gives to `app.quit` and `window.fullscreen` still
    /// work; with one, the session's keymap has them.
    fn key_without_session(
        &mut self,
        event: &gpui::KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session().is_some() {
            return;
        }
        let Some(key) = crate::input::key_from_keystroke(&event.keystroke) else {
            return;
        };
        let keymap = application::keymap::default_keymap();
        let Some(application::keymap::Action::Builtin(name)) =
            keymap.lookup(std::slice::from_ref(&key)).exact
        else {
            return;
        };
        match name.as_str() {
            crate::session::QUIT_ACTION => {
                window.dispatch_action(Box::new(crate::app::Quit), cx);
            }
            crate::session::FULLSCREEN_ACTION => window.toggle_fullscreen(),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Shows or hides the chats sidebar; hidden, it cannot keep the focus.
    pub fn toggle_chats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.chats.focus_handle(cx).contains_focused(window, cx);
        self.chats_shown = !self.chats_shown;
        let shown = self.chats_shown;
        if let Some(session) = self.session().cloned() {
            session.update(cx, |session, cx| {
                session.set_chats_shown(shown);
                if !shown && had_focus {
                    session.focus_input(window, cx);
                }
            });
        }
        cx.notify();
    }

    /// Runs what a key asked the session about the chats.
    fn chats_command(
        &mut self,
        command: ChatsCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Content::Session(live) = &self.content else {
            return;
        };
        let dir = live.dir.clone();
        match command {
            ChatsCommand::New => self.switch_chat(dir, SessionOpen::New, window, cx),
            ChatsCommand::Next | ChatsCommand::Prev => {
                let step = if command == ChatsCommand::Next { 1 } else { -1 };
                match self.chats.read(cx).neighbor(step) {
                    Some((root, id)) => {
                        self.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
                    }
                    None => self.inform("this project has no other chat".into(), cx),
                }
            }
            ChatsCommand::Rename | ChatsCommand::Delete => {
                if !self.chats_shown {
                    self.toggle_chats(window, cx);
                }
                self.chats.update(cx, |chats, cx| {
                    if command == ChatsCommand::Rename {
                        chats.rename_current(window, cx);
                    } else {
                        chats.delete_current(window, cx);
                    }
                });
            }
            ChatsCommand::TogglePanel => self.toggle_chats(window, cx),
            ChatsCommand::Focus => {
                if self.chats_shown {
                    self.chats.update(cx, |chats, cx| chats.focus(window, cx));
                } else if let Some(session) = self.session().cloned() {
                    session.update(cx, |session, cx| session.focus_input(window, cx));
                }
            }
        }
    }

    /// Shows `message` as information in the shown session, if any.
    fn inform(&self, message: String, cx: &mut Context<Self>) {
        if let Some(session) = self.session().cloned() {
            session.update(cx, |session, cx| {
                session.show_notice(NoticeLevel::Info, message, cx);
            });
        }
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
            |this, _, event: &ProjectPickerEvent, window, cx| match event {
                ProjectPickerEvent::Chosen(dir) => this.open(dir.clone(), window, cx),
                ProjectPickerEvent::Back => this.go_back(window, cx),
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
        // Dropping the session `/project` left drops its controller.
        self.back = None;
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
        let chats_shown = self.chats_shown;
        live.session.update(cx, |session, cx| {
            session.set_chats_shown(chats_shown);
            session.set_shown(true, window, cx);
        });
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
            SessionEvent::Chats(command) => {
                // Only the shown session has the keyboard.
                if self.session() == Some(session) {
                    self.chats_command(*command, window, cx);
                }
            }
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
            ChatsEvent::Dismissed => {
                if let Some(session) = self.session().cloned() {
                    session.update(cx, |session, cx| session.focus_input(window, cx));
                }
            }
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
        let Content::Session(live) =
            std::mem::replace(&mut self.content, Content::Opening(PathBuf::new()))
        else {
            return;
        };
        // Kept, out of sight and without the keyboard, until a project is
        // chosen: `Esc` or Back returns to it.
        live.session
            .update(cx, |session, cx| session.set_shown(false, window, cx));
        self.back = Some(live);
        self.show_picker(at.as_deref(), None, window, cx);
        if let Content::Picker(picker) = &self.content {
            picker.update(cx, |picker, cx| picker.set_can_go_back(true, cx));
        }
    }

    /// Back from the picker `/project` opened to the session it left.
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(live) = self.back.take() {
            self.show_live(live, window, cx);
        }
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
                .when(self.chats_shown, |row| row.child(self.chats.clone()))
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

    #[gpui::test]
    fn the_chat_menu_cancels_by_button_esc_or_click_outside(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        cx.update(|window, _| window.activate_window());
        open(&view, "/home/u/proj", cx);
        let session = shown(&view, cx);
        let chats = view.read_with(cx, |view, _| view.chats.clone());
        let redraw = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let at = |cx: &mut VisualTestContext, selector: &'static str| {
            cx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("no {selector}"))
                .center()
        };
        let right_click = |cx: &mut VisualTestContext| {
            redraw(cx);
            let row = at(cx, "chats-session-0-1");
            cx.simulate_mouse_down(row, gpui::MouseButton::Right, gpui::Modifiers::none());
            cx.simulate_mouse_up(row, gpui::MouseButton::Right, gpui::Modifiers::none());
            redraw(cx);
            assert!(chats.read_with(cx, |c, _| c.menu_open()));
        };
        let prompt_focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let input = session.read(cx).input().focus_handle(cx);
                input.contains_focused(window, cx)
            })
        };
        // Cancel.
        right_click(cx);
        let cancel = at(cx, "chats-cancel-0-1");
        cx.simulate_click(cancel, gpui::Modifiers::none());
        assert!(
            !chats.read_with(cx, |c, _| c.menu_open()),
            "Cancel closes it"
        );
        // Esc while renaming gives the prompt the focus back.
        right_click(cx);
        let rename = at(cx, "chats-rename-0-1");
        cx.simulate_click(rename, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(
            cx.update(|window, cx| crate::chats::rename_has_focus(window, cx)),
            "the rename field has the focus"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!chats.read_with(cx, |c, _| c.menu_open()), "Esc closes it");
        assert!(prompt_focused(cx), "the prompt has the focus again");
        // A click elsewhere.
        right_click(cx);
        cx.simulate_click(
            gpui::point(gpui::px(900.), gpui::px(300.)),
            gpui::Modifiers::none(),
        );
        assert!(
            !chats.read_with(cx, |c, _| c.menu_open()),
            "a click outside closes it"
        );
    }

    #[gpui::test]
    fn esc_or_back_returns_from_slash_project_to_the_session(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/proj", cx);
        let session = shown(&view, cx);
        let to_picker = |cx: &mut VisualTestContext| {
            session.update_in(cx, |session, window, cx| {
                session.command("project", window, cx)
            });
            cx.run_until_parked();
            view.read_with(cx, |view, _| assert!(view.project_picker().is_some()));
        };
        to_picker(cx);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(
            shown(&view, cx) == session,
            "Esc goes back to the same session"
        );
        cx.update(|window, cx| {
            let input = session.read(cx).input().focus_handle(cx);
            assert!(
                input.contains_focused(window, cx),
                "with the prompt focused"
            );
        });

        to_picker(cx);
        let back = cx.debug_bounds("project-back").expect("Back drawn");
        cx.simulate_click(back.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(shown(&view, cx) == session, "so does Back");

        // Choosing a project drops it.
        to_picker(cx);
        let weak = session.downgrade();
        drop(session);
        view.update_in(cx, |view, window, cx| {
            view.open(PathBuf::from("/home/u/other"), window, cx)
        });
        cx.run_until_parked();
        cx.update(|_, _| {});
        assert!(weak.upgrade().is_none(), "the left session was dropped");
    }

    fn shown_id(view: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
        view.read_with(cx, |view, _| view.open_id())
    }

    fn list_focused(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| crate::chats::list_has_focus(window, cx))
    }

    #[gpui::test]
    fn the_chat_keys_switch_create_and_hide_chats(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        open(&view, "/home/u/proj", cx);
        assert_eq!(shown_id(&view, cx).as_deref(), Some("latest"));
        cx.simulate_keystrokes("ctrl-g n");
        cx.run_until_parked();
        assert_eq!(shown_id(&view, cx).as_deref(), Some("second"));
        cx.simulate_keystrokes("ctrl-g n");
        cx.run_until_parked();
        assert_eq!(shown_id(&view, cx).as_deref(), Some("latest"), "wraps");
        cx.simulate_keystrokes("ctrl-g b");
        cx.run_until_parked();
        assert_eq!(shown_id(&view, cx).as_deref(), Some("second"));
        cx.simulate_keystrokes("ctrl-g c");
        cx.run_until_parked();
        assert_eq!(shown_id(&view, cx).as_deref(), Some("new-1"));
        // The sidebar hides and comes back; hidden, the cycle skips it.
        cx.simulate_keystrokes("ctrl-g s");
        assert!(!view.read_with(cx, |view, _| view.chats_shown()));
        draw(&view, cx);
        assert!(cx.debug_bounds("chats-empty").is_none());
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        assert!(!list_focused(cx));
        cx.simulate_keystrokes("ctrl-g s");
        assert!(view.read_with(cx, |view, _| view.chats_shown()));
    }

    #[gpui::test]
    fn the_chats_list_takes_the_keyboard(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        cx.update(|window, _| window.activate_window());
        open(&view, "/home/u/proj", cx);
        let chats = view.read_with(cx, |view, _| view.chats.clone());
        let session = shown(&view, cx);
        // prompt → timeline → (no tree) → chats, on the open chat.
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        assert!(list_focused(cx), "Ctrl+Tab reaches the chats");
        let selected = |cx: &mut VisualTestContext| chats.read_with(cx, |c, _| c.selected_index());
        assert_eq!(
            selected(cx),
            Some(1),
            "on the open chat (row 0 is the project)"
        );
        // Letters do not land in the prompt; arrows move.
        cx.simulate_keystrokes("j");
        assert_eq!(selected(cx), Some(2));
        assert_eq!(prompt_text(&session, cx), "");
        // A session kept alive here would still take the keys after the
        // switch below.
        drop(session);
        cx.simulate_keystrokes("up up up");
        assert_eq!(selected(cx), Some(0), "stops at the top");
        cx.simulate_keystrokes("end");
        assert_eq!(selected(cx), Some(2));
        // Delete asks; Esc cancels.
        cx.simulate_keystrokes("delete");
        assert!(chats.read_with(cx, |c, _| c.menu_open()));
        cx.simulate_keystrokes("escape");
        assert!(!chats.read_with(cx, |c, _| c.menu_open()));
        // F2 opens the name field.
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        assert!(list_focused(cx));
        cx.simulate_keystrokes("down f2");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| crate::chats::rename_has_focus(window, cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        // Enter opens the selected chat.
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        let target = chats.read_with(cx, |c, _| c.rows()[2].clone());
        cx.simulate_keystrokes("end enter");
        cx.run_until_parked();
        let crate::chats::ChatRow::Chat { id, .. } = target else {
            panic!("a chat row");
        };
        assert_eq!(shown_id(&view, cx), Some(id));
        // Ctrl+Tab from the list goes back to the prompt.
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-tab ctrl-tab");
        assert!(list_focused(cx));
        cx.simulate_keystrokes("ctrl-tab");
        assert!(!list_focused(cx));
    }

    #[gpui::test]
    fn chat_rename_and_delete_keys_act_on_the_open_chat(cx: &mut TestAppContext) {
        let (view, cx) = workbench(cx);
        cx.update(|window, _| window.activate_window());
        open(&view, "/home/u/proj", cx);
        let chats = view.read_with(cx, |view, _| view.chats.clone());
        cx.simulate_keystrokes("ctrl-g ,");
        cx.run_until_parked();
        assert!(cx.update(|window, cx| crate::chats::rename_has_focus(window, cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        // Hidden, the sidebar comes back to ask.
        cx.simulate_keystrokes("ctrl-g s ctrl-g x");
        assert!(view.read_with(cx, |view, _| view.chats_shown()));
        assert!(chats.read_with(cx, |c, _| c.menu_open()), "asks first");
        assert!(list_focused(cx), "so Enter confirms");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(!chats.read_with(cx, |c, _| c.menu_open()));
        // The test opener cannot delete: the session says so.
        let session = shown(&view, cx);
        session.read_with(cx, |s, cx| {
            let now = cx.background_executor().now();
            let notice = s
                .status()
                .read(cx)
                .state()
                .notice(now)
                .map(|n| n.message.clone());
            assert!(
                notice.is_some_and(|m| m.contains("could not delete")),
                "the delete was asked"
            );
        });
    }

    #[gpui::test]
    fn quit_works_in_the_project_picker(cx: &mut TestAppContext) {
        let (_view, cx) = workbench(cx);
        let quits = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = quits.clone();
        cx.update(|_, cx| {
            cx.on_action(move |_: &crate::app::Quit, _| counter.set(counter.get() + 1));
        });
        cx.simulate_keystrokes("ctrl-q");
        assert_eq!(quits.get(), 1);
    }
}
