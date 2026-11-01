//! The agent session in the main window: wires the timeline, the input, the
//! status bar, the workspace tree, the pickers and the approval dialog to a
//! [`SessionController`].
//!
//! The UI never touches the agent's adapters: it calls the controller (the
//! input port, implemented by the binary over the `AgentService`) and shows
//! the [`DomainEvent`]s that the controller's event sink sends back on a
//! channel. One consumer task applies them, in order, to the timeline and
//! the status bar; plugin modal events go to the [`ModalHost`] overlay.
//!
//! # Turns
//!
//! One turn runs at a time, and a `/` command answered by the controller
//! (`/research`, `/implement`, `/!cmd`...) keeps the session busy too until
//! it replies. Commands are not queued: they run at once, even during a
//! turn (`/open`, `/!cmd`, `/help` do not wait for it; `/implement`,
//! `/aside` and `/flow` do, as they drive the agent too). A prompt
//! submitted while the session is busy is **queued**:
//! the input stays usable, the [`ActivityView`] above it lists what waits,
//! and the prompts are sent one by one once the session is free. `Up` in an
//! empty prompt takes the last queued one back to edit it. `Ctrl+C`
//! (`input.cancel`, wherever the focus is) copies the prompt's selection,
//! or else clears the prompt (`Up` brings it back); in an empty prompt it
//! is `turn.interrupt`, which cancels the [`CancelToken`] of the running
//! turn, or asks the controller to stop the running commands
//! ([`SessionController::interrupt_commands`]), and drops the queue.
//!
//! While busy, the activity line says what goes on (spinner, stage, tool,
//! elapsed time) from the events of the work.
//!
//! # Keys, focus and commands
//!
//! There are no modes: the prompt is always ready to type. Shortcuts are
//! chords or the leader chord (`<C-g>` by default) and work wherever
//! the focus is: keys typed while the timeline or the tree has focus go
//! through the input's keymap too, a printable key there is typed into the
//! prompt (focusing it), and `Esc` brings the focus back to the prompt.
//! The exception are the vim keys of the focused timeline (`j`/`k`/`g`/`G`,
//! `o`, `t`, `y`), which act on its blocks.
//! `<C-Tab>` / `<leader>w` cycle the focus prompt → timeline → tree.
//!
//! - `<leader>p` cycles the permission mode; `<leader>m` and `/model` put
//!   `/model ` in the prompt, whose popup lists the models of the providers
//!   with a key ([`crate::input::model_popup`]): `Enter` saves the chosen
//!   one, and `/model <alias>` sent as typed is the model of the **next**
//!   turn only.
//! - `<leader>e` shows the tree, `<leader>ff` / `<leader>fg` the pickers.
//!   Choosing a file inserts `@path ` (or `@path:line ` for a text match)
//!   at the cursor of the prompt, so it can be referenced in the next
//!   prompt. A file cited by a block (`Sources`) does the same.
//! - `/model <alias>` (no argument clears it), `/mode <ask|auto-edit|plan>`,
//!   `/clear` (a new, empty session; not while a turn runs), `/modal close`
//!   (closes the front plugin modal), `/help [section|search]`, `/login`
//!   and `/logout` (the API key in the system keyring, through the
//!   [`LoginView`] dialog; the window then reopens the workspace) and `/q`
//!   (see [`BUILTIN_COMMANDS`](crate::commands::BUILTIN_COMMANDS)).
//! - `F1`, `<leader>h` (`help.open`) and `/help` open the help modal with
//!   what [`SessionController::help`] reports.
//! - Any other `/name args` goes to [`SessionController::run_command`]
//!   (commands of `init.lua`, `/mcp`, `/plugin`, `/implement`, `/research`,
//!   `/simulate`, `/!cmd`); a name with a dot runs that builtin action
//!   (`/picker.files`).
//! - Recompute on a statistics card runs [`SessionController::recompute`].
//! - A mapping to a Lua function runs [`SessionController::run_callback`].
//!   What it answers ([`Reply`]) is shown or run here: `app.cmd(name)` as a
//!   `/` command and `app.send(text, { role })` as a prompt with that role.
//!
//! The status bar shows the git branch and changes of the workspace
//! ([`SessionController::git_status`]), refreshed at start, after every
//! turn or command and every few seconds.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use application_core::keymap::{Action, Keymap, default_keymap};
use application_core::ports::events::{DomainEvent, NoticeLevel};
use application_core::ports::fs::FileSystemPort;
use application_core::ports::plugin::PluginAction;
use application_core::ports::search::{ContentSearchPort, FileSearchPort};
use application_core::usecases::cancel::CancelToken;
use application_core::usecases::permissions::RuleListing;
use domain::block::Block;
use domain::conversation::{ImageAttachment, StopReason};
use domain::git::GitStatus;
use domain::model::{Account, ModelId, ModelRole};
use domain::permission::{PermissionMode, PermissionRule};
use domain::workspace::WorkspacePath;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, Render, Subscription, Task, WeakEntity, Window, div,
};

use crate::activity::ActivityView;
use crate::app::RootView;
use crate::approval::{ApprovalInbox, ApprovalView};
use crate::blocks::sources::{BlockEvent, block_events};
use crate::help::{HelpSnapshot, LocalHelp};
use crate::input::InputView;
use crate::login::LoginView;
use crate::modal::ModalHost;
use crate::picker::{PickerEvent, PickerView};
use crate::status::StatusView;
use crate::timeline::TimelineView;
use crate::tree::{TreePanel, TreePanelEvent};

mod commands;
mod keys;
mod login;
mod model;
mod overlays;
mod queue;
mod theme;

use model::mode_rows;
use queue::Queued;

/// Action of `Ctrl+C`.
pub const INTERRUPT_ACTION: &str = "turn.interrupt";
/// Action of `<leader>p`.
pub const PERMISSION_ACTION: &str = "permission.cycle";
/// Action of `<leader>m`.
pub const MODEL_ACTION: &str = "model.pick";
/// Action of `<C-Tab>` and `<leader>w`.
pub const FOCUS_NEXT_ACTION: &str = "focus.next";
/// Action of `<P-q>` (Ctrl+Q, Cmd+Q on macOS).
pub const QUIT_ACTION: &str = "app.quit";
/// Action of `<F11>` (and Ctrl+Cmd+F on macOS).
pub const FULLSCREEN_ACTION: &str = "window.fullscreen";
/// Action of `<leader>T`.
pub const THEME_ACTION: &str = "theme.pick";
/// Action of `<leader>P`.
pub const MODE_ACTION: &str = "mode.pick";
/// Action of `<leader>A` and `/permissions`.
pub const PERMISSIONS_ACTION: &str = "permissions.open";
/// Action of `<leader>W`.
pub const PROJECT_ACTION: &str = "project.switch";
/// Action of `<leader>O`.
pub const OUTLINE_ACTION: &str = "timeline.outline";
/// Action of `<leader><CR>`.
pub const FOCUS_WIDGET_ACTION: &str = "block.focus_widget";

/// How long a second quit has to come while a turn runs.
pub const QUIT_CONFIRM: Duration = Duration::from_secs(3);

/// How often the git state of the status bar is refreshed while idle.
const GIT_REFRESH: Duration = Duration::from_secs(5);

/// One turn asked by the UI.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// What the user typed.
    pub prompt: String,
    /// Images pasted with the prompt, sent after it in the same message.
    pub images: Vec<ImageAttachment>,
    /// Alias, role or id for this turn only (`<leader>m`, `/model`).
    pub model_override: Option<String>,
    /// Role of the turn instead of `main` (`app.send(text, { role })`).
    pub role: Option<ModelRole>,
    /// Cancelled by `Ctrl+C`.
    pub cancel: CancelToken,
}

/// How a turn sent from the UI ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// The turn finished (`Interrupted` after `Ctrl+C`).
    Finished(StopReason),
    /// The turn failed; the controller already emitted `TurnFailed`.
    Failed(String),
    /// The agent cannot run turns at all (no API key...); shown as an
    /// error block.
    Unavailable(String),
}

/// What a command or a Lua callback run by the controller asks the
/// session to do once it finished.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Nothing to show.
    Done,
    /// A message for the status bar.
    Notice(NoticeLevel, String),
    /// A block for the timeline (boxed: blocks are large).
    Block(Box<Block>),
    /// Actions requested by Lua (`app.cmd`, `app.send`), in order.
    Actions(Vec<PluginAction>),
}

impl Reply {
    /// A reply that shows `block`.
    #[must_use]
    pub fn block(block: Block) -> Self {
        Self::Block(Box::new(block))
    }
}

/// A model the menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// Name to send as the override (`opus`).
    pub alias: String,
    /// What it resolves to.
    pub model: ModelId,
}

/// A theme the `/theme` popup offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeChoice {
    /// Its name (`tokyonight-night`).
    pub name: String,
    /// Where it comes from: `built in`, `file` or `init.lua`.
    pub origin: String,
    /// The theme, resolved, for the preview.
    pub theme: crate::app::ColorTheme,
}

/// A theme saved with `/theme set`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTheme {
    /// The theme to show now.
    pub theme: crate::app::ColorTheme,
    /// What to tell the user (saved, or that `init.lua` wins at start).
    pub notice: String,
}

/// The input port of the session, implemented by the binary over the
/// `AgentService`.
///
/// The returned futures are polled on GPUI's executor: they must not need
/// any other runtime (send the work to it and await a channel instead).
pub trait SessionController: Send + Sync {
    /// Runs one turn; its events go to the controller's event sink. The
    /// future resolves once the turn ended and the session was saved.
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd>;

    /// The current permission mode.
    fn permission_mode(&self) -> PermissionMode;

    /// Changes the permission mode for the next decisions.
    fn set_permission_mode(&self, mode: PermissionMode);

    /// The models `<leader>m` offers.
    fn models(&self) -> Vec<ModelChoice>;

    /// The permission rules in force, for `/permissions`, with the
    /// problems of the project file. The default has none.
    fn permission_rules(&self) -> BoxFuture<'static, RuleListing> {
        futures::future::ready(RuleListing::default()).boxed()
    }

    /// Removes `rule` from the policy (and from the project file, for a
    /// project rule); `Err` says why it stays.
    fn remove_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let _ = rule;
        futures::future::ready(Err("this session cannot change its rules".to_owned())).boxed()
    }

    /// Makes the session rule `rule` a project rule, written to the
    /// project file; `Err` says why it stays a session rule.
    fn promote_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let _ = rule;
        futures::future::ready(Err("this session cannot change its rules".to_owned())).boxed()
    }

    /// Every model the providers with a key offer, for the `/model`
    /// picker; `Err` says why they could not be listed.
    fn catalog_models(&self) -> BoxFuture<'static, Result<Vec<ModelChoice>, String>> {
        futures::future::ready(Ok(Vec::new())).boxed()
    }

    /// Saves `model` as the user's pick for the default provider; `Ok`
    /// says what changed. The session then reopens so every role uses it.
    fn select_model(&self, model: ModelId) -> BoxFuture<'static, Result<String, String>> {
        let _ = model;
        futures::future::ready(Err("this session cannot save a model".to_owned())).boxed()
    }

    /// Every theme, resolved, for the `/theme` popup; `Err` says why they
    /// could not be listed.
    fn themes(&self) -> BoxFuture<'static, Result<Vec<ThemeChoice>, String>> {
        futures::future::ready(Ok(Vec::new())).boxed()
    }

    /// Saves `name` as the user's theme and resolves it.
    fn save_theme(&self, name: String) -> BoxFuture<'static, Result<SavedTheme, String>> {
        let _ = name;
        futures::future::ready(Err("this session cannot save a theme".to_owned())).boxed()
    }

    /// Adds a line sent from the prompt to the session's history, saved
    /// with it.
    fn record_prompt(&self, line: String) {
        let _ = line;
    }

    /// Starts a new, empty session. Only called with no turn running.
    fn new_session(&self) -> BoxFuture<'static, ()>;

    /// Continues the turn a restart cut while `ask_user` waited: asks the
    /// form again and goes on once it is answered. The default does
    /// nothing.
    fn resume_pending(&self, cancel: CancelToken) -> BoxFuture<'static, TurnEnd> {
        let _ = cancel;
        futures::future::ready(TurnEnd::Finished(StopReason::EndTurn)).boxed()
    }

    /// Stores `title` on the open chat and saves the session, so the next
    /// save keeps the name. The default fails: this session has no store.
    fn set_chat_title(&self, title: String) -> BoxFuture<'static, Result<(), String>> {
        let _ = title;
        futures::future::ready(Err("this session cannot rename its chat".into())).boxed()
    }

    /// Runs the Lua function `id` of a mapping with `args`.
    fn run_callback(&self, id: u64, args: serde_json::Value) -> BoxFuture<'static, Reply> {
        let _ = (id, args);
        futures::future::ready(Reply::Notice(
            NoticeLevel::Warning,
            "no plugins are loaded".into(),
        ))
        .boxed()
    }

    /// Re-runs the envelope of the computed `block` (Recompute on a
    /// statistics card), re-reading its data file; the new version comes
    /// back as a `BlockUpdated` event with `verification` filled in.
    /// `Err` says why it could not run.
    fn recompute(&self, block: Block) -> BoxFuture<'static, Result<(), String>> {
        let _ = block;
        futures::future::ready(Err("this session cannot recompute".to_owned())).boxed()
    }

    /// Adds `images` to the chat as image blocks (after `caption`, when not
    /// empty), saved with the session. The model is not called.
    fn post_images(
        &self,
        caption: String,
        images: Vec<ImageAttachment>,
    ) -> BoxFuture<'static, Reply> {
        let _ = (caption, images);
        futures::future::ready(Reply::Notice(
            NoticeLevel::Warning,
            "images cannot be added to this chat".into(),
        ))
        .boxed()
    }

    /// Runs the `/name args` command, if the controller has one by that
    /// name; `None` means there is none.
    fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
        let _ = (name, args);
        None
    }

    /// Stops the commands it is running (`Ctrl+C`); returns whether there
    /// was one. A stopped flow stays saved and can be resumed.
    fn interrupt_commands(&self) -> bool {
        false
    }

    /// The git state of the workspace for the status bar; `None` outside a
    /// repository.
    fn git_status(&self) -> BoxFuture<'static, Option<GitStatus>> {
        futures::future::ready(None).boxed()
    }

    /// Checks and stores the API key of `account` typed in the `/login`
    /// dialog; `Ok` says what happened, `Err` why the key was not stored.
    fn login(&self, account: Account, key: String) -> BoxFuture<'static, Result<String, String>> {
        let _ = (account, key);
        futures::future::ready(Err("this session cannot store API keys".to_owned())).boxed()
    }

    /// `/logout [account]`: deletes the stored API key of `account`, or of
    /// every account.
    fn logout(&self, account: Option<Account>) -> BoxFuture<'static, Result<String, String>> {
        let _ = account;
        futures::future::ready(Err("this session cannot store API keys".to_owned())).boxed()
    }

    /// What the help shows beyond what the UI knows by itself (its builtin
    /// commands, its keymap and the models of [`models`](Self::models)):
    /// the controller's commands, tools, plugins, MCP servers, flags and
    /// paths. Rows it leaves empty are filled in by the session.
    fn help(&self) -> BoxFuture<'static, HelpSnapshot> {
        futures::future::ready(HelpSnapshot::default()).boxed()
    }
}

/// The block that shows a prompt sent by the user in the timeline.
pub use application_core::usecases::user_blocks::prompt_block;

/// What `@` reference a file chosen in the tree or a picker inserts.
/// A path with spaces is quoted: `@"My Docs/a.rs":12 `.
pub fn file_reference(path: &WorkspacePath, line: Option<u64>) -> String {
    let at = crate::input::paths::at_path(&path.to_string());
    match line {
        Some(line) => format!("{at}:{line} "),
        None => format!("{at} "),
    }
}

/// How long `/login` waits for the provider to check a key.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything the session needs from the binary.
pub struct SessionParts {
    /// The agent.
    pub controller: Arc<dyn SessionController>,
    /// Events of the controller's sink.
    pub events: UnboundedReceiver<DomainEvent>,
    /// Requests of the `UiApproval` given to the agent.
    pub approvals: ApprovalInbox,
    /// Requests of the `UiForm` given to `ask_user`, if any; each is
    /// answered in its `Form` block.
    pub forms: Option<crate::form::FormInbox>,
    /// File search for the tree and `<leader>ff`.
    pub files: Arc<dyn FileSearchPort>,
    /// Text search for `<leader>fg`.
    pub content: Arc<dyn ContentSearchPort>,
    /// Reads the files the pickers preview; without it the text picker
    /// previews the context lines of its matches only.
    pub fs: Option<Arc<dyn FileSystemPort>>,
    /// Workspace name for the status bar.
    pub workspace_label: String,
    /// Workspace directory the tree watches, if any.
    pub workspace_dir: Option<PathBuf>,
    /// Model shown before the first turn.
    pub model: Option<ModelId>,
    /// Keys of the input.
    pub keymap: Keymap<Action>,
    /// Wait for the next key of a sequence (`app.opt.timeoutlen`).
    pub timeoutlen: Option<Duration>,
    /// Timeline of a resumed session, shown before any event.
    pub history: Vec<Block>,
    /// Lines sent from the prompt of a resumed session, oldest first.
    pub prompt_history: Vec<String>,
    /// Color theme (`app.opt.theme`), applied before the window opens.
    pub theme: crate::app::ColorTheme,
    /// The agent has no API key: the `/login` dialog opens at once.
    pub needs_login: bool,
    /// No model is selected for the default provider: the `/model` popup
    /// opens at once.
    pub needs_model: bool,
    /// Id of the open session, so the chats panel can mark it.
    pub session_id: Option<String>,
    /// The saved session ends in an `ask_user` call nobody answered (the
    /// process ended while the form waited): the session asks it again as
    /// soon as it opens ([`SessionController::resume_pending`]).
    pub resume_pending: bool,
}

impl SessionParts {
    /// Parts with the default keymap and no directory watching.
    pub fn new(
        controller: Arc<dyn SessionController>,
        events: UnboundedReceiver<DomainEvent>,
        approvals: ApprovalInbox,
        files: Arc<dyn FileSearchPort>,
        content: Arc<dyn ContentSearchPort>,
    ) -> Self {
        Self {
            controller,
            events,
            approvals,
            forms: None,
            files,
            content,
            fs: None,
            workspace_label: String::new(),
            workspace_dir: None,
            model: None,
            keymap: default_keymap(),
            timeoutlen: None,
            history: Vec::new(),
            prompt_history: Vec::new(),
            theme: crate::theme::default_theme(),
            needs_login: false,
            needs_model: false,
            session_id: None,
            resume_pending: false,
        }
    }
}

/// What the session asks the window ([`crate::workbench::Workbench`]) to do.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Open the workspace again (the API key changed), then show `notice`.
    Reopen {
        /// What to tell once the new session shows.
        notice: Option<(NoticeLevel, String)>,
    },
    /// A key or a command asks the window about the chats (the sidebar
    /// lives beside the session): `chat.new`, `focus.next` into the list...
    Chats(crate::chats::ChatsCommand),
    /// The session got busy or idle, or started or stopped waiting on the
    /// user (an approval, a form). Sent on each change.
    Activity {
        /// A turn or a `/` command runs.
        busy: bool,
        /// The turn waits for an approval or a form.
        waiting: bool,
    },
}

/// The session: owns the views of the main window and the turn in flight.
pub struct Session {
    controller: Arc<dyn SessionController>,
    root: WeakEntity<RootView>,
    timeline: Entity<TimelineView>,
    input: Entity<InputView>,
    status: Entity<StatusView>,
    activity: Entity<ActivityView>,
    tree: Entity<TreePanel>,
    picker: Entity<PickerView>,
    approval: Entity<ApprovalView>,
    modal: Entity<ModalHost>,
    login: Entity<LoginView>,
    focus_handle: FocusHandle,
    running: Option<CancelToken>,
    /// Names of the `/` commands waiting for the controller's reply.
    commands: Vec<String>,
    turn: Option<Task<()>>,
    /// `/clear` was asked during a turn: the turn was interrupted and the
    /// session resets once it has ended.
    pending_clear: bool,
    /// Whether the session is on screen. Parked (its chat left while a
    /// turn ran) it keeps running but takes no keys and no focus.
    shown: bool,
    /// The last [`SessionEvent::Activity`] sent, to send only changes.
    last_activity: Option<(bool, bool)>,
    queue: VecDeque<Queued>,
    model_override: Option<String>,
    /// The catalogs' models, listed once per session for the popup.
    catalog_models: Option<Vec<ModelChoice>>,
    /// The theme in use (not a `/theme` preview).
    theme: crate::app::ColorTheme,
    /// The themes the `/theme` popup lists, for its previews.
    theme_choices: Vec<ThemeChoice>,
    git: Option<Task<()>>,
    /// The `/login` key being checked; dropping it cancels the check.
    login_check: Option<Task<()>>,
    /// The window shows the chats sidebar: `focus.next` goes through it.
    chats_shown: bool,
    /// When quitting was asked during a turn: a second ask within
    /// [`QUIT_CONFIRM`] quits.
    quit_asked_at: Option<Instant>,
    /// `m` of the `/permissions` modal (replaced when it opens again).
    permissions_events: Option<Subscription>,
    _git_timer: Task<()>,
    _events: Task<()>,
    _forms: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Builds the root view of the main window with a session in it; the input
/// gets the focus. The root keeps the session alive (it is one of its
/// overlays).
pub fn open(
    parts: SessionParts,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<RootView>, Entity<Session>) {
    let root = cx.new(RootView::new);
    let session = cx.new(|cx| Session::new(parts, root.downgrade(), window, cx));
    let s = session.read(cx);
    let login = s.login.clone();
    let (timeline, input, status, activity, tree, picker, approval, modal) = (
        s.timeline.clone(),
        s.input.clone(),
        s.status.clone(),
        s.activity.clone(),
        s.tree.clone(),
        s.picker.clone(),
        s.approval.clone(),
        s.modal.clone(),
    );
    root.update(cx, |root, cx| {
        root.set_timeline(timeline, cx);
        root.set_activity(activity, cx);
        root.set_input(input.clone(), cx);
        root.set_status(status, cx);
        let panel = tree.clone();
        root.set_side_panel(tree, move |cx| panel.read(cx).width(), cx);
        root.push_overlay(picker, cx);
        root.push_overlay(session.clone(), cx);
        // Plugin modals sit below the approval dialog, which must stay
        // reachable while one is open.
        root.push_cached_overlay(modal, cx);
        root.push_overlay(login, cx);
        root.push_overlay(approval, cx);
    });
    session.update(cx, |session, cx| session.focus_input(window, cx));
    (root, session)
}

impl Session {
    fn new(
        parts: SessionParts,
        root: WeakEntity<RootView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let SessionParts {
            controller,
            events,
            approvals,
            forms,
            files,
            content,
            fs,
            workspace_label,
            workspace_dir,
            model,
            keymap,
            timeoutlen,
            history,
            prompt_history,
            theme,
            needs_login,
            needs_model,
            session_id: _,
            resume_pending,
        } = parts;
        let timeline = cx.new(|cx| {
            let mut timeline = TimelineView::new(cx);
            let restored = history
                .into_iter()
                .map(|block| DomainEvent::BlockAdded { block });
            timeline.apply_all(restored, cx);
            // The welcome screen names the keys of this keymap.
            timeline.set_welcome_keys(&keymap, cx);
            timeline
        });
        let status = cx.new(|_| StatusView::new(workspace_label));
        // The approval dialog names the keys that undo "allow all edits".
        let mode_keys = crate::help::action_keys(&keymap, PERMISSION_ACTION);
        let mode = controller.permission_mode();
        status.update(cx, |status, cx| {
            let state = status.state_mut(cx);
            state.set_permission_mode(mode);
            if let Some(model) = model.clone() {
                state.set_model(model);
            }
        });
        let input = cx.new(|cx| {
            let mut input = InputView::with_keymap(keymap, window, cx);
            input.set_current_model(model);
            input.set_current_theme(&theme.name);
            input.set_file_search(files.clone());
            input.set_history(prompt_history);
            if let Some(timeoutlen) = timeoutlen {
                input.sequencer_mut().set_timeoutlen(timeoutlen);
            }
            input
        });
        let activity = cx.new(|_| ActivityView::new());
        let tree = cx.new(|cx| TreePanel::new(files.clone(), cx));
        let workspace_root = fs.as_ref().map(|fs| fs.root().clone());
        let picker = cx.new(|cx| {
            let mut picker = PickerView::new(files, content, window, cx);
            picker.set_file_system(fs);
            picker
        });
        let approval = cx.new(|cx| {
            let mut approval = ApprovalView::new(approvals, window, cx);
            approval.set_mode_keys(mode_keys);
            approval
        });
        let modal = cx.new(|cx| {
            let mut modal = ModalHost::new(window, cx);
            modal.set_workspace_root(workspace_root);
            modal
        });
        let login = cx.new(|cx| LoginView::new(window, cx));

        if let Some(dir) = &workspace_dir {
            tree.update(cx, |tree, cx| tree.watch_dir(dir, cx));
        }

        let mut subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input),
            cx.subscribe_in(
                &tree,
                window,
                |this, _, event: &TreePanelEvent, window, cx| match event {
                    TreePanelEvent::Open(path) => {
                        this.insert_reference(&file_reference(path, None), window, cx);
                    }
                    TreePanelEvent::WatchFailed(message) => {
                        this.notice(NoticeLevel::Warning, message.clone(), cx);
                    }
                },
            ),
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &PickerEvent, window, cx| {
                    match event {
                        PickerEvent::Open { path, line } => {
                            this.insert_reference(&file_reference(path, *line), window, cx);
                        }
                        // The picker closed: back to the prompt.
                        PickerEvent::Dismissed => {
                            this.input.update(cx, |input, cx| input.focus(window, cx));
                        }
                    }
                },
            ),
            Self::intercept_keys(cx),
            cx.subscribe_in(&login, window, Self::on_login),
        ];
        // A file cited by a block (Sources) opens like a picker choice; a
        // clicked image opens in a modal to zoom in.
        let block_events = block_events(cx);
        subscriptions.push(cx.subscribe_in(
            &block_events,
            window,
            |this, _, event: &BlockEvent, window, cx| {
                // The blocks on screen belong to the shown session.
                if !this.shown {
                    return;
                }
                match event {
                    BlockEvent::OpenFile { path, line } => {
                        this.insert_reference(&file_reference(path, *line), window, cx);
                    }
                    BlockEvent::InspectImage {
                        block,
                        media_type,
                        data,
                    } => this.inspect_image(*block, media_type, data, window, cx),
                    BlockEvent::Recompute { block } => this.recompute(*block, cx),
                    BlockEvent::OpenUrl { .. } => {}
                }
            },
        ));

        let mut this = Self {
            controller,
            root,
            timeline,
            input,
            status,
            activity,
            tree,
            picker,
            approval,
            modal,
            login,
            focus_handle: cx.focus_handle(),
            running: None,
            commands: Vec::new(),
            turn: None,
            pending_clear: false,
            shown: true,
            last_activity: None,
            queue: VecDeque::new(),
            model_override: None,
            catalog_models: None,
            theme,
            theme_choices: Vec::new(),
            git: None,
            login_check: None,
            chats_shown: false,
            quit_asked_at: None,
            permissions_events: None,
            _git_timer: Self::git_timer(cx),
            _events: Self::consume(events, cx),
            _forms: forms.map(|forms| Self::listen_forms(forms, window, cx)),
            _subscriptions: subscriptions,
        };
        this.load_commands(window, cx);
        this.refresh_git(cx);
        if resume_pending {
            this.resume(cx);
        }
        if needs_login {
            this.login
                .update(cx, |login, cx| login.show(Account::Anthropic, window, cx));
        } else if needs_model {
            this.notice(
                NoticeLevel::Info,
                "pick the model crest uses with this provider (Enter saves it)",
                cx,
            );
            this.open_model_popup(window, cx);
        }
        this
    }

    /// Gives the `/` palette every command the help knows.
    fn load_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.controller.help();
        cx.spawn_in(window, async move |this, cx| {
            let snapshot = snapshot.await;
            this.update(cx, |this, cx| {
                let keymap = this.input.read(cx).keymap().clone();
                let models = this.controller.models();
                let full = snapshot.complete(&LocalHelp {
                    keymap: &keymap,
                    models: &models,
                    permission_mode: this.controller.permission_mode(),
                    turn_override: None,
                });
                let mut commands = full.commands;
                commands.extend(mode_rows());
                this.input
                    .update(cx, |input, cx| input.set_commands(commands, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Asks the controller for the git state and shows it, unless a
    /// refresh is already on its way.
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if self.git.is_some() {
            return;
        }
        let status = self.controller.git_status();
        self.git = Some(cx.spawn(async move |this, cx| {
            let status = status.await;
            this.update(cx, |this, cx| {
                this.git = None;
                this.status.update(cx, |view, cx| view.set_git(status, cx));
            })
            .ok();
        }));
    }

    /// Refreshes the git state every [`GIT_REFRESH`].
    fn git_timer(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(GIT_REFRESH).await;
                if this.update(cx, |this, cx| this.refresh_git(cx)).is_err() {
                    break;
                }
            }
        })
    }

    /// Attaches every `ask_user` request to its `Form` block. The form takes
    /// the focus only while the session is shown; a parked one gets it when
    /// the session comes back ([`Self::set_shown`]).
    fn listen_forms(
        mut inbox: crate::form::FormInbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(pending) = inbox.next().await {
                let attached = this.update_in(cx, |this, window, cx| {
                    crate::blocks::form::attach(pending, this.shown, window, cx);
                    this.sync_activity(cx);
                });
                if attached.is_err() {
                    break;
                }
            }
        })
    }

    /// Whether the session is on screen.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// Tells the session whether it is on screen. Shown again, it takes
    /// the focus back: the approval dialog if one is open, else the form
    /// waiting for an answer, else the prompt.
    pub fn set_shown(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        self.approval
            .update(cx, |dialog, cx| dialog.set_shown(shown, window, cx));
        if shown && !self.approval.read(cx).is_open() {
            match self.waiting_form(cx) {
                Some(form) => form.update(cx, |form, cx| form.focus(window, cx)),
                None => self.focus_input(window, cx),
            }
        }
        self.sync_activity(cx);
        cx.notify();
    }

    /// The form of this session waiting for an answer, if any.
    pub fn waiting_form(&self, cx: &App) -> Option<Entity<crate::blocks::form::FormView>> {
        let state = self.timeline.read(cx).state();
        crate::blocks::form::waiting_forms(cx)
            .into_iter()
            .find(|(id, _)| state.position(*id).is_some())
            .map(|(_, form)| form)
    }

    /// The single consumer of the controller's events.
    fn consume(mut events: UnboundedReceiver<DomainEvent>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update(cx, |this, cx| this.dispatch_all(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// Gives the focus back to the prompt.
    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The key dialog keeps the focus while it is open.
        if self.login.read(cx).is_open() {
            self.login.update(cx, |login, cx| login.focus(window, cx));
        } else {
            self.input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// The `/login` dialog.
    pub fn login_view(&self) -> &Entity<LoginView> {
        &self.login
    }

    /// Shows `message` in the status bar.
    pub fn show_notice(&mut self, level: NoticeLevel, message: String, cx: &mut Context<Self>) {
        self.notice(level, message, cx);
    }

    /// Tells the session whether the window shows the chats sidebar, so
    /// `focus.next` goes through it.
    pub fn set_chats_shown(&mut self, shown: bool) {
        self.chats_shown = shown;
    }

    /// `app.quit` and `/quit`. While a turn runs the first ask only warns;
    /// a second one within [`QUIT_CONFIRM`] quits.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let confirmed = self
            .quit_asked_at
            .is_some_and(|at| now.saturating_duration_since(at) < QUIT_CONFIRM);
        if self.running.is_some() && !confirmed {
            self.quit_asked_at = Some(now);
            let keys = crate::help::action_keys(self.input.read(cx).keymap(), QUIT_ACTION)
                .unwrap_or_else(|| "/quit".to_owned());
            let message = format!(
                "a turn is running; {keys} again within {} s quits anyway",
                QUIT_CONFIRM.as_secs()
            );
            self.notice(NoticeLevel::Warning, message, cx);
            return;
        }
        self.quit_asked_at = None;
        window.dispatch_action(Box::new(crate::app::Quit), cx);
    }

    /// Whether the session waits for a second quit.
    pub fn quit_pending(&self) -> bool {
        self.quit_asked_at.is_some()
    }

    /// The timeline view (replaced by `/clear`).
    pub fn timeline(&self) -> &Entity<TimelineView> {
        &self.timeline
    }

    /// The prompt input.
    pub fn input(&self) -> &Entity<InputView> {
        &self.input
    }

    /// The status bar.
    pub fn status(&self) -> &Entity<StatusView> {
        &self.status
    }

    /// The activity line and the queue.
    pub fn activity(&self) -> &Entity<ActivityView> {
        &self.activity
    }

    /// The approval dialog.
    pub fn approval(&self) -> &Entity<ApprovalView> {
        &self.approval
    }

    /// The plugin modals.
    pub fn modal(&self) -> &Entity<ModalHost> {
        &self.modal
    }

    /// Whether a turn is running.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Whether a turn or a `/` command is running; prompts are queued.
    pub fn is_busy(&self) -> bool {
        self.running.is_some() || !self.commands.is_empty()
    }

    /// Renames the open chat and saves it.
    pub fn retitle(&self, title: String) -> BoxFuture<'static, Result<(), String>> {
        self.controller.set_chat_title(title)
    }

    /// Prompts waiting for the session to be free.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The override of the next turn.
    pub fn model_override(&self) -> Option<&str> {
        self.model_override.as_deref()
    }

    /// Applies an event to the timeline and the status bar.
    pub fn dispatch(&mut self, event: DomainEvent, cx: &mut Context<Self>) {
        self.dispatch_all(vec![event], cx);
    }

    fn dispatch_all(&mut self, events: Vec<DomainEvent>, cx: &mut Context<Self>) {
        let (modal, events): (Vec<_>, Vec<_>) = events.into_iter().partition(DomainEvent::is_modal);
        let host = self.modal.read(cx);
        for event in modal {
            host.post(event);
        }
        if events.is_empty() {
            return;
        }
        self.status.update(cx, |status, cx| {
            for event in &events {
                status.apply(event, cx);
            }
        });
        let mode = events.iter().rev().find_map(|event| match event {
            DomainEvent::ModeChanged { mode } => Some(*mode),
            _ => None,
        });
        if let Some((mode, view)) = mode.zip(self.permissions_view(cx)) {
            view.update(cx, |view, cx| view.set_mode(mode, cx));
        }
        self.activity.update(cx, |activity, cx| {
            for event in &events {
                activity.apply(event, cx);
            }
        });
        self.timeline
            .update(cx, |timeline, cx| timeline.apply_all(events, cx));
    }

    fn notice(&mut self, level: NoticeLevel, message: impl Into<String>, cx: &mut Context<Self>) {
        self.dispatch(
            DomainEvent::Notice {
                level,
                message: message.into(),
            },
            cx,
        );
    }
}

impl EventEmitter<SessionEvent> for Session {}

impl Render for Session {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().id("session").track_focus(&self.focus_handle)
    }
}

#[cfg(test)]
mod tests;
