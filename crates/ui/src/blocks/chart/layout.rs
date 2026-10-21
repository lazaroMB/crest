//! Chart geometry: where every bar, tick and label goes for a given width.
//!
//! Pure and testable: text is measured through an injected function, so the
//! same [`plan`] gives the layout's height (through
//! [`measured`](crate::blocks::measured)) and the painted marks, and the two
//! always agree. All positions are in pixels from the chart's top-left
//! corner.
//!
//! Bars are sized from the data, not stretched to the width: a bar is at most
//! [`BAR_MAX`] wide alone and [`GROUPED_BAR_MAX`] in a group, bars of a group
//! sit [`INNER_GAP`] apart and groups half a group apart (within
//! [`GROUP_GAP_MIN`]..[`GROUP_GAP_MAX`]). The plot is as wide as the bars
//! need (at least [`MIN_PLOT_WIDTH`]), centered, with each group centered in
//! its slot.

use domain::block::{ChartKind, ChartSpec};

use super::scale::{format_compact, nice_ticks_n, value_domain};

/// Measures the width in pixels of a label drawn in the chart's font.
pub type Measure<'a> = &'a dyn Fn(&str) -> f32;

/// Widest bar of a single-series chart.
pub const BAR_MAX: f32 = 40.0;
/// Widest bar of a group (several series).
pub const GROUPED_BAR_MAX: f32 = 24.0;
/// Narrowest bar drawn upright; narrower ones turn the chart on its side.
pub const BAR_MIN: f32 = 6.0;
/// Space between the bars of a group.
pub const INNER_GAP: f32 = 2.0;
/// Smallest space between two groups.
pub const GROUP_GAP_MIN: f32 = 12.0;
/// Largest space between two groups.
pub const GROUP_GAP_MAX: f32 = 48.0;
/// Narrowest plot area, however few bars there are.
pub const MIN_PLOT_WIDTH: f32 = 320.0;
/// Plot height as a fraction of the chart width.
pub const ASPECT: f32 = 0.5;
/// Shortest plot area.
pub const MIN_PLOT_HEIGHT: f32 = 200.0;
/// Tallest plot area of an upright chart.
pub const MAX_PLOT_HEIGHT: f32 = 360.0;
/// Height of a line of label text.
pub const LABEL_HEIGHT: f32 = 16.0;
/// Space between an axis and its labels.
pub const LABEL_GAP: f32 = 6.0;
/// Space kept at the chart's edges.
pub const PAD: f32 = 8.0;
/// Most categories drawn upright.
pub const MAX_UPRIGHT_CATEGORIES: usize = 16;
/// Most bars drawn upright.
pub const MAX_UPRIGHT_BARS: usize = 30;
/// Fewest characters an upright category label may be cut to.
pub const MIN_LABEL_CHARS: usize = 6;
/// Smallest share of its length an upright category label may be cut to.
pub const MIN_LABEL_SHARE: f32 = 0.7;
/// Widest bar of a single-series chart on its side.
pub const H_BAR_MAX: f32 = 24.0;
/// Widest bar of a group on its side.
pub const H_GROUPED_BAR_MAX: f32 = 12.0;
/// Smallest space between two groups on a chart on its side.
pub const H_GROUP_GAP_MIN: f32 = 8.0;
/// Largest space between two groups on a chart on its side.
pub const H_GROUP_GAP_MAX: f32 = 24.0;
/// Largest share of the width the category labels of a chart on its side
/// take; longer labels are cut with an ellipsis.
pub const MAX_LABEL_MARGIN: f32 = 0.35;
/// Most points of a line that get a marker.
pub const MAX_MARKERS: usize = 24;
/// Diameter of a point marker.
pub const MARKER_SIZE: f32 = 8.0;
/// Widest x-axis label of a line or area chart; longer ones are cut.
pub const MAX_X_LABEL: f32 = 120.0;
/// Least space between two x-axis labels.
pub const X_LABEL_SPACE: f32 = 8.0;
/// Most bars that get their value written at the tip.
pub const MAX_VALUE_LABELS: usize = 12;

/// The chart's data in drawing order: `values[series][label]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    /// How the series are drawn.
    pub kind: ChartKind,
    /// Category labels.
    pub labels: Vec<String>,
    /// Series names.
    pub names: Vec<String>,
    /// One row of values per series, aligned with `labels`.
    pub values: Vec<Vec<f64>>,
}

impl ChartData {
    /// The data of a chart block.
    pub fn from_spec(spec: &ChartSpec) -> Self {
        Self {
            kind: spec.kind,
            labels: spec.labels.clone(),
            names: spec.series.iter().map(|s| s.name.clone()).collect(),
            values: spec.series.iter().map(|s| s.values.clone()).collect(),
        }
    }

    /// Value of `series` at category `label`, zero when missing.
    pub fn value(&self, series: usize, label: usize) -> f64 {
        self.values
            .get(series)
            .and_then(|v| v.get(label))
            .copied()
            .filter(|v| v.is_finite())
            .unwrap_or_default()
    }

    /// Number of series, at least one.
    pub fn series_count(&self) -> usize {
        self.values.len().max(1)
    }

    fn all_values(&self) -> impl Iterator<Item = f64> + '_ {
        self.values.iter().flatten().copied()
    }
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }
}

/// How a label sits on its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// The anchor is the left edge.
    Left,
    /// The anchor is the center.
    Center,
    /// The anchor is the right edge.
    Right,
}

/// A line of text: anchored at `x` per `align`, its top at `y`.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub align: Align,
}

impl Label {
    /// Left and right edges of the label, measured with `measure`.
    pub fn span(&self, measure: Measure) -> (f32, f32) {
        let w = measure(&self.text);
        match self.align {
            Align::Left => (self.x, self.x + w),
            Align::Center => (self.x - w / 2.0, self.x + w / 2.0),
            Align::Right => (self.x - w, self.x),
        }
    }
}

/// Which way bars grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Upright bars over category labels.
    Vertical,
    /// Bars growing right from labels on the left.
    Horizontal,
}

/// A value-axis tick: its value and where it falls along the value axis
/// (`y` upright, `x` on its side).
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub pos: f32,
}

/// One category's band of bars.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// Category index.
    pub label: usize,
    /// The category's slot across the plot.
    pub slot: Rect,
    /// The bars of the group, without the space around them.
    pub bars: Rect,
}

/// One bar.
#[derive(Debug, Clone, PartialEq)]
pub struct BarMark {
    /// Category index.
    pub label: usize,
    /// Series index.
    pub series: usize,
    pub value: f64,
    pub rect: Rect,
    /// Whether the bar grows below (or left of) the baseline; its rounded end
    /// is then the bottom (left) one.
    pub negative: bool,
}

/// Everything a chart paints, for one width.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLayout {
    pub width: f32,
    pub height: f32,
    pub orientation: Orientation,
    /// The area the marks are drawn in.
    pub plot: Rect,
    /// Value-axis ticks, from the lowest value up.
    pub ticks: Vec<Tick>,
    /// The tick labels, one per tick.
    pub tick_labels: Vec<Label>,
    /// Where zero falls along the value axis, when it is in the domain.
    pub baseline: Option<f32>,
    /// Category labels, possibly truncated.
    pub category_labels: Vec<Label>,
    /// Bar groups, one per category.
    pub groups: Vec<Group>,
    pub bars: Vec<BarMark>,
    /// The values written at the bar tips, if any.
    pub value_labels: Vec<Label>,
    /// Lines and areas: the x of each category.
    pub xs: Vec<f32>,
    /// Lines and areas: each series' points, `(x, y)` per category.
    pub points: Vec<Vec<(f32, f32)>>,
    /// Lines and areas: whether points get a marker (few points only).
    pub markers: bool,
}

/// The layout of `data` at `width`.
pub fn plan(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    let width = width.max(1.0);
    if matches!(data.kind, ChartKind::Line | ChartKind::Area) {
        return lines(data, width, measure);
    }
    match choose_orientation(data, width, measure) {
        Orientation::Vertical => vertical_bars(data, width, measure),
        Orientation::Horizontal => horizontal_bars(data, width, measure),
    }
}

/// The height of `data` drawn at `width`; see [`plan`].
pub fn height_for(data: &ChartData, width: f32, measure: Measure) -> f32 {
    plan(data, width, measure).height
}

/// `text` cut with an ellipsis to fit `max_width`, and how many characters
/// of it are kept.
pub fn truncate_to(text: &str, max_width: f32, measure: Measure) -> (String, usize) {
    let total = text.chars().count();
    if measure(text) <= max_width {
        return (text.to_owned(), total);
    }
    let cut = |n: usize| -> String {
        let mut s: String = text.chars().take(n).collect();
        s.push('…');
        s
    };
    let (mut lo, mut hi) = (0usize, total);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&cut(mid)) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (cut(lo), lo)
}

/// Plot height for a chart `width` wide.
fn plot_height(width: f32) -> f32 {
    (width * ASPECT).clamp(MIN_PLOT_HEIGHT, MAX_PLOT_HEIGHT)
}

/// Ticks for `data` over a value axis `length` pixels long.
fn ticks_for(data: &ChartData, length: f32) -> Vec<f64> {
    let (lo, hi) = value_domain(data.kind, data.all_values());
    let target = (length / 60.0).round().clamp(3.0, 6.0) as usize;
    nice_ticks_n(lo, hi, target)
}

/// Maps a value to a position along an axis from `start` (lowest tick) to
/// `end` (highest tick).
fn scale(ticks: &[f64], start: f32, end: f32) -> impl Fn(f64) -> f32 + use<> {
    let lo = ticks.first().copied().unwrap_or(0.0);
    let hi = ticks.last().copied().unwrap_or(1.0);
    let span = if hi > lo { hi - lo } else { 1.0 };
    move |v: f64| start + ((v - lo) / span) as f32 * (end - start)
}

/// Width of a group of `series` bars `bar` wide.
fn group_width(series: usize, bar: f32) -> f32 {
    series as f32 * bar + series.saturating_sub(1) as f32 * INNER_GAP
}

/// Space between groups `group` wide.
fn group_gap(group: f32) -> f32 {
    (group * 0.5).clamp(GROUP_GAP_MIN, GROUP_GAP_MAX)
}

/// The widest bar, up to `cap`, whose `groups` groups of `series` fit in
/// `room`; may fall under [`BAR_MIN`].
fn bar_thickness(groups: usize, series: usize, cap: f32, room: f32) -> f32 {
    let needed = |bar: f32| {
        let group = group_width(series, bar);
        groups as f32 * (group + group_gap(group))
    };
    if needed(cap) <= room {
        return cap;
    }
    let (mut lo, mut hi) = (0.0f32, cap);
    for _ in 0..32 {
        let mid = (lo + hi) / 2.0;
        if needed(mid) <= room {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The widest bar allowed for `series` series.
fn bar_cap(series: usize) -> f32 {
    if series > 1 { GROUPED_BAR_MAX } else { BAR_MAX }
}

/// Tick labels of the value axis and the widest of them.
fn tick_texts(ticks: &[f64], measure: Measure) -> (Vec<String>, f32) {
    let texts: Vec<String> = ticks.iter().map(|&t| format_compact(t)).collect();
    let widest = texts.iter().map(|t| measure(t)).fold(0.0, f32::max);
    (texts, widest)
}

/// How upright bars fit a width, before anything is placed.
struct VerticalFit {
    plot_h: f32,
    tick_values: Vec<f64>,
    tick_texts: Vec<String>,
    gutter: f32,
    /// The bar width the room allows, possibly under [`BAR_MIN`].
    fitted_bar: f32,
    /// Width of the slot each category gets.
    slot_w: f32,
}

fn vertical_fit(data: &ChartData, width: f32, measure: Measure) -> VerticalFit {
    let n = data.labels.len().max(1);
    let series = data.series_count();
    let plot_h = plot_height(width);
    let tick_values = ticks_for(data, plot_h);
    let (tick_texts, widest_tick) = tick_texts(&tick_values, measure);
    let gutter = widest_tick + LABEL_GAP;
    let room = (width - gutter - PAD).max(1.0);
    let fitted_bar = bar_thickness(n, series, bar_cap(series), room);
    let bar = fitted_bar.max(BAR_MIN.min(bar_cap(series)));
    let group = group_width(series, bar);
    let needed = n as f32 * (group + group_gap(group));
    let plot_w = needed.max(MIN_PLOT_WIDTH).min(room);
    VerticalFit {
        plot_h,
        tick_values,
        tick_texts,
        gutter,
        fitted_bar,
        slot_w: plot_w / n as f32,
    }
}

/// Whether `data` reads better on its side at `width`: too many categories
/// or bars, bars thinner than [`BAR_MIN`], or category labels that would be
/// cut under [`MIN_LABEL_CHARS`] characters or [`MIN_LABEL_SHARE`] of their
/// length (GPUI cannot rotate text, so upright labels must fit their slot).
pub fn choose_orientation(data: &ChartData, width: f32, measure: Measure) -> Orientation {
    let n = data.labels.len();
    if n > MAX_UPRIGHT_CATEGORIES || n * data.series_count() > MAX_UPRIGHT_BARS {
        return Orientation::Horizontal;
    }
    let fit = vertical_fit(data, width.max(1.0), measure);
    if fit.fitted_bar < BAR_MIN {
        return Orientation::Horizontal;
    }
    let label_room = (fit.slot_w - 4.0).max(0.0);
    let cut_too_short = data.labels.iter().any(|label| {
        let len = label.chars().count();
        let (_, kept) = truncate_to(label, label_room, measure);
        kept < len && (kept < MIN_LABEL_CHARS || (kept as f32) < len as f32 * MIN_LABEL_SHARE)
    });
    if cut_too_short {
        Orientation::Horizontal
    } else {
        Orientation::Vertical
    }
}

/// Upright bars: groups of bars along x, values up the y axis.
fn vertical_bars(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    let n = data.labels.len().max(1);
    let series = data.series_count();
    let VerticalFit {
        plot_h,
        tick_values,
        tick_texts,
        gutter,
        fitted_bar,
        slot_w,
    } = vertical_fit(data, width, measure);
    let bar = fitted_bar.max(BAR_MIN.min(bar_cap(series)));
    let group = group_width(series, bar);
    let plot_w = slot_w * n as f32;
    let left = ((width - gutter - plot_w) / 2.0).max(0.0) + gutter;

    // Values at the tips when few and each fits over its bar.
    let bar_count = data.labels.len() * series;
    let tip_room = if series > 1 { bar + INNER_GAP } else { slot_w };
    let show_values = bar_count <= MAX_VALUE_LABELS
        && (0..series).all(|s| {
            (0..data.labels.len()).all(|l| measure(&format_compact(data.value(s, l))) <= tip_room)
        });
    let has_negative = data.all_values().any(|v| v < 0.0);
    let top_pad = if show_values {
        LABEL_HEIGHT + LABEL_GAP
    } else {
        LABEL_HEIGHT / 2.0
    };
    let below_pad = if show_values && has_negative {
        LABEL_HEIGHT + LABEL_GAP
    } else {
        0.0
    };
    let plot = Rect::new(left, top_pad, plot_w, plot_h);
    let height = top_pad + plot_h + below_pad + LABEL_GAP + LABEL_HEIGHT + PAD / 2.0;

    let y = scale(&tick_values, plot.bottom(), plot.y);
    let ticks: Vec<Tick> = tick_values
        .iter()
        .map(|&value| Tick {
            value,
            pos: y(value),
        })
        .collect();
    let tick_labels = ticks
        .iter()
        .zip(tick_texts)
        .map(|(tick, text)| Label {
            text,
            x: plot.x - LABEL_GAP,
            y: tick.pos - LABEL_HEIGHT / 2.0,
            align: Align::Right,
        })
        .collect();
    let base = y(0.0);
    let baseline = (base >= plot.y - 0.5 && base <= plot.bottom() + 0.5).then_some(base);
    let base = base.clamp(plot.y, plot.bottom());

    let mut groups = Vec::with_capacity(data.labels.len());
    let mut bars = Vec::with_capacity(bar_count);
    let mut category_labels = Vec::with_capacity(data.labels.len());
    let mut value_labels = Vec::new();
    for (label, text) in data.labels.iter().enumerate() {
        let slot = Rect::new(plot.x + label as f32 * slot_w, plot.y, slot_w, plot_h);
        let gx = slot.x + (slot_w - group) / 2.0;
        groups.push(Group {
            label,
            slot,
            bars: Rect::new(gx, plot.y, group, plot_h),
        });
        let (text, _) = truncate_to(text, (slot_w - 4.0).max(0.0), measure);
        category_labels.push(Label {
            text,
            x: slot.center_x(),
            y: plot.bottom() + below_pad + LABEL_GAP,
            align: Align::Center,
        });
        for s in 0..series {
            let value = data.value(s, label);
            let tip = y(value).clamp(plot.y, plot.bottom());
            let x = gx + s as f32 * (bar + INNER_GAP);
            let negative = value < 0.0;
            let rect = Rect::new(x, tip.min(base), bar, (tip - base).abs());
            if show_values {
                let ly = if negative {
                    rect.bottom() + 2.0
                } else {
                    rect.y - LABEL_HEIGHT - 2.0
                };
                value_labels.push(Label {
                    text: format_compact(value),
                    x: rect.center_x(),
                    y: ly,
                    align: Align::Center,
                });
            }
            bars.push(BarMark {
                label,
                series: s,
                value,
                rect,
                negative,
            });
        }
    }

    ChartLayout {
        width,
        height,
        orientation: Orientation::Vertical,
        plot,
        ticks,
        tick_labels,
        baseline,
        category_labels,
        groups,
        bars,
        value_labels,
        xs: Vec::new(),
        points: Vec::new(),
        markers: false,
    }
}

/// Bars on their side: one row per category, labels on the left, values
/// along the x axis. The height grows with the categories.
fn horizontal_bars(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    let n = data.labels.len();
    let series = data.series_count();
    let bar = if series > 1 {
        H_GROUPED_BAR_MAX
    } else {
        H_BAR_MAX
    };
    let group = group_width(series, bar);
    let gap = (group * 0.5).clamp(H_GROUP_GAP_MIN, H_GROUP_GAP_MAX);
    let row_h = group + gap;

    let widest_label = data.labels.iter().map(|l| measure(l)).fold(0.0, f32::max);
    let margin = (widest_label + LABEL_GAP).min(width * MAX_LABEL_MARGIN);
    let bar_count = n * series;
    let show_values = bar_count <= MAX_VALUE_LABELS;
    let value_width = |negative: bool| {
        if !show_values {
            return 0.0;
        }
        data.all_values()
            .filter(|v| (*v < 0.0) == negative)
            .map(|v| measure(&format_compact(v)) + LABEL_GAP)
            .fold(0.0, f32::max)
    };
    let (left_pad, right_pad) = (value_width(true), value_width(false).max(PAD));
    let plot_x = margin + left_pad;
    let plot_w = (width - plot_x - right_pad).max(1.0);
    let plot = Rect::new(plot_x, PAD / 2.0, plot_w, n as f32 * row_h);
    let height = plot.bottom() + LABEL_GAP + LABEL_HEIGHT + PAD / 2.0;

    let tick_values = {
        let (lo, hi) = value_domain(data.kind, data.all_values());
        let target = (plot_w / 90.0).round().clamp(2.0, 6.0) as usize;
        nice_ticks_n(lo, hi, target)
    };
    let x = scale(&tick_values, plot.x, plot.right());
    let ticks: Vec<Tick> = tick_values
        .iter()
        .map(|&value| Tick {
            value,
            pos: x(value),
        })
        .collect();
    let tick_labels = ticks
        .iter()
        .map(|tick| Label {
            text: format_compact(tick.value),
            x: tick.pos,
            y: plot.bottom() + LABEL_GAP,
            align: Align::Center,
        })
        .collect();
    let base = x(0.0);
    let baseline = (base >= plot.x - 0.5 && base <= plot.right() + 0.5).then_some(base);
    let base = base.clamp(plot.x, plot.right());

    let mut groups = Vec::with_capacity(n);
    let mut bars = Vec::with_capacity(bar_count);
    let mut category_labels = Vec::with_capacity(n);
    let mut value_labels = Vec::new();
    for (label, text) in data.labels.iter().enumerate() {
        let slot = Rect::new(plot.x, plot.y + label as f32 * row_h, plot.w, row_h);
        let gy = slot.y + (row_h - group) / 2.0;
        groups.push(Group {
            label,
            slot,
            bars: Rect::new(plot.x, gy, plot.w, group),
        });
        let (text, _) = truncate_to(text, (margin - LABEL_GAP).max(0.0), measure);
        category_labels.push(Label {
            text,
            x: margin - LABEL_GAP,
            y: slot.center_y() - LABEL_HEIGHT / 2.0,
            align: Align::Right,
        });
        for s in 0..series {
            let value = data.value(s, label);
            let tip = x(value).clamp(plot.x, plot.right());
            let y = gy + s as f32 * (bar + INNER_GAP);
            let negative = value < 0.0;
            let rect = Rect::new(tip.min(base), y, (tip - base).abs(), bar);
            if show_values {
                let (lx, align) = if negative {
                    (rect.x - LABEL_GAP / 2.0, Align::Right)
                } else {
                    (rect.right() + LABEL_GAP / 2.0, Align::Left)
                };
                value_labels.push(Label {
                    text: format_compact(value),
                    x: lx,
                    y: rect.center_y() - LABEL_HEIGHT / 2.0,
                    align,
                });
            }
            bars.push(BarMark {
                label,
                series: s,
                value,
                rect,
                negative,
            });
        }
    }

    ChartLayout {
        width,
        height,
        orientation: Orientation::Horizontal,
        plot,
        ticks,
        tick_labels,
        baseline,
        category_labels,
        groups,
        bars,
        value_labels,
        xs: Vec::new(),
        points: Vec::new(),
        markers: false,
    }
}

/// Lines and areas: one point per category across the plot, values up the
/// y axis, and as many x labels as fit without overlapping.
fn lines(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    let n = data.labels.len();
    let plot_h = plot_height(width);
    let tick_values = ticks_for(data, plot_h);
    let (tick_texts, widest_tick) = tick_texts(&tick_values, measure);
    let gutter = widest_tick + LABEL_GAP;
    let plot = Rect::new(
        gutter,
        LABEL_HEIGHT / 2.0,
        (width - gutter - PAD).max(1.0),
        plot_h,
    );
    let height = plot.bottom() + LABEL_GAP + LABEL_HEIGHT + PAD / 2.0;

    let y = scale(&tick_values, plot.bottom(), plot.y);
    let ticks: Vec<Tick> = tick_values
        .iter()
        .map(|&value| Tick {
            value,
            pos: y(value),
        })
        .collect();
    let tick_labels = ticks
        .iter()
        .zip(tick_texts)
        .map(|(tick, text)| Label {
            text,
            x: plot.x - LABEL_GAP,
            y: tick.pos - LABEL_HEIGHT / 2.0,
            align: Align::Right,
        })
        .collect();
    let base = y(0.0);
    let baseline = (base >= plot.y - 0.5 && base <= plot.bottom() + 0.5).then_some(base);

    // Points keep half a marker from the plot's edges.
    let inset = MARKER_SIZE / 2.0;
    let span = (plot.w - 2.0 * inset).max(0.0);
    let xs: Vec<f32> = (0..n)
        .map(|i| {
            if n > 1 {
                plot.x + inset + i as f32 * span / (n - 1) as f32
            } else {
                plot.center_x()
            }
        })
        .collect();
    let points = (0..data.series_count())
        .map(|s| {
            xs.iter()
                .enumerate()
                .map(|(i, &x)| (x, y(data.value(s, i)).clamp(plot.y, plot.bottom())))
                .collect()
        })
        .collect();
    let category_labels = thin_labels(&data.labels, &xs, plot.bottom() + LABEL_GAP, width, measure);

    ChartLayout {
        width,
        height,
        orientation: Orientation::Vertical,
        plot,
        ticks,
        tick_labels,
        baseline,
        category_labels,
        groups: Vec::new(),
        bars: Vec::new(),
        value_labels: Vec::new(),
        xs,
        points,
        markers: n <= MAX_MARKERS,
    }
}

/// The x-axis labels of a line: each cut to [`MAX_X_LABEL`], kept inside the
/// chart, and only every `k`-th one for the smallest `k` at which no two
/// overlap (with [`X_LABEL_SPACE`] between them).
pub fn thin_labels(
    labels: &[String],
    xs: &[f32],
    top: f32,
    width: f32,
    measure: Measure,
) -> Vec<Label> {
    let texts: Vec<(String, f32)> = labels
        .iter()
        .map(|label| {
            let (text, _) = truncate_to(label, MAX_X_LABEL.min(width), measure);
            let w = measure(&text);
            (text, w)
        })
        .collect();
    // A label centered on its point, moved inward at the chart's edges.
    let place = |ix: usize| -> Option<Label> {
        let (text, w) = texts.get(ix)?;
        let x = xs.get(ix)?.clamp(w / 2.0, (width - w / 2.0).max(w / 2.0));
        Some(Label {
            text: text.clone(),
            x,
            y: top,
            align: Align::Center,
        })
    };
    let n = texts.len().min(xs.len());
    for k in 1..=n.max(1) {
        let chosen: Vec<Label> = (0..n).step_by(k).filter_map(place).collect();
        let fits = chosen.windows(2).all(|pair| {
            let (_, right) = pair[0].span(measure);
            let (left, _) = pair[1].span(measure);
            left - right >= X_LABEL_SPACE
        });
        if fits {
            return chosen;
        }
    }
    Vec::new()
}

/// The category nearest `x` on a line or area chart.
pub fn nearest_index(layout: &ChartLayout, x: f32) -> Option<usize> {
    layout
        .xs
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - x).abs().total_cmp(&(*b - x).abs()))
        .map(|(ix, _)| ix)
}

/// The bar under `(x, y)`: the bar of the hovered category slot nearest the
/// cursor across the category axis.
pub fn hit_bar(layout: &ChartLayout, x: f32, y: f32) -> Option<usize> {
    if !layout.plot.contains(x, y) {
        return None;
    }
    let vertical = layout.orientation == Orientation::Vertical;
    let across = if vertical { x } else { y };
    let group = layout.groups.iter().find(|g| {
        let (start, len) = if vertical {
            (g.slot.x, g.slot.w)
        } else {
            (g.slot.y, g.slot.h)
        };
        across >= start && across <= start + len
    })?;
    layout
        .bars
        .iter()
        .enumerate()
        .filter(|(_, bar)| bar.label == group.label)
        .min_by(|(_, a), (_, b)| {
            let center = |r: &Rect| if vertical { r.center_x() } else { r.center_y() };
            (center(&a.rect) - across)
                .abs()
                .total_cmp(&(center(&b.rect) - across).abs())
        })
        .map(|(ix, _)| ix)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A monospace stand-in for the chart font: 7px per character.
    pub(crate) fn mono(text: &str) -> f32 {
        text.chars().count() as f32 * 7.0
    }

    pub(crate) fn data(kind: ChartKind, labels: &[&str], series: &[&[f64]]) -> ChartData {
        ChartData {
            kind,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            names: (0..series.len()).map(|i| format!("s{i}")).collect(),
            values: series.iter().map(|v| v.to_vec()).collect(),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn few_bars_on_a_wide_chart_stay_narrow_and_centered() {
        let d = data(
            ChartKind::Bar,
            &["a", "b", "c", "d", "e"],
            &[&[5.0, 3.0, 8.0, 1.0, 4.0]],
        );
        let l = plan(&d, 2560.0, &mono);
        assert_eq!(l.orientation, Orientation::Vertical);
        assert!(l.bars.iter().all(|b| b.rect.w <= BAR_MAX));
        assert!(l.plot.w < 600.0, "plot {}", l.plot.w);
        // The value axis and the plot together are centered.
        let block_left = l
            .tick_labels
            .iter()
            .map(|t| t.span(&mono).0)
            .fold(f32::MAX, f32::min);
        assert!(close(block_left, l.width - l.plot.right()));
        for (bar, group) in l.bars.iter().zip(&l.groups) {
            assert!(close(bar.rect.center_x(), group.slot.center_x()));
        }
    }

    #[test]
    fn grouped_bars_touch_within_and_part_between_groups() {
        let values: [&[f64]; 3] = [&[1.0; 5], &[2.0; 5], &[3.0; 5]];
        let d = data(ChartKind::Bar, &["a", "b", "c", "d", "e"], &values);
        let l = plan(&d, 1100.0, &mono);
        assert!(l.bars.iter().all(|b| b.rect.w <= GROUPED_BAR_MAX));
        let first: Vec<&BarMark> = l.bars.iter().filter(|b| b.label == 0).collect();
        assert_eq!(first.len(), 3);
        for pair in first.windows(2) {
            assert!(close(pair[1].rect.x - pair[0].rect.right(), INNER_GAP));
        }
        for pair in l.groups.windows(2) {
            assert!(pair[1].bars.x - pair[0].bars.right() >= GROUP_GAP_MIN - 0.01);
        }
    }

    #[test]
    fn negative_bars_hang_from_a_baseline_inside_the_plot() {
        let d = data(ChartKind::Bar, &["a", "b", "c"], &[&[5.0, -3.0, 2.0]]);
        let l = plan(&d, 800.0, &mono);
        let base = l.baseline.unwrap_or_else(|| panic!("zero in domain"));
        assert!(base > l.plot.y && base < l.plot.bottom());
        let neg = &l.bars[1];
        assert!(neg.negative);
        assert!(close(neg.rect.y, base) && neg.rect.h > 0.0);
        let pos = &l.bars[0];
        assert!(!pos.negative && close(pos.rect.bottom(), base));
    }

    #[test]
    fn height_grows_with_width_within_bounds() {
        let d = data(ChartKind::Bar, &["a", "b", "c"], &[&[5.0, 3.0, 2.0]]);
        let mut last = 0.0;
        for w in (300..3000).step_by(50) {
            let l = plan(&d, w as f32, &mono);
            assert!(l.height >= last, "height shrank at {w}");
            assert!(l.plot.h >= MIN_PLOT_HEIGHT && l.plot.h <= MAX_PLOT_HEIGHT);
            last = l.height;
        }
    }

    #[test]
    fn values_label_few_bars_only() {
        let few = data(ChartKind::Bar, &["a", "b"], &[&[1200.0, 3.0]]);
        let l = plan(&few, 800.0, &mono);
        let texts: Vec<&str> = l.value_labels.iter().map(|v| v.text.as_str()).collect();
        assert_eq!(texts, ["1.2K", "3"]);
        let labels: Vec<String> = (0..13).map(|i| i.to_string()).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let many = data(ChartKind::Bar, &labels, &[&[1.0; 13]]);
        assert!(plan(&many, 1100.0, &mono).value_labels.is_empty());
    }

    #[test]
    fn truncates_with_an_ellipsis() {
        assert_eq!(truncate_to("short", 100.0, &mono), ("short".into(), 5));
        let (text, kept) = truncate_to("a long label", 35.0, &mono);
        assert_eq!(kept, 4);
        assert_eq!(text, "a lo…");
        assert!(mono(&text) <= 35.0);
    }

    #[test]
    fn hovering_a_slot_picks_the_nearest_bar() {
        let values: [&[f64]; 2] = [&[1.0, 2.0], &[3.0, 4.0]];
        let d = data(ChartKind::Bar, &["a", "b"], &values);
        let l = plan(&d, 800.0, &mono);
        let target = &l.bars[3].rect;
        assert_eq!(
            hit_bar(&l, target.center_x(), l.plot.bottom() - 1.0),
            Some(3)
        );
        assert_eq!(hit_bar(&l, 0.0, 0.0), None);
    }

    fn numbered(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    #[test]
    fn many_categories_or_long_labels_lie_on_their_side() {
        let labels = numbered(40);
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let many = data(ChartKind::Bar, &labels, &[&[1.0; 40]]);
        assert_eq!(
            choose_orientation(&many, 1100.0, &mono),
            Orientation::Horizontal
        );
        let long = data(
            ChartKind::Bar,
            &[
                "src/blocks/chart/layout.rs",
                "src/blocks/chart/paint.rs",
                "src/blocks/chart/plot.rs",
                "src/blocks/chart/scale.rs",
                "src/blocks/chart/mod.rs",
                "src/blocks/measured.rs",
                "src/timeline/column.rs",
                "src/timeline/view.rs",
            ],
            &[&[1.0; 8]],
        );
        assert_eq!(
            choose_orientation(&long, 800.0, &mono),
            Orientation::Horizontal
        );
        let short = data(
            ChartKind::Bar,
            &["mon", "tue", "wed", "thu", "fri", "sat"],
            &[&[1.0; 6]],
        );
        assert_eq!(
            choose_orientation(&short, 800.0, &mono),
            Orientation::Vertical
        );
    }

    #[test]
    fn bars_on_their_side_keep_labels_within_a_margin() {
        let labels: Vec<String> = (0..30)
            .map(|i| format!("a rather long category name number {i}"))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let d = data(ChartKind::Bar, &labels, &[&[3.0; 30]]);
        let l = plan(&d, 600.0, &mono);
        assert_eq!(l.orientation, Orientation::Horizontal);
        assert!(l.plot.x <= 600.0 * MAX_LABEL_MARGIN + 1.0);
        assert!(l.category_labels.iter().all(|c| c.text.ends_with('…')));
        assert!(l.category_labels.iter().all(|c| c.span(&mono).0 >= -0.5));
        assert!(l.value_labels.is_empty(), "30 bars: values in the tooltip");
        let fewer = data(ChartKind::Bar, &labels[..20], &[&[3.0; 20]]);
        assert!(plan(&fewer, 600.0, &mono).height < l.height);
        // Every bar grows from the baseline to the right.
        let base = l.baseline.unwrap_or_else(|| panic!("zero in domain"));
        assert!(
            l.bars
                .iter()
                .all(|b| close(b.rect.x, base) && b.rect.w > 0.0)
        );
        assert_eq!(
            hit_bar(&l, l.plot.x + 5.0, l.bars[7].rect.center_y()),
            Some(7)
        );
    }

    #[test]
    fn negative_bars_on_their_side_grow_left() {
        let labels = numbered(20);
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let mut values = vec![2.0; 20];
        values[3] = -4.0;
        let d = data(ChartKind::Bar, &labels, &[&values]);
        let l = plan(&d, 900.0, &mono);
        assert_eq!(l.orientation, Orientation::Horizontal);
        let base = l.baseline.unwrap_or_else(|| panic!("zero in domain"));
        assert!(base > l.plot.x && base < l.plot.right());
        assert!(l.bars[3].negative && close(l.bars[3].rect.right(), base));
    }

    #[test]
    fn line_labels_are_thinned_until_none_overlap() {
        let labels: Vec<String> = (0..100).map(|i| format!("2024-01-{i:02}")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let d = data(ChartKind::Line, &labels, &[&[1.0; 100]]);
        let l = plan(&d, 800.0, &mono);
        assert!(l.category_labels.len() > 1 && l.category_labels.len() < 100);
        for pair in l.category_labels.windows(2) {
            assert!(pair[1].span(&mono).0 - pair[0].span(&mono).1 >= X_LABEL_SPACE);
        }
        assert!(l.category_labels.iter().all(|c| {
            let (a, b) = c.span(&mono);
            a >= -0.01 && b <= l.width + 0.01
        }));
        assert!(!l.markers, "100 points: no markers");
        let few = data(ChartKind::Line, &["a", "b", "c"], &[&[1.0, 2.0, 3.0]]);
        let l = plan(&few, 800.0, &mono);
        assert_eq!(l.category_labels.len(), 3);
        assert!(l.markers);
    }

    #[test]
    fn line_points_follow_the_values_and_the_cursor_finds_them() {
        let values: [&[f64]; 2] = [&[1.0, 5.0, 3.0], &[2.0, 2.0, 2.0]];
        let d = data(ChartKind::Line, &["a", "b", "c"], &values);
        let l = plan(&d, 900.0, &mono);
        assert_eq!(l.points.len(), 2);
        assert!(
            l.points[0][1].1 < l.points[0][0].1,
            "higher value, higher up"
        );
        assert!(l.bars.is_empty());
        assert_eq!(nearest_index(&l, l.xs[1] + 3.0), Some(1));
        assert_eq!(nearest_index(&l, -50.0), Some(0));
        let area = data(ChartKind::Area, &["a", "b"], &[&[5.0, 9.0]]);
        let l = plan(&area, 900.0, &mono);
        assert_eq!(l.ticks.first().map(|t| t.value), Some(0.0));
    }
}
