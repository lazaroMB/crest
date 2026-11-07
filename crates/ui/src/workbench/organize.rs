//! What the window does for the menus of the sidebar: pin, move, archive,
//! branch and delete chats and projects, add directories, and keep the open
//! chat in a sane state while the store changes under it.
//!
//! The store's changes run in the binary ([`ProjectOpener::organize`]); the
//! window then refreshes the list. A chat or a project that is open and
//! goes away (archived, deleted) leaves the window on another chat or a new
//! one; a chat that changes directories (moved, or its project gained or
//! lost one) is opened again, so its tools reach the right ones.

use std::path::PathBuf;

use gpui::{Context, Window};

use super::{Content, Workbench};
use crate::chats::{ChatOp, Organize, Organized, ProjectOp, SessionOpen};

/// What the open chat needs after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Follow {
    /// Nothing.
    Nothing,
    /// The open chat is gone: open `fallback` (directory, id), or a new
    /// chat where it was.
    LeaveChat(Option<(String, String)>),
    /// The open chat's project is gone: a new chat with no project.
    LeaveProject,
    /// Open the same chat again, to pick up its directories.
    Reopen(String),
}

/// A change that ran: what was asked, what the store said, and what the open
/// chat needs.
struct Ran {
    change: Organize,
    result: Result<Organized, String>,
    next: Follow,
}

/// What the open chat (`open`, in `project`) needs after `change`.
fn follow(
    change: &Organize,
    open: Option<&str>,
    project: Option<&str>,
    fallback: Option<(String, String)>,
) -> Follow {
    match change {
        Organize::Chat { id, op } if Some(id.as_str()) == open => match op {
            ChatOp::Archive(true) => Follow::LeaveChat(fallback),
            ChatOp::Move(_) | ChatOp::Promote(_) => Follow::Reopen(id.clone()),
            ChatOp::Pin(_) | ChatOp::Archive(false) | ChatOp::Branch => Follow::Nothing,
        },
        Organize::Project { id, op } if Some(id.as_str()) == project => match (op, open) {
            (ProjectOp::Archive(true), _) => Follow::LeaveProject,
            (
                ProjectOp::Delete | ProjectOp::AddDirectory(_) | ProjectOp::RemoveDirectory(_),
                Some(chat),
            ) => Follow::Reopen(chat.to_owned()),
            _ => Follow::Nothing,
        },
        _ => Follow::Nothing,
    }
}

impl Workbench {
    /// The open chat's id and project, while a session shows.
    fn open_ids(&self) -> Option<(Option<String>, Option<String>)> {
        match &self.content {
            Content::Session(live) => Some((live.session_id.clone(), live.project_id.clone())),
            _ => None,
        }
    }

    /// Runs `change` in the store, refreshes the list and then does what the
    /// open chat needs. A refusal shows as a warning with the store's own
    /// message.
    pub(super) fn organize(
        &mut self,
        change: Organize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((open, project)) = self.open_ids() else {
            return;
        };
        let fallback = open
            .as_deref()
            .and_then(|id| self.chats.read(cx).fallback_for(id));
        let next = follow(&change, open.as_deref(), project.as_deref(), fallback);
        let running = self.opener.organize(change.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = running.await;
            this.update_in(cx, |this, window, cx| {
                let ran = Ran {
                    change,
                    result,
                    next,
                };
                this.organized(ran, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn organized(&mut self, ran: Ran, window: &mut Window, cx: &mut Context<Self>) {
        let Ran {
            change,
            result,
            next,
        } = ran;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.notify(format!("could not {}: {err}", change.doing()), cx);
                self.reload_chats(cx);
                return;
            }
        };
        if let Some(message) = change.done(&outcome) {
            self.inform(message, cx);
        }
        self.reload_chats(cx);
        match (outcome, next) {
            (Organized::Branch { id, root }, _) => {
                self.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
            }
            (_, Follow::LeaveChat(Some((root, id)))) => {
                self.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
            }
            (_, Follow::LeaveChat(None)) => self.new_chat_here(window, cx),
            (_, Follow::LeaveProject) => {
                self.switch_chat(PathBuf::new(), SessionOpen::NewStandalone, window, cx);
            }
            (_, Follow::Reopen(id)) => self.reopen(id, window, cx),
            (_, Follow::Nothing) => {}
        }
    }

    /// Opens the chat `id` again where the list now has it. A chat with a
    /// turn running keeps going; it picks up the change when opened again.
    fn reopen(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .session()
            .is_some_and(|session| session.read(cx).is_busy())
        {
            self.inform(
                "this chat is running; open it again once it stops to use its new directories"
                    .into(),
                cx,
            );
            return;
        }
        let opener = self.opener.catalog();
        cx.spawn_in(window, async move |this, cx| {
            let catalog = opener.await;
            this.update_in(cx, |this, window, cx| {
                this.chats
                    .update(cx, |chats, cx| chats.set_catalog(catalog, cx));
                if this.open_id().as_deref() != Some(id.as_str()) {
                    return;
                }
                let root = this.chats.read(cx).root_of(&id).unwrap_or_default();
                this.switch_chat(PathBuf::from(root), SessionOpen::Id(id), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// `chat.branch`, or the menu's Branch chat on the open chat.
    pub(super) fn branch_open_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.chats.read(cx).current_chat() {
            Some(id) => self.organize(
                Organize::Chat {
                    id,
                    op: ChatOp::Branch,
                },
                window,
                cx,
            ),
            None => self.inform(
                "send a message first: a new chat has nothing to branch".into(),
                cx,
            ),
        }
    }

    /// Puts the branch banner of the open chat (if it is a branch) on its
    /// timeline.
    pub(super) fn sync_banner(&mut self, cx: &mut Context<Self>) {
        let banner = self
            .open_id()
            .and_then(|id| self.chats.read(cx).branch_of(&id))
            .map(|branch| branch.banner());
        let Some(session) = self.session() else {
            return;
        };
        let timeline = session.read(cx).timeline().clone();
        timeline.update(cx, |timeline, cx| {
            if timeline.banner.set(banner) {
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, op: ChatOp) -> Organize {
        Organize::Chat { id: id.into(), op }
    }

    fn project(id: &str, op: ProjectOp) -> Organize {
        Organize::Project { id: id.into(), op }
    }

    #[test]
    fn the_open_chat_follows_what_happens_to_it_or_to_its_project() {
        let other = || Some(("/w".to_owned(), "b".to_owned()));
        let cases = [
            (chat("a", ChatOp::Archive(true)), Follow::LeaveChat(other())),
            (chat("a", ChatOp::Move(None)), Follow::Reopen("a".into())),
            (
                chat("a", ChatOp::Promote("/x".into())),
                Follow::Reopen("a".into()),
            ),
            (chat("a", ChatOp::Pin(true)), Follow::Nothing),
            (chat("a", ChatOp::Branch), Follow::Nothing),
            (chat("b", ChatOp::Archive(true)), Follow::Nothing),
            (project("p", ProjectOp::Archive(true)), Follow::LeaveProject),
            (project("p", ProjectOp::Delete), Follow::Reopen("a".into())),
            (
                project("p", ProjectOp::AddDirectory("/y".into())),
                Follow::Reopen("a".into()),
            ),
            (project("p", ProjectOp::Rename("n".into())), Follow::Nothing),
            (project("q", ProjectOp::Delete), Follow::Nothing),
        ];
        for (change, want) in cases {
            let got = follow(&change, Some("a"), Some("p"), other());
            assert_eq!(got, want, "{change:?}");
        }
    }

    #[test]
    fn what_was_done_is_told_but_a_pin_or_a_name_is_just_shown() {
        let cases = [
            (
                chat("a", ChatOp::Archive(true)),
                Organized::Done,
                Some("chat archived"),
            ),
            (
                chat("a", ChatOp::Archive(false)),
                Organized::Done,
                Some("chat restored"),
            ),
            (
                chat("a", ChatOp::Move(None)),
                Organized::Done,
                Some("now a standalone chat"),
            ),
            (chat("a", ChatOp::Pin(true)), Organized::Done, None),
            (
                project("p", ProjectOp::Rename("n".into())),
                Organized::Done,
                None,
            ),
            (
                project("p", ProjectOp::Archive(true)),
                Organized::Done,
                Some("project archived"),
            ),
            (
                project("p", ProjectOp::Delete),
                Organized::Freed(1),
                Some("project deleted; 1 chat is standalone now"),
            ),
            (
                project("p", ProjectOp::Delete),
                Organized::Freed(3),
                Some("project deleted; 3 chats are standalone now"),
            ),
        ];
        for (change, outcome, want) in cases {
            assert_eq!(change.done(&outcome).as_deref(), want, "{change:?}");
        }
    }
}
