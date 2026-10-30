//! Renderer of the `waveform` block kind (`show_waveform`): digital
//! waves of a VCD, FST or GHW dump.
//!
//! A summary line comes first (format, timescale, time range, signal
//! count). Then one canvas: the signal tree in the left column (scope
//! rows, click to fold), the value of each signal at the cursor in the
//! next column, and the waves on a time axis. One-bit signals are drawn
//! as high/low traces (unknown `x` as a red band, high impedance `z` as a
//! middle line), buses, reals and text as value boxes with their value
//! written in the chosen radix when it fits. Changes closer than a pixel
//! merge into a dense block, so a long dump stays cheap. Ctrl/cmd +
//! wheel (or `+`/`-`) zooms around the pointer, a drag or shift + wheel
//! pans, a click in the waves puts the cursor there; Escape clears it.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::waveform::{Radix, SignalKind, WaveSignal, WaveformSpec, format_value};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, fill, point,
    px, size,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::trace::file_name;
use crate::blocks::chart::layout::{Align, Label};
use crate::blocks::chart::paint::LabelFont;
use crate::timeline::TimelineEntry;
use crate::widgets::timeline::axis::{Viewport, visible_rows};
use crate::widgets::{readout, tool_button};

/// Height of the axis band.
pub const AXIS_HEIGHT: f32 = 22.0;
/// Height of a row.
pub const ROW_HEIGHT: f32 = 22.0;
/// Tallest the rows get before they scroll.
pub const MAX_BODY_HEIGHT: f32 = 440.0;
/// Width of the value-at-cursor column.
pub const VALUE_WIDTH: f32 = 84.0;
/// Indentation per scope level.
pub const INDENT: f32 = 10.0;
/// Zoom step of the keys, buttons and a wheel notch.
pub const ZOOM_STEP: f64 = 1.5;
/// Pixels a press may move and still be a click.
const CLICK_SLOP: f32 = 3.0;
/// Pixels between axis ticks, about.
const TICK_SPACING: f32 = 100.0;

/// One row of the canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A scope header: its dotted path, its own name and depth.
    Scope {
        /// Dotted path.
        path: String,
        /// Last component.
        name: String,
        /// Nesting level (0 at the top).
        depth: usize,
    },
    /// A signal of the spec, by index, at a depth.
    Signal {
        /// Index into `spec.signals`.
        index: usize,
        /// Nesting level.
        depth: usize,
    },
}

/// The rows of `spec` with the scopes in `folded` closed: each scope
/// once, before its first signal, then its signals and inner scopes.
#[must_use]
pub fn rows(spec: &WaveformSpec, folded: &HashSet<String>) -> Vec<Row> {
    let mut out = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    for (index, signal) in spec.signals.iter().enumerate() {
        let parts: Vec<&str> = if signal.scope.is_empty() {
            Vec::new()
        } else {
            signal.scope.split('.').collect()
        };
        // Scopes already open that this signal shares.
        let common = open.iter().zip(&parts).take_while(|(a, b)| a == b).count();
        open.truncate(common);
        for (depth, name) in parts.iter().enumerate().skip(common) {
            open.push(name);
            let path = parts[..=depth].join(".");
            let hidden = (0..depth).any(|d| folded.contains(&parts[..=d].join(".")));
            if !hidden {
                out.push(Row::Scope {
                    path,
                    name: (*name).to_owned(),
                    depth,
                });
            }
        }
        let hidden = (0..parts.len()).any(|d| folded.contains(&parts[..=d].join(".")));
        if !hidden {
            out.push(Row::Signal {
                index,
                depth: parts.len(),
            });
        }
    }
    out
}

/// A tick step near `raw` from 1, 2, 5 × 10^k.
fn nice_step(raw: f64) -> f64 {
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let fraction = raw / magnitude;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    (nice * magnitude).max(1.0)
}

/// Tick times (in ticks of the dump) for `viewport` over `width` pixels.
#[must_use]
pub fn tick_times(viewport: Viewport, width: f32) -> Vec<f64> {
    let count = f64::from((width / TICK_SPACING).clamp(2.0, 12.0));
    let step = nice_step(viewport.span() / count);
    let first = (viewport.start / step).ceil();
    let last = (viewport.end / step).floor();
    if last - first > 200.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut k = first;
    while k <= last {
        out.push(k * step);
        k += 1.0;
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    last: Point<Pixels>,
    moved: f32,
}

/// Per-block state: the spec, folded scopes, the time window, the
/// vertical scroll, the cursor and the radix.
pub struct WaveformView {
    spec: Rc<WaveformSpec>,
    folded: HashSet<String>,
    rows: Rc<Vec<Row>>,
    extent: (f64, f64),
    viewport: Viewport,
    scroll: f32,
    cursor: Option<u64>,
    radix: Radix,
    drag: Option<Drag>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

/// Width of the name column in a canvas `width` wide.
fn gutter(width: f32) -> f32 {
    (width * 0.22).clamp(80.0, 220.0)
}

impl WaveformView {
    fn new(spec: &WaveformSpec, cx: &mut Context<Self>) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let extent = (spec.start as f64, (spec.end.max(spec.start + 1)) as f64);
        let folded = HashSet::new();
        Self {
            rows: Rc::new(rows(spec, &folded)),
            folded,
            spec: Rc::new(spec.clone()),
            extent,
            viewport: Viewport::fit(extent),
            scroll: 0.0,
            cursor: None,
            radix: spec.radix,
            drag: None,
            bounds: Rc::default(),
            focus: cx.focus_handle(),
        }
    }

    fn sync(&mut self, spec: &WaveformSpec, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            *self = Self {
                focus: self.focus.clone(),
                ..Self::new(spec, cx)
            };
            cx.notify();
        }
    }

    /// The spec drawn.
    pub fn spec(&self) -> &WaveformSpec {
        &self.spec
    }

    /// The rows shown.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// How buses are written.
    pub fn radix(&self) -> Radix {
        self.radix
    }

    /// Writes buses in `radix`.
    pub fn set_radix(&mut self, radix: Radix, cx: &mut Context<Self>) {
        self.radix = radix;
        cx.notify();
    }

    /// The cursor time, in ticks.
    pub fn cursor(&self) -> Option<u64> {
        self.cursor
    }

    /// Puts the cursor at `t` (clamped to the dump), or removes it.
    pub fn set_cursor(&mut self, t: Option<u64>, cx: &mut Context<Self>) {
        self.cursor = t.map(|t| t.clamp(self.spec.start, self.spec.end));
        cx.notify();
    }

    /// Each shown signal's path and value at the cursor.
    pub fn cursor_values(&self) -> Vec<(String, Option<String>)> {
        let Some(t) = self.cursor else {
            return Vec::new();
        };
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::Signal { index, .. } => self.spec.signals.get(*index),
                Row::Scope { .. } => None,
            })
            .map(|s| (s.path(), s.formatted_at(t, self.radix)))
            .collect()
    }

    /// Folds or unfolds the scope at `path`.
    pub fn toggle_scope(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.folded.remove(path) {
            self.folded.insert(path.to_owned());
        }
        self.rows = Rc::new(rows(&self.spec, &self.folded));
        self.scroll = self.scroll.min(self.max_scroll());
        cx.notify();
    }

    /// The visible time window, in ticks.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn data_width(&self) -> (f32, f32) {
        let width = f32::from(self.bounds.get().size.width);
        let left = gutter(width) + VALUE_WIDTH;
        (left, (width - left).max(1.0))
    }

    /// Zooms by `factor` (> 1 in) around `anchor` (window coordinates) or
    /// the middle.
    pub fn zoom_by(&mut self, factor: f64, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let origin = self.bounds.get().origin.x;
        let (left, data) = self.data_width();
        let fraction = anchor.map_or(0.5, |p| f64::from((f32::from(p.x - origin) - left) / data));
        self.viewport
            .zoom_at(factor, fraction.clamp(0.0, 1.0), self.extent);
        cx.notify();
    }

    /// The whole dump.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.viewport = Viewport::fit(self.extent);
        cx.notify();
    }

    /// Pans by `dx` pixels.
    pub fn pan_by(&mut self, dx: f32, cx: &mut Context<Self>) {
        let (_, data) = self.data_width();
        self.viewport.pan_by(dx, data, self.extent);
        cx.notify();
    }

    fn body_height(&self) -> f32 {
        (self.rows.len() as f32 * ROW_HEIGHT).clamp(ROW_HEIGHT, MAX_BODY_HEIGHT)
    }

    fn max_scroll(&self) -> f32 {
        (self.rows.len() as f32 * ROW_HEIGHT - self.body_height()).max(0.0)
    }

    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll());
        let moved = (next - self.scroll).abs() > f32::EPSILON;
        self.scroll = next;
        if moved {
            cx.notify();
        }
        moved
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(Drag {
            last: event.position,
            moved: 0.0,
        });
        window.focus(&self.focus, cx);
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let delta = event.position - drag.last;
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        drag.moved += dx.abs() + dy.abs();
        drag.last = event.position;
        if drag.moved > CLICK_SLOP {
            self.pan_by(dx, cx);
            self.scroll_by(-dy, cx);
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if drag.moved > CLICK_SLOP {
            return;
        }
        let bounds = self.bounds.get();
        let x = f32::from(event.position.x - bounds.origin.x);
        let y = f32::from(event.position.y - bounds.origin.y);
        let (left, data) = self.data_width();
        if x >= left {
            let t = self.viewport.t_of(x - left, data).round().max(0.0);
            // Times are non-negative and within the dump.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            self.set_cursor(Some(t as u64), cx);
        } else if y >= AXIS_HEIGHT {
            let ix = ((y - AXIS_HEIGHT + self.scroll) / ROW_HEIGHT).floor() as usize;
            if let Some(Row::Scope { path, .. }) = self.rows.get(ix).cloned() {
                self.toggle_scope(&path, cx);
            }
        }
        cx.stop_propagation();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(ROW_HEIGHT));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control || event.modifiers.platform {
            self.zoom_by(
                ZOOM_STEP.powf(f64::from(dy / ROW_HEIGHT)),
                Some(event.position),
                cx,
            );
            cx.stop_propagation();
        } else if event.modifiers.shift || dx.abs() > dy.abs() {
            let along = if dx.abs() > dy.abs() { dx } else { dy };
            self.pan_by(along, cx);
            cx.stop_propagation();
        } else if self.scroll_by(-dy, cx) {
            cx.stop_propagation();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event
            .keystroke
            .key_char
            .as_deref()
            .unwrap_or(event.keystroke.key.as_str());
        let step = f32::from(self.bounds.get().size.width) / 10.0;
        match key {
            "+" | "=" => self.zoom_by(ZOOM_STEP, None, cx),
            "-" => self.zoom_by(1.0 / ZOOM_STEP, None, cx),
            "0" => self.fit(cx),
            "left" => self.pan_by(step, cx),
            "right" => self.pan_by(-step, cx),
            "escape" if self.cursor.is_some() => self.set_cursor(None, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = self
            .spec
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_name(&self.spec.path).to_owned());
        let cursor = self.cursor.map_or_else(
            || "click to place the cursor".to_owned(),
            |t| format!("cursor {}", self.spec.time_text(t)),
        );
        let radix_button = |id: &'static str, radix: Radix, cx: &mut Context<Self>| {
            let label = if self.radix == radix {
                format!("[{}]", radix.label())
            } else {
                radix.label().to_owned()
            };
            tool_button(id, label, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.set_radix(radix, cx)))
        };
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
                    .debug_selector(|| "waveform-cursor".to_owned())
                    .child(readout(cursor, cx)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .child(radix_button("waveform-radix-hex", Radix::Hex, cx))
                    .child(radix_button("waveform-radix-bin", Radix::Bin, cx))
                    .child(radix_button("waveform-radix-dec", Radix::Dec, cx))
                    .child(tool_button("waveform-zoom-out", "−", cx).on_click(
                        cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, None, cx)),
                    ))
                    .child(
                        tool_button("waveform-zoom-in", "+", cx).on_click(
                            cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, None, cx)),
                        ),
                    )
                    .child(
                        tool_button("waveform-fit", "fit", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                    ),
            )
            .into_any_element()
    }
}

/// The summary text: format, timescale, range, signals.
#[must_use]
pub fn summary_text(spec: &WaveformSpec) -> String {
    let mut text = format!(
        "{} · timescale {} · {} – {} · {} of {} signals",
        spec.format.label(),
        spec.timescale.label(),
        spec.time_text(spec.start),
        spec.time_text(spec.end),
        spec.signals.len(),
        spec.total_signals
    );
    if spec.truncated() {
        text.push_str(" · some signals cut short");
    }
    text
}

#[derive(Clone)]
struct Colors {
    wave: Hsla,
    bus: Hsla,
    unknown: Hsla,
    high_z: Hsla,
    foreground: Hsla,
    muted: Hsla,
    grid: Hsla,
    band: Hsla,
    cursor: Hsla,
}

struct Frame {
    spec: Rc<WaveformSpec>,
    rows: Rc<Vec<Row>>,
    folded: HashSet<String>,
    viewport: Viewport,
    scroll: f32,
    body: f32,
    cursor: Option<u64>,
    radix: Radix,
    colors: Colors,
    font: LabelFont,
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

impl Frame {
    fn label(&self, text: &str, x: f32, y: f32, color: Hsla, window: &mut Window, cx: &mut App) {
        let label = Label {
            text: text.to_owned(),
            x,
            y,
            align: Align::Left,
        };
        self.font
            .paint(&label, color, point(px(0.), px(0.)), window, cx);
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let width = f32::from(bounds.size.width);
        let names = gutter(width);
        let left = names + VALUE_WIDTH;
        let (data_x, data_w) = (ox + left, (width - left).max(1.0));
        let body_y = oy + AXIS_HEIGHT;
        let c = &self.colors;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for at in tick_times(self.viewport, data_w) {
                let x = (data_x + self.viewport.x_of(at, data_w)).round();
                window.paint_quad(fill(
                    rect(x, body_y - 5.0, 1.0, self.body + 5.0),
                    c.grid.opacity(0.5),
                ));
                let text = self.spec.timescale.format(at);
                if x + 3.0 + self.font.measure(&text, window) <= ox + width {
                    self.label(&text, x + 3.0, oy + 3.0, c.muted, window, cx);
                }
            }
            self.label("value", ox + names + 6.0, oy + 3.0, c.muted, window, cx);
            window.paint_quad(fill(rect(ox, body_y - 1.0, width, 1.0), c.grid));
            let body = rect(ox, body_y, width, self.body);
            window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
                for ix in visible_rows(self.scroll, self.body, ROW_HEIGHT, self.rows.len()) {
                    let y = body_y + ix as f32 * ROW_HEIGHT - self.scroll;
                    if ix % 2 == 1 {
                        window.paint_quad(fill(rect(ox, y, width, ROW_HEIGHT), c.band));
                    }
                    match &self.rows[ix] {
                        Row::Scope { path, name, depth } => {
                            let mark = if self.folded.contains(path) {
                                "▸"
                            } else {
                                "▾"
                            };
                            let text = format!("{mark} {name}");
                            let x = ox + 4.0 + *depth as f32 * INDENT;
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: rect(ox, y, names - 2.0, ROW_HEIGHT),
                                }),
                                |window| self.label(&text, x, y + 3.0, c.muted, window, cx),
                            );
                        }
                        Row::Signal { index, depth } => {
                            if let Some(signal) = self.spec.signals.get(*index) {
                                self.paint_signal(
                                    signal,
                                    *depth,
                                    y,
                                    (ox, names, data_x, data_w),
                                    window,
                                    cx,
                                );
                            }
                        }
                    }
                }
                window.paint_quad(fill(rect(ox + names, body_y, 1.0, self.body), c.grid));
                window.paint_quad(fill(rect(data_x - 1.0, body_y, 1.0, self.body), c.grid));
                let total = self.rows.len() as f32 * ROW_HEIGHT;
                if total > self.body {
                    let h = (self.body * self.body / total).max(12.0);
                    let y = body_y + (self.body - h) * self.scroll / (total - self.body);
                    window.paint_quad(
                        fill(rect(ox + width - 4.0, y, 3.0, h), c.muted.opacity(0.5))
                            .corner_radii(px(1.5)),
                    );
                }
            });
            if let Some(t) = self.cursor {
                #[allow(clippy::cast_precision_loss)]
                let x = data_x + self.viewport.x_of(t as f64, data_w);
                if x >= data_x && x <= data_x + data_w {
                    window.paint_quad(fill(
                        rect(x - 0.75, oy, 1.5, AXIS_HEIGHT + self.body),
                        c.cursor,
                    ));
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_signal(
        &self,
        signal: &WaveSignal,
        depth: usize,
        y: f32,
        (ox, names, data_x, data_w): (f32, f32, f32, f32),
        window: &mut Window,
        cx: &mut App,
    ) {
        let c = &self.colors;
        let name_x = ox + 4.0 + depth as f32 * INDENT;
        window.with_content_mask(
            Some(ContentMask {
                bounds: rect(ox, y, names - 2.0, ROW_HEIGHT),
            }),
            |window| self.label(&signal.name, name_x, y + 3.0, c.foreground, window, cx),
        );
        if let Some(value) = self.cursor.and_then(|t| signal.formatted_at(t, self.radix)) {
            window.with_content_mask(
                Some(ContentMask {
                    bounds: rect(ox + names + 1.0, y, VALUE_WIDTH - 3.0, ROW_HEIGHT),
                }),
                |window| self.label(&value, ox + names + 6.0, y + 3.0, c.foreground, window, cx),
            );
        }
        let area = rect(data_x, y, data_w, ROW_HEIGHT);
        let (top, bottom) = (y + 4.0, y + ROW_HEIGHT - 4.0);
        let line = 1.5;
        window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
            let end_x = data_x + data_w;
            #[allow(clippy::cast_precision_loss)]
            let x_of = |t: u64| data_x + self.viewport.x_of(t as f64, data_w);
            #[allow(clippy::cast_precision_loss)]
            let last_t = self.spec.end as f64;
            let changes = &signal.changes;
            // First change in view (or the one in force at its start).
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let first = signal
                .index_at(self.viewport.start.max(0.0) as u64)
                .unwrap_or(0);
            let mut dense_to = f32::NEG_INFINITY;
            for i in first..changes.len() {
                let x0 = x_of(changes[i].t);
                if x0 > end_x + 1.0 {
                    break;
                }
                let x1 = changes
                    .get(i + 1)
                    .map_or(data_x + self.viewport.x_of(last_t, data_w), |n| x_of(n.t));
                let (a, b) = (x0.max(data_x - 2.0), x1.min(end_x + 2.0));
                if b < data_x {
                    continue;
                }
                let v = changes[i].v.as_str();
                if b - a < 1.5 {
                    // Too narrow to read: one dense block per pixel.
                    if a >= dense_to {
                        window
                            .paint_quad(fill(rect(a, top, 1.0, bottom - top), c.wave.opacity(0.6)));
                        dense_to = a + 1.0;
                    }
                    continue;
                }
                match signal.kind {
                    SignalKind::Bit => {
                        let level = match v {
                            "1" | "H" | "h" => Some(top),
                            "0" | "L" | "l" => Some(bottom - line),
                            _ => None,
                        };
                        match level {
                            Some(ly) => window.paint_quad(fill(rect(a, ly, b - a, line), c.wave)),
                            None if v.eq_ignore_ascii_case("z") => window.paint_quad(fill(
                                rect(a, (top + bottom) / 2.0, b - a, line),
                                c.high_z,
                            )),
                            None => window.paint_quad(fill(
                                rect(a, top, b - a, bottom - top),
                                c.unknown.opacity(0.35),
                            )),
                        }
                        if i > first || x0 >= data_x {
                            window.paint_quad(fill(
                                rect(x0 - line / 2.0, top, line, bottom - top),
                                c.wave,
                            ));
                        }
                    }
                    SignalKind::Event => {
                        window.paint_quad(fill(rect(x0 - 0.75, top, 1.5, bottom - top), c.wave));
                    }
                    _ => {
                        let unknown = matches!(signal.kind, SignalKind::Vector)
                            && v.chars().any(|ch| ch != '0' && ch != '1');
                        let color = if unknown { c.unknown } else { c.bus };
                        if unknown {
                            window.paint_quad(fill(
                                rect(a, top, b - a, bottom - top),
                                c.unknown.opacity(0.18),
                            ));
                        }
                        window.paint_quad(fill(
                            rect(a + 1.0, top, (b - a - 2.0).max(0.0), 1.0),
                            color,
                        ));
                        window.paint_quad(fill(
                            rect(a + 1.0, bottom - 1.0, (b - a - 2.0).max(0.0), 1.0),
                            color,
                        ));
                        window.paint_quad(fill(rect(x0 - 0.5, top, 1.0, bottom - top), color));
                        let text = format_value(v, signal.kind, self.radix);
                        let room = b.min(end_x) - a.max(data_x) - 6.0;
                        if room > 8.0 && self.font.measure(&text, window) <= room {
                            self.label(
                                &text,
                                a.max(data_x) + 4.0,
                                y + 3.0,
                                c.foreground,
                                window,
                                cx,
                            );
                        }
                    }
                }
            }
        });
    }
}

impl Render for WaveformView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let body = self.body_height();
        let frame = Frame {
            spec: self.spec.clone(),
            rows: self.rows.clone(),
            folded: self.folded.clone(),
            viewport: self.viewport,
            scroll: self.scroll,
            body,
            cursor: self.cursor,
            radix: self.radix,
            colors: Colors {
                wave: theme.success,
                bus: crate::widgets::palette(0, cx),
                unknown: theme.danger,
                high_z: theme.warning,
                foreground: theme.foreground,
                muted: theme.muted_foreground,
                grid: theme.border,
                band: theme.muted.opacity(0.35),
                cursor: theme.info,
            },
            font: LabelFont {
                font: window.text_style().font(),
                size: window.rem_size() * 0.75,
            },
        };
        let bounds_cell = self.bounds.clone();
        let surface = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, cx| frame.paint(bounds, window, cx),
        )
        .size_full();
        let canvas = div()
            .id("waveform-canvas")
            .debug_selector(|| "waveform-canvas".to_owned())
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .h(px(AXIS_HEIGHT + body + 2.0))
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .cursor_crosshair()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_key_down(cx.listener(Self::key_down))
            .child(surface);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "waveform".to_owned())
            .child(self.header(cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .debug_selector(|| "waveform-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(canvas)
    }
}

/// The waveform entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &WaveformSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<WaveformView> {
    let view = super::block_state(id, window, cx, |_, cx| WaveformView::new(spec, cx));
    let focus = view.read(cx).focus.clone();
    crate::blocks::remember_focus(id, &focus, cx);
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Waveform(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::waveform::{Timescale, WaveChange, WaveFormat};
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn signal(scope: &str, name: &str, kind: SignalKind, changes: &[(u64, &str)]) -> WaveSignal {
        WaveSignal {
            name: name.into(),
            scope: scope.into(),
            kind,
            width: if kind == SignalKind::Bit { 1 } else { 8 },
            changes: changes
                .iter()
                .map(|(t, v)| WaveChange {
                    t: *t,
                    v: (*v).into(),
                })
                .collect(),
            truncated: false,
        }
    }

    fn spec() -> WaveformSpec {
        WaveformSpec {
            title: None,
            path: "sim/tb.vcd".into(),
            format: WaveFormat::Vcd,
            timescale: Timescale::default(),
            start: 0,
            end: 40,
            signals: vec![
                signal(
                    "tb",
                    "clk",
                    SignalKind::Bit,
                    &[(0, "0"), (5, "1"), (10, "0")],
                ),
                signal(
                    "tb.dut",
                    "q[7:0]",
                    SignalKind::Vector,
                    &[(0, "00000000"), (20, "00011111")],
                ),
                signal("tb.dut", "z", SignalKind::Bit, &[(0, "x"), (30, "z")]),
            ],
            total_signals: 3,
            radix: Radix::Hex,
            generator: None,
        }
    }

    #[test]
    fn rows_put_each_scope_before_its_signals_and_fold() {
        let s = spec();
        let all = rows(&s, &HashSet::new());
        assert_eq!(all.len(), 5);
        assert!(matches!(&all[0], Row::Scope { path, depth: 0, .. } if path == "tb"));
        assert!(matches!(all[1], Row::Signal { index: 0, depth: 1 }));
        assert!(matches!(&all[2], Row::Scope { path, depth: 1, .. } if path == "tb.dut"));
        let folded: HashSet<String> = ["tb.dut".to_owned()].into();
        assert_eq!(rows(&s, &folded).len(), 3);
        let top: HashSet<String> = ["tb".to_owned()].into();
        assert_eq!(rows(&s, &top).len(), 1);
    }

    #[test]
    fn ticks_are_round_steps() {
        let v = Viewport {
            start: 0.0,
            end: 1000.0,
        };
        let t = tick_times(v, 500.0);
        assert_eq!(t.first(), Some(&0.0));
        assert!(t.windows(2).all(|w| (w[1] - w[0] - 200.0).abs() < 1e-9));
        assert_eq!(
            summary_text(&spec()),
            "VCD · timescale 1 ns · 0 – 40 ns · 3 of 3 signals"
        );
    }

    #[gpui::test]
    fn draws_and_reads_values_at_the_cursor(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Waveform(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(800.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "waveform",
            "waveform-summary",
            "waveform-canvas",
            "waveform-cursor",
            "waveform-radix-dec",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let wave = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        wave.update(cx, |w, cx| w.set_cursor(Some(25), cx));
        let values = wave.read_with(cx, |w, _| w.cursor_values());
        assert_eq!(
            values,
            [
                ("tb.clk".to_owned(), Some("0".to_owned())),
                ("tb.dut.q[7:0]".to_owned(), Some("1f".to_owned())),
                ("tb.dut.z".to_owned(), Some("x".to_owned())),
            ]
        );
        wave.update(cx, |w, cx| w.set_radix(Radix::Dec, cx));
        let values = wave.read_with(cx, |w, _| w.cursor_values());
        assert_eq!(values[1].1.as_deref(), Some("31"));
        wave.update(cx, |w, cx| {
            w.toggle_scope("tb.dut", cx);
            w.zoom_by(2.0, None, cx);
        });
        assert_eq!(wave.read_with(cx, |w, _| w.rows().len()), 3);
        assert!(wave.read_with(cx, |w, _| w.viewport().span()) < 40.0);
    }
}
