//! The numeric axes of a plot: the visible view, nice ticks from the chart
//! scale ([`nice_step`]), tick labels written to the step's precision, and
//! the layout of the plot area inside the element, all in pixels relative
//! to the element's top-left corner.
//!
//! The view is in axis units: the values themselves on a linear axis,
//! their `log10` on a logarithmic one, whose ticks fall on whole decades
//! with minor ticks at 2..9 (or at nice values when the view spans less
//! than a decade). An equal-aspect plot sizes its area so a unit is as
//! long on both axes.

use domain::plot::AxisScale;

use crate::blocks::chart::layout::{Align, LABEL_GAP, LABEL_HEIGHT, Label, Measure, PAD, Rect};
use crate::blocks::chart::scale::{format_compact, nice_step, nice_ticks_n};

/// Height of the plot area per pixel of width.
pub const ASPECT: f32 = 0.55;
/// Smallest plot area height.
pub const MIN_PLOT_HEIGHT: f32 = 180.0;
/// Largest plot area height.
pub const MAX_PLOT_HEIGHT: f32 = 380.0;
/// Pixels per x tick the axis aims for.
pub const X_TICK_SPACING: f32 = 90.0;
/// Pixels per y tick the axis aims for.
pub const Y_TICK_SPACING: f32 = 50.0;
/// Tallest plot area of an equal-aspect plot; a taller one gets narrower.
pub const MAX_EQUAL_HEIGHT: f32 = 520.0;
/// Shortest plot area of an equal-aspect plot (below it the aspect gives).
pub const MIN_EQUAL_HEIGHT: f32 = 40.0;

/// How the axes of a plot are drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Axes {
    pub x: AxisScale,
    pub y: AxisScale,
    /// One axis unit as long on both axes.
    pub equal: bool,
    /// The x axis runs right to left.
    pub x_reversed: bool,
}

/// The visible ranges of both axes, each with `min < max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub x: (f64, f64),
    pub y: (f64, f64),
}

impl View {
    /// `(x, y)` as fractions of the view, `(0, 0)` bottom-left and `(1, 1)`
    /// top-right.
    pub fn normalize(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            (x - self.x.0) / (self.x.1 - self.x.0),
            (y - self.y.0) / (self.y.1 - self.y.0),
        )
    }

    /// The data point at fractions `(nx, ny)` of the view.
    pub fn denormalize(&self, (nx, ny): (f64, f64)) -> (f64, f64) {
        (
            self.x.0 + nx * (self.x.1 - self.x.0),
            self.y.0 + ny * (self.y.1 - self.y.0),
        )
    }
}

/// A range widened so it is never empty: a flat one grows around itself.
pub fn widen(lo: f64, hi: f64) -> (f64, f64) {
    if !(lo.is_finite() && hi.is_finite()) {
        return (-1.0, 1.0);
    }
    if hi > lo {
        return (lo, hi);
    }
    let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
    (lo - pad, hi + pad)
}

/// `[lo, hi]` stretched to the nice ticks around it, for fitted ranges.
pub fn nice_range(lo: f64, hi: f64, target: usize) -> (f64, f64) {
    let (lo, hi) = widen(lo, hi);
    let ticks = nice_ticks_n(lo, hi, target);
    match (ticks.first(), ticks.last()) {
        (Some(&a), Some(&b)) if b > a => (a, b),
        _ => (lo, hi),
    }
}

/// The ticks of about `target` intervals that fall inside `[lo, hi]`, and
/// their step.
pub fn ticks(lo: f64, hi: f64, target: usize) -> (Vec<f64>, f64) {
    let step = nice_step(hi - lo, target);
    let first = (lo / step).ceil() as i64;
    let last = (hi / step).floor() as i64;
    // `+ 0.0` turns a negative zero into zero.
    let ticks = (first..=last).map(|i| i as f64 * step + 0.0).collect();
    (ticks, step)
}

/// Major ticks (in axis units) inside `[lo, hi]` with their labels, minor
/// tick positions, and the step readouts use, for an axis of `scale`.
pub fn scale_ticks(
    scale: AxisScale,
    (lo, hi): (f64, f64),
    target: usize,
) -> (Vec<(f64, String)>, Vec<f64>, f64) {
    if scale == AxisScale::Linear {
        let (values, step) = ticks(lo, hi, target);
        let major = values
            .into_iter()
            .map(|v| (v, tick_label(v, step)))
            .collect();
        return (major, Vec::new(), step);
    }
    let first = lo.ceil() as i64;
    let last = hi.floor() as i64;
    if last - first < 1 {
        // Less than two decades in view: nice ticks of the values.
        let (data_lo, data_hi) = (10f64.powf(lo), 10f64.powf(hi));
        let (values, step) = ticks(data_lo, data_hi, target);
        let major = values
            .into_iter()
            .filter(|v| *v > 0.0)
            .map(|v| (v.log10(), tick_label(v, step)))
            .collect();
        return (major, Vec::new(), step);
    }
    // Every `every`-th decade, so labels do not crowd.
    let every = ((last - first) as usize).div_ceil(target.max(1)).max(1) as i64;
    let major = (first..=last)
        .filter(|k| k.rem_euclid(every) == 0)
        .map(|k| (k as f64, decade_label(k)))
        .collect();
    let mut minor = Vec::new();
    if every == 1 {
        for k in (lo.floor() as i64)..=(last) {
            for m in 2..=9 {
                let u = k as f64 + f64::from(m).log10();
                if u > lo && u < hi {
                    minor.push(u);
                }
            }
        }
    }
    (major, minor, 1.0)
}

/// The label of decade `10^k`: plain for `1e-3..1e5`, else `1e{k}`.
pub fn decade_label(k: i64) -> String {
    let value = 10f64.powi(k as i32);
    if (-3..=5).contains(&k) {
        tick_label(value, value)
    } else {
        format!("1e{k}")
    }
}

/// A readout of `u` axis units as a value: as [`readout`] on a linear
/// axis, with about four significant digits on a log one.
pub fn axis_readout(u: f64, scale: AxisScale, step: f64) -> String {
    match scale {
        AxisScale::Linear => readout(u, step),
        AxisScale::Log => {
            let value = scale.inverse(u);
            readout(value, 10f64.powf(value.abs().log10().floor()) * 10.0)
        }
    }
}

/// Decimals that tell ticks `step` apart.
pub fn step_decimals(step: f64) -> usize {
    if !(step.is_finite() && step > 0.0) {
        return 2;
    }
    (-step.log10().floor()).clamp(0.0, 10.0) as usize
}

/// A tick label: compact for big numbers, otherwise with the decimals its
/// step needs (`0.0002`, not `0`).
pub fn tick_label(value: f64, step: f64) -> String {
    if value.abs() >= 1e4 || step >= 1e3 {
        return format_compact(value);
    }
    trim(&format!("{value:.*}", step_decimals(step)))
}

/// A readout of `value` with about 4 significant digits beyond the view's
/// tick step.
pub fn readout(value: f64, step: f64) -> String {
    if !value.is_finite() {
        return "undefined".to_owned();
    }
    if value != 0.0 && (value.abs() >= 1e6 || value.abs() < 1e-4) {
        return format!("{value:.4e}");
    }
    trim(&format!("{value:.*}", step_decimals(step) + 2))
}

/// `s` without trailing fractional zeros, never `-0`.
fn trim(s: &str) -> String {
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    };
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// A tick and where it falls on its axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub pos: f32,
}

/// Where everything of a plot goes for one width.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotLayout {
    pub view: View,
    pub axes: Axes,
    /// The plot area.
    pub plot: Rect,
    pub x_ticks: Vec<Tick>,
    pub y_ticks: Vec<Tick>,
    /// Positions of the minor x grid lines (log axes).
    pub x_minor: Vec<f32>,
    /// Positions of the minor y grid lines (log axes).
    pub y_minor: Vec<f32>,
    pub x_step: f64,
    pub y_step: f64,
    /// Tick labels and axis titles.
    pub labels: Vec<Label>,
    /// The whole element's height.
    pub height: f32,
}

impl PlotLayout {
    /// Screen position of a data point.
    pub fn to_px(&self, p: (f64, f64)) -> (f32, f32) {
        let (nx, ny) = self.view.normalize(p);
        let nx = if self.axes.x_reversed { 1.0 - nx } else { nx };
        (
            self.plot.x + (nx as f32) * self.plot.w,
            self.plot.bottom() - (ny as f32) * self.plot.h,
        )
    }

    /// The data point at a screen position.
    pub fn to_data(&self, x: f32, y: f32) -> (f64, f64) {
        let nx = f64::from((x - self.plot.x) / self.plot.w.max(1.0));
        let nx = if self.axes.x_reversed { 1.0 - nx } else { nx };
        let ny = f64::from((self.plot.bottom() - y) / self.plot.h.max(1.0));
        self.view.denormalize((nx, ny))
    }
}

/// Plot area height for an element `width` wide.
fn plot_height(width: f32) -> f32 {
    (width * ASPECT).clamp(MIN_PLOT_HEIGHT, MAX_PLOT_HEIGHT)
}

/// Lays out `view` with its axis titles in `width` pixels.
pub fn plan(
    view: View,
    x_title: Option<&str>,
    y_title: Option<&str>,
    width: f32,
    measure: Measure,
    axes: Axes,
) -> PlotLayout {
    let auto = plan_with(view, x_title, y_title, width, None, measure, axes);
    if !axes.equal {
        return auto;
    }
    // Pixels per unit equal on both axes: the height follows the width
    // and the view's shape; too tall a plot gets narrower instead.
    let (x_span, y_span) = (view.x.1 - view.x.0, view.y.1 - view.y.0);
    let ratio = (y_span / x_span) as f32;
    let full_w = auto.plot.w;
    let mut h = full_w * ratio;
    let mut w = full_w;
    if h > MAX_EQUAL_HEIGHT {
        h = MAX_EQUAL_HEIGHT;
        w = h / ratio;
    }
    let h = h.max(MIN_EQUAL_HEIGHT);
    plan_with(view, x_title, y_title, width, Some((h, w)), measure, axes)
}

/// [`plan`] with the plot area height, or its height and width, fixed.
fn plan_with(
    view: View,
    x_title: Option<&str>,
    y_title: Option<&str>,
    width: f32,
    area: Option<(f32, f32)>,
    measure: Measure,
    axes: Axes,
) -> PlotLayout {
    let plot_h = area.map_or_else(|| plot_height(width), |(h, _)| h);
    let y_target = ((plot_h / Y_TICK_SPACING) as usize).clamp(2, 8);
    let (y_major, y_minor, y_step) = scale_ticks(axes.y, view.y, y_target);
    let label_w = y_major
        .iter()
        .map(|(_, t)| measure(t))
        .fold(0.0f32, f32::max);
    let left = PAD + label_w + LABEL_GAP;
    let top = PAD
        + if y_title.is_some() {
            LABEL_HEIGHT + 2.0
        } else {
            0.0
        };
    let right = PAD * 2.0;
    let available = (width - left - right).max(1.0);
    let plot_w = area.map_or(available, |(_, w)| w.min(available));
    let plot = Rect::new(left, top, plot_w, plot_h);
    let x_target = ((plot_w / X_TICK_SPACING) as usize).clamp(2, 10);
    let (x_major, x_minor, x_step) = scale_ticks(axes.x, view.x, x_target);
    let mut layout = PlotLayout {
        view,
        axes,
        plot,
        x_ticks: Vec::new(),
        y_ticks: Vec::new(),
        x_minor: Vec::new(),
        y_minor: Vec::new(),
        x_step,
        y_step,
        labels: Vec::new(),
        height: 0.0,
    };
    for (value, text) in y_major {
        let pos = layout.to_px((view.x.0, value)).1;
        layout.labels.push(Label {
            text,
            x: left - LABEL_GAP,
            y: pos - LABEL_HEIGHT / 2.0,
            align: Align::Right,
        });
        layout.y_ticks.push(Tick { value, pos });
    }
    layout.y_minor = y_minor
        .into_iter()
        .map(|v| layout.to_px((view.x.0, v)).1)
        .collect();
    let labels_top = plot.bottom() + LABEL_GAP;
    for (value, text) in x_major {
        let pos = layout.to_px((value, view.y.0)).0;
        layout.labels.push(Label {
            text,
            x: pos,
            y: labels_top,
            align: Align::Center,
        });
        layout.x_ticks.push(Tick { value, pos });
    }
    layout.x_minor = x_minor
        .into_iter()
        .map(|v| layout.to_px((v, view.y.0)).0)
        .collect();
    let mut bottom = labels_top + LABEL_HEIGHT;
    if let Some(title) = x_title {
        layout.labels.push(Label {
            text: title.to_owned(),
            x: plot.center_x(),
            y: bottom + 2.0,
            align: Align::Center,
        });
        bottom += LABEL_HEIGHT + 2.0;
    }
    if let Some(title) = y_title {
        layout.labels.push(Label {
            text: title.to_owned(),
            x: PAD,
            y: PAD,
            align: Align::Left,
        });
    }
    layout.height = bottom + PAD / 2.0;
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str) -> f32 {
        text.chars().count() as f32 * 7.0
    }

    #[test]
    fn a_reversed_x_axis_runs_right_to_left() {
        let view = View {
            x: (0.0, 10.0),
            y: (0.0, 1.0),
        };
        let reversed = Axes {
            x_reversed: true,
            ..Axes::default()
        };
        let layout = plan(view, None, None, 600.0, &measure, reversed);
        let (left, _) = layout.to_px((10.0, 0.0));
        let (right, _) = layout.to_px((0.0, 0.0));
        assert!((left - layout.plot.x).abs() < 1e-3 && right > left);
        let (x, _) = layout.to_data(left, layout.plot.bottom());
        assert!((x - 10.0).abs() < 1e-6);
        // The tick labels follow: the first tick (0) sits at the right.
        assert!(layout.x_ticks[0].pos > layout.x_ticks[1].pos);
    }

    #[test]
    fn ticks_stay_inside_the_view() {
        let (ticks, step) = ticks(-0.3, 2.7, 6);
        assert_eq!(step, 0.5);
        assert_eq!(ticks, [0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        assert_eq!(nice_range(0.3, 9.2, 5), (0.0, 10.0));
        assert_eq!(widen(2.0, 2.0), (1.8, 2.2));
        assert_eq!(widen(f64::NAN, 1.0), (-1.0, 1.0));
    }

    #[test]
    fn tick_labels_keep_the_precision_of_their_step() {
        assert_eq!(tick_label(0.0004, 0.0002), "0.0004");
        assert_eq!(tick_label(2.5, 0.5), "2.5");
        assert_eq!(tick_label(-0.0, 1.0), "0");
        assert_eq!(tick_label(20_000.0, 5_000.0), "20K");
        assert_eq!(readout(1.234_567, 0.5), "1.235");
        assert_eq!(readout(f64::INFINITY, 0.5), "undefined");
        assert_eq!(readout(1.5e-7, 1.0), "1.5000e-7");
    }

    #[test]
    fn the_layout_maps_the_view_onto_the_plot_area() {
        let view = View {
            x: (0.0, 10.0),
            y: (-1.0, 1.0),
        };
        let layout = plan(
            view,
            Some("time (s)"),
            Some("amplitude"),
            600.0,
            &measure,
            Axes::default(),
        );
        let plot = layout.plot;
        assert_eq!(layout.to_px((0.0, -1.0)), (plot.x, plot.bottom()));
        assert_eq!(layout.to_px((10.0, 1.0)), (plot.right(), plot.y));
        let (x, y) = layout.to_data(plot.center_x(), plot.center_y());
        assert!((x - 5.0).abs() < 1e-6 && y.abs() < 1e-6);
        assert!(layout.height > plot.bottom() + 2.0 * LABEL_HEIGHT);
        assert!(layout.labels.iter().any(|l| l.text == "amplitude"));
        assert!(layout.x_ticks.len() >= 3);
        assert!(layout.y_ticks.iter().any(|t| t.value == 0.0));
        let wider = plan(view, None, None, 1000.0, &measure, Axes::default());
        assert!(wider.plot.h > plot.h);
    }

    #[test]
    fn log_axes_tick_at_decades_with_minor_ticks() {
        // 1..1000 in decades.
        let (major, minor, _) = scale_ticks(AxisScale::Log, (0.0, 3.0), 6);
        let labels: Vec<&str> = major.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(labels, ["1", "10", "100", "1K"]);
        assert_eq!(minor.len(), 3 * 8);
        assert!((minor[0] - 2f64.log10()).abs() < 1e-12);
        // Many decades: every other one, no minor ticks.
        let (major, minor, _) = scale_ticks(AxisScale::Log, (-12.0, 12.0), 6);
        assert!(major.len() <= 7 && minor.is_empty());
        assert!(major.iter().any(|(_, t)| t == "1e-12"));
        // Under a decade: nice values.
        let (major, _, _) = scale_ticks(AxisScale::Log, (2f64.log10(), 8f64.log10()), 4);
        assert!(major.iter().any(|(_, t)| t == "4"));
        assert_eq!(decade_label(-3), "0.001");
        assert_eq!(decade_label(4), "10K");
        assert_eq!(axis_readout(2.0, AxisScale::Log, 1.0), "100");
        assert_eq!(axis_readout(2.5, AxisScale::Linear, 0.5), "2.5");
    }

    #[test]
    fn equal_aspect_makes_units_equally_long() {
        let view = View {
            x: (-2.0, 2.0),
            y: (-1.0, 1.0),
        };
        let equal = Axes {
            equal: true,
            ..Axes::default()
        };
        let layout = plan(view, None, None, 600.0, &measure, equal);
        let unit_x = layout.plot.w / 4.0;
        let unit_y = layout.plot.h / 2.0;
        assert!((unit_x - unit_y).abs() < 0.01, "{unit_x} vs {unit_y}");
        // A tall view gets a narrower plot, not a huge height.
        let tall = View {
            x: (0.0, 1.0),
            y: (0.0, 10.0),
        };
        let layout = plan(tall, None, None, 600.0, &measure, equal);
        assert_eq!(layout.plot.h, MAX_EQUAL_HEIGHT);
        assert!((layout.plot.w - MAX_EQUAL_HEIGHT / 10.0).abs() < 0.01);
    }
}
