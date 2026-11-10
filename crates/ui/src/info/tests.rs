use std::time::Duration;

use application_core::ports::events::{DomainEvent, NoticeLevel};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::activity::{SHOW_AFTER, WorkKind};
use crate::app::RootView;
use crate::input::InputView;
use crate::status::NOTICE_TTL;

/// A window with the status bar and the mark in their slots.
struct Rig {
    status: Entity<StatusView>,
    activity: Entity<ActivityView>,
    info: Entity<InfoView>,
}

fn rig(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let (root, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    let status = cx.new(|_| StatusView::new("w"));
    let activity = cx.new(|_| ActivityView::new());
    let input = cx.update(|window, cx| cx.new(|cx| InputView::new(window, cx)));
    let info = cx.new(|cx| InfoView::new(status.clone(), activity.clone(), input, cx));
    status.update(cx, |status, cx| status.set_info(info.clone(), cx));
    root.update(cx, |root, cx| root.set_status(status.clone(), cx));
    cx.run_until_parked();
    (
        Rig {
            status,
            activity,
            info,
        },
        cx,
    )
}

impl Rig {
    fn notice(&self, level: NoticeLevel, text: &str, cx: &mut VisualTestContext) {
        self.status.update(cx, |status, cx| {
            let event = DomainEvent::Notice {
                level,
                message: text.into(),
            };
            status.apply(&event, cx);
        });
        cx.run_until_parked();
    }

    /// A step that ran for `ran`.
    fn step(&self, ran: Duration, cx: &mut VisualTestContext) {
        self.activity
            .update(cx, |a, cx| a.set_work(Some(WorkKind::Turn), cx));
        cx.executor().advance_clock(ran);
        self.activity.update(cx, |a, cx| a.set_work(None, cx));
        cx.run_until_parked();
    }

    fn rows(&self, cx: &VisualTestContext) -> Vec<String> {
        self.info.read_with(cx, |info, cx| {
            info.rows(cx).into_iter().map(|row| row.text).collect()
        })
    }
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).expect(selector).center();
    cx.simulate_click(at, gpui::Modifiers::none());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

/// What the person sees right after something is added and 3 s later.
/// (what is added, toast now, mark now, mark after the toast)
#[gpui::test]
fn an_entry_toasts_for_three_seconds_then_waits_behind_the_mark(cx: &mut TestAppContext) {
    type Add = fn(&Rig, &mut VisualTestContext);
    let cases: [(&str, Add, bool, bool); 5] = [
        (
            "error",
            |r, cx| r.notice(NoticeLevel::Error, "boom", cx),
            true,
            true,
        ),
        (
            "warning",
            |r, cx| r.notice(NoticeLevel::Warning, "slow", cx),
            true,
            true,
        ),
        ("short step", |r, cx| r.step(Duration::ZERO, cx), true, true),
        // A transient notice is not kept (NFR-1): no toast, no mark.
        (
            "info",
            |r, cx| r.notice(NoticeLevel::Info, "saved", cx),
            false,
            false,
        ),
        // A step the live line showed is not a short one (NFR-2).
        (
            "long step",
            |r, cx| r.step(SHOW_AFTER * 2, cx),
            false,
            false,
        ),
    ];
    for (name, add, toast, mark) in cases {
        let (rig, cx) = rig(cx);
        assert!(!shown(cx, "info-mark"), "{name}: quiet while empty");
        add(&rig, cx);
        assert_eq!(shown(cx, "info-toast"), toast, "{name}: toast on add");
        assert_eq!(shown(cx, "info-mark"), mark, "{name}: mark on add");
        cx.executor().advance_clock(TOAST_FOR);
        cx.run_until_parked();
        assert!(!shown(cx, "info-toast"), "{name}: toast gone after 3 s");
        assert_eq!(shown(cx, "info-mark"), mark, "{name}: mark stays");
    }
}

#[gpui::test]
fn what_was_there_before_the_view_is_no_toast(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let (root, cx) = cx.add_window_view(|_, cx| RootView::new(cx));
    let status = cx.new(|_| StatusView::new("w"));
    status.update(cx, |status, cx| {
        let event = DomainEvent::Notice {
            level: NoticeLevel::Error,
            message: "old".into(),
        };
        status.apply(&event, cx);
    });
    let activity = cx.new(|_| ActivityView::new());
    let input = cx.update(|window, cx| cx.new(|cx| InputView::new(window, cx)));
    let info = cx.new(|cx| InfoView::new(status.clone(), activity, input, cx));
    status.update(cx, |status, cx| status.set_info(info, cx));
    root.update(cx, |root, cx| root.set_status(status, cx));
    cx.run_until_parked();
    assert!(!shown(cx, "info-toast") && shown(cx, "info-mark"));
}

#[gpui::test]
fn a_kept_notice_outlives_the_bar_and_a_transient_one_does_not(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    rig.notice(NoticeLevel::Info, "saved", cx);
    rig.notice(NoticeLevel::Error, "boom", cx);
    cx.executor().advance_clock(NOTICE_TTL);
    cx.run_until_parked();
    rig.status.read_with(cx, |status, cx| {
        let now = cx.background_executor().now();
        assert!(status.state().notice(now).is_none(), "the bar cleared it");
        let kept: Vec<_> = status
            .state()
            .kept
            .items()
            .iter()
            .map(|n| &n.message)
            .collect();
        assert_eq!(kept, ["boom"], "only the error is kept");
    });
    assert_eq!(rig.rows(cx), ["boom"]);
}

#[gpui::test]
fn a_short_step_is_in_the_panel_and_never_on_the_live_line(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    rig.step(Duration::from_millis(20), cx);
    rig.step(SHOW_AFTER * 2, cx);
    rig.activity.read_with(cx, |activity, cx| {
        assert!(activity.line(cx).is_none(), "NFR-2: the line stays blank");
    });
    assert_eq!(
        rig.rows(cx),
        ["Working · 20 ms"],
        "the long step is not here"
    );
}

#[gpui::test]
fn the_panel_lists_newest_first_and_dismiss_removes_only_its_notice(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    for text in ["first", "second", "third"] {
        rig.notice(NoticeLevel::Warning, text, cx);
    }
    rig.step(Duration::ZERO, cx);
    rig.info
        .update_in(cx, |info, window, cx| info.toggle(window, cx));
    cx.run_until_parked();
    assert!(shown(cx, "info-panel") && !shown(cx, "info-toast"));
    assert_eq!(rig.rows(cx), ["third", "second", "first", "Working · 0 ms"]);
    // The selector names every Dismiss button; it resolves to the last drawn,
    // the oldest notice's.
    click(cx, "info-dismiss");
    cx.run_until_parked();
    assert_eq!(rig.rows(cx), ["third", "second", "Working · 0 ms"]);
    rig.info.update_in(cx, |info, window, cx| {
        info.dismiss(1, cx);
        info.settle(window, cx);
    });
    cx.run_until_parked();
    assert_eq!(rig.rows(cx), ["third", "Working · 0 ms"]);
}

#[gpui::test]
fn the_mark_opens_the_panel_and_escape_closes_it(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    rig.notice(NoticeLevel::Error, "boom", cx);
    click(cx, "info-mark");
    cx.run_until_parked();
    assert!(shown(cx, "info-panel"), "a click opens");
    cx.simulate_keystrokes("escape");
    assert!(!shown(cx, "info-panel"), "Esc closes");
    click(cx, "info-mark");
    click(cx, "info-mark");
    assert!(!shown(cx, "info-panel"), "a second click closes");
}

#[gpui::test]
fn dismissing_the_last_entry_closes_the_panel_and_hides_the_mark(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    rig.notice(NoticeLevel::Error, "boom", cx);
    rig.info
        .update_in(cx, |info, window, cx| info.toggle(window, cx));
    rig.info.update_in(cx, |info, window, cx| {
        info.dismiss(0, cx);
        info.settle(window, cx);
    });
    cx.run_until_parked();
    assert!(!shown(cx, "info-panel") && !shown(cx, "info-mark"));
}

#[gpui::test]
fn the_panel_floats_above_the_status_bar_and_moves_nothing(cx: &mut TestAppContext) {
    let (rig, cx) = rig(cx);
    for ix in 0..30 {
        rig.notice(NoticeLevel::Error, &format!("error {ix}"), cx);
    }
    let names = ["prompt-row", "status-row", "conversation"];
    let before = names.map(|name| cx.debug_bounds(name));
    let toast = cx.debug_bounds("info-toast").expect("toast");
    rig.info
        .update_in(cx, |info, window, cx| info.toggle(window, cx));
    cx.run_until_parked();
    let after = names.map(|name| cx.debug_bounds(name));
    assert_eq!(before, after, "no sibling moved or resized");
    let (panel, status) = (
        cx.debug_bounds("info-panel").expect("panel"),
        cx.debug_bounds("status-row").expect("status"),
    );
    let mark = cx.debug_bounds("info-mark").expect("mark");
    assert!(panel.bottom() <= mark.top() && mark.bottom() <= status.top());
    assert!(panel.right() <= status.right() && mark.right() <= status.right());
    assert!(
        panel.size.height <= gpui::px(PANEL_MAX_HEIGHT),
        "it scrolls"
    );
    assert!(
        toast.bottom() <= status.top(),
        "the toast is above the bar too"
    );
}

#[test]
fn rows_and_toasts_follow_the_logs() {
    use crate::activity::Step;
    use crate::status::KeptNotice;
    use std::time::Instant;
    let now = Instant::now();
    let notice = |id, message: &str| KeptNotice {
        id,
        level: NoticeLevel::Error,
        message: message.into(),
        added_at: now,
    };
    let step = |id, short| Step {
        id,
        label: "Working".into(),
        elapsed: Duration::from_millis(5),
        short,
    };
    let (kept, steps) = (
        [notice(0, "a"), notice(1, "b")],
        [step(0, true), step(1, false)],
    );
    let texts: Vec<_> = rows(&kept, &steps).into_iter().map(|r| r.text).collect();
    assert_eq!(texts, ["b", "a", "Working · 5 ms"]);
    let mut state = InfoState::seen(&kept[..1], &[]);
    assert!(
        state.sync(&kept, &steps, now),
        "one notice and a step are new"
    );
    assert_eq!(state.toast(now).map(|t| t.text.as_str()), Some("b"));
    assert!(state.toast(now + TOAST_FOR).is_none(), "up at 3 s");
    assert!(
        !state.sync(&kept, &steps, now),
        "nothing new the second time"
    );
}
