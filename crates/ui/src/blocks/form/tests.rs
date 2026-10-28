use application::ports::events::DomainEvent;
use application::ports::form::FormRequestId;
use chrono::NaiveDate;
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
            {"name": "notes", "type": "multiline"},
            {"name": "due", "type": "date", "min": "2024-01-01", "max": "2024-12-31"},
            {"name": "stay", "type": "date_range", "min": "2026-01-01", "max": "2026-12-31"}
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
    cx.update(|window, cx| attach(pending, true, window, cx));
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
fn a_click_on_the_form_keeps_the_focus_in_it(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let _answer = ask(cx, &view, &block);
    let form_view = form(cx, &block);
    let card = form_view.read_with(cx, |form, _| form.focus_handle.clone());

    // A click on an input: the input takes the focus on mouse down and
    // the timeline row behind it must not take it back on mouse up.
    click(cx, "form-input-0");
    cx.run_until_parked();
    assert!(
        cx.update(|window, cx| owns_focus(window, cx)),
        "the form keeps the focus after a click on an input"
    );
    cx.simulate_input("a");
    form_view.read_with(cx, |form, cx| {
        let typed = form.read(cx).ok().and_then(|a| a.get("label").cloned());
        assert_eq!(typed, Some(json!("a")), "the key reached the input");
    });

    // The focus elsewhere, then a click on the card (its title): the
    // first input takes it.
    cx.update(|window, cx| {
        let other = cx.focus_handle();
        window.focus(&other, cx);
    });
    assert!(!cx.update(|window, cx| card.contains_focused(window, cx)));
    click(cx, "form-card");
    cx.run_until_parked();
    assert!(
        cx.update(|window, cx| card.contains_focused(window, cx)),
        "a click on the card focuses the form"
    );
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
fn required_fields_are_listed_and_block_submit(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let mut reply = ask(cx, &view, &block);
    assert!(cx.debug_bounds("form-required-legend").is_some());
    let missing = cx.debug_bounds("form-missing");
    assert!(
        missing.is_some(),
        "the footer names the empty required field"
    );
    // The button is disabled: a click does nothing.
    click(cx, "form-submit");
    cx.run_until_parked();
    assert_eq!(reply.try_recv().ok().flatten(), None);
    form(cx, &block).read_with(cx, |form, cx| {
        assert_eq!(form.missing_required(cx), ["Label"]);
    });

    // Filled in, the note goes and the button submits.
    cx.simulate_input("run");
    draw(cx, &view);
    assert!(cx.debug_bounds("form-missing").is_none());
    click(cx, "form-submit");
    cx.run_until_parked();
    let Some(Ok(FormResponse::Submitted { answer })) = reply.now_or_never() else {
        panic!("not submitted");
    };
    assert_eq!(answer.get("label"), Some(&json!("run")));
}

#[gpui::test]
fn a_date_range_is_picked_and_submitted_as_start_and_end(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let mut reply = ask(cx, &view, &block);
    let form_view = form(cx, &block);
    let picker = form_view.read_with(cx, |form, _| match &form.inputs[7] {
        FieldInput::DateRange(state) => state.clone(),
        _ => panic!("no range picker"),
    });
    let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap_or_else(|| panic!("{y}-{m}-{d}"));

    draw(cx, &view);
    cx.simulate_input("run");
    // A period starting before the bound.
    picker.update_in(cx, |picker, window, cx| {
        picker.set_date((day(2025, 12, 30), day(2026, 1, 2)), window, cx);
    });
    draw(cx, &view);
    click(cx, "form-submit");
    cx.run_until_parked();
    form_view.read_with(cx, |form, _| {
        assert_eq!(
            form.errors().get("stay").map(String::as_str),
            Some("Pick a start and an end date between 2026-01-01 and 2026-12-31")
        );
    });
    assert_eq!(reply.try_recv().ok().flatten(), None);

    picker.update_in(cx, |picker, window, cx| {
        picker.set_date((day(2026, 5, 10), day(2026, 5, 14)), window, cx);
    });
    draw(cx, &view);
    click(cx, "form-submit");
    cx.run_until_parked();
    let Some(Ok(FormResponse::Submitted { answer })) = reply.now_or_never() else {
        panic!("not submitted");
    };
    assert_eq!(
        answer.get("stay"),
        Some(&json!({"start": "2026-05-10", "end": "2026-05-14"}))
    );
}

#[gpui::test]
fn a_date_is_picked_and_submitted_as_iso(cx: &mut TestAppContext) {
    let (cx, view, block) = window(cx);
    let mut reply = ask(cx, &view, &block);
    let form_view = form(cx, &block);
    let picker = form_view.read_with(cx, |form, _| match &form.inputs[6] {
        FieldInput::Date(state) => state.clone(),
        _ => panic!("no date picker"),
    });
    let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap_or_else(|| panic!("{y}-{m}-{d}"));

    // The focus is in the label; the date is before the bound.
    draw(cx, &view);
    cx.simulate_input("run");
    picker.update_in(cx, |picker, window, cx| {
        picker.set_date(day(2023, 12, 31), window, cx);
    });
    draw(cx, &view);
    click(cx, "form-submit");
    cx.run_until_parked();
    form_view.read_with(cx, |form, _| {
        assert_eq!(
            form.errors().get("due").map(String::as_str),
            Some("Pick a date between 2024-01-01 and 2024-12-31")
        );
    });
    assert_eq!(reply.try_recv().ok().flatten(), None);

    // A day inside the bounds goes out as an ISO date.
    picker.update_in(cx, |picker, window, cx| {
        picker.set_date(day(2024, 3, 5), window, cx);
    });
    draw(cx, &view);
    click(cx, "form-submit");
    cx.run_until_parked();
    let Some(Ok(FormResponse::Submitted { answer })) = reply.now_or_never() else {
        panic!("not submitted");
    };
    assert_eq!(answer.get("due"), Some(&json!("2024-03-05")));
    assert_eq!(answer.get("label"), Some(&json!("run")));
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
