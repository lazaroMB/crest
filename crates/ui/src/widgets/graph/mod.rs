//! The graph widget: nodes and edges of a [`GraphSpec`], painted natively.
//!
//! The labels are measured on the UI thread, then the layout
//! ([`domain::widget::graph::layout`]) runs on the background executor;
//! the canvas says "laying out…" until it is back, and draws a message
//! instead of a graph past [`DRAW_LIMIT`] nodes. Nodes are rounded boxes
//! colored by `kind` (and up to twice as large by `size`), edges polylines with arrowheads (directed graphs)
//! and labels, groups boxes around their nodes. A drag (or shift + wheel)
//! pans, ctrl/cmd + wheel (or `+`/`-` and the toolbar) zooms around the
//! cursor, `0` fits. A click on a node focuses it: its neighbourhood
//! stays bright, the rest dims, a detail line lists its fields and
//! [`GraphEvent::Focused`] tells the parent; a click on the background
//! or Escape clears it.
//!
//! [`view`] gives the entity of an id (to subscribe or drive it) and
//! [`render`] the whole widget with its header.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use domain::block::BlockId;
use domain::widget::graph::layout::{Geometry, LayoutError, Rect, layout};
use domain::widget::graph::{DRAW_LIMIT, GraphSpec};
use gpui::{
    AnyElement, App, BorderStyle, Bounds, ContentMask, Context, EventEmitter, FocusHandle, Font,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point,
    Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    TextAlign, TextRun, Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, quad,
    size,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::timeline::axis::color_slot;
use super::timeline::value_text;

/// Font size of node labels at zoom 1.
pub const FONT_SIZE: f32 = 12.0;
/// Height of a node box.
pub const NODE_HEIGHT: f32 = 26.0;
/// Narrowest and widest a node box gets.
pub const NODE_WIDTH: (f32, f32) = (44.0, 220.0);
/// Space between a label and the sides of its box.
const NODE_PAD: f32 = 10.0;
/// Shortest and tallest the canvas gets.
pub const CANVAS_HEIGHT: (f32, f32) = (180.0, 480.0);
/// Zoom step of the keys, the buttons and one wheel notch.
pub const ZOOM_STEP: f32 = 1.25;
/// Zoom limits.
pub const ZOOM_RANGE: (f32, f32) = (0.05, 4.0);
/// Below this zoom, labels are left out.
const MIN_TEXT_ZOOM: f32 = 0.35;
/// Pixels a press may move and still be a click.
const CLICK_SLOP: f32 = 3.0;
/// Opacity of what is outside the focused neighbourhood.
const DIM: f32 = 0.18;
/// Attributes listed in the detail line.
const DETAIL_ATTRS: usize = 12;

/// What the graph tells its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEvent {
    /// A click focused a node (by index in the spec), or cleared it.
    Focused(Option<usize>),
}

/// Where the layout is.
#[derive(Debug, Clone)]
pub enum LayoutState {
    /// Running on the background executor.
    Pending,
    /// Done.
    Ready(Rc<Geometry>),
    /// Not drawn (too many nodes).
    Failed(SharedString),
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    last: Point<Pixels>,
    moved: f32,
}

/// Per-id state of a graph: the spec, its layout, the zoom and pan, and
/// the hovered and focused node.
pub struct GraphView {
    spec: Rc<GraphSpec>,
    /// Neighbours of each node.
    adjacent: Rc<Vec<BTreeSet<usize>>>,
    state: LayoutState,
    /// Screen pixels per layout unit.
    scale: f32,
    /// Where the layout's origin is, relative to the canvas.
    offset: (f32, f32),
    /// Fit the drawing at the next paint (after a layout).
    fit_pending: Rc<Cell<bool>>,
    hover: Option<usize>,
    focus: Option<usize>,
    drag: Option<Drag>,
    show_details: bool,
    /// Canvas bounds of the last paint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus_handle: FocusHandle,
    _layout: Option<Task<()>>,
}

impl EventEmitter<GraphEvent> for GraphView {}

/// Neighbours of each node of `spec`.
fn adjacency(spec: &GraphSpec) -> Vec<BTreeSet<usize>> {
    let mut out = vec![BTreeSet::new(); spec.nodes.len()];
    for (a, b) in spec.ends() {
        if a != b && a < out.len() && b < out.len() {
            out[a].insert(b);
            out[b].insert(a);
        }
    }
    out
}

/// Box sizes of the nodes: the label width plus padding, in bounds,
/// grown by the node's `size`.
fn measure(spec: &GraphSpec, window: &Window) -> Vec<(f64, f64)> {
    let font = window.text_style().font();
    spec.nodes
        .iter()
        .map(|node| {
            let text = shape(node.text(), &font, px(FONT_SIZE), gpui::black(), window);
            let w = (f32::from(text.width()) + 2.0 * NODE_PAD).clamp(NODE_WIDTH.0, NODE_WIDTH.1);
            // A sized node (a metric) grows up to twice as wide and tall.
            let grow = 1.0 + node.size.unwrap_or(0.0).clamp(0.0, 1.0);
            (f64::from(w) * grow, f64::from(NODE_HEIGHT) * grow)
        })
        .collect()
}

fn shape(text: &str, font: &Font, size: Pixels, color: Hsla, window: &Window) -> gpui::ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(SharedString::from(text.to_owned()), size, &[run], None)
}

impl GraphView {
    fn new(spec: &GraphSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            spec: Rc::new(spec.clone()),
            adjacent: Rc::new(adjacency(spec)),
            state: LayoutState::Pending,
            scale: 1.0,
            offset: (0.0, 0.0),
            fit_pending: Rc::new(Cell::new(true)),
            hover: None,
            focus: None,
            drag: None,
            show_details: true,
            bounds: Rc::default(),
            focus_handle: cx.focus_handle(),
            _layout: None,
        };
        view.start_layout(window, cx);
        view
    }

    fn start_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spec.nodes.len() > DRAW_LIMIT {
            self.state = LayoutState::Failed(
                LayoutError::TooLarge {
                    nodes: self.spec.nodes.len(),
                    limit: DRAW_LIMIT,
                }
                .to_string()
                .into(),
            );
            return;
        }
        let sizes = measure(&self.spec, window);
        let spec = (*self.spec).clone();
        let task = cx
            .background_executor()
            .spawn(async move { layout(&spec, &sizes) });
        self._layout = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(geometry) => LayoutState::Ready(Rc::new(geometry)),
                    Err(err) => LayoutState::Failed(err.to_string().into()),
                };
                this.fit_pending.set(true);
                cx.notify();
            })
            .ok();
        }));
    }

    fn sync(&mut self, spec: &GraphSpec, window: &mut Window, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            *self = Self {
                focus_handle: self.focus_handle.clone(),
                show_details: self.show_details,
                ..Self::new(spec, window, cx)
            };
            cx.notify();
        }
    }

    /// The spec drawn.
    pub fn spec(&self) -> &GraphSpec {
        &self.spec
    }

    /// Where the layout is.
    pub fn state(&self) -> &LayoutState {
        &self.state
    }

    /// The layout, once done.
    pub fn geometry(&self) -> Option<&Geometry> {
        match &self.state {
            LayoutState::Ready(geometry) => Some(geometry),
            _ => None,
        }
    }

    /// Screen pixels per layout unit.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The focused node.
    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// The focused node and its neighbours, when a node is focused.
    pub fn neighbourhood(&self) -> Option<BTreeSet<usize>> {
        let focus = self.focus?;
        let mut set = self.adjacent.get(focus).cloned().unwrap_or_default();
        set.insert(focus);
        Some(set)
    }

    /// Whether the built-in detail line shows the focused node (on by
    /// default; off when the parent has a detail pane of its own).
    pub fn set_show_details(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_details = show;
        cx.notify();
    }

    /// Focuses `node` (or nothing) and tells the parent.
    pub fn set_focus(&mut self, node: Option<usize>, cx: &mut Context<Self>) {
        let node = node.filter(|&n| n < self.spec.nodes.len());
        if self.focus != node {
            self.focus = node;
            cx.emit(GraphEvent::Focused(node));
            cx.notify();
        }
    }

    /// Zooms by `factor` (> 1 in) around `anchor` (window coordinates),
    /// or the middle of the canvas.
    pub fn zoom_by(&mut self, factor: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let bounds = self.bounds.get();
        let (ax, ay) = anchor.map_or(
            (
                f32::from(bounds.size.width) / 2.0,
                f32::from(bounds.size.height) / 2.0,
            ),
            |p| {
                (
                    f32::from(p.x - bounds.origin.x),
                    f32::from(p.y - bounds.origin.y),
                )
            },
        );
        let next = (self.scale * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let ratio = next / self.scale;
        self.offset = (
            ax - (ax - self.offset.0) * ratio,
            ay - (ay - self.offset.1) * ratio,
        );
        self.scale = next;
        cx.notify();
    }

    /// The whole drawing in view, at most at zoom 1.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.fit_to(self.bounds.get());
        cx.notify();
    }

    fn fit_to(&mut self, bounds: Bounds<Pixels>) {
        if let Some(geometry) = self.geometry() {
            (self.scale, self.offset) = fitted(geometry, bounds);
        }
    }

    /// Pans by `(dx, dy)` pixels.
    pub fn pan_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.offset = (self.offset.0 + dx, self.offset.1 + dy);
        cx.notify();
    }

    /// Window point of layout point `at`, as last painted.
    pub fn to_window(&self, at: (f64, f64)) -> Point<Pixels> {
        let origin = self.bounds.get().origin;
        origin
            + point(
                px(self.offset.0 + at.0 as f32 * self.scale),
                px(self.offset.1 + at.1 as f32 * self.scale),
            )
    }

    /// The node under `position` (window coordinates).
    fn node_at(&self, position: Point<Pixels>) -> Option<usize> {
        let geometry = self.geometry()?;
        let bounds = self.bounds.get();
        if !bounds.contains(&position) {
            return None;
        }
        let x = (f32::from(position.x - bounds.origin.x) - self.offset.0) / self.scale;
        let y = (f32::from(position.y - bounds.origin.y) - self.offset.1) / self.scale;
        geometry.node_at((f64::from(x), f64::from(y)))
    }

    fn canvas_height(&self) -> f32 {
        match &self.state {
            LayoutState::Ready(geometry) => {
                (geometry.height as f32).clamp(CANVAS_HEIGHT.0, CANVAS_HEIGHT.1)
            }
            _ => CANVAS_HEIGHT.0,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(Drag {
            last: event.position,
            moved: 0.0,
        });
        window.focus(&self.focus_handle, cx);
        // Keep the timeline row from treating the press as its click.
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.as_mut() {
            if event.pressed_button == Some(MouseButton::Left) {
                let delta = event.position - drag.last;
                let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
                drag.moved += dx.abs() + dy.abs();
                drag.last = event.position;
                if drag.moved > CLICK_SLOP {
                    self.pan_by(dx, dy, cx);
                }
                return;
            }
            self.drag = None;
        }
        let hover = self.node_at(event.position);
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if drag.moved > CLICK_SLOP {
            return;
        }
        let node = self.node_at(event.position);
        // A second click on the focused node clears the focus.
        let next = if node == self.focus { None } else { node };
        self.set_focus(next, cx);
        cx.stop_propagation();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control || event.modifiers.platform {
            self.zoom_by(ZOOM_STEP.powf(dy / 20.0), Some(event.position), cx);
            cx.stop_propagation();
        } else if event.modifiers.shift {
            self.pan_by(dy, dx, cx);
            cx.stop_propagation();
        } else if dx.abs() > dy.abs() {
            self.pan_by(dx, 0.0, cx);
            cx.stop_propagation();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event
            .keystroke
            .key_char
            .as_deref()
            .unwrap_or(event.keystroke.key.as_str());
        let step = 40.0;
        match key {
            "+" | "=" => self.zoom_by(ZOOM_STEP, None, cx),
            "-" => self.zoom_by(1.0 / ZOOM_STEP, None, cx),
            "0" => self.fit(cx),
            "left" => self.pan_by(step, 0.0, cx),
            "right" => self.pan_by(-step, 0.0, cx),
            "up" => self.pan_by(0.0, step, cx),
            "down" => self.pan_by(0.0, -step, cx),
            "escape" if self.focus.is_some() => self.set_focus(None, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = self
            .spec
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("Graph");
        let hint = format!(
            "{} nodes · {} edges · {} · drag to pan · ctrl+wheel to zoom · click a node to focus",
            self.spec.nodes.len(),
            self.spec.edges.len(),
            self.spec.layout.name(),
        );
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
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(super::readout(format!("{:.0}%", self.scale * 100.0), cx))
                    .child(super::tool_button("graph-zoom-out", "−", cx).on_click(
                        cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, None, cx)),
                    ))
                    .child(
                        super::tool_button("graph-zoom-in", "+", cx).on_click(
                            cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, None, cx)),
                        ),
                    )
                    .child(
                        super::tool_button("graph-fit", "fit", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                    ),
            )
            .into_any_element()
    }

    fn details(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_details {
            return None;
        }
        let ix = self.focus?;
        let node = self.spec.nodes.get(ix)?;
        let theme = cx.theme();
        let mut head = node.text().to_owned();
        if node.label.is_some() && node.label.as_deref() != Some(node.id.as_str()) {
            head.push_str(&format!(" ({})", node.id));
        }
        if let Some(kind) = &node.kind {
            head.push_str(&format!(" · {kind}"));
        }
        if let Some(group) = &node.group {
            head.push_str(&format!(" · {}", self.spec.group_label(group)));
        }
        let degree = self.adjacent.get(ix).map_or(0, BTreeSet::len);
        head.push_str(&format!(
            " · {degree} neighbour{}",
            if degree == 1 { "" } else { "s" }
        ));
        let mut attrs: Vec<String> = node
            .attrs
            .iter()
            .take(DETAIL_ATTRS)
            .map(|(k, v)| format!("{k}: {}", value_text(v)))
            .collect();
        if node.attrs.len() > DETAIL_ATTRS {
            attrs.push(format!("… {} more", node.attrs.len() - DETAIL_ATTRS));
        }
        let attrs = attrs.join("   ");
        Some(
            v_flex()
                .debug_selector(|| "graph-details".to_owned())
                .px_1()
                .text_xs()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(head),
                        )
                        .child(
                            div()
                                .id("graph-clear")
                                .debug_selector(|| "graph-clear".to_owned())
                                .cursor_pointer()
                                .text_color(theme.muted_foreground)
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_focus(None, cx);
                                })),
                        ),
                )
                .when(!attrs.is_empty(), |d| {
                    d.child(div().text_color(theme.muted_foreground).child(attrs))
                })
                .into_any_element(),
        )
    }
}

/// Zoom and offset that show all of `geometry` in `bounds`, at most at
/// zoom 1, centred.
fn fitted(geometry: &Geometry, bounds: Bounds<Pixels>) -> (f32, (f32, f32)) {
    let (w, h) = (
        f32::from(bounds.size.width).max(1.0),
        f32::from(bounds.size.height).max(1.0),
    );
    let (gw, gh) = (geometry.width as f32, geometry.height as f32);
    let scale = (w / gw).min(h / gh).clamp(ZOOM_RANGE.0, 1.0);
    (scale, ((w - gw * scale) / 2.0, (h - gh * scale) / 2.0))
}

/// Colors the canvas paints with, read from the theme at render time.
#[derive(Clone)]
struct Colors {
    palette: [Hsla; 5],
    surface: Hsla,
    foreground: Hsla,
    muted: Hsla,
    edge: Hsla,
    group: Hsla,
    border: Hsla,
}

impl Colors {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            palette: [0, 1, 2, 3, 4].map(|ix| super::palette(ix, cx)),
            surface: theme.background,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            edge: theme.muted_foreground,
            group: theme.muted,
            border: theme.border,
        }
    }
}

/// Everything one paint needs.
struct Frame {
    spec: Rc<GraphSpec>,
    geometry: Rc<Geometry>,
    scale: f32,
    offset: (f32, f32),
    hover: Option<usize>,
    focus: Option<usize>,
    near: Option<BTreeSet<usize>>,
    colors: Colors,
    font: Font,
}

impl Frame {
    fn at(&self, origin: Point<Pixels>, (x, y): (f64, f64)) -> Point<Pixels> {
        origin
            + point(
                px(self.offset.0 + x as f32 * self.scale),
                px(self.offset.1 + y as f32 * self.scale),
            )
    }

    fn rect(&self, origin: Point<Pixels>, rect: &Rect) -> Bounds<Pixels> {
        Bounds::new(
            self.at(origin, (rect.x, rect.y)),
            size(
                px(rect.w as f32 * self.scale),
                px(rect.h as f32 * self.scale),
            ),
        )
    }

    fn lit(&self, node: usize) -> bool {
        self.near.as_ref().is_none_or(|near| near.contains(&node))
    }

    /// Paints `text` with its left edge at `x` (or centred on it) and
    /// vertically centred on `y`.
    fn text(
        &self,
        text: &str,
        (x, y): (Pixels, Pixels),
        centred: bool,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        let font_size = px(FONT_SIZE * self.scale.min(1.6));
        let line = shape(text, &self.font, font_size, color, window);
        let left = if centred { x - line.width() / 2.0 } else { x };
        let height = font_size * 1.3;
        line.paint(
            point(left, y - height / 2.0),
            height,
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let origin = bounds.origin;
        let colors = &self.colors;
        let text = self.scale >= MIN_TEXT_ZOOM;
        let radius = px((4.0 * self.scale).min(6.0));
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for group in &self.geometry.groups {
                let rect = self.rect(origin, &group.rect);
                window.paint_quad(quad(
                    rect,
                    radius,
                    colors.group.opacity(0.35),
                    px(1.0),
                    colors.border,
                    BorderStyle::Dashed,
                ));
                if text {
                    let at = (
                        rect.origin.x + px(6.0),
                        rect.origin.y + px(FONT_SIZE * self.scale.min(1.6) * 0.8 + 2.0),
                    );
                    self.text(&group.label, at, false, colors.muted, window, cx);
                }
            }
            self.paint_edges(origin, bounds, text, window, cx);
            for (ix, (node, rect)) in self.spec.nodes.iter().zip(&self.geometry.nodes).enumerate() {
                let screen = self.rect(origin, rect);
                if !screen.intersects(&bounds) {
                    continue;
                }
                let alpha = if self.lit(ix) { 1.0 } else { DIM };
                let color = colors.palette[node.kind.as_deref().map_or(0, color_slot)];
                let (border, width) = if self.focus == Some(ix) {
                    (colors.foreground, 2.0)
                } else if self.hover == Some(ix) {
                    (colors.foreground.opacity(0.7), 1.5)
                } else {
                    (color, 1.0)
                };
                window.paint_quad(fill(screen, colors.surface).corner_radii(radius));
                window.paint_quad(quad(
                    screen,
                    radius,
                    color.opacity(0.18 * alpha),
                    px(width),
                    border.opacity(alpha),
                    BorderStyle::Solid,
                ));
                if text {
                    window.with_content_mask(Some(ContentMask { bounds: screen }), |window| {
                        let centre = screen.center();
                        self.text(
                            node.text(),
                            (centre.x, centre.y),
                            true,
                            colors.foreground.opacity(alpha),
                            window,
                            cx,
                        );
                    });
                }
            }
        });
    }

    fn paint_edges(
        &self,
        origin: Point<Pixels>,
        bounds: Bounds<Pixels>,
        text: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let colors = &self.colors;
        let heaviest = self
            .spec
            .edges
            .iter()
            .filter_map(|e| e.weight)
            .map(f64::abs)
            .fold(0.0, f64::max);
        let ends = self.spec.ends();
        for route in &self.geometry.edges {
            if route.points.len() < 2 {
                continue;
            }
            let (a, b) = ends.get(route.edge).copied().unwrap_or((0, 0));
            let lit = match self.focus {
                Some(f) => a == f || b == f,
                None => true,
            };
            let color = if lit {
                colors.edge
            } else {
                colors.edge.opacity(DIM)
            };
            let edge = self.spec.edges.get(route.edge);
            let weight = edge
                .and_then(|e| e.weight)
                .filter(|_| heaviest > 0.0)
                .map_or(0.0, |w| (w.abs() / heaviest) as f32);
            let width = (1.2 + 2.0 * weight) * self.scale.clamp(0.5, 1.5);
            let points: Vec<Point<Pixels>> =
                route.points.iter().map(|&p| self.at(origin, p)).collect();
            let mut line = PathBuilder::stroke(px(width));
            let head = (7.0 * self.scale.clamp(0.5, 1.5)).max(3.0);
            let tip = points[points.len() - 1];
            let before = points[points.len() - 2];
            let (dx, dy) = (f32::from(tip.x - before.x), f32::from(tip.y - before.y));
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let (ux, uy) = (dx / len, dy / len);
            let directed = self.spec.directed && len > head;
            line.move_to(points[0]);
            for &p in &points[1..points.len() - 1] {
                line.line_to(p);
            }
            if directed {
                line.line_to(tip - point(px(ux * head), px(uy * head)));
            } else {
                line.line_to(tip);
            }
            if let Ok(path) = line.build() {
                window.paint_path(path, color);
            }
            if directed {
                let base = tip - point(px(ux * head), px(uy * head));
                let side = point(px(-uy * head / 2.0), px(ux * head / 2.0));
                let mut arrow = PathBuilder::fill();
                arrow.move_to(tip);
                arrow.line_to(base + side);
                arrow.line_to(base - side);
                arrow.close();
                if let Ok(path) = arrow.build() {
                    window.paint_path(path, color);
                }
            }
            if text
                && let (Some(label), Some(at)) =
                    (edge.and_then(|e| e.label.as_deref()), route.label_at)
            {
                let at = self.at(origin, at);
                if bounds.contains(&at) {
                    let font_size = FONT_SIZE * 0.9 * self.scale.min(1.6);
                    let shaped = shape(label, &self.font, px(font_size), color, window);
                    let w = shaped.width() + px(6.0);
                    let h = px(font_size * 1.3);
                    window.paint_quad(
                        fill(
                            Bounds::new(at - point(w / 2.0, h / 2.0), size(w, h)),
                            colors.surface.opacity(0.85),
                        )
                        .corner_radii(px(3.0)),
                    );
                    shaped
                        .paint(
                            at - point(shaped.width() / 2.0, h / 2.0),
                            h,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                }
            }
        }
    }
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let height = self.canvas_height();
        let message: Option<SharedString> = match &self.state {
            LayoutState::Pending => {
                Some(format!("laying out {} nodes…", self.spec.nodes.len()).into())
            }
            LayoutState::Failed(reason) => Some(reason.clone()),
            LayoutState::Ready(_) if self.spec.nodes.is_empty() => Some("no nodes".into()),
            LayoutState::Ready(_) => None,
        };
        let geometry = match &self.state {
            LayoutState::Ready(geometry) => Some(geometry.clone()),
            _ => None,
        };
        let frame = geometry.map(|geometry| Frame {
            spec: self.spec.clone(),
            geometry,
            scale: self.scale,
            offset: self.offset,
            hover: self.hover,
            focus: self.focus,
            near: self.neighbourhood(),
            colors: Colors::new(cx),
            font: window.text_style().font(),
        });
        let bounds_cell = self.bounds.clone();
        let fit = self.fit_pending.clone();
        let this = cx.entity().downgrade();
        let surface = canvas(
            move |bounds, _, cx| {
                bounds_cell.set(bounds);
                let mut frame = frame;
                // A fresh layout is fitted once the canvas knows its size.
                if let Some(frame) = frame.as_mut()
                    && fit.get()
                {
                    fit.set(false);
                    let (scale, offset) = fitted(&frame.geometry, bounds);
                    (frame.scale, frame.offset) = (scale, offset);
                    this.update(cx, |this, _| (this.scale, this.offset) = (scale, offset))
                        .ok();
                }
                frame
            },
            move |bounds, frame, window, cx| {
                if let Some(frame) = frame {
                    frame.paint(bounds, window, cx);
                }
            },
        )
        .size_full();
        let canvas = div()
            .id("graph-canvas")
            .debug_selector(|| "graph-canvas".to_owned())
            .track_focus(&self.focus_handle)
            .relative()
            .w_full()
            .h(px(height + 2.0))
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .when(self.hover.is_some(), |d| d.cursor_pointer())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_key_down(cx.listener(Self::key_down))
            .child(surface)
            .children(message.map(|text| {
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .right_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .debug_selector(|| "graph-message".to_owned())
                    .child(text)
            }));
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "graph".to_owned())
            .child(self.header(cx))
            .child(canvas)
            .children(self.details(cx))
    }
}

/// The graph entity of `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &GraphSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<GraphView> {
    let view = crate::blocks::block_state(id, window, cx, |window, cx| {
        GraphView::new(spec, window, cx)
    });
    let focus = view.read(cx).focus_handle.clone();
    crate::blocks::remember_focus(id, &focus, cx);
    view.update(cx, |view, cx| view.sync(spec, window, cx));
    view
}

/// The whole widget (header, canvas, detail line) of `id`.
pub fn render(id: BlockId, spec: &GraphSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    view(id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
