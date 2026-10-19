use application::ports::events::DomainEvent;
use application::testing::sim;
use domain::sysmodel::{Board, SysModel, run_all};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point, size};

use super::*;
use crate::modal::{ModalHost, board_modal_id};

fn board() -> Board {
    let model = SysModel::from_value(sim::model()).unwrap();
    let runs = run_all(&model).unwrap();
    Board {
        spec: sim::FOLDER.into(),
        title: "Orders API".into(),
        model,
        runs,
    }
}

fn view(cx: &mut TestAppContext) -> (Entity<SysBoardView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|window, cx| {
        SysBoardView::with_backend(board(), Backend::Vector("tests".into()), window, cx)
    });
    cx.simulate_resize(size(px(1200.), px(800.)));
    cx.run_until_parked();
    (view, cx)
}

fn draw(view: &Entity<SysBoardView>, cx: &mut VisualTestContext) {
    let viewport = cx.update(|window, _| window.viewport_size());
    let view = view.clone();
    cx.draw(point(px(0.), px(0.)), viewport, move |_, _| {
        div().size_full().child(view.clone())
    });
    cx.run_until_parked();
}

#[gpui::test]
fn the_fallback_board_draws_chips_labels_and_checks(cx: &mut TestAppContext) {
    let (view, cx) = view(cx);
    draw(&view, cx);
    // Labels need the size of the first paint.
    draw(&view, cx);
    let name = view.read_with(cx, |v, _| v.backend_name());
    assert_eq!(name, "2.5D · tests");
    assert!(cx.debug_bounds("sysboard-canvas").is_some());
    assert!(cx.debug_bounds("sysboard-label").is_some(), "labels drawn");
    assert!(cx.debug_bounds("sysboard-checks").is_some());
    assert!(
        cx.debug_bounds("sysboard-inspector").is_none(),
        "nothing selected yet"
    );
    let scenario = view.read_with(cx, |v, _| v.run().unwrap().scenario.clone());
    assert_eq!(scenario, "one_request");
}

#[gpui::test]
fn clicking_a_chip_inspects_it(cx: &mut TestAppContext) {
    let (view, cx) = view(cx);
    draw(&view, cx);
    draw(&view, cx);
    let canvas = cx.debug_bounds("sysboard-canvas").unwrap();
    // Where the api chip is drawn.
    let at = view.read_with(cx, |v, _| {
        let chip = v.scene.chips.iter().find(|c| c.id == "api").unwrap();
        v.project(chip.center).unwrap()
    });
    let pos = point(canvas.origin.x + px(at.0), canvas.origin.y + px(at.1));
    cx.simulate_click(pos, Modifiers::none());
    draw(&view, cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.selected().map(str::to_owned)),
        Some("api".into())
    );
    assert!(cx.debug_bounds("sysboard-inspector").is_some());
}

#[gpui::test]
fn the_timeline_scrubs_steps_and_marks_failures(cx: &mut TestAppContext) {
    let (view, cx) = view(cx);
    draw(&view, cx);
    let bar = cx.debug_bounds("sysboard-scrub").unwrap();
    // A click in the middle of the bar jumps to the middle of the run.
    let middle = point(
        bar.origin.x + bar.size.width / 2.0,
        bar.origin.y + bar.size.height / 2.0,
    );
    cx.simulate_click(middle, Modifiers::none());
    draw(&view, cx);
    let p = view.read_with(cx, |v, _| v.playback().clone());
    let half = p.until_us / 2;
    assert!(
        p.t_us.abs_diff(half) <= p.until_us / 50,
        "{} vs {half}",
        p.t_us
    );

    view.update(cx, |v, cx| v.scrub(0.0, cx));
    view.update(cx, |v, cx| v.step_forward(cx));
    let (t, next) = view.read_with(cx, |v, _| {
        let events = &v.run().unwrap().trace.events;
        (
            v.playback().t_us,
            events.iter().find(|e| e.at_us > 0).unwrap().at_us,
        )
    });
    assert_eq!(t, next, "the next event after the start");

    // Packets follow the domain's pure frame at the same time.
    view.update(cx, |v, cx| v.seek(500, cx));
    let (frame, expected) = view.read_with(cx, |v, _| {
        let run = v.run().unwrap();
        (
            v.frame(),
            frame_at(&v.board().model, &v.scene, &run.trace, 500),
        )
    });
    assert_eq!(frame, expected);
    assert_eq!(frame.packets.len(), 1, "the request travels its trace");

    assert!(view.read_with(cx, |v, _| v.markers().is_empty()));
    view.update(cx, |v, cx| v.select_scenario(1, cx));
    let markers = view.read_with(cx, |v, _| v.markers());
    assert_eq!(markers.len(), 1);
    assert!(markers[0].label.contains("api.inflight <= 1"));
    assert_eq!(view.read_with(cx, |v, _| v.playback().t_us), 0);

    view.update(cx, |v, cx| v.toggle_play(cx));
    assert!(view.read_with(cx, |v, _| v.playback().playing));
    cx.simulate_keystrokes("space");
}

#[gpui::test]
fn reseeding_reruns_the_scenario(cx: &mut TestAppContext) {
    let (view, cx) = view(cx);
    let before = view.read_with(cx, |v, _| v.run().unwrap().trace.clone());
    view.update(cx, |v, cx| v.reseed(1, cx));
    let after = view.read_with(cx, |v, _| v.run().unwrap().trace.clone());
    assert_eq!(after.seed, before.seed + 1);
    assert_ne!(after.hash(), before.hash());
}

#[gpui::test]
fn the_modal_host_opens_the_board_at_full_size(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        disable_gpu(cx);
    });
    let (host, cx) = cx.add_window_view(ModalHost::new);
    host.update_in(cx, |host, window, cx| {
        host.apply(
            DomainEvent::SimulationReady {
                board: Box::new(board()),
            },
            window,
            cx,
        );
    });
    let viewport = cx.update(|window, _| window.viewport_size());
    let h = host.clone();
    cx.draw(point(px(0.), px(0.)), viewport, move |_, _| {
        div().relative().size_full().child(h.clone())
    });
    cx.run_until_parked();
    let id = board_modal_id(sim::FOLDER);
    host.read_with(cx, |host, _| {
        assert_eq!(host.ids(), std::slice::from_ref(&id));
        assert!(host.view(&id).is_some());
        assert_eq!(host.spec(&id).unwrap().width, domain::modal::Percent::FULL);
    });
    assert!(cx.debug_bounds("sysboard-canvas").is_some());
    cx.simulate_keystrokes("escape");
    host.read_with(cx, |host, _| assert!(!host.is_open()));
}
