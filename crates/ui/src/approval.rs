//! The approval dialog: [`UiApproval`] implements [`ApprovalPort`] for the
//! use cases, which run off the UI thread, and hands each request to the
//! window through a channel.
//!
//! `request()` sends a [`PendingApproval`] (the request, a oneshot to reply
//! on and a cancellation signal) and waits for the reply. Dropping that
//! future drops its end of the signal, so the dialog showing the request
//! closes; a closed dialog drops the reply, so the request fails with
//! [`ApprovalError::Cancelled`].

use std::collections::VecDeque;

use application_core::keymap::KeyCode;
use application_core::ports::approval::{
    ApprovalPreview, ApprovalRequest, ApprovalRequestId, ApprovalResponse,
};
use domain::permission::{
    RUN_COMMAND_TOOL, RuleMatcher, RuleOption, RuleScope, RungKind, ToolEffect, default_rung,
    is_exact_only, rule_ladder, split_command,
};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Global, KeyDownEvent, ScrollHandle, Task,
    WeakEntity, Window,
};

use crate::input::key_from_keystroke;

mod inbox;
mod render;

pub use inbox::{ApprovalInbox, PendingApproval, UiApproval};

/// The command line as shown to the user: the program and its arguments,
/// with arguments that need it in single quotes.
pub fn command_line(command: &str, args: &[String]) -> String {
    let quote = |arg: &str| {
        let plain = !arg.is_empty()
            && arg
                .chars()
                .all(|c| c.is_alphanumeric() || "-_./=:,+@%".contains(c));
        if plain {
            arg.to_owned()
        } else {
            format!("'{}'", arg.replace('\'', r"'\''"))
        }
    };
    std::iter::once(quote(command))
        .chain(args.iter().map(|arg| quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Dialog view
// ---------------------------------------------------------------------------

/// What the dialog reports after it closes a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalEvent {
    /// The user answered.
    Answered {
        /// Request that was answered.
        id: ApprovalRequestId,
        /// The answer sent to the requester.
        response: ApprovalResponse,
    },
    /// The requester stopped waiting before the user answered.
    Cancelled(ApprovalRequestId),
}

/// One answer the dialog offers: a row of its list, picked with the
/// cursor, a click or its letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// `y`: run the call once.
    Once,
    /// `s` (or `a`): run it and allow the pattern for this session.
    Session,
    /// `p`: run it and allow the pattern in every session of the project
    /// (saved in `.crest/permissions.local.toml`).
    Project,
    /// `e`: run it and stop asking about edits this session (auto-edit).
    /// Only offered for writes.
    AllEdits,
    /// `n`: reject, after an optional reason.
    Reject,
}

impl Choice {
    /// The letter that picks it.
    pub fn key(self) -> char {
        match self {
            Self::Once => 'y',
            Self::Session => 's',
            Self::Project => 'p',
            Self::AllEdits => 'e',
            Self::Reject => 'n',
        }
    }

    /// The choice a letter picks; `a` ("always") is kept as `s`.
    fn from_key(c: char) -> Option<Self> {
        match c {
            'y' => Some(Self::Once),
            's' | 'a' => Some(Self::Session),
            'p' => Some(Self::Project),
            'e' => Some(Self::AllEdits),
            'n' => Some(Self::Reject),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Once => "Yes, once",
            Self::Session => "Yes, this session for",
            Self::Project => "Yes, always in project",
            Self::AllEdits => "Yes, allow all edits this session",
            Self::Reject => "No, tell crest why…",
        }
    }
}

/// Why the pattern of a command cannot be widened.
const EXACT_ONLY_REASON: &str = "shells and interpreters are only allowed exactly";

/// The text of the pattern chip: `cargo test *`, `src/*`, the exact
/// subject, or "any input".
/// What the "all edits" row adds: the mode it switches to and how to
/// undo it.
fn all_edits_hint(mode_keys: Option<&str>) -> String {
    match mode_keys {
        Some(keys) => format!("(auto-edit · {keys} to undo)"),
        None => "(auto-edit · /mode ask to undo)".to_owned(),
    }
}

fn pattern_text(rung: &RuleOption) -> String {
    rung.rule
        .matcher
        .pattern()
        .unwrap_or_else(|| "any input".to_owned())
}

/// What a pattern broader than the exact action lets through, in words.
fn pattern_hint(rung: &RuleOption) -> Option<String> {
    if rung.kind == RungKind::Exact {
        return None;
    }
    match &rung.rule.matcher {
        RuleMatcher::Command(tokens) => Some(format!(
            "allows any `{} …`",
            domain::permission::command_line(tokens)
        )),
        RuleMatcher::Prefix(prefix) => Some(format!("allows any file under `{prefix}`")),
        RuleMatcher::Any => Some(format!("allows any input of `{}`", rung.rule.tool)),
        RuleMatcher::Exact(_) => None,
    }
}

/// The dialog the diff blocks' buttons answer: the last one created.
struct ActiveDialog(WeakEntity<ApprovalView>);

/// Whether the approval dialog is on screen, so other panes leave its keys
/// alone even when the prompt still holds the focus.
pub fn dialog_is_open(cx: &App) -> bool {
    cx.try_global::<ActiveDialog>()
        .and_then(|active| active.0.upgrade())
        .is_some_and(|view| view.read(cx).is_open())
}

impl Global for ActiveDialog {}

/// Answers the open request when it previews a change to `path`: approves
/// it once, or rejects it without a reason. Returns whether it answered.
/// Used by the buttons of a pending `Diff` block.
pub fn answer_diff(path: &str, approve: bool, window: &mut Window, cx: &mut App) -> bool {
    let Some(view) = cx
        .try_global::<ActiveDialog>()
        .and_then(|active| active.0.upgrade())
    else {
        return false;
    };
    view.update(cx, |view, cx| {
        let matches = matches!(
            view.current().map(|request| &request.preview),
            Some(ApprovalPreview::Diff(spec)) if spec.path == path
        );
        if matches {
            if approve {
                view.approve(window, cx);
            } else {
                view.reject(None, window, cx);
            }
        }
        matches
    })
}

/// A request on screen or in the queue.
struct Shown {
    key: u64,
    pending: PendingApproval,
    _cancel: Task<()>,
}

/// The modal approval dialog. Render it above the rest of the window (as
/// the last child of a `relative` container): it is empty while nothing is
/// pending and covers the window otherwise.
///
/// It shows the request and a list of answers ([`Choice`]): once, this
/// session or always in the project for a pattern, all edits this session
/// (writes only), or no. Keys while open: `Up`/`Down` move the cursor and
/// `Enter` picks the row; the letters `y`, `s` (or `a`), `p`, `e` and `n`
/// pick one directly. `Tab`/`Shift+Tab` widen and narrow the pattern of the
/// session and project rows ([`rule_ladder`]). `n` opens a reason line
/// (typed, then `Enter`; `Esc` goes back); `Esc` rejects without a reason.
/// `PageUp`/`PageDown`, `Shift+Up`/`Shift+Down` and `Home`/`End` scroll
/// the preview. Requests that arrive while one is open wait in a queue.
pub struct ApprovalView {
    focus_handle: FocusHandle,
    current: Option<Shown>,
    queue: VecDeque<Shown>,
    /// The reason being typed after `n`, if any.
    reason: Option<String>,
    /// Patterns the session and project rows offer for the current
    /// request, narrowest first.
    ladder: Vec<RuleOption>,
    /// The rung of `ladder` chosen.
    rung: usize,
    /// Whether `ladder` holds only the exact command because the program
    /// runs whatever its arguments say (a shell, `python -c`).
    exact_only: bool,
    /// Row of [`choices`](Self::choices) under the cursor.
    cursor: usize,
    /// Scroll of the preview.
    preview_scroll: ScrollHandle,
    restore_focus: Option<FocusHandle>,
    next_key: u64,
    /// Whether its session is on screen. A parked session's dialog keeps
    /// its queue but never takes the focus.
    shown: bool,
    /// Keys of `permission.cycle` (`Ctrl+G p`), named by the "all edits"
    /// row; `None` when unbound.
    mode_keys: Option<String>,
    _inbox: Task<()>,
}

impl EventEmitter<ApprovalEvent> for ApprovalView {}

impl ApprovalView {
    /// A dialog fed by `inbox`. It becomes the one that [`answer_diff`]
    /// answers.
    pub fn new(mut inbox: ApprovalInbox, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        cx.set_global(ActiveDialog(this));
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(pending) = inbox.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.push(pending, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            current: None,
            queue: VecDeque::new(),
            reason: None,
            ladder: Vec::new(),
            rung: 0,
            exact_only: false,
            cursor: 0,
            preview_scroll: ScrollHandle::new(),
            restore_focus: None,
            next_key: 0,
            shown: true,
            mode_keys: Some("<leader>p".to_owned()),
            _inbox: task,
        }
    }

    /// Sets the keys the "all edits" row names to undo it (those of
    /// `permission.cycle` in the session's keymap; `None` when unbound).
    pub fn set_mode_keys(&mut self, keys: Option<String>) {
        self.mode_keys = keys;
    }

    /// Tells the dialog whether its session is on screen. Shown again, it
    /// becomes the one [`answer_diff`] answers and takes the focus back if
    /// a request is open.
    pub fn set_shown(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = shown;
        if shown {
            let this = cx.entity().downgrade();
            cx.set_global(ActiveDialog(this));
            if self.current.is_some() {
                window.focus(&self.focus_handle, cx);
            }
        }
        cx.notify();
    }

    /// The request on screen, if any.
    pub fn current(&self) -> Option<&ApprovalRequest> {
        self.current.as_ref().map(|shown| shown.pending.request())
    }

    /// Requests waiting behind the current one.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether a request is on screen.
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    /// Whether the user is typing a rejection reason.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The rows offered for the request on screen, top to bottom; empty
    /// when nothing is open. [`Choice::AllEdits`] only for writes.
    pub fn choices(&self) -> Vec<Choice> {
        let Some(request) = self.current() else {
            return Vec::new();
        };
        let mut choices = vec![Choice::Once, Choice::Session, Choice::Project];
        if request.effect == ToolEffect::Write {
            choices.push(Choice::AllEdits);
        }
        choices.push(Choice::Reject);
        choices
    }

    /// The row under the cursor.
    pub fn selected(&self) -> Option<Choice> {
        self.choices().get(self.cursor).copied()
    }

    /// The pattern the session and project rows grant, as a session rule.
    pub fn pattern(&self) -> Option<&RuleOption> {
        self.ladder.get(self.rung)
    }

    /// Whether the pattern holds only the exact command because shells and
    /// interpreters are never widened.
    pub fn is_exact_only(&self) -> bool {
        self.exact_only
    }

    /// Moves the pattern one rung broader (`Tab`), if there is one.
    pub fn widen(&mut self, cx: &mut Context<Self>) {
        if self.rung + 1 < self.ladder.len() {
            self.rung += 1;
            cx.notify();
        }
    }

    /// Moves the pattern one rung narrower (`Shift+Tab`), if there is one.
    pub fn narrow(&mut self, cx: &mut Context<Self>) {
        if self.rung > 0 {
            self.rung -= 1;
            cx.notify();
        }
    }

    fn move_cursor(&mut self, down: bool) {
        let last = self.choices().len().saturating_sub(1);
        self.cursor = if down {
            (self.cursor + 1).min(last)
        } else {
            self.cursor.saturating_sub(1)
        };
    }

    fn push(&mut self, mut pending: PendingApproval, window: &mut Window, cx: &mut Context<Self>) {
        if pending.is_cancelled() {
            return;
        }
        let key = self.next_key;
        self.next_key += 1;
        let cancellation = pending.cancellation();
        let cancel = cx.spawn_in(window, async move |this, cx| {
            cancellation.await;
            this.update_in(cx, |this, window, cx| this.cancel(key, window, cx))
                .ok();
        });
        let shown = Shown {
            key,
            pending,
            _cancel: cancel,
        };
        if self.current.is_some() {
            self.queue.push_back(shown);
        } else {
            self.restore_focus = self.shown.then(|| window.focused(cx)).flatten();
            self.show(shown, window, cx);
        }
        cx.notify();
    }

    fn show(&mut self, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        let action = shown.pending.request().action();
        self.ladder = rule_ladder(&action);
        self.rung = default_rung(&self.ladder);
        self.exact_only = action.tool == RUN_COMMAND_TOOL && self.ladder.len() == 1 && {
            let argv = action
                .argv
                .clone()
                .or_else(|| split_command(&action.subject).ok())
                .unwrap_or_default();
            !argv.is_empty() && is_exact_only(&argv)
        };
        self.cursor = 0;
        self.current = Some(shown);
        self.reason = None;
        if self.shown {
            window.focus(&self.focus_handle, cx);
        }
    }

    /// Shows the next queued request, or closes and gives focus back.
    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.current = None;
        self.reason = None;
        self.ladder.clear();
        match self.queue.pop_front() {
            Some(next) => self.show(next, window, cx),
            None => {
                if let Some(handle) = self.restore_focus.take() {
                    window.focus(&handle, cx);
                }
            }
        }
        cx.notify();
    }

    /// Answers the request on screen. Does nothing when nothing is open.
    pub fn answer(
        &mut self,
        response: ApprovalResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shown) = self.current.take() else {
            return;
        };
        let id = shown.pending.request().id;
        if shown.pending.respond(response.clone()) {
            cx.emit(ApprovalEvent::Answered { id, response });
        } else {
            cx.emit(ApprovalEvent::Cancelled(id));
        }
        self.advance(window, cx);
    }

    /// Answers with `choice`, as its row does; [`Choice::Reject`] opens the
    /// reason line. A choice the request does not offer does nothing.
    pub fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        if !self.choices().contains(&choice) {
            return;
        }
        match choice {
            Choice::Once => self.approve(window, cx),
            Choice::Session => self.approve_pattern(RuleScope::Session, window, cx),
            Choice::Project => self.approve_pattern(RuleScope::Project, window, cx),
            Choice::AllEdits => self.answer(ApprovalResponse::ApproveAllEdits, window, cx),
            Choice::Reject => self.begin_reason(window, cx),
        }
    }

    /// Approves the request on screen once.
    pub fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.answer(ApprovalResponse::Approve, window, cx);
    }

    /// Approves the request on screen and remembers the chosen pattern in
    /// `scope`.
    fn approve_pattern(&mut self, scope: RuleScope, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rung) = self.pattern() else {
            return;
        };
        let rule = rung.rule.clone().with_scope(scope);
        self.answer(ApprovalResponse::ApproveAlways { rule }, window, cx);
    }

    /// Rejects the request on screen; an empty reason counts as none.
    pub fn reject(&mut self, reason: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let reason = reason
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        self.answer(ApprovalResponse::Deny { reason }, window, cx);
    }

    fn cancel(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.current.as_ref().is_some_and(|shown| shown.key == key) {
            if let Some(shown) = self.current.take() {
                cx.emit(ApprovalEvent::Cancelled(shown.pending.request().id));
            }
            self.advance(window, cx);
        } else if let Some(ix) = self.queue.iter().position(|shown| shown.key == key) {
            if let Some(shown) = self.queue.remove(ix) {
                cx.emit(ApprovalEvent::Cancelled(shown.pending.request().id));
            }
            cx.notify();
        }
    }

    /// Opens the reason line and keeps the keyboard on this dialog.
    ///
    /// The reason is not a text field: keystrokes land here only while this
    /// view is focused. The rows do not take focus when clicked, so without
    /// this the prompt keeps the keys.
    fn begin_reason(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reason = Some(String::new());
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_keystroke(&event.keystroke, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Applies one key while a request is on screen. Returns whether the
    /// dialog took it. Chords (`Ctrl`, `Alt`, `Cmd`) are left to the
    /// session: `Ctrl+C` interrupts.
    pub fn handle_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.current.is_none() {
            return false;
        }
        let Some(key) = key_from_keystroke(keystroke) else {
            return false;
        };
        if key.mods.ctrl || key.mods.alt || key.mods.cmd {
            return false;
        }
        let handled = match self.reason.as_mut() {
            Some(reason) => match key.code {
                KeyCode::Enter => {
                    let reason = self.reason.take();
                    self.reject(reason, window, cx);
                    true
                }
                KeyCode::Esc => {
                    self.reason = None;
                    true
                }
                KeyCode::Backspace => {
                    reason.pop();
                    true
                }
                KeyCode::Char(c) => {
                    reason.push(c);
                    true
                }
                _ => false,
            },
            None => match key.code {
                KeyCode::Up | KeyCode::Down if key.mods.shift => {
                    crate::keyscroll::scroll_key(&self.preview_scroll, &keystroke.key)
                }
                KeyCode::Up | KeyCode::Down => {
                    self.move_cursor(key.code == KeyCode::Down);
                    true
                }
                KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                    crate::keyscroll::scroll_key(&self.preview_scroll, &keystroke.key)
                }
                KeyCode::Tab => {
                    if key.mods.shift {
                        self.narrow(cx);
                    } else {
                        self.widen(cx);
                    }
                    true
                }
                KeyCode::Enter => {
                    if let Some(choice) = self.selected() {
                        self.choose(choice, window, cx);
                    }
                    true
                }
                KeyCode::Esc => {
                    self.reject(None, window, cx);
                    true
                }
                KeyCode::Char(c) => match Choice::from_key(c) {
                    Some(choice) if self.choices().contains(&choice) => {
                        self.choose(choice, window, cx);
                        true
                    }
                    _ => false,
                },
                _ => false,
            },
        };
        if handled {
            cx.notify();
        }
        handled
    }
}

impl Focusable for ApprovalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests;
