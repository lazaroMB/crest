use std::cell::RefCell;
use std::rc::Rc;

use application::ports::events::DomainEvent;
use domain::block::Block;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px};

use super::*;
use crate::timeline::TimelineView;

fn file(path: &str, start: Option<u32>, end: Option<u32>) -> SourceRef {
    SourceRef::File {
        path: path.into(),
        line_start: start,
        line_end: end,
    }
}

fn sources() -> Vec<SourceRef> {
    vec![
        file("src/lib.rs", Some(42), Some(60)),
        SourceRef::Url {
            url: "https://docs.rs/gpui/latest".into(),
            title: Some("gpui docs".into()),
        },
        SourceRef::Commit {
            sha: "0123456789abcdef".into(),
            summary: "fix the parser".into(),
        },
        SourceRef::Command {
            command: "cargo test".into(),
            output_excerpt: "test a ... ok\ntest b ... FAILED".into(),
        },
    ]
}

#[test]
fn labels_each_kind_of_source() {
    let labels: Vec<String> = sources().iter().map(entry_label).collect();
    assert_eq!(
        labels,
        [
            "src/lib.rs:42-60",
            "gpui docs — docs.rs",
            "0123456 fix the parser",
            "$ cargo test",
        ]
    );
    assert_eq!(entry_label(&file("a.rs", Some(3), Some(3))), "a.rs:3");
    assert_eq!(entry_label(&file("a.rs", Some(3), None)), "a.rs:3");
    assert_eq!(entry_label(&file("a.rs", None, None)), "a.rs");
    let untitled = SourceRef::Url {
        url: "https://x.dev/a".into(),
        title: None,
    };
    assert_eq!(entry_label(&untitled), "https://x.dev/a");
}

#[test]
fn host_drops_scheme_credentials_port_and_path() {
    assert_eq!(
        host("https://user:pw@example.com:8080/a?b#c"),
        "example.com"
    );
    assert_eq!(host("http://docs.rs"), "docs.rs");
    assert_eq!(host("example.com/path"), "example.com");
}

#[test]
fn only_workspace_files_and_urls_are_links() {
    let actions: Vec<Option<BlockEvent>> = sources().iter().map(action).collect();
    assert_eq!(
        actions,
        [
            Some(BlockEvent::OpenFile {
                path: WorkspacePath::parse("src/lib.rs").unwrap(),
                line: Some(42),
            }),
            Some(BlockEvent::OpenUrl {
                url: "https://docs.rs/gpui/latest".into(),
            }),
            None,
            None,
        ]
    );
    assert_eq!(action(&file("/etc/passwd", Some(1), None)), None);
    assert_eq!(action(&file("../outside.rs", None, None)), None);
}

/// A window showing one `Sources` block, plus every block event emitted.
fn window(cx: &mut TestAppContext) -> (&mut VisualTestContext, Rc<RefCell<Vec<BlockEvent>>>) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let block = Block::new(BlockKind::Sources { sources: sources() });
    view.update(cx, |view, cx| {
        view.apply(&DomainEvent::BlockAdded { block }, cx);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = Rc::clone(&events);
    cx.update(|_, cx| {
        let emitter = block_events(cx);
        cx.subscribe(&emitter, move |_, event: &BlockEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
    });
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(800.), px(600.)),
        move |_, _| div().size_full().child(view.clone()),
    );
    (cx, events)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} not drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

#[gpui::test]
fn clicking_a_file_citation_opens_it_at_its_line(cx: &mut TestAppContext) {
    let (cx, events) = window(cx);
    click(cx, "source-0");
    assert_eq!(
        *events.borrow(),
        [BlockEvent::OpenFile {
            path: WorkspacePath::parse("src/lib.rs").unwrap(),
            line: Some(42),
        }]
    );
}

#[gpui::test]
fn clicking_a_url_opens_the_browser(cx: &mut TestAppContext) {
    let (cx, events) = window(cx);
    click(cx, "source-1");
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://docs.rs/gpui/latest")
    );
    assert_eq!(
        *events.borrow(),
        [BlockEvent::OpenUrl {
            url: "https://docs.rs/gpui/latest".into(),
        }]
    );
}

#[gpui::test]
fn enter_on_a_focused_citation_opens_it(cx: &mut TestAppContext) {
    let (cx, events) = window(cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    assert_eq!(
        *events.borrow(),
        [BlockEvent::OpenFile {
            path: WorkspacePath::parse("src/lib.rs").unwrap(),
            line: Some(42),
        }]
    );
}
