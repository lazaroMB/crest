//! The timeline widget: rows of spans on a time axis.
//!
//! One GPUI canvas paints the axis, the label column (indented by depth)
//! and the bars of the rows in view only, so thousands of rows stay
//! cheap. Ctrl/cmd + wheel (or `+`/`-` and the toolbar) zooms around the
//! cursor, a drag or shift + wheel pans in time, and a plain wheel scrolls
//! the rows while there are more to see (then the chat scrolls). Hovering
//! a bar shows its label, times, duration and attributes; a click selects
//! it, outlines it, shows it in the detail line below and emits
//! [`TimelineEvent::Selected`] for a parent with a detail pane of its own.
//! Bars take a palette color by `color_key`, and the theme's danger or
//! warning color by status (a Gantt chart's critical tasks take danger,
//! its slack a faint muted bar).
//!
//! [`view`] gives the entity of an id (to subscribe or drive it) and
//! [`render`] the whole widget with its header.

pub mod axis;

use std::cell::Cell;
use std::rc::Rc;

use domain::block::BlockId;
use domain::widget::timeline::{
    Marker, Span, SpanStatus, TimelineSpec, format_duration, format_time,
};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, EventEmitter, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, fill, point,
    prelude::FluentBuilder as _, px, quad, size,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use self::axis::{Viewport, color_slot, hit_span, ticks, visible_rows};
use crate::blocks::chart::layout::{Align, Label};
use crate::blocks::chart::paint::LabelFont;

/// Height of the axis band.
pub const AXIS_HEIGHT: f32 = 22.0;
/// Height of a row.
pub const ROW_HEIGHT: f32 = 20.0;
/// Tallest the body (rows) gets before it scrolls.
pub const MAX_BODY_HEIGHT: f32 = 360.0;
/// Widest the label column gets.
pub const LABEL_WIDTH: f32 = 200.0;
/// Indentation per depth level.
pub const INDENT: f32 = 12.0;
/// Zoom step of the keys, the buttons and one wheel notch.
pub const ZOOM_STEP: f64 = 1.5;
/// Pixels a press may move and still be a click.
const CLICK_SLOP: f32 = 3.0;
/// Pixels around a bar or marker that still hit it.
const HIT_SLOP: f32 = 2.0;
/// Attributes listed in the tooltip.
const TOOLTIP_ATTRS: usize = 8;
/// Width of the tooltip.
const TOOLTIP_WIDTH: f32 = 280.0;

/// A span of the spec: row index and span index within the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanRef {
    /// Row index.
    pub row: usize,
    /// Span index within the row.
    pub span: usize,
}

/// What the timeline tells its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineEvent {
    /// A click selected a span, or cleared the selection.
    Selected(Option<SpanRef>),
}

/// What is under the cursor.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Span(usize),
    Marker(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Hover {
    row: usize,
    item: Item,
    /// Cursor relative to the canvas origin.
    at: (f32, f32),
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    last: Point<Pixels>,
    moved: f32,
}

/// Per-id state of a timeline: the spec, the visible time window, the
/// vertical scroll and the hovered and selected span.
pub struct TimelineView {
    spec: Rc<TimelineSpec>,
    /// Band parity of each row (consecutive rows of a group share one).
    bands: Rc<[bool]>,
    extent: (f64, f64),
    viewport: Viewport,
    scroll: f32,
    hover: Option<Hover>,
    selected: Option<SpanRef>,
    drag: Option<Drag>,
    show_details: bool,
    /// Canvas bounds of the last paint.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

impl EventEmitter<TimelineEvent> for TimelineView {}

/// Band parity per row: rows alternate, or groups alternate when rows
/// have groups.
fn bands(spec: &TimelineSpec) -> Rc<[bool]> {
    let grouped = spec.rows.iter().any(|r| r.group.is_some());
    let mut parity = false;
    spec.rows
        .iter()
        .enumerate()
        .map(|(ix, row)| {
            if grouped {
                if ix > 0 && spec.rows[ix - 1].group != row.group {
                    parity = !parity;
                }
                parity
            } else {
                ix % 2 == 1
            }
        })
        .collect()
}

/// Width of the label column in a canvas `width` wide.
fn gutter(width: f32) -> f32 {
    (width * 0.3).clamp(60.0, LABEL_WIDTH)
}

impl TimelineView {
    fn new(spec: &TimelineSpec, cx: &mut Context<Self>) -> Self {
        let extent = spec.extent();
        Self {
            bands: bands(spec),
            spec: Rc::new(spec.clone()),
            extent,
            viewport: Viewport::fit(extent),
            scroll: 0.0,
            hover: None,
            selected: None,
            drag: None,
            show_details: true,
            bounds: Rc::default(),
            focus: cx.focus_handle(),
        }
    }

    fn sync(&mut self, spec: &TimelineSpec, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            *self = Self {
                focus: self.focus.clone(),
                show_details: self.show_details,
                ..Self::new(spec, cx)
            };
            cx.notify();
        }
    }

    /// The spec drawn.
    pub fn spec(&self) -> &TimelineSpec {
        &self.spec
    }

    /// The visible window of time.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// How far in the view is (1 = the whole extent).
    pub fn zoom(&self) -> f64 {
        self.viewport.zoom(self.extent)
    }

    /// Pixels the rows are scrolled down by.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// The selected span.
    pub fn selected(&self) -> Option<SpanRef> {
        self.selected
    }

    /// The span `at` refers to, if it exists.
    pub fn span(&self, at: SpanRef) -> Option<&Span> {
        self.spec.rows.get(at.row)?.spans.get(at.span)
    }

    /// Whether the built-in detail line shows the selection (on by
    /// default; off when the parent has a detail pane of its own).
    pub fn set_show_details(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_details = show;
        cx.notify();
    }

    /// Selects `span` (or nothing) and tells the parent.
    pub fn select(&mut self, span: Option<SpanRef>, cx: &mut Context<Self>) {
        let span = span.filter(|s| self.span(*s).is_some());
        if self.selected != span {
            self.selected = span;
            cx.emit(TimelineEvent::Selected(span));
            cx.notify();
        }
    }

    /// Zooms by `factor` (> 1 in) around `anchor` (window coordinates),
    /// or the middle of the data area.
    pub fn zoom_by(&mut self, factor: f64, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let bounds = self.bounds.get();
        let width = f32::from(bounds.size.width);
        let left = gutter(width);
        let data = (width - left).max(1.0);
        let fraction = anchor.map_or(0.5, |p| {
            f64::from((f32::from(p.x - bounds.origin.x) - left) / data)
        });
        self.viewport.zoom_at(factor, fraction, self.extent);
        cx.notify();
    }

    /// Back to the whole extent.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.viewport = Viewport::fit(self.extent);
        cx.notify();
    }

    /// Pans by `dx` pixels in time.
    pub fn pan_by(&mut self, dx: f32, cx: &mut Context<Self>) {
        let width = f32::from(self.bounds.get().size.width);
        let data = (width - gutter(width)).max(1.0);
        self.viewport.pan_by(dx, data, self.extent);
        cx.notify();
    }

    fn body_height(&self) -> f32 {
        (self.spec.rows.len() as f32 * ROW_HEIGHT).clamp(ROW_HEIGHT, MAX_BODY_HEIGHT)
    }

    fn max_scroll(&self) -> f32 {
        (self.spec.rows.len() as f32 * ROW_HEIGHT - self.body_height()).max(0.0)
    }

    /// Scrolls the rows by `dy` pixels; whether it moved.
    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll());
        let moved = (next - self.scroll).abs() > f32::EPSILON;
        self.scroll = next;
        if moved {
            cx.notify();
        }
        moved
    }

    /// The row and item under `position` (window coordinates).
    fn locate(&self, position: Point<Pixels>) -> Option<(usize, Option<Item>, (f32, f32))> {
        let bounds = self.bounds.get();
        let (x, y) = (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        );
        let width = f32::from(bounds.size.width);
        if x < 0.0 || x > width || y < AXIS_HEIGHT || y > AXIS_HEIGHT + self.body_height() {
            return None;
        }
        let row = ((y - AXIS_HEIGHT + self.scroll) / ROW_HEIGHT).floor() as usize;
        let spec_row = self.spec.rows.get(row)?;
        let left = gutter(width);
        if x < left {
            return Some((row, None, (x, y)));
        }
        let (dx, data) = (x - left, width - left);
        let marker = spec_row
            .markers
            .iter()
            .position(|m| (self.viewport.x_of(m.at, data) - dx).abs() <= HIT_SLOP + 2.0);
        let item = match marker {
            Some(ix) => Some(Item::Marker(ix)),
            None => hit_span(&spec_row.spans, self.viewport, data, dx, HIT_SLOP).map(Item::Span),
        };
        Some((row, item, (x, y)))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(Drag {
            last: event.position,
            moved: 0.0,
        });
        window.focus(&self.focus, cx);
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
                    self.hover = None;
                    self.pan_by(dx, cx);
                    self.scroll_by(-dy, cx);
                }
                return;
            }
            self.drag = None;
        }
        let hover = self
            .locate(event.position)
            .and_then(|(row, item, at)| item.map(|item| Hover { row, item, at }));
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
        let span = match self.locate(event.position) {
            Some((row, Some(Item::Span(span)), _)) => Some(SpanRef { row, span }),
            _ => None,
        };
        self.select(span, cx);
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
            "escape" if self.selected.is_some() => self.select(None, cx),
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
            .unwrap_or("Timeline");
        let hint = format!(
            "{} rows · {} spans · drag to pan · ctrl+wheel to zoom",
            self.spec.rows.len(),
            self.spec.span_count()
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
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
            .child(self.toolbar(cx))
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_1()
            .text_xs()
            .child(super::readout(format!("×{}", zoom_text(self.zoom())), cx))
            .child(
                super::tool_button("timeline-zoom-out", "−", cx).on_click(
                    cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, None, cx)),
                ),
            )
            .child(
                super::tool_button("timeline-zoom-in", "+", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, None, cx))),
            )
            .child(
                super::tool_button("timeline-fit", "fit", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .into_any_element()
    }

    fn tooltip(&self, hover: Hover, cx: &App) -> Option<AnyElement> {
        let row = self.spec.rows.get(hover.row)?;
        let theme = cx.theme();
        let unit = self.spec.unit;
        let mut lines: Vec<(SharedString, Option<Hsla>)> = Vec::new();
        match hover.item {
            Item::Span(ix) => {
                let span = row.spans.get(ix)?;
                lines.push((
                    span.label
                        .clone()
                        .unwrap_or_else(|| row.label.clone())
                        .into(),
                    None,
                ));
                lines.push((
                    format!(
                        "{} · {} → {}",
                        format_duration(span.duration(), unit),
                        format_time(span.start, unit),
                        format_time(span.end, unit)
                    )
                    .into(),
                    Some(theme.muted_foreground),
                ));
                if let Some(status) = span.status {
                    lines.push((status.name().into(), Some(status_color(status, cx))));
                }
                for (key, value) in span.attrs.iter().take(TOOLTIP_ATTRS) {
                    lines.push((
                        format!("{key}: {}", value_text(value)).into(),
                        Some(theme.muted_foreground),
                    ));
                }
                if span.attrs.len() > TOOLTIP_ATTRS {
                    lines.push((
                        format!("… {} more", span.attrs.len() - TOOLTIP_ATTRS).into(),
                        Some(theme.muted_foreground),
                    ));
                }
            }
            Item::Marker(ix) => {
                let marker = row.markers.get(ix)?;
                lines.push((
                    marker
                        .label
                        .clone()
                        .unwrap_or_else(|| "marker".into())
                        .into(),
                    None,
                ));
                lines.push((
                    format_time(marker.at, unit).into(),
                    Some(theme.muted_foreground),
                ));
            }
        }
        if let Some(group) = &row.group {
            lines.push((
                format!("group: {group}").into(),
                Some(theme.muted_foreground),
            ));
        }
        let width = f32::from(self.bounds.get().size.width);
        let left = if hover.at.0 + 14.0 + TOOLTIP_WIDTH > width {
            (hover.at.0 - 14.0 - TOOLTIP_WIDTH).max(0.0)
        } else {
            hover.at.0 + 14.0
        };
        let first = lines.first().cloned();
        Some(
            v_flex()
                .absolute()
                .left(px(left))
                .top(px(hover.at.1 + 14.0))
                .w(px(TOOLTIP_WIDTH))
                .p_1p5()
                .gap_0p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .text_xs()
                .debug_selector(|| "timeline-tooltip".to_owned())
                .children(
                    first.map(|(text, _)| div().font_weight(FontWeight::SEMIBOLD).child(text)),
                )
                .children(lines.into_iter().skip(1).map(|(text, color)| {
                    div().when_some(color, |d, c| d.text_color(c)).child(text)
                }))
                .into_any_element(),
        )
    }

    fn details(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_details {
            return None;
        }
        let at = self.selected?;
        let row = self.spec.rows.get(at.row)?;
        let span = row.spans.get(at.span)?;
        let theme = cx.theme();
        let unit = self.spec.unit;
        let mut head = format!(
            "{} · {} · {} → {}",
            span.label.as_deref().unwrap_or(&row.label),
            format_duration(span.duration(), unit),
            format_time(span.start, unit),
            format_time(span.end, unit),
        );
        if span.label.is_some() {
            head = format!("{} · {head}", row.label);
        }
        if let Some(status) = span.status {
            head.push_str(" · ");
            head.push_str(status.name());
        }
        let attrs = span
            .attrs
            .iter()
            .map(|(k, v)| format!("{k}: {}", value_text(v)))
            .collect::<Vec<_>>()
            .join("   ");
        Some(
            v_flex()
                .debug_selector(|| "timeline-details".to_owned())
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
                                .id("timeline-clear")
                                .debug_selector(|| "timeline-clear".to_owned())
                                .cursor_pointer()
                                .text_color(theme.muted_foreground)
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.select(None, cx);
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

/// A zoom factor for the readout: `1`, `2.5`, `1.2K`.
fn zoom_text(zoom: f64) -> String {
    crate::blocks::chart::scale::format_compact(zoom)
}

/// A JSON value as plain text: strings without quotes.
pub fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Theme color of a span status.
pub fn status_color(status: SpanStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        SpanStatus::Ok => theme.success,
        SpanStatus::Warning => theme.warning,
        SpanStatus::Error | SpanStatus::Critical => theme.danger,
        SpanStatus::Slack => theme.muted_foreground,
    }
}

/// Colors the canvas paints with, read from the theme at render time.
#[derive(Clone)]
struct Colors {
    palette: [Hsla; 5],
    danger: Hsla,
    warning: Hsla,
    foreground: Hsla,
    muted: Hsla,
    band: Hsla,
    grid: Hsla,
    selection: Hsla,
}

impl Colors {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            palette: [0, 1, 2, 3, 4].map(|ix| super::palette(ix, cx)),
            danger: theme.danger,
            warning: theme.warning,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            band: theme.muted.opacity(0.45),
            grid: theme.border,
            selection: theme.foreground,
        }
    }

    fn of_key(&self, key: Option<&str>) -> Hsla {
        self.palette[key.map_or(0, color_slot)]
    }

    fn of_span(&self, span: &Span) -> Hsla {
        match span.status {
            Some(SpanStatus::Error | SpanStatus::Critical) => self.danger,
            Some(SpanStatus::Slack) => self.muted.opacity(0.35),
            Some(SpanStatus::Warning) => self.warning,
            _ => self.of_key(span.color_key.as_deref()),
        }
    }

    fn of_marker(&self, marker: &Marker) -> Hsla {
        marker
            .color_key
            .as_deref()
            .map_or(self.foreground, |key| self.of_key(Some(key)))
    }
}

/// Everything one paint needs.
struct Frame {
    spec: Rc<TimelineSpec>,
    bands: Rc<[bool]>,
    viewport: Viewport,
    scroll: f32,
    body: f32,
    selected: Option<SpanRef>,
    hovered: Option<(usize, Item)>,
    colors: Colors,
    font: LabelFont,
}

fn bounds_at(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

impl Frame {
    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let width = f32::from(bounds.size.width);
        let left = gutter(width);
        let (data_x, data_w) = (ox + left, (width - left).max(1.0));
        let body_y = oy + AXIS_HEIGHT;
        let colors = &self.colors;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Axis: grid lines through the body and labels in the band.
            for tick in ticks(self.viewport, self.spec.unit, data_w) {
                let x = (data_x + self.viewport.x_of(tick.at, data_w)).round();
                window.paint_quad(fill(
                    bounds_at(x, oy + AXIS_HEIGHT - 5.0, 1.0, self.body + 5.0),
                    colors.grid.opacity(0.6),
                ));
                let label = Label {
                    text: tick.label,
                    x: x + 3.0,
                    y: oy + 2.0,
                    align: Align::Left,
                };
                if label.x + self.font.measure(&label.text, window) <= ox + width {
                    self.font
                        .paint(&label, colors.muted, point(px(0.), px(0.)), window, cx);
                }
            }
            window.paint_quad(fill(bounds_at(ox, body_y - 1.0, width, 1.0), colors.grid));
            let body = bounds_at(ox, body_y, width, self.body);
            window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
                let rows = visible_rows(self.scroll, self.body, ROW_HEIGHT, self.spec.rows.len());
                for ix in rows {
                    let y = body_y + ix as f32 * ROW_HEIGHT - self.scroll;
                    self.paint_row(ix, y, (ox, left, data_x, data_w), window, cx);
                }
                // The label column's edge.
                window.paint_quad(fill(
                    bounds_at(data_x - 1.0, body_y, 1.0, self.body),
                    colors.grid,
                ));
                // A scroll thumb when the rows do not fit.
                let total = self.spec.rows.len() as f32 * ROW_HEIGHT;
                if total > self.body {
                    let h = (self.body * self.body / total).max(12.0);
                    let y = body_y + (self.body - h) * self.scroll / (total - self.body);
                    window.paint_quad(
                        fill(
                            bounds_at(ox + width - 4.0, y, 3.0, h),
                            colors.muted.opacity(0.5),
                        )
                        .corner_radii(px(1.5)),
                    );
                }
            });
        });
    }

    fn paint_row(
        &self,
        ix: usize,
        y: f32,
        (ox, left, data_x, data_w): (f32, f32, f32, f32),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(row) = self.spec.rows.get(ix) else {
            return;
        };
        let colors = &self.colors;
        if self.bands.get(ix).copied().unwrap_or_default() {
            window.paint_quad(fill(
                bounds_at(ox, y, left + data_w, ROW_HEIGHT),
                colors.band,
            ));
        }
        if ix > 0 && row.group.is_some() && self.spec.rows[ix - 1].group != row.group {
            window.paint_quad(fill(bounds_at(ox, y, left + data_w, 1.0), colors.grid));
        }
        // The label, indented, cut at the column's edge.
        let failed = row
            .spans
            .iter()
            .any(|s| matches!(s.status, Some(SpanStatus::Error | SpanStatus::Critical)));
        let label = Label {
            text: row.label.clone(),
            x: ox + 6.0 + row.depth as f32 * INDENT,
            y: y + 2.0,
            align: Align::Left,
        };
        let column = bounds_at(ox, y, left - 4.0, ROW_HEIGHT);
        window.with_content_mask(Some(ContentMask { bounds: column }), |window| {
            let color = if failed {
                colors.danger
            } else {
                colors.foreground
            };
            self.font
                .paint(&label, color, point(px(0.), px(0.)), window, cx);
        });
        let area = bounds_at(data_x, y, data_w, ROW_HEIGHT);
        window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
            // Bars no wider than the pixel already painted are skipped, so
            // a row of a million tiny spans costs one pass and few quads.
            let mut painted_to = f32::NEG_INFINITY;
            for (sx, span) in row.spans.iter().enumerate() {
                let x0 = data_x + self.viewport.x_of(span.start, data_w);
                let x1 = data_x + self.viewport.x_of(span.end, data_w);
                if x1 < data_x - 1.0 || x0 > data_x + data_w + 1.0 {
                    continue;
                }
                let (x0, x1) = (x0.max(data_x - 2.0), x1.min(data_x + data_w + 2.0));
                let w = (x1 - x0).max(1.0);
                if x0 + w <= painted_to && w <= 1.0 {
                    continue;
                }
                painted_to = painted_to.max(x0 + w);
                let here = SpanRef { row: ix, span: sx };
                let hovered = self.hovered == Some((ix, Item::Span(sx)));
                let color = colors
                    .of_span(span)
                    .opacity(if hovered { 1.0 } else { 0.8 });
                let bar = bounds_at(x0, y + 3.0, w, ROW_HEIGHT - 6.0);
                let radius = if w > 4.0 { px(2.0) } else { px(0.) };
                if self.selected == Some(here) {
                    window.paint_quad(quad(
                        bar,
                        radius,
                        color,
                        px(1.5),
                        colors.selection,
                        gpui::BorderStyle::Solid,
                    ));
                } else {
                    window.paint_quad(fill(bar, color).corner_radii(radius));
                }
                if w > 40.0
                    && let Some(text) = &span.label
                {
                    window.with_content_mask(Some(ContentMask { bounds: bar }), |window| {
                        let label = Label {
                            text: text.clone(),
                            x: x0.max(data_x) + 4.0,
                            y: y + 2.0,
                            align: Align::Left,
                        };
                        self.font.paint(
                            &label,
                            colors.foreground,
                            point(px(0.), px(0.)),
                            window,
                            cx,
                        );
                    });
                }
            }
            for marker in &row.markers {
                let x = data_x + self.viewport.x_of(marker.at, data_w);
                if x < data_x - 4.0 || x > data_x + data_w + 4.0 {
                    continue;
                }
                let color = colors.of_marker(marker);
                window.paint_quad(fill(
                    bounds_at(x - 0.75, y + 1.0, 1.5, ROW_HEIGHT - 2.0),
                    color,
                ));
                window.paint_quad(
                    fill(bounds_at(x - 3.0, y + 1.0, 6.0, 4.0), color).corner_radii(px(1.0)),
                );
            }
        });
    }
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let body = self.body_height();
        let empty = self.spec.rows.is_empty();
        let frame = Frame {
            spec: self.spec.clone(),
            bands: self.bands.clone(),
            viewport: self.viewport,
            scroll: self.scroll,
            body,
            selected: self.selected,
            hovered: self.hover.map(|h| (h.row, h.item)),
            colors: Colors::new(cx),
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
        let tooltip = self.hover.and_then(|hover| self.tooltip(hover, cx));
        let viewport = div()
            .id("timeline-canvas")
            .debug_selector(|| "timeline-canvas".to_owned())
            .track_focus(&self.focus)
            .relative()
            .w_full()
            // The border is outside the canvas.
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
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_key_down(cx.listener(Self::key_down))
            .child(surface)
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(AXIS_HEIGHT + 2.0))
                        .left_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("no rows"),
                )
            })
            .children(tooltip);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "timeline".to_owned())
            .child(self.header(cx))
            .child(viewport)
            .children(self.details(cx))
    }
}

/// The timeline entity of `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TimelineSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TimelineView> {
    let view = crate::blocks::block_state(id, window, cx, |_, cx| TimelineView::new(spec, cx));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// The whole widget (header, timeline, detail line) of `id`.
pub fn render(id: BlockId, spec: &TimelineSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    view(id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests;
