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
}

/// The layout of `data` at `width`.
pub fn plan(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    vertical_bars(data, width.max(1.0), measure)
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

/// Upright bars: groups of bars along x, values up the y axis.
fn vertical_bars(data: &ChartData, width: f32, measure: Measure) -> ChartLayout {
    let n = data.labels.len().max(1);
    let series = data.series_count();
    let plot_h = plot_height(width);
    let tick_values = ticks_for(data, plot_h);
    let (tick_texts, widest_tick) = tick_texts(&tick_values, measure);
    let gutter = widest_tick + LABEL_GAP;
    let room = (width - gutter - PAD).max(1.0);

    let cap = bar_cap(series);
    let bar = bar_thickness(n, series, cap, room).max(BAR_MIN.min(cap));
    let group = group_width(series, bar);
    let needed = n as f32 * (group + group_gap(group));
    let plot_w = needed.max(MIN_PLOT_WIDTH).min(room);
    let left = ((width - gutter - plot_w) / 2.0).max(0.0) + gutter;
    let slot_w = plot_w / n as f32;

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
    }
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
}
