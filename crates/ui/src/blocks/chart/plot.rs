//! The chart element: a [`Plot`] over a [`ChartLayout`], hosted by
//! gpui-component's [`PlotElement`] for the hitbox, hover tracking and
//! tooltip overlay, and wrapped in [`measured`] so its height follows the
//! width ([`chart_frame`]).
//!
//! Bars show one tooltip per bar ("category", then "series: value") over a
//! soft band on the hovered group. Lines and areas show a vertical
//! crosshair at the nearest category, a dot on each series and one row per
//! series.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ElementId, Hsla, IntoElement, Pixels, Point, Window, point, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::plot::tooltip::{CrossLine, Dot, Tooltip};
use gpui_component::plot::{Plot, PlotElement, TooltipState};

use domain::block::ChartKind;

use super::layout::{
    ChartData, ChartLayout, Orientation, height_for, hit_bar, nearest_index, plan,
};
use super::paint::{LabelFont, paint_bars, paint_grid, paint_labels, paint_lines};
use super::scale::format_full;
use super::series_color;
use crate::blocks::measured::measured;

/// Extra width of the hover band on each side of a bar group.
const BAND_PAD: f32 = 4.0;
/// Diameter of the dot marking a hovered point.
const HOVER_DOT: f32 = 8.0;
/// Diameter of the faint ring around a hovered point's dot.
const HOVER_HALO: f32 = 16.0;

/// A bar, line or area chart drawn from its own layout.
pub struct ChartPlot {
    id: ElementId,
    data: Rc<ChartData>,
    colors: Rc<[Hsla]>,
    font: LabelFont,
    /// The layout for this frame's bounds, set in prepaint.
    layout: Option<ChartLayout>,
}

impl ChartPlot {
    fn color(&self, series: usize) -> Hsla {
        self.colors
            .get(series % self.colors.len().max(1))
            .copied()
            .unwrap_or_default()
    }

    fn is_line(&self) -> bool {
        matches!(self.data.kind, ChartKind::Line | ChartKind::Area)
    }

    fn name(&self, series: usize) -> String {
        match self.data.names.get(series) {
            Some(name) if !name.trim().is_empty() => name.clone(),
            _ => "value".to_owned(),
        }
    }
}

impl IntoElement for ChartPlot {
    type Element = PlotElement<Self>;

    fn into_element(self) -> Self::Element {
        PlotElement::new(self)
    }
}

impl Plot for ChartPlot {
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Vec<AnyElement> {
        let window: &Window = window;
        let measure = |text: &str| self.font.measure(text, window);
        self.layout = Some(plan(&self.data, bounds.size.width.as_f32(), &measure));
        vec![]
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let theme = cx.theme();
        let (grid, base) = (theme.chart_grid, theme.border);
        let (muted, strong) = (theme.muted_foreground, theme.foreground);
        let surface = theme.background;
        let origin = bounds.origin;
        paint_grid(layout, origin, grid, base, window);
        let color = |s| self.color(s);
        match self.data.kind {
            ChartKind::Line => paint_lines(layout, origin, color, false, surface, window),
            ChartKind::Area => paint_lines(layout, origin, color, true, surface, window),
            ChartKind::Bar | ChartKind::Pie => paint_bars(layout, origin, color, window),
        }
        paint_labels(layout, origin, &self.font, muted, strong, window, cx);
    }

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn tooltip_state(
        &self,
        position: Point<Pixels>,
        _bounds: Bounds<Pixels>,
        _cx: &App,
    ) -> Option<TooltipState> {
        let layout = self.layout.as_ref()?;
        let (x, y) = (position.x.as_f32(), position.y.as_f32());
        if self.is_line() {
            let plot = &layout.plot;
            if y < plot.y || y > plot.bottom() || x < plot.x - HOVER_DOT || x > plot.right() {
                return None;
            }
            let index = nearest_index(layout, x)?;
            let px_x = *layout.xs.get(index)?;
            let dots = layout
                .points
                .iter()
                .filter_map(|series| series.get(index))
                .map(|&(x, y)| point(px(x), px(y)))
                .collect();
            return Some(TooltipState::new(index, point(px(px_x), position.y), dots));
        }
        let index = hit_bar(layout, x, y)?;
        let group = layout.groups.get(layout.bars.get(index)?.label)?;
        let cross = match layout.orientation {
            Orientation::Vertical => point(px(group.bars.center_x()), position.y),
            Orientation::Horizontal => point(position.x, px(group.bars.center_y())),
        };
        Some(TooltipState::new(index, cross, vec![]))
    }

    fn tooltip(
        &self,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let layout = self.layout.as_ref()?;
        if self.is_line() {
            return Some(self.line_tooltip(layout, state, cursor, bounds, cx));
        }
        let bar = layout.bars.get(state.index)?;
        let group = layout.groups.get(bar.label)?;
        let plot = &layout.plot;
        let band = match layout.orientation {
            Orientation::Vertical => CrossLine::new(state.cross_line)
                .band(px(group.bars.w + 2.0 * BAND_PAD))
                .span(plot.y, plot.h),
            Orientation::Horizontal => CrossLine::new(state.cross_line)
                .horizontal()
                .band(px(group.bars.h + 2.0 * BAND_PAD))
                .h_span(plot.x, plot.w),
        };
        let title = self.data.labels.get(bar.label).cloned().unwrap_or_default();
        let tooltip = Tooltip::new(cursor, bounds.size)
            .gap(px(8.))
            .cross_line(band)
            .title(title)
            .row(
                self.color(bar.series),
                self.name(bar.series),
                format_full(bar.value),
            );
        Some(tooltip.into_any_element())
    }
}

impl ChartPlot {
    /// The crosshair, the dots and one row per series at a category.
    fn line_tooltip(
        &self,
        layout: &ChartLayout,
        state: &TooltipState,
        cursor: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> AnyElement {
        let surface = cx.theme().background;
        let plot = &layout.plot;
        let title = self
            .data
            .labels
            .get(state.index)
            .cloned()
            .unwrap_or_default();
        let mut tooltip = Tooltip::new(cursor, bounds.size)
            .gap(px(8.))
            .cross_line(CrossLine::new(state.cross_line).span(plot.y, plot.h))
            .dots(state.dots.iter().enumerate().map(|(series, &at)| {
                Dot::new(at)
                    .size(px(HOVER_DOT))
                    .halo(px(HOVER_HALO))
                    .stroke(surface)
                    .fill(self.color(series))
            }))
            .title(title);
        for series in 0..self.data.values.len() {
            tooltip = tooltip.row(
                self.color(series),
                self.name(series),
                format_full(self.data.value(series, state.index)),
            );
        }
        tooltip.into_any_element()
    }
}

/// The chart element for `data`: as wide as its container and as tall as
/// its layout needs at that width.
pub fn chart_frame(data: ChartData, id: ElementId, window: &Window, cx: &App) -> AnyElement {
    let font = LabelFont {
        font: window.text_style().font(),
        size: window.rem_size() * 0.75,
    };
    let data = Rc::new(data);
    let colors: Rc<[Hsla]> = (0..data.series_count())
        .map(|ix| series_color(ix, cx))
        .collect();
    let plot = ChartPlot {
        id,
        data: data.clone(),
        colors,
        font: font.clone(),
        layout: None,
    };
    measured(
        move |width, window, _| {
            let window: &Window = window;
            let measure = |text: &str| font.measure(text, window);
            px(height_for(&data, width.as_f32(), &measure))
        },
        plot,
    )
    .into_any_element()
}
