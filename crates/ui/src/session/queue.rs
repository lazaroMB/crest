//! Prompts and turns: sending a prompt, queueing it while the session is
//! busy, and what follows once a turn ends or is interrupted.

use application_core::ports::events::{DomainEvent, NoticeLevel};
use application_core::usecases::cancel::CancelToken;
use domain::block::{Block, BlockKind};
use domain::conversation::{ImageAttachment, StopReason};
use domain::model::ModelRole;
use gpui::{Context, Window};

use super::{Session, SessionEvent, TurnEnd, TurnRequest};
use crate::activity::WorkKind;

/// A prompt waiting for the session to be free.
#[derive(Debug, Clone)]
pub(super) struct Queued {
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

impl Session {
    /// Whether nothing runs; otherwise says that `command` must wait.
    pub(super) fn idle_for(&mut self, command: &str, cx: &mut Context<Self>) -> bool {
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
    pub(super) fn resume(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn drain_queue(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy()
            && let Some(next) = self.queue.pop_front()
        {
            self.submit_with(next.prompt, next.role, next.images, cx);
        }
        self.sync_activity(cx);
    }

    /// Shows the current work and the queue in the activity line, and tells
    /// the input whether `Up` takes a queued prompt back.
    pub(super) fn sync_activity(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn edit_queued(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
}
