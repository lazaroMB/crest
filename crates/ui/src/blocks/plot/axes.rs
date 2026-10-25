//! The numeric axes of a plot: the visible view, nice ticks from the chart
//! scale ([`nice_step`]), tick labels written to the step's precision, and
//! the layout of the plot area inside the element, all in pixels relative
//! to the element's top-left corner.

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
    /// The plot area.
    pub plot: Rect,
    pub x_ticks: Vec<Tick>,
    pub y_ticks: Vec<Tick>,
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
        (
            self.plot.x + (nx as f32) * self.plot.w,
            self.plot.bottom() - (ny as f32) * self.plot.h,
        )
    }

    /// The data point at a screen position.
    pub fn to_data(&self, x: f32, y: f32) -> (f64, f64) {
        let nx = f64::from((x - self.plot.x) / self.plot.w.max(1.0));
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
) -> PlotLayout {
    let plot_h = plot_height(width);
    let y_target = ((plot_h / Y_TICK_SPACING) as usize).clamp(2, 8);
    let (y_values, y_step) = ticks(view.y.0, view.y.1, y_target);
    let y_texts: Vec<String> = y_values.iter().map(|v| tick_label(*v, y_step)).collect();
    let label_w = y_texts.iter().map(|t| measure(t)).fold(0.0f32, f32::max);
    let left = PAD + label_w + LABEL_GAP;
    let top = PAD
        + if y_title.is_some() {
            LABEL_HEIGHT + 2.0
        } else {
            0.0
        };
    let right = PAD * 2.0;
    let plot_w = (width - left - right).max(1.0);
    let plot = Rect::new(left, top, plot_w, plot_h);
    let x_target = ((plot_w / X_TICK_SPACING) as usize).clamp(2, 10);
    let (x_values, x_step) = ticks(view.x.0, view.x.1, x_target);
    let mut layout = PlotLayout {
        view,
        plot,
        x_ticks: Vec::new(),
        y_ticks: Vec::new(),
        x_step,
        y_step,
        labels: Vec::new(),
        height: 0.0,
    };
    for (value, text) in y_values.into_iter().zip(y_texts) {
        let pos = layout.to_px((view.x.0, value)).1;
        layout.labels.push(Label {
            text,
            x: left - LABEL_GAP,
            y: pos - LABEL_HEIGHT / 2.0,
            align: Align::Right,
        });
        layout.y_ticks.push(Tick { value, pos });
    }
    let labels_top = plot.bottom() + LABEL_GAP;
    for value in x_values {
        let pos = layout.to_px((value, view.y.0)).0;
        layout.labels.push(Label {
            text: tick_label(value, x_step),
            x: pos,
            y: labels_top,
            align: Align::Center,
        });
        layout.x_ticks.push(Tick { value, pos });
    }
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
        let layout = plan(view, Some("time (s)"), Some("amplitude"), 600.0, &measure);
        let plot = layout.plot;
        assert_eq!(layout.to_px((0.0, -1.0)), (plot.x, plot.bottom()));
        assert_eq!(layout.to_px((10.0, 1.0)), (plot.right(), plot.y));
        let (x, y) = layout.to_data(plot.center_x(), plot.center_y());
        assert!((x - 5.0).abs() < 1e-6 && y.abs() < 1e-6);
        assert!(layout.height > plot.bottom() + 2.0 * LABEL_HEIGHT);
        assert!(layout.labels.iter().any(|l| l.text == "amplitude"));
        assert!(layout.x_ticks.len() >= 3);
        assert!(layout.y_ticks.iter().any(|t| t.value == 0.0));
        let wider = plan(view, None, None, 1000.0, &measure);
        assert!(wider.plot.h > plot.h);
    }
}
