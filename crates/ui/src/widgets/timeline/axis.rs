//! The time axis of a timeline: the visible window of time, its zoom and
//! pan, the ticks and their labels, and which span is under the cursor.
//!
//! Everything here is pure arithmetic on numbers in the spec's unit, so it
//! is tested without a window.

use std::ops::Range;

use domain::widget::timeline::{Span, TimeUnit, date_parts};

use crate::blocks::chart::scale::nice_step;

/// Narrowest window, as a fraction of the extent (a million-fold zoom).
pub const MIN_ZOOM_FRACTION: f64 = 1e-6;
/// Pixels between ticks the axis aims for.
pub const TICK_SPACING: f32 = 96.0;

/// The interval of time shown across the data area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Time at the left edge.
    pub start: f64,
    /// Time at the right edge.
    pub end: f64,
}

impl Viewport {
    /// The whole `extent`.
    pub fn fit(extent: (f64, f64)) -> Self {
        Self {
            start: extent.0,
            end: extent.1,
        }
    }

    /// Length of the window.
    pub fn span(&self) -> f64 {
        self.end - self.start
    }

    /// Screen x (from the data area's left edge) of time `t`.
    pub fn x_of(&self, t: f64, width: f32) -> f32 {
        ((t - self.start) / self.span() * f64::from(width)) as f32
    }

    /// Time at screen x (from the data area's left edge).
    pub fn t_of(&self, x: f32, width: f32) -> f64 {
        self.start + f64::from(x) / f64::from(width.max(1.0)) * self.span()
    }

    /// How far in the window is over `extent` (1 = fitted).
    pub fn zoom(&self, extent: (f64, f64)) -> f64 {
        (extent.1 - extent.0) / self.span()
    }

    /// Divides the window by `factor` (> 1 zooms in) keeping the time at
    /// `anchor` (0 = left edge, 1 = right edge) in place, within `extent`:
    /// never wider than it, nor narrower than [`MIN_ZOOM_FRACTION`] of it.
    pub fn zoom_at(&mut self, factor: f64, anchor: f64, extent: (f64, f64)) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let full = extent.1 - extent.0;
        let span = (self.span() / factor).clamp(full * MIN_ZOOM_FRACTION, full);
        let anchor = anchor.clamp(0.0, 1.0);
        let at = self.start + anchor * self.span();
        self.start = at - anchor * span;
        self.end = self.start + span;
        self.clamp(extent);
    }

    /// Moves the window by `dx` pixels of a `width`-pixel data area
    /// (positive moves the content right, so earlier times come in).
    pub fn pan_by(&mut self, dx: f32, width: f32, extent: (f64, f64)) {
        let dt = f64::from(dx) / f64::from(width.max(1.0)) * self.span();
        self.start -= dt;
        self.end -= dt;
        self.clamp(extent);
    }

    /// Keeps the window inside `extent`, shifting it without resizing.
    fn clamp(&mut self, extent: (f64, f64)) {
        let span = self.span().min(extent.1 - extent.0);
        if self.start < extent.0 {
            self.start = extent.0;
        }
        if self.start + span > extent.1 {
            self.start = extent.1 - span;
        }
        self.end = self.start + span;
    }
}

/// One tick of the axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Time of the tick.
    pub at: f64,
    /// Its label.
    pub label: String,
}

/// `n` with at most `decimals` decimals, without trailing zeros.
fn trimmed(n: f64, decimals: usize) -> String {
    let s = format!("{n:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" { "0".to_owned() } else { s }
}

/// Decimals that tell apart multiples of `step`.
fn decimals_for(step: f64) -> usize {
    if step >= 1.0 {
        0
    } else {
        (-step.log10()).ceil().clamp(0.0, 9.0) as usize
    }
}

/// The unit labels use for ticks `step_seconds` apart on an axis that
/// reaches `max_seconds`: the largest in which the step is at least 1,
/// or the next one up when the values would pass 1000 in it (`1.1 s`
/// rather than `1100 ms`). Seconds at most: nice steps of seconds do not
/// fall on minutes.
fn label_unit(step_seconds: f64, max_seconds: f64) -> (f64, &'static str) {
    const UNITS: [(f64, &str); 4] = [(1.0, "s"), (1e-3, "ms"), (1e-6, "µs"), (1e-9, "ns")];
    let ix = UNITS
        .iter()
        .position(|(scale, _)| step_seconds >= *scale * 0.999)
        .unwrap_or(UNITS.len() - 1);
    let ix = if ix > 0 && max_seconds / UNITS[ix].0 > 1_000.5 {
        ix - 1
    } else {
        ix
    };
    UNITS[ix]
}

/// Calendar steps of a date axis, in seconds.
const DATE_STEPS: [f64; 22] = [
    1.0,
    2.0,
    5.0,
    10.0,
    15.0,
    30.0,
    60.0,
    120.0,
    300.0,
    600.0,
    900.0,
    1_800.0,
    3_600.0,
    7_200.0,
    10_800.0,
    21_600.0,
    43_200.0,
    86_400.0,
    172_800.0,
    604_800.0,
    2_592_000.0,
    31_536_000.0,
];

/// Ticks of `viewport` for a data area `width` pixels wide, labelled in a
/// unit that suits their spacing (`200 ms`, `1.5 s`) or, on a date axis,
/// as clock times or days.
pub fn ticks(viewport: Viewport, unit: TimeUnit, width: f32) -> Vec<Tick> {
    let target = (width / TICK_SPACING).clamp(2.0, 12.0) as usize;
    let span = viewport.span();
    if !(span.is_finite() && span > 0.0) {
        return Vec::new();
    }
    let step = match unit {
        TimeUnit::Date => {
            let raw = span / target as f64;
            if raw < 1.0 {
                nice_step(span, target)
            } else {
                DATE_STEPS
                    .into_iter()
                    .find(|s| *s >= raw)
                    .unwrap_or_else(|| nice_step(span / 31_536_000.0, target) * 31_536_000.0)
            }
        }
        _ => nice_step(span, target),
    };
    let first = (viewport.start / step).ceil() as i64;
    let last = (viewport.end / step).floor() as i64;
    if last - first > 200 {
        return Vec::new();
    }
    let max = viewport.start.abs().max(viewport.end.abs());
    (first..=last)
        .map(|i| {
            let at = i as f64 * step + 0.0;
            Tick {
                at,
                label: tick_label(at, step, max, unit),
            }
        })
        .collect()
}

/// Label of a tick at `at`, ticks being `step` apart on an axis whose
/// largest absolute time is `max`.
pub fn tick_label(at: f64, step: f64, max: f64, unit: TimeUnit) -> String {
    if unit == TimeUnit::Date {
        let digits = decimals_for(step).clamp(1, 6);
        // Half a last digit up, then cut: 12.99999 s is 13.0, not 12.10.
        let unit_digit = 10f64.powi(-(digits as i32));
        let at = if step < 1.0 {
            at + unit_digit / 2.0
        } else {
            at + 0.5e-3
        };
        let ((y, mo, d), (h, mi, s, fraction)) = date_parts(at);
        return if step < 1.0 {
            let scaled = (fraction / unit_digit).floor() as u64;
            let frac = format!("{scaled:0digits$}");
            let frac = frac.trim_end_matches('0');
            let frac = if frac.is_empty() { "0" } else { frac };
            format!("{h:02}:{mi:02}:{s:02}.{frac}")
        } else if step < 60.0 {
            format!("{h:02}:{mi:02}:{s:02}")
        } else if step < 86_400.0 {
            format!("{h:02}:{mi:02}")
        } else if step < 2_592_000.0 {
            format!("{mo:02}-{d:02}")
        } else {
            format!("{y:04}-{mo:02}")
        };
    }
    let seconds = unit.seconds();
    let (scale, suffix) = label_unit(step * seconds, max * seconds);
    let value = at * seconds / scale;
    if value == 0.0 {
        return "0".to_owned();
    }
    let decimals = decimals_for(step * seconds / scale);
    format!("{} {suffix}", trimmed(value, decimals))
}

/// Rows (of `row_height` pixels, `count` in all) that show in a body
/// `height` pixels tall scrolled down by `scroll` pixels.
pub fn visible_rows(scroll: f32, height: f32, row_height: f32, count: usize) -> Range<usize> {
    if count == 0 || row_height <= 0.0 {
        return 0..0;
    }
    let first = (scroll.max(0.0) / row_height).floor() as usize;
    let last = ((scroll.max(0.0) + height) / row_height).ceil() as usize;
    first.min(count)..last.min(count)
}

/// The span of `spans` under screen x (from the data area's left edge),
/// within `slop` pixels. Of overlapping spans the shortest wins (the
/// innermost call of a trace).
pub fn hit_span(
    spans: &[Span],
    viewport: Viewport,
    width: f32,
    x: f32,
    slop: f32,
) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .filter(|(_, span)| {
            let x0 = viewport.x_of(span.start, width);
            let x1 = viewport.x_of(span.end, width).max(x0 + 1.0);
            x >= x0 - slop && x <= x1 + slop
        })
        .min_by(|(_, a), (_, b)| a.duration().total_cmp(&b.duration()))
        .map(|(ix, _)| ix)
}

/// Palette slot (0–4) of a color key: stable across runs and machines.
pub fn color_slot(key: &str) -> usize {
    // FNV-1a.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash % 5) as usize
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn span(start: f64, end: f64) -> Span {
        Span {
            start,
            end,
            label: None,
            color_key: None,
            status: None,
            attrs: Map::new(),
        }
    }

    #[test]
    fn zoom_keeps_the_anchor_and_stays_in_the_extent() {
        let extent = (0.0, 100.0);
        let mut view = Viewport::fit(extent);
        view.zoom_at(2.0, 0.25, extent);
        assert_eq!(
            view,
            Viewport {
                start: 12.5,
                end: 62.5
            }
        );
        assert!((view.t_of(250.0, 1000.0) - 25.0).abs() < 1e-9);
        // Zooming out past the extent fits it again.
        view.zoom_at(0.1, 0.5, extent);
        assert_eq!(view, Viewport::fit(extent));
        // A million-fold at most.
        view.zoom_at(1e12, 0.5, extent);
        assert!((view.span() - 100.0 * MIN_ZOOM_FRACTION).abs() < 1e-12);
        assert!((view.zoom(extent) - 1e6).abs() < 1.0);
    }

    #[test]
    fn pan_moves_by_pixels_and_stops_at_the_edges() {
        let extent = (0.0, 100.0);
        let mut view = Viewport {
            start: 40.0,
            end: 60.0,
        };
        view.pan_by(-100.0, 1000.0, extent);
        assert!((view.start - 42.0).abs() < 1e-9 && (view.end - 62.0).abs() < 1e-9);
        view.pan_by(1e6, 1000.0, extent);
        assert_eq!(
            view,
            Viewport {
                start: 0.0,
                end: 20.0
            }
        );
        view.pan_by(-1e6, 1000.0, extent);
        assert_eq!(
            view,
            Viewport {
                start: 80.0,
                end: 100.0
            }
        );
    }

    #[test]
    fn tick_labels_follow_their_spacing() {
        let labels = |start, end, unit, width| -> Vec<String> {
            ticks(Viewport { start, end }, unit, width)
                .into_iter()
                .map(|t| t.label)
                .collect()
        };
        assert_eq!(
            labels(0.0, 1000.0, TimeUnit::Ms, 500.0),
            ["0", "200 ms", "400 ms", "600 ms", "800 ms", "1000 ms"]
        );
        assert_eq!(
            labels(0.0, 5000.0, TimeUnit::Ms, 500.0),
            ["0", "1 s", "2 s", "3 s", "4 s", "5 s"]
        );
        assert_eq!(labels(0.0, 1.0, TimeUnit::Ms, 500.0)[1], "200 µs");
        assert_eq!(labels(0.0, 3.0, TimeUnit::S, 300.0)[1], "1 s");
        assert_eq!(labels(1.0, 1.5, TimeUnit::S, 600.0)[1], "1.1 s");
        assert_eq!(labels(0.0, 7200.0, TimeUnit::S, 200.0)[1], "5000 s");
        assert!(
            ticks(
                Viewport {
                    start: 0.0,
                    end: 0.0
                },
                TimeUnit::Ms,
                500.0
            )
            .is_empty()
        );
    }

    #[test]
    fn date_ticks_are_clock_times_or_days() {
        // 2025-10-24 00:00 UTC.
        let day = 1_761_264_000.0;
        let hours = ticks(
            Viewport {
                start: day,
                end: day + 6.0 * 3_600.0,
            },
            TimeUnit::Date,
            600.0,
        );
        assert_eq!(hours[0].label, "00:00");
        assert_eq!(hours[1].label, "01:00");
        let days = ticks(
            Viewport {
                start: day,
                end: day + 6.0 * 86_400.0,
            },
            TimeUnit::Date,
            600.0,
        );
        assert_eq!(days[0].label, "10-24");
        assert_eq!(days[1].label, "10-25");
        let fine = ticks(
            Viewport {
                start: day,
                end: day + 0.5,
            },
            TimeUnit::Date,
            600.0,
        );
        assert_eq!(fine[1].label, "00:00:00.1");
    }

    #[test]
    fn rows_are_virtualized() {
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 1_000), 0..5);
        assert_eq!(visible_rows(30.0, 100.0, 20.0, 1_000), 1..7);
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 3), 0..3);
        assert_eq!(visible_rows(19_990.0, 100.0, 20.0, 1_000), 999..1_000);
        assert_eq!(visible_rows(0.0, 100.0, 20.0, 0), 0..0);
    }

    #[test]
    fn the_innermost_span_is_hit() {
        let view = Viewport {
            start: 0.0,
            end: 100.0,
        };
        let spans = [span(0.0, 100.0), span(40.0, 60.0), span(90.0, 90.0)];
        assert_eq!(hit_span(&spans, view, 1000.0, 500.0, 2.0), Some(1));
        assert_eq!(hit_span(&spans, view, 1000.0, 100.0, 2.0), Some(0));
        // An instant is one pixel wide plus the slop.
        assert_eq!(hit_span(&spans, view, 1000.0, 902.0, 2.0), Some(2));
        assert_eq!(hit_span(&spans[1..], view, 1000.0, 100.0, 2.0), None);
    }

    #[test]
    fn color_slots_are_stable() {
        assert_eq!(color_slot("db"), color_slot("db"));
        assert!(color_slot("frontend") < 5);
    }
}
