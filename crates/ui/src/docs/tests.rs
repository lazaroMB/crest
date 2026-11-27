//! Tests of the doc panel and its diagram editor.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use application_core::keymap::Action;
use base64::Engine as _;
use domain::conversation::SessionId;
use domain::doc::{Doc, DocId, ExportFormat, insert_diagram_after};
use gpui::{TestAppContext, VisualTestContext, div, point, px, size};

use super::diagram_editor::{
    SPLIT_MAX, SPLIT_MIN, SplitMemory, clamp_split_percent, drawable_templates, filter_drawable,
    zoom_step,
};
use super::toolbar::Tool;
use super::*;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
const FLOW: &str = "flowchart TD\n  A --> B";
const TEXT: &str =
    "# Title\n\nfirst para\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nlast para\n";

fn doc() -> Doc {
    Doc {
        id: DocId::new(),
        session_id: SessionId::new(),
        title: "Notes".into(),
        file_name: "notes.md".into(),
        dir: "/tmp/docs/s".into(),
        created_at: 1,
        updated_at: 1,
    }
}

type Log = Rc<RefCell<Vec<DocPanelEvent>>>;

fn setup<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<DocPanel>, &'a mut VisualTestContext, Log) {
    cx.update(gpui_component::init);
    cx.update(zorite_editor::bind_keys);
    let (panel, cx) = cx.add_window_view(DocPanel::new);
    let log: Log = Rc::default();
    let sink = log.clone();
    cx.update(|_, cx| {
        cx.subscribe(&panel, move |_, event: &DocPanelEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
    });
    let shown = doc();
    panel.update(cx, |panel, cx| {
        panel.show(shown, text.to_owned(), Vec::new(), cx)
    });
    draw(cx, &panel);
    (panel, cx, log)
}

fn draw(cx: &mut VisualTestContext, panel: &Entity<DocPanel>) {
    for _ in 0..2 {
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            div().size_full().child(panel.clone())
        });
        cx.run_until_parked();
    }
}

/// Lets the window draw its own frame. A manual `cx.draw` leaves deferred
/// layers (menus, the selection bar) in the arena, and the window's next
/// frame would read them after it was cleared.
fn draw_once(cx: &mut VisualTestContext, _panel: &Entity<DocPanel>) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn source_text(panel: &Entity<DocPanel>, cx: &mut VisualTestContext) -> String {
    panel.read_with(cx, |p, cx| p.source.read(cx).value().to_string())
}

fn tool(panel: &Entity<DocPanel>, which: Tool, cx: &mut VisualTestContext) {
    panel.update_in(cx, |p, window, cx| p.run_tool(which, window, cx));
}

fn editor_of(panel: &Entity<DocPanel>, cx: &mut VisualTestContext) -> Entity<DiagramEditor> {
    panel.read_with(cx, |p, _| {
        p.diagram_editor().cloned().expect("diagram editor open")
    })
}

fn set_code(editor: &Entity<DiagramEditor>, code: &str, cx: &mut VisualTestContext) {
    editor.update_in(cx, |e, window, cx| e.set_code(code, window, cx));
}

#[gpui::test]
fn the_panel_shows_a_doc_and_toggles_the_source(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.read_with(cx, |p, _| {
        assert_eq!(p.doc().map(|d| d.title.as_str()), Some("Notes"));
        assert_eq!(p.text(), TEXT);
        assert!(!p.is_source());
    });
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    assert!(panel.read_with(cx, |p, _| p.is_source()));
    assert_eq!(source_text(&panel, cx), TEXT);
    panel.update(cx, |p, cx| p.toggle_source(cx));
    assert!(!panel.read_with(cx, |p, _| p.is_source()));
}

#[gpui::test]
fn the_page_draws_text_a_diagram_and_an_image(cx: &mut TestAppContext) {
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap();
    let text = format!("{TEXT}\n![shot](assets/shot.png)\n\n![gone](assets/gone.png)\n");
    cx.update(gpui_component::init);
    let (panel, cx) = cx.add_window_view(DocPanel::new);
    panel.update(cx, |p, cx| {
        p.show(doc(), text, vec![("shot.png".into(), png)], cx);
    });
    draw(cx, &panel);
    assert!(cx.debug_bounds("doc-page").is_some());
}

#[gpui::test]
fn the_saved_state_has_its_words(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    let label = |cx: &mut VisualTestContext| panel.read_with(cx, |p, _| p.saved_label());
    assert_eq!(label(cx), "saved");
    panel.update(cx, |p, cx| p.set_saved(SavedState::Saving, None, cx));
    assert_eq!(label(cx), "saving…");
    panel.update(cx, |p, cx| {
        p.set_saved(SavedState::Failed("disk full".into()), None, cx)
    });
    assert_eq!(label(cx), "save failed: disk full");
    panel.update(cx, |p, cx| {
        p.set_saved(SavedState::Saved, Some("agent"), cx)
    });
    assert_eq!(label(cx), "edited by agent");
}

#[gpui::test]
fn the_view_cycles_and_actions_reach_the_panel(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, TEXT);
    for expected in [DocView::Split, DocView::Alone, DocView::Hidden] {
        panel.update_in(cx, |p, window, cx| {
            assert!(p.handle_action(&Action::builtin(TOGGLE_ACTION), window, cx));
        });
        assert_eq!(panel.read_with(cx, |p, _| p.view()), expected);
    }
    panel.update_in(cx, |p, window, cx| {
        assert!(p.handle_action(&Action::builtin(SOURCE_ACTION), window, cx));
        assert!(p.handle_action(&Action::builtin(NEW_ACTION), window, cx));
        assert!(p.handle_action(&Action::builtin(EXPORT_ACTION), window, cx));
        assert!(!p.handle_action(&Action::builtin("block.next"), window, cx));
    });
    assert!(panel.read_with(cx, |p, _| p.is_source()));
    assert!(log.borrow().contains(&DocPanelEvent::New));
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|e| matches!(e, DocPanelEvent::ViewChanged(_)))
            .count(),
        3
    );
    panel.update(cx, |p, cx| p.export(ExportFormat::Html, cx));
    assert!(
        log.borrow()
            .contains(&DocPanelEvent::Export(ExportFormat::Html))
    );
}

#[gpui::test]
fn bold_on_a_selection_changes_the_text_and_saves_after_the_pause(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "a bold b\n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    panel.update(cx, |p, cx| {
        p.source.update(cx, |s, cx| s.set_selected_range(2..6, cx))
    });
    tool(&panel, Tool::Bold, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "a **bold** b\n");
    assert_eq!(source_text(&panel, cx), "a **bold** b\n");
    assert_eq!(
        panel.read_with(cx, |p, _| p.saved().clone()),
        SavedState::Dirty
    );
    assert!(log.borrow().is_empty(), "nothing is saved before the pause");
    cx.executor()
        .advance_clock(Duration::from_millis(SAVE_AFTER_MS + 50));
    cx.run_until_parked();
    let saves: Vec<_> = log
        .borrow()
        .iter()
        .filter_map(|e| match e {
            DocPanelEvent::Save { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(saves, vec!["a **bold** b\n".to_owned()]);
    assert_eq!(
        panel.read_with(cx, |p, _| p.saved().clone()),
        SavedState::Saving
    );
}

#[gpui::test]
fn typing_in_the_source_is_saved_once_after_the_last_key(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "x\n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    for text in ["xa\n", "xab\n"] {
        panel.update_in(cx, |p, window, cx| {
            p.source
                .update(cx, |s, cx| s.replace_all(text.to_owned(), window, cx));
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    assert!(log.borrow().is_empty());
    cx.executor()
        .advance_clock(Duration::from_millis(SAVE_AFTER_MS));
    cx.run_until_parked();
    let saves = log
        .borrow()
        .iter()
        .filter(|e| matches!(e, DocPanelEvent::Save { text, .. } if text == "xab\n"))
        .count();
    assert_eq!(saves, 1, "{:?}", log.borrow());
}

#[gpui::test]
fn the_link_button_asks_for_an_address_and_ignores_an_empty_one(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "see docs here\n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    panel.update(cx, |p, cx| {
        p.source.update(cx, |s, cx| s.set_selected_range(4..8, cx))
    });
    tool(&panel, Tool::Link, cx);
    assert!(panel.read_with(cx, |p, _| p.is_asking_link()));
    panel.update_in(cx, |p, window, cx| p.confirm_link(window, cx));
    assert!(!panel.read_with(cx, |p, _| p.is_asking_link()));
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "see docs here\n");

    panel.update(cx, |p, cx| {
        p.source.update(cx, |s, cx| s.set_selected_range(4..8, cx))
    });
    tool(&panel, Tool::Link, cx);
    panel.update_in(cx, |p, window, cx| {
        p.set_link_address("https://example.com", window, cx);
        p.confirm_link(window, cx);
    });
    assert_eq!(
        panel.read_with(cx, |p, _| p.text()),
        "see [docs](https://example.com) here\n"
    );
}

#[gpui::test]
fn an_inserted_image_is_stored_named_and_linked(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "pic:\n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap();
    let name = panel.update_in(cx, |p, window, cx| {
        p.insert_image("Shot One.PNG", png.clone(), window, cx)
    });
    assert_eq!(name.as_deref(), Some("shot-one.png"));
    assert!(
        panel
            .read_with(cx, |p, _| p.text())
            .contains("![Shot One](assets/shot-one.png)")
    );
    assert!(log.borrow().contains(&DocPanelEvent::WriteAsset {
        name: "shot-one.png".into(),
        bytes: png.clone()
    }));
    let again = panel.update_in(cx, |p, window, cx| {
        p.insert_image("Shot One.PNG", png, window, cx)
    });
    assert_eq!(again.as_deref(), Some("shot-one-2.png"));
    let refused = panel.update_in(cx, |p, window, cx| {
        p.insert_image("notes.txt", vec![1], window, cx)
    });
    assert_eq!(refused, None);
}

#[gpui::test]
fn the_selection_bar_asks_the_agent_and_the_title_renames(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, TEXT);
    panel.update(cx, |p, cx| {
        p.selection = Some(SelectionBar {
            at: point(px(10.), px(10.)),
            text: "first para".into(),
        });
        cx.notify();
    });
    panel.update(cx, |p, cx| p.ask_agent(cx));
    let id = panel.read_with(cx, |p, _| p.doc().unwrap().id);
    assert!(log.borrow().contains(&DocPanelEvent::AskAgent {
        id,
        passage: "first para".into()
    }));

    panel.update_in(cx, |p, window, cx| p.start_rename(window, cx));
    assert!(panel.read_with(cx, |p, _| p.is_renaming()));
    panel.update_in(cx, |p, window, cx| {
        let input = p.rename.as_ref().unwrap().input.clone();
        input.update(cx, |s, cx| s.set_value("Release notes", window, cx));
        p.confirm_rename(cx);
    });
    assert!(log.borrow().contains(&DocPanelEvent::Rename {
        id,
        title: "Release notes".into()
    }));
}

#[gpui::test]
fn the_export_menu_draws_over_the_panel(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update_in(cx, |p, window, cx| {
        p.handle_action(&Action::builtin(EXPORT_ACTION), window, cx);
    });
    draw_once(cx, &panel);
    assert!(cx.debug_bounds("doc-menu").is_some());
}

#[gpui::test]
fn the_selection_bar_draws_over_the_page(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| {
        p.selection = Some(SelectionBar {
            at: point(px(10.), px(10.)),
            text: "first para".into(),
        });
        cx.notify();
    });
    draw_once(cx, &panel);
    assert!(cx.debug_bounds("doc-selection-bar").is_some());
}

#[gpui::test]
fn the_diagram_editor_saves_in_place(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.source(cx)), FLOW);
    set_code(&editor, "flowchart TD\n  X --> Y", cx);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    let text = panel.read_with(cx, |p, _| p.text());
    assert!(
        text.contains("X --> Y") && !text.contains("A --> B"),
        "{text}"
    );
    assert!(text.starts_with("# Title") && text.ends_with("last para\n"));
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, DocPanelEvent::Save { text, .. } if text.contains("X --> Y")))
    );
}

#[gpui::test]
fn a_new_diagram_goes_after_the_block_of_the_cursor(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "# Title\n\nfirst para\n\nlast para\n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    // The cursor is in "first para", the second block.
    panel.update(cx, |p, cx| {
        p.source
            .update(cx, |s, cx| s.set_selected_range(12..12, cx))
    });
    panel.update(cx, |p, cx| p.open_new_diagram(cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(
        editor.read_with(cx, |e, _| e.spec()),
        DiagramSpec::New { after: Some(1) }
    );
    set_code(&editor, FLOW, cx);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    let expected = insert_diagram_after("# Title\n\nfirst para\n\nlast para\n", Some(1), FLOW);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), expected);
    assert!(expected.find("```mermaid").unwrap() < expected.find("last para").unwrap());
    assert!(
        panel.read_with(cx, |p, _| p.is_source()),
        "back where it came from"
    );
    assert_eq!(source_text(&panel, cx), expected);
}

#[gpui::test]
fn an_emptied_diagram_is_removed_and_an_empty_new_one_adds_nothing(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    set_code(&editor, "", cx);
    draw(cx, &panel);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    let text = panel.read_with(cx, |p, _| p.text());
    assert!(
        !text.contains("mermaid") && !text.contains("\n\n\n"),
        "{text:?}"
    );

    panel.update(cx, |p, cx| p.open_new_diagram(cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    // A new diagram starts from the Flowchart template; emptied, it adds nothing.
    set_code(&editor, "", cx);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    assert_eq!(panel.read_with(cx, |p, _| p.text()), text);
}

#[gpui::test]
fn escape_leaves_the_diagram_editor_with_the_text_unchanged(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    set_code(&editor, "flowchart TD\n  P --> Q", cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
    assert_eq!(panel.read_with(cx, |p, _| p.text()), TEXT);
    assert!(
        log.borrow().is_empty(),
        "nothing to save: {:?}",
        log.borrow()
    );
}

#[gpui::test]
fn the_splitter_stays_within_bounds_and_survives_reopening(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(editor.read_with(cx, |e, _| e.split()), 50.0);
    editor.update(cx, |e, cx| e.set_split(3.0, cx));
    assert_eq!(editor.read_with(cx, |e, _| e.split()), SPLIT_MIN);
    editor.update(cx, |e, cx| e.set_split(99.0, cx));
    assert_eq!(editor.read_with(cx, |e, _| e.split()), SPLIT_MAX);
    editor.update(cx, |e, cx| e.set_split(70.0, cx));
    editor.update(cx, |e, cx| e.cancel(cx));
    cx.run_until_parked();
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
    assert_eq!(cx.update(|_, cx| cx.global::<SplitMemory>().0), 70.0);

    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    assert_eq!(editor_of(&panel, cx).read_with(cx, |e, _| e.split()), 70.0);
}

#[gpui::test]
fn the_drawing_follows_the_code_and_a_long_source_says_it_is_cut(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_new_diagram(cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    set_code(&editor, FLOW, cx);
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.error().map(str::to_owned)),
        None
    );
    assert!(editor.read_with(cx, |e, _| e.effective_scale()).is_some());
    set_code(&editor, "this is not a diagram", cx);
    cx.run_until_parked();
    assert!(editor.read_with(cx, |e, _| e.error().is_some()));
    set_code(&editor, &"x".repeat(9_000), cx);
    cx.run_until_parked();
    assert!(editor.read_with(cx, |e, _| e.is_cut()));
    draw(cx, &panel);
    // The code is kept whole even though only 8,000 characters are drawn.
    assert_eq!(editor.read_with(cx, |e, cx| e.source(cx).len()), 9_000);
}

#[gpui::test]
fn zoom_steps_and_fit(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(
        editor.read_with(cx, |e, _| e.zoom()),
        None,
        "fit is the default"
    );
    editor.update(cx, |e, cx| e.zoom_to(1.0, None, cx));
    assert_eq!(editor.read_with(cx, |e, _| e.zoom()), Some(1.0));
    editor.update(cx, |e, cx| e.step(1, cx));
    assert_eq!(editor.read_with(cx, |e, _| e.zoom()), Some(1.25));
    editor.update(cx, |e, cx| e.fit(cx));
    assert_eq!(editor.read_with(cx, |e, _| e.zoom()), None);
}

#[test]
fn zoom_steps_run_from_25_to_400_percent() {
    assert_eq!(zoom_step(1.0, 1), 1.25);
    assert_eq!(zoom_step(1.0, -1), 0.75);
    assert_eq!(zoom_step(0.25, -1), 0.25);
    assert_eq!(zoom_step(4.0, 1), 4.0);
    assert_eq!(zoom_step(6.0, 1), 6.0);
    assert_eq!(zoom_step(6.0, -1), 4.0);
    assert_eq!(zoom_step(0.9, 1), 1.0);
}

#[test]
fn the_splitter_percent_is_clamped() {
    assert_eq!(clamp_split_percent(0.0), 15.0);
    assert_eq!(clamp_split_percent(100.0), 85.0);
    assert_eq!(clamp_split_percent(f32::NAN), 50.0);
    assert_eq!(clamp_split_percent(42.0), 42.0);
}

#[test]
fn views_cycle_hidden_split_alone() {
    assert_eq!(DocView::Hidden.cycle(), DocView::Split);
    assert_eq!(DocView::Split.cycle(), DocView::Alone);
    assert_eq!(DocView::Alone.cycle(), DocView::Hidden);
}

#[gpui::test]
fn the_diagram_editor_opens_in_fit_and_keeps_fit_until_zoomed(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_new_diagram(cx));
    draw(cx, &panel);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    let (zoom, scale) = editor.read_with(cx, |e, _| (e.zoom(), e.effective_scale()));
    assert!(scale.is_some(), "drawn");
    assert_eq!(zoom, None, "fit is the default mode");
    editor.update(cx, |e, cx| e.step(1, cx));
    assert!(editor.read_with(cx, |e, _| e.zoom()).is_some());
}

#[gpui::test]
fn the_source_editor_takes_the_focus_when_it_opens(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update_in(cx, |p, window, cx| {
        p.handle_action(
            &application_core::keymap::Action::builtin(SOURCE_ACTION),
            window,
            cx,
        );
    });
    draw(cx, &panel);
    let focus = panel.read_with(cx, |p, cx| p.source_focus_handle(cx));
    assert!(cx.update(|window, _| focus.is_focused(window)));
}

#[gpui::test]
fn a_new_diagram_can_open_on_a_named_template(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_new_diagram_with("Bar chart", cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    let bar = domain::doc::DIAGRAM_TEMPLATES
        .iter()
        .find(|(name, _)| *name == "Bar chart")
        .map(|(_, source)| *source)
        .unwrap();
    assert!(bar.starts_with("xychart-beta"));
    assert_eq!(editor.read_with(cx, |e, cx| e.source(cx)), bar);
    cx.run_until_parked();
    draw(cx, &panel);
    assert_eq!(
        editor.read_with(cx, |e, _| e.error().map(str::to_owned)),
        None
    );
    // The next plain open starts from the first template again.
    panel.update(cx, |p, cx| p.open_new_diagram(cx));
}

// ---- the WYSIWYG page ----

fn page_text(panel: &Entity<DocPanel>, cx: &mut VisualTestContext) -> String {
    panel.read_with(cx, |p, cx| p.page_editor().read(cx).text().to_owned())
}

/// Focuses the page and puts the caret at `at`.
fn caret(panel: &Entity<DocPanel>, at: usize, cx: &mut VisualTestContext) {
    panel.update_in(cx, |p, window, cx| {
        p.focus(window, cx);
        p.page_editor().update(cx, |e, cx| e.set_cursor(at, cx));
    });
    draw(cx, panel);
}

fn saves(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter_map(|e| match e {
            DocPanelEvent::Save { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn show_puts_the_text_in_the_page_editor(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    assert_eq!(page_text(&panel, cx), TEXT);
    assert!(cx.debug_bounds("doc-toolbar").is_some());
}

#[gpui::test]
fn a_key_typed_in_the_page_changes_the_text_and_saves_after_the_pause(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "x\n");
    caret(&panel, 1, cx);
    cx.simulate_input("y");
    cx.run_until_parked();
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "xy\n");
    assert_eq!(
        panel.read_with(cx, |p, _| p.saved().clone()),
        SavedState::Dirty
    );
    assert!(saves(&log).is_empty());
    cx.executor()
        .advance_clock(Duration::from_millis(SAVE_AFTER_MS + 50));
    cx.run_until_parked();
    assert_eq!(saves(&log), vec!["xy\n".to_owned()]);
}

#[gpui::test]
fn bold_on_a_page_selection_wraps_it(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "a bold b\n");
    caret(&panel, 2, cx);
    cx.simulate_keystrokes("shift-right shift-right shift-right shift-right");
    tool(&panel, Tool::Bold, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "a **bold** b\n");
    assert_eq!(page_text(&panel, cx), "a **bold** b\n");
    cx.executor()
        .advance_clock(Duration::from_millis(SAVE_AFTER_MS + 50));
    cx.run_until_parked();
    assert_eq!(saves(&log), vec!["a **bold** b\n".to_owned()]);
}

#[gpui::test]
fn the_bold_action_wraps_the_selection_too(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "a bold b\n");
    caret(&panel, 2, cx);
    cx.simulate_keystrokes("shift-right shift-right shift-right shift-right cmd-b");
    cx.run_until_parked();
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "a **bold** b\n");
}

#[gpui::test]
fn heading_list_and_quote_toggle_the_line_of_the_caret(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "one\n\ntwo words\n");
    caret(&panel, 7, cx);
    tool(&panel, Tool::Heading, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.text()),
        "one\n\n## two words\n"
    );
    assert_eq!(page_text(&panel, cx), "one\n\n## two words\n");
    tool(&panel, Tool::Heading, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "one\n\ntwo words\n");
    tool(&panel, Tool::List, cx);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "one\n\n- two words\n");
}

#[gpui::test]
fn the_table_button_inserts_a_template_block_with_the_caret_in_the_first_cell(
    cx: &mut TestAppContext,
) {
    let (panel, cx, log) = setup(cx, "intro\n");
    caret(&panel, 5, cx);
    tool(&panel, Tool::Table, cx);
    let table = "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |";
    let expected = format!("intro\n\n{table}\n");
    assert_eq!(panel.read_with(cx, |p, _| p.text()), expected);
    assert_eq!(page_text(&panel, cx), expected);
    let cursor = panel.read_with(cx, |p, cx| p.page_editor().read(cx).cursor());
    let cell = expected.find("Column 1").unwrap();
    assert!(
        (cell..=cell + "Column 1".len()).contains(&cursor),
        "caret {cursor} is outside the first header cell"
    );
    cx.executor()
        .advance_clock(Duration::from_millis(SAVE_AFTER_MS + 50));
    cx.run_until_parked();
    assert_eq!(saves(&log), vec![expected]);
}

#[gpui::test]
fn the_link_button_wraps_the_page_selection(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "see docs here\n");
    caret(&panel, 4, cx);
    cx.simulate_keystrokes("shift-right shift-right shift-right shift-right");
    tool(&panel, Tool::Link, cx);
    assert!(panel.read_with(cx, |p, _| p.is_asking_link()));
    panel.update_in(cx, |p, window, cx| {
        p.set_link_address("https://example.com", window, cx);
        p.confirm_link(window, cx);
    });
    assert_eq!(
        panel.read_with(cx, |p, _| p.text()),
        "see [docs](https://example.com) here\n"
    );
}

#[gpui::test]
fn the_source_and_the_page_round_trip_the_text(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    let edited = TEXT.replace("first para", "first para edited");
    panel.update_in(cx, |p, window, cx| {
        p.source
            .update(cx, |s, cx| s.replace_all(edited.clone(), window, cx));
    });
    cx.run_until_parked();
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    assert!(!panel.read_with(cx, |p, _| p.is_source()));
    assert_eq!(page_text(&panel, cx), edited);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), edited);
    // And back: the source shows what the page has.
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    assert_eq!(source_text(&panel, cx), edited);
}

#[gpui::test]
fn a_double_click_inside_a_diagram_opens_the_diagram_editor(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, TEXT);
    let inside = TEXT.find("A --> B").unwrap();
    caret(&panel, inside, cx);
    panel.update_in(cx, |p, window, cx| {
        p.page_mouse_up(
            &MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(5.), px(5.)),
                modifiers: gpui::Modifiers::default(),
                click_count: 2,
            },
            window,
            cx,
        );
    });
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(editor.read_with(cx, |e, cx| e.source(cx)), FLOW);
    set_code(&editor, "flowchart TD\n  X --> Y", cx);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    let text = panel.read_with(cx, |p, _| p.text());
    assert!(
        text.contains("X --> Y") && !text.contains("A --> B"),
        "{text}"
    );
    assert_eq!(page_text(&panel, cx), text);
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
    assert!(saves(&log).iter().any(|t| t.contains("X --> Y")));
}

#[gpui::test]
fn a_click_outside_the_diagrams_does_not_open_the_editor(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    caret(&panel, 3, cx);
    panel.update(cx, |p, cx| p.open_diagram_at_caret(cx));
    draw(cx, &panel);
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
}

#[gpui::test]
fn a_new_diagram_goes_after_the_block_of_the_page_caret(cx: &mut TestAppContext) {
    let start = "# Title\n\nfirst para\n\nlast para\n";
    let (panel, cx, _) = setup(cx, start);
    caret(&panel, 12, cx);
    tool(&panel, Tool::Diagram, cx);
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert_eq!(
        editor.read_with(cx, |e, _| e.spec()),
        DiagramSpec::New { after: Some(1) }
    );
    set_code(&editor, FLOW, cx);
    editor.update(cx, |e, cx| e.save(cx));
    cx.run_until_parked();
    let expected = insert_diagram_after(start, Some(1), FLOW);
    assert_eq!(panel.read_with(cx, |p, _| p.text()), expected);
    assert_eq!(page_text(&panel, cx), expected);
    assert!(!panel.read_with(cx, |p, _| p.is_source()));
}

#[gpui::test]
fn an_image_inserted_in_the_page_is_linked_and_resolved(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "pic:\n");
    caret(&panel, 0, cx);
    let png = base64::engine::general_purpose::STANDARD
        .decode(PNG)
        .unwrap();
    let name = panel.update_in(cx, |p, window, cx| {
        p.insert_image("Shot One.PNG", png.clone(), window, cx)
    });
    assert_eq!(name.as_deref(), Some("shot-one.png"));
    let text = panel.read_with(cx, |p, _| p.text());
    assert!(text.contains("![Shot One](assets/shot-one.png)"), "{text}");
    assert_eq!(page_text(&panel, cx), text);
    assert!(log.borrow().contains(&DocPanelEvent::WriteAsset {
        name: "shot-one.png".into(),
        bytes: png
    }));
    let resolved = panel.read_with(cx, |p, _| {
        p.page
            .shared
            .borrow_mut()
            .picture("assets/shot-one.png")
            .is_some()
    });
    assert!(resolved);
    draw(cx, &panel);
}

#[gpui::test]
fn an_agent_reload_of_the_same_doc_updates_the_page(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "one\n");
    let shown = panel.read_with(cx, |p, _| p.doc().cloned().unwrap());
    panel.update(cx, |p, cx| {
        p.show(shown.clone(), "one\ntwo\n".into(), Vec::new(), cx)
    });
    draw(cx, &panel);
    assert_eq!(page_text(&panel, cx), "one\ntwo\n");
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "one\ntwo\n");
    // The same text again leaves the editor (caret, undo) alone.
    caret(&panel, 2, cx);
    panel.update(cx, |p, cx| {
        p.show(shown, "one\ntwo\n".into(), Vec::new(), cx)
    });
    let at = panel.read_with(cx, |p, cx| p.page_editor().read(cx).cursor());
    assert_eq!(at, 2);
}

#[gpui::test]
fn a_selection_in_the_page_floats_the_bar(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "alpha beta\n");
    caret(&panel, 0, cx);
    cx.simulate_keystrokes("shift-right shift-right shift-right shift-right shift-right");
    panel.update_in(cx, |p, window, cx| {
        p.page_mouse_up(
            &MouseUpEvent {
                button: MouseButton::Left,
                position: point(px(5.), px(5.)),
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
            },
            window,
            cx,
        );
    });
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected_passage().map(str::to_owned)),
        Some("alpha".to_owned())
    );
    panel.update(cx, |p, cx| p.ask_agent(cx));
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, DocPanelEvent::AskAgent { passage, .. } if passage == "alpha"))
    );
}

// ---- tables ----

const TABLE: &str = "| name | note |\n| --- | --- |\n| decision model | x |\n\ntail\n";

fn cursor_of(panel: &Entity<DocPanel>, cx: &mut VisualTestContext) -> usize {
    panel.read_with(cx, |p, cx| p.page_editor().read(cx).cursor())
}

#[gpui::test]
fn space_in_a_table_cell_moves_the_caret_like_in_a_paragraph(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TABLE);
    // Mid-word: `decision| model`.
    let mid = TABLE.find("decision").unwrap() + "decision".len();
    caret(&panel, mid, cx);
    cx.simulate_keystrokes("space");
    assert_eq!(cursor_of(&panel, cx), mid + 1);
    assert!(page_text(&panel, cx).contains("decision  model"));
    cx.simulate_input("z");
    assert!(page_text(&panel, cx).contains("decision z model"));
    assert_eq!(cursor_of(&panel, cx), mid + 2);
    // At the end of the cell: `model|`, a trailing space.
    let end = page_text(&panel, cx).find("model").unwrap() + "model".len();
    caret(&panel, end, cx);
    cx.simulate_keystrokes("space");
    assert_eq!(cursor_of(&panel, cx), end + 1);
    cx.simulate_input("q");
    assert!(page_text(&panel, cx).contains("model q"));
    assert_eq!(cursor_of(&panel, cx), end + 2);
}

#[test]
fn every_starting_point_crest_can_draw_is_offered() {
    let names: Vec<&str> = drawable_templates()
        .iter()
        .map(|&ix| domain::doc::DIAGRAM_TEMPLATES[ix].0)
        .collect();
    let all: Vec<&str> = domain::doc::DIAGRAM_TEMPLATES.iter().map(|t| t.0).collect();
    assert_eq!(all.len(), 6);
    assert_eq!(names, all);
}

#[test]
fn a_starting_point_crest_cannot_draw_is_not_offered() {
    let items = [
        ("Flowchart", "flowchart TD\n  A --> B"),
        ("Bogus", "nonsense diagram"),
    ];
    let colors = crate::theme::default_theme();
    let kept = filter_drawable(&items, |s| {
        crate::blocks::mermaid::diagram::parse_with(s, &colors).is_ok()
    });
    assert_eq!(kept, vec![0]);
}

#[gpui::test]
fn flash_added_lights_a_span_for_a_moment(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    assert_eq!(panel.read_with(cx, |p, _| p.added_range()), None);
    let range = TEXT.len() - 10..TEXT.len() - 1;
    panel.update(cx, |p, cx| p.flash_added(range.clone(), cx));
    draw(cx, &panel);
    assert_eq!(panel.read_with(cx, |p, _| p.added_range()), Some(range));
    // Painting (the wash, the scroll) is not unit-tested; the clock is.
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(40));
        cx.run_until_parked();
    }
    assert!(panel.read_with(cx, |p, _| p.added_range()).is_some());
    for _ in 0..21 {
        cx.executor().advance_clock(Duration::from_millis(40));
        cx.run_until_parked();
    }
    assert_eq!(panel.read_with(cx, |p, _| p.added_range()), None);
}

#[gpui::test]
fn showing_another_doc_clears_the_flash_but_a_reload_keeps_it(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.flash_added(0..5, cx));
    let same = panel.read_with(cx, |p, _| p.doc.clone().expect("shown"));
    panel.update(cx, |p, cx| p.show(same, TEXT.to_owned(), Vec::new(), cx));
    assert_eq!(panel.read_with(cx, |p, _| p.added_range()), Some(0..5));
    panel.update(cx, |p, cx| {
        p.show(doc(), "other\n".to_owned(), Vec::new(), cx)
    });
    assert_eq!(panel.read_with(cx, |p, _| p.added_range()), None);
}

// ---- named actions of the buttons ----

#[gpui::test]
fn every_toolbar_button_has_an_action_that_runs_it(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, "line\n");
    caret(&panel, 0, cx);
    panel.update_in(cx, |p, window, cx| {
        assert!(p.handle_action(&Action::builtin("doc.heading"), window, cx));
    });
    assert_eq!(panel.read_with(cx, |p, _| p.text()), "## line\n");
    for tool in Tool::ALL {
        assert!(
            is_panel_action(tool.action_name()),
            "{}",
            tool.action_name()
        );
    }
    panel.update_in(cx, |p, window, cx| {
        assert!(p.handle_action(&Action::builtin("doc.diagram"), window, cx));
    });
    draw(cx, &panel);
    assert!(panel.read_with(cx, |p, _| p.is_diagram_open()));
}

#[gpui::test]
fn the_head_actions_open_the_rename_field_the_delete_prompt_and_the_switcher(
    cx: &mut TestAppContext,
) {
    let (panel, cx, _) = setup(cx, TEXT);
    let act = |name: &str, cx: &mut VisualTestContext| {
        panel.update_in(cx, |p, window, cx| {
            p.handle_action(&Action::builtin(name), window, cx)
        })
    };
    assert!(act("doc.rename", cx));
    assert!(panel.read_with(cx, |p, _| p.is_renaming()));
    assert!(act("doc.delete", cx));
    assert_eq!(
        panel.read_with(cx, |p, _| p.menu),
        Some(Menu::ConfirmDelete),
        "asks first, deletes nothing"
    );
    // One doc only: the head has no switcher, so the action opens nothing.
    panel.update(cx, |p, _| p.menu = None);
    act("doc.switch", cx);
    assert_eq!(panel.read_with(cx, |p, _| p.menu), None);
    panel.update(cx, |p, _| p.docs = vec![doc(), doc()]);
    act("doc.switch", cx);
    assert_eq!(panel.read_with(cx, |p, _| p.menu), Some(Menu::Switch));
}

#[gpui::test]
fn the_diagram_actions_only_run_in_the_diagram_editor(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    let act = |name: &str, cx: &mut VisualTestContext| {
        panel.update_in(cx, |p, window, cx| {
            p.handle_action(&Action::builtin(name), window, cx)
        })
    };
    assert!(!act("diagram.cancel", cx), "no diagram open");
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    assert!(act("diagram.zoom_in", cx));
    assert!(editor.read_with(cx, |e, _| e.zoom()).is_some());
    assert!(act("diagram.zoom_fit", cx));
    assert_eq!(editor.read_with(cx, |e, _| e.zoom()), None);
    assert!(act("diagram.zoom_out", cx));
    assert!(editor.read_with(cx, |e, _| e.zoom()).is_some());
    assert!(act("diagram.cancel", cx));
    cx.run_until_parked();
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    set_code(&editor_of(&panel, cx), FLOW, cx);
    assert!(act("diagram.save", cx));
    cx.run_until_parked();
    assert!(!panel.read_with(cx, |p, _| p.is_diagram_open()));
}

#[gpui::test]
fn tab_in_the_diagram_code_inserts_two_spaces(cx: &mut TestAppContext) {
    let (panel, cx, _) = setup(cx, TEXT);
    panel.update(cx, |p, cx| p.open_diagram(0, cx));
    draw(cx, &panel);
    let editor = editor_of(&panel, cx);
    set_code(&editor, "A", cx);
    cx.simulate_keystrokes("tab");
    assert_eq!(editor.read_with(cx, |e, cx| e.source(cx)), "  A");
}

// ---- the selection bar over the source ----

#[gpui::test]
fn a_selection_in_the_source_floats_the_bar_and_blanks_clear_it(cx: &mut TestAppContext) {
    let (panel, cx, log) = setup(cx, "alpha beta\n   \n");
    panel.update(cx, |p, cx| p.toggle_source(cx));
    draw(cx, &panel);
    let at = point(px(40.), px(40.));
    let select = |range: std::ops::Range<usize>, cx: &mut VisualTestContext| {
        panel.update(cx, |p, cx| {
            p.source.update(cx, |s, cx| s.set_selected_range(range, cx));
            p.source_selection_changed(at, cx);
        });
    };
    select(6..10, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected_passage().map(str::to_owned)),
        Some("beta".to_owned())
    );
    select(11..14, cx);
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected_passage().map(str::to_owned)),
        None
    );
    select(0..5, cx);
    panel.update(cx, |p, cx| p.ask_agent(cx));
    assert!(
        log.borrow()
            .iter()
            .any(|e| matches!(e, DocPanelEvent::AskAgent { passage, .. } if passage == "alpha"))
    );
    // Leaving the source drops the bar.
    select(0..5, cx);
    panel.update(cx, |p, cx| p.toggle_source(cx));
    assert_eq!(
        panel.read_with(cx, |p, _| p.selected_passage().map(str::to_owned)),
        None
    );
}
