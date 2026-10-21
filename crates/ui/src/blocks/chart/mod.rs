//! Renderer of the `chart` block kind: bar, line, area and pie charts from
//! gpui-component, colored with the theme's chart palette, with a title and
//! a legend. `t` ([`TimelineEntry::alt_view`]) shows the same data as a
//! table ([`ChartSpec::to_table`] through the table block's renderer).
//!
//! gpui-component's `BarChart` and `LineChart` draw one series, so several
//! series become grouped bars (one band per label and series, the label
//! under the middle bar) and lines are an `AreaChart` with transparent
//! fills, which overlays any number of series on one axis.

pub mod layout;
pub mod scale;

use std::rc::Rc;

use domain::block::{BlockKind, ChartKind, ChartSpec};
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, Window, div, px, transparent_black,
};
use gpui_component::chart::{AreaChart, BarChart, PieChart};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// Height of the plot area.
const CHART_HEIGHT: f32 = 240.0;

/// Theme color of series (or pie slice) `ix`, cycling the five chart colors.
pub fn series_color(ix: usize, cx: &App) -> Hsla {
    let theme = cx.theme();
    let palette = [
        theme.chart_1,
        theme.chart_2,
        theme.chart_3,
        theme.chart_4,
        theme.chart_5,
    ];
    palette[ix % palette.len()]
}

/// Name of a chart kind, for the header.
pub fn kind_label(kind: ChartKind) -> &'static str {
    match kind {
        ChartKind::Bar => "bar chart",
        ChartKind::Line => "line chart",
        ChartKind::Area => "area chart",
        ChartKind::Pie => "pie chart",
    }
}

/// Legend entries: series names, or the slice labels of a pie.
pub fn legend_names(spec: &ChartSpec) -> Vec<String> {
    match spec.kind {
        ChartKind::Pie => spec.labels.clone(),
        _ => spec.series.iter().map(|s| s.name.clone()).collect(),
    }
}

/// A bar's band on the category axis: unique per bar, labelled only under
/// the middle bar of its group so each label shows once.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Band {
    ix: usize,
    label: SharedString,
}

impl From<Band> for SharedString {
    fn from(band: Band) -> Self {
        band.label
    }
}

/// One bar: category `label` of series `series`.
#[derive(Clone, Copy)]
struct Bar {
    label: usize,
    series: usize,
}

/// The chart's data, shared by the accessor closures.
struct Data {
    labels: Vec<SharedString>,
    names: Vec<SharedString>,
    values: Vec<Vec<f64>>,
}

impl Data {
    fn new(spec: &ChartSpec) -> Rc<Self> {
        Rc::new(Self {
            labels: spec
                .labels
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect(),
            names: spec
                .series
                .iter()
                .map(|s| SharedString::from(s.name.clone()))
                .collect(),
            values: spec.series.iter().map(|s| s.values.clone()).collect(),
        })
    }

    fn value(&self, series: usize, label: usize) -> f64 {
        self.values
            .get(series)
            .and_then(|v| v.get(label))
            .copied()
            .unwrap_or_default()
    }

    fn label(&self, ix: usize) -> SharedString {
        self.labels.get(ix).cloned().unwrap_or_default()
    }
}

fn bar_chart(data: Rc<Data>, id: ElementId, cx: &App) -> AnyElement {
    let groups = data.values.len().max(1);
    let colors: Vec<Hsla> = (0..groups).map(|ix| series_color(ix, cx)).collect();
    let bars = (0..data.labels.len())
        .flat_map(|label| (0..groups).map(move |series| Bar { label, series }));
    let (band_data, value_data, tooltip_data) = (data.clone(), data.clone(), data);
    BarChart::new(bars)
        .id(id)
        .appear(false)
        .value_axis(true)
        .padding_inner(if groups > 1 { 0.1 } else { 0.4 })
        .band(move |bar: &Bar| Band {
            ix: bar.label * groups + bar.series,
            label: if bar.series == groups / 2 {
                band_data.label(bar.label)
            } else {
                SharedString::default()
            },
        })
        .value(move |bar| value_data.value(bar.series, bar.label))
        .fill(move |bar, _, _, _| colors[bar.series % colors.len()])
        .tooltip_title(move |bar| {
            let label = tooltip_data.label(bar.label);
            match tooltip_data.names.get(bar.series) {
                Some(name) if groups > 1 => format!("{label} · {name}").into(),
                _ => label,
            }
        })
        .into_any_element()
}

fn area_chart(data: Rc<Data>, filled: bool, id: ElementId, cx: &App) -> AnyElement {
    let x_data = data.clone();
    let mut chart = AreaChart::new(0..data.labels.len())
        .id(id)
        .appear(false)
        .y_axis(true)
        .x(move |&ix| x_data.label(ix));
    for series in 0..data.values.len() {
        let color = series_color(series, cx);
        let y_data = data.clone();
        let fill = if filled {
            color.opacity(0.25)
        } else {
            transparent_black()
        };
        chart = chart
            .y(move |&ix| y_data.value(series, ix))
            .stroke(color)
            .fill(fill)
            .linear()
            .name(data.names[series].clone());
    }
    chart.into_any_element()
}

fn pie_chart(data: Rc<Data>, id: ElementId, cx: &App) -> AnyElement {
    let colors: Vec<Hsla> = (0..data.labels.len())
        .map(|ix| series_color(ix, cx))
        .collect();
    let (value_data, name_data) = (data.clone(), data.clone());
    PieChart::new(0..data.labels.len())
        .id(id)
        .appear(false)
        .value(move |&ix| value_data.value(0, ix).max(0.0) as f32)
        .color(move |&ix| colors[ix % colors.len().max(1)])
        .tooltip_name(move |&ix| name_data.label(ix))
        .into_any_element()
}

fn legend(spec: &ChartSpec, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .flex_wrap()
        .gap_x_3()
        .gap_y_1()
        .text_xs()
        .text_color(theme.muted_foreground)
        .children(
            legend_names(spec)
                .into_iter()
                .enumerate()
                .map(|(ix, name)| {
                    h_flex()
                        .gap_1()
                        .child(div().size(px(10.)).rounded_sm().bg(series_color(ix, cx)))
                        .child(SharedString::from(name))
                }),
        )
        .into_any_element()
}

fn chart_body(spec: &ChartSpec, id: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    if let Err(err) = spec.validate() {
        return div()
            .text_color(theme.danger)
            .child(format!("Invalid chart: {err}. Press t to see the data."))
            .into_any_element();
    }
    if spec.labels.is_empty() {
        return div()
            .text_color(theme.muted_foreground)
            .child("No data to plot.")
            .into_any_element();
    }
    let data = Data::new(spec);
    let id = ElementId::Name(SharedString::from(format!("chart-{id}")));
    let chart = match spec.kind {
        ChartKind::Bar => bar_chart(data, id, cx),
        ChartKind::Line => area_chart(data, false, id, cx),
        ChartKind::Area => area_chart(data, true, id, cx),
        ChartKind::Pie => pie_chart(data, id, cx),
    };
    v_flex()
        .gap_2()
        .child(div().w_full().h(px(CHART_HEIGHT)).child(chart))
        .child(legend(spec, cx))
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Chart(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let id = entry.block.id;
    let body = if entry.alt_view {
        super::table::render_spec(id, &spec.to_table(), window, cx)
    } else {
        chart_body(spec, &id.to_string(), cx)
    };
    let theme = cx.theme();
    let hint = if entry.alt_view {
        "table · t: chart"
    } else {
        "t: table"
    };
    let title = if spec.title.trim().is_empty() {
        "Chart"
    } else {
        spec.title.as_str()
    };
    v_flex()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(title.to_owned())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{} · {hint}", kind_label(spec.kind))),
                ),
        )
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::keymap::Action;
    use application::ports::events::DomainEvent;
    use domain::block::{Block, Series};
    use gpui::{TestAppContext, point, size};

    use crate::timeline::TimelineView;

    fn chart(kind: ChartKind, series: usize) -> ChartSpec {
        ChartSpec {
            title: format!("{kind:?}"),
            kind,
            labels: ["rs", "toml", "md", "lua"].map(String::from).to_vec(),
            series: (0..series)
                .map(|s| Series {
                    name: format!("s{s}"),
                    values: vec![4.0 + s as f64, 1.0, 2.5, 0.5],
                })
                .collect(),
        }
    }

    #[test]
    fn legend_lists_series_or_pie_slices() {
        assert_eq!(legend_names(&chart(ChartKind::Line, 2)), ["s0", "s1"]);
        assert_eq!(
            legend_names(&chart(ChartKind::Pie, 1)),
            ["rs", "toml", "md", "lua"]
        );
    }

    #[gpui::test]
    fn palette_cycles_theme_chart_colors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            assert_eq!(series_color(0, cx), cx.theme().chart_1);
            assert_eq!(series_color(5, cx), cx.theme().chart_1);
            assert_ne!(series_color(1, cx), series_color(0, cx));
        });
    }

    #[gpui::test]
    fn draws_every_kind_and_its_table(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let mut invalid = chart(ChartKind::Pie, 2);
        invalid.title = "invalid".into();
        let mut empty = chart(ChartKind::Bar, 1);
        empty.labels.clear();
        empty.series[0].values.clear();
        let specs = [
            chart(ChartKind::Bar, 1),
            chart(ChartKind::Bar, 3),
            chart(ChartKind::Line, 2),
            chart(ChartKind::Area, 2),
            chart(ChartKind::Pie, 1),
            invalid,
            empty,
        ];
        let count = specs.len();
        view.update(cx, |view, cx| {
            for spec in specs {
                let block = Block::new(BlockKind::Chart(spec));
                view.apply(&DomainEvent::BlockAdded { block }, cx);
            }
        });
        for alt_view in [false, true] {
            for ix in 0..count {
                view.update(cx, |view, cx| {
                    view.handle_action(&Action::builtin("block.first"), cx);
                    for _ in 0..ix {
                        view.handle_action(&Action::builtin("block.next"), cx);
                    }
                    if alt_view {
                        view.handle_action(&Action::builtin("block.toggle_view"), cx);
                    }
                });
                cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
                    div().size_full().child(view.clone())
                });
                view.read_with(cx, |view, _| {
                    assert_eq!(view.state().entries()[ix].alt_view, alt_view);
                });
            }
        }
    }
}
