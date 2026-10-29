//! Renderer of the `drawing` block kind (`show_dxf`): a 2D DXF drawing.
//!
//! A summary line comes first (release, units, header and geometry
//! extents, entity and layer counts, what was not drawn). The canvas
//! strokes every entity of the visible layers in its ACI color, mapped onto
//! the theme ([`themed_color`]: ACI 7 is the foreground, very light colors
//! are darkened on a light theme), with text at its height (unrotated;
//! skipped below a few pixels). Drag pans, `Ctrl`/`Cmd` + wheel zooms about
//! the cursor (a plain wheel scrolls the chat), `+`/`-`/`0` and the toolbar
//! zoom and fit; the cursor position is read out in drawing units. The
//! layer list toggles layers (the file's off layers start off) and shows
//! each one's color and entity count. Reader notes come last.

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::drawing::{DrawingSpec, Extents, P2, Shape, aci_rgb, flatten};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, FocusHandle, Font, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, PathBuilder, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, TextAlign, TextRun, Window, canvas,
    div, point, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::theme::token;
use crate::timeline::TimelineEntry;
use crate::widgets::{readout, tool_button};

/// Height of the drawing canvas, in pixels.
pub const CANVAS_HEIGHT: f32 = 480.0;
/// Zoom step of the keys and buttons.
const ZOOM_STEP: f64 = 1.25;
/// Zoom step of one wheel notch.
const WHEEL_STEP: f64 = 1.15;
/// Share of the canvas left around the drawing when fitted.
const MARGIN: f64 = 0.05;
/// Texts smaller than this (pixels) are not drawn.
const MIN_TEXT_PX: f32 = 4.0;
/// Most texts drawn per frame.
const MAX_TEXTS: usize = 2000;
/// Layers listed; the rest are counted.
const MAX_LAYER_ROWS: usize = 200;
/// Notes listed.
const LISTED: usize = 6;

/// Where the drawing is on the canvas: the world point at the canvas
/// centre and pixels per drawing unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// World point at the centre.
    pub center: P2,
    /// Pixels per drawing unit.
    pub scale: f64,
}

impl Viewport {
    /// The view that shows `extents` whole in a canvas of `size` pixels.
    #[must_use]
    pub fn fit(extents: Option<Extents>, size: (f64, f64)) -> Self {
        let Some(x) = extents else {
            return Self {
                center: [0.0, 0.0],
                scale: 1.0,
            };
        };
        let [w, h] = x.size();
        let (cw, ch) = (
            size.0.max(1.0) * (1.0 - 2.0 * MARGIN),
            size.1.max(1.0) * (1.0 - 2.0 * MARGIN),
        );
        let scale = match (w > 0.0, h > 0.0) {
            (true, true) => (cw / w).min(ch / h),
            (true, false) => cw / w,
            (false, true) => ch / h,
            (false, false) => 1.0,
        };
        Self {
            center: [(x.min[0] + x.max[0]) / 2.0, (x.min[1] + x.max[1]) / 2.0],
            scale: if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            },
        }
    }

    /// Canvas pixel of world point `p` (y up in the drawing, down on
    /// screen).
    #[must_use]
    pub fn to_screen(&self, p: P2, size: (f64, f64)) -> (f64, f64) {
        (
            (p[0] - self.center[0]) * self.scale + size.0 / 2.0,
            size.1 / 2.0 - (p[1] - self.center[1]) * self.scale,
        )
    }

    /// World point under canvas pixel `(x, y)`.
    #[must_use]
    pub fn to_world(&self, (x, y): (f64, f64), size: (f64, f64)) -> P2 {
        [
            (x - size.0 / 2.0) / self.scale + self.center[0],
            (size.1 / 2.0 - y) / self.scale + self.center[1],
        ]
    }

    /// Zooms by `factor`, keeping the world point under `(x, y)` there.
    pub fn zoom_about(&mut self, factor: f64, at: (f64, f64), size: (f64, f64)) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let anchor = self.to_world(at, size);
        self.scale = (self.scale * factor).clamp(1e-9, 1e9);
        let moved = self.to_world(at, size);
        self.center = [
            self.center[0] + anchor[0] - moved[0],
            self.center[1] + anchor[1] - moved[1],
        ];
    }

    /// Moves the drawing by `(dx, dy)` pixels.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.center = [
            self.center[0] - dx / self.scale,
            self.center[1] + dy / self.scale,
        ];
    }
}

/// The theme color of ACI `index`: 7 (white/black) is `foreground`; on a
/// light background bright colors (yellow, cyan, light grays) are
/// darkened, on a dark one very dark colors (blue) lightened, so they
/// show.
#[must_use]
pub fn themed_color(index: u8, foreground: Hsla, dark_background: bool) -> Hsla {
    if index == 7 || index == 0 {
        return foreground;
    }
    let [r, g, b] = aci_rgb(index);
    let mut color: Hsla = gpui::Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
    .into();
    let luminance = (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0;
    if !dark_background && luminance > 0.7 {
        color.l = color.l.min(0.35);
    } else if dark_background && luminance < 0.2 {
        color.l = color.l.max(0.65);
    }
    color
}

/// The summary text.
#[must_use]
pub fn summary_text(spec: &DrawingSpec) -> String {
    let unit = spec.units.as_deref().unwrap_or("units");
    let mut parts = Vec::new();
    if let Some(v) = &spec.version {
        parts.push(v.clone());
    }
    parts.push(match &spec.units {
        Some(u) => u.clone(),
        None => "no units set".to_owned(),
    });
    let size = |x: &Extents| {
        let [w, h] = x.size();
        format!("{} × {} {unit}", number(w), number(h))
    };
    if let Some(x) = spec.computed_extents() {
        parts.push(format!("geometry {}", size(&x)));
    }
    if let Some(x) = &spec.header_extents {
        parts.push(format!("header extents {}", size(x)));
    }
    parts.push(format!("{} entities", spec.entities.len()));
    parts.push(format!("{} layers", spec.layers.len()));
    if spec.inserts > 0 {
        parts.push(format!("{} inserts", spec.inserts));
    }
    let skipped: usize = spec.skipped.iter().map(|s| s.count).sum();
    if skipped > 0 {
        parts.push(format!("{skipped} not drawn"));
    }
    parts.join(" · ")
}

/// A coordinate for readouts: up to 3 decimals.
fn number(v: f64) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// An entity ready to stroke: its points, box and color index.
struct Stroke {
    layer: usize,
    color: u8,
    points: Vec<P2>,
    min: P2,
    max: P2,
    closed_dot: bool,
}

fn strokes(spec: &DrawingSpec) -> Vec<Stroke> {
    spec.entities
        .iter()
        .filter(|e| !matches!(e.shape, Shape::Text { .. }))
        .map(|e| {
            let points = flatten(&e.shape);
            let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
            for p in &points {
                for k in 0..2 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
            Stroke {
                layer: e.layer,
                color: spec.color_of(e),
                closed_dot: matches!(e.shape, Shape::Point { .. }),
                points,
                min,
                max,
            }
        })
        .collect()
}

/// Per-block state: the drawing, its strokes, the visible layers and
/// the viewport.
pub struct DrawingView {
    spec: Rc<DrawingSpec>,
    strokes: Rc<Vec<Stroke>>,
    visible: Vec<bool>,
    view: Rc<Cell<Option<Viewport>>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Point<Pixels>>,
    cursor: Option<P2>,
    focus: FocusHandle,
}

impl DrawingView {
    fn new(spec: &DrawingSpec, cx: &mut Context<Self>) -> Self {
        Self {
            spec: Rc::new(spec.clone()),
            strokes: Rc::new(strokes(spec)),
            visible: spec.layers.iter().map(|l| l.visible()).collect(),
            view: Rc::default(),
            bounds: Rc::default(),
            drag: None,
            cursor: None,
            focus: cx.focus_handle(),
        }
    }

    fn sync(&mut self, spec: &DrawingSpec, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            *self = Self::new(spec, cx);
        }
    }

    fn size(&self) -> (f64, f64) {
        let size = self.bounds.get().size;
        (
            f64::from(f32::from(size.width)).max(1.0),
            f64::from(f32::from(size.height)).max(1.0),
        )
    }

    /// The viewport (fitted to the visible layers until zoomed or panned).
    pub fn viewport(&self) -> Viewport {
        self.view
            .get()
            .unwrap_or_else(|| Viewport::fit(self.visible_extents(), self.size()))
    }

    fn visible_extents(&self) -> Option<Extents> {
        self.spec
            .extents_where(|l| self.visible.get(l).copied().unwrap_or(true))
            .or_else(|| self.spec.computed_extents())
    }

    /// Whether layer `index` is shown.
    pub fn layer_visible(&self, index: usize) -> bool {
        self.visible.get(index).copied().unwrap_or(false)
    }

    /// Shows layer `index`, or hides it.
    pub fn toggle_layer(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(v) = self.visible.get_mut(index) {
            *v = !*v;
            cx.notify();
        }
    }

    /// Zooms by `factor` about the canvas centre.
    pub fn zoom_by(&mut self, factor: f64, cx: &mut Context<Self>) {
        let size = self.size();
        let mut view = self.viewport();
        view.zoom_about(factor, (size.0 / 2.0, size.1 / 2.0), size);
        self.view.set(Some(view));
        cx.notify();
    }

    /// Fits the visible layers again.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.view.set(None);
        cx.notify();
    }

    /// Pans by `(dx, dy)` pixels.
    pub fn pan_by(&mut self, dx: f64, dy: f64, cx: &mut Context<Self>) {
        let mut view = self.viewport();
        view.pan(dx, dy);
        self.view.set(Some(view));
        cx.notify();
    }

    /// The drawing point under the mouse, if it is over the canvas.
    pub fn cursor(&self) -> Option<P2> {
        self.cursor
    }

    fn local(&self, at: Point<Pixels>) -> (f64, f64) {
        let origin = self.bounds.get().origin;
        (
            f64::from(f32::from(at.x - origin.x)),
            f64::from(f32::from(at.y - origin.y)),
        )
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.drag = Some(event.position);
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let size = self.size();
        if self.bounds.get().contains(&event.position) {
            self.cursor = Some(self.viewport().to_world(self.local(event.position), size));
        }
        if let Some(last) = self.drag {
            if event.pressed_button.is_none() {
                self.drag = None;
            } else {
                let d = event.position - last;
                self.pan_by(f64::from(f32::from(d.x)), f64::from(f32::from(d.y)), cx);
                self.drag = Some(event.position);
            }
        }
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A plain wheel is the chat's: it keeps scrolling over the block.
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let dy = f64::from(f32::from(event.delta.pixel_delta(px(20.0)).y));
        let size = self.size();
        let mut view = self.viewport();
        view.zoom_about(WHEEL_STEP.powf(dy / 20.0), self.local(event.position), size);
        self.view.set(Some(view));
        cx.notify();
        cx.stop_propagation();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event
            .keystroke
            .key_char
            .as_deref()
            .unwrap_or(event.keystroke.key.as_str());
        match key {
            "+" | "=" => self.zoom_by(ZOOM_STEP, cx),
            "-" => self.zoom_by(1.0 / ZOOM_STEP, cx),
            "0" => self.fit(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn canvas(&self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let dark = theme.background.l < 0.5;
        let background = token(cx, "canvas.bg");
        let strokes = Rc::clone(&self.strokes);
        let spec = Rc::clone(&self.spec);
        let visible = self.visible.clone();
        let (view_cell, bounds_cell) = (Rc::clone(&self.view), Rc::clone(&self.bounds));
        let extents = self.visible_extents();
        let font = window.text_style().font();
        canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, cx| {
                let size = (
                    f64::from(f32::from(bounds.size.width)).max(1.0),
                    f64::from(f32::from(bounds.size.height)).max(1.0),
                );
                let view = view_cell
                    .get()
                    .unwrap_or_else(|| Viewport::fit(extents, size));
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(gpui::fill(bounds, background));
                    paint_strokes(
                        &strokes, &visible, &view, size, bounds, foreground, dark, window,
                    );
                    paint_texts(
                        &spec, &visible, &view, size, bounds, &font, foreground, dark, window, cx,
                    );
                });
            },
        )
        .size_full()
    }

    fn layer_rows(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border, foreground) = (theme.muted_foreground, theme.border, theme.foreground);
        let dark = theme.background.l < 0.5;
        let rows: Vec<AnyElement> = self
            .spec
            .layers
            .iter()
            .enumerate()
            .take(MAX_LAYER_ROWS)
            .map(|(i, layer)| {
                let shown = self.layer_visible(i);
                let swatch = themed_color(layer.color, foreground, dark);
                h_flex()
                    .id(("drawing-layer", i))
                    .debug_selector(move || format!("drawing-layer-{i}"))
                    .gap_2()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_layer(i, cx);
                    }))
                    .child(div().w_4().child(if shown { "●" } else { "○" }))
                    .child(div().size_3().rounded_sm().bg(swatch))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(!shown, |d| d.text_color(muted))
                            .child(SharedString::from(layer.name.clone())),
                    )
                    .child(div().text_color(muted).child(format!(
                        "ACI {} · {} entities{}",
                        layer.color,
                        layer.entities,
                        if layer.off { " · off in the file" } else { "" }
                    )))
                    .into_any_element()
            })
            .collect();
        let more = self.spec.layers.len().saturating_sub(MAX_LAYER_ROWS);
        v_flex()
            .debug_selector(|| "drawing-layers".to_owned())
            .text_xs()
            .border_t_1()
            .border_color(border)
            .pt_1()
            .children(rows)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_color(muted)
                        .child(format!("… {more} more layers")),
                )
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_strokes(
    strokes: &[Stroke],
    visible: &[bool],
    view: &Viewport,
    size: (f64, f64),
    bounds: Bounds<Pixels>,
    foreground: Hsla,
    dark: bool,
    window: &mut Window,
) {
    // World box of the canvas, to skip what is off screen.
    let a = view.to_world((0.0, size.1), size);
    let b = view.to_world((size.0, 0.0), size);
    let at = |p: P2| {
        let (x, y) = view.to_screen(p, size);
        // Screen coordinates fit an f32.
        #[allow(clippy::cast_possible_truncation)]
        point(
            bounds.origin.x + px(x as f32),
            bounds.origin.y + px(y as f32),
        )
    };
    // One path per color.
    let mut colors: Vec<u8> = strokes.iter().map(|s| s.color).collect();
    colors.sort_unstable();
    colors.dedup();
    for color in colors {
        let mut path = PathBuilder::stroke(px(1.0));
        let mut any = false;
        let mut dots = Vec::new();
        for s in strokes.iter().filter(|s| s.color == color) {
            if !visible.get(s.layer).copied().unwrap_or(true)
                || s.max[0] < a[0]
                || s.min[0] > b[0]
                || s.max[1] < a[1]
                || s.min[1] > b[1]
            {
                continue;
            }
            if s.closed_dot {
                dots.extend(s.points.first().map(|p| at(*p)));
                continue;
            }
            let mut points = s.points.iter();
            if let Some(first) = points.next() {
                path.move_to(at(*first));
                for p in points {
                    path.line_to(at(*p));
                }
                any = true;
            }
        }
        let hsla = themed_color(color, foreground, dark);
        if any && let Ok(path) = path.build() {
            window.paint_path(path, hsla);
        }
        for dot in dots {
            let r = px(1.5);
            window.paint_quad(gpui::fill(
                Bounds::new(point(dot.x - r, dot.y - r), gpui::size(r * 2.0, r * 2.0)),
                hsla,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_texts(
    spec: &DrawingSpec,
    visible: &[bool],
    view: &Viewport,
    size: (f64, f64),
    bounds: Bounds<Pixels>,
    font: &Font,
    foreground: Hsla,
    dark: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let mut drawn = 0;
    for e in &spec.entities {
        let Shape::Text {
            at, height, text, ..
        } = &e.shape
        else {
            continue;
        };
        if !visible.get(e.layer).copied().unwrap_or(true) || drawn >= MAX_TEXTS {
            continue;
        }
        // A text height in pixels fits an f32.
        #[allow(clippy::cast_possible_truncation)]
        let font_px = (height * view.scale) as f32;
        if font_px < MIN_TEXT_PX {
            continue;
        }
        let font_px = font_px.min(400.0);
        let (x, y) = view.to_screen(*at, size);
        if x > size.0 || y < -f64::from(font_px) * 4.0 || y > size.1 + f64::from(font_px) * 40.0 {
            continue;
        }
        drawn += 1;
        let color = themed_color(spec.color_of(e), foreground, dark);
        for (line_no, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let run = TextRun {
                len: line.len(),
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(line.to_owned()),
                px(font_px),
                &[run],
                None,
            );
            // The insertion point is the baseline's left end; each further
            // line is a line height (1.4 × the text height) lower.
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let top = y as f32 - font_px + line_no as f32 * font_px * 1.4;
            #[allow(clippy::cast_possible_truncation)]
            let origin = point(bounds.origin.x + px(x as f32), bounds.origin.y + px(top));
            shaped
                .paint(origin, px(font_px * 1.2), TextAlign::Left, None, window, cx)
                .ok();
        }
    }
}

impl Render for DrawingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.canvas(window, cx);
        let rows = self.layer_rows(cx);
        let theme = cx.theme();
        let (muted, radius) = (theme.muted_foreground, theme.radius);
        let unit = self
            .spec
            .units
            .clone()
            .unwrap_or_else(|| "units".to_owned());
        let cursor = self
            .cursor
            .map(|p| format!("x {} · y {} {unit}", number(p[0]), number(p[1])));
        let warnings = self.spec.warnings.clone();
        let skipped: Vec<String> = self
            .spec
            .skipped
            .iter()
            .map(|s| format!("{} {}", s.count, s.kind))
            .collect();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "drawing".to_owned())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.spec.display_title()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "drawing-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(
                div()
                    .id("drawing-canvas")
                    .debug_selector(|| "drawing-canvas".to_owned())
                    .track_focus(&self.focus)
                    .relative()
                    .w_full()
                    .h(px(CANVAS_HEIGHT))
                    .overflow_hidden()
                    .rounded(radius)
                    .border_1()
                    .border_color(token(cx, "accent.bg"))
                    .cursor_crosshair()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_key_down(cx.listener(Self::key_down))
                    .child(canvas)
                    .child(
                        h_flex()
                            .absolute()
                            .top_1()
                            .right_1()
                            .gap_1()
                            .text_xs()
                            .child(tool_button("drawing-zoom-out", "−", cx).on_click(
                                cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, cx)),
                            ))
                            .child(tool_button("drawing-zoom-in", "+", cx).on_click(
                                cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, cx)),
                            ))
                            .child(
                                tool_button("drawing-fit", "fit", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                            ),
                    )
                    .when_some(cursor, |d, text| {
                        d.child(
                            div()
                                .absolute()
                                .left_1()
                                .bottom_1()
                                .text_xs()
                                .debug_selector(|| "drawing-cursor".to_owned())
                                .child(readout(text, cx)),
                        )
                    }),
            )
            .child(rows)
            .when(!skipped.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "drawing-skipped".to_owned())
                        .child(format!("Not drawn: {}", skipped.join(", "))),
                )
            })
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "drawing-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The drawing entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &DrawingSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<DrawingView> {
    let view = super::block_state(id, window, cx, |_, cx| DrawingView::new(spec, cx));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Drawing(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::drawing::{DrawEntity, DrawingLayer, PolyVertex, SkippedKind};
    use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> DrawingSpec {
        let corner = |x: f64, y: f64| PolyVertex { x, y, bulge: 0.0 };
        DrawingSpec {
            title: None,
            path: "dwg/plate.dxf".into(),
            version: Some("R2000".into()),
            units: Some("mm".into()),
            header_extents: None,
            layers: vec![
                DrawingLayer {
                    name: "Outline".into(),
                    color: 5,
                    off: false,
                    entities: 2,
                },
                DrawingLayer {
                    name: "Holes".into(),
                    color: 1,
                    off: true,
                    entities: 1,
                },
            ],
            entities: vec![
                DrawEntity {
                    layer: 0,
                    color: None,
                    shape: Shape::Polyline {
                        vertices: vec![
                            corner(0.0, 0.0),
                            corner(100.0, 0.0),
                            corner(100.0, 50.0),
                            corner(0.0, 50.0),
                        ],
                        closed: true,
                    },
                },
                DrawEntity {
                    layer: 0,
                    color: Some(7),
                    shape: Shape::Text {
                        at: [5.0, 5.0],
                        height: 5.0,
                        rotation: 0.0,
                        text: "PLATE\nS235".into(),
                    },
                },
                DrawEntity {
                    layer: 1,
                    color: None,
                    shape: Shape::Circle {
                        center: [150.0, 25.0],
                        radius: 3.0,
                    },
                },
            ],
            inserts: 0,
            skipped: vec![SkippedKind {
                kind: "hatch".into(),
                count: 2,
            }],
            truncated: false,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn viewports_fit_round_trip_and_zoom_about_a_point() {
        let x = Extents {
            min: [0.0, 0.0],
            max: [100.0, 50.0],
        };
        let size = (400.0, 400.0);
        let view = Viewport::fit(Some(x), size);
        assert!((view.scale - 3.6).abs() < 1e-9, "{view:?}");
        assert_eq!(view.to_screen([50.0, 25.0], size), (200.0, 200.0));
        let p = view.to_world((10.0, 30.0), size);
        let back = view.to_screen(p, size);
        assert!((back.0 - 10.0).abs() < 1e-9 && (back.1 - 30.0).abs() < 1e-9);
        // Y up: a higher world point is higher on screen.
        assert!(view.to_screen([0.0, 50.0], size).1 < view.to_screen([0.0, 0.0], size).1);
        let mut zoomed = view;
        let anchor = zoomed.to_world((100.0, 100.0), size);
        zoomed.zoom_about(2.0, (100.0, 100.0), size);
        let after = zoomed.to_world((100.0, 100.0), size);
        assert!((anchor[0] - after[0]).abs() < 1e-9 && (anchor[1] - after[1]).abs() < 1e-9);
        assert!((zoomed.scale - 7.2).abs() < 1e-9);
        let mut panned = view;
        panned.pan(36.0, 0.0);
        assert!((panned.center[0] - 40.0).abs() < 1e-9, "{panned:?}");
        assert_eq!(Viewport::fit(None, size).scale, 1.0);
    }

    #[test]
    fn colors_follow_the_theme() {
        let fg = gpui::black();
        assert_eq!(themed_color(7, fg, false), fg);
        let yellow_on_light = themed_color(2, fg, false);
        assert!(yellow_on_light.l < 0.4, "{yellow_on_light:?}");
        let yellow_on_dark = themed_color(2, fg, true);
        assert!((yellow_on_dark.l - 0.5).abs() < 0.01);
        assert!(
            themed_color(5, fg, true).l >= 0.65,
            "blue lightened on dark"
        );
        assert!(
            summary_text(&spec()).starts_with("R2000 · mm · geometry 153 × 50 mm · 3 entities"),
            "{}",
            summary_text(&spec())
        );
    }

    #[gpui::test]
    fn draws_the_drawing_toggles_layers_and_zooms(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (timeline, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Drawing(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
            div().size_full().child(timeline.clone())
        });
        cx.run_until_parked();
        for selector in [
            "drawing",
            "drawing-summary",
            "drawing-canvas",
            "drawing-layers",
            "drawing-layer-1",
            "drawing-skipped",
            "drawing-fit",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        // The holes layer is off in the file: the fit is the outline's.
        let fitted = view.read_with(cx, |v, _| v.viewport());
        assert!((fitted.center[0] - 50.0).abs() < 1e-9, "{fitted:?}");
        assert!(!view.read_with(cx, |v, _| v.layer_visible(1)));
        let row = cx.debug_bounds("drawing-layer-1").unwrap().center();
        cx.simulate_click(row, Modifiers::none());
        assert!(view.read_with(cx, |v, _| v.layer_visible(1)));
        view.update(cx, |v, cx| v.fit(cx));
        let refit = view.read_with(cx, |v, _| v.viewport());
        assert!(refit.center[0] > 70.0, "both layers now: {refit:?}");
        // Ctrl + wheel zooms and the cursor reads the drawing position.
        let canvas = cx.debug_bounds("drawing-canvas").unwrap();
        cx.simulate_mouse_move(canvas.center(), None, Modifiers::none());
        cx.simulate_event(ScrollWheelEvent {
            position: canvas.center(),
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        let plain = view.read_with(cx, |v, _| v.viewport());
        assert_eq!(plain, refit, "a plain wheel is the chat's");
        cx.simulate_event(ScrollWheelEvent {
            position: canvas.center(),
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers: Modifiers::control(),
            touch_phase: TouchPhase::Moved,
        });
        let zoomed = view.read_with(cx, |v, _| v.viewport());
        assert!(zoomed.scale > refit.scale);
        assert!(view.read_with(cx, |v, _| v.cursor()).is_some());
        assert!(cx.debug_bounds("drawing-cursor").is_some());
    }

    #[gpui::test]
    fn a_plain_wheel_goes_on_to_the_chat(cx: &mut TestAppContext) {
        use crate::blocks::wheel_host::{host, wheel};
        cx.update(gpui_component::init);
        let (timeline, wheels, cx) = host(cx, |_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Drawing(Box::new(spec())));
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let canvas = cx.debug_bounds("drawing-canvas").expect("canvas").center();
        wheel(cx, canvas, Modifiers::none());
        assert_eq!(wheels.get(), 1);
        wheel(cx, canvas, Modifiers::control());
        assert_eq!(wheels.get(), 1, "Ctrl + wheel stays in the drawing");
    }
}
