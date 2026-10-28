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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use application::ports::events::NoticeLevel;
use application::ports::projects::{ProjectBrowserPort, RecentWorkspace};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, actions, div,
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

/// What the window shows.
enum Content {
    Picker(Entity<ProjectPickerView>),
    Opening(PathBuf),
    Session {
        root: Entity<RootView>,
        session: Entity<Session>,
        chats: Entity<ChatsPanel>,
        dir: PathBuf,
        session_id: Option<String>,
    },
}

/// The window's root view.
pub struct Workbench {
    opener: Arc<dyn ProjectOpener>,
    content: Content,
    focus_handle: FocusHandle,
}

impl Workbench {
    /// A window that starts in the picker, showing `notice` (why the last
    /// project did not open), if any.
    pub fn picker(
        opener: Arc<dyn ProjectOpener>,
        notice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            opener,
            content: Content::Opening(PathBuf::new()),
            focus_handle: cx.focus_handle(),
        };
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
        let mut this = Self {
            opener,
            content: Content::Opening(dir.clone()),
            focus_handle: cx.focus_handle(),
        };
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
            Content::Session { session, .. } => Some(session),
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
        window.set_window_title(&window_title(&parts.workspace_label));
        set_theme(&parts.theme, cx);
        let session_id = parts.session_id.clone();
        let dir = parts.workspace_dir.clone().unwrap_or(dir);
        let (root, session) = crate::session::open(parts, window, cx);
        session.update(cx, |session, cx| session.focus_input(window, cx));
        let current_root = dir.display().to_string();
        let chats = cx.new(|cx| ChatsPanel::new(current_root, session_id.clone(), cx));
        let catalog = self.opener.catalog();
        let weak = chats.downgrade();
        cx.spawn(async move |_, cx| {
            let projects = catalog.await;
            weak.update(cx, |panel, cx| panel.set_projects(projects, cx))
                .ok();
        })
        .detach();
        cx.subscribe_in(
            &session,
            window,
            |this, _, event: &SessionEvent, window, cx| {
                let SessionEvent::Reopen { notice } = event;
                let (dir, which) = {
                    let Content::Session {
                        dir, session_id, ..
                    } = &this.content
                    else {
                        return;
                    };
                    let which = match session_id {
                        Some(id) => SessionOpen::Id(id.clone()),
                        None => SessionOpen::Latest,
                    };
                    (dir.clone(), which)
                };
                // Dropping the session drops its controller and what it started.
                this.open_with_notice(dir, which, notice.clone(), window, cx);
            },
        )
        .detach();
        cx.subscribe_in(&chats, window, |this, _, event: &ChatsEvent, window, cx| {
            let (dir, open_id) = {
                let Content::Session {
                    dir, session_id, ..
                } = &this.content
                else {
                    return;
                };
                (dir.clone(), session_id.clone())
            };
            match event.clone() {
                ChatsEvent::New => this.switch_chat(dir, SessionOpen::New, window, cx),
                ChatsEvent::Open { root, id } => {
                    if open_id.as_deref() == Some(id.as_str()) {
                        return;
                    }
                    this.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
                }
                ChatsEvent::Rename { id, title } => this.rename_chat(id, title, window, cx),
                ChatsEvent::Delete { root, id } => this.delete_chat(root, id, window, cx),
            }
        })
        .detach();
        self.content = Content::Session {
            root,
            session,
            chats,
            dir,
            session_id,
        };
        cx.notify();
    }

    /// Opens `which` in `dir`, unless a turn or a command is still running.
    fn switch_chat(
        &mut self,
        dir: PathBuf,
        which: SessionOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session().cloned() else {
            self.open_with_notice(dir, which, None, window, cx);
            return;
        };
        if session.read(cx).is_busy() {
            let message = if session.read(cx).is_running() {
                "a turn is running; Ctrl+C to interrupt it before switching chats"
            } else {
                "a command is running; switch chats once it stops"
            };
            session.update(cx, |session, cx| {
                session.show_notice(NoticeLevel::Warning, message.into(), cx);
            });
            return;
        }
        self.open_with_notice(dir, which, None, window, cx);
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
        let primary = current.then(|| session.read(cx).retitle(title.clone()));
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
        if let Err(err) = &result
            && let Some(session) = self.session().cloned()
        {
            session.update(cx, |session, cx| {
                session.show_notice(
                    NoticeLevel::Warning,
                    format!("could not rename the chat: {err}"),
                    cx,
                );
            });
        }
        self.reload_chats(cx);
        if result.is_ok()
            && let Some(session) = self.session().cloned()
        {
            session.update(cx, |session, cx| session.focus_input(window, cx));
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
        let next = self
            .chats()
            .and_then(|panel| panel.read(cx).next_chat(&root, &id));
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
                    if let Some(session) = this.session().cloned() {
                        session.update(cx, |session, cx| {
                            session.show_notice(
                                NoticeLevel::Warning,
                                format!("could not delete the chat: {err}"),
                                cx,
                            );
                        });
                    }
                    this.reload_chats(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn reload_chats(&mut self, cx: &mut Context<Self>) {
        let Some(chats) = self.chats().cloned() else {
            return;
        };
        let catalog = self.opener.catalog();
        cx.spawn(async move |_, cx| {
            let projects = catalog.await;
            chats.update(cx, |panel, cx| panel.set_projects(projects, cx));
        })
        .detach();
    }

    fn chats(&self) -> Option<&Entity<ChatsPanel>> {
        match &self.content {
            Content::Session { chats, .. } => Some(chats),
            _ => None,
        }
    }

    fn open_id(&self) -> Option<String> {
        match &self.content {
            Content::Session { session_id, .. } => session_id.clone(),
            _ => None,
        }
    }

    /// `/project`: back to the picker, at the parent of the open project.
    fn switch_project(&mut self, _: &SwitchProject, window: &mut Window, cx: &mut Context<Self>) {
        let Content::Session { dir, .. } = &self.content else {
            return;
        };
        let at = dir.parent().map(Path::to_path_buf);
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
            Content::Session { root, chats, .. } => h_flex()
                .size_full()
                .child(chats.clone())
                .child(div().flex_1().h_full().min_w_0().child(root.clone()))
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
    use std::sync::Mutex;

    use application::ports::events::DomainEvent;
    use application::testing::fs::{InMemoryFs, test_root};
    use application::testing::projects::InMemoryDirs;
    use domain::permission::PermissionMode;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::approval::UiApproval;
    use crate::session::{ModelChoice, SessionController, TurnEnd, TurnRequest};

    /// A controller whose turns never end, so a prompt keeps it busy.
    struct Idle;

    impl SessionController for Idle {
        fn run_turn(&self, _request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
            futures::future::pending().boxed()
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
    struct Opener {
        opened: Mutex<Vec<PathBuf>>,
    }

    impl ProjectOpener for Opener {
        fn open(&self, dir: PathBuf) -> BoxFuture<'static, Result<SessionParts, String>> {
            self.opened
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
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

        fn recent(&self) -> BoxFuture<'static, Vec<RecentWorkspace>> {
            futures::future::ready(Vec::new()).boxed()
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
