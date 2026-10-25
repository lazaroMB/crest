use application::ports::events::DomainEvent;
use domain::block::Block;
use gpui::{Modifiers, TestAppContext, VisualTestContext, size};
use serde_json::json;

use super::*;
use crate::timeline::TimelineView;

fn wave() -> PlotSpec {
    serde_json::from_value(json!({
        "title": "Damped wave",
        "x_range": {"min": 0, "max": 10},
        "x_label": "t (s)",
        "functions": [
            {"kind": "explicit", "expr": "a exp(-k x) sin(x)", "label": "f"},
            {"kind": "explicit", "expr": "tan(x)"},
            {"kind": "parametric", "expr": "5 + cos(t)", "y_expr": "sin(t)",
             "t_range": {"min": 0, "max": 6.3}},
            {"kind": "implicit", "expr": "(x - 5)^2 + y^2 - a^2"}
        ],
        "params": [
            {"name": "a", "min": 0, "max": 2, "value": 1},
            {"name": "k", "min": 0, "max": 1, "step": 0.01, "value": 0.2}
        ],
        "series": [{"name": "measured", "x": [1, 2, 3], "y": [0.7, 0.6, 0.1]}]
    }))
    .unwrap_or_else(|e| panic!("{e}"))
}

/// A window showing `specs` as plot blocks, drawn with real frames so
/// hover tooltips render.
fn window(cx: &mut TestAppContext, specs: Vec<PlotSpec>) -> (&mut VisualTestContext, Vec<BlockId>) {
    cx.update(gpui_component::init);
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let ids = specs
        .into_iter()
        .map(|spec| {
            let block = Block::new(BlockKind::Plot(spec));
            let id = block.id;
            view.update(cx, |view, cx| {
                view.apply(&DomainEvent::BlockAdded { block }, cx)
            });
            id
        })
        .collect();
    cx.simulate_resize(size(px(1000.), px(3000.)));
    cx.run_until_parked();
    (cx, ids)
}

#[gpui::test]
fn draws_a_plot_with_sliders_and_replots_when_one_moves(cx: &mut TestAppContext) {
    let (cx, ids) = window(cx, vec![wave()]);
    let id = ids[0];
    let canvas = cx
        .debug_bounds(format!("plot-canvas-{id}").leak())
        .unwrap_or_else(|| panic!("plot drawn"));
    assert!(canvas.size.height > px(180.));
    for ix in 0..2 {
        assert!(
            cx.debug_bounds(format!("plot-slider-{id}-{ix}").leak())
                .is_some()
        );
    }
    let view = cx
        .update(|_, cx| crate::blocks::existing_block_state::<PlotView>(id, cx))
        .unwrap_or_else(|| panic!("plot view kept"));
    let before = view
        .read_with(cx, |v, _| v.scene())
        .unwrap_or_else(|| panic!("scene"));
    assert_eq!(before.layers.len(), 5);
    view.update(cx, |v, cx| v.set_param(0, 1.987, cx));
    let after = view
        .read_with(cx, |v, _| v.scene())
        .unwrap_or_else(|| panic!("scene"));
    // Snapped to the continuous slider's step, (2 - 0) / 200.
    assert_eq!(view.read_with(cx, |v, _| v.values().to_vec()), [1.99, 0.2]);
    assert_ne!(before.layers[0], after.layers[0]);
    assert_eq!(before.layers[1], after.layers[1], "tan does not use a");
    cx.run_until_parked();

    // Hovering reads out the curves without panicking.
    for (fx, fy) in [(0.5, 0.5), (0.15, 0.4), (0.9, 0.2)] {
        let at = gpui::point(
            canvas.origin.x + canvas.size.width * fx,
            canvas.origin.y + canvas.size.height * fy,
        );
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.run_until_parked();
    }
}

#[gpui::test]
fn dragging_a_slider_changes_its_parameter(cx: &mut TestAppContext) {
    let (cx, ids) = window(cx, vec![wave()]);
    let id = ids[0];
    let slider = cx
        .debug_bounds(format!("plot-slider-{id}-1").leak())
        .unwrap_or_else(|| panic!("slider drawn"));
    let at = gpui::point(slider.origin.x + slider.size.width * 0.9, slider.center().y);
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    let view = cx
        .update(|_, cx| crate::blocks::existing_block_state::<PlotView>(id, cx))
        .unwrap_or_else(|| panic!("plot view kept"));
    let k = view.read_with(cx, |v, _| v.values()[1]);
    assert!(k > 0.5, "k = {k}");
}

#[gpui::test]
fn invalid_and_data_only_plots_still_draw(cx: &mut TestAppContext) {
    let mut bad = wave();
    bad.functions[0].expr = "sin(".into();
    let data: PlotSpec = serde_json::from_value(json!({
        "series": [
            {"name": "a", "x": [1, 2, 3], "y": [1, 4, 9], "style": "line"},
            {"name": "b", "x": [1, 2, 3], "y": [2, 3, 4]}
        ]
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    let (cx, ids) = window(cx, vec![bad, data]);
    assert!(
        cx.debug_bounds(format!("plot-canvas-{}", ids[0]).leak())
            .is_none()
    );
    assert!(
        cx.debug_bounds(format!("plot-canvas-{}", ids[1]).leak())
            .is_some()
    );
}

#[gpui::test]
fn log_and_equal_aspect_plots_draw_and_hover(cx: &mut TestAppContext) {
    let log: PlotSpec = serde_json::from_value(json!({
        "x_scale": "log", "y_scale": "log",
        "functions": [{"kind": "explicit", "expr": "x^2"}],
        "series": [{"name": "s", "x": [1, 10, 100, 0], "y": [1, 90, 11000, 4]}]
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    let circle: PlotSpec = serde_json::from_value(json!({
        "aspect": "equal",
        "x_range": {"min": -2, "max": 2},
        "y_range": {"min": -1, "max": 1},
        "functions": [{"kind": "implicit", "expr": "x^2 + y^2 - 1"}]
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    let (cx, ids) = window(cx, vec![log, circle]);
    let view = cx
        .update(|_, cx| crate::blocks::existing_block_state::<PlotView>(ids[0], cx))
        .unwrap_or_else(|| panic!("plot view kept"));
    let scene = view
        .read_with(cx, |v, _| v.scene())
        .unwrap_or_else(|| panic!("scene"));
    assert_eq!(scene.skipped, 1);
    assert_eq!(
        skipped_note(scene.skipped).as_deref(),
        Some("1 point not shown: it is not positive on a log axis.")
    );
    let canvas = cx
        .debug_bounds(format!("plot-canvas-{}", ids[0]).leak())
        .unwrap_or_else(|| panic!("log plot drawn"));
    for fx in [0.2, 0.5, 0.8] {
        let at = gpui::point(
            canvas.origin.x + canvas.size.width * fx,
            canvas.origin.y + canvas.size.height * 0.5,
        );
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.run_until_parked();
    }
    // The equal-aspect plot is about half as tall as wide (4 by 2 units).
    let equal = cx
        .debug_bounds(format!("plot-canvas-{}", ids[1]).leak())
        .unwrap_or_else(|| panic!("equal plot drawn"));
    let ratio = equal.size.height / equal.size.width;
    assert!(ratio > 0.4 && ratio < 0.65, "{ratio}");
}
