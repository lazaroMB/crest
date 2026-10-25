//! Renderer of the `plot` block kind: functions and xy series on numeric
//! axes, with a slider per parameter.
//!
//! [`sample`] turns the spec into geometry for the current parameter
//! values (adaptive curves broken at discontinuities, marching squares for
//! implicit curves, a view fitted robustly), [`axes`] lays out ticks and
//! the plot area for the width, and [`paint`] draws it with GPUI paths.
//! The canvas is a gpui-component [`Plot`] in a [`measured`] frame, so its
//! height follows the width, and hovering shows a crosshair with `x`, the
//! value of every explicit function there and the nearest data point.
//!
//! The block's [`PlotView`] is kept per block ([`super::block_state`]): it
//! holds the slider states and the scene, re-sampled only when a slider
//! moves. [`static_plot`] draws a spec without sliders, for the
//! statistics cards (residuals vs fitted, QQ).

pub mod axes;
pub mod paint;
pub mod sample;

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::plot::{CompiledFunction, PlotSpec};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, ElementId, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    Styled as _, Subscription, Window, div, point, prelude::FluentBuilder as _, px,
};
use gpui_component::plot::tooltip::{CrossLine, Dot, Tooltip};
use gpui_component::plot::{Plot, PlotElement, TooltipState};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use self::axes::{PlotLayout, axis_readout, plan, step_decimals};
use self::paint::{paint_grid, paint_labels, paint_layers};
use self::sample::{Mark, Scene, scene};
use super::chart::paint::LabelFont;
use super::chart::series_color;
use super::measured::measured;
use crate::timeline::TimelineEntry;

/// Pixels from the cursor within which a data point is read out.
const HOVER_RADIUS: f32 = 12.0;
/// Diameter of the dot marking a hovered point.
const HOVER_DOT: f32 = 8.0;
/// Diameter of the faint ring around a hovered point's dot.
const HOVER_HALO: f32 = 16.0;
/// Slider positions when a parameter has no step.
const SLIDER_STEPS: f64 = 200.0;

/// A data point near the cursor: its layer and coordinates.
type NearPoint = (usize, (f64, f64));

/// The axis titles of a plot.
#[derive(Clone, Default)]
struct Titles {
    x: Option<String>,
    y: Option<String>,
}

/// A scene drawn on its own axes.
pub struct PlotCanvas {
    id: ElementId,
    scene: Rc<Scene>,
    titles: Titles,
    colors: Rc<[Hsla]>,
    font: LabelFont,
    /// The layout for this frame's bounds, set in prepaint.
    layout: Option<PlotLayout>,
}

impl PlotCanvas {
    fn color(&self, ix: usize) -> Hsla {
        self.colors
            .get(ix % self.colors.len().max(1))
            .copied()
            .unwrap_or_default()
    }

    /// The explicit functions' values at `x` (axis units in and out) with
    /// their layer, and the data point nearest to `cursor` within
    /// [`HOVER_RADIUS`].
    fn readings(
        &self,
        layout: &PlotLayout,
        x: f64,
        cursor: (f32, f32),
    ) -> (Vec<(usize, f64)>, Option<NearPoint>) {
        let scene = &self.scene;
        let data_x = scene.axes.x.inverse(x);
        let values = scene
            .explicit
            .iter()
            .map(|(layer, f): &(usize, CompiledFunction)| {
                (*layer, scene.axes.y.forward(scene.eval_explicit(f, data_x)))
            })
            .collect();
        let mut nearest: Option<(f32, usize, (f64, f64))> = None;
        for (ix, layer) in self.scene.layers.iter().enumerate() {
            let Mark::Points(points) = &layer.mark else {
                continue;
            };
            for &p in points {
                let (px_x, px_y) = layout.to_px(p);
                let d = (px_x - cursor.0).hypot(px_y - cursor.1);
                if d <= HOVER_RADIUS && nearest.is_none_or(|(best, _, _)| d < best) {
                    nearest = Some((d, ix, p));
                }
            }
        }
        (values, nearest.map(|(_, ix, p)| (ix, p)))
    }

    /// The hover dots for `readings` at `x` (drawn at `cursor_x`), each in
    /// its layer's color: one per explicit function in view, then the
    /// nearest data point.
    fn hover_dots(
        &self,
        layout: &PlotLayout,
        x: f64,
        cursor_x: Pixels,
        values: &[(usize, f64)],
        nearest: Option<NearPoint>,
    ) -> Vec<(Point<Pixels>, Hsla)> {
        let view = layout.view;
        let layer_color = |ix: usize| self.color(self.scene.layers.get(ix).map_or(ix, |l| l.color));
        let mut dots: Vec<(Point<Pixels>, Hsla)> = values
            .iter()
            .filter(|(_, y)| y.is_finite() && *y >= view.y.0 && *y <= view.y.1)
            .map(|&(ix, y)| {
                let (_, py) = layout.to_px((x, y));
                (point(cursor_x, px(py)), layer_color(ix))
            })
            .collect();
        if let Some((ix, p)) = nearest {
            let (px_x, px_y) = layout.to_px(p);
            dots.push((point(px(px_x), px(px_y)), layer_color(ix)));
        }
        dots
    }
}

impl IntoElement for PlotCanvas {
    type Element = PlotElement<Self>;

    fn into_element(self) -> Self::Element {
        PlotElement::new(self)
    }
}

impl Plot for PlotCanvas {
    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Vec<AnyElement> {
        let window: &Window = window;
        let measure = |text: &str| self.font.measure(text, window);
        self.layout = Some(plan(
            self.scene.view,
            self.titles.x.as_deref(),
            self.titles.y.as_deref(),
            bounds.size.width.as_f32(),
            &measure,
            self.scene.axes,
        ));
        vec![]
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let Some(layout) = self.layout.as_ref() else {
            return;
        };
        let theme = cx.theme();
        let (grid, axis) = (theme.chart_grid, theme.border);
        let (muted, surface) = (theme.muted_foreground, theme.background);
        let origin = bounds.origin;
        paint_grid(layout, origin, grid, axis, window);
        paint_layers(
            &self.scene,
            layout,
            origin,
            |ix| self.color(ix),
            surface,
            window,
        );
        paint_labels(layout, origin, &self.font, muted, window, cx);
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
        let (mx, my) = (position.x.as_f32(), position.y.as_f32());
        if !layout.plot.contains(mx, my) {
            return None;
        }
        let (x, _) = layout.to_data(mx, my);
        let (values, nearest) = self.readings(layout, x, (mx, my));
        let dots = self
            .hover_dots(layout, x, position.x, &values, nearest)
            .into_iter()
            .map(|(at, _)| at)
            .collect();
        Some(TooltipState::new(
            mx.max(0.0) as usize,
            point(position.x, position.y),
            dots,
        ))
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
        let surface = cx.theme().background;
        let (px_x, px_y) = (state.cross_line.x.as_f32(), state.cross_line.y.as_f32());
        let (x, y) = layout.to_data(px_x, px_y);
        let (values, nearest) = self.readings(layout, x, (px_x, px_y));
        let plot = &layout.plot;
        let dots = self.hover_dots(layout, x, state.cross_line.x, &values, nearest);
        let axes = layout.axes;
        let (x_text, y_text) = (
            |v: f64| axis_readout(v, axes.x, layout.x_step),
            |v: f64| axis_readout(v, axes.y, layout.y_step),
        );
        let mut tooltip = Tooltip::new(cursor, bounds.size)
            .gap(px(8.))
            .cross_line(CrossLine::new(state.cross_line).span(plot.y, plot.h))
            .dots(dots.into_iter().map(|(at, color)| {
                Dot::new(at)
                    .size(px(HOVER_DOT))
                    .halo(px(HOVER_HALO))
                    .stroke(surface)
                    .fill(color)
            }))
            .title(format!("x = {}", x_text(x)));
        let label = |ix: usize| {
            self.scene
                .layers
                .get(ix)
                .map(|l| l.label.clone())
                .unwrap_or_default()
        };
        for (ix, value) in &values {
            let color = self.scene.layers.get(*ix).map_or(0, |l| l.color);
            tooltip = tooltip.row(self.color(color), label(*ix), y_text(*value));
        }
        if let Some((ix, (px_, py_))) = nearest {
            let color = self.scene.layers.get(ix).map_or(0, |l| l.color);
            tooltip = tooltip.row(
                self.color(color),
                label(ix),
                format!("({}, {})", x_text(px_), y_text(py_)),
            );
        }
        if values.is_empty() && nearest.is_none() {
            tooltip = tooltip.plain_row("y", y_text(y));
        }
        Some(tooltip.into_any_element())
    }
}

/// The canvas for `scene`: as wide as its container and as tall as its
/// layout needs at that width.
fn plot_frame(
    scene: Rc<Scene>,
    titles: Titles,
    id: ElementId,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let font = LabelFont {
        font: window.text_style().font(),
        size: window.rem_size() * 0.75,
    };
    let colors: Rc<[Hsla]> = (0..scene.layers.len().max(1))
        .map(|ix| series_color(ix, cx))
        .collect();
    let (view, axes) = (scene.view, scene.axes);
    let canvas = PlotCanvas {
        id,
        scene,
        titles: titles.clone(),
        colors,
        font: font.clone(),
        layout: None,
    };
    measured(
        move |width, window, _| {
            let window: &Window = window;
            let measure = |text: &str| font.measure(text, window);
            let layout = plan(
                view,
                titles.x.as_deref(),
                titles.y.as_deref(),
                width.as_f32(),
                &measure,
                axes,
            );
            px(layout.height)
        },
        canvas,
    )
    .into_any_element()
}

/// The legend: one swatch per layer, for two or more layers.
fn legend(scene: &Scene, cx: &App) -> Option<AnyElement> {
    if scene.layers.len() < 2 {
        return None;
    }
    let theme = cx.theme();
    Some(
        h_flex()
            .flex_wrap()
            .gap_x_3()
            .gap_y_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .children(scene.layers.iter().map(|layer| {
                h_flex()
                    .gap_1()
                    .child(
                        div()
                            .size(px(10.))
                            .rounded_sm()
                            .bg(series_color(layer.color, cx)),
                    )
                    .child(SharedString::from(layer.label.clone()))
            }))
            .into_any_element(),
    )
}

/// The plot of `spec` with its current values, its legend under it.
fn plot_with_legend(
    scene: Rc<Scene>,
    spec: &PlotSpec,
    id: &str,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let selector = format!("plot-canvas-{id}");
    let titles = Titles {
        x: spec.x_label.clone(),
        y: spec.y_label.clone(),
    };
    let legend = legend(&scene, cx);
    let note = skipped_note(scene.skipped).map(|text| {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(text))
    });
    let frame = plot_frame(
        scene,
        titles,
        ElementId::Name(SharedString::from(format!("plot-{id}"))),
        window,
        cx,
    );
    v_flex()
        .gap_2()
        .child(div().w_full().debug_selector(move || selector).child(frame))
        .children(legend)
        .children(note)
        .into_any_element()
}

/// The note under a plot whose log axes left out `skipped` points.
pub fn skipped_note(skipped: usize) -> Option<String> {
    match skipped {
        0 => None,
        1 => Some("1 point not shown: it is not positive on a log axis.".to_owned()),
        n => Some(format!(
            "{n} points not shown: they are not positive on a log axis."
        )),
    }
}

/// `spec` drawn once without sliders (with its parameters' values), or
/// the reason it cannot be drawn.
pub fn static_plot(spec: &PlotSpec, id: &str, window: &Window, cx: &App) -> AnyElement {
    if let Err(err) = spec.validate() {
        return div()
            .text_sm()
            .text_color(cx.theme().danger)
            .child(SharedString::from(format!("Invalid plot: {err}")))
            .into_any_element();
    }
    let compiled = spec.compile().unwrap_or_default();
    let scene = Rc::new(scene(spec, &compiled, &spec.param_values()));
    plot_with_legend(scene, spec, id, window, cx)
}

/// The value of a parameter as its slider writes it.
fn param_text(value: f64, step: f64) -> String {
    format!("{value:.*}", step_decimals(step).min(6))
}

/// The state of a plot block: its spec, slider states and current scene.
pub struct PlotView {
    id: BlockId,
    spec: PlotSpec,
    compiled: Result<Vec<CompiledFunction>, String>,
    values: Vec<f64>,
    sliders: Vec<Entity<SliderState>>,
    scene: Option<Rc<Scene>>,
    _subscriptions: Vec<Subscription>,
}

impl PlotView {
    fn new(id: BlockId, spec: PlotSpec, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            id,
            spec: spec.clone(),
            compiled: Ok(Vec::new()),
            values: Vec::new(),
            sliders: Vec::new(),
            scene: None,
            _subscriptions: Vec::new(),
        };
        view.reset(spec, cx);
        view
    }

    /// Slider step of parameter `ix`.
    fn step(&self, ix: usize) -> f64 {
        self.spec
            .params
            .get(ix)
            .map_or(1.0, |p| p.step.unwrap_or((p.max - p.min) / SLIDER_STEPS))
    }

    /// Starts over from `spec`: new sliders at its values.
    fn reset(&mut self, spec: PlotSpec, cx: &mut Context<Self>) {
        self.compiled = spec
            .validate()
            .and_then(|()| spec.compile())
            .map_err(|err| err.to_string());
        self.values = spec.param_values();
        self.spec = spec;
        self._subscriptions.clear();
        self.sliders = (0..self.spec.params.len())
            .map(|ix| {
                let p = &self.spec.params[ix];
                let step = self.step(ix);
                let state = cx.new(|_| {
                    SliderState::new()
                        .min(p.min as f32)
                        .max(p.max as f32)
                        .step(step as f32)
                        .default_value(p.value as f32)
                });
                self._subscriptions.push(cx.subscribe(
                    &state,
                    move |this: &mut Self, _, event: &SliderEvent, cx| {
                        if let SliderEvent::Change(value) = event {
                            this.set_param(ix, f64::from(value.start()), cx);
                        }
                    },
                ));
                state
            })
            .collect();
        self.rebuild();
    }

    /// Re-samples the scene for the current values.
    fn rebuild(&mut self) {
        self.scene = self
            .compiled
            .as_ref()
            .ok()
            .map(|compiled| Rc::new(scene(&self.spec, compiled, &self.values)));
    }

    /// Sets parameter `ix` (snapped to its step and range) and re-plots.
    pub fn set_param(&mut self, ix: usize, value: f64, cx: &mut Context<Self>) {
        let Some(p) = self.spec.params.get(ix) else {
            return;
        };
        let step = self.step(ix);
        let snapped = (p.min + ((value - p.min) / step).round() * step).clamp(p.min, p.max);
        if self.values.get(ix) == Some(&snapped) {
            return;
        }
        if let Some(slot) = self.values.get_mut(ix) {
            *slot = snapped;
        }
        self.rebuild();
        cx.notify();
    }

    /// The current parameter values.
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// The current scene, if the spec is valid.
    pub fn scene(&self) -> Option<Rc<Scene>> {
        self.scene.clone()
    }

    /// Follows a changed spec (a block update), keeping nothing.
    fn sync(&mut self, spec: &PlotSpec, cx: &mut Context<Self>) {
        if &self.spec != spec {
            self.reset(spec.clone(), cx);
            cx.notify();
        }
    }

    fn render_sliders(&self, cx: &App) -> Option<AnyElement> {
        if self.sliders.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let rows = self.spec.params.iter().enumerate().map(|(ix, p)| {
            let value = self.values.get(ix).copied().unwrap_or(p.value);
            let id = self.id;
            h_flex()
                .gap_3()
                .text_sm()
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_family(theme.mono_font_family.clone())
                        .child(SharedString::from(p.name.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(80.))
                        .debug_selector(move || format!("plot-slider-{id}-{ix}"))
                        .child(Slider::new(&self.sliders[ix])),
                )
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_right()
                        .font_family(theme.mono_font_family.clone())
                        .child(SharedString::from(param_text(value, self.step(ix)))),
                )
        });
        Some(v_flex().gap_1().children(rows).into_any_element())
    }
}

impl Render for PlotView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, danger) = (theme.border, theme.muted_foreground, theme.danger);
        let radius = theme.radius;
        let title = self.spec.title.clone().unwrap_or_else(|| "Plot".to_owned());
        let id = self.id.to_string();
        let body = match (&self.compiled, &self.scene) {
            (Ok(_), Some(scene)) => plot_with_legend(scene.clone(), &self.spec, &id, window, cx),
            (Err(err), _) => div()
                .text_sm()
                .text_color(danger)
                .child(SharedString::from(format!("Invalid plot: {err}")))
                .into_any_element(),
            (Ok(_), None) => div().into_any_element(),
        };
        let hint = if self.sliders.is_empty() {
            "plot".to_owned()
        } else {
            format!(
                "plot · {} to explore",
                if self.sliders.len() == 1 {
                    "drag the slider"
                } else {
                    "drag the sliders"
                }
            )
        };
        v_flex()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(radius)
            .border_1()
            .border_color(border)
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
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(hint)),
                    ),
            )
            .child(body)
            .when_some(self.render_sliders(cx), |this, sliders| this.child(sliders))
    }
}

/// The view of block `id`, in sync with `spec`.
pub fn view(id: BlockId, spec: &PlotSpec, window: &mut Window, cx: &mut App) -> Entity<PlotView> {
    let view = super::block_state(id, window, cx, |_, cx| PlotView::new(id, spec.clone(), cx));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Plot(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
