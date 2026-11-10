//! What the window does to the chats of the sidebar: start one where the
//! open chat is, rename one, delete one.

use std::path::PathBuf;
use std::sync::Arc;

use application_core::ports::events::NoticeLevel;
use gpui::{Context, Focusable as _, Window};

use super::{Content, Workbench};
use crate::chats::SessionOpen;

impl Workbench {
    /// A new chat where the open one is: in its project, in none when it has
    /// none and no directory, else in its directory.
    pub(super) fn new_chat_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Content::Session(live) = &self.content else {
            return;
        };
        let dir = live.dir.clone();
        match live.project_id.clone() {
            Some(project) => self.switch_chat(dir, SessionOpen::NewIn(project), window, cx),
            None if dir.as_os_str().is_empty() => {
                self.switch_chat(dir, SessionOpen::NewStandalone, window, cx);
            }
            None => self.switch_chat(dir, SessionOpen::New, window, cx),
        }
    }

    /// Saves `title` on the chat `id` and refreshes the list. The list
    /// already shows the title and the prompt has the focus back: the save
    /// of a chat with a turn running waits for the turn.
    pub(super) fn rename_chat(
        &mut self,
        id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session().cloned() else {
            return;
        };
        // The field is gone: a focus left nowhere (or in the list) goes
        // back to the prompt; a click elsewhere keeps its target.
        let lost = window.focused(cx).is_none()
            || self.chats.focus_handle(cx).contains_focused(window, cx);
        if lost {
            session.update(cx, |session, cx| session.focus_input(window, cx));
        }
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
        let renamed = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = if let Some(primary) = primary {
                match primary.await {
                    Ok(()) => Ok(()),
                    Err(_) => opener.rename_chat(id, title).await,
                }
            } else {
                opener.rename_chat(id, title).await
            };
            this.update(cx, |this, cx| this.finish_rename(&renamed, result, cx))
                .ok();
        })
        .detach();
    }

    pub(super) fn finish_rename(
        &mut self,
        id: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = &result {
            self.notify(format!("could not rename the chat: {err}"), cx);
            self.chats
                .update(cx, |chats, cx| chats.forget_title(id, cx));
        }
        self.reload_chats(cx);
    }

    /// Shows `message` as a warning in the shown session, if any.
    pub(super) fn notify(&self, message: String, cx: &mut Context<Self>) {
        if let Some(session) = self.session().cloned() {
            session.update(cx, |session, cx| {
                session.show_notice(NoticeLevel::Warning, message, cx);
            });
        }
    }

    /// Removes the chat `id`. The open one is left for another chat of
    /// `root`, or a new chat when it was the last.
    pub(super) fn delete_chat(
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
        let next = self.chats.read(cx).next_chat(&id);
        let delete = self.opener.delete_chat(id);
        cx.spawn_in(window, async move |this, cx| {
            let result = delete.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) if current => match next {
                    Some(id) => {
                        this.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx)
                    }
                    None => this.new_chat_here(window, cx),
                },
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
}
