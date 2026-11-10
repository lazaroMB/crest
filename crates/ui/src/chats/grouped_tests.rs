//! The list with projects and chats in none: what it shows, what its
//! buttons ask for, and how the keyboard and the strip treat the groups.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::model::{CurrentChat, listed_with};

use super::*;

fn chat(id: &str) -> ChatEntry {
    ChatEntry::new(id, id)
}

fn project(id: &str, dirs: usize, chats: &[&str]) -> ProjectChats {
    ProjectChats {
        id: id.into(),
        name: id.into(),
        root: format!("/w/{id}"),
        roots: (0..dirs)
            .map(|n| format!("/w/{id}{}", "+".repeat(n)))
            .collect(),
        pinned: false,
        chats: chats.iter().copied().map(chat).collect(),
    }
}

/// `api` (3 directories, chats a1 a2), `web` (no chat), and the chats l1 l2
/// in no project; the open chat is `current` of the standalone ones.
fn panel<'a>(
    loose: &[&str],
    width: SidebarWidth,
    cx: &'a mut TestAppContext,
) -> (
    Entity<ChatsPanel>,
    Rc<RefCell<Vec<ChatsEvent>>>,
    &'a mut VisualTestContext,
) {
    cx.update(gpui_component::init);
    let catalog = Catalog {
        projects: vec![project("api", 3, &["a1", "a2"]), project("web", 1, &[])],
        chats: loose.iter().copied().map(chat).collect(),
        archived: Archive::default(),
    };
    let (view, cx) = cx.add_window_view(move |_, cx| {
        let mut panel = ChatsPanel::new("/w/api".into(), Some("a1".into()), cx);
        panel.set_catalog(catalog, cx);
        panel.set_width(width, cx);
        panel
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &ChatsEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
    });
    events.borrow_mut().clear();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (view, events, cx)
}

pub(super) fn click(selector: &'static str, cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let bounds = cx.debug_bounds(selector).expect(selector);
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn the_list_has_the_projects_and_then_the_chats_in_none(cx: &mut TestAppContext) {
    let (view, _, cx) = panel(&["l1", "l2"], SidebarWidth::Full, cx);
    let rows = view.read_with(cx, |panel, _| panel.rows());
    let project = |id: &str| ChatRow::Project(id.into());
    let chat = |project: Option<&str>, id: &str| ChatRow::Chat {
        project: project.map(str::to_owned),
        id: id.into(),
    };
    assert_eq!(
        rows,
        [
            project("api"),
            chat(Some("api"), "a1"),
            chat(Some("api"), "a2"),
            project("web"),
            chat(None, "l1"),
            chat(None, "l2"),
        ],
        "the open project is unfolded, the other folded; the chats in none follow"
    );
    // One element per row, so the scroll keeps the selection in view.
    for selector in ["chats-project-0", "chats-session-0-1", "chats-session-2-1"] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(cx.debug_bounds("chats-loose-empty").is_none());
}

#[gpui::test]
fn without_chats_in_none_the_section_says_how_they_appear(cx: &mut TestAppContext) {
    let (view, _, cx) = panel(&[], SidebarWidth::Full, cx);
    assert!(cx.debug_bounds("chats-loose-empty").is_some());
    view.read_with(cx, |panel, _| assert_eq!(panel.rows().len(), 4));
}

#[gpui::test]
fn the_buttons_and_the_initials_ask_for_the_chat_in_their_group(cx: &mut TestAppContext) {
    let new_in = |project: &str| ChatsEvent::NewIn {
        project: project.into(),
        root: format!("/w/{project}"),
    };
    let open = |root: &str, id: &str| ChatsEvent::Open {
        root: root.into(),
        id: id.into(),
    };
    use SidebarWidth::{Full, Strip};
    // (width, what is clicked, what the panel asks for)
    let cases = [
        (Full, "chats-project-new-1", new_in("web")),
        (Full, "chats-session-2-0", open("", "l1")),
        (Full, "chats-new", ChatsEvent::NewStandalone),
        (Strip, "sidebar-initial-1", new_in("web")),
        (Strip, "sidebar-initial-0", open("/w/api", "a1")),
    ];
    for (width, selector, want) in cases {
        let (view, events, cx) = panel(&["l1"], width, cx);
        let rows = view.read_with(cx, |panel, _| panel.rows());
        click(selector, cx);
        assert_eq!(*events.borrow(), [want], "{selector}");
        let after = view.read_with(cx, |panel, _| panel.rows());
        assert_eq!(after, rows, "{selector} did not fold or unfold anything");
    }
}

#[gpui::test]
fn neighbors_and_the_next_chat_stay_inside_the_group(cx: &mut TestAppContext) {
    let (view, _, cx) = panel(&["l1", "l2", "l3"], SidebarWidth::Full, cx);
    let step = |id: &str, by: isize, cx: &mut VisualTestContext| {
        view.update(cx, |panel, cx| {
            panel.set_current(String::new(), None, Some(id.into()), cx);
        });
        view.read_with(cx, |panel, _| panel.neighbor(by))
    };
    let some = |root: &str, id: &str| Some((root.to_owned(), id.to_owned()));
    // (the open chat, step, what the key opens)
    let cases = [
        ("l1", 1, some("", "l2")),
        ("l3", 1, some("", "l1")),
        ("l1", -1, some("", "l3")),
        ("a1", 1, some("/w/api", "a2")),
        ("a1", -1, some("/w/api", "a2")),
    ];
    for (open, by, want) in cases {
        assert_eq!(step(open, by, cx), want, "{open} {by}");
    }
    view.read_with(cx, |panel, _| {
        assert_eq!(panel.next_chat("l2").as_deref(), Some("l1"));
        assert_eq!(panel.next_chat("a1").as_deref(), Some("a2"));
        assert_eq!(panel.next_chat("zzz"), None);
    });
}

#[gpui::test]
fn the_keyboard_walks_from_the_projects_into_the_chats_in_none(cx: &mut TestAppContext) {
    let (view, events, cx) = panel(&["l1"], SidebarWidth::Full, cx);
    cx.update(|window, _| window.activate_window());
    view.update_in(cx, |panel, window, cx| panel.focus(window, cx));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    cx.simulate_keystrokes("end");
    let selected = view.read_with(cx, |panel, _| panel.selected().cloned());
    assert_eq!(
        selected,
        Some(ChatRow::Chat {
            project: None,
            id: "l1".into()
        })
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [ChatsEvent::Open {
            root: String::new(),
            id: "l1".into()
        }]
    );
}

#[gpui::test]
fn the_strip_has_one_initial_per_project_and_none_for_chats_in_none(cx: &mut TestAppContext) {
    let (_view, _, cx) = panel(&["l1"], SidebarWidth::Strip, cx);
    let initials = [
        "sidebar-initial-0",
        "sidebar-initial-1",
        "sidebar-initial-2",
    ]
    .map(|selector| cx.debug_bounds(selector).is_some());
    assert_eq!(initials, [true, true, false]);
}

/// The open chat as the list gets it, from `root|project|id` (a part may be
/// empty).
fn at(spec: &str) -> CurrentChat<'_> {
    let mut parts = spec.split('|');
    let mut next = || parts.next().unwrap_or_default();
    let root = next();
    let (project, id) = (next(), next());
    CurrentChat {
        root,
        project: Some(project).filter(|p| !p.is_empty()),
        id: Some(id).filter(|id| !id.is_empty()),
    }
}

/// What the list shows of `catalog` for `current`: each project with its
/// chats, then the chats in none.
fn shown(catalog: Catalog, current: CurrentChat<'_>) -> String {
    let listed = listed_with(catalog, &current);
    let projects: Vec<String> = listed
        .projects
        .iter()
        .map(|p| format!("{}={}", p.id, ids(&p.chats)))
        .collect();
    format!("{} | {}", projects.join(" "), ids(&listed.chats))
}

fn ids(chats: &[ChatEntry]) -> String {
    chats
        .iter()
        .map(|c| c.id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// What the list adds for the open chat, by case.
#[test]
fn the_open_project_and_chat_stay_on_the_list() {
    let one = || Catalog {
        projects: vec![project("a", 1, &["x"])],
        chats: vec![chat("loose")],
        archived: Archive::default(),
    };
    // (case, whether the catalog has `a` and a chat in none, the open chat,
    // what the list shows)
    let cases = [
        ("made for a directory", false, "/w/new||n", "/w/new=n | "),
        (
            "a project not listed keeps its id",
            false,
            "/w/n|p9|n",
            "p9=n | ",
        ),
        (
            "none for a chat with no directory",
            true,
            "||n",
            "a=x | n,loose",
        ),
        ("not repeated when listed", true, "/w/a|a|x", "a=x | loose"),
        (
            "first in its project, by id",
            true,
            "/else|a|n",
            "a=n,x | loose",
        ),
        (
            "first in its project, by directory",
            true,
            "/w/a/||n",
            "a=n,x | loose",
        ),
        ("nothing without a chat or directory", false, "||", " | "),
    ];
    for (name, listed, current, want) in cases {
        let catalog = if listed { one() } else { Catalog::default() };
        assert_eq!(shown(catalog, at(current)), want, "{name}");
    }
}
