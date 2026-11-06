//! The pointer over a popup row chooses it, as the arrows do.

use application_testing::fs::{InMemoryFs, test_root};

use super::*;
use crate::session::ThemeChoice;

/// The row the popup has chosen: the mode menu's, or the list's.
fn chosen(view: &Entity<InputView>, cx: &mut VisualTestContext) -> usize {
    view.read_with(cx, |view, _| {
        view.mode_control.menu.unwrap_or(view.palette_selected)
    })
}

#[gpui::test]
fn the_pointer_over_a_row_of_any_popup_chooses_it(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let theme = |name: &str| ThemeChoice {
        name: name.into(),
        origin: "file".into(),
        theme: crate::theme::default_theme(),
    };
    let models = (0..3).map(|i| ModelChoice {
        alias: format!("anthropic:m{i}"),
        model: ModelId::new(format!("anthropic:m{i}")),
    });
    let files = InMemoryFs::new(test_root())
        .with_file("a.md", "")
        .with_file("b.md", "")
        .with_file("c.md", "");
    let models: Vec<_> = models.collect();
    view.update(cx, |view, cx| {
        view.set_commands(groups::sample(), cx);
        view.set_file_search(Arc::new(files));
    });
    // (what opens the popup, the id of its rows, how many are checked)
    let table = [
        ("button", "mode-menu", 5),
        ("/", "palette", 3),
        ("/mo", "palette", 1),
        ("@", "path", 3),
        ("/model ", "model", 3),
        ("/theme ", "theme", 3),
    ];
    for (open, id, rows) in table {
        if open == "button" {
            click("prompt-mode-button", cx);
        } else {
            cx.simulate_input(open);
            // What the list needs arrives after the request, as in the app.
            view.update(cx, |view, cx| {
                view.set_models(Ok(models.clone()), cx);
                view.set_themes(Ok(vec![theme("a"), theme("b"), theme("c")]), cx);
            });
        }
        draw(&view, 1280., cx);
        take(&events);
        // Down the list and back up: each move changes the chosen row.
        for ix in (0..rows).rev().chain(0..rows) {
            let selector: &'static str = Box::leak(format!("{id}-{ix}").into_boxed_str());
            let at = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{open:?} row {ix} not drawn"));
            cx.simulate_mouse_move(at.center(), None, gpui::Modifiers::none());
            assert_eq!(chosen(&view, cx), ix, "{open:?} row {ix}");
        }
        cx.simulate_keystrokes("escape");
        view.update_in(cx, |view, window, cx| {
            view.mode_control.menu = None;
            view.set_text("", window, cx);
        });
    }
}

#[gpui::test]
fn a_click_still_picks_the_row_the_pointer_is_on(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    click("prompt-mode-button", cx);
    draw(&view, 1280., cx);
    let at = cx.debug_bounds("mode-menu-2").expect("row drawn");
    cx.simulate_mouse_move(at.center(), None, gpui::Modifiers::none());
    cx.simulate_click(at.center(), gpui::Modifiers::none());
    assert_eq!(mode_of(&view, cx), ChatMode::Deep);
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
}
