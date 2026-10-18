//! The agent session in the main window: wires the timeline, the input, the
//! status bar, the workspace tree, the pickers, the model menu and the
//! approval dialog to a [`SessionController`].
//!
//! The UI never touches the agent's adapters: it calls the controller (the
//! input port, implemented by the binary over the `AgentService`) and shows
//! the [`DomainEvent`]s that the controller's event sink sends back on a
//! channel. One consumer task applies them, in order, to the timeline and
//! the status bar.
//!
//! # Turns
//!
//! One turn runs at a time. A prompt submitted while a turn runs is
//! **queued** (the input stays usable, the status bar says how many are
//! waiting) and sent when the turn ends. `Ctrl+C` (`turn.interrupt`, in any
//! mode and also while a picker or the approval dialog has focus) cancels
//! the [`CancelToken`] of the running turn and drops the queue; the turn
//! ends as `Interrupted` and the next prompt starts a new one.
//!
//! # Keys and commands
//!
//! - `<leader>p` cycles the permission mode, `<leader>m` opens the model
//!   menu: the policy's aliases plus "default"; the choice is the override
//!   of the **next** turn only.
//! - `<leader>e` shows the tree, `<leader>ff` / `<leader>fg` the pickers.
//!   Choosing a file inserts `@path ` (or `@path:line ` for a text match)
//!   at the cursor of the prompt, in insert mode, so it can be referenced
//!   in the next prompt. A file cited by a block (`Sources`) does the same.
//! - `:model <alias>` (no argument clears it), `:mode <ask|auto-edit|plan>`,
//!   `:clear` (a new, empty session; not while a turn runs) and `:q`.
//! - Any other `:name args` goes to [`SessionController::run_command`]
//!   (commands of `init.lua`, `:mcp`, `:implement`, `:research`); a name
//!   with a dot runs that builtin action (`:picker.files`).
//! - A mapping to a Lua function runs [`SessionController::run_callback`].
//!   What it answers ([`Reply`]) is shown or run here: `app.cmd(name)` as a
//!   `:` command and `app.send(text, { role })` as a prompt with that role.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use application::keymap::{Action, Keymap, Mode, default_keymap};
use application::ports::events::{DomainEvent, NoticeLevel};
use application::ports::plugin::PluginAction;
use application::ports::search::{ContentSearchPort, FileSearchPort};
use application::usecases::cancel::CancelToken;
use domain::block::{Block, BlockKind};
use domain::conversation::StopReason;
use domain::model::{ModelId, ModelRole};
use domain::permission::PermissionMode;
use domain::workspace::WorkspacePath;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, KeyDownEvent, KeystrokeEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::app::RootView;
use crate::approval::{ApprovalInbox, ApprovalView};
use crate::blocks::sources::{BlockEvent, block_events};
use crate::input::{InputEvent, InputView};
use crate::picker::{PickerEvent, PickerView};
use crate::status::StatusView;
use crate::timeline::TimelineView;
use crate::tree::{TreePanel, TreePanelEvent};

/// Action of `Ctrl+C`.
pub const INTERRUPT_ACTION: &str = "turn.interrupt";
/// Action of `<leader>p`.
pub const PERMISSION_ACTION: &str = "permission.cycle";
/// Action of `<leader>m`.
pub const MODEL_ACTION: &str = "model.pick";

/// One turn asked by the UI.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// What the user typed.
    pub prompt: String,
    /// Alias, role or id for this turn only (`<leader>m`, `:model`).
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
    /// A block for the timeline.
    Block(Block),
    /// Actions requested by Lua (`app.cmd`, `app.send`), in order.
    Actions(Vec<PluginAction>),
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

    /// Starts a new, empty session. Only called with no turn running.
    fn new_session(&self) -> BoxFuture<'static, ()>;

    /// Runs the Lua function `id` of a mapping with `args`.
    fn run_callback(&self, id: u64, args: serde_json::Value) -> BoxFuture<'static, Reply> {
        let _ = (id, args);
        futures::future::ready(Reply::Notice(
            NoticeLevel::Warning,
            "no hay plugins cargados".into(),
        ))
        .boxed()
    }

    /// Runs the `:name args` command, if the controller has one by that
    /// name; `None` means there is none.
    fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
        let _ = (name, args);
        None
    }
}

/// The block that shows a prompt sent by the user in the timeline.
pub fn prompt_block(prompt: &str) -> Block {
    let quoted: String = prompt.lines().map(|line| format!("> {line}\n")).collect();
    Block::new(BlockKind::Text { markdown: quoted })
}

/// What `@` reference a file chosen in the tree or a picker inserts.
pub fn file_reference(path: &WorkspacePath, line: Option<u64>) -> String {
    match line {
        Some(line) => format!("@{path}:{line} "),
        None => format!("@{path} "),
    }
}

/// Everything the session needs from the binary.
pub struct SessionParts {
    /// The agent.
    pub controller: Arc<dyn SessionController>,
    /// Events of the controller's sink.
    pub events: UnboundedReceiver<DomainEvent>,
    /// Requests of the `UiApproval` given to the agent.
    pub approvals: ApprovalInbox,
    /// File search for the tree and `<leader>ff`.
    pub files: Arc<dyn FileSearchPort>,
    /// Text search for `<leader>fg`.
    pub content: Arc<dyn ContentSearchPort>,
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
            files,
            content,
            workspace_label: String::new(),
            workspace_dir: None,
            model: None,
            keymap: default_keymap(),
            timeoutlen: None,
            history: Vec::new(),
        }
    }
}

/// The `<leader>m` menu.
struct ModelMenu {
    choices: Vec<Option<ModelChoice>>,
    selected: usize,
}

/// The session: owns the views of the main window and the turn in flight.
/// It is also the overlay that draws the model menu.
pub struct Session {
    controller: Arc<dyn SessionController>,
    root: WeakEntity<RootView>,
    timeline: Entity<TimelineView>,
    input: Entity<InputView>,
    status: Entity<StatusView>,
    tree: Entity<TreePanel>,
    picker: Entity<PickerView>,
    approval: Entity<ApprovalView>,
    focus_handle: FocusHandle,
    running: Option<CancelToken>,
    turn: Option<Task<()>>,
    queue: VecDeque<(String, Option<ModelRole>)>,
    model_override: Option<String>,
    menu: Option<ModelMenu>,
    _events: Task<()>,
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
    let (timeline, input, status, tree, picker, approval) = (
        s.timeline.clone(),
        s.input.clone(),
        s.status.clone(),
        s.tree.clone(),
        s.picker.clone(),
        s.approval.clone(),
    );
    root.update(cx, |root, cx| {
        root.set_timeline(timeline, cx);
        root.set_input(input.clone(), cx);
        root.set_status(status, cx);
        root.set_side_panel(tree, cx);
        root.push_overlay(picker, cx);
        root.push_overlay(session.clone(), cx);
        root.push_overlay(approval, cx);
    });
    input.update(cx, |input, cx| input.focus(window, cx));
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
            files,
            content,
            workspace_label,
            workspace_dir,
            model,
            keymap,
            timeoutlen,
            history,
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
            if let Some(model) = model {
                state.set_model(model);
            }
        });
        let input = cx.new(|cx| {
            let mut input = InputView::with_keymap(keymap, window, cx);
            if let Some(timeoutlen) = timeoutlen {
                input.sequencer_mut().set_timeoutlen(timeoutlen);
            }
            input
        });
        let tree = cx.new(|cx| TreePanel::new(files.clone(), cx));
        let picker = cx.new(|cx| PickerView::new(files, content, window, cx));
        let approval = cx.new(|cx| ApprovalView::new(approvals, window, cx));

        let mut watch_error = None;
        if let Some(dir) = &workspace_dir {
            tree.update(cx, |tree, cx| {
                if let Err(err) = tree.watch_dir(dir, cx) {
                    watch_error = Some(format!("el árbol no se refrescará solo: {err}"));
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
                        // The picker gave focus back; keep the input's mode in
                        // sync with it.
                        PickerEvent::Dismissed => {
                            this.input.update(cx, |input, cx| input.focus(window, cx));
                        }
                    }
                },
            ),
            Self::intercept_interrupt(cx),
        ];
        // A file cited by a block (Sources) opens like a picker choice.
        let block_events = block_events(cx);
        subscriptions.push(cx.subscribe_in(
            &block_events,
            window,
            |this, _, event: &BlockEvent, window, cx| {
                if let BlockEvent::OpenFile { path, line } = event {
                    this.insert_reference(&file_reference(path, *line), window, cx);
                }
            },
        ));

        let mut this = Self {
            controller,
            root,
            timeline,
            input,
            status,
            tree,
            picker,
            approval,
            focus_handle: cx.focus_handle(),
            running: None,
            turn: None,
            queue: VecDeque::new(),
            model_override: None,
            menu: None,
            _events: Self::consume(events, cx),
            _subscriptions: subscriptions,
        };
        if let Some(message) = watch_error {
            this.notice(NoticeLevel::Warning, message, cx);
        }
        this
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

    /// `Ctrl+C` while focus is outside the input (picker, dialog, menu):
    /// the input maps it to `turn.interrupt` itself.
    fn intercept_interrupt(cx: &mut Context<Self>) -> Subscription {
        let weak = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            let keys = &event.keystroke;
            let m = &keys.modifiers;
            if keys.key != "c" || !m.control || m.alt || m.platform || m.shift {
                return;
            }
            weak.update(cx, |this, cx| {
                let in_input = this.input.focus_handle(cx).contains_focused(window, cx);
                if !in_input {
                    this.interrupt(cx);
                }
            })
            .ok();
        })
    }

    /// The timeline view (replaced by `:clear`).
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

    /// The approval dialog.
    pub fn approval(&self) -> &Entity<ApprovalView> {
        &self.approval
    }

    /// Whether a turn is running.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Prompts waiting for the running turn.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The override of the next turn.
    pub fn model_override(&self) -> Option<&str> {
        self.model_override.as_deref()
    }

    /// Whether the model menu is open.
    pub fn is_menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Applies an event to the timeline and the status bar.
    pub fn dispatch(&mut self, event: DomainEvent, cx: &mut Context<Self>) {
        self.dispatch_all(vec![event], cx);
    }

    fn dispatch_all(&mut self, events: Vec<DomainEvent>, cx: &mut Context<Self>) {
        self.status.update(cx, |status, cx| {
            for event in &events {
                status.apply(event, cx);
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
    /// it while a turn runs.
    pub fn submit_as(&mut self, prompt: String, role: Option<ModelRole>, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.queue.push_back((prompt, role));
            let waiting = self.queue.len();
            self.notice(
                NoticeLevel::Info,
                format!("en cola: {waiting} (se envía al terminar el turno; Ctrl+C la descarta)"),
                cx,
            );
            return;
        }
        let cancel = CancelToken::new();
        self.running = Some(cancel.clone());
        let turn = self.controller.run_turn(TurnRequest {
            prompt,
            model_override: self.model_override.take(),
            role,
            cancel,
        });
        self.turn = Some(cx.spawn(async move |this, cx| {
            let end = turn.await;
            this.update(cx, |this, cx| this.turn_ended(end, cx)).ok();
        }));
        cx.notify();
    }

    fn turn_ended(&mut self, end: TurnEnd, cx: &mut Context<Self>) {
        self.running = None;
        self.turn = None;
        match end {
            TurnEnd::Finished(StopReason::Interrupted) => {
                self.notice(NoticeLevel::Info, "turno interrumpido", cx);
            }
            TurnEnd::Finished(_) | TurnEnd::Failed(_) => {}
            TurnEnd::Unavailable(message) => {
                self.queue.clear();
                self.dispatch(
                    DomainEvent::BlockAdded {
                        block: Block::new(BlockKind::Error {
                            message,
                            hint: Some("corrige la configuración y vuelve a abrir crest".into()),
                        }),
                    },
                    cx,
                );
            }
        }
        if let Some((next, role)) = self.queue.pop_front() {
            self.submit_as(next, role, cx);
        }
        cx.notify();
    }

    /// `Ctrl+C`: cancels the running turn and drops the queue. Returns
    /// whether a turn was running.
    pub fn interrupt(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(cancel) = &self.running else {
            return false;
        };
        if !cancel.is_cancelled() {
            cancel.cancel();
            let dropped = std::mem::take(&mut self.queue).len();
            if dropped > 0 {
                self.notice(
                    NoticeLevel::Info,
                    format!("interrumpiendo; {dropped} mensajes en cola descartados"),
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
            Some(model) => format!("siguiente turno con `{model}`"),
            None => "siguiente turno con el modelo por defecto".to_owned(),
        };
        self.model_override = model;
        self.notice(NoticeLevel::Info, message, cx);
    }

    /// Runs a `:` command.
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
        let (name, arg) = command.split_once(' ').unwrap_or((command, ""));
        let arg = arg.trim();
        match name {
            "q" | "quit" | "qa" => cx.quit(),
            "model" => {
                let model = (!arg.is_empty()).then(|| arg.to_owned());
                self.set_model_override(model, cx);
            }
            "mode" => match arg.parse::<PermissionMode>() {
                Ok(mode) => self.set_permission_mode(mode, cx),
                Err(err) => self.notice(NoticeLevel::Warning, format!(":mode: {err}"), cx),
            },
            "clear" => self.clear(window, cx),
            _ => {
                if let Some(reply) = self.controller.run_command(name, arg) {
                    self.await_reply(reply, depth, window, cx);
                } else if name.contains('.') {
                    self.action(&Action::Builtin(name.to_owned()), window, cx);
                } else {
                    self.notice(
                        NoticeLevel::Warning,
                        format!(
                            "comando desconocido :{command} (usa :model, :mode, :clear, :mcp, :implement, :research o :q)"
                        ),
                        cx,
                    );
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
        cx.spawn_in(window, async move |this, cx| {
            let reply = reply.await;
            this.update_in(cx, |this, window, cx| this.reply(reply, depth, window, cx))
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
            Reply::Block(block) => self.dispatch(DomainEvent::BlockAdded { block }, cx),
            Reply::Actions(actions) => {
                for action in actions {
                    match action {
                        PluginAction::Send { text, role } => self.submit_as(text, role, cx),
                        PluginAction::Cmd { command } if depth < MAX_COMMAND_DEPTH => {
                            self.command_at(&command, depth + 1, window, cx);
                        }
                        PluginAction::Cmd { command } => self.notice(
                            NoticeLevel::Warning,
                            format!(
                                "app.cmd(\"{command}\") ignorado: demasiados comandos anidados"
                            ),
                            cx,
                        ),
                    }
                }
            }
        }
    }

    /// `:clear`: a new session and an empty timeline.
    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.notice(
                NoticeLevel::Warning,
                "hay un turno en curso; Ctrl+C para interrumpirlo antes de :clear",
                cx,
            );
            return;
        }
        let reset = self.controller.new_session();
        let timeline = cx.new(TimelineView::new);
        self.timeline = timeline.clone();
        self.root
            .update(cx, |root, cx| root.set_timeline(timeline, cx))
            .ok();
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.spawn(async move |_, _| reset.await).detach();
        cx.notify();
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
            InputEvent::Submit(prompt) => self.submit(prompt.clone(), cx),
            InputEvent::Command(command) => self.command(command, window, cx),
            InputEvent::ModeChanged(mode) => {
                let mode: Mode = *mode;
                self.status
                    .update(cx, |status, cx| status.set_input_mode(mode, cx));
            }
            InputEvent::Action(action) => self.action(action, window, cx),
        }
    }

    fn action(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .timeline
            .update(cx, |view, cx| view.handle_action(action, cx))
            || self
                .tree
                .update(cx, |tree, cx| tree.handle_action(action, cx))
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
                    self.notice(NoticeLevel::Info, "no hay ningún turno en curso", cx);
                }
            }
            PERMISSION_ACTION => self.cycle_permission_mode(cx),
            MODEL_ACTION => self.open_menu(window, cx),
            _ => self.notice(
                NoticeLevel::Info,
                format!("la acción {action:?} todavía no hace nada"),
                cx,
            ),
        }
    }

    /// Opens the model menu with the focus on it.
    pub fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut choices = vec![None];
        choices.extend(self.controller.models().into_iter().map(Some));
        let selected = choices
            .iter()
            .position(|choice| {
                choice.as_ref().map(|c| c.alias.as_str()) == self.model_override.as_deref()
            })
            .unwrap_or(0);
        self.menu = Some(ModelMenu { choices, selected });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn close_menu(&mut self, choose: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.input.update(cx, |input, cx| input.focus(window, cx));
        if choose && let Some(choice) = menu.choices.get(menu.selected) {
            self.set_model_override(choice.as_ref().map(|c| c.alias.clone()), cx);
        }
        cx.notify();
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.menu else {
            return;
        };
        let len = menu.choices.len().max(1);
        match event.keystroke.key.as_str() {
            "down" | "j" | "tab" => menu.selected = (menu.selected + 1) % len,
            "up" | "k" => menu.selected = (menu.selected + len - 1) % len,
            "enter" => self.close_menu(true, window, cx),
            "escape" | "q" => self.close_menu(false, window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_menu(&self, menu: &ModelMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows = menu.choices.iter().enumerate().map(|(ix, choice)| {
            let label = match choice {
                Some(choice) => format!("{:<10} {}", choice.alias, choice.model),
                None => "default    (rol main)".to_owned(),
            };
            let mut row = div()
                .id(ix)
                .px_2()
                .child(SharedString::from(label))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(menu) = &mut this.menu {
                        menu.selected = ix;
                    }
                    this.close_menu(true, window, cx);
                }));
            if ix == menu.selected {
                row = row.bg(theme.list_active);
            }
            row
        });
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(80.))
            .child(
                v_flex()
                    .w(px(480.))
                    .max_w_full()
                    .gap_1()
                    .p_2()
                    .bg(theme.popover)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .child(
                        h_flex()
                            .text_color(theme.muted_foreground)
                            .child("Modelo del siguiente turno  (j/k, Enter, Esc)"),
                    )
                    .children(rows),
            )
    }
}

impl Render for Session {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self
            .menu
            .as_ref()
            .map(|menu| self.render_menu(menu, cx).into_any_element());
        div()
            .id("model-menu")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::menu_key))
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use application::ports::events::EventSink;
    use application::testing::fs::{InMemoryFs, ScriptedShell};
    use application::testing::llm::{ScriptedLlm, ScriptedResponse};
    use application::testing::system::{RecordingSink, RecordingTelemetry};
    use application::tools::files::{FileToolPorts, register_file_tools};
    use application::tools::registry::ToolRegistry;
    use application::usecases::run_turn::{AgentDeps, AgentService, TurnOptions};
    use domain::block::ToolStatus;
    use domain::conversation::Session as Conversation;
    use domain::model::{ModelPolicy, ModelRole};
    use domain::permission::PermissionPolicy;
    use domain::workspace::WorkspaceRoot;
    use futures::channel::oneshot;
    use gpui::{TestAppContext, VisualTestContext};
    use serde_json::json;

    use super::*;
    use crate::approval::UiApproval;

    /// A controller over the real `AgentService`, running each turn on its
    /// own thread as the binary runs it on tokio.
    struct AgentController {
        service: Arc<AgentService>,
        conversation: Arc<futures::lock::Mutex<Conversation>>,
        events: Arc<dyn EventSink>,
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
                    match service
                        .run_turn(&mut conversation, request.prompt, opts)
                        .await
                    {
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

        fn new_session(&self) -> BoxFuture<'static, ()> {
            let conversation = Arc::clone(&self.conversation);
            async move {
                *conversation.lock().await = Conversation::new("/ws", 0);
            }
            .boxed()
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

        /// `:review` sends a prompt with the plan role; `:loop` calls itself.
        fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
            let reply = match name {
                "review" => Reply::Actions(vec![PluginAction::Send {
                    text: format!("revisa {args}"),
                    role: Some(ModelRole::Plan),
                }]),
                "loop" => Reply::Actions(vec![PluginAction::Cmd {
                    command: "loop".into(),
                }]),
                _ => return None,
            };
            Some(futures::future::ready(reply).boxed())
        }
    }

    struct Harness {
        session: Entity<Session>,
        llm: Arc<ScriptedLlm>,
        sink: Arc<RecordingSink>,
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
        cx.executor().allow_parking();
        cx.update(gpui_component::init);
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                .with_file("src/lib.rs", "pub fn add() {}\n"),
        );
        let (sink, events) = RecordingSink::with_channel();
        let sink = Arc::new(sink);
        let (approval, approvals) = UiApproval::channel();
        let llm = Arc::new(ScriptedLlm::new(script));
        let mut tools = ToolRegistry::new();
        register_file_tools(
            &mut tools,
            FileToolPorts {
                file_search: fs.clone(),
                content_search: fs.clone(),
                fs: fs.clone(),
                shell: Arc::new(ScriptedShell::new()),
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
        let controller = Arc::new(AgentController {
            service: Arc::new(service),
            conversation: Arc::new(futures::lock::Mutex::new(Conversation::new("/ws", 0))),
            events: sink.clone(),
        });
        let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs);
        parts
            .keymap
            .set(Mode::Normal, "<leader>x", Action::Callback(7))
            .unwrap();
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
        (Harness { session, llm, sink }, cx)
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
        cx.simulate_keystrokes("escape space p");
        h.session.read_with(cx, |s, cx| {
            assert_eq!(s.controller.permission_mode(), PermissionMode::Ask.next());
            assert_eq!(
                s.status().read(cx).state().permission_mode(),
                PermissionMode::Ask.next()
            );
        });
    }

    #[gpui::test]
    fn the_model_menu_sets_the_override_of_the_next_turn(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
        cx.simulate_keystrokes("escape space m");
        h.session
            .read_with(cx, |s, _| assert!(s.is_menu_open(), "the menu is open"));
        cx.simulate_keystrokes("j enter");
        let alias = h.session.read_with(cx, |s, _| {
            assert!(!s.is_menu_open());
            s.model_override().map(str::to_owned)
        });
        let first = ModelPolicy::default().aliases().into_keys().next();
        assert_eq!(alias, first);

        cx.simulate_keystrokes("i");
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.model_override(), None, "one turn only")
        });
        let model = h.llm.last_request().map(|r| r.model.spec.id);
        let expected = ModelPolicy::default().expand(&alias.unwrap_or_default());
        assert_eq!(model, Some(expected));
    }

    #[gpui::test]
    fn commands_set_mode_and_model_and_clear(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
        send("hola", cx);
        wait_until(&h, cx, "the turn", |s, _| !s.is_running());
        assert!(!blocks(&h, cx).is_empty());

        cx.simulate_keystrokes("escape :");
        cx.simulate_input("mode plan");
        cx.simulate_keystrokes("enter :");
        cx.simulate_input("model opus");
        cx.simulate_keystrokes("enter :");
        cx.simulate_input("clear");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        h.session.read_with(cx, |s, _| {
            assert_eq!(s.controller.permission_mode(), PermissionMode::Plan);
            assert_eq!(s.model_override(), Some("opus"));
        });
        assert!(blocks(&h, cx).is_empty(), ":clear empties the timeline");
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
        cx.simulate_keystrokes("escape space x");
        cx.run_until_parked();
        assert!(blocks(&h, cx).is_empty(), "app.cmd(\"clear\") ran");
    }

    #[gpui::test]
    fn a_controller_command_sends_a_prompt_with_its_role(cx: &mut TestAppContext) {
        let (h, cx) = harness(vec![ScriptedResponse::text("sin riesgos")], cx);
        cx.simulate_keystrokes("escape :");
        cx.simulate_input("review todo");
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
        cx.simulate_keystrokes("escape :");
        cx.simulate_input("loop");
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

    #[test]
    fn file_references_name_the_line_of_a_match() {
        let path = WorkspacePath::parse("src/lib.rs").unwrap();
        assert_eq!(file_reference(&path, None), "@src/lib.rs ");
        assert_eq!(file_reference(&path, Some(3)), "@src/lib.rs:3 ");
    }

    #[test]
    fn the_prompt_block_quotes_every_line() {
        let block = prompt_block("uno\ndos");
        assert_eq!(text_of(&block.kind), Some("> uno\n> dos\n"));
    }
}
