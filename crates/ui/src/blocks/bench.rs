//! Renderer of the `benchmarks` block kind (`show_benchmarks`): criterion
//! results, baseline against new, as a table with a forest plot.
//!
//! A summary line gives the directory, the baseline and the count per
//! verdict, and a caption says where the numbers come from: criterion's
//! estimates files, read as data, not computed here. Each row is one
//! benchmark: the new and baseline mean with their intervals, the change
//! of the mean with its interval, the verdict (regressed in the danger
//! color, improved in the success color) and the change drawn on a
//! shared symmetric axis, with the noise band shaded and zero marked.
//! Regressions come first.

use domain::block::BlockKind;
use domain::devops::bench::{Bench, BenchSpec, Estimate, Verdict, format_change, format_ns};
use gpui::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::trace::caption;
use crate::timeline::TimelineEntry;

/// Rows drawn; the rest are counted.
pub const MAX_ROWS: usize = 500;
/// Width of the forest plot column.
pub const PLOT_WIDTH: f32 = 200.0;
/// The widest change the plot axis shows (±200%).
pub const MAX_EXTENT: f64 = 2.0;
const TIME_WIDTH: f32 = 190.0;
const CHANGE_WIDTH: f32 = 200.0;
const VERDICT_WIDTH: f32 = 90.0;

/// Order of the rows: regressions, improvements, within noise, no
/// change, no baseline; then by id.
#[must_use]
pub fn row_order(spec: &BenchSpec) -> Vec<usize> {
    let rank = |v: Verdict| match v {
        Verdict::Regressed => 0,
        Verdict::Improved => 1,
        Verdict::WithinNoise => 2,
        Verdict::NoChange => 3,
        Verdict::Unknown => 4,
    };
    let mut ixs: Vec<usize> = (0..spec.benches.len()).collect();
    ixs.sort_by(|&a, &b| {
        let (x, y) = (&spec.benches[a], &spec.benches[b]);
        rank(x.verdict)
            .cmp(&rank(y.verdict))
            .then_with(|| x.id.cmp(&y.id))
    });
    ixs
}

/// A time with its interval: `25.1 µs ± [24.9 µs, 25.3 µs]`.
#[must_use]
pub fn time_text(e: &Estimate) -> String {
    format!(
        "{} [{}, {}]",
        format_ns(e.point),
        format_ns(e.lo),
        format_ns(e.hi)
    )
}

/// The change cell: `+14.1% [+12.9%, +15.6%]`, or the plain ratio.
#[must_use]
pub fn change_text(bench: &Bench) -> String {
    match (&bench.change, bench.ratio) {
        (Some(c), _) => format!(
            "{} [{}, {}]",
            format_change(c.point),
            format_change(c.lo),
            format_change(c.hi)
        ),
        (None, Some(r)) => format!("{} (means only)", format_change(r)),
        (None, None) => "—".to_owned(),
    }
}

/// Where `fraction` falls on an axis of ±`extent`, from 0 to 1.
#[must_use]
pub fn axis_position(fraction: f64, extent: f64) -> f32 {
    (0.5 + fraction / (2.0 * extent)).clamp(0.0, 1.0) as f32
}

fn verdict_color(verdict: Verdict, cx: &App) -> Hsla {
    let theme = cx.theme();
    match verdict {
        Verdict::Regressed => theme.danger,
        Verdict::Improved => theme.success,
        _ => theme.muted_foreground,
    }
}

fn plot(bench: &Bench, extent: f64, noise: f64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = verdict_color(bench.verdict, cx);
    let at = |f: f64| axis_position(f, extent);
    let band = (at(-noise), at(noise));
    let mut cell = div()
        .relative()
        .w(px(PLOT_WIDTH))
        .h(px(12.))
        .flex_none()
        .child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(relative(band.0))
                .w(relative((band.1 - band.0).max(0.002)))
                .bg(theme.muted.opacity(0.6)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(relative(at(0.0)))
                .w(px(1.))
                .bg(theme.border),
        );
    let point = bench.change.map(|c| c.point).or(bench.ratio);
    if let Some(c) = &bench.change {
        let (lo, hi) = (at(c.lo), at(c.hi));
        cell = cell.child(
            div()
                .absolute()
                .top(px(4.))
                .h(px(4.))
                .left(relative(lo))
                .w(relative((hi - lo).max(0.004)))
                .rounded_sm()
                .bg(color.opacity(0.6)),
        );
    }
    if let Some(p) = point {
        cell = cell.child(
            div()
                .absolute()
                .top(px(2.))
                .size(px(8.))
                .ml(px(-4.))
                .left(relative(at(p)))
                .rounded_full()
                .bg(color),
        );
    }
    cell.into_any_element()
}

fn header(cx: &App) -> AnyElement {
    let cell = |w: f32, text: &'static str| div().w(px(w)).flex_none().child(text);
    h_flex()
        .gap_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_1().min_w(px(120.)).child("benchmark"))
        .child(cell(TIME_WIDTH, "new mean"))
        .child(cell(TIME_WIDTH, "baseline mean"))
        .child(cell(CHANGE_WIDTH, "change of the mean"))
        .child(cell(VERDICT_WIDTH, "verdict"))
        .child(cell(PLOT_WIDTH, "change (± axis)"))
        .into_any_element()
}

fn row(ix: usize, spec: &BenchSpec, extent: f64, cx: &App) -> AnyElement {
    let bench = &spec.benches[ix];
    let theme = cx.theme();
    let cell = |w: f32, text: String| div().w(px(w)).flex_none().child(text);
    h_flex()
        .gap_2()
        .text_xs()
        .debug_selector(move || format!("bench-row-{ix}"))
        .child(div().flex_1().min_w(px(120.)).child(bench.id.clone()))
        .child(cell(TIME_WIDTH, time_text(&bench.new.mean)))
        .child(cell(
            TIME_WIDTH,
            bench.base.map_or("—".to_owned(), |b| time_text(&b.mean)),
        ))
        .child(cell(CHANGE_WIDTH, change_text(bench)))
        .child(
            div()
                .w(px(VERDICT_WIDTH))
                .flex_none()
                .text_color(verdict_color(bench.verdict, cx))
                .when(
                    matches!(bench.verdict, Verdict::Regressed | Verdict::Improved),
                    |d| d.font_weight(gpui::FontWeight::SEMIBOLD),
                )
                .child(bench.verdict.name()),
        )
        .child(plot(bench, extent, spec.noise_threshold, cx))
        .when(ix % 2 == 1, |d| d.bg(theme.muted.opacity(0.25)))
        .into_any_element()
}

fn render_spec(spec: &BenchSpec, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let count = |v| spec.with_verdict(v).len();
    let title = caption(spec.title.as_deref(), Some(&spec.path), "Benchmarks");
    let summary = format!(
        "{title} · baseline {} vs new · {} benchmarks",
        spec.baseline,
        spec.benches.len()
    );
    let extent = spec.change_extent().min(MAX_EXTENT);
    let order = row_order(spec);
    let rows = order
        .iter()
        .take(MAX_ROWS)
        .map(|&ix| row(ix, spec, extent, cx));
    v_flex()
        .w_full()
        .gap_1()
        .debug_selector(|| "benchmarks".to_owned())
        .child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .debug_selector(|| "benchmarks-summary".to_owned())
                .child(summary)
                .child(
                    div()
                        .text_color(theme.danger)
                        .child(format!("{} regressed", count(Verdict::Regressed))),
                )
                .child(
                    div()
                        .text_color(theme.success)
                        .child(format!("{} improved", count(Verdict::Improved))),
                )
                .child(format!("{} within noise", count(Verdict::WithinNoise)))
                .child(format!("{} no change", count(Verdict::NoChange))),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .debug_selector(|| "benchmarks-source".to_owned())
                .child(format!(
                    "Data: criterion's estimates.json (95% intervals), read, not computed. \
                     Verdict: interval of the mean change against ±{}% noise; axis ±{}.",
                    spec.noise_threshold * 100.0,
                    format_change(extent).trim_start_matches('+')
                )),
        )
        .child(header(cx))
        .children(rows)
        .when(order.len() > MAX_ROWS, |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("… {} more (copy as CSV)", order.len() - MAX_ROWS)),
            )
        })
        .when(!spec.skipped.is_empty(), |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.warning)
                    .child(format!("skipped: {}", spec.skipped.join(", "))),
            )
        })
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Benchmarks(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    render_spec(spec, cx)
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, size};
    use serde_json::json;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> BenchSpec {
        let est =
            |p: f64, lo: f64, hi: f64| json!({"point": p, "lo": lo, "hi": hi, "confidence": 0.95});
        serde_json::from_value(json!({
            "path": "target/criterion",
            "baseline": "base",
            "noise_threshold": 0.01,
            "benches": [
                {"id": "a", "dir": "a", "new": {"mean": est(10.0, 9.0, 11.0)}, "verdict": "no_change",
                 "base": {"mean": est(10.0, 9.0, 11.0)}, "change": est(0.0, -0.02, 0.02)},
                {"id": "b", "dir": "b", "new": {"mean": est(25_100.0, 24_900.0, 25_300.0)},
                 "base": {"mean": est(22_000.0, 21_800.0, 22_200.0)},
                 "change": est(0.141, 0.129, 0.156), "verdict": "regressed"},
                {"id": "c", "dir": "c", "new": {"mean": est(5.0, 4.0, 6.0)}, "verdict": "unknown"}
            ]
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn cells_and_axis() {
        let spec = spec();
        assert_eq!(row_order(&spec), [1, 0, 2]);
        assert_eq!(change_text(&spec.benches[1]), "+14.1% [+12.9%, +15.6%]");
        assert_eq!(change_text(&spec.benches[2]), "—");
        assert_eq!(
            time_text(&spec.benches[1].new.mean),
            "25.1 µs [24.9 µs, 25.3 µs]"
        );
        assert!((axis_position(0.0, 0.2) - 0.5).abs() < 1e-6);
        assert!((axis_position(0.2, 0.2) - 1.0).abs() < 1e-6);
        assert!((axis_position(-5.0, 0.2)).abs() < 1e-6);
    }

    #[gpui::test]
    fn draws_summary_source_and_rows(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Benchmarks(Box::new(spec())));
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1200.), px(700.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "benchmarks",
            "benchmarks-summary",
            "benchmarks-source",
            "bench-row-0",
            "bench-row-2",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
