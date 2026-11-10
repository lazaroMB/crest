use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use application_core::keymap::Key;
use gpui::{
    InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, Render, ScrollWheelEvent,
    Styled as _, TestAppContext, VisualTestContext, div, px,
};
use gpui_component::v_flex;

use super::completion::{PALETTE_ROWS, scroll_window, window_note};
use super::groups;
use super::palette::{Choice, choice};
use super::*;
use crate::session::ModelChoice;

type Events = Rc<RefCell<Vec<InputEvent>>>;

/// The input at the bottom of a 1280 px wide column, as in the app: what is
/// above it is a spacer, so a popup that pushes or floats shows in the bounds.
struct AtBottom(Entity<InputView>);

impl Render for AtBottom {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(1280.))
            .h_full()
            .child(div().flex_1())
            .child(self.0.clone())
    }
}

fn input(cx: &mut TestAppContext) -> (Entity<InputView>, &mut VisualTestContext, Events) {
    cx.update(gpui_component::init);
    let (root, cx) =
        cx.add_window_view(|window, cx| AtBottom(cx.new(|cx| InputView::new(window, cx))));
    let view = root.read_with(cx, |root, _| root.0.clone());
    let events = Events::default();
    let recorded = events.clone();
    cx.update(|window, cx| {
        cx.subscribe(&view, move |_, event: &InputEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        view.update(cx, |view, cx| view.focus(window, cx));
    });
    cx.run_until_parked();
    (view, cx, events)
}

#[gpui::test]
fn a_click_outside_hides_the_palette(cx: &mut TestAppContext) {
    struct Wrapper(Entity<InputView>);
    impl Render for Wrapper {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(div().id("outside").h(gpui::px(200.)).w_full())
                .child(self.0.clone())
        }
    }
    cx.update(gpui_component::init);
    let (wrapper, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| InputView::new(window, cx));
        Wrapper(input)
    });
    let view = wrapper.read_with(cx, |w, _| w.0.clone());
    view.update_in(cx, |view, window, cx| {
        view.set_commands(vec![row("model", "/model")], cx);
        view.focus(window, cx);
    });
    cx.simulate_input("/mo");
    assert!(view.read_with(cx, |v, cx| !v.palette(cx).is_empty()));
    cx.simulate_click(
        gpui::point(gpui::px(10.), gpui::px(10.)),
        gpui::Modifiers::none(),
    );
    assert!(view.read_with(cx, |v, cx| v.palette(cx).is_empty()));
    cx.update(|window, cx| {
        assert!(view.read(cx).owns_focus(window, cx), "the focus stayed");
    });
}

/// The events so far but the history's [`InputEvent::Recorded`].
fn take(events: &Events) -> Vec<InputEvent> {
    take_all(events)
        .into_iter()
        .filter(|event| !matches!(event, InputEvent::Recorded(_)))
        .collect()
}

fn take_all(events: &Events) -> Vec<InputEvent> {
    events.borrow_mut().drain(..).collect()
}

fn action(name: &str) -> InputEvent {
    InputEvent::Action(Action::builtin(name))
}

fn row(name: &str, usage: &str) -> CommandRow {
    CommandRow {
        name: name.into(),
        usage: usage.into(),
        description: format!("does {name}"),
        source: "crest".into(),
        group: "Chat".into(),
    }
}

#[gpui::test]
fn types_and_submits(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("hola");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("mundo");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "hola\nmundo"));
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&events), [InputEvent::Submit("hola\nmundo".into())]);
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
}

#[gpui::test]
fn plain_keys_are_always_typed(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("jk:?Gi");
    assert!(take(&events).is_empty());
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "jk:?Gi"));
}

#[gpui::test]
fn empty_prompt_is_not_submitted(cx: &mut TestAppContext) {
    let (_view, cx, events) = input(cx);
    cx.simulate_keystrokes("space enter");
    assert!(take(&events).is_empty());
}

#[gpui::test]
fn a_slash_line_is_a_command(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("/model opus");
    cx.simulate_keystrokes("enter");
    // `/model ` asks for the models of its popup; with none listed,
    // Enter sends the line.
    assert_eq!(
        take(&events),
        [
            InputEvent::ModelsWanted,
            InputEvent::Command("model opus".into())
        ]
    );
    cx.simulate_input("//etc/hosts");
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&events), [InputEvent::Submit("/etc/hosts".into())]);
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
}

#[gpui::test]
fn the_palette_completes_and_runs_commands(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    view.update(cx, |view, cx| {
        view.set_commands(
            vec![
                row("clear", "/clear"),
                row("implement", "/implement <spec>"),
                row("help", "/help [section]"),
            ],
            cx,
        );
    });
    cx.simulate_input("/cl");
    view.read_with(cx, |view, cx| assert_eq!(view.palette(cx).len(), 1));
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&events), [InputEvent::Command("clear".into())]);
    // A command that needs an argument is completed, not run.
    cx.simulate_input("/imp");
    cx.simulate_keystrokes("enter");
    assert!(take(&events).is_empty());
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/implement "));
    view.update_in(cx, |view, window, cx| view.set_text("", window, cx));
    // Up/Down choose; Tab completes (a `/` alone lists the groups: Tab opens one).
    cx.simulate_input("/");
    cx.simulate_keystrokes("tab down tab");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/implement "));
    // Esc hides it until the text changes.
    view.update_in(cx, |view, window, cx| view.set_text("/h", window, cx));
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |view, cx| assert!(view.palette(cx).is_empty()));
}

fn entries(view: &Entity<InputView>, cx: &mut VisualTestContext) -> Vec<String> {
    cx.run_until_parked();
    view.read_with(cx, |view, cx| match view.path_popup(cx) {
        Some(PathPopup::Entries(entries)) => entries
            .into_iter()
            .map(|e| {
                if e.dir {
                    format!("{}/", e.name)
                } else {
                    e.name
                }
            })
            .collect(),
        other => panic!("no entries: {other:?}"),
    })
}

fn message(view: &Entity<InputView>, cx: &mut VisualTestContext) -> String {
    cx.run_until_parked();
    view.read_with(cx, |view, cx| match view.path_popup(cx) {
        Some(PathPopup::Message(message)) => message,
        other => panic!("no message: {other:?}"),
    })
}

#[gpui::test]
fn at_words_complete_workspace_paths_level_by_level(cx: &mut TestAppContext) {
    use application_testing::fs::{InMemoryFs, test_root};

    let (view, cx, events) = input(cx);
    let fs = InMemoryFs::new(test_root())
        .with_file("README.md", "")
        .with_file("src/main.rs", "")
        .with_file("src/components/button.rs", "")
        .with_file(".env", "SECRET=1");
    view.update(cx, |view, cx| {
        view.set_commands(vec![row("open", "/open @<path>")], cx);
        view.set_file_search(Arc::new(fs));
    });
    // The palette completes `/open` up to the `@`, which lists the root.
    cx.simulate_input("/op");
    cx.simulate_keystrokes("enter");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/open @"));
    assert_eq!(entries(&view, cx), ["src/", "README.md"]);
    cx.simulate_input("s");
    assert_eq!(entries(&view, cx), ["src/"]);
    cx.simulate_keystrokes("tab");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/open @src/"));
    assert_eq!(entries(&view, cx), ["components/", "main.rs"]);
    cx.simulate_keystrokes("enter");
    view.read_with(cx, |view, cx| {
        assert_eq!(view.text(cx), "/open @src/components/");
    });
    assert_eq!(entries(&view, cx), ["button.rs"]);
    // Enter completes the file, then submits the line.
    cx.simulate_keystrokes("enter");
    view.read_with(cx, |view, cx| {
        assert_eq!(view.text(cx), "/open @src/components/button.rs ");
        assert_eq!(view.path_popup(cx), None);
    });
    assert!(take(&events).is_empty());
    cx.simulate_keystrokes("enter");
    assert_eq!(
        take(&events),
        [InputEvent::Command("open @src/components/button.rs".into())]
    );
    // Unmatched prefixes and paths out of the root only get a message.
    cx.simulate_input("look at @zz");
    assert_eq!(message(&view, cx), "nothing in the workspace matches `zz`");
    view.update_in(cx, |view, window, cx| view.set_text("@../", window, cx));
    assert!(message(&view, cx).contains("inside the workspace"));
    view.update_in(cx, |view, window, cx| view.set_text("@/etc/", window, cx));
    assert!(message(&view, cx).contains("inside the workspace"));
    // Up/Down move the choice; Esc hides the popup.
    view.update_in(cx, |view, window, cx| view.set_text("@", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("down tab");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "@README.md "));
    view.update_in(cx, |view, window, cx| view.set_text("@", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |view, cx| assert_eq!(view.path_popup(cx), None));
}

fn clipboard_png(width: u32, cx: &mut VisualTestContext) {
    let png = attachments::tests::encoded(width, 2, image::ImageFormat::Png);
    let item =
        gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(gpui::ImageFormat::Png, png));
    cx.write_to_clipboard(item);
}

#[gpui::test]
fn pasted_images_wait_in_the_tray_until_sent(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("describe");
    clipboard_png(3, cx);
    cx.simulate_keystrokes("secondary-v");
    clipboard_png(5, cx);
    cx.simulate_keystrokes("secondary-v");
    // They are prepared in the background.
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        // The typed text stays; the images are in the tray.
        assert_eq!(view.text(cx), "describe");
        let widths: Vec<u32> = view.images().iter().map(|i| i.attachment.width).collect();
        assert_eq!(widths, [3, 5]);
        assert_eq!(view.image_mode(), ImageMode::Model);
    });
    // Text still pastes as text.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(" it".into()));
    cx.simulate_keystrokes("secondary-v");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "describe it"));
    // A command leaves the images for the next prompt.
    view.update_in(cx, |view, window, cx| {
        view.set_text("/mode ask", window, cx)
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&events), [InputEvent::Command("mode ask".into())]);
    view.read_with(cx, |view, _| assert_eq!(view.images().len(), 2));
    // Backspace in an empty prompt removes the last one; `<leader>i` switches
    // the mode, which the submission carries.
    cx.simulate_keystrokes("backspace");
    view.read_with(cx, |view, _| assert_eq!(view.images().len(), 1));
    cx.simulate_keystrokes("ctrl-g i");
    view.read_with(cx, |view, _| {
        assert_eq!(view.image_mode(), ImageMode::ChatOnly)
    });
    cx.simulate_keystrokes("ctrl-g i");
    cx.simulate_keystrokes("enter");
    let sent = take(&events);
    let [InputEvent::SubmitImages { text, images, mode }] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!((text.as_str(), *mode), ("", ImageMode::Model));
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].media_type, "image/png");
    view.read_with(cx, |view, _| assert!(view.images().is_empty()));
    // An image that cannot be attached is reported.
    clipboard_png(domain::conversation::MAX_IMAGE_SIDE + 1, cx);
    cx.simulate_keystrokes("secondary-v");
    cx.run_until_parked();
    assert!(matches!(take(&events).as_slice(), [InputEvent::Notice(m)] if m.contains("px")));
    view.read_with(cx, |view, _| assert!(view.images().is_empty()));
}

#[gpui::test]
fn a_prompt_sent_during_a_paste_waits_for_its_images(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let png = |width| {
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            attachments::tests::encoded(width, 2, image::ImageFormat::Png),
        )
    };
    // Pasted and sent before the background preparation lands.
    view.update_in(cx, |view, window, cx| {
        view.attach_images(vec![png(3)], cx);
        view.set_text("what is this", window, cx);
        view.submit(window, cx);
        // The prompt left the editor; a paste after `Enter` is for the
        // next prompt.
        assert_eq!(view.text(cx), "");
        assert!(view.pastes_pending());
        view.attach_images(vec![png(5)], cx);
    });
    assert_eq!(take(&events), [], "nothing is sent before the image lands");
    cx.run_until_parked();
    let sent = take(&events);
    let [InputEvent::SubmitImages { text, images, mode }] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!((text.as_str(), *mode), ("what is this", ImageMode::Model));
    assert_eq!(
        images.iter().map(|image| image.width).collect::<Vec<_>>(),
        [3]
    );
    view.read_with(cx, |view, _| {
        assert!(!view.pastes_pending());
        let widths: Vec<u32> = view.images().iter().map(|i| i.attachment.width).collect();
        assert_eq!(widths, [5]);
    });
    // Without pending pastes `Enter` sends at once, and an empty prompt
    // with nothing pasted sends nothing.
    view.update_in(cx, |view, window, cx| {
        view.remove_image(0, cx);
        view.submit(window, cx);
        view.set_text("hi", window, cx);
        view.submit(window, cx);
    });
    let sent = take(&events);
    assert_eq!(sent, [InputEvent::Submit("hi".into())]);
}

#[gpui::test]
fn a_mode_switch_during_a_paste_applies_once_the_images_land(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let png = |width| {
        gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            attachments::tests::encoded(width, 2, image::ImageFormat::Png),
        )
    };
    // `<leader>i` right after the paste, before the image is in the tray.
    view.update(cx, |view, cx| {
        view.attach_images(vec![png(3)], cx);
        view.toggle_image_mode(cx);
        assert!(view.images().is_empty());
        assert_eq!(view.image_mode(), ImageMode::Model);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(view.images().len(), 1);
        assert_eq!(view.image_mode(), ImageMode::ChatOnly);
    });
    // Switches and a prompt sent during a later paste land in order: the
    // prompt goes with the mode switched twice more, back and forth again.
    view.update_in(cx, |view, window, cx| {
        view.attach_images(vec![png(5)], cx);
        view.toggle_image_mode(cx);
        view.toggle_image_mode(cx);
        view.set_text("keep these", window, cx);
        view.submit(window, cx);
        view.toggle_image_mode(cx);
    });
    cx.run_until_parked();
    let sent = take(&events);
    let [InputEvent::SubmitImages { text, images, mode }] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!((text.as_str(), *mode), ("keep these", ImageMode::ChatOnly));
    assert_eq!(images.len(), 2);
    // The switch after `Enter` found an empty tray: nothing to switch.
    view.read_with(cx, |view, _| {
        assert!(!view.pastes_pending());
        assert!(view.images().is_empty());
        assert_eq!(view.image_mode(), ImageMode::Model);
    });
}

#[gpui::test]
fn the_tray_buttons_remove_images_and_switch_the_mode(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    clipboard_png(4, cx);
    cx.simulate_keystrokes("secondary-v");
    cx.run_until_parked();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(gpui::px(800.), gpui::px(400.)),
        |_, _| div().size_full().child(view.clone()),
    );
    let chat = cx.debug_bounds("tray-chat-only").expect("mode chip drawn");
    cx.simulate_click(chat.center(), gpui::Modifiers::none());
    view.read_with(cx, |view, _| {
        assert_eq!(view.image_mode(), ImageMode::ChatOnly)
    });
    let remove = cx.debug_bounds("tray-remove-0").expect("× drawn");
    cx.simulate_click(remove.center(), gpui::Modifiers::none());
    view.read_with(cx, |view, _| {
        assert!(view.images().is_empty());
        assert_eq!(view.image_mode(), ImageMode::Model);
    });
}

#[gpui::test]
fn leader_and_chords_fire_actions(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_keystrokes("ctrl-g e ctrl-g k ctrl-g j ctrl-g g g ctrl-g shift-g ctrl-c");
    assert_eq!(
        take(&events),
        [
            action("panel.tree"),
            action("block.prev"),
            action("block.next"),
            action("block.first"),
            action("block.last"),
            action("turn.interrupt")
        ]
    );
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
}

#[gpui::test]
fn ctrl_c_clears_the_prompt_and_up_brings_it_back(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let text = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.text(cx));
    cx.simulate_input("a medio escribir");
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(text(cx), "");
    assert_eq!(take_all(&events), [InputEvent::Cleared], "not recorded");
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "a medio escribir");
    cx.simulate_keystrokes("ctrl-c ctrl-c");
    assert_eq!(text(cx), "");
    assert_eq!(
        take(&events),
        [InputEvent::Cleared, action("turn.interrupt")]
    );
}

#[gpui::test]
fn ctrl_c_copies_the_selection(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("copiame");
    // Select all is Cmd+A on macOS (Ctrl+A goes to the line start).
    cx.simulate_keystrokes("secondary-a ctrl-c");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("copiame"));
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "copiame"));
    assert!(take(&events).is_empty());
}

fn commands(n: usize) -> Vec<CommandRow> {
    (0..n)
        .map(|i| row(&format!("cmd{i:02}"), &format!("/cmd{i:02}")))
        .collect()
}

#[test]
fn popup_windows_say_what_is_left_and_scroll_with_the_choice() {
    assert_eq!(window_note(0, 8), None);
    assert_eq!(
        window_note(0, 20).as_deref(),
        Some(" · 1–8 of 20 · 12 more")
    );
    assert_eq!(window_note(12, 20).as_deref(), Some(" · 13–20 of 20"));
    // Down by 3: the choice at 1 is pulled to the window's first row.
    assert_eq!(scroll_window(0, 1, 3, 20), (3, 3));
    // Up past the top, and down past the end, stop there.
    assert_eq!(scroll_window(2, 5, -9, 20), (0, 5));
    assert_eq!(scroll_window(10, 15, 9, 20), (12, 15));
}

#[gpui::test]
fn arrows_reach_the_last_command_of_the_palette(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    view.update(cx, |view, cx| view.set_commands(commands(20), cx));
    cx.simulate_input("/cmd");
    let total = view.read_with(cx, |view, cx| view.palette(cx).len());
    assert!(total > PALETTE_ROWS, "more commands than rows: {total}");
    let last = view.read_with(cx, |view, cx| view.palette(cx)[total - 1].name.clone());
    cx.simulate_keystrokes(&vec!["down"; total - 1].join(" "));
    view.read_with(cx, |view, _| {
        assert_eq!(view.palette_selected, total - 1);
        assert_eq!(view.palette_top, total - PALETTE_ROWS, "scrolled");
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let drawn = cx.debug_bounds("input-popups").is_some();
    assert!(drawn);
    // Down wraps to the first.
    cx.simulate_keystrokes("down");
    view.read_with(cx, |view, _| {
        assert_eq!((view.palette_selected, view.palette_top), (0, 0));
    });
    cx.simulate_keystrokes("up tab");
    let text = view.read_with(cx, |view, cx| view.text(cx));
    assert!(text.starts_with(&format!("/{last}")), "{text} for {last}");
    assert!(take(&events).is_empty());
}

#[gpui::test]
fn the_wheel_scrolls_the_palette(cx: &mut TestAppContext) {
    use gpui::{Modifiers, ScrollDelta, TouchPhase, point};
    let (view, cx, _) = input(cx);
    view.update(cx, |view, cx| view.set_commands(commands(20), cx));
    cx.simulate_input("/cmd");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let at = cx
        .debug_bounds("input-popups")
        .expect("the palette is drawn")
        .center();
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), px(-3. * WHEEL_ROW))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    view.read_with(cx, |view, _| {
        assert_eq!(view.palette_top, 3, "three rows down");
        assert_eq!(view.palette_selected, 3, "the choice came along");
    });
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Lines(point(0., 1.)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    view.read_with(cx, |view, _| assert_eq!(view.palette_top, 2, "a line up"));
}

#[gpui::test]
fn arrows_reach_every_model_without_typing(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let models: Vec<ModelChoice> = (0..12)
        .map(|i| ModelChoice {
            alias: format!("anthropic:m{i}"),
            model: ModelId::new(format!("anthropic:m{i}")),
        })
        .collect();
    view.update(cx, |view, cx| view.set_models(Ok(models), cx));
    cx.simulate_input("/model ");
    cx.simulate_keystrokes(&["down"; 11].join(" "));
    view.read_with(cx, |view, _| assert_eq!(view.palette_top, 4, "scrolled"));
    cx.simulate_keystrokes("enter");
    assert_eq!(
        take(&events),
        [InputEvent::Command("model set anthropic:m11".into())]
    );
    cx.simulate_input("/model ");
    cx.simulate_keystrokes("up enter");
    assert_eq!(
        take(&events),
        [InputEvent::Command("model set anthropic:m11".into())]
    );
}

#[gpui::test]
fn pending_leader_times_out(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_keystrokes("ctrl-g");
    view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), "<C-g>"));
    cx.executor().advance_clock(Duration::from_millis(1100));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), ""));
    cx.simulate_keystrokes("e");
    assert!(
        take(&events).is_empty(),
        "the timed-out leader does not pair up"
    );
}

#[gpui::test]
fn up_and_down_browse_the_sent_lines_and_return_to_the_draft(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_input("uno");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("/model opus");
    cx.simulate_keystrokes("enter");
    let recorded: Vec<_> = take_all(&events)
        .into_iter()
        .filter(|event| matches!(event, InputEvent::Recorded(_)))
        .collect();
    assert_eq!(
        recorded,
        [
            InputEvent::Recorded("uno".into()),
            InputEvent::Recorded("/model opus".into())
        ]
    );
    let text = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.text(cx));
    cx.simulate_input("bor");
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "/model opus");
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "uno");
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "uno", "the oldest line stays");
    cx.simulate_keystrokes("down");
    assert_eq!(text(cx), "/model opus");
    cx.simulate_keystrokes("down");
    assert_eq!(text(cx), "bor", "the draft comes back");
    assert!(take(&events).is_empty());
}

#[gpui::test]
fn up_moves_the_cursor_until_the_first_line(cx: &mut TestAppContext) {
    let (view, cx, _) = input(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, _| view.set_history(vec!["antes".into()]));
        view.update(cx, |view, cx| view.set_text("a\nb", window, cx));
    });
    let text = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.text(cx));
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "a\nb", "the cursor went to the first line");
    cx.simulate_keystrokes("up");
    assert_eq!(text(cx), "antes");
    cx.simulate_keystrokes("down");
    assert_eq!(text(cx), "a\nb");
}

#[gpui::test]
fn a_queued_prompt_comes_back_before_the_history(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    view.update(cx, |view, _| {
        view.set_history(vec!["antes".into()]);
        view.set_queued(true);
    });
    cx.simulate_keystrokes("up");
    assert_eq!(take(&events), [action(QUEUE_EDIT_ACTION)]);
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
}

#[gpui::test]
fn up_takes_back_a_queued_prompt_only_from_an_empty_prompt(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    cx.simulate_keystrokes("up");
    assert!(take(&events).is_empty(), "nothing is queued");
    view.update(cx, |view, _| view.set_queued(true));
    cx.simulate_input("x");
    cx.simulate_keystrokes("up");
    assert!(take(&events).is_empty(), "the prompt has text");
    cx.simulate_keystrokes("backspace up");
    assert_eq!(take(&events), [action(QUEUE_EDIT_ACTION)]);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.set_text("de vuelta", window, cx));
    });
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "de vuelta"));
}

#[gpui::test]
fn the_cursor_blinks_and_stays_on_while_typing(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    let visible = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.cursor_visible());
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    assert!(visible(cx));
    cx.executor().advance_clock(BLINK_INTERVAL);
    cx.run_until_parked();
    assert!(!visible(cx), "blinked off");
    cx.simulate_input("a");
    assert!(visible(cx), "a key turns it on");
    cx.executor().advance_clock(BLINK_INTERVAL / 2);
    cx.run_until_parked();
    assert!(visible(cx), "still typing");
    cx.executor().advance_clock(BLINK_INTERVAL / 2);
    cx.run_until_parked();
    assert!(!visible(cx), "blinks again a whole interval after the key");
}

#[test]
fn the_placeholder_names_the_picker_keys() {
    assert_eq!(
        prompt_placeholder(&default_keymap()),
        "Ask crest…  (/ for commands · Ctrl+G ff files · Ctrl+G fg text)"
    );
    assert_eq!(
        prompt_placeholder(&Keymap::new()),
        "Ask crest…  (/ for commands)"
    );
}

#[test]
fn lines_are_prompts_or_commands() {
    assert_eq!(classify_line("  "), None);
    assert_eq!(classify_line("/"), None);
    assert_eq!(classify_line("hola"), Some(Line::Prompt("hola".into())));
    assert_eq!(
        classify_line(" /mode plan "),
        Some(Line::Command("mode plan".into()))
    );
    assert_eq!(classify_line("//x"), Some(Line::Prompt("/x".into())));
    assert_eq!(
        classify_line("/!ls -la"),
        Some(Line::Command("!ls -la".into()))
    );
    assert_eq!(
        classify_line("  ! cat ~/file  "),
        Some(Line::Command("! cat ~/file".into()))
    );
    assert_eq!(classify_line("!ls"), Some(Line::Command("!ls".into())));
}

#[test]
fn palette_choices_run_fixed_words_and_complete_arguments() {
    assert_eq!(choice(&row("clear", "/clear")), Choice::Run("clear".into()));
    assert_eq!(
        choice(&row("help", "/help [section|search]")),
        Choice::Run("help".into())
    );
    assert_eq!(
        choice(&row("plugin", "/plugin list")),
        Choice::Run("plugin list".into())
    );
    assert_eq!(
        choice(&row("plugin", "/plugin trust <name>")),
        Choice::Complete("/plugin trust ".into())
    );
    assert_eq!(
        choice(&row("!", "/! <command>")),
        Choice::Complete("/! ".into())
    );
    assert_eq!(
        choice(&row("!", "! <command>")),
        Choice::Complete("/! ".into())
    );
}

#[test]
fn the_palette_prefers_prefixes() {
    let rows = [
        row("research", "/research"),
        row("clear", "/clear"),
        row("mcp", "/mcp"),
    ];
    let names = |text| -> Vec<&str> {
        palette_matches(&rows, text)
            .iter()
            .map(|r| r.name.as_str())
            .collect()
    };
    assert_eq!(
        names("/"),
        ["research", "clear", "mcp"],
        "the rows behind the groups"
    );
    assert_eq!(names("/c"), ["clear", "research", "mcp"]);
    assert_eq!(names("/re"), ["research"]);
    assert!(names("/research x").is_empty());
    assert!(names("hola").is_empty());
    assert!(names("//").is_empty());
}

fn key(notation: &str) -> Option<Key> {
    let keystroke = Keystroke::parse(notation)
        .unwrap_or_else(|err| panic!("{err}"))
        .with_simulated_ime();
    key_from_keystroke(&keystroke)
}

fn parsed(notation: &str) -> Option<Key> {
    application_core::keymap::parse_keys(notation, " ")
        .ok()
        .and_then(|keys| keys.first().copied())
}

#[test]
fn printable_keys_use_the_typed_character() {
    assert_eq!(key("g"), parsed("g"));
    assert_eq!(key("shift-g"), parsed("G"));
    assert_eq!(key(":"), parsed(":"));
    assert_eq!(key("space"), parsed("<Space>"));
}

#[test]
fn named_and_chorded_keys_keep_modifiers() {
    assert_eq!(key("enter"), parsed("<CR>"));
    assert_eq!(key("shift-enter"), parsed("<S-CR>"));
    assert_eq!(key("escape"), parsed("<Esc>"));
    assert_eq!(key("ctrl-c"), parsed("<C-c>"));
    assert_eq!(key("ctrl-insert"), parsed("<C-Insert>"));
    assert_eq!(key("f5"), parsed("<F5>"));
}

#[test]
fn bare_modifiers_are_not_keys() {
    assert_eq!(key("shift"), None);
}

fn listed(view: &Entity<InputView>, cx: &mut VisualTestContext) -> (Vec<String>, Vec<String>) {
    view.read_with(cx, |view, cx| {
        let groups = view
            .palette_groups(cx)
            .iter()
            .map(|g| format!("{} {}", g.label, g.count))
            .collect();
        (
            groups,
            view.palette(cx).iter().map(|r| r.name.clone()).collect(),
        )
    })
}

#[gpui::test]
fn a_slash_alone_lists_groups_and_a_group_lists_its_commands(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    view.update(cx, |view, cx| view.set_commands(groups::sample(), cx));
    cx.simulate_input("/");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("input-popups").is_some(), "drawn");
    let (groups, commands) = listed(&view, cx);
    let wanted = ["Chat 2", "Model & permissions 1", "Workspace 2", "App 1"];
    assert_eq!(groups[..4], wanted);
    assert_eq!(groups[4], "Plugin · jira-tools 2");
    assert!(commands.is_empty());
    // Down to the second group, Right opens it; Left goes back with it chosen.
    cx.simulate_keystrokes("down right");
    assert_eq!(listed(&view, cx), (vec![], vec!["mode".to_string()]));
    cx.simulate_keystrokes("left");
    assert_eq!(listed(&view, cx).0.len(), 5);
    view.read_with(cx, |view, _| assert_eq!(view.palette_selected, 1));
    // Enter opens a group, Backspace goes back and keeps the slash.
    cx.simulate_keystrokes("enter backspace");
    assert_eq!(listed(&view, cx).0.len(), 5);
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/"));
    // On the groups, Backspace deletes the slash.
    cx.simulate_keystrokes("backspace");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
    // A command of an opened group runs as it does anywhere.
    cx.simulate_input("/");
    cx.simulate_keystrokes("tab enter");
    assert_eq!(take(&events), [InputEvent::Command("clear".into())]);
}

#[gpui::test]
fn typing_after_the_slash_searches_every_group(cx: &mut TestAppContext) {
    let (view, cx, _) = input(cx);
    view.update(cx, |view, cx| view.set_commands(groups::sample(), cx));
    cx.simulate_input("/mo");
    assert_eq!(listed(&view, cx), (vec![], vec!["mode".to_string()]));
    view.update_in(cx, |view, window, cx| view.set_text("/mode ", window, cx));
    assert_eq!(listed(&view, cx).1, ["mode plan"]);
    // Opening a group and typing more leaves the group.
    view.update_in(cx, |view, window, cx| view.set_text("/", window, cx));
    cx.simulate_keystrokes("right");
    cx.simulate_input("h");
    assert_eq!(listed(&view, cx).1, ["help"]);
}

/// Draws the window again (the clicks and keys of the test draw it too).
fn draw(_: &Entity<InputView>, _: f32, cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn set_mode(view: &Entity<InputView>, mode: ChatMode, cx: &mut VisualTestContext) {
    view.update_in(cx, |view, window, cx| view.set_chat_mode(mode, window, cx));
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds(selector).expect("drawn");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
}

fn mode_of(view: &Entity<InputView>, cx: &mut VisualTestContext) -> ChatMode {
    view.read_with(cx, |view, _| view.chat_mode())
}

#[gpui::test]
fn a_mode_sends_its_command_and_stays_until_changed_back(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let command = |line: &str| InputEvent::Command(line.into());
    // (mode, typed, what the input emits)
    let table = [
        (ChatMode::Research, "why", Some(command("research why"))),
        (ChatMode::Deep, "why", Some(command("research --deep why"))),
        (ChatMode::Deep, "!ls", Some(command("!ls"))),
        (ChatMode::Deep, "/help", Some(command("help"))),
        (ChatMode::Research, "", None),
        (
            ChatMode::Chat,
            "why",
            Some(InputEvent::Submit("why".into())),
        ),
    ];
    for (mode, typed, sent) in table {
        set_mode(&view, mode, cx);
        cx.simulate_input(typed);
        cx.simulate_keystrokes("enter");
        assert_eq!(
            take(&events),
            sent.into_iter().collect::<Vec<_>>(),
            "{mode:?} {typed:?}"
        );
        assert_eq!(mode_of(&view, cx), mode, "the mode stays after a send");
    }
    // The history keeps what was typed.
    view.read_with(cx, |view, _| {
        assert_eq!(view.history(), ["why", "!ls", "/help", "why"]);
    });
}

#[gpui::test]
fn the_x_backspace_and_chat_in_the_menu_go_back_to_chat(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    set_mode(&view, ChatMode::Deep, cx);
    draw(&view, 1280., cx);
    click("prompt-mode-clear", cx);
    assert_eq!(mode_of(&view, cx), ChatMode::Chat);
    set_mode(&view, ChatMode::Research, cx);
    cx.simulate_input("x");
    cx.simulate_keystrokes("backspace");
    assert_eq!(
        mode_of(&view, cx),
        ChatMode::Research,
        "the prompt had text"
    );
    cx.simulate_keystrokes("backspace");
    assert_eq!(mode_of(&view, cx), ChatMode::Chat);
    set_mode(&view, ChatMode::Deep, cx);
    draw(&view, 1280., cx);
    click("prompt-mode-button", cx);
    draw(&view, 1280., cx);
    click("mode-menu-0", cx);
    assert_eq!(mode_of(&view, cx), ChatMode::Chat);
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
}

#[gpui::test]
fn the_button_opens_a_keyboard_menu_of_modes_and_inserts(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    draw(&view, 1280., cx);
    click("prompt-mode-button", cx);
    draw(&view, 1280., cx);
    for (ix, row) in ROWS.into_iter().enumerate() {
        assert!(cx.debug_bounds(row).is_some(), "row {ix}");
    }
    cx.simulate_keystrokes("down down enter");
    assert_eq!(mode_of(&view, cx), ChatMode::Deep);
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
    click("prompt-mode-button", cx);
    cx.simulate_keystrokes("escape");
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
    assert_eq!(mode_of(&view, cx), ChatMode::Deep, "Esc changes nothing");
    draw(&view, 1280., cx);
    // The inserts type their character, which opens its completion.
    click("prompt-mode-button", cx);
    assert!(view.read_with(cx, |view, _| view.mode_menu_open()), "open");
    cx.simulate_keystrokes("down down enter");
    view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/"));
}

#[gpui::test]
fn prompt_mode_cycles_and_the_placeholder_follows(cx: &mut TestAppContext) {
    let (view, cx, events) = input(cx);
    let keymap = default_keymap();
    let hints = "  (/ for commands · Ctrl+G ff files · Ctrl+G fg text)";
    for mode in [ChatMode::Research, ChatMode::Deep, ChatMode::Chat] {
        cx.simulate_keystrokes("ctrl-g shift-m");
        assert_eq!(mode_of(&view, cx), mode);
        assert_eq!(
            placeholder_with(mode, &keymap),
            format!("{}{hints}", mode.placeholder())
        );
    }
    assert!(
        take(&events).is_empty(),
        "the input handles prompt.mode itself"
    );
}

#[gpui::test]
fn the_button_and_the_chip_take_at_most_160_px_at_1280(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    for mode in ChatMode::ALL {
        set_mode(&view, mode, cx);
        draw(&view, 1280., cx);
        let control = cx.debug_bounds("prompt-mode-control").expect("drawn");
        let width = f32::from(control.size.width);
        assert!(
            width <= mode_view::CONTROL_MAX_PX,
            "{mode:?} takes {width} px"
        );
        let chip = cx
            .debug_bounds("prompt-mode-chip")
            .map(|b| f32::from(b.size.width));
        assert_eq!(chip.is_some(), mode != ChatMode::Chat);
    }
}

const ROWS: [&str; 5] = [
    "mode-menu-0",
    "mode-menu-1",
    "mode-menu-2",
    "mode-menu-3",
    "mode-menu-4",
];

#[gpui::test]
fn the_open_menu_floats_above_the_prompt_and_adds_no_height(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    draw(&view, 1280., cx);
    let closed = cx.debug_bounds("crest-input").expect("drawn");
    click("prompt-mode-button", cx);
    draw(&view, 1280., cx);
    let open = cx.debug_bounds("crest-input").expect("drawn");
    assert_eq!(open, closed, "the menu takes no room in the layout");
    let row = cx.debug_bounds(ROWS[0]).expect("menu drawn");
    let control = cx.debug_bounds("prompt-mode-control").expect("drawn");
    assert!(
        row.bottom() <= control.top(),
        "the menu is above the prompt row"
    );
    assert!(
        row.top() < open.top(),
        "and it reaches over what is above the input"
    );
}

#[gpui::test]
fn a_click_away_closes_the_floating_menu(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    click("prompt-mode-button", cx);
    cx.simulate_click(gpui::point(px(900.), px(100.)), gpui::Modifiers::none());
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
}

#[gpui::test]
fn every_popup_floats_above_the_prompt_and_adds_no_height(cx: &mut TestAppContext) {
    use crate::session::ThemeChoice;
    use application_testing::fs::{InMemoryFs, test_root};

    let (view, cx, _events) = input(cx);
    let models = (0..12).map(|i| ModelChoice {
        alias: format!("anthropic:m{i}"),
        model: ModelId::new(format!("anthropic:m{i}")),
    });
    let theme = |name: &str| ThemeChoice {
        name: name.into(),
        origin: "file".into(),
        theme: crate::theme::default_theme(),
    };
    view.update(cx, |view, cx| {
        view.set_commands(groups::sample(), cx);
        view.set_file_search(Arc::new(InMemoryFs::new(test_root()).with_file("a.md", "")));
        view.set_models(Ok(models.collect()), cx);
        view.set_themes(Ok(vec![theme("a"), theme("b")]), cx);
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let closed = cx.debug_bounds("crest-input").expect("drawn");
    // (what is typed: the palette, the groups, `@`, models, themes, which-key)
    for typed in ["/mo", "/", "@", "/model ", "/theme ", "ctrl-g"] {
        if typed == "ctrl-g" {
            cx.simulate_keystrokes(typed);
        } else {
            cx.simulate_input(typed);
        }
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let open = cx.debug_bounds("crest-input").expect("drawn");
        // The line under the prompt that echoes a pending prefix is not a popup.
        if typed != "ctrl-g" {
            assert_eq!(open, closed, "{typed:?} takes no room in the layout");
        }
        let popups = cx.debug_bounds("input-popups").expect("popup drawn");
        assert!(
            popups.bottom() <= open.top(),
            "{typed:?} is above the input"
        );
        cx.simulate_keystrokes("escape");
        view.update_in(cx, |view, window, cx| view.set_text("", window, cx));
    }
}

#[gpui::test]
fn the_mode_menu_and_a_popup_are_never_open_together(cx: &mut TestAppContext) {
    let (view, cx, _events) = input(cx);
    view.update(cx, |view, cx| view.set_commands(groups::sample(), cx));
    click("prompt-mode-button", cx);
    cx.simulate_input("/");
    assert!(view.read_with(cx, |view, _| !view.mode_menu_open()));
    assert!(view.read_with(cx, |view, cx| !view.palette_groups(cx).is_empty()));
}

#[cfg(test)]
mod hover;
