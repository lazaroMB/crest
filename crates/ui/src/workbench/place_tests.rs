//! Where a new chat opens, with the real window and a scripted opener.

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::tests::{open_full, workbench};
use super::*;

/// Where the open chat is: its directory, its project and its id.
fn place(view: &Entity<Workbench>, cx: &mut VisualTestContext) -> (String, Option<String>, String) {
    view.read_with(cx, |view, _| match &view.content {
        Content::Session(live) => (
            live.dir.display().to_string(),
            live.project_id.clone(),
            live.session_id.clone().unwrap_or_default(),
        ),
        _ => panic!("no session is open"),
    })
}

/// New chats open where the open one is, and the sidebar's own buttons
/// say where: in no project, or in the project of the row.
#[gpui::test]
fn a_new_chat_opens_in_the_right_place(cx: &mut TestAppContext) {
    let (view, cx) = workbench(cx);
    open_full(&view, "/home/u/proj", cx);
    let chats = view.read_with(cx, |view, _| view.chats.clone());
    let proj = "/home/u/proj".to_owned();
    let new_in = ChatsEvent::NewIn {
        project: "p1".into(),
        root: proj.clone(),
    };
    let steps = [
        (
            "a directory chat with no project",
            Some(ChatsEvent::New),
            (proj.as_str(), None),
        ),
        (
            "the sidebar button",
            Some(ChatsEvent::NewStandalone),
            ("", None),
        ),
        (
            "new from a chat with none",
            Some(ChatsEvent::New),
            ("", None),
        ),
        ("a project row", Some(new_in), (proj.as_str(), Some("p1"))),
        (
            "new from a project chat",
            Some(ChatsEvent::New),
            (proj.as_str(), Some("p1")),
        ),
        ("the key does the same", None, (proj.as_str(), Some("p1"))),
    ];
    for (step, (name, event, (dir, project))) in steps.into_iter().enumerate() {
        let before = place(&view, cx).2;
        match event {
            Some(event) => chats.update(cx, |_, cx| cx.emit(event)),
            None => view.update_in(cx, |view, window, cx| {
                view.chats_command(ChatsCommand::New, window, cx);
            }),
        }
        cx.run_until_parked();
        let (open_dir, open_project, id) = place(&view, cx);
        assert_eq!(
            (open_dir.as_str(), open_project.as_deref()),
            (dir, project),
            "{name}"
        );
        assert_ne!(id, before, "{name} (step {step}) opened a new chat");
    }
}
