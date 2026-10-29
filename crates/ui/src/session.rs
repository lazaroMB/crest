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
//! it replies. A prompt submitted while the session is busy is **queued**:
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
use std::time::Duration;

use application::keymap::{Action, Key, KeyCode, Keymap, default_keymap};
use application::ports::events::{DomainEvent, NoticeLevel};
use application::ports::fs::FileSystemPort;
use application::ports::plugin::PluginAction;
use application::ports::search::{ContentSearchPort, FileSearchPort};
use application::usecases::cancel::CancelToken;
use domain::block::{Block, BlockId, BlockKind, CustomNode, ImageData, ImageFit};
use domain::conversation::{ImageAttachment, StopReason};
use domain::git::GitStatus;
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use domain::model::{Account, ModelId, ModelRole};
use domain::permission::PermissionMode;
use domain::workspace::WorkspacePath;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeystrokeEvent, Render, Subscription, Task, WeakEntity,
    Window, div,
};

use crate::activity::{ActivityView, WorkKind};
use crate::app::RootView;
use crate::approval::{ApprovalInbox, ApprovalView};
use crate::blocks::sources::{BlockEvent, block_events};
use crate::commands::unknown_command_hint;
use crate::help::{
    HELP_ACTION, HelpSnapshot, HelpView, LocalHelp, Section, help_modal_id, parse_help_arg,
};
use crate::input::attachments::ImageMode;
use crate::input::{
    FOCUS_INPUT_ACTION, InputEvent, InputView, QUEUE_EDIT_ACTION, key_from_keystroke,
};
use crate::login::{LoginEvent, LoginView};
use crate::modal::ModalHost;
use crate::picker::{PickerEvent, PickerView};
use crate::status::StatusView;
use crate::timeline::{DeferredKey, TimelineView};
use crate::tree::{TreePanel, TreePanelEvent};

/// Action of `Ctrl+C`.
pub const INTERRUPT_ACTION: &str = "turn.interrupt";
/// Action of `<leader>p`.
pub const PERMISSION_ACTION: &str = "permission.cycle";
/// Action of `<leader>m`.
pub const MODEL_ACTION: &str = "model.pick";
/// Action of `<C-Tab>` and `<leader>w`.
pub const FOCUS_NEXT_ACTION: &str = "focus.next";

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

/// Share of the window an inspected image takes, in percent.
const INSPECT_PERCENT: f32 = 90.0;

/// A prompt waiting for the session to be free.
#[derive(Debug, Clone)]
struct Queued {
    prompt: String,
    role: Option<ModelRole>,
    images: Vec<ImageAttachment>,
}

impl Queued {
    /// What the activity line shows for it.
    fn label(&self) -> String {
        match self.images.len() {
            0 => self.prompt.clone(),
            1 => format!("{} [+1 image]", self.prompt),
            n => format!("{} [+{n} images]", self.prompt),
        }
    }
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

/// How many times a command may lead to another one (`app.cmd` inside a
/// command), so a command calling itself stops.
const MAX_COMMAND_DEPTH: usize = 4;

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

/// Text a key typed outside the prompt puts in it: a printable character
/// without Ctrl, Alt or Cmd.
fn typed_text(key: &Key) -> Option<String> {
    if key.mods.ctrl || key.mods.alt || key.mods.cmd {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c.to_string()),
        _ => None,
    }
}

/// The block that shows a prompt sent by the user in the timeline.
pub use application::usecases::user_blocks::prompt_block;

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
        root.set_side_panel(tree, cx);
        root.push_overlay(picker, cx);
        root.push_overlay(session.clone(), cx);
        // Plugin modals sit below the approval dialog, which must stay
        // reachable while one is open.
        root.push_overlay(modal, cx);
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
            timeline
        });
        let status = cx.new(|_| StatusView::new(workspace_label));
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
        let picker = cx.new(|cx| {
            let mut picker = PickerView::new(files, content, window, cx);
            picker.set_file_system(fs);
            picker
        });
        let approval = cx.new(|cx| ApprovalView::new(approvals, window, cx));
        let modal = cx.new(|cx| ModalHost::new(window, cx));
        let login = cx.new(|cx| LoginView::new(window, cx));

        let mut watch_error = None;
        if let Some(dir) = &workspace_dir {
            tree.update(cx, |tree, cx| {
                if let Err(err) = tree.watch_dir(dir, cx) {
                    watch_error = Some(format!("the tree will not refresh by itself: {err}"));
                }
            });
        }

        let mut subscriptions = vec![
            cx.subscribe_in(&input, window, Self::on_input),
            cx.subscribe_in(
                &tree,
                window,
                |this, _, event: &TreePanelEvent, window, cx| {
                    let TreePanelEvent::Open(path) = event;
                    this.insert_reference(&file_reference(path, None), window, cx);
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
            _git_timer: Self::git_timer(cx),
            _events: Self::consume(events, cx),
            _forms: forms.map(|forms| Self::listen_forms(forms, window, cx)),
            _subscriptions: subscriptions,
        };
        if let Some(message) = watch_error {
            this.notice(NoticeLevel::Warning, message, cx);
        }
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
                this.input
                    .update(cx, |input, cx| input.set_commands(full.commands, cx));
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

    /// Keys typed while the focus is outside the prompt. In an overlay
    /// (picker, dialog, menu, modal) only `Ctrl+C` is taken. With the focus
    /// in the timeline, `Esc` and printable keys go first to the focused
    /// element (a block widget, the outline menu) and fall back to the
    /// prompt only when nothing takes them ([`DeferredKey`]). Elsewhere
    /// (the tree) the keys go through the input's keymap, any other
    /// printable key is typed into the prompt and the rest is left to the
    /// panel.
    fn intercept_keys(cx: &mut Context<Self>) -> Subscription {
        let weak = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            weak.update(cx, |this, cx| this.outside_key(event, window, cx))
                .ok();
        })
    }

    /// `Ctrl+C` with no other modifier (Shift makes it `Ctrl+Shift+C`).
    fn is_ctrl_c(key: &Key) -> bool {
        key.code == KeyCode::Char('c')
            && key.mods.ctrl
            && !key.mods.alt
            && !key.mods.cmd
            && !key.mods.shift
    }

    fn outside_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A parked session has no keyboard.
        if !self.shown {
            return;
        }
        // A key deferred before was taken by an element of the timeline.
        if self.timeline.read(cx).has_deferred_key() {
            self.timeline
                .update(cx, |timeline, _| timeline.clear_deferred_key());
        }
        // The dialog may be up while the prompt still has focus (Reject does
        // not move it). Take the keys here, before the prompt sees them.
        if self.approval.read(cx).is_open() {
            let handled = self.approval.update(cx, |dialog, cx| {
                dialog.handle_keystroke(&event.keystroke, window, cx)
            });
            if !handled
                && key_from_keystroke(&event.keystroke).is_some_and(|key| Self::is_ctrl_c(&key))
            {
                self.interrupt(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.input.read(cx).owns_focus(window, cx) {
            return;
        }
        // The chat rename field is a text input outside the prompt. Its
        // keys stay there; a printable key must not fall into the prompt.
        if crate::chats::rename_has_focus(window, cx) {
            return;
        }
        let Some(key) = key_from_keystroke(&event.keystroke) else {
            return;
        };
        if self.overlay_focused(window, cx) {
            let m = key.mods;
            if key.code == KeyCode::Char('c') && m.ctrl && !m.alt && !m.cmd && !m.shift {
                self.interrupt(cx);
                return;
            }
            // A waiting form keeps what is typed in it; a chord (`Ctrl+Tab`
            // to the prompt) still goes through the keymap, so the user can
            // leave the form and answer in the chat without the mouse. In
            // the picker too, minus its own `Ctrl+P`/`Ctrl+N` and `Alt+…`
            // toggles, and with the
            // keys after the leader: the shortcut that opened it closes it.
            let chord = m.ctrl || m.alt || m.cmd;
            let picker_keys = self.picker.read(cx).is_open()
                && !self.login.read(cx).is_open()
                && !self.modal.read(cx).is_open();
            let picker_own =
                (m.ctrl && !m.alt && !m.cmd && matches!(key.code, KeyCode::Char('p' | 'n')))
                    || (m.alt
                        && !m.ctrl
                        && !m.cmd
                        && crate::picker::PickerView::owns_alt_key(&key.code));
            let to_keymap = if crate::blocks::form::owns_focus(window, cx) {
                chord
            } else if picker_keys {
                (chord && !picker_own) || self.input.read(cx).has_pending_keys()
            } else {
                false
            };
            if to_keymap
                && self
                    .input
                    .update(cx, |input, cx| input.feed_key(key, window, cx))
            {
                cx.stop_propagation();
            }
            return;
        }
        // Ctrl+C with the chat focused and text selected in it copies, as
        // in Windows Terminal; without a selection it keeps its meaning.
        if Self::is_ctrl_c(&key)
            && self.timeline.focus_handle(cx).contains_focused(window, cx)
            && self
                .timeline
                .update(cx, |timeline, cx| timeline.copy_selection(window, cx))
        {
            cx.stop_propagation();
            return;
        }
        if self.defer_to_timeline(key, window, cx) {
            return;
        }
        let consumed = self
            .input
            .update(cx, |input, cx| input.feed_key(key, window, cx));
        if consumed {
            cx.stop_propagation();
            return;
        }
        if let Some(text) = typed_text(&key) {
            self.input
                .update(cx, |input, cx| input.insert_text(&text, window, cx));
            cx.stop_propagation();
        }
    }

    /// With the focus in the timeline, lets a plain `Esc` or printable key
    /// through to the focused element (it may close a menu, clear a
    /// cursor, zoom). The timeline runs [`Self::fallback_key`] if the key
    /// reaches it unhandled. A key after a pending prefix (the leader) is
    /// the keymap's. Returns whether the key was deferred.
    fn defer_to_timeline(&mut self, key: Key, window: &Window, cx: &mut Context<Self>) -> bool {
        let plain_esc = key.code == KeyCode::Esc
            && !(key.mods.ctrl || key.mods.alt || key.mods.cmd || key.mods.shift);
        if !(plain_esc || typed_text(&key).is_some())
            || self.input.read(cx).has_pending_keys()
            || !self.timeline.focus_handle(cx).contains_focused(window, cx)
        {
            return false;
        }
        let session = cx.entity().downgrade();
        let deferred = DeferredKey::new(key, move |window, cx| {
            session
                .update(cx, |this, cx| this.fallback_key(key, window, cx))
                .ok();
        });
        self.timeline
            .update(cx, |timeline, _| timeline.defer_key(deferred));
        true
    }

    /// A deferred key nothing in the timeline took: it goes through the
    /// keymap (`Esc` focuses the prompt) or is typed into the prompt.
    fn fallback_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let consumed = self
            .input
            .update(cx, |input, cx| input.feed_key(key, window, cx));
        if !consumed && let Some(text) = typed_text(&key) {
            self.input
                .update(cx, |input, cx| input.insert_text(&text, window, cx));
        }
    }

    /// Whether a picker, the approval dialog, a modal, the key dialog or a
    /// waiting form (or a widget's text input) has the focus. Keys typed there never reach the
    /// prompt (the API key must not land in it).
    fn overlay_focused(&self, window: &Window, cx: &App) -> bool {
        self.login.read(cx).is_open()
            || self.picker.read(cx).is_open()
            || self.approval.focus_handle(cx).contains_focused(window, cx)
            || self.modal.read(cx).is_open()
            || crate::blocks::form::owns_focus(window, cx)
            || crate::widgets::owns_focus(window, cx)
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

    /// `/login [account]`: opens the key dialog, once nothing runs.
    fn open_login(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.account_arg("/login", arg, cx) else {
            return;
        };
        if !self.idle_for("/login", cx) {
            return;
        }
        let account = account.unwrap_or(Account::Anthropic);
        self.login
            .update(cx, |login, cx| login.show(account, window, cx));
    }

    /// The account named by the argument of `command` (`None` when there
    /// is none); warns and returns `None` for an unknown one.
    fn account_arg(
        &mut self,
        command: &str,
        arg: &str,
        cx: &mut Context<Self>,
    ) -> Option<Option<Account>> {
        if arg.is_empty() {
            return Some(None);
        }
        match arg.parse::<Account>() {
            Ok(account) => Some(Some(account)),
            Err(_) => {
                let known = Account::ALL.map(Account::as_str).join(", ");
                let message = format!("{command}: unknown account `{arg}` (known: {known})");
                self.notice(NoticeLevel::Warning, message, cx);
                None
            }
        }
    }

    /// What the key dialog asks.
    fn on_login(
        &mut self,
        _: &Entity<LoginView>,
        event: &LoginEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            LoginEvent::Cancelled => {
                // A check in flight is dropped, which cancels it.
                self.login_check = None;
                self.focus_input(window, cx);
            }
            LoginEvent::Submit { account, key } => {
                let login = self.controller.login(*account, key.clone());
                let timeout = cx.background_executor().timer(LOGIN_TIMEOUT);
                let name = account.display_name();
                self.login_check = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = match futures::future::select(login, timeout).await {
                        futures::future::Either::Left((result, _)) => result,
                        futures::future::Either::Right(_) => Err(format!(
                            "{name} did not answer in {} s; check the network and try again",
                            LOGIN_TIMEOUT.as_secs()
                        )),
                    };
                    this.update_in(cx, |this, window, cx| {
                        this.login_check = None;
                        match result {
                            Ok(message) => {
                                this.login.update(cx, |login, cx| login.close(window, cx));
                                cx.emit(SessionEvent::Reopen {
                                    notice: Some((NoticeLevel::Info, message)),
                                });
                            }
                            Err(error) => {
                                this.login.update(cx, |login, cx| login.fail(error, cx));
                            }
                        }
                    })
                    .ok();
                }));
            }
        }
    }

    /// `/logout [account]`: deletes the stored key (of every account
    /// without an argument) and reopens the workspace.
    fn logout(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.account_arg("/logout", arg, cx) else {
            return;
        };
        if !self.idle_for("/logout", cx) {
            return;
        }
        let logout = self.controller.logout(account);
        cx.spawn_in(window, async move |this, cx| {
            let result = logout.await;
            this.update(cx, |this, cx| match result {
                Ok(message) => cx.emit(SessionEvent::Reopen {
                    notice: Some((NoticeLevel::Info, message)),
                }),
                Err(error) => this.notice(NoticeLevel::Warning, error, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Whether nothing runs; otherwise says that `command` must wait.
    fn idle_for(&mut self, command: &str, cx: &mut Context<Self>) -> bool {
        if self.running.is_some() {
            let message = format!("a turn is running; Ctrl+C to interrupt it before {command}");
            self.notice(NoticeLevel::Warning, message, cx);
            return false;
        }
        if let Some(running) = self.commands.last() {
            let message = format!("/{running} is running; {command} once it stops");
            self.notice(NoticeLevel::Warning, message, cx);
            return false;
        }
        true
    }

    /// `<C-Tab>`: prompt → timeline → tree (when shown) → prompt.
    pub fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let timeline = self.timeline.focus_handle(cx);
        let tree_shown = self.tree.read(cx).is_visible();
        if self.input.read(cx).owns_focus(window, cx) {
            window.focus(&timeline, cx);
            self.timeline
                .update(cx, |view, cx| view.select_last_if_none(cx));
        } else if timeline.contains_focused(window, cx) && tree_shown {
            self.tree.update(cx, |tree, cx| tree.focus(window, cx));
        } else {
            self.focus_input(window, cx);
        }
        cx.notify();
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

    /// Sends `prompt`, or queues it while a turn runs.
    pub fn submit(&mut self, prompt: String, cx: &mut Context<Self>) {
        self.submit_as(prompt, None, cx);
    }

    /// Sends `prompt` answered with `role` (`main` when `None`), or queues
    /// it while the session is busy.
    pub fn submit_as(&mut self, prompt: String, role: Option<ModelRole>, cx: &mut Context<Self>) {
        self.submit_with(prompt, role, Vec::new(), cx);
    }

    /// Sends `prompt` with the `images` pasted with it, or queues both
    /// while the session is busy.
    pub fn submit_with(
        &mut self,
        prompt: String,
        role: Option<ModelRole>,
        images: Vec<ImageAttachment>,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            self.queue.push_back(Queued {
                prompt,
                role,
                images,
            });
            self.sync_activity(cx);
            return;
        }
        let cancel = CancelToken::new();
        self.running = Some(cancel.clone());
        let turn = self.controller.run_turn(TurnRequest {
            prompt,
            images,
            model_override: self.model_override.take(),
            role,
            cancel,
        });
        self.turn = Some(cx.spawn(async move |this, cx| {
            let end = turn.await;
            this.update(cx, |this, cx| this.turn_ended(end, cx)).ok();
        }));
        self.sync_activity(cx);
        cx.notify();
    }

    /// Continues the turn a restart cut while a form waited: the controller
    /// asks the form again (in the block the timeline already shows) and
    /// goes on with the model once it is answered. Runs like a turn:
    /// Ctrl+C interrupts it and prompts sent meanwhile answer the form or
    /// wait in the queue.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let cancel = CancelToken::new();
        self.running = Some(cancel.clone());
        let turn = self.controller.resume_pending(cancel);
        self.turn = Some(cx.spawn(async move |this, cx| {
            let end = turn.await;
            this.update(cx, |this, cx| this.turn_ended(end, cx)).ok();
        }));
        self.notice(
            NoticeLevel::Info,
            "a form was left unanswered when crest closed; it is asked again",
            cx,
        );
        self.sync_activity(cx);
        cx.notify();
    }

    /// Sends the next queued prompt once the session is free.
    fn drain_queue(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy()
            && let Some(next) = self.queue.pop_front()
        {
            self.submit_with(next.prompt, next.role, next.images, cx);
        }
        self.sync_activity(cx);
    }

    /// Shows the current work and the queue in the activity line, and tells
    /// the input whether `Up` takes a queued prompt back.
    fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let work = if self.running.is_some() {
            Some(WorkKind::Turn)
        } else {
            self.commands.last().cloned().map(WorkKind::Command)
        };
        let queue: Vec<String> = self.queue.iter().map(Queued::label).collect();
        let queued = !queue.is_empty();
        self.activity.update(cx, |activity, cx| {
            activity.set_work(work, cx);
            activity.set_queue(queue, cx);
        });
        self.input.update(cx, |input, _| input.set_queued(queued));
        // The window shows busy chats in the sidebar and drops a parked
        // session once it is idle.
        let busy = self.is_busy();
        let waiting = busy && (self.approval.read(cx).is_open() || self.waiting_form(cx).is_some());
        if self.last_activity != Some((busy, waiting)) {
            self.last_activity = Some((busy, waiting));
            cx.emit(SessionEvent::Activity { busy, waiting });
        }
    }

    /// `Up` in an empty prompt: the last queued prompt goes back to the
    /// input to be edited (or emptied to drop it).
    fn edit_queued(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(queued) = self.queue.pop_back() else {
            return;
        };
        self.input.update(cx, |input, cx| {
            input.set_text(&queued.prompt, window, cx);
            input.restore_images(queued.images, cx);
        });
        self.sync_activity(cx);
    }

    fn turn_ended(&mut self, end: TurnEnd, cx: &mut Context<Self>) {
        self.running = None;
        self.turn = None;
        if std::mem::take(&mut self.pending_clear) {
            self.reset(cx);
        }
        match end {
            TurnEnd::Finished(StopReason::Interrupted) => {
                self.notice(NoticeLevel::Info, "turn interrupted", cx);
            }
            TurnEnd::Finished(_) | TurnEnd::Failed(_) => {}
            TurnEnd::Unavailable(message) => {
                self.queue.clear();
                self.dispatch(
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Error {
                            message,
                            hint: Some(
                                "run /login to add your API key, or fix the configuration and \
                                 reopen crest"
                                    .into(),
                            ),
                        }),
                    },
                    cx,
                );
            }
        }
        self.refresh_git(cx);
        self.drain_queue(cx);
        cx.notify();
    }

    /// `Ctrl+C`: cancels the running turn (or asks the controller to stop
    /// its commands) and drops the queue. Returns whether something was
    /// running.
    pub fn interrupt(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(cancel) = &self.running else {
            let Some(command) = self.commands.last().cloned() else {
                return false;
            };
            let stopped = self.controller.interrupt_commands();
            let dropped = std::mem::take(&mut self.queue).len();
            self.sync_activity(cx);
            let mut message = if stopped {
                format!("interrupting /{command}")
            } else {
                format!("/{command} cannot be interrupted; it stops at its next question")
            };
            if dropped > 0 {
                message.push_str(&format!("; {dropped} queued messages discarded"));
            }
            self.notice(NoticeLevel::Info, message, cx);
            return true;
        };
        if !cancel.is_cancelled() {
            cancel.cancel();
            let dropped = std::mem::take(&mut self.queue).len();
            self.sync_activity(cx);
            if dropped > 0 {
                self.notice(
                    NoticeLevel::Info,
                    format!("interrupting; {dropped} queued messages discarded"),
                    cx,
                );
            }
        }
        true
    }

    /// Sets the permission mode and tells the status bar.
    pub fn set_permission_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.controller.set_permission_mode(mode);
        self.dispatch(DomainEvent::ModeChanged { mode }, cx);
    }

    /// `<leader>p`.
    pub fn cycle_permission_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self.controller.permission_mode().next();
        self.set_permission_mode(mode, cx);
    }

    /// Sets (or clears) the override of the next turn.
    pub fn set_model_override(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        let message = match &model {
            Some(model) => format!("next turn with `{model}`"),
            None => "next turn with the default model".to_owned(),
        };
        self.model_override = model;
        self.notice(NoticeLevel::Info, message, cx);
    }

    /// Runs a `/` command (without the slash).
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.command_at(command, 0, window, cx);
    }

    fn command_at(
        &mut self,
        command: &str,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `/!cmd args` and `/! cmd args` are the shell command `!`.
        let (name, arg) = match command.strip_prefix('!') {
            Some(line) => ("!", line),
            None => command.split_once(' ').unwrap_or((command, "")),
        };
        let arg = arg.trim();
        match name {
            "q" | "quit" | "qa" => cx.quit(),
            "model" => match arg
                .split_once(' ')
                .map_or((arg, ""), |(a, b)| (a, b.trim()))
            {
                ("", _) => self.open_model_popup(window, cx),
                ("clear", "") => self.set_model_override(None, cx),
                ("set", "") => self.notice(NoticeLevel::Warning, "usage: /model set <id>", cx),
                ("set", id) => {
                    let choice = ModelChoice {
                        alias: id.to_owned(),
                        model: ModelId::new(id),
                    };
                    self.select_model(choice, window, cx);
                }
                _ => self.set_model_override(Some(arg.to_owned()), cx),
            },
            "theme" => match arg
                .split_once(' ')
                .map_or((arg, ""), |(a, b)| (a, b.trim()))
            {
                ("", _) => self.open_theme_popup(window, cx),
                ("set", "") => self.notice(NoticeLevel::Warning, "usage: /theme set <name>", cx),
                ("set", name) => self.save_theme(name.to_owned(), window, cx),
                (name, "") => self.save_theme(name.to_owned(), window, cx),
                _ => self.notice(NoticeLevel::Warning, "usage: /theme [set] <name>", cx),
            },
            "mode" => match arg.parse::<PermissionMode>() {
                Ok(mode) => self.set_permission_mode(mode, cx),
                Err(err) => self.notice(NoticeLevel::Warning, format!("/mode: {err}"), cx),
            },
            "clear" => self.clear(window, cx),
            "project" => self.switch_project(window, cx),
            "login" => self.open_login(arg, window, cx),
            "logout" => self.logout(arg, window, cx),
            "help" | "h" => self.open_help(arg, window, cx),
            "modal" if arg == "close" => {
                let closed = self
                    .modal
                    .update(cx, |modal, cx| modal.close_top(window, cx));
                if !closed {
                    self.notice(NoticeLevel::Info, "no modal is open", cx);
                }
            }
            "modal" => self.notice(
                NoticeLevel::Warning,
                format!("/modal {arg}: use /modal close"),
                cx,
            ),
            _ => {
                if crate::picker::PickerMode::for_action(&Action::Builtin(name.to_owned()))
                    .is_some()
                {
                    // `/picker.files` from the palette.
                    self.action(&Action::Builtin(name.to_owned()), window, cx);
                } else if let Some(reply) = self.controller.run_command(name, arg) {
                    self.commands.push(name.to_owned());
                    self.sync_activity(cx);
                    let name = name.to_owned();
                    self.await_reply_then(reply, depth, Some(name), window, cx);
                } else if name.contains('.') {
                    self.action(&Action::Builtin(name.to_owned()), window, cx);
                } else {
                    self.notice(NoticeLevel::Warning, unknown_command_hint(command), cx);
                }
            }
        }
    }

    /// Runs `reply` once the controller answers.
    fn await_reply(
        &mut self,
        reply: BoxFuture<'static, Reply>,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.await_reply_then(reply, depth, None, window, cx);
    }

    /// [`Self::await_reply`] for the `/` command `command`, which keeps the
    /// session busy until it replies.
    fn await_reply_then(
        &mut self,
        reply: BoxFuture<'static, Reply>,
        depth: usize,
        command: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let reply = reply.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(command) = &command
                    && let Some(ix) = this.commands.iter().rposition(|c| c == command)
                {
                    this.commands.remove(ix);
                }
                this.reply(reply, depth, window, cx);
                if command.is_some() {
                    this.refresh_git(cx);
                    this.drain_queue(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows or runs what a command or callback answered.
    pub fn reply(
        &mut self,
        reply: Reply,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match reply {
            Reply::Done => {}
            Reply::Notice(level, message) => self.notice(level, message, cx),
            Reply::Block(block) => self.dispatch(DomainEvent::BlockAdded { block: *block }, cx),
            Reply::Actions(actions) => {
                for action in actions {
                    match action {
                        PluginAction::Send { text, role } => self.submit_as(text, role, cx),
                        PluginAction::Cmd { command } if depth < MAX_COMMAND_DEPTH => {
                            self.command_at(&command, depth + 1, window, cx);
                        }
                        PluginAction::Cmd { command } => self.notice(
                            NoticeLevel::Warning,
                            format!("app.cmd(\"{command}\") ignored: too many nested commands"),
                            cx,
                        ),
                    }
                }
            }
        }
    }

    /// `/clear`: a new session and an empty timeline.
    /// `/project`: back to the project picker, once nothing runs. The
    /// window ([`crate::workbench::Workbench`]) handles the action.
    fn switch_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.notice(
                NoticeLevel::Warning,
                "a turn is running; Ctrl+C to interrupt it before /project",
                cx,
            );
            return;
        }
        if let Some(command) = self.commands.last() {
            let message = format!("/{command} is running; /project once it stops");
            self.notice(NoticeLevel::Warning, message, cx);
            return;
        }
        window.dispatch_action(Box::new(crate::workbench::SwitchProject), cx);
    }

    /// `/clear`: a new, empty session. During a turn (a form waiting, a
    /// long answer) the turn is interrupted first and the reset follows
    /// once it has ended and saved; the prompt keeps the focus meanwhile.
    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.commands.last() {
            let message = format!("/{command} is running; /clear once it stops");
            self.notice(NoticeLevel::Warning, message, cx);
            return;
        }
        if self.running.is_some() {
            self.pending_clear = true;
            self.interrupt(cx);
            self.notice(
                NoticeLevel::Info,
                "interrupting the turn; clearing when it stops",
                cx,
            );
            return;
        }
        self.reset(cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Replaces the timeline, the prompt history and the controller's
    /// session with empty ones.
    fn reset(&mut self, cx: &mut Context<Self>) {
        let reset = self.controller.new_session();
        let timeline = cx.new(TimelineView::new);
        self.timeline = timeline.clone();
        self.root
            .update(cx, |root, cx| root.set_timeline(timeline, cx))
            .ok();
        self.input
            .update(cx, |input, _| input.set_history(Vec::new()));
        cx.spawn(async move |_, _| reset.await).detach();
        cx.notify();
    }

    /// Shows the picture of the image block `block` in a modal.
    /// Recompute on a statistics card: the controller re-runs the
    /// envelope and sends the new version as a `BlockUpdated`; a failure
    /// is shown on the card and as a notice.
    fn recompute(&mut self, id: BlockId, cx: &mut Context<Self>) {
        let block = {
            let state = self.timeline.read(cx).state();
            state
                .position(id)
                .and_then(|ix| state.get(ix))
                .map(|entry| entry.block.clone())
        };
        let Some(block) = block else {
            return;
        };
        let task = self.controller.recompute(block);
        cx.spawn(async move |this, cx| {
            if let Err(message) = task.await {
                this.update(cx, |this, cx| {
                    crate::blocks::computed::recompute_failed(id, message.clone(), cx);
                    this.notice(
                        NoticeLevel::Warning,
                        format!("Recompute failed: {message}"),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    fn inspect_image(
        &mut self,
        block: BlockId,
        media_type: &str,
        data: &ImageData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let size = Percent::new(INSPECT_PERCENT).unwrap_or_default();
        let spec = ModalSpec {
            id: ModalId::new(format!("image:{block}")),
            plugin: "crest".into(),
            title: "Image".into(),
            width: size,
            height: size,
            position: ModalPosition::CENTER,
            content: CustomNode::Image {
                source: data.clone(),
                media_type: Some(media_type.to_owned()),
                alt: None,
                fit: ImageFit::Contain,
            },
        };
        self.modal.update(cx, |modal, cx| {
            modal.apply(DomainEvent::ModalOpened { spec }, window, cx);
        });
    }

    /// Adds pasted images to the chat only: the controller saves them as
    /// image blocks and the model is never called.
    pub fn post_images(
        &mut self,
        caption: String,
        images: Vec<ImageAttachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reply = self.controller.post_images(caption, images);
        self.await_reply(reply, 0, window, cx);
    }

    fn insert_reference(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.insert_text(text, window, cx));
    }

    fn on_input(
        &mut self,
        _input: &Entity<InputView>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A form waiting for an answer takes the prompt as its answer.
            InputEvent::Submit(prompt) => match self.waiting_form(cx) {
                Some(form) => {
                    form.update(cx, |form, cx| form.reply(prompt.clone(), window, cx));
                }
                None => self.submit(prompt.clone(), cx),
            },
            InputEvent::SubmitImages {
                text,
                images,
                mode: ImageMode::Model,
            } => {
                if self.waiting_form(cx).is_some() {
                    let message = "a form is waiting for an answer; the images go to the next turn";
                    self.notice(NoticeLevel::Info, message.to_owned(), cx);
                }
                self.submit_with(text.clone(), None, images.clone(), cx);
            }
            InputEvent::SubmitImages {
                text,
                images,
                mode: ImageMode::ChatOnly,
            } => self.post_images(text.clone(), images.clone(), window, cx),
            InputEvent::Notice(message) => self.notice(NoticeLevel::Warning, message.clone(), cx),
            InputEvent::Command(command) => self.command(command, window, cx),
            InputEvent::Action(action) => self.action(action, window, cx),
            InputEvent::Recorded(line) => self.controller.record_prompt(line.clone()),
            InputEvent::ModelsWanted => self.load_catalog(window, cx),
            InputEvent::ThemesWanted => self.load_themes(window, cx),
            InputEvent::ThemePreview(name) => {
                if let Some(choice) = self.theme_choices.iter().find(|c| &c.name == name) {
                    crate::app::set_theme(&choice.theme, cx);
                }
            }
            InputEvent::ThemePreviewEnd => crate::app::set_theme(&self.theme, cx),
            InputEvent::Cleared => {
                self.notice(NoticeLevel::Info, "prompt cleared · ↑ restores", cx)
            }
        }
    }

    fn action(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .tree
            .update(cx, |tree, cx| tree.handle_action(action, window, cx))
        {
            // A hidden tree cannot keep the focus.
            if !self.tree.read(cx).is_visible() {
                self.focus_input(window, cx);
            }
            return;
        }
        if self
            .timeline
            .update(cx, |view, cx| view.handle_action(action, cx))
            || self
                .picker
                .update(cx, |picker, cx| picker.handle_action(action, window, cx))
        {
            return;
        }
        let name = match action {
            Action::Builtin(name) => name.as_str(),
            Action::Callback(id) => {
                let reply = self.controller.run_callback(*id, serde_json::Value::Null);
                self.await_reply(reply, 0, window, cx);
                return;
            }
        };
        match name {
            INTERRUPT_ACTION => {
                if !self.interrupt(cx) {
                    self.notice(NoticeLevel::Info, "nothing to interrupt", cx);
                }
            }
            // Copies from anywhere and never interrupts: the prompt's
            // selection first, else the chat's.
            "selection.copy" => {
                let copied = self.input.read(cx).copy_selection(cx)
                    || self
                        .timeline
                        .update(cx, |timeline, cx| timeline.copy_selection(window, cx));
                if !copied {
                    self.notice(NoticeLevel::Info, "nothing selected to copy", cx);
                }
            }
            PERMISSION_ACTION => self.cycle_permission_mode(cx),
            QUEUE_EDIT_ACTION => self.edit_queued(window, cx),
            FOCUS_INPUT_ACTION => self.focus_input(window, cx),
            FOCUS_NEXT_ACTION => self.focus_next(window, cx),
            MODEL_ACTION => self.open_model_popup(window, cx),
            HELP_ACTION => self.open_help("", window, cx),
            _ => self.notice(
                NoticeLevel::Info,
                format!("the action {action:?} does nothing yet"),
                cx,
            ),
        }
    }

    /// Opens the help modal: `arg` names a section or is a search (`/help
    /// tools`, `/help jira`). The controller's part arrives first.
    pub fn open_help(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (section, query) = parse_help_arg(arg);
        let snapshot = self.controller.help();
        cx.spawn_in(window, async move |this, cx| {
            let snapshot = snapshot.await;
            this.update_in(cx, |this, window, cx| {
                this.show_help(snapshot, section, &query, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_help(
        &mut self,
        snapshot: HelpSnapshot,
        section: Option<Section>,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keymap = self.input.read(cx).keymap().clone();
        let models = self.controller.models();
        let snapshot = snapshot.complete(&LocalHelp {
            keymap: &keymap,
            models: &models,
            permission_mode: self.controller.permission_mode(),
            turn_override: self.model_override.as_deref(),
        });
        let view = cx.new(|cx| HelpView::new(snapshot, section, query, window, cx));
        let focus = view.focus_handle(cx);
        let size = Percent::new(90.0).unwrap_or(Percent::FULL);
        let spec = ModalSpec {
            id: help_modal_id(),
            plugin: "crest".into(),
            title: "Help".into(),
            width: size,
            height: size,
            position: ModalPosition::CENTER,
            content: CustomNode::Text {
                text: String::new(),
            },
        };
        self.modal.update(cx, |modal, cx| {
            modal.open_view(spec, view.into(), window, cx)
        });
        // Keys reach the help first; Esc still bubbles up to the host.
        window.focus(&focus, cx);
    }

    /// Puts `/model ` in the prompt, whose popup lists the models (the
    /// input asks for them with [`InputEvent::ModelsWanted`]).
    pub fn open_model_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("/model ", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Puts `/theme ` in the prompt, whose popup lists the themes.
    pub fn open_theme_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("/theme ", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Resolves the themes and hands them to the input's popup.
    fn load_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let listing = self.controller.themes();
        cx.spawn_in(window, async move |this, cx| {
            let result = listing.await;
            this.update(cx, |this, cx| {
                if let Ok(themes) = &result {
                    this.theme_choices.clone_from(themes);
                }
                this.input
                    .update(cx, |input, cx| input.set_themes(result, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Saves `name` as the theme and shows it; a theme that cannot be
    /// saved or resolved leaves the one in use.
    fn save_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let save = self.controller.save_theme(name);
        cx.spawn_in(window, async move |this, cx| {
            let result = save.await;
            this.update(cx, |this, cx| match result {
                Ok(saved) => {
                    this.theme = saved.theme;
                    crate::app::set_theme(&this.theme, cx);
                    let name = this.theme.name.clone();
                    this.input
                        .update(cx, |input, _| input.set_current_theme(&name));
                    this.notice(NoticeLevel::Info, saved.notice, cx);
                }
                Err(error) => {
                    crate::app::set_theme(&this.theme, cx);
                    this.notice(NoticeLevel::Warning, format!("/theme: {error}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Lists the providers' models once per session and hands them to the
    /// input's popup, after the aliases whose model a provider serves.
    fn load_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(models) = self.catalog_models.clone() {
            self.input
                .update(cx, |input, cx| input.set_models(Ok(models), cx));
            return;
        }
        let listing = self.controller.catalog_models();
        let aliases = self.controller.models();
        cx.spawn_in(window, async move |this, cx| {
            let result = listing.await.map(|catalog| with_aliases(aliases, catalog));
            this.update(cx, |this, cx| {
                if let Ok(models) = &result {
                    this.catalog_models = Some(models.clone());
                }
                this.input
                    .update(cx, |input, cx| input.set_models(result, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Saves `choice` as the selected model, then reopens the session so
    /// every role and flow uses it; if it cannot be saved, it is still the
    /// model of the next turn.
    fn select_model(&mut self, choice: ModelChoice, window: &mut Window, cx: &mut Context<Self>) {
        if !self.idle_for("/model", cx) {
            self.set_model_override(Some(choice.alias), cx);
            return;
        }
        let select = self.controller.select_model(choice.model.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = select.await;
            this.update(cx, |this, cx| match result {
                Ok(message) => cx.emit(SessionEvent::Reopen {
                    notice: Some((NoticeLevel::Info, message)),
                }),
                Err(error) => {
                    this.set_model_override(Some(choice.alias), cx);
                    this.notice(NoticeLevel::Warning, error, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// The aliases whose model is in `catalog` (an alias of a provider without
/// a key is no choice), then the catalog.
fn with_aliases(aliases: Vec<ModelChoice>, catalog: Vec<ModelChoice>) -> Vec<ModelChoice> {
    let served = |model: &ModelId| catalog.iter().any(|c| &c.model == model);
    let mut models: Vec<ModelChoice> = aliases
        .into_iter()
        .filter(|a| a.alias != a.model.as_str() && served(&a.model))
        .collect();
    models.extend(catalog);
    models
}

impl EventEmitter<SessionEvent> for Session {}

impl Render for Session {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().id("session").track_focus(&self.focus_handle)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use application::ports::events::EventSink;
    use application::ports::form::FormPort;
    use application::testing::fs::{InMemoryFs, ScriptedShell, test_root};
    use application::testing::llm::{ScriptedLlm, ScriptedResponse};
    use application::testing::system::{RecordingSink, RecordingTelemetry};
    use application::tools::files::{FileToolPorts, register_file_tools};
    use application::tools::registry::ToolRegistry;
    use application::usecases::run_turn::{AgentDeps, AgentService, TurnOptions, UserInput};
    use domain::block::ToolStatus;
    use domain::conversation::ContentPart;
    use domain::conversation::Session as Conversation;
    use domain::model::{ModelPolicy, ModelRole};
    use domain::permission::PermissionPolicy;

    use futures::channel::oneshot;
    use gpui::{ParentElement as _, Styled as _, TestAppContext, VisualTestContext};
    use serde_json::json;

    use super::*;
    use crate::approval::UiApproval;
    use crate::input::ModelPopup;

    /// A controller over the real `AgentService`, running each turn on its
    /// own thread as the binary runs it on tokio.
    struct AgentController {
        service: Arc<AgentService>,
        conversation: Arc<futures::lock::Mutex<Conversation>>,
        events: Arc<dyn EventSink>,
        /// Keys given to `login`, in order.
        logins: Arc<Mutex<Vec<String>>>,
    }

    impl SessionController for AgentController {
        fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
            let (tx, rx) = oneshot::channel();
            let service = Arc::clone(&self.service);
            let conversation = Arc::clone(&self.conversation);
            let events = Arc::clone(&self.events);
            std::thread::spawn(move || {
                let end = futures::executor::block_on(async move {
                    events.emit(DomainEvent::BlockAdded {
                        block: prompt_block(&request.prompt),
                    });
                    let role = request.role.unwrap_or(ModelRole::Main);
                    let mut opts = TurnOptions::new(role).with_cancel(request.cancel);
                    if let Some(model) = request.model_override {
                        opts = opts.with_override(model);
                    }
                    let mut conversation = conversation.lock().await;
                    let input = UserInput::new(request.prompt, request.images);
                    match service.run_turn(&mut conversation, input, opts).await {
                        Ok(outcome) => TurnEnd::Finished(outcome.stop_reason),
                        Err(err) => TurnEnd::Failed(err.to_string()),
                    }
                });
                tx.send(end).ok();
            });
            rx.map(|end| end.unwrap_or(TurnEnd::Failed("turn lost".into())))
                .boxed()
        }

        fn permission_mode(&self) -> PermissionMode {
            self.service
                .permissions()
                .lock()
                .map(|policy| policy.mode)
                .unwrap_or_default()
        }

        fn set_permission_mode(&self, mode: PermissionMode) {
            if let Ok(mut policy) = self.service.permissions().lock() {
                policy.mode = mode;
            }
        }

        fn models(&self) -> Vec<ModelChoice> {
            ModelPolicy::default()
                .aliases()
                .into_iter()
                .map(|(alias, model)| ModelChoice { alias, model })
                .collect()
        }

        /// Two OpenCode models, besides the aliases.
        fn catalog_models(&self) -> BoxFuture<'static, Result<Vec<ModelChoice>, String>> {
            let models = ["opencode-go:glm-5.3", "opencode-go:kimi-k3"]
                .map(|id| ModelChoice {
                    alias: id.to_owned(),
                    model: ModelId::new(id),
                })
                .to_vec();
            futures::future::ready(Ok(models)).boxed()
        }

        /// Records the pick, as saving it would.
        fn select_model(&self, model: ModelId) -> BoxFuture<'static, Result<String, String>> {
            self.logins.lock().unwrap().push(format!("model {model}"));
            futures::future::ready(Ok(format!("model `{model}` selected"))).boxed()
        }

        fn new_session(&self) -> BoxFuture<'static, ()> {
            let conversation = Arc::clone(&self.conversation);
            async move {
                *conversation.lock().await = Conversation::new("/ws", 0);
            }
            .boxed()
        }

        fn resume_pending(&self, cancel: CancelToken) -> BoxFuture<'static, TurnEnd> {
            let (tx, rx) = oneshot::channel();
            let service = Arc::clone(&self.service);
            let conversation = Arc::clone(&self.conversation);
            std::thread::spawn(move || {
                let end = futures::executor::block_on(async move {
                    let opts = TurnOptions::new(ModelRole::Main).with_cancel(cancel);
                    let mut conversation = conversation.lock().await;
                    match service.resume_pending_tools(&mut conversation, opts).await {
                        Ok(Some(outcome)) => TurnEnd::Finished(outcome.stop_reason),
                        Ok(None) => TurnEnd::Finished(StopReason::EndTurn),
                        Err(err) => TurnEnd::Failed(err.to_string()),
                    }
                });
                tx.send(end).ok();
            });
            rx.map(|end| end.unwrap_or(TurnEnd::Failed("turn lost".into())))
                .boxed()
        }

        /// Adds the images as blocks, as the binary does.
        fn post_images(
            &self,
            caption: String,
            images: Vec<ImageAttachment>,
        ) -> BoxFuture<'static, Reply> {
            for block in application::usecases::user_blocks::user_blocks(&caption, &images) {
                self.events.emit(DomainEvent::BlockAdded { block });
            }
            futures::future::ready(Reply::Done).boxed()
        }

        /// Callback 7 is `function() app.cmd("clear") end`.
        fn run_callback(&self, id: u64, _args: serde_json::Value) -> BoxFuture<'static, Reply> {
            let reply = match id {
                7 => Reply::Actions(vec![PluginAction::Cmd {
                    command: "clear".into(),
                }]),
                _ => Reply::Notice(NoticeLevel::Error, format!("callback {id}?")),
            };
            futures::future::ready(reply).boxed()
        }

        /// Only `sk-ant-good` is accepted; `sk-ant-slow` never answers.
        fn login(
            &self,
            _account: Account,
            key: String,
        ) -> BoxFuture<'static, Result<String, String>> {
            self.logins.lock().unwrap().push(key.clone());
            if key == "sk-ant-slow" {
                return futures::future::pending().boxed();
            }
            let result = if key == "sk-ant-good" {
                Ok("stored".to_owned())
            } else {
                Err("Anthropic rejected this key".to_owned())
            };
            futures::future::ready(result).boxed()
        }

        fn logout(&self, _account: Option<Account>) -> BoxFuture<'static, Result<String, String>> {
            futures::future::ready(Ok("deleted".to_owned())).boxed()
        }

        /// `/review` sends a prompt with the plan role; `/loop` calls itself;
        /// `/slow` answers after 2 s, like a flow running to its gate.
        fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
            let reply = match name {
                "slow" => {
                    let (tx, rx) = oneshot::channel();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(2));
                        tx.send(Reply::Notice(NoticeLevel::Info, "slow done".into()))
                            .ok();
                    });
                    return Some(rx.map(|reply| reply.unwrap_or(Reply::Done)).boxed());
                }
                "review" => Reply::Actions(vec![PluginAction::Send {
                    text: format!("revisa {args}"),
                    role: Some(ModelRole::Plan),
                }]),
                "loop" => Reply::Actions(vec![PluginAction::Cmd {
                    command: "loop".into(),
                }]),
                "!" => Reply::Notice(NoticeLevel::Info, format!("ran [{args}]")),
                _ => return None,
            };
            Some(futures::future::ready(reply).boxed())
        }
    }

    struct Harness {
        session: Entity<Session>,
        llm: Arc<ScriptedLlm>,
        sink: Arc<RecordingSink>,
        logins: Arc<Mutex<Vec<String>>>,
        reopens: Rc<RefCell<Vec<SessionEvent>>>,
    }

    /// Hosts the root view as the window's root.
    struct Host(Entity<RootView>);

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    fn harness(
        script: Vec<ScriptedResponse>,
        cx: &mut TestAppContext,
    ) -> (Harness, &mut VisualTestContext) {
        harness_with(script, Vec::new(), cx)
    }

    fn harness_with(
        script: Vec<ScriptedResponse>,
        history: Vec<Block>,
        cx: &mut TestAppContext,
    ) -> (Harness, &mut VisualTestContext) {
        harness_full(script, history, |_, _| {}, cx)
    }

    /// A harness whose agent also has the tools `register` adds; the
    /// `FormPort` given answers in the session's form blocks.
    fn harness_full(
        script: Vec<ScriptedResponse>,
        history: Vec<Block>,
        register: impl FnOnce(&mut ToolRegistry, Arc<dyn FormPort>),
        cx: &mut TestAppContext,
    ) -> (Harness, &mut VisualTestContext) {
        harness_resuming(script, Conversation::new("/ws", 0), history, register, cx)
    }

    /// [`harness_full`] over a saved `conversation`; a turn it leaves
    /// pending is resumed as the session opens.
    fn harness_resuming(
        script: Vec<ScriptedResponse>,
        conversation: Conversation,
        history: Vec<Block>,
        register: impl FnOnce(&mut ToolRegistry, Arc<dyn FormPort>),
        cx: &mut TestAppContext,
    ) -> (Harness, &mut VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(gpui_component::init);
        let fs =
            Arc::new(InMemoryFs::new(test_root()).with_file("src/lib.rs", "pub fn add() {}\n"));
        let (sink, events) = RecordingSink::with_channel();
        let sink = Arc::new(sink);
        let (approval, approvals) = UiApproval::channel();
        let (form, forms) = crate::form::UiForm::channel();
        let llm = Arc::new(ScriptedLlm::new(script));
        let mut tools = ToolRegistry::new();
        register(&mut tools, Arc::new(form));
        register_file_tools(
            &mut tools,
            FileToolPorts {
                file_search: fs.clone(),
                content_search: fs.clone(),
                fs: fs.clone(),
                shell: Arc::new(ScriptedShell::new()),
                rewriter: None,
            },
        )
        .unwrap();
        let service = AgentService::new(
            AgentDeps {
                llm: llm.clone(),
                events: sink.clone(),
                approval: Arc::new(approval),
                telemetry: Arc::new(RecordingTelemetry::new()),
            },
            tools,
            ModelPolicy::default(),
            Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask))),
        );
        let logins = Arc::new(Mutex::new(Vec::new()));
        let resume_pending = !conversation.pending_tool_uses().is_empty();
        let controller = Arc::new(AgentController {
            service: Arc::new(service),
            conversation: Arc::new(futures::lock::Mutex::new(conversation)),
            events: sink.clone(),
            logins: Arc::clone(&logins),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs.clone());
        parts.fs = Some(fs);
        parts.forms = Some(forms);
        parts.resume_pending = resume_pending;
        parts.keymap.set("<leader>x", Action::Callback(7)).unwrap();
        parts.history = history;
        let slot = Rc::new(RefCell::new(None));
        let out = slot.clone();
        let (_host, cx) = cx.add_window_view(move |window, cx| {
            let (root, session) = open(parts, window, cx);
            *out.borrow_mut() = Some(session);
            Host(root)
        });
        cx.run_until_parked();
        let session = slot.borrow_mut().take().unwrap();
        let reopens = Rc::new(RefCell::new(Vec::new()));
        let seen = reopens.clone();
        cx.update(|_, cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
            .detach();
        });
        (
            Harness {
                session,
                llm,
                sink,
                logins,
                reopens,
            },
            cx,
        )
    }

    /// Runs the UI until `done` holds; the agent runs on other threads.
    fn wait_until(
        h: &Harness,
        cx: &mut VisualTestContext,
        what: &str,
        done: impl Fn(&Session, &App) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if h.session.read_with(cx, |s, cx| done(s, cx)) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn blocks(h: &Harness, cx: &mut VisualTestContext) -> Vec<BlockKind> {
        h.session.read_with(cx, |s, cx| {
            s.timeline()
                .read(cx)
                .state()
                .entries()
                .iter()
                .map(|e| e.block.kind.clone())
                .collect()
        })
    }

    fn text_of(kind: &BlockKind) -> Option<&str> {
        match kind {
            BlockKind::Text { markdown } => Some(markdown),
            _ => None,
        }
    }

    fn send(text: &str, cx: &mut VisualTestContext) {
        cx.simulate_input(text);
        cx.simulate_keystrokes("enter");
    }

    fn finished(h: &Harness) -> Vec<StopReason> {
        h.sink
            .events()
            .into_iter()
            .filter_map(|event| match event {
                DomainEvent::TurnFinished { stop_reason, .. } => Some(stop_reason),
                _ => None,
            })
            .collect()
    }

    fn login_view(h: &Harness, cx: &mut VisualTestContext) -> Entity<LoginView> {
        h.session.read_with(cx, |s, _| s.login_view().clone())
    }

    fn submit_key(h: &Harness, key: &str, cx: &mut VisualTestContext) {
        let login = login_view(h, cx);
        cx.update(|window, cx| {
            login.update(cx, |login, cx| {
                login.set_key(key, window, cx);
                login.submit(cx);
            });
        });
    }

    #[gpui::test]
    fn login_checks_the_key_out_of_the_prompt_and_reopens(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/login", cx);
        cx.run_until_parked();
        let login = login_view(&h, cx);
        assert!(login.read_with(cx, |l, _| l.is_open()));

        submit_key(&h, "sk-ant-bad", cx);
        wait_until(&h, cx, "the rejection", |s, cx| {
            s.login_view().read(cx).error().is_some()
        });
        assert!(
            login.read_with(cx, |l, _| l.is_open()),
            "an error keeps it open"
        );
        assert!(h.reopens.borrow().is_empty());

        submit_key(&h, "sk-ant-good", cx);
        wait_until(&h, cx, "the dialog to close", |s, cx| {
            !s.login_view().read(cx).is_open()
        });
        assert_eq!(*h.logins.lock().unwrap(), ["sk-ant-bad", "sk-ant-good"]);
        assert_eq!(
            *h.reopens.borrow(),
            [SessionEvent::Reopen {
                notice: Some((NoticeLevel::Info, "stored".into()))
            }]
        );
        let history = h
            .session
            .read_with(cx, |s, cx| s.input().read(cx).history().to_vec());
        assert!(
            history.iter().all(|line| !line.contains("sk-ant")),
            "{history:?}"
        );
        assert!(h.llm.requests().is_empty(), "no turn ran");
    }

    #[gpui::test]
    fn login_names_the_account_and_tab_switches_it(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/login opencode", cx);
        cx.run_until_parked();
        let login = login_view(&h, cx);
        assert_eq!(
            login.read_with(cx, |l, _| (l.is_open(), l.account())),
            (true, Account::OpenCode)
        );
        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        assert_eq!(login.read_with(cx, |l, _| l.account()), Account::Anthropic);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        send("/login openai", cx);
        cx.run_until_parked();
        assert!(!login.read_with(cx, |l, _| l.is_open()), "unknown account");
    }

    #[gpui::test]
    fn a_login_check_can_be_cancelled_and_times_out(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/login", cx);
        cx.run_until_parked();
        let login = login_view(&h, cx);
        login.update_in(cx, |l, window, cx| {
            l.set_key("sk-ant-slow", window, cx);
            l.submit(cx);
        });
        cx.run_until_parked();
        assert!(login.read_with(cx, |l, _| l.is_busy()));
        cx.simulate_keystrokes("escape");
        assert!(
            !login.read_with(cx, |l, _| l.is_open()),
            "Esc cancels the check"
        );
        h.session.read_with(cx, |s, _| {
            assert!(s.login_check.is_none(), "the check is dropped")
        });
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            assert!(input.contains_focused(window, cx), "back in the prompt");
        });

        // The Cancel button works as well, and an unanswered check times out.
        send("/login", cx);
        cx.run_until_parked();
        login.update_in(cx, |l, window, cx| {
            l.set_key("sk-ant-slow", window, cx);
            l.submit(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(LOGIN_TIMEOUT);
        cx.run_until_parked();
        login.read_with(cx, |l, _| {
            assert!(l.is_open() && !l.is_busy());
            assert!(l.error().is_some_and(|e| e.contains("did not answer")));
        });
        let cancel = cx.debug_bounds("login-cancel").expect("Cancel drawn");
        cx.simulate_click(cancel.center(), gpui::Modifiers::none());
        assert!(!login.read_with(cx, |l, _| l.is_open()), "Cancel closes it");
    }

    #[gpui::test]
    fn login_tabs_switch_the_account_on_click(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/login", cx);
        cx.run_until_parked();
        let login = login_view(&h, cx);
        assert_eq!(login.read_with(cx, |l, _| l.account()), Account::Anthropic);

        let tab = cx
            .debug_bounds("login-tab-opencode")
            .expect("OpenCode tab drawn");
        cx.simulate_click(tab.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(login.read_with(cx, |l, _| l.account()), Account::OpenCode);

        cx.simulate_input("oc-typed");
        cx.run_until_parked();
        login.update(cx, |l, cx| l.submit(cx));
        cx.run_until_parked();
        assert_eq!(
            *h.logins.lock().unwrap(),
            ["oc-typed"],
            "the field kept the focus"
        );
    }

    #[gpui::test]
    fn logout_reopens_and_esc_closes_the_dialog(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/logout", cx);
        cx.run_until_parked();
        assert_eq!(
            *h.reopens.borrow(),
            [SessionEvent::Reopen {
                notice: Some((NoticeLevel::Info, "deleted".into()))
            }]
        );
        let login = login_view(&h, cx);
        assert!(!login.read_with(cx, |l, _| l.is_open()));
        cx.update(|window, cx| {
            login.update(cx, |login, cx| login.show(Account::Anthropic, window, cx));
        });
        cx.run_until_parked();
        cx.simulate_input("sk-ant-typed");
        cx.run_until_parked();
        let prompt = h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
        assert!(
            !prompt.contains("sk-ant"),
            "the key reached the prompt: {prompt}"
        );
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!login.read_with(cx, |l, _| l.is_open()), "Esc closes it");
    }

    #[gpui::test]
    fn a_prompt_runs_a_full_turn_with_streaming_and_a_tool(cx: &mut TestAppContext) {
        let script = vec![
            ScriptedResponse::text("Busco la función.").then_tool_use(
                "toolu_1",
                "find_file",
                json!({ "query": "lib" }),
            ),
            ScriptedResponse::text("Está en src/lib.rs."),
        ];
        let (h, cx) = harness(script, cx);
        send("dónde está add", cx);
        wait_until(&h, cx, "the turn to end", |s, _| !s.is_running());

        let kinds = blocks(&h, cx);
        let texts: Vec<_> = kinds.iter().filter_map(text_of).collect();
        assert_eq!(
            texts,
            [
                "> dónde está add\n",
                "Busco la función.",
                "Está en src/lib.rs."
            ]
        );
        assert!(
            kinds.iter().any(|kind| matches!(
                kind,
                BlockKind::ToolCall { name, status: ToolStatus::Succeeded, .. } if name == "find_file"
            )),
            "{kinds:?}"
        );
        let deltas = h
            .sink
            .events()
            .iter()
            .filter(|e| matches!(e, DomainEvent::TextDelta { .. }))
            .count();
        assert!(deltas >= 2, "the text arrived as deltas");
        assert_eq!(finished(&h), [StopReason::EndTurn]);
        assert_eq!(h.llm.remaining(), 0);
        h.session.read_with(cx, |s, cx| {
            assert!(!s.status().read(cx).state().is_busy());
        });
    }

    #[gpui::test]
    fn ctrl_c_interrupts_the_turn_and_the_input_accepts_the_next(cx: &mut TestAppContext) {
        let script = vec![ScriptedResponse::text("hola mundo cruel").hang_after(4)];
        let (h, cx) = harness(script, cx);
        send("saluda", cx);
        wait_until(&h, cx, "the partial answer", |s, cx| {
            s.timeline()
                .read(cx)
                .state()
                .entries()
                .iter()
                .filter_map(|e| text_of(&e.block.kind))
                .any(|text| text.starts_with("hola"))
        });
        h.session
            .read_with(cx, |s, _| assert!(s.is_running(), "the model hangs"));

        cx.simulate_input("borrador");
        cx.simulate_keystrokes("ctrl-c");
        cx.run_until_parked();
        h.session.read_with(cx, |s, cx| {
            assert_eq!(
                s.input().read(cx).text(cx),
                "",
                "the first clears the prompt"
            );
            assert!(s.is_running(), "and leaves the turn alone");
        });

        cx.simulate_keystrokes("ctrl-c");
        wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
        assert_eq!(finished(&h), [StopReason::Interrupted]);

        h.llm.push(ScriptedResponse::text("de nuevo"));
        send("otra vez", cx);
        wait_until(&h, cx, "the second turn", |s, _| !s.is_running());
        assert_eq!(finished(&h), [StopReason::Interrupted, StopReason::EndTurn]);
        let kinds = blocks(&h, cx);
        assert_eq!(
            kinds.iter().filter_map(text_of).next_back(),
            Some("de nuevo")
        );
    }

    #[gpui::test]
    fn a_running_command_keeps_the_session_busy_and_queues_prompts(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("hecho")], cx);
        cx.simulate_input("/slow");
        cx.simulate_keystrokes("enter");
        h.session.read_with(cx, |s, cx| {
            assert!(s.is_busy() && !s.is_running());
            assert!(s.activity().read(cx).state().is_working());
        });
        send("mientras", cx);
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.queued(), 1);
            assert_eq!(s.activity().read(cx).state().queue(), ["mientras"]);
        });
        assert_eq!(h.llm.remaining(), 1, "nothing was sent yet");

        // Up in the empty prompt takes it back; Enter queues it again.
        cx.simulate_keystrokes("up");
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.queued(), 0);
            assert_eq!(s.input().read(cx).text(cx), "mientras");
        });
        cx.simulate_input(" tanto");
        cx.simulate_keystrokes("enter");
        h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 1));

        wait_until(&h, cx, "the command and the queued turn", |s, _| {
            !s.is_busy() && s.queued() == 0
        });
        assert_eq!(h.llm.remaining(), 0, "the queued prompt was sent");
        let kinds = blocks(&h, cx);
        assert!(
            kinds
                .iter()
                .filter_map(text_of)
                .any(|text| text == "> mientras tanto\n"),
            "{kinds:?}"
        );
        h.session.read_with(cx, |s, cx| {
            let activity = s.activity().read(cx).state();
            assert!(!activity.is_working() && activity.queue().is_empty());
        });
    }

    #[gpui::test]
    fn a_prompt_sent_while_a_form_waits_answers_it_in_prose(cx: &mut TestAppContext) {
        use application::tools::form::AskUserTool;
        use domain::form::FormState;

        let script = vec![
            ScriptedResponse::tool_use(
                "toolu_1",
                "ask_user",
                json!({"title": "Which?", "schema": {"properties": {
                    "lang": {"type": "string", "enum": ["rust", "lua"]}}}}),
            ),
            ScriptedResponse::text("vale"),
        ];
        let register = |tools: &mut ToolRegistry, port: Arc<dyn FormPort>| {
            tools.register_typed(AskUserTool::new(port)).unwrap();
        };
        let (h, cx) = harness_full(script, Vec::new(), register, cx);
        send("hola", cx);
        wait_until(&h, cx, "the form", |s, cx| s.waiting_form(cx).is_some());
        h.session.read_with(cx, |s, _| assert!(s.is_running()));

        // Back to the prompt, and the answer is typed there.
        h.session
            .update_in(cx, |s, window, cx| s.focus_input(window, cx));
        send("mejor en python", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 0));
        assert_eq!(
            h.llm.remaining(),
            0,
            "the tool returned and the model answered"
        );
        let replied: Vec<Option<String>> = h
            .sink
            .events()
            .into_iter()
            .filter_map(|event| match event {
                DomainEvent::BlockUpdated { block } => match block.kind {
                    BlockKind::Form(spec) if spec.state == FormState::Replied => Some(spec.reply),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(replied, [Some("mejor en python".to_owned())]);
        let kinds = blocks(&h, cx);
        assert_eq!(
            kinds.iter().filter_map(text_of).next_back(),
            Some("vale"),
            "{kinds:?}"
        );
    }

    #[gpui::test]
    fn a_form_left_waiting_by_a_restart_is_asked_again_on_open(cx: &mut TestAppContext) {
        use application::tools::form::AskUserTool;
        use domain::block::BlockId;
        use domain::conversation::{Message, Role, ToolUseId, Turn};
        use domain::form::{FormSpec, FormState};

        // The saved session: a turn cut while `ask_user` waited, and the
        // blocks the timeline showed then.
        let input = json!({"title": "Which?", "schema": {"properties": {
            "lang": {"type": "string", "enum": ["rust", "lua"]}}}});
        let mut conversation = Conversation::new("/ws", 0);
        let mut turn = Turn::new(Message::user_text("hola"));
        turn.messages.push(Message::new(
            Role::Assistant,
            vec![ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "ask_user".into(),
                input: input.clone(),
            }],
        ));
        conversation.push_turn(turn, 1);
        let spec: FormSpec = serde_json::from_value(json!({
            "title": "Which?",
            "fields": [{"name": "lang", "type": "enum",
                        "options": [{"value": "rust"}, {"value": "lua"}]}]
        }))
        .unwrap();
        let history = vec![
            prompt_block("hola"),
            Block {
                id: BlockId::for_tool_use("toolu_1", "tool_call"),
                kind: BlockKind::ToolCall {
                    tool_use_id: "toolu_1".into(),
                    name: "ask_user".into(),
                    summary: "Which?".into(),
                    input,
                    status: ToolStatus::Running,
                    output: None,
                },
            },
            Block {
                id: BlockId::for_tool_use("toolu_1", "form"),
                kind: BlockKind::Form(spec),
            },
        ];
        let register = |tools: &mut ToolRegistry, port: Arc<dyn FormPort>| {
            tools.register_typed(AskUserTool::new(port)).unwrap();
        };
        let (h, cx) = harness_resuming(
            vec![ScriptedResponse::text("vale")],
            conversation,
            history,
            register,
            cx,
        );

        // Opened: the turn resumes and the form waits again, in its block.
        wait_until(&h, cx, "the form", |s, cx| s.waiting_form(cx).is_some());
        h.session.read_with(cx, |s, _| assert!(s.is_running()));
        let forms = blocks(&h, cx)
            .iter()
            .filter(|kind| matches!(kind, BlockKind::Form(_)))
            .count();
        assert_eq!(forms, 1, "the same block, not a second form");

        // Answered in the chat: the model goes on.
        h.session
            .update_in(cx, |s, window, cx| s.focus_input(window, cx));
        send("rust", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        let kinds = blocks(&h, cx);
        assert!(
            kinds.iter().any(|kind| matches!(
                kind,
                BlockKind::Form(spec) if spec.state == FormState::Replied
            )),
            "{kinds:?}"
        );
        assert_eq!(
            kinds.iter().filter_map(text_of).next_back(),
            Some("vale"),
            "{kinds:?}"
        );
        assert_eq!(finished(&h), [StopReason::EndTurn]);
    }

    #[gpui::test]
    fn prompts_sent_during_a_turn_are_queued(cx: &mut TestAppContext) {
        let script = vec![
            ScriptedResponse::text("uno").hang_after(2),
            ScriptedResponse::text("dos"),
        ];
        let (h, cx) = harness(script, cx);
        send("primero", cx);
        // The turn runs on another thread: wait until it has taken its
        // answer, or Ctrl+C could land before the model is ever called.
        let llm = h.llm.clone();
        wait_until(&h, cx, "the first turn to reach the model", move |_, _| {
            llm.remaining() == 1
        });
        send("segundo", cx);
        h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 1));
        // Ctrl+C drops the queue with the turn.
        cx.simulate_keystrokes("ctrl-c");
        wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
        h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 0));
        assert_eq!(h.llm.remaining(), 1, "the queued prompt was not sent");
    }

    #[gpui::test]
    fn leader_p_cycles_the_permission_mode(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_keystrokes("ctrl-g p");
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.controller.permission_mode(), PermissionMode::Ask.next());
            assert_eq!(
                s.status().read(cx).state().permission_mode(),
                PermissionMode::Ask.next()
            );
        });
    }

    #[gpui::test]
    fn leader_m_puts_model_in_the_prompt_and_esc_keeps_it_for_one_turn(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
        cx.simulate_keystrokes("ctrl-g m");
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.input().read(cx).text(cx), "/model ");
        });
        cx.simulate_input("opus");
        cx.simulate_keystrokes("escape enter");
        cx.run_until_parked();
        h.session
            .read_with(cx, |s, _| assert_eq!(s.model_override(), Some("opus")));

        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.model_override(), None, "one turn only")
        });
        let model = h.llm.last_request().map(|r| r.model.spec.id);
        assert_eq!(model, Some(ModelPolicy::default().expand("opus")));
    }

    #[gpui::test]
    fn the_model_popup_lists_the_catalog_filters_and_saves(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/model", cx);
        cx.run_until_parked();
        h.session.read_with(cx, |s, cx| {
            let input = s.input().read(cx);
            let Some(ModelPopup::Entries(entries)) = input.model_popup(cx) else {
                panic!("the popup lists the catalog");
            };
            assert!(
                entries
                    .iter()
                    .any(|c| c.model.as_str() == "opencode-go:kimi-k3")
            );
            // Claude aliases: no provider here serves them.
            assert!(
                entries.iter().all(|c| c.model.provider().is_some()),
                "{entries:?}"
            );
        });
        cx.simulate_input("kimi");
        h.session.read_with(cx, |s, cx| {
            let Some(ModelPopup::Entries(entries)) = s.input().read(cx).model_popup(cx) else {
                panic!("still entries");
            };
            assert_eq!(entries.len(), 1, "{entries:?}");
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(*h.logins.lock().unwrap(), ["model opencode-go:kimi-k3"]);
        assert_eq!(
            *h.reopens.borrow(),
            [SessionEvent::Reopen {
                notice: Some((
                    NoticeLevel::Info,
                    "model `opencode-go:kimi-k3` selected".into()
                )),
            }]
        );
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.model_override(), None, "a saved pick is not an override");
        });
    }

    #[gpui::test]
    fn model_set_saves_and_model_clear_drops_the_override(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        send("/model opus", cx);
        cx.run_until_parked();
        h.session
            .read_with(cx, |s, _| assert_eq!(s.model_override(), Some("opus")));
        send("/model clear", cx);
        cx.run_until_parked();
        h.session
            .read_with(cx, |s, _| assert_eq!(s.model_override(), None));
        send("/model set opencode-go:glm-5.3", cx);
        cx.run_until_parked();
        assert_eq!(*h.logins.lock().unwrap(), ["model opencode-go:glm-5.3"]);
        assert_eq!(h.reopens.borrow().len(), 1);
    }

    #[gpui::test]
    fn plugin_modals_open_over_the_window_and_close(cx: &mut TestAppContext) {
        use domain::block::CustomNode;
        use domain::modal::{ModalId, ModalSpec};
        let (h, cx) = harness(Vec::new(), cx);
        let id = ModalId::new("cov/report");
        h.sink.emit(DomainEvent::ModalOpened {
            spec: ModalSpec {
                id: id.clone(),
                plugin: "cov".into(),
                title: "Report".into(),
                width: domain::modal::Percent::FULL,
                height: domain::modal::Percent::FULL,
                position: domain::modal::ModalPosition::CENTER,
                content: CustomNode::Markdown {
                    source: "# Coverage".into(),
                },
            },
        });
        cx.run_until_parked();
        let modal = h.session.read_with(cx, |s, _| s.modal().clone());
        modal.read_with(cx, |m, _| assert_eq!(m.ids(), std::slice::from_ref(&id)));
        assert!(blocks(&h, cx).is_empty(), "modals are not timeline blocks");
        // Esc closes it and the input gets the focus back.
        cx.simulate_keystrokes("escape");
        modal.read_with(cx, |m, _| assert!(!m.is_open()));
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            assert!(input.contains_focused(window, cx));
        });
        h.sink.emit(DomainEvent::ModalOpened {
            spec: ModalSpec {
                id: id.clone(),
                plugin: "cov".into(),
                title: "Report".into(),
                width: domain::modal::Percent::FULL,
                height: domain::modal::Percent::FULL,
                position: domain::modal::ModalPosition::CENTER,
                content: CustomNode::Text { text: "x".into() },
            },
        });
        cx.run_until_parked();
        h.session
            .update_in(cx, |s, window, cx| s.command("modal close", window, cx));
        modal.read_with(cx, |m, _| assert!(!m.is_open()));
    }

    #[gpui::test]
    fn commands_set_mode_and_model_and_clear(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        assert!(!blocks(&h, cx).is_empty());

        send("/mode plan", cx);
        send("/model opus", cx);
        send("/clear", cx);
        cx.run_until_parked();
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.controller.permission_mode(), PermissionMode::Plan);
            assert_eq!(s.model_override(), Some("opus"));
        });
        assert!(blocks(&h, cx).is_empty(), "/clear empties the timeline");
    }

    #[gpui::test]
    fn clear_during_a_turn_interrupts_it_and_resets_once_it_ends(cx: &mut TestAppContext) {
        let script = vec![
            ScriptedResponse::text("hola mundo cruel").hang_after(4),
            ScriptedResponse::text("nuevo"),
        ];
        let (h, cx) = harness(script, cx);
        send("saluda", cx);
        wait_until(&h, cx, "the partial answer", |s, cx| {
            s.timeline()
                .read(cx)
                .state()
                .entries()
                .iter()
                .filter_map(|e| text_of(&e.block.kind))
                .any(|text| text.starts_with("hola"))
        });

        send("/clear", cx);
        wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
        cx.run_until_parked();
        assert_eq!(finished(&h), [StopReason::Interrupted]);
        assert!(blocks(&h, cx).is_empty(), "the timeline was cleared");
        let session = h.session.clone();
        assert!(
            cx.update(|window, cx| session.read(cx).input().read(cx).owns_focus(window, cx)),
            "the prompt keeps the focus"
        );

        // The session starts over: the model sees only the new prompt.
        send("otra", cx);
        wait_until(&h, cx, "the new turn", |s, _| !s.is_running());
        let kinds = blocks(&h, cx);
        assert_eq!(
            kinds.iter().filter_map(text_of).collect::<Vec<_>>(),
            ["> otra\n", "nuevo"]
        );
        let last = h.llm.last_request().expect("a request");
        assert_eq!(last.messages.len(), 1, "{last:?}");
    }

    #[gpui::test]
    fn a_resumed_history_is_shown_before_new_blocks(cx: &mut TestAppContext) {
        let (h, cx) = harness_with(
            vec![ScriptedResponse::text("sigo")],
            vec![
                prompt_block("antes"),
                Block::new(BlockKind::Text {
                    markdown: "respuesta".into(),
                }),
            ],
            cx,
        );
        send("después", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        let kinds = blocks(&h, cx);
        assert_eq!(
            kinds.iter().filter_map(text_of).collect::<Vec<_>>(),
            ["> antes\n", "respuesta", "> después\n", "sigo"]
        );
    }

    #[gpui::test]
    fn a_lua_mapping_runs_the_command_it_asks_for(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        assert!(!blocks(&h, cx).is_empty());
        cx.simulate_keystrokes("ctrl-g x");
        cx.run_until_parked();
        assert!(blocks(&h, cx).is_empty(), "app.cmd(\"clear\") ran");
    }

    #[gpui::test]
    fn a_controller_command_sends_a_prompt_with_its_role(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("sin riesgos")], cx);
        cx.simulate_input("/review todo");
        cx.simulate_keystrokes("enter");
        wait_until(&h, cx, "the turn", |s, _| {
            !s.is_running() && h.llm.remaining() == 0
        });
        let request = h.llm.last_request().unwrap();
        assert_eq!(request.model.role, ModelRole::Plan);
        let kinds = blocks(&h, cx);
        assert_eq!(
            kinds.iter().filter_map(text_of).next(),
            Some("> revisa todo\n")
        );
    }

    #[gpui::test]
    fn a_command_calling_itself_stops(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_input("/loop");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        h.session.read_with(cx, |s, _| assert!(!s.is_running()));
    }

    #[gpui::test]
    fn a_file_cited_by_a_block_is_referenced_in_the_prompt(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.update(|_, cx| {
            crate::blocks::sources::emit(
                BlockEvent::OpenFile {
                    path: WorkspacePath::parse("src/lib.rs").unwrap(),
                    line: Some(7),
                },
                cx,
            );
        });
        cx.run_until_parked();
        let text = h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
        assert_eq!(text, "@src/lib.rs:7 ");
    }

    fn help_view(h: &Harness, cx: &mut VisualTestContext) -> Option<Entity<HelpView>> {
        let modal = h.session.read_with(cx, |s, _| s.modal().clone());
        modal.read_with(cx, |m, _| {
            m.view(&help_modal_id())
                .and_then(|view| view.clone().downcast::<HelpView>().ok())
        })
    }

    #[gpui::test]
    fn f1_opens_the_help_and_esc_closes_it(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_keystrokes("f1");
        cx.run_until_parked();
        let view = help_view(&h, cx).expect("the help is open");
        view.read_with(cx, |v, _| {
            assert_eq!(v.section(), Section::Commands);
            let names: Vec<_> = v
                .snapshot()
                .commands
                .iter()
                .map(|c| c.name.clone())
                .collect();
            assert!(names.contains(&"help".to_owned()), "{names:?}");
            assert!(v.snapshot().keys.iter().any(|k| k.keys == "<F1>"));
            assert!(
                v.snapshot()
                    .models
                    .iter()
                    .any(|m| m.name == "permission mode" && m.value == "ask")
            );
        });
        cx.update(|window, cx| {
            assert!(
                view.focus_handle(cx).contains_focused(window, cx),
                "the help has the focus"
            );
        });
        // Number keys switch tabs; Tab goes on.
        cx.simulate_keystrokes("2");
        view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Keys));
        cx.simulate_keystrokes("8 tab");
        view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Commands));
        cx.simulate_keystrokes("j j");
        view.read_with(cx, |v, _| assert_eq!(v.selected(), 2));
        cx.simulate_keystrokes("k y");
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("/model"));

        cx.simulate_keystrokes("escape");
        assert!(help_view(&h, cx).is_none(), "Esc closes it");
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            assert!(input.contains_focused(window, cx), "the focus is back");
        });
    }

    #[gpui::test]
    fn leader_h_opens_the_help_and_typing_filters(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_keystrokes("ctrl-g h");
        cx.run_until_parked();
        let view = help_view(&h, cx).expect("F1 opens the help");
        cx.simulate_keystrokes("/");
        cx.update(|window, cx| assert!(view.read(cx).is_searching(window, cx)));
        cx.simulate_input("block.toggle");
        cx.run_until_parked();
        view.read_with(cx, |v, cx| {
            assert_eq!(v.query(cx), "block.toggle");
            let f = v.filtered();
            assert_eq!(f.commands.len(), 0);
            assert_eq!(f.keys.len(), 2, "block.toggle and block.toggle_view");
            // The search moved to the section with matches.
            assert_eq!(v.section(), Section::Keys);
        });
        // Down moves while typing; Enter goes back to the list.
        cx.simulate_keystrokes("down enter");
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert_eq!(v.selected(), 1);
            assert!(!v.is_searching(window, cx));
        });
        cx.simulate_keystrokes("escape");
        assert!(help_view(&h, cx).is_none());
        h.session
            .read_with(cx, |s, cx| assert!(!s.modal().read(cx).is_open()));
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            assert!(input.contains_focused(window, cx), "back in the prompt");
        });
    }

    #[gpui::test]
    fn the_focus_cycles_and_typing_or_esc_brings_it_back(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        let in_input = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let input = h.session.read(cx).input().focus_handle(cx);
                input.contains_focused(window, cx)
            })
        };
        let in_timeline = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let timeline = h.session.read(cx).timeline().focus_handle(cx);
                timeline.contains_focused(window, cx)
            })
        };
        cx.simulate_keystrokes("ctrl-tab");
        assert!(in_timeline(cx), "Ctrl+Tab moves to the timeline");
        let selected = |cx: &mut VisualTestContext| {
            h.session
                .read_with(cx, |s, cx| s.timeline().read(cx).state().selected())
        };
        let last = selected(cx);
        assert!(last.is_some(), "a block is selected on entering");
        cx.simulate_keystrokes("up");
        assert_ne!(selected(cx), last, "arrows move the selection");
        // Esc goes back to the prompt.
        cx.simulate_keystrokes("escape");
        assert!(in_input(cx));
        // Typing in the timeline types into the prompt.
        cx.simulate_keystrokes("ctrl-tab");
        assert!(in_timeline(cx));
        cx.simulate_input("x");
        assert!(in_input(cx));
        h.session
            .read_with(cx, |s, cx| assert_eq!(s.input().read(cx).text(cx), "x"));
        // Shortcuts work from the timeline too.
        cx.simulate_keystrokes("ctrl-tab ctrl-g p");
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.controller.permission_mode(), PermissionMode::Ask.next());
        });
    }

    #[gpui::test]
    fn esc_closes_the_outline_before_focusing_the_prompt(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        let timeline = h.session.read_with(cx, |s, _| s.timeline().clone());
        timeline.update_in(cx, |t, window, cx| t.toggle_outline(window, cx));
        cx.run_until_parked();
        let in_input = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let input = h.session.read(cx).input().focus_handle(cx);
                input.contains_focused(window, cx)
            })
        };
        assert!(timeline.read_with(cx, |t, _| t.outline_open()));
        assert!(!in_input(cx), "the open menu takes the focus");
        cx.simulate_keystrokes("escape");
        assert!(
            !timeline.read_with(cx, |t, _| t.outline_open()),
            "Esc closes the menu"
        );
        assert!(!in_input(cx), "the first Esc only closes the menu");
        cx.simulate_keystrokes("escape");
        assert!(in_input(cx), "a second Esc goes back to the prompt");
    }

    #[gpui::test]
    fn a_focused_block_widget_gets_esc_and_its_keys(cx: &mut TestAppContext) {
        use domain::devops::flame::{FlameTree, ProfileFormat};
        let mut tree = FlameTree::new();
        tree.add(["main", "parse"], 30);
        tree.add(["idle"], 10);
        let spec = tree.finish(ProfileFormat::Folded, "samples");
        let block = Block::new(BlockKind::FlameGraph(Box::new(spec.clone())));
        let id = block.id;
        let (h, cx) = harness_with(Vec::new(), vec![block], cx);
        cx.run_until_parked();
        let flame = cx.update(|window, cx| crate::blocks::flame::view(id, &spec, window, cx));
        flame.update_in(cx, |f, window, cx| {
            f.zoom_to(1, cx);
            window.focus(&f.focus_handle(cx), cx);
        });
        cx.run_until_parked();
        let prompt = |cx: &mut VisualTestContext| {
            h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx))
        };
        let in_input = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let input = h.session.read(cx).input().focus_handle(cx);
                input.contains_focused(window, cx)
            })
        };
        cx.simulate_keystrokes("escape");
        assert_eq!(flame.read_with(cx, |f, _| f.zoom()), 0, "Esc zooms out");
        assert!(!in_input(cx), "the widget took the Esc");
        // A key the widget does not take still lands in the prompt.
        cx.simulate_keystrokes("x");
        assert_eq!(prompt(cx), "x");
        assert!(in_input(cx));
    }

    #[gpui::test]
    fn a_focused_list_widget_gets_the_arrows_and_enter(cx: &mut TestAppContext) {
        use domain::devops::logs::{LogFormat, LogSpec};
        let list: domain::widget::list::ListSpec = serde_json::from_value(serde_json::json!({
            "items": [
                { "text": "one", "fields": { "k": "v" } },
                { "text": "two", "fields": { "k": "v" } },
                { "text": "three" },
            ]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let spec = LogSpec {
            path: None,
            format: LogFormat::Json,
            total_lines: 3,
            truncated: false,
            list: list.clone(),
            histogram: None,
        };
        let block = Block::new(BlockKind::Logs(Box::new(spec)));
        let id = block.id;
        let (h, cx) = harness_with(Vec::new(), vec![block], cx);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let list = cx.update(|window, cx| {
            crate::widgets::list::view(crate::blocks::custom::node_id(id, 0), &list, window, cx)
        });
        list.update_in(cx, |l, window, cx| {
            window.focus(&l.focus_handle().clone(), cx);
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("down down enter");
        list.read_with(cx, |l, _| {
            assert_eq!(l.selected(), Some(1), "the arrows reach the list");
            assert!(l.is_open(1), "and so does Enter");
        });
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.input().read(cx).text(cx), "", "nothing reached the prompt");
        });
    }

    #[gpui::test]
    fn the_picker_closes_by_shortcut_click_outside_and_esc(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        let mode =
            |cx: &mut VisualTestContext| h.session.read_with(cx, |s, cx| s.picker.read(cx).mode());
        let in_input = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let input = h.session.read(cx).input().focus_handle(cx);
                input.contains_focused(window, cx)
            })
        };
        // The shortcut toggles it; the other one switches the mode.
        cx.simulate_keystrokes("ctrl-g f f");
        assert_eq!(mode(cx), Some(crate::picker::PickerMode::Files));
        cx.simulate_keystrokes("ctrl-g f g");
        assert_eq!(mode(cx), Some(crate::picker::PickerMode::Grep));
        cx.simulate_keystrokes("ctrl-g f g");
        assert_eq!(mode(cx), None, "the same shortcut closes it");
        assert!(in_input(cx));
        assert_eq!(
            h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx)),
            "",
            "the keys after the leader are not typed"
        );
        // A click on the backdrop closes it.
        cx.simulate_keystrokes("ctrl-g f f");
        cx.run_until_parked();
        let backdrop = cx.debug_bounds("picker-backdrop").expect("backdrop drawn");
        let corner = backdrop.origin + gpui::point(gpui::px(4.), gpui::px(4.));
        cx.simulate_click(corner, gpui::Modifiers::none());
        assert_eq!(mode(cx), None, "a click outside closes it");
        // So does its close button.
        cx.simulate_keystrokes("ctrl-g f f");
        cx.run_until_parked();
        let close = cx.debug_bounds("picker-close").expect("close drawn");
        cx.simulate_click(close.center(), gpui::Modifiers::none());
        assert_eq!(mode(cx), None, "the close button closes it");
        // Esc closes it with the focus outside its input.
        cx.simulate_keystrokes("ctrl-g f f");
        h.session.update_in(cx, |s, window, cx| {
            window.focus(&s.timeline().focus_handle(cx), cx);
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(mode(cx), None, "Esc closes it from anywhere");
    }

    #[gpui::test]
    fn the_palette_opens_the_pickers_and_their_alt_keys_reach_them(cx: &mut TestAppContext) {
        use crate::picker::PickerMode;
        let (h, cx) = harness(Vec::new(), cx);
        let picker = h.session.read_with(cx, |s, _| s.picker.clone());
        // The `/` palette lists them with their keys.
        cx.simulate_input("/picker");
        cx.run_until_parked();
        h.session.read_with(cx, |s, cx| {
            let rows = s.input().read(cx).palette(cx);
            let files = rows
                .iter()
                .find(|row| row.name == "picker.files")
                .expect("listed");
            assert!(files.description.ends_with("(Ctrl+G ff)"), "{}", files.description);
        });
        // Running it from the palette opens the picker.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(picker.read_with(cx, |p, _| p.mode()), Some(PickerMode::Files));
        h.session.update_in(cx, |s, window, cx| s.command("picker.grep", window, cx));
        assert_eq!(picker.read_with(cx, |p, _| p.mode()), Some(PickerMode::Grep));
        // `Alt+…` flips the options without going to the keymap or being typed.
        cx.simulate_keystrokes("alt-c alt-w alt-p");
        picker.read_with(cx, |p, cx| {
            assert!(p.options().match_case && p.options().whole_word);
            assert!(!p.options().regex);
            assert!(!p.preview_enabled());
            assert_eq!(p.query(cx), "");
        });
        assert_eq!(
            h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx)),
            ""
        );
        // A match is previewed from the file, through the file system.
        cx.simulate_keystrokes("alt-p");
        cx.simulate_input("add");
        cx.run_until_parked();
        cx.executor().advance_clock(crate::picker::SEARCH_DEBOUNCE);
        cx.run_until_parked();
        cx.executor().advance_clock(crate::picker::preview::PREVIEW_DEBOUNCE);
        cx.run_until_parked();
        picker.read_with(cx, |p, _| {
            let preview = p.preview().expect("previewed");
            assert_eq!(preview.path.as_str(), "src/lib.rs");
            assert_eq!(preview.lines, ["pub fn add() {}"]);
            assert!(!preview.loading);
            assert_eq!(preview.match_index(), Some(0));
        });
    }

    #[gpui::test]
    fn vim_keys_move_in_the_focused_timeline(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        let selected = |cx: &mut VisualTestContext| {
            h.session
                .read_with(cx, |s, cx| s.timeline().read(cx).state().selected())
        };
        let prompt = |cx: &mut VisualTestContext| {
            h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx))
        };
        cx.simulate_keystrokes("ctrl-g w");
        let last = selected(cx);
        assert!(last.is_some());
        cx.simulate_keystrokes("k");
        assert_ne!(selected(cx), last, "k moves up");
        cx.simulate_keystrokes("shift-g");
        assert_eq!(selected(cx), last, "G goes to the last block");
        cx.simulate_keystrokes("g");
        assert_eq!(selected(cx), Some(0), "g goes to the first block");
        cx.simulate_keystrokes("j y");
        assert!(cx.read_from_clipboard().is_some(), "y copies the block");
        assert_eq!(prompt(cx), "", "vim keys are not typed");
        // Any other letter is still typed into the prompt.
        cx.simulate_keystrokes("x");
        assert_eq!(prompt(cx), "x");
    }

    #[gpui::test]
    fn a_bang_line_is_the_shell_command(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        let notice = |cx: &mut VisualTestContext| {
            h.session.read_with(cx, |s, cx| {
                let now = cx.background_executor().now();
                s.status()
                    .read(cx)
                    .state()
                    .notice(now)
                    .map(|n| n.message.clone())
            })
        };
        send("/!git status -s", cx);
        cx.run_until_parked();
        assert_eq!(notice(cx).as_deref(), Some("ran [git status -s]"));
        send("/! ls", cx);
        cx.run_until_parked();
        assert_eq!(notice(cx).as_deref(), Some("ran [ls]"));
    }

    #[gpui::test]
    fn the_palette_lists_the_commands(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_input("/mo");
        h.session.read_with(cx, |s, cx| {
            let input = s.input().read(cx);
            let names: Vec<_> = input.palette(cx).iter().map(|r| r.name.clone()).collect();
            assert_eq!(names, ["model", "mode", "modal"]);
        });
    }

    #[gpui::test]
    fn the_palette_hides_on_blur_and_on_a_click_outside(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        // Focus events are only sent to the active window.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let shown = |cx: &mut VisualTestContext| {
            h.session
                .read_with(cx, |s, cx| !s.input().read(cx).palette(cx).is_empty())
        };
        cx.simulate_input("/mo");
        assert!(shown(cx));
        h.session.update_in(cx, |s, window, cx| {
            window.focus(&s.timeline().focus_handle(cx), cx);
        });
        cx.run_until_parked();
        assert!(!shown(cx), "the prompt lost the focus");
        cx.simulate_keystrokes("escape");
        cx.simulate_input("d");
        assert!(shown(cx), "typing shows it again");
        // A click on the timeline.
        cx.simulate_click(
            gpui::point(gpui::px(20.), gpui::px(20.)),
            gpui::Modifiers::none(),
        );
        assert!(!shown(cx), "a click outside hides it");
    }

    #[gpui::test]
    fn help_command_opens_a_section_or_a_search(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_input("/help models");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let view = help_view(&h, cx).expect("/help opens the help");
        view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Models));
        cx.simulate_keystrokes("escape");
        h.session
            .update_in(cx, |s, window, cx| s.command("help clear", window, cx));
        cx.run_until_parked();
        let view = help_view(&h, cx).expect("a search opens it too");
        view.read_with(cx, |v, cx| {
            assert_eq!(v.query(cx), "clear");
            assert_eq!(v.section(), Section::Commands);
            let rows = v.rows();
            assert!(rows.iter().any(|r| r.copy == "/clear"), "{rows:?}");
        });
    }

    #[gpui::test]
    fn an_unknown_command_points_to_the_help(cx: &mut TestAppContext) {
        let (h, cx) = harness(Vec::new(), cx);
        cx.simulate_input("/frobnicate");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        h.session.read_with(cx, |s, cx| {
            let state = s.status().read(cx).state();
            let notice = state.notice(Instant::now()).map(|n| n.message.clone());
            assert_eq!(notice, Some(unknown_command_hint("frobnicate")));
        });
    }

    #[test]
    fn file_references_name_the_line_of_a_match() {
        let path = WorkspacePath::parse("src/lib.rs").unwrap();
        assert_eq!(file_reference(&path, None), "@src/lib.rs ");
        assert_eq!(file_reference(&path, Some(3)), "@src/lib.rs:3 ");
        let spaced = WorkspacePath::parse("My Docs/a b.rs").unwrap();
        assert_eq!(file_reference(&spaced, Some(3)), "@\"My Docs/a b.rs\":3 ");
    }

    fn paste_png(h: &Harness, cx: &mut VisualTestContext) {
        let png = crate::input::attachments::tests::encoded(2, 2, image::ImageFormat::Png);
        let input = h.session.read_with(cx, |s, _| s.input().clone());
        input.update(cx, |input, cx| {
            input.attach_images(
                vec![gpui::Image::from_bytes(gpui::ImageFormat::Png, png)],
                cx,
            );
        });
    }

    fn image_blocks(h: &Harness, cx: &mut VisualTestContext) -> usize {
        blocks(h, cx)
            .iter()
            .filter(|kind| matches!(kind, BlockKind::Image { .. }))
            .count()
    }

    #[gpui::test]
    fn chat_only_images_become_blocks_without_calling_the_model(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![], cx);
        paste_png(&h, cx);
        cx.simulate_keystrokes("ctrl-g i");
        let input = h.session.read_with(cx, |s, _| s.input().clone());
        input.read_with(cx, |input, _| {
            assert_eq!(input.image_mode(), ImageMode::ChatOnly);
        });
        send("the bug", cx);
        wait_until(&h, cx, "the image block", |s, cx| {
            s.timeline()
                .read(cx)
                .state()
                .entries()
                .iter()
                .any(|e| matches!(e.block.kind, BlockKind::Image { .. }))
        });
        assert!(h.llm.requests().is_empty(), "the model was called");
        assert_eq!(image_blocks(&h, cx), 1);
        assert!(
            blocks(&h, cx)
                .iter()
                .filter_map(text_of)
                .any(|t| t == "> the bug\n")
        );
        input.read_with(cx, |input, _| {
            assert!(input.images().is_empty());
            assert_eq!(input.image_mode(), ImageMode::Model);
        });
    }

    #[gpui::test]
    fn model_images_go_with_the_prompt(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("a red square")], cx);
        paste_png(&h, cx);
        send("what is it?", cx);
        wait_until(&h, cx, "the answer", |s, _| !s.is_busy());
        let request = h.llm.last_request().unwrap();
        let content = &request.messages[0].content;
        assert!(
            matches!(&content[0], ContentPart::Text { text, .. } if text == "what is it?"),
            "{content:?}"
        );
        assert!(matches!(
            &content[1],
            ContentPart::Image { media_type, .. } if media_type == "image/png"
        ));
    }
}
