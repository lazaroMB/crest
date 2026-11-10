//! What the window does when a menu row changes chats and projects, with the
//! real window and a scripted opener (which refuses the chat `boom`, opens
//! `branch-1` for a branch and frees two chats when a project goes).

use std::time::Instant;

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::tests::{draw, open_full, workbench};
use super::*;
use crate::chats::{ChatOp, ProjectOp};

fn ask(view: &Entity<Workbench>, event: ChatsEvent, cx: &mut VisualTestContext) {
    let chats = view.read_with(cx, |view, _| view.chats.clone());
    chats.update(cx, |_, cx| cx.emit(event));
    cx.run_until_parked();
}

fn open_id_of(view: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |view, _| view.open_id())
}

fn notice(view: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<String> {
    let session = view.read_with(cx, |view, _| view.session().cloned())?;
    session.read_with(cx, |session, cx| {
        let state = session.status().read(cx).state();
        state
            .notice(Instant::now())
            .map(|notice| notice.message.clone())
    })
}

fn chat(id: &str, op: ChatOp) -> ChatsEvent {
    ChatsEvent::Chat { id: id.into(), op }
}

/// The open chat after each change to it or to its project, by case.
#[gpui::test]
fn the_open_chat_stays_sane_when_it_is_archived_branched_or_its_project_goes(
    cx: &mut TestAppContext,
) {
    // (what is asked, the chat open before, the chat open after; `None`
    // after means "a new chat, not `before`")
    let cases = [
        (
            "a pin changes nothing",
            chat("latest", ChatOp::Pin(true)),
            Some("latest"),
        ),
        (
            "archiving the open chat opens its neighbour",
            chat("latest", ChatOp::Archive(true)),
            Some("second"),
        ),
        (
            "archiving another chat changes nothing",
            chat("second", ChatOp::Archive(true)),
            Some("latest"),
        ),
        (
            "a branch opens",
            chat("latest", ChatOp::Branch),
            Some("branch-1"),
        ),
        (
            "moving the open chat opens it again where it now is",
            chat("latest", ChatOp::Move(None)),
            Some("latest"),
        ),
    ];
    for (name, event, after) in cases {
        let (view, cx) = workbench(cx);
        open_full(&view, "/home/u/proj", cx);
        assert_eq!(open_id_of(&view, cx).as_deref(), Some("latest"), "{name}");
        ask(&view, event, cx);
        assert_eq!(open_id_of(&view, cx).as_deref(), after, "{name}");
    }
}

#[gpui::test]
fn a_refusal_shows_the_stores_message_and_keeps_the_chat(cx: &mut TestAppContext) {
    let (view, cx) = workbench(cx);
    open_full(&view, "/home/u/proj", cx);
    ask(&view, chat("boom", ChatOp::Archive(true)), cx);
    assert_eq!(
        notice(&view, cx).as_deref(),
        Some("could not archive the chat: the store says no")
    );
    assert_eq!(open_id_of(&view, cx).as_deref(), Some("latest"));
}

#[gpui::test]
fn archiving_or_deleting_the_open_chats_project_leaves_it(cx: &mut TestAppContext) {
    let in_project = |name: &str, op: ProjectOp| {
        (
            name.to_owned(),
            ChatsEvent::Project {
                id: "p1".into(),
                op,
            },
        )
    };
    let cases = [
        in_project("archive", ProjectOp::Archive(true)),
        in_project("delete", ProjectOp::Delete),
    ];
    for (name, event) in cases {
        let (view, cx) = workbench(cx);
        open_full(&view, "/home/u/proj", cx);
        // A chat of the project `p1`.
        ask(
            &view,
            ChatsEvent::NewIn {
                project: "p1".into(),
                root: "/home/u/proj".into(),
            },
            cx,
        );
        let before = open_id_of(&view, cx);
        ask(&view, event, cx);
        let after = open_id_of(&view, cx);
        match name.as_str() {
            // Out of the archived project: a new chat with no project.
            "archive" => assert_ne!(after, before, "{name}"),
            // Its chats are free: the same chat opens again with no project.
            _ => assert_eq!(after, before, "{name}"),
        }
        let project = view.read_with(cx, |view, _| match &view.content {
            Content::Session(live) => live.project_id.clone(),
            _ => panic!("no session"),
        });
        assert_eq!(project, None, "{name}: no project now");
    }
}

#[gpui::test]
fn a_menu_row_that_needs_a_directory_shows_the_picker_and_comes_back(cx: &mut TestAppContext) {
    let (view, cx) = workbench(cx);
    open_full(&view, "/home/u/proj", cx);
    let pick = ChatsEvent::Pick(PickFor::AddDirectory {
        project: "p1".into(),
    });
    // Esc or Back: the session returns untouched.
    ask(&view, pick.clone(), cx);
    assert!(view.read_with(cx, |view, _| view.project_picker().is_some()));
    view.update_in(cx, |view, window, cx| view.go_back(window, cx));
    cx.run_until_parked();
    assert_eq!(open_id_of(&view, cx).as_deref(), Some("latest"));
    assert!(view.read_with(cx, |view, _| view.pending_pick.is_none()));
    // A directory: the row runs and the session returns.
    ask(&view, pick, cx);
    view.update_in(cx, |view, window, cx| {
        view.chosen(PathBuf::from("/home/u/lib"), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(open_id_of(&view, cx).as_deref(), Some("latest"));
    assert!(view.read_with(cx, |view, _| view.project_picker().is_none()));
}

/// A branch says where it came from above its blocks; a chat that is not
/// one has no banner, and it follows the chat that is open.
#[gpui::test]
fn the_open_branch_shows_its_banner_and_the_other_chats_do_not(cx: &mut TestAppContext) {
    let (view, cx) = workbench(cx);
    open_full(&view, "/home/u/proj", cx);
    let banner = |cx: &mut VisualTestContext| {
        draw(&view, cx);
        cx.debug_bounds("timeline-branch").is_some()
    };
    assert!(!banner(cx), "the latest chat is not a branch");
    ask(
        &view,
        ChatsEvent::Open {
            root: "/home/u/proj".into(),
            id: "second".into(),
        },
        cx,
    );
    assert_eq!(open_id_of(&view, cx).as_deref(), Some("second"));
    assert!(banner(cx), "second is a branch");
    ask(
        &view,
        ChatsEvent::Open {
            root: "/home/u/proj".into(),
            id: "latest".into(),
        },
        cx,
    );
    assert!(!banner(cx));
}
