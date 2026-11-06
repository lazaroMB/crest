//! The menus of chats and projects, drawn: where they float, what each row
//! asks for, the keyboard, and the Archived section.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};

use super::grouped_tests::click;
use super::*;

type Events = Rc<RefCell<Vec<ChatsEvent>>>;

fn chat(id: &str) -> ChatEntry {
    ChatEntry::new(id, id)
}

/// `api` (two directories; chats a1 a2), `web` (one directory, no chat), the
/// chat `l1` in no project, and an archive with a project, a chat and a
/// branch in the list. The open chat is `a1`; the sidebar is full.
fn panel(cx: &mut TestAppContext) -> (Entity<ChatsPanel>, Events, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let catalog = Catalog {
        projects: vec![
            ProjectChats {
                id: "api".into(),
                name: "api".into(),
                root: "/w/api".into(),
                roots: vec!["/w/api".into(), "/w/lib".into()],
                chats: vec![
                    chat("a1"),
                    ChatEntry {
                        branch: Some(Branch {
                            from: "a1".into(),
                            after: 3,
                        }),
                        ..chat("a2")
                    },
                ],
                ..ProjectChats::default()
            },
            ProjectChats {
                id: "web".into(),
                name: "web".into(),
                root: "/w/web".into(),
                roots: vec!["/w/web".into()],
                ..ProjectChats::default()
            },
        ],
        chats: vec![chat("l1")],
        archived: Archive {
            projects: vec![ArchivedProject {
                id: "old".into(),
                name: "old".into(),
                chats: 2,
            }],
            chats: vec![chat("z1")],
        },
    };
    let (view, cx) = cx.add_window_view(move |_, cx| {
        let mut panel = ChatsPanel::new("/w/api".into(), Some("a1".into()), cx);
        panel.set_catalog(catalog, cx);
        panel.set_width(SidebarWidth::Full, cx);
        panel
    });
    let events: Events = Rc::default();
    let sink = Rc::clone(&events);
    cx.update(|window, cx| {
        window.activate_window();
        cx.subscribe(&view, move |_, event: &ChatsEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
    });
    events.borrow_mut().clear();
    redraw(cx);
    (view, events, cx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn right_click(selector: &'static str, cx: &mut VisualTestContext) {
    redraw(cx);
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    redraw(cx);
}

fn chat_event(id: &str, op: ChatOp) -> ChatsEvent {
    ChatsEvent::Chat { id: id.into(), op }
}

fn project_event(id: &str, op: ProjectOp) -> ChatsEvent {
    ChatsEvent::Project { id: id.into(), op }
}

#[gpui::test]
fn a_menu_floats_over_the_rows_and_moves_none_of_them(cx: &mut TestAppContext) {
    let (view, _, cx) = panel(cx);
    let rows = ["chats-session-0-0", "chats-session-0-1", "chats-project-1"];
    let before: Vec<_> = rows.iter().map(|row| cx.debug_bounds(row)).collect();
    right_click("chats-session-0-0", cx);
    assert!(view.read_with(cx, |panel, _| panel.menu_open()));
    let menu = cx.debug_bounds("chats-menu").expect("the menu is drawn");
    let after: Vec<_> = rows.iter().map(|row| cx.debug_bounds(row)).collect();
    assert_eq!(before, after, "no row moved to make room");
    let below = cx.debug_bounds("chats-session-0-1").unwrap();
    assert!(
        menu.intersects(&below),
        "the menu is over the row under it, not between rows"
    );
    // The `...` button, which shows under the pointer, opens the menu of
    // another row (the first menu covers it: close that one first).
    keys("escape", cx);
    assert!(cx.debug_bounds("chats-menu").is_none());
    let row = cx.debug_bounds("chats-session-0-1").unwrap().center();
    cx.simulate_mouse_move(row, None, Modifiers::none());
    redraw(cx);
    click("chats-more-0-1", cx);
    assert!(cx.debug_bounds("chats-menu").is_some());
    assert_eq!(
        rows.map(|row| cx.debug_bounds(row)).to_vec(),
        before,
        "still no row moved"
    );
}

/// A case of the menu rows: the row right-clicked, the rows clicked in order,
/// and what the window is asked for.
type Case = (&'static str, Vec<&'static str>, Vec<ChatsEvent>);

/// A click path in the menu of the chat `a1`.
fn on_chat(path: &[&'static str], want: ChatsEvent) -> Case {
    ("chats-session-0-0", path.to_vec(), vec![want])
}

/// A click path in the menu of the project `api`.
fn on_project(path: &[&'static str], want: ChatsEvent) -> Case {
    ("chats-project-0", path.to_vec(), vec![want])
}

fn chat_cases() -> Vec<Case> {
    let a1 = |op| chat_event("a1", op);
    let new_in_api = ChatsEvent::NewIn {
        project: "api".into(),
        root: "/w/api".into(),
    };
    let delete = ChatsEvent::Delete {
        root: "/w/api".into(),
        id: "a1".into(),
    };
    vec![
        on_chat(&["chats-new-0-0"], new_in_api),
        on_chat(&["chats-branch-0-0"], a1(ChatOp::Branch)),
        on_chat(&["chats-pin-0-0"], a1(ChatOp::Pin(true))),
        on_chat(&["chats-archive-0-0"], a1(ChatOp::Archive(true))),
        on_chat(&["chats-remove-0-0"], a1(ChatOp::Move(None))),
        on_chat(
            &["chats-move-0-0", "chats-project-0-0"],
            a1(ChatOp::Move(Some("web".into()))),
        ),
        on_chat(
            &["chats-move-0-0", "chats-none-0-0"],
            a1(ChatOp::Move(None)),
        ),
        on_chat(
            &["chats-move-0-0", "chats-new-project-0-0"],
            ChatsEvent::Pick(PickFor::NewProject { chat: "a1".into() }),
        ),
        on_chat(&["chats-delete-0-0", "chats-confirm-delete-0-0"], delete),
        // Cancelling anywhere asks for nothing (but gives the prompt the focus).
        on_chat(
            &["chats-delete-0-0", "chats-cancel-delete-0-0"],
            ChatsEvent::Dismissed,
        ),
        on_chat(&["chats-cancel-0-0"], ChatsEvent::Dismissed),
        // A chat in no project.
        (
            "chats-session-2-0",
            vec!["chats-new-2-0"],
            vec![ChatsEvent::NewStandalone],
        ),
    ]
}

fn project_cases() -> Vec<Case> {
    let api = |op| project_event("api", op);
    let add_dir = ChatsEvent::Pick(PickFor::AddDirectory {
        project: "api".into(),
    });
    let new_in_api = ChatsEvent::NewIn {
        project: "api".into(),
        root: "/w/api".into(),
    };
    let remove = ProjectOp::RemoveDirectory("/w/api".into());
    vec![
        on_project(&["chats-new-p0"], new_in_api),
        on_project(&["chats-add-dir-p0"], add_dir),
        on_project(&["chats-pin-p0"], api(ProjectOp::Pin(true))),
        on_project(&["chats-archive-p0"], api(ProjectOp::Archive(true))),
        on_project(&["chats-remove-dir-p0", "chats-dir-remove-p0"], api(remove)),
        on_project(
            &["chats-delete-p0", "chats-confirm-delete-p0"],
            api(ProjectOp::Delete),
        ),
        // The last directory of a project cannot be removed: the row is
        // there, says why, and does nothing.
        ("chats-project-1", vec!["chats-remove-dir-p1"], vec![]),
    ]
}

#[gpui::test]
fn the_rows_of_a_menu_ask_the_window_for_what_they_say(cx: &mut TestAppContext) {
    let (_, events, cx) = panel(cx);
    for (target, path, want) in chat_cases().into_iter().chain(project_cases()) {
        events.borrow_mut().clear();
        right_click(target, cx);
        for row in &path {
            click(row, cx);
        }
        assert_eq!(*events.borrow(), want, "{target} {path:?}");
        if want.is_empty() {
            assert!(
                cx.debug_bounds("chats-menu").is_some(),
                "{target} stays open"
            );
            click("chats-cancel-p1", cx);
        }
        redraw(cx);
        assert!(
            cx.debug_bounds("chats-menu").is_none(),
            "{target} {path:?} closed"
        );
    }
}

#[gpui::test]
fn a_chat_in_a_project_never_offers_its_own_project_or_none_when_it_has_none(
    cx: &mut TestAppContext,
) {
    let (_, _, cx) = panel(cx);
    // (right-clicked row, the move rows shown: project rows, "No project")
    let cases = [
        (
            "chats-session-0-0",
            [("chats-project-0-0", true), ("chats-none-0-0", true)],
        ),
        (
            "chats-session-2-0",
            [("chats-project-2-0", true), ("chats-none-2-0", false)],
        ),
    ];
    for (target, rows) in cases {
        right_click(target, cx);
        let tag = target.trim_start_matches("chats-session-");
        click(Box::leak(format!("chats-move-{tag}").into_boxed_str()), cx);
        for (row, shown) in rows {
            assert_eq!(cx.debug_bounds(row).is_some(), shown, "{target} {row}");
        }
        // Only `web`: the archived project `old` is never offered.
        assert!(cx.debug_bounds("chats-project-0-1").is_none(), "{target}");
        click(
            Box::leak(format!("chats-cancel-{tag}").into_boxed_str()),
            cx,
        );
    }
}

#[gpui::test]
fn the_keyboard_opens_walks_goes_back_and_closes_a_menu(cx: &mut TestAppContext) {
    let (view, events, cx) = panel(cx);
    focus(&view, cx);
    redraw(cx);
    // `m` opens the menu of the selected row (a1); Down twice reaches Rename.
    keys("m", cx);
    assert!(cx.debug_bounds("chats-menu").is_some());
    let open = |cx: &mut VisualTestContext| view.read_with(cx, |panel, _| panel.menu_open());
    keys("down down enter", cx);
    assert!(
        cx.update(|window, cx| rename_has_focus(window, cx)),
        "Enter on Rename opens the name field"
    );
    keys("escape", cx);
    assert!(!open(cx));
    // Into "Move to project" with Enter, back with Left, out with Esc.
    focus(&view, cx);
    keys("m down down down down enter", cx);
    assert!(
        cx.debug_bounds("chats-new-project-0-0").is_some(),
        "the move page"
    );
    keys("left", cx);
    assert!(
        cx.debug_bounds("chats-branch-0-0").is_some(),
        "back on the list"
    );
    keys("escape", cx);
    assert!(!open(cx));
    // Enter on the Archive row asks for it; the menu is gone.
    focus(&view, cx);
    events.borrow_mut().clear();
    keys("m up up up enter", cx);
    assert_eq!(
        *events.borrow(),
        [chat_event("a1", ChatOp::Archive(true))],
        "up from the first row wraps to Cancel, then Delete, then Archive"
    );
    assert!(!open(cx));
    // Delete on a project row asks first; Enter on the question's row deletes.
    view.update_in(cx, |panel, window, cx| {
        panel.selected = Some(ChatRow::Project("api".into()));
        panel.focus(window, cx);
    });
    events.borrow_mut().clear();
    keys("delete", cx);
    assert!(cx.debug_bounds("chats-confirm-delete-p0").is_some());
    cx.simulate_keystrokes("enter");
    assert_eq!(*events.borrow(), [project_event("api", ProjectOp::Delete)]);
}

#[gpui::test]
fn renaming_a_project_edits_its_row_in_place(cx: &mut TestAppContext) {
    let (view, events, cx) = panel(cx);
    right_click("chats-project-0", cx);
    click("chats-rename-p0", cx);
    assert!(cx.update(|window, cx| rename_has_focus(window, cx)));
    assert!(
        cx.debug_bounds("chats-menu").is_none(),
        "the field is in the row"
    );
    cx.simulate_input("rename");
    let input = view
        .read_with(cx, |panel, _| panel.rename_input.clone())
        .expect("the field is open");
    input.update_in(cx, |state, window, cx| {
        state.set_value("  server ", window, cx)
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [project_event("api", ProjectOp::Rename("server".into()))]
    );
}

#[gpui::test]
fn the_archived_section_is_folded_and_restores_or_deletes_each_entry(cx: &mut TestAppContext) {
    let (view, events, cx) = panel(cx);
    assert!(
        cx.debug_bounds("chats-archived").is_some(),
        "the heading shows"
    );
    assert!(
        cx.debug_bounds("chats-archived-0").is_none(),
        "folded by default"
    );
    assert_eq!(
        view.read_with(cx, |panel, _| panel.rows().len()),
        6,
        "api, a1, a2, web, l1 and the Archived heading"
    );
    click("chats-archived", cx);
    assert!(view.read_with(cx, |panel, _| panel.archived_is_open()));
    // (what is clicked, what the window is asked)
    let cases: Vec<(Vec<&str>, Vec<ChatsEvent>)> = vec![
        (
            vec!["chats-restore-0"],
            vec![project_event("old", ProjectOp::Archive(false))],
        ),
        (
            vec!["chats-restore-1"],
            vec![chat_event("z1", ChatOp::Archive(false))],
        ),
        (
            vec!["chats-archived-delete-0", "chats-confirm-delete-a0"],
            vec![project_event("old", ProjectOp::Delete)],
        ),
        (
            vec!["chats-archived-delete-1", "chats-confirm-delete-a1"],
            vec![ChatsEvent::Delete {
                root: String::new(),
                id: "z1".into(),
            }],
        ),
    ];
    for (path, want) in cases {
        events.borrow_mut().clear();
        for row in &path {
            click(row, cx);
        }
        assert_eq!(*events.borrow(), want, "{path:?}");
    }
    // Strip and hidden have no rows for it.
    for width in [SidebarWidth::Strip, SidebarWidth::Hidden] {
        view.update(cx, |panel, cx| panel.set_width(width, cx));
        redraw(cx);
        assert!(cx.debug_bounds("chats-archived").is_none(), "{width:?}");
    }
}

#[gpui::test]
fn a_branch_shows_its_icon_and_a_chat_that_is_not_one_does_not(cx: &mut TestAppContext) {
    let (_, _, cx) = panel(cx);
    assert!(cx.debug_bounds("chats-branch").is_some());
    let row = cx.debug_bounds("chats-session-0-1").unwrap();
    let icon = cx.debug_bounds("chats-branch").unwrap();
    assert!(
        row.contains(&icon.center()),
        "the icon is on a2's row, the branch"
    );
}

/// Gives the list the keyboard.
fn focus(view: &Entity<ChatsPanel>, cx: &mut VisualTestContext) {
    view.update_in(cx, |panel, window, cx| panel.focus(window, cx));
    redraw(cx);
}

/// Types `keys` and draws what they did.
fn keys(keys: &str, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes(keys);
    redraw(cx);
}
