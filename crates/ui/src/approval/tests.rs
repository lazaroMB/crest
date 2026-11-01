use application::ports::approval::{
    ApprovalError, ApprovalPort as _, ApprovalPreview, ApprovalRequestId,
};
use domain::conversation::ToolUseId;
use domain::permission::ToolEffect;
use futures::FutureExt as _;
use futures::executor::block_on;
use gpui::{ParentElement as _, Styled as _, div, px};

use super::*;

#[test]
fn the_all_edits_row_names_the_keys_of_the_keymap() {
    assert_eq!(
        all_edits_hint(Some("Ctrl+G p")),
        "(auto-edit · Ctrl+G p to undo)"
    );
    assert_eq!(all_edits_hint(None), "(auto-edit · /mode ask to undo)");
}

pub(super) fn command_request(id: u64) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalRequestId(id),
        tool_use_id: ToolUseId::new(format!("toolu_{id}")),
        tool: "run_command".into(),
        effect: ToolEffect::Execute,
        subject: "cargo test".into(),
        preview: ApprovalPreview::Command {
            command: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
        },
    }
}

#[test]
fn reply_reaches_the_requester() {
    let (port, mut inbox) = UiApproval::channel();
    let mut request = port.request(command_request(1)).boxed();
    assert!(
        request.as_mut().now_or_never().is_none(),
        "waits for the UI"
    );
    let pending = block_on(inbox.next()).unwrap();
    assert_eq!(pending.request().id, ApprovalRequestId(1));
    assert!(pending.respond(ApprovalResponse::Approve));
    assert_eq!(block_on(request), Ok(ApprovalResponse::Approve));
}

#[test]
fn closed_inbox_is_unavailable() {
    let (port, inbox) = UiApproval::channel();
    drop(inbox);
    let result = block_on(port.request(command_request(1)));
    assert!(matches!(result, Err(ApprovalError::Unavailable(_))));
}

#[test]
fn dropped_dialog_cancels_the_request() {
    let (port, mut inbox) = UiApproval::channel();
    let request = port.request(command_request(1));
    let (result, ()) = block_on(futures::future::join(request, async {
        drop(inbox.next().await);
    }));
    assert_eq!(result, Err(ApprovalError::Cancelled));
}

#[test]
fn dropped_request_signals_the_dialog() {
    let (port, mut inbox) = UiApproval::channel();
    let mut request = port.request(command_request(1)).boxed();
    assert!(request.as_mut().now_or_never().is_none());
    let mut pending = block_on(inbox.next()).unwrap();
    let cancellation = pending.cancellation();
    assert!(!pending.is_cancelled());
    drop(request);
    assert!(pending.is_cancelled());
    block_on(cancellation);
    assert!(!pending.respond(ApprovalResponse::Approve));
}

#[test]
fn command_line_quotes_only_when_needed() {
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        command_line("cargo", &args(&["test", "-p", "ui"])),
        "cargo test -p ui"
    );
    assert_eq!(
        command_line("git", &args(&["commit", "-m", "it's done", ""])),
        r"git commit -m 'it'\''s done' ''"
    );
}

// -- dialog -------------------------------------------------------------

use gpui::{Entity, Task, TestAppContext, VisualTestContext};

type Answer = Task<Result<ApprovalResponse, ApprovalError>>;

fn dialog(cx: &mut TestAppContext) -> (UiApproval, Entity<ApprovalView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let (port, inbox) = UiApproval::channel();
    let (view, cx) = cx.add_window_view(|window, cx| ApprovalView::new(inbox, window, cx));
    (port, view, cx)
}

fn ask(port: &UiApproval, req: ApprovalRequest, cx: &mut VisualTestContext) -> Answer {
    let port = port.clone();
    let task = cx.executor().spawn(async move { port.request(req).await });
    cx.run_until_parked();
    task
}

fn answered(task: Answer, cx: &mut VisualTestContext) -> Result<ApprovalResponse, ApprovalError> {
    cx.run_until_parked();
    task.now_or_never().expect("the request was answered")
}

fn diff_request(id: u64) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalRequestId(id),
        tool_use_id: ToolUseId::new(format!("toolu_{id}")),
        tool: "edit_file".into(),
        effect: ToolEffect::Write,
        subject: "src/lib.rs".into(),
        preview: ApprovalPreview::Diff(domain::block::DiffSpec {
            path: "src/lib.rs".into(),
            old_text: Some("a\n".into()),
            new_text: "b\n".into(),
            unified: "@@ -1 +1 @@\n-a\n+b\n".into(),
            approval: Some(domain::block::ApprovalState::Pending),
        }),
    }
}

#[gpui::test]
fn y_approves(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let answer = ask(&port, diff_request(1), cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(1)));
    });
    cx.simulate_keystrokes("y");
    assert_eq!(answered(answer, cx), Ok(ApprovalResponse::Approve));
    view.read_with(cx, |view, _| assert!(view.current().is_none()));
}

#[gpui::test]
fn n_rejects_with_a_typed_reason(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let answer = ask(&port, command_request(1), cx);
    cx.simulate_keystrokes("n u s e space x x backspace enter");
    view.read_with(cx, |view, _| assert!(view.reason().is_none()));
    assert_eq!(
        answered(answer, cx),
        Ok(ApprovalResponse::Deny {
            reason: Some("use x".into()),
        })
    );
}

#[gpui::test]
fn a_reason_can_be_typed_after_the_prompt_had_focus(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let answer = ask(&port, command_request(1), cx);
    cx.update(|window, cx| {
        let prompt = cx.focus_handle();
        window.focus(&prompt, cx);
    });
    view.update_in(cx, |view, window, cx| view.begin_reason(window, cx));
    cx.simulate_keystrokes("u s e escape");
    view.read_with(cx, |view, _| {
        assert!(view.reason().is_none(), "escape leaves the reason line");
        assert!(view.is_open(), "the dialog stays open");
    });
    cx.simulate_keystrokes("n n o enter");
    assert_eq!(
        answered(answer, cx),
        Ok(ApprovalResponse::Deny {
            reason: Some("no".into()),
        })
    );
}

#[gpui::test]
fn n_then_enter_rejects_without_reason(cx: &mut TestAppContext) {
    let (port, _view, cx) = dialog(cx);
    let answer = ask(&port, command_request(1), cx);
    cx.simulate_keystrokes("n enter");
    assert_eq!(
        answered(answer, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );
}

#[gpui::test]
fn escape_rejects_and_backs_out_of_the_reason(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let first = ask(&port, command_request(1), cx);
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("approval-keys").is_some(),
        "the dialog says what Esc does"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(
        answered(first, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );

    let second = ask(&port, command_request(2), cx);
    cx.simulate_keystrokes("n x escape");
    view.read_with(cx, |view, _| {
        assert!(view.reason().is_none());
        assert!(view.current().is_some(), "still open");
    });
    cx.simulate_keystrokes("y");
    assert_eq!(answered(second, cx), Ok(ApprovalResponse::Approve));
}

#[gpui::test]
fn queued_requests_are_shown_in_order(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let first = ask(&port, diff_request(1), cx);
    let second = ask(&port, command_request(2), cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(1)));
        assert_eq!(view.queued(), 1);
    });
    cx.simulate_keystrokes("y");
    view.read_with(cx, |view, _| {
        assert_eq!(view.current().map(|r| r.id), Some(ApprovalRequestId(2)));
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(answered(first, cx), Ok(ApprovalResponse::Approve));
    assert_eq!(
        answered(second, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );
}

#[gpui::test]
fn dropping_the_request_closes_the_dialog(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &ApprovalEvent, _| {
            seen.borrow_mut().push(event.clone());
        })
        .detach();
    });
    let answer = ask(&port, diff_request(1), cx);
    view.read_with(cx, |view, _| assert!(view.current().is_some()));
    drop(answer);
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert!(view.current().is_none()));
    assert_eq!(
        *events.borrow(),
        [ApprovalEvent::Cancelled(ApprovalRequestId(1))]
    );
    // Keys no longer answer anything.
    cx.simulate_keystrokes("y");
    assert_eq!(events.borrow().len(), 1);
}

#[gpui::test]
fn focus_returns_after_answering(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let before = cx.update(|window, cx| {
        let handle = cx.focus_handle();
        window.focus(&handle, cx);
        handle
    });
    let answer = ask(&port, command_request(1), cx);
    cx.update(|window, cx| {
        assert!(view.read(cx).focus_handle.is_focused(window));
    });
    cx.simulate_keystrokes("y");
    assert_eq!(answered(answer, cx), Ok(ApprovalResponse::Approve));
    cx.update(|window, _| assert!(before.is_focused(window)));
}

#[gpui::test]
fn draws_every_preview(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let text = ApprovalRequest {
        preview: ApprovalPreview::Text {
            text: "fetch https://example.com".into(),
        },
        ..command_request(3)
    };
    let _answers = [
        ask(&port, diff_request(1), cx),
        ask(&port, command_request(2), cx),
        ask(&port, text, cx),
    ];
    for keys in ["", "y", "n"] {
        if !keys.is_empty() {
            cx.simulate_keystrokes(keys);
        }
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(1000.), gpui::px(700.)),
            |_, _| div().relative().size_full().child(view.clone()),
        );
    }
}

#[gpui::test]
fn a_long_preview_scrolls_with_the_keys(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let text = (0..200)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let _answer = ask(
        &port,
        ApprovalRequest {
            preview: ApprovalPreview::Text { text },
            ..command_request(1)
        },
        cx,
    );
    let draw = |cx: &mut VisualTestContext| {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    };
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.preview_scroll.max_offset().y > px(0.), "it overflows");
    });
    cx.simulate_keystrokes("down");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.preview_scroll.offset().y,
            px(0.),
            "Down moves the cursor"
        );
        assert_eq!(view.selected(), Some(Choice::Session));
    });
    cx.simulate_keystrokes("pagedown");
    draw(cx);
    let paged = view.read_with(cx, |view, _| view.preview_scroll.offset().y);
    assert!(paged < px(0.), "PageDown scrolls");
    cx.simulate_keystrokes("shift-down");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.preview_scroll.offset().y < paged, "Shift+Down scrolls");
        assert_eq!(view.selected(), Some(Choice::Session), "not the cursor");
    });
    cx.simulate_keystrokes("home");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.preview_scroll.offset().y, px(0.), "Home goes back");
    });
}

#[gpui::test]
fn diff_buttons_answer_only_their_diff(cx: &mut TestAppContext) {
    let (port, _view, cx) = dialog(cx);
    let answer = ask(&port, diff_request(1), cx);
    let other = cx.update(|window, cx| answer_diff("src/main.rs", true, window, cx));
    assert!(!other, "another path is not the open request");
    let done = cx.update(|window, cx| answer_diff("src/lib.rs", false, window, cx));
    assert!(done);
    assert_eq!(
        answered(answer, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );
    let idle = cx.update(|window, cx| answer_diff("src/lib.rs", true, window, cx));
    assert!(!idle, "nothing is open");
}

// -- options and patterns -------------------------------------------------

/// A `run_command` request for the shell-split `line`.
fn run_request(id: u64, line: &str) -> ApprovalRequest {
    let argv = split_command(line).unwrap();
    ApprovalRequest {
        subject: line.into(),
        preview: ApprovalPreview::Command {
            command: argv[0].clone(),
            args: argv[1..].to_vec(),
            cwd: ".".into(),
        },
        ..command_request(id)
    }
}

fn command_rule(tokens: &[&str], scope: RuleScope) -> ApprovalResponse {
    ApprovalResponse::ApproveAlways {
        rule: domain::permission::PermissionRule::new(
            "run_command",
            RuleMatcher::Command(tokens.iter().map(|t| (*t).to_owned()).collect()),
            domain::permission::RuleDecision::AllowAlways,
        )
        .with_scope(scope),
    }
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn click(selector: &'static str, cx: &mut VisualTestContext) {
    redraw(cx);
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} drawn"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn the_pattern_starts_at_the_subcommand_and_tab_widens_it(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let answer = ask(&port, run_request(1, "cargo test -p ui --lib"), cx);
    let pattern = |view: &Entity<ApprovalView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |view, _| view.pattern().map(pattern_text))
    };
    assert_eq!(pattern(&view, cx).as_deref(), Some("cargo test *"));
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.choices(),
            [
                Choice::Once,
                Choice::Session,
                Choice::Project,
                Choice::Reject
            ],
            "no all-edits row for a command"
        );
    });
    redraw(cx);
    assert!(cx.debug_bounds("approval-pattern").is_some());
    assert!(cx.debug_bounds("approval-pattern-hint").is_some());
    assert!(cx.debug_bounds("approval-keys").is_some());

    cx.simulate_keystrokes("tab");
    assert_eq!(pattern(&view, cx).as_deref(), Some("cargo *"));
    cx.simulate_keystrokes("tab");
    assert_eq!(pattern(&view, cx).as_deref(), Some("cargo *"), "the widest");
    cx.simulate_keystrokes("shift-tab shift-tab shift-tab");
    assert_eq!(
        pattern(&view, cx).as_deref(),
        Some("cargo test -p ui --lib"),
        "the exact command"
    );
    redraw(cx);
    assert!(
        cx.debug_bounds("approval-pattern-hint").is_none(),
        "exact needs no hint"
    );
    cx.simulate_keystrokes("tab tab s");
    assert_eq!(
        answered(answer, cx),
        Ok(command_rule(&["cargo"], RuleScope::Session))
    );
}

#[gpui::test]
fn p_allows_the_pattern_in_the_project(cx: &mut TestAppContext) {
    let (port, _view, cx) = dialog(cx);
    let answer = ask(&port, run_request(1, "cargo test -p ui --lib"), cx);
    cx.simulate_keystrokes("p");
    assert_eq!(
        answered(answer, cx),
        Ok(command_rule(&["cargo", "test"], RuleScope::Project))
    );
}

#[gpui::test]
fn arrows_and_enter_pick_a_row(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let first = ask(&port, run_request(1, "cargo test -p ui"), cx);
    cx.simulate_keystrokes("up down down");
    view.read_with(cx, |view, _| {
        assert_eq!(view.selected(), Some(Choice::Project));
    });
    redraw(cx);
    assert!(
        cx.debug_bounds("approval-project-file").is_some(),
        "the project row says where it saves"
    );
    cx.simulate_keystrokes("up enter");
    assert_eq!(
        answered(first, cx),
        Ok(command_rule(&["cargo", "test"], RuleScope::Session))
    );

    let second = ask(&port, run_request(2, "cargo test -p ui"), cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.selected(),
            Some(Choice::Once),
            "a new request starts on top"
        );
    });
    cx.simulate_keystrokes("down down down down down enter");
    view.read_with(cx, |view, _| {
        assert_eq!(view.reason(), Some(""), "the last row asks why");
    });
    cx.simulate_keystrokes("n o enter");
    assert_eq!(
        answered(second, cx),
        Ok(ApprovalResponse::Deny {
            reason: Some("no".into())
        })
    );
}

#[gpui::test]
fn a_is_kept_as_this_session(cx: &mut TestAppContext) {
    let (port, _view, cx) = dialog(cx);
    let first = ask(&port, run_request(1, "cargo build --release"), cx);
    cx.simulate_keystrokes("a");
    let second = ask(&port, run_request(2, "cargo build --release"), cx);
    cx.simulate_keystrokes("s");
    let expected = Ok(command_rule(&["cargo", "build"], RuleScope::Session));
    assert_eq!(answered(first, cx), expected);
    assert_eq!(answered(second, cx), expected);
}

#[gpui::test]
fn all_edits_is_offered_only_for_writes(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let command = ask(&port, command_request(1), cx);
    cx.simulate_keystrokes("e");
    view.read_with(cx, |view, _| {
        assert!(view.is_open(), "`e` does nothing for a command");
    });
    redraw(cx);
    assert!(cx.debug_bounds("approval-row-e").is_none());
    cx.simulate_keystrokes("y");
    assert_eq!(answered(command, cx), Ok(ApprovalResponse::Approve));

    let edit = ask(&port, diff_request(2), cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.choices(),
            [
                Choice::Once,
                Choice::Session,
                Choice::Project,
                Choice::AllEdits,
                Choice::Reject
            ]
        );
        assert_eq!(view.pattern().map(pattern_text).as_deref(), Some("src/*"));
    });
    redraw(cx);
    assert!(cx.debug_bounds("approval-row-e").is_some());
    cx.simulate_keystrokes("e");
    assert_eq!(answered(edit, cx), Ok(ApprovalResponse::ApproveAllEdits));
}

#[gpui::test]
fn shells_are_only_allowed_exactly(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let line = "sh -c 'rm -rf target'";
    let answer = ask(&port, run_request(1, line), cx);
    view.read_with(cx, |view, _| {
        assert!(view.is_exact_only());
        assert_eq!(view.pattern().map(pattern_text).as_deref(), Some(line));
    });
    redraw(cx);
    assert!(cx.debug_bounds("approval-pattern-reason").is_some());
    assert!(
        cx.debug_bounds("approval-pattern-wider").is_none(),
        "no arrows for a single rung"
    );
    cx.simulate_keystrokes("tab");
    view.read_with(cx, |view, _| {
        assert_eq!(view.pattern().map(pattern_text).as_deref(), Some(line));
    });
    cx.simulate_keystrokes("s");
    assert_eq!(
        answered(answer, cx),
        Ok(ApprovalResponse::ApproveAlways {
            rule: domain::permission::PermissionRule::new(
                "run_command",
                RuleMatcher::Exact(line.into()),
                domain::permission::RuleDecision::AllowAlways,
            ),
        })
    );

    let plain = ask(&port, run_request(2, "ls"), cx);
    view.read_with(cx, |view, _| {
        assert!(!view.is_exact_only(), "one rung, but not a shell");
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(
        answered(plain, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );
}

#[gpui::test]
fn clicking_rows_and_arrows(cx: &mut TestAppContext) {
    let (port, view, cx) = dialog(cx);
    let first = ask(&port, run_request(1, "cargo test -p ui"), cx);
    click("approval-pattern-wider-project", cx);
    view.read_with(cx, |view, _| {
        assert!(view.is_open(), "the arrow does not answer");
        assert_eq!(view.pattern().map(pattern_text).as_deref(), Some("cargo *"));
    });
    click("approval-pattern-narrower", cx);
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.pattern().map(pattern_text).as_deref(),
            Some("cargo test *")
        );
    });
    click("approval-row-p", cx);
    assert_eq!(
        answered(first, cx),
        Ok(command_rule(&["cargo", "test"], RuleScope::Project))
    );

    let second = ask(&port, command_request(2), cx);
    click("approval-row-y", cx);
    assert_eq!(answered(second, cx), Ok(ApprovalResponse::Approve));

    let third = ask(&port, command_request(3), cx);
    click("approval-row-n", cx);
    view.read_with(cx, |view, _| assert_eq!(view.reason(), Some("")));
    cx.simulate_keystrokes("enter");
    assert_eq!(
        answered(third, cx),
        Ok(ApprovalResponse::Deny { reason: None })
    );
}
