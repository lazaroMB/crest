//! `/` commands, the replies of the controller and what they ask for.

use application_core::keymap::Action;
use application_core::ports::events::{DomainEvent, NoticeLevel};
use application_core::ports::plugin::PluginAction;
use application_core::usecases::commands::{
    Command, ModelCommand, ThemeCommand, parse, unknown_command_hint,
};
use domain::block::{BlockId, CustomNode, ImageData, ImageFit};
use domain::conversation::ImageAttachment;
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use domain::model::ModelId;
use futures::future::BoxFuture;
use gpui::{AppContext as _, Context, Window};

use super::{ModelChoice, Reply, Session};
use crate::timeline::TimelineView;

/// Share of the window an inspected image takes, in percent.
const INSPECT_PERCENT: f32 = 90.0;

/// How many times a command may lead to another one (`app.cmd` inside a
/// command), so a command calling itself stops.
const MAX_COMMAND_DEPTH: usize = 4;

impl Session {
    /// Runs a `/` command (without the slash).
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.command_at(command, 0, window, cx);
    }

    fn command_at(
        &mut self,
        command_text: &str,
        depth: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line = parse(command_text);
        match Command::from_line(line) {
            Command::Quit => self.quit(window, cx),
            Command::Model(model) => match model {
                ModelCommand::Popup => self.open_model_popup(window, cx),
                ModelCommand::Clear => self.set_model_override(None, cx),
                ModelCommand::Set(id) => {
                    let choice = ModelChoice {
                        model: ModelId::new(&id),
                        alias: id,
                    };
                    self.select_model(choice, window, cx);
                }
                ModelCommand::Override(model) => self.set_model_override(Some(model), cx),
                ModelCommand::Invalid(usage) => self.notice(NoticeLevel::Warning, usage, cx),
            },
            Command::Theme(theme) => match theme {
                ThemeCommand::Popup => self.open_theme_popup(window, cx),
                ThemeCommand::Save(name) => self.save_theme(name, window, cx),
                ThemeCommand::Invalid(usage) => self.notice(NoticeLevel::Warning, usage, cx),
            },
            Command::Mode(Ok(mode)) => self.set_permission_mode(mode, cx),
            Command::Mode(Err(err)) => {
                self.notice(NoticeLevel::Warning, format!("/mode: {err}"), cx);
            }
            Command::Clear => self.clear(window, cx),
            Command::Project => self.switch_project(window, cx),
            Command::Login(account) => self.open_login(&account, window, cx),
            Command::Logout(account) => self.logout(&account, window, cx),
            Command::Help(search) => self.open_help(&search, window, cx),
            Command::Permissions => self.open_permissions(window, cx),
            Command::ModalClose => {
                let closed = self
                    .modal
                    .update(cx, |modal, cx| modal.close_top(window, cx));
                if !closed {
                    self.notice(NoticeLevel::Info, "no modal is open", cx);
                }
            }
            Command::Modal(arg) => self.notice(
                NoticeLevel::Warning,
                format!("/modal {arg}: use /modal close"),
                cx,
            ),
            // `/picker.files` from the palette.
            Command::Other { name, .. }
                if crate::picker::PickerMode::for_action(&Action::Builtin(name.clone()))
                    .is_some() =>
            {
                self.action(&Action::Builtin(name), window, cx);
            }
            command => {
                let name = line.name.to_owned();
                if let Some(reply) = self.controller.run_command(command) {
                    self.commands.push(name.clone());
                    self.sync_activity(cx);
                    self.await_reply_then(reply, depth, Some(name), window, cx);
                } else if name.contains('.') {
                    self.action(&Action::Builtin(name), window, cx);
                } else {
                    self.notice(NoticeLevel::Warning, unknown_command_hint(command_text), cx);
                }
            }
        }
    }

    /// Runs `reply` once the controller answers.
    pub(super) fn await_reply(
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
    pub(super) fn switch_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn recompute(&mut self, id: BlockId, cx: &mut Context<Self>) {
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

    pub(super) fn inspect_image(
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

    pub(super) fn insert_reference(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.insert_text(text, window, cx));
    }
}
