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

use application::keymap::KeyCode;
use application::ports::approval::{
    ApprovalError, ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalRequestId,
    ApprovalResponse,
};
use async_trait::async_trait;
use domain::permission::{PermissionRule, RuleDecision, RuleMatcher};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::input::key_from_keystroke;

/// The "allow always" rule for `request`: same tool, exactly this subject.
pub fn allow_always_rule(request: &ApprovalRequest) -> PermissionRule {
    PermissionRule::new(
        request.tool.clone(),
        RuleMatcher::Exact(request.subject.clone()),
        RuleDecision::AllowAlways,
    )
}

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

/// One request waiting for the user, as received by the dialog.
#[derive(Debug)]
pub struct PendingApproval {
    request: ApprovalRequest,
    reply: oneshot::Sender<ApprovalResponse>,
    cancelled: Option<oneshot::Receiver<()>>,
}

impl PendingApproval {
    /// The request to show.
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }

    /// Whether the requester stopped waiting (its future was dropped).
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_canceled()
    }

    /// Sends the answer. Returns `false` when nobody is waiting any more.
    pub fn respond(self, response: ApprovalResponse) -> bool {
        self.reply.send(response).is_ok()
    }

    /// A future that resolves when the requester stops waiting, either
    /// because it was answered or because its future was dropped. Only the
    /// first call gets the signal; later ones never resolve.
    pub fn cancellation(&mut self) -> impl Future<Output = ()> + Send + 'static {
        let signal = self.cancelled.take();
        async move {
            match signal {
                // The sender is never used: the only outcome is `Canceled`.
                Some(signal) => {
                    let _ = signal.await;
                }
                None => futures::future::pending::<()>().await,
            }
        }
    }
}

/// The receiving end of [`UiApproval`], consumed by the dialog view.
#[derive(Debug)]
pub struct ApprovalInbox {
    rx: mpsc::UnboundedReceiver<PendingApproval>,
}

impl ApprovalInbox {
    /// The next request, or `None` when every [`UiApproval`] is gone.
    pub async fn next(&mut self) -> Option<PendingApproval> {
        self.rx.next().await
    }
}

/// [`ApprovalPort`] answered by the UI. Cheap to clone; every clone feeds the
/// same [`ApprovalInbox`].
#[derive(Debug, Clone)]
pub struct UiApproval {
    tx: mpsc::UnboundedSender<PendingApproval>,
}

impl UiApproval {
    /// A port and the inbox the dialog reads its requests from.
    pub fn channel() -> (Self, ApprovalInbox) {
        let (tx, rx) = mpsc::unbounded();
        (Self { tx }, ApprovalInbox { rx })
    }
}

#[async_trait]
impl ApprovalPort for UiApproval {
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        let (reply, answer) = oneshot::channel();
        // Held until this future ends; dropping it signals cancellation.
        let (_alive, cancelled) = oneshot::channel::<()>();
        let pending = PendingApproval {
            request: req,
            reply,
            cancelled: Some(cancelled),
        };
        self.tx
            .unbounded_send(pending)
            .map_err(|_| ApprovalError::Unavailable("the approval dialog is closed".into()))?;
        answer.await.map_err(|_| ApprovalError::Cancelled)
    }
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
/// Keys while open: `y` approve, `a` approve always, `n` reject with an
/// optional reason (typed, then `Enter`; `Esc` goes back), `Esc` reject;
/// `Up`/`Down`, `PageUp`/`PageDown` and `Home`/`End` scroll the preview.
/// Requests that arrive while one is open wait in a queue.
pub struct ApprovalView {
    focus_handle: FocusHandle,
    current: Option<Shown>,
    queue: VecDeque<Shown>,
    /// The reason being typed after `n`, if any.
    reason: Option<String>,
    /// Scroll of the preview (`Up`/`Down`, `PageUp`/`PageDown`).
    preview_scroll: ScrollHandle,
    restore_focus: Option<FocusHandle>,
    next_key: u64,
    /// Whether its session is on screen. A parked session's dialog keeps
    /// its queue but never takes the focus.
    shown: bool,
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
            preview_scroll: ScrollHandle::new(),
            restore_focus: None,
            next_key: 0,
            shown: true,
            _inbox: task,
        }
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

    /// Approves the request on screen once.
    pub fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.answer(ApprovalResponse::Approve, window, cx);
    }

    /// Approves the request on screen and remembers the rule.
    pub fn approve_always(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.current() else {
            return;
        };
        let rule = allow_always_rule(request);
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
    /// view is focused. The Reject button does not take focus, and neither
    /// does the line that replaces it, so without this the prompt keeps the
    /// keys.
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
    /// dialog took it. `Ctrl+C` is left to the session, which interrupts.
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
            None if crate::keyscroll::scroll_key(&self.preview_scroll, &keystroke.key) => true,
            None => match key.code {
                KeyCode::Char('y') => {
                    self.approve(window, cx);
                    true
                }
                KeyCode::Char('a') => {
                    self.approve_always(window, cx);
                    true
                }
                KeyCode::Char('n') => {
                    self.begin_reason(window, cx);
                    true
                }
                KeyCode::Esc => {
                    self.reject(None, window, cx);
                    true
                }
                _ => false,
            },
        };
        if handled {
            cx.notify();
        }
        handled
    }

    fn render_preview(preview: &ApprovalPreview, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match preview {
            ApprovalPreview::Diff(spec) => v_flex()
                .gap_1()
                .child(crate::blocks::diff::render_header(spec, cx))
                .child(crate::blocks::diff::render_unified(&spec.unified, cx))
                .into_any_element(),
            ApprovalPreview::Command { command, args, cwd } => v_flex()
                .gap_1()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("in {cwd}"))),
                )
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .p_2()
                        .bg(theme.secondary)
                        .child(SharedString::from(format!(
                            "$ {}",
                            command_line(command, args)
                        ))),
                )
                .into_any_element(),
            ApprovalPreview::Text { text } => div()
                .child(SharedString::from(text.clone()))
                .into_any_element(),
        }
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(reason) = &self.reason {
            let theme = cx.theme();
            return h_flex()
                .gap_2()
                .child(div().text_color(theme.muted_foreground).child("Reason:"))
                .child(
                    div()
                        .flex_1()
                        .px_1()
                        .border_1()
                        .border_color(theme.border)
                        .child(SharedString::from(format!("{reason}▏"))),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Enter reject · Esc back"),
                )
                .into_any_element();
        }
        h_flex()
            .gap_2()
            .justify_end()
            .child(
                div()
                    .debug_selector(|| "approval-keys".to_owned())
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Esc rejects without a reason"),
            )
            .child(
                Button::new("approval-approve")
                    .primary()
                    .label("Approve (y)")
                    .on_click(cx.listener(|this, _, window, cx| this.approve(window, cx))),
            )
            .child(
                Button::new("approval-always")
                    .label("Always (a)")
                    .on_click(cx.listener(|this, _, window, cx| this.approve_always(window, cx))),
            )
            .child(
                Button::new("approval-reject")
                    .danger()
                    .label("Reject (n)")
                    .on_click(cx.listener(|this, _, window, cx| this.begin_reason(window, cx))),
            )
            .into_any_element()
    }
}

impl Focusable for ApprovalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ApprovalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = &self.current else {
            return div().id("approval").into_any_element();
        };
        let request = shown.pending.request();
        let theme = cx.theme();
        let title = format!("Allow {}?", request.tool);
        let mut card = v_flex()
            .w(px(760.))
            .max_w_full()
            .gap_2()
            .p_4()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(request.subject.clone())),
                    ),
            )
            .child(
                crate::keyscroll::list(
                    "approval-preview",
                    &self.preview_scroll,
                    [Self::render_preview(&request.preview, cx)],
                )
                .max_h(px(420.)),
            );
        if !self.queue.is_empty() {
            card = card.child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!(
                        "{} more pending",
                        self.queue.len()
                    ))),
            );
        }
        let footer = self.render_footer(cx);
        div()
            .id("approval")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().overlay)
            .child(card.child(footer))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use application::ports::approval::{ApprovalPreview, ApprovalRequestId};
    use domain::conversation::ToolUseId;
    use domain::permission::ToolEffect;
    use futures::FutureExt as _;
    use futures::executor::block_on;

    use super::*;

    pub(super) fn command_request(id: u64) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test".into(),
            preview: ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
            },
        }
    }

    #[test]
    fn reply_reaches_the_requester() {
        let (port, mut inbox) = UiApproval::channel();
        let mut request = port.request(command_request(1)).boxed();
        assert!(
            request.as_mut().now_or_never().is_none(),
            "waits for the UI"
        );
        let pending = block_on(inbox.next()).unwrap();
        assert_eq!(pending.request().id, ApprovalRequestId(1));
        assert!(pending.respond(ApprovalResponse::Approve));
        assert_eq!(block_on(request), Ok(ApprovalResponse::Approve));
    }

    #[test]
    fn closed_inbox_is_unavailable() {
        let (port, inbox) = UiApproval::channel();
        drop(inbox);
        let result = block_on(port.request(command_request(1)));
        assert!(matches!(result, Err(ApprovalError::Unavailable(_))));
    }

    #[test]
    fn dropped_dialog_cancels_the_request() {
        let (port, mut inbox) = UiApproval::channel();
        let request = port.request(command_request(1));
        let (result, ()) = block_on(futures::future::join(request, async {
            drop(inbox.next().await);
        }));
        assert_eq!(result, Err(ApprovalError::Cancelled));
    }

    #[test]
    fn dropped_request_signals_the_dialog() {
        let (port, mut inbox) = UiApproval::channel();
        let mut request = port.request(command_request(1)).boxed();
        assert!(request.as_mut().now_or_never().is_none());
        let mut pending = block_on(inbox.next()).unwrap();
        let cancellation = pending.cancellation();
        assert!(!pending.is_cancelled());
        drop(request);
        assert!(pending.is_cancelled());
        block_on(cancellation);
        assert!(!pending.respond(ApprovalResponse::Approve));
    }

    #[test]
    fn allow_always_matches_exactly_the_subject() {
        let rule = allow_always_rule(&command_request(1));
        assert_eq!(rule.tool, "run_command");
        assert_eq!(rule.matcher, RuleMatcher::Exact("cargo test".into()));
        assert_eq!(rule.decision, RuleDecision::AllowAlways);
    }

    #[test]
    fn command_line_quotes_only_when_needed() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            command_line("cargo", &args(&["test", "-p", "ui"])),
            "cargo test -p ui"
        );
        assert_eq!(
            command_line("git", &args(&["commit", "-m", "it's done", ""])),
            r"git commit -m 'it'\''s done' ''"
        );
    }

    // -- dialog -------------------------------------------------------------

    use gpui::{Entity, Task, TestAppContext, VisualTestContext};

    type Answer = Task<Result<ApprovalResponse, ApprovalError>>;

    fn dialog(
        cx: &mut TestAppContext,
    ) -> (UiApproval, Entity<ApprovalView>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let (port, inbox) = UiApproval::channel();
        let (view, cx) = cx.add_window_view(|window, cx| ApprovalView::new(inbox, window, cx));
        (port, view, cx)
    }

    fn ask(port: &UiApproval, req: ApprovalRequest, cx: &mut VisualTestContext) -> Answer {
        let port = port.clone();
        let task = cx.executor().spawn(async move { port.request(req).await });
        cx.run_until_parked();
        task
    }

    fn answered(
        task: Answer,
        cx: &mut VisualTestContext,
    ) -> Result<ApprovalResponse, ApprovalError> {
        cx.run_until_parked();
        task.now_or_never().expect("the request was answered")
    }

    fn diff_request(id: u64) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "edit_file".into(),
            effect: ToolEffect::Write,
            subject: "src/lib.rs".into(),
            preview: ApprovalPreview::Diff(domain::block::DiffSpec {
                path: "src/lib.rs".into(),
                old_text: Some("a\n".into()),
                new_text: "b\n".into(),
                unified: "@@ -1 +1 @@\n-a\n+b\n".into(),
                approval: Some(domain::block::ApprovalState::Pending),
            }),
        }
    }

    #[gpui::test]
    fn y_approves(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let answer = ask(&port, diff_request(1), cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(1)));
        });
        cx.simulate_keystrokes("y");
        assert_eq!(answered(answer, cx), Ok(ApprovalResponse::Approve));
        view.read_with(cx, |view, _| assert!(view.current().is_none()));
    }

    #[gpui::test]
    fn a_approves_always_with_the_exact_rule(cx: &mut TestAppContext) {
        let (port, _view, cx) = dialog(cx);
        let answer = ask(&port, command_request(1), cx);
        cx.simulate_keystrokes("a");
        assert_eq!(
            answered(answer, cx),
            Ok(ApprovalResponse::ApproveAlways {
                rule: allow_always_rule(&command_request(1)),
            })
        );
    }

    #[gpui::test]
    fn n_rejects_with_a_typed_reason(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let answer = ask(&port, command_request(1), cx);
        cx.simulate_keystrokes("n u s e space x x backspace enter");
        view.read_with(cx, |view, _| assert!(view.reason().is_none()));
        assert_eq!(
            answered(answer, cx),
            Ok(ApprovalResponse::Deny {
                reason: Some("use x".into()),
            })
        );
    }

    #[gpui::test]
    fn a_reason_can_be_typed_after_the_prompt_had_focus(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let answer = ask(&port, command_request(1), cx);
        cx.update(|window, cx| {
            let prompt = cx.focus_handle();
            window.focus(&prompt, cx);
        });
        view.update_in(cx, |view, window, cx| view.begin_reason(window, cx));
        cx.simulate_keystrokes("u s e escape");
        view.read_with(cx, |view, _| {
            assert!(view.reason().is_none(), "escape leaves the reason line");
            assert!(view.is_open(), "the dialog stays open");
        });
        cx.simulate_keystrokes("n n o enter");
        assert_eq!(
            answered(answer, cx),
            Ok(ApprovalResponse::Deny {
                reason: Some("no".into()),
            })
        );
    }

    #[gpui::test]
    fn n_then_enter_rejects_without_reason(cx: &mut TestAppContext) {
        let (port, _view, cx) = dialog(cx);
        let answer = ask(&port, command_request(1), cx);
        cx.simulate_keystrokes("n enter");
        assert_eq!(
            answered(answer, cx),
            Ok(ApprovalResponse::Deny { reason: None })
        );
    }

    #[gpui::test]
    fn escape_rejects_and_backs_out_of_the_reason(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let first = ask(&port, command_request(1), cx);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("approval-keys").is_some(),
            "the dialog says what Esc does"
        );
        cx.simulate_keystrokes("escape");
        assert_eq!(
            answered(first, cx),
            Ok(ApprovalResponse::Deny { reason: None })
        );

        let second = ask(&port, command_request(2), cx);
        cx.simulate_keystrokes("n x escape");
        view.read_with(cx, |view, _| {
            assert!(view.reason().is_none());
            assert!(view.current().is_some(), "still open");
        });
        cx.simulate_keystrokes("y");
        assert_eq!(answered(second, cx), Ok(ApprovalResponse::Approve));
    }

    #[gpui::test]
    fn queued_requests_are_shown_in_order(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let first = ask(&port, diff_request(1), cx);
        let second = ask(&port, command_request(2), cx);
        view.read_with(cx, |view, _| {
            assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(1)));
            assert_eq!(view.queued(), 1);
        });
        cx.simulate_keystrokes("y");
        view.read_with(cx, |view, _| {
            assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(2)));
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(answered(first, cx), Ok(ApprovalResponse::Approve));
        assert_eq!(
            answered(second, cx),
            Ok(ApprovalResponse::Deny { reason: None })
        );
    }

    #[gpui::test]
    fn dropping_the_request_closes_the_dialog(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ApprovalEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
            .detach();
        });
        let answer = ask(&port, diff_request(1), cx);
        view.read_with(cx, |view, _| assert!(view.current().is_some()));
        drop(answer);
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.current().is_none()));
        assert_eq!(
            *events.borrow(),
            [ApprovalEvent::Cancelled(ApprovalRequestId(1))]
        );
        // Keys no longer answer anything.
        cx.simulate_keystrokes("y");
        assert_eq!(events.borrow().len(), 1);
    }

    #[gpui::test]
    fn focus_returns_after_answering(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let before = cx.update(|window, cx| {
            let handle = cx.focus_handle();
            window.focus(&handle, cx);
            handle
        });
        let answer = ask(&port, command_request(1), cx);
        cx.update(|window, cx| {
            assert!(view.read(cx).focus_handle.is_focused(window));
        });
        cx.simulate_keystrokes("y");
        assert_eq!(answered(answer, cx), Ok(ApprovalResponse::Approve));
        cx.update(|window, _| assert!(before.is_focused(window)));
    }

    #[gpui::test]
    fn draws_every_preview(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let text = ApprovalRequest {
            preview: ApprovalPreview::Text {
                text: "fetch https://example.com".into(),
            },
            ..command_request(3)
        };
        let _answers = [
            ask(&port, diff_request(1), cx),
            ask(&port, command_request(2), cx),
            ask(&port, text, cx),
        ];
        for keys in ["", "y", "n"] {
            if !keys.is_empty() {
                cx.simulate_keystrokes(keys);
            }
            cx.draw(
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::size(gpui::px(1000.), gpui::px(700.)),
                |_, _| div().relative().size_full().child(view.clone()),
            );
        }
    }

    #[gpui::test]
    fn a_long_preview_scrolls_with_the_keys(cx: &mut TestAppContext) {
        let (port, view, cx) = dialog(cx);
        let text = (0..200)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let _answer = ask(
            &port,
            ApprovalRequest {
                preview: ApprovalPreview::Text { text },
                ..command_request(1)
            },
            cx,
        );
        let draw = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(view.preview_scroll.max_offset().y > px(0.), "it overflows");
        });
        cx.simulate_keystrokes("pagedown");
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(view.preview_scroll.offset().y < px(0.), "PageDown scrolls");
        });
    }

    #[gpui::test]
    fn diff_buttons_answer_only_their_diff(cx: &mut TestAppContext) {
        let (port, _view, cx) = dialog(cx);
        let answer = ask(&port, diff_request(1), cx);
        let other = cx.update(|window, cx| answer_diff("src/main.rs", true, window, cx));
        assert!(!other, "another path is not the open request");
        let done = cx.update(|window, cx| answer_diff("src/lib.rs", false, window, cx));
        assert!(done);
        assert_eq!(
            answered(answer, cx),
            Ok(ApprovalResponse::Deny { reason: None })
        );
        let idle = cx.update(|window, cx| answer_diff("src/lib.rs", true, window, cx));
        assert!(!idle, "nothing is open");
    }
}
