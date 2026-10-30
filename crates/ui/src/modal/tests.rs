use domain::block::CustomNode;
use domain::modal::{ModalPosition, Percent, modal_bounds};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, size};

use super::*;

fn spec(id: &str, width: f32, height: f32, position: &str, content: CustomNode) -> ModalSpec {
    ModalSpec {
        id: ModalId::new(id),
        plugin: "cov".into(),
        title: format!("Modal {id}"),
        width: Percent::new(width).unwrap(),
        height: Percent::new(height).unwrap(),
        position: position.parse::<ModalPosition>().unwrap(),
        content,
    }
}

fn text(s: &str) -> CustomNode {
    CustomNode::Text { text: s.into() }
}

fn host(cx: &mut TestAppContext) -> (Entity<ModalHost>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    cx.add_window_view(ModalHost::new)
}

fn apply(host: &Entity<ModalHost>, event: DomainEvent, cx: &mut VisualTestContext) {
    host.update_in(cx, |host, window, cx| host.apply(event, window, cx));
}

fn draw(host: &Entity<ModalHost>, cx: &mut VisualTestContext) {
    let viewport = cx.update(|window, _| window.viewport_size());
    let host = host.clone();
    cx.draw(point(px(0.), px(0.)), viewport, move |_, _| {
        div().relative().size_full().child(host.clone())
    });
    cx.run_until_parked();
}

#[test]
fn node_ids_are_stable_per_modal() {
    let a = ModalId::new("cov/a");
    assert_eq!(modal_node_id(&a), modal_node_id(&a));
    assert_ne!(modal_node_id(&a), modal_node_id(&ModalId::new("cov/b")));
}

#[gpui::test]
fn an_open_modal_is_drawn_at_its_bounds(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let spec = spec("cov/a", 50.0, 40.0, "top-right", text("hello"));
    apply(&host, DomainEvent::ModalOpened { spec: spec.clone() }, cx);
    draw(&host, cx);
    let viewport = cx.update(|window, _| window.viewport_size());
    let expected = modal_bounds(f32::from(viewport.width), f32::from(viewport.height), &spec);
    let drawn = cx
        .debug_bounds("modal-card")
        .unwrap_or_else(|| panic!("the modal was not drawn"));
    let got = (
        f32::from(drawn.origin.x),
        f32::from(drawn.origin.y),
        f32::from(drawn.size.width),
        f32::from(drawn.size.height),
    );
    let want = (expected.x, expected.y, expected.width, expected.height);
    assert!(
        (got.0 - want.0).abs() < 1.0
            && (got.1 - want.1).abs() < 1.0
            && (got.2 - want.2).abs() < 1.0
            && (got.3 - want.3).abs() < 1.0,
        "{got:?} != {want:?}"
    );
}

/// A window with a 300 px sidebar and the host over the rest, like the
/// workbench with the chats sidebar shown.
struct BesideSidebar(Entity<ModalHost>);

impl Render for BesideSidebar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .size_full()
            .child(div().w(px(300.)).h_full())
            .child(div().relative().flex_1().h_full().child(self.0.clone()))
    }
}

#[gpui::test]
fn beside_a_sidebar_the_modal_fits_the_session_area(cx: &mut TestAppContext) {
    // `/open` with the chats sidebar shown: the host covers only the area
    // right of the sidebar, and the card must stay inside it.
    cx.update(gpui_component::init);
    let (root, cx) =
        cx.add_window_view(|window, cx| BesideSidebar(cx.new(|cx| ModalHost::new(window, cx))));
    let host = cx.update(|_, cx| root.read(cx).0.clone());
    let spec = spec("cov/a", 100.0, 100.0, "center", text("x"));
    apply(&host, DomainEvent::ModalOpened { spec }, cx);
    cx.simulate_resize(size(px(1000.), px(600.)));
    cx.run_until_parked();
    let drawn = cx
        .debug_bounds("modal-card")
        .unwrap_or_else(|| panic!("not drawn"));
    assert_eq!(drawn.origin, point(px(320.), px(20.)));
    assert_eq!(drawn.size, size(px(660.), px(560.)));
}

#[gpui::test]
fn the_full_size_keeps_the_margin_after_a_resize(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let spec = spec("cov/a", 100.0, 100.0, "center", text("x"));
    apply(&host, DomainEvent::ModalOpened { spec }, cx);
    cx.simulate_resize(size(px(900.), px(500.)));
    draw(&host, cx);
    let drawn = cx
        .debug_bounds("modal-card")
        .unwrap_or_else(|| panic!("not drawn"));
    assert_eq!(drawn.origin, point(px(20.), px(20.)));
    assert_eq!(drawn.size, size(px(860.), px(460.)));
}

#[gpui::test]
fn update_replaces_and_reopen_brings_to_front(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let a = ModalId::new("cov/a");
    let b = ModalId::new("cov/b");
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec("cov/a", 50.0, 50.0, "left", text("one")),
        },
        cx,
    );
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec("cov/b", 50.0, 50.0, "right", text("two")),
        },
        cx,
    );
    apply(
        &host,
        DomainEvent::ModalUpdated {
            id: a.clone(),
            content: text("uno"),
        },
        cx,
    );
    // An update of a modal that is not open does nothing.
    apply(
        &host,
        DomainEvent::ModalUpdated {
            id: ModalId::new("cov/zzz"),
            content: text("?"),
        },
        cx,
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.ids(), [a.clone(), b.clone()]);
        assert_eq!(host.spec(&a).map(|s| s.content.clone()), Some(text("uno")));
    });
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec("cov/a", 30.0, 30.0, "bottom", text("again")),
        },
        cx,
    );
    host.read_with(cx, |host, _| {
        assert_eq!(host.ids(), [b.clone(), a.clone()]);
        assert_eq!(host.spec(&a).map(|s| s.width.get()), Some(30.0));
    });
    draw(&host, cx);
    apply(&host, DomainEvent::ModalClosed { id: b.clone() }, cx);
    host.read_with(cx, |host, _| {
        assert_eq!(host.ids(), std::slice::from_ref(&a))
    });
}

#[gpui::test]
fn escape_closes_the_front_modal_and_focus_returns(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let before = cx.update(|window, cx| {
        let handle = cx.focus_handle();
        window.focus(&handle, cx);
        handle
    });
    for id in ["cov/a", "cov/b"] {
        apply(
            &host,
            DomainEvent::ModalOpened {
                spec: spec(id, 50.0, 50.0, "center", text(id)),
            },
            cx,
        );
    }
    draw(&host, cx);
    cx.update(|window, cx| assert!(host.read(cx).focus_handle.is_focused(window)));
    cx.simulate_keystrokes("escape");
    host.read_with(cx, |host, _| {
        assert_eq!(host.ids(), [ModalId::new("cov/a")])
    });
    draw(&host, cx);
    cx.simulate_keystrokes("escape");
    host.read_with(cx, |host, _| assert!(!host.is_open()));
    cx.update(|window, _| assert!(before.is_focused(window)));
}

#[gpui::test]
fn the_close_button_and_the_backdrop_close(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec("cov/a", 40.0, 40.0, "center", text("x")),
        },
        cx,
    );
    draw(&host, cx);
    // A click inside the card keeps it open.
    let card = cx
        .debug_bounds("modal-card")
        .unwrap_or_else(|| panic!("not drawn"));
    cx.simulate_click(card.center(), Modifiers::none());
    host.read_with(cx, |host, _| assert!(host.is_open()));
    // Outside the card is the backdrop.
    cx.simulate_click(point(px(5.), px(5.)), Modifiers::none());
    host.read_with(cx, |host, _| assert!(!host.is_open()));
}

#[gpui::test]
fn long_content_scrolls(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let lines = (0..300).map(|i| text(&format!("line {i}"))).collect();
    let id = ModalId::new("cov/long");
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec(
                "cov/long",
                50.0,
                30.0,
                "center",
                CustomNode::Col { children: lines },
            ),
        },
        cx,
    );
    draw(&host, cx);
    let max = host.read_with(cx, |host, _| {
        host.scroll_handle(&id)
            .map(|s| s.max_offset())
            .unwrap_or_default()
    });
    assert!(max.y > px(1000.), "the body scrolls: {max:?}");
}

#[test]
fn document_images_resolve_under_the_root() {
    let root = WorkspaceRoot::new(if cfg!(windows) { r"C:\ws" } else { "/ws" }).unwrap();
    let docs = WorkspacePath::parse("docs").unwrap();
    let at = |dir: Option<&WorkspacePath>, uri: &str| {
        document_image(&root, dir, uri).map(|p| p.display().to_string().replace('\\', "/"))
    };
    let ws = if cfg!(windows) { "C:/ws" } else { "/ws" };
    assert_eq!(
        at(None, "assets/logo.svg"),
        Some(format!("{ws}/assets/logo.svg"))
    );
    assert_eq!(
        at(Some(&docs), "./img/a.png"),
        Some(format!("{ws}/docs/img/a.png"))
    );
    assert_eq!(
        at(Some(&docs), "../assets/a.png"),
        Some(format!("{ws}/assets/a.png"))
    );
    assert_eq!(at(None, "../etc/passwd"), None, "outside the workspace");
    assert_eq!(at(None, "https://img.shields.io/badge/x"), None, "a URL");
    assert_eq!(at(None, "data:image/png;base64,AAAA"), None, "a data URI");
    assert_eq!(at(None, ""), None);
}

#[gpui::test]
fn a_document_scrolls_virtualized(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let source: String = (0..2000).map(|i| format!("line {i}\n\n")).collect();
    let id = ModalId::new("cov/doc");
    let open = |source: String| DomainEvent::ModalOpened {
        spec: spec(
            "cov/doc",
            50.0,
            30.0,
            "center",
            CustomNode::Markdown { source },
        ),
    };
    apply(&host, open(source.clone()), cx);
    draw(&host, cx);
    let doc = host.read_with(cx, |host, _| {
        host.document(&id)
            .cloned()
            .unwrap_or_else(|| panic!("a markdown modal has a document"))
    });
    // The plain body is not used: nothing was laid out under its scroll.
    let max = host.read_with(cx, |host, _| {
        host.scroll_handle(&id)
            .map(|s| s.max_offset())
            .unwrap_or_default()
    });
    assert_eq!(max.y, px(0.), "the document is not in the scrolled div");
    let top = |cx: &mut VisualTestContext| {
        doc.read_with(cx, |doc, _| doc.list_state().logical_scroll_top().item_ix)
    };
    assert_eq!(top(cx), 0);
    cx.simulate_keystrokes("end");
    draw(&host, cx);
    assert!(top(cx) > 0, "End scrolls the document's list");
    cx.simulate_keystrokes("home");
    draw(&host, cx);
    assert_eq!(top(cx), 0, "Home goes back to the top");
    // An update of the same modal keeps the document (and its scroll).
    apply(&host, open(format!("{source}tail\n")), cx);
    let same = host.read_with(cx, |host, _| {
        host.document(&id).map(|d| d.entity_id()) == Some(doc.entity_id())
    });
    assert!(same, "the update reused the document");
}

#[gpui::test]
fn links_in_a_modal_open_the_browser(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    let content = CustomNode::Col {
        children: vec![
            text("see"),
            CustomNode::Link {
                text: "the docs".into(),
                url: "https://example.com/modal".into(),
            },
        ],
    };
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec("cov/l", 60.0, 60.0, "center", content),
        },
        cx,
    );
    draw(&host, cx);
    let link = cx
        .debug_bounds("custom-link")
        .unwrap_or_else(|| panic!("no link"));
    cx.simulate_click(link.center(), Modifiers::none());
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/modal")
    );
    host.read_with(cx, |host, _| {
        assert!(host.is_open(), "a link click keeps it")
    });
}

#[gpui::test]
fn posted_events_are_applied_in_order(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    host.read_with(cx, |host, _| {
        host.post(DomainEvent::ModalOpened {
            spec: spec("cov/a", 50.0, 50.0, "center", text("x")),
        });
        host.post(DomainEvent::ModalUpdated {
            id: ModalId::new("cov/a"),
            content: text("y"),
        });
        host.post(DomainEvent::TurnFailed {
            turn_id: domain::conversation::TurnId::new(),
            error: "ignored".into(),
        });
    });
    cx.run_until_parked();
    host.read_with(cx, |host, _| {
        assert_eq!(
            host.spec(&ModalId::new("cov/a")).map(|s| s.content.clone()),
            Some(text("y"))
        );
    });
}

#[gpui::test]
fn every_rich_node_draws_in_a_modal(cx: &mut TestAppContext) {
    let (host, cx) = host(cx);
    apply(
        &host,
        DomainEvent::ModalOpened {
            spec: spec(
                "cov/rich",
                90.0,
                90.0,
                "center",
                crate::blocks::custom::tests::rich_tree(),
            ),
        },
        cx,
    );
    draw(&host, cx);
    host.read_with(cx, |host, _| assert!(host.is_open()));
}
