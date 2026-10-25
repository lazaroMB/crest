use application::ports::events::DomainEvent;
use application::ports::form::FormRequestId;
use domain::block::Block;
use domain::conversation::ToolUseId;
use futures::FutureExt as _;
use futures::channel::oneshot;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px};
use serde_json::json;

use super::*;
use crate::timeline::TimelineView;

fn spec() -> FormSpec {
    serde_json::from_value(json!({
        "title": "Run settings",
        "description": "How to compare the groups",
        "fields": [
            {"name": "label", "type": "string", "required": true, "label": "Label"},
            {"name": "n", "type": "integer", "min": 1, "max": 9},
            {"name": "paired", "type": "boolean"},
            {"name": "test", "type": "enum", "default": "welch",
             "options": [{"value": "welch", "label": "Welch t"}, {"value": "student"}]},
            {"name": "plots", "type": "multi_enum",
             "options": [{"value": "qq"}, {"value": "hist"}]},
            {"name": "notes", "type": "multiline"}
        ]
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

type Setup<'a> = (&'a mut VisualTestContext, Entity<TimelineView>, Block);

/// A timeline showing the pending form.
fn window(cx: &mut TestAppContext) -> Setup<'_> {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let block = Block::new(BlockKind::Form(spec()));
    view.update(cx, |view, cx| {
        view.apply(
            &DomainEvent::BlockAdded {
                block: block.clone(),
            },
            cx,
        );
    });
    draw(cx, &view);
    (cx, view, block)
}

fn draw(cx: &mut VisualTestContext, view: &Entity<TimelineView>) {
    let view = view.clone();
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(1000.), px(2400.)),
        move |_, _| div().size_full().child(view.clone()),
    );
}

/// Attaches a request for `block` and returns where its answer arrives.
fn ask(
    cx: &mut VisualTestContext,
    view: &Entity<TimelineView>,
    block: &Block,
) -> oneshot::Receiver<FormResponse> {
    let (pending, answer) = PendingForm::new(FormRequest {
        id: FormRequestId(1),
        block_id: block.id,
        tool_use_id: ToolUseId::new("toolu_1"),
        spec: spec(),
        error: None,
    });
    cx.update(|window, cx| attach(pending, window, cx));
    draw(cx, view);
    cx.run_until_parked();
    answer
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} not drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

fn form(cx: &mut VisualTestContext, block: &Block) -> Entity<FormView> {
    cx.update(|_, cx| existing(block.id, cx))
        .unwrap_or_else(|| panic!("no form view"))
}

#[gpui::test]
fn fill_and_submit_with_inline_errors(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let form_view = form(cx, &block);
    // Drawn before any request: nothing to answer yet.
    form_view.read_with(cx, |form, _| assert!(form.request().is_none()));
    let mut answer = ask(cx, &view, &block);
    form_view.read_with(cx, |form, _| assert!(form.request().is_some()));
    assert!(
        cx.update(|window, cx| owns_focus(window, cx)),
        "the form has the focus"
    );

    // The focus is in the label, still empty though required.
    cx.simulate_keystrokes("enter");
    form_view.read_with(cx, |form, _| {
        assert_eq!(
            form.errors().get("label").map(String::as_str),
            Some("Required")
        );
    });
    assert!(cx.debug_bounds("form-field-error").is_some());

    // Tab goes from the label to `n`, typed out of its range.
    draw(cx, &view);
    cx.simulate_input("run 1");
    cx.simulate_keystrokes("tab");
    draw(cx, &view);
    cx.simulate_input("12");
    cx.simulate_keystrokes("enter");
    form_view.read_with(cx, |form, _| {
        assert!(!form.errors().contains_key("label"), "{:?}", form.errors());
        assert_eq!(
            form.errors().get("n").map(String::as_str),
            Some("Enter a whole number between 1 and 9")
        );
    });
    assert_eq!(answer.try_recv().ok().flatten(), None);

    cx.simulate_keystrokes("backspace backspace");
    draw(cx, &view);
    cx.simulate_input("3");
    click(cx, "form-option-2-0");
    click(cx, "form-option-3-1");
    click(cx, "form-option-4-1");
    click(cx, "form-submit");
    cx.run_until_parked();
    let expected: BTreeMap<String, Value> = serde_json::from_value(json!({
        "label": "run 1", "n": 3, "paired": true, "test": "student", "plots": ["hist"]
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        answer.now_or_never(),
        Some(Ok(FormResponse::Submitted {
            answer: expected.clone()
        }))
    );
    form_view.read_with(cx, |form, _| assert!(form.request().is_none()));

    // The tool marks the block submitted: the answers show read-only.
    let mut done = spec();
    done.state = FormState::Submitted;
    done.answer = Some(expected);
    view.update(cx, |view, cx| {
        view.apply(
            &DomainEvent::BlockUpdated {
                block: Block {
                    id: block.id,
                    kind: BlockKind::Form(done),
                },
            },
            cx,
        );
    });
    draw(cx, &view);
    assert!(cx.debug_bounds("form-summary").is_some());
    assert!(cx.debug_bounds("form-submit").is_none());
}

#[gpui::test]
fn escape_and_the_cancel_button_cancel(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let answer = ask(cx, &view, &block);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        answer.now_or_never(),
        Some(Ok(FormResponse::Cancelled { reason: None }))
    );
    assert!(!cx.update(|window, cx| owns_focus(window, cx)));

    let answer = ask(cx, &view, &block);
    click(cx, "form-cancel");
    cx.run_until_parked();
    assert_eq!(
        answer.now_or_never(),
        Some(Ok(FormResponse::Cancelled { reason: None }))
    );

    let mut done = spec();
    done.state = FormState::Cancelled;
    view.update(cx, |view, cx| {
        view.apply(
            &DomainEvent::BlockUpdated {
                block: Block {
                    id: block.id,
                    kind: BlockKind::Form(done),
                },
            },
            cx,
        );
    });
    draw(cx, &view);
    assert!(cx.debug_bounds("form-cancelled").is_some());
}

#[gpui::test]
fn the_port_reaches_the_block_and_a_withdrawn_form_stops_waiting(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let (port, inbox) = crate::form::UiForm::channel();
    let listener = cx.update(|window, cx| crate::form::listen(inbox, window, cx));
    let request = FormRequest {
        id: FormRequestId(7),
        block_id: block.id,
        tool_use_id: ToolUseId::new("toolu_7"),
        spec: spec(),
        error: Some("\"label\" is required".into()),
    };
    let asking = {
        let port = port.clone();
        let request = request.clone();
        cx.executor().spawn(async move {
            use application::ports::form::FormPort as _;
            port.ask(request).await
        })
    };
    cx.run_until_parked();
    let form_view = form(cx, &block);
    form_view.read_with(cx, |form, _| {
        assert_eq!(form.request().map(|r| r.id), Some(FormRequestId(7)));
    });
    // The turn is interrupted: the tool stops waiting.
    drop(asking);
    cx.run_until_parked();
    form_view.read_with(cx, |form, _| assert!(form.request().is_none()));
    draw(cx, &view);
    drop(listener);
}

#[gpui::test]
fn a_clicked_field_keeps_the_focus(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let form_view = form(cx, &block);
    let _answer = ask(cx, &view, &block);
    // Somewhere else first, so the click has to move the focus.
    let timeline = view.read_with(cx, |view, cx| view.focus_handle(cx));
    cx.update(|window, cx| window.focus(&timeline, cx));
    assert!(!cx.update(|window, cx| owns_focus(window, cx)));

    // The click lands in the second field; the timeline row around it
    // must not take the focus back.
    click(cx, "form-input-1");
    draw(cx, &view);
    assert!(
        cx.update(|window, cx| owns_focus(window, cx)),
        "the clicked field has the focus"
    );
    cx.simulate_input("7");
    form_view.read_with(cx, |form, cx| {
        let Some(FieldInput::Line(state)) = form.inputs.get(1) else {
            panic!("no line input for `n`");
        };
        assert_eq!(state.read(cx).value().as_ref(), "7");
    });
}
