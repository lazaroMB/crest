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
use futures::future::BoxFuture;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, actions, div,
};
use gpui_component::ActiveTheme as _;

use crate::app::{RootView, set_theme, window_title};
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
}

/// What the window shows.
enum Content {
    Picker(Entity<ProjectPickerView>),
    Opening(PathBuf),
    Session {
        root: Entity<RootView>,
        session: Entity<Session>,
        dir: PathBuf,
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
        self.open_with_notice(dir, None, window, cx);
    }

    /// [`Self::open`], showing `notice` in the new session.
    fn open_with_notice(
        &mut self,
        dir: PathBuf,
        notice: Option<(NoticeLevel, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opening = self.opener.open(dir.clone());
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
        let (root, session) = crate::session::open(parts, window, cx);
        session.update(cx, |session, cx| session.focus_input(window, cx));
        cx.subscribe_in(
            &session,
            window,
            |this, _, event: &SessionEvent, window, cx| {
                let SessionEvent::Reopen { notice } = event;
                let Content::Session { dir, .. } = &this.content else {
                    return;
                };
                // Dropping the session drops its controller and what it started.
                this.open_with_notice(dir.clone(), notice.clone(), window, cx);
            },
        )
        .detach();
        self.content = Content::Session { root, session, dir };
        cx.notify();
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
            Content::Session { root, .. } => root.clone().into_any_element(),
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

    use futures::FutureExt as _;
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
