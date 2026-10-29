//! Renderer of the `flame_graph` block kind (`show_flamegraph`): a
//! profile as an icicle (callers above their callees), painted on one
//! canvas.
//!
//! - Frame widths are their total value; colors are warm hues picked by
//!   name, so a function keeps its color everywhere. Names are drawn in
//!   the frames wide enough for them.
//! - A click on a frame zooms into it: it fills the width and its
//!   callers stay above it, dimmed. A click on the zoomed frame (or on a
//!   caller) zooms back out to that frame; `reset` and Escape go back to
//!   the root.
//! - The search box highlights the frames whose name contains the text
//!   and dims the rest; the readout gives the share of the total they
//!   cover (nested matches counted once).
//! - Deep stacks scroll: the wheel moves the rows when there are more
//!   than [`MAX_VISIBLE_ROWS`].
//! - Below, the functions with the most self time ([`TOP_ROWS`]), with
//!   their self and total shares; a click searches for that function.
//!
//! The view is an entity per block ([`super::block_state`]).

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::flame::{FlameSpec, FrameStat};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ContentMask, Context, ElementId, Entity, FocusHandle,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div,
    fill, hsla, point, prelude::FluentBuilder as _, px, quad, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};

use super::trace::caption;
use crate::blocks::chart::layout::{Align, Label};
use crate::blocks::chart::paint::LabelFont;
use crate::timeline::TimelineEntry;
use crate::widgets::{readout, register_input, tool_button};

/// Height of a frame row.
pub const ROW_HEIGHT: f32 = 18.0;
/// Rows shown before the graph scrolls.
pub const MAX_VISIBLE_ROWS: usize = 28;
/// Functions in the self-time table.
pub const TOP_ROWS: usize = 12;
/// Narrowest frame drawn, in pixels.
const MIN_WIDTH: f32 = 0.5;
/// Narrowest frame that gets its name.
const LABEL_MIN_WIDTH: f32 = 28.0;
/// Width of the tooltip.
const TOOLTIP_WIDTH: f32 = 300.0;

/// A warm color for frame `name` (red to yellow), stable across draws.
#[must_use]
pub fn frame_color(name: &str) -> Hsla {
    // FNV-1a.
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    let hue = (hash % 1_000) as f32 / 1_000.0 * 0.12;
    let light = 0.52 + ((hash >> 16) % 100) as f32 / 1_000.0;
    hsla(hue, 0.75, light, 1.0)
}

/// `value` as a share of `total`, one decimal.
#[must_use]
pub fn share(value: u64, total: u64) -> String {
    if total == 0 {
        return "0%".into();
    }
    format!("{:.1}%", value as f64 * 100.0 / total as f64)
}

/// The frames under `x` (0..`width`) at `depth`, given the zoom window
/// `[lo, lo + span)` and the nodes of that depth sorted by start.
fn hit(spec: &FlameSpec, row: &[usize], lo: f64, span: f64, width: f32, x: f32) -> Option<usize> {
    if span <= 0.0 || width <= 0.0 {
        return None;
    }
    let value = lo + f64::from(x / width) * span;
    // The last node starting at or before `value`.
    let pos = row.partition_point(|&ix| spec.nodes[ix].start as f64 <= value);
    let ix = *row.get(pos.checked_sub(1)?)?;
    let node = &spec.nodes[ix];
    (value < (node.start + node.total) as f64).then_some(ix)
}

/// Per-block state of a flame graph.
pub struct FlameView {
    spec: Rc<FlameSpec>,
    title: SharedString,
    parents: Rc<Vec<Option<usize>>>,
    /// Node indices per depth, by start.
    rows: Rc<Vec<Vec<usize>>>,
    stats: Rc<Vec<FrameStat>>,
    zoom: usize,
    scroll: usize,
    hover: Option<(usize, (f32, f32))>,
    query: String,
    matches: Rc<Vec<bool>>,
    match_count: usize,
    match_share: f64,
    input: Entity<InputState>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    _input_events: Subscription,
}

impl FlameView {
    fn new(spec: &FlameSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search frames"));
        register_input(&input, cx);
        let subscription =
            cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).value().to_string();
                    this.set_query(&text, cx);
                }
            });
        let spec = Rc::new(spec.clone());
        let mut view = Self {
            spec: spec.clone(),
            title: SharedString::default(),
            parents: Rc::default(),
            rows: Rc::default(),
            stats: Rc::default(),
            zoom: 0,
            scroll: 0,
            hover: None,
            query: String::new(),
            matches: Rc::default(),
            match_count: 0,
            match_share: 0.0,
            input,
            bounds: Rc::default(),
            focus: cx.focus_handle(),
            _input_events: subscription,
        };
        view.load(spec);
        view
    }

    fn load(&mut self, spec: Rc<FlameSpec>) {
        let mut rows: Vec<Vec<usize>> = vec![Vec::new(); spec.max_depth() as usize + 1];
        for (ix, node) in spec.nodes.iter().enumerate() {
            if let Some(row) = rows.get_mut(node.depth as usize) {
                row.push(ix);
            }
        }
        for row in &mut rows {
            row.sort_by_key(|&ix| spec.nodes[ix].start);
        }
        self.title = caption(spec.title.as_deref(), spec.path.as_deref(), "Flame graph").into();
        self.parents = Rc::new(spec.parents());
        self.stats = Rc::new(spec.frame_stats().into_iter().take(TOP_ROWS).collect());
        self.rows = Rc::new(rows);
        self.spec = spec;
        self.zoom = 0;
        self.scroll = 0;
        self.hover = None;
        self.refresh_matches();
    }

    fn sync(&mut self, spec: &FlameSpec, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            self.load(Rc::new(spec.clone()));
            cx.notify();
        }
    }

    fn refresh_matches(&mut self) {
        let (hits, share) = self.spec.search(&self.query);
        let mut matches = vec![false; self.spec.nodes.len()];
        for &ix in &hits {
            matches[ix] = true;
        }
        self.match_count = hits.len();
        self.match_share = share;
        self.matches = Rc::new(matches);
    }

    /// The spec drawn.
    pub fn spec(&self) -> &FlameSpec {
        &self.spec
    }

    /// The node the view is zoomed into (0 = the root).
    pub fn zoom(&self) -> usize {
        self.zoom
    }

    /// The frames the search matches, and the share of the total they
    /// cover.
    pub fn matches(&self) -> (usize, f64) {
        (self.match_count, self.match_share)
    }

    /// Rows scrolled past at the top.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Zooms into node `ix` (the root when out of range).
    pub fn zoom_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ix = if ix < self.spec.nodes.len() { ix } else { 0 };
        if self.zoom != ix {
            self.zoom = ix;
            // Keep the zoomed frame in view.
            let depth = self.spec.nodes[ix].depth as usize;
            if depth < self.scroll {
                self.scroll = depth;
            }
            cx.notify();
        }
    }

    /// Highlights the frames whose name contains `text`.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.query != text {
            self.query = text.to_owned();
            self.refresh_matches();
            cx.notify();
        }
    }

    fn visible_rows(&self) -> usize {
        self.rows.len().clamp(1, MAX_VISIBLE_ROWS)
    }

    fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows())
    }

    /// Zoom window on the value axis: start and width.
    fn window(&self) -> (f64, f64) {
        self.spec
            .nodes
            .get(self.zoom)
            .map_or((0.0, 1.0), |n| (n.start as f64, (n.total as f64).max(1.0)))
    }

    /// The node under `position` (window coordinates).
    fn locate(&self, position: Point<Pixels>) -> Option<(usize, (f32, f32))> {
        let bounds = self.bounds.get();
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        let width = f32::from(bounds.size.width);
        if x < 0.0 || y < 0.0 || x > width {
            return None;
        }
        let depth = (y / ROW_HEIGHT).floor() as usize + self.scroll;
        if depth >= self.scroll + self.visible_rows() {
            return None;
        }
        let row = self.rows.get(depth)?;
        let (lo, span) = self.window();
        hit(&self.spec, row, lo, span, width, x).map(|ix| (ix, (x, y)))
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let hover = self.locate(event.position);
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.stop_propagation();
        let Some((ix, _)) = self.locate(event.position) else {
            return;
        };
        // The zoomed frame or one of its callers zooms out to it; a
        // second click on the zoomed frame goes up one level.
        let target = if ix == self.zoom {
            self.parents.get(ix).copied().flatten().unwrap_or(0)
        } else {
            ix
        };
        self.zoom_to(target, cx);
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(ROW_HEIGHT)).y);
        let steps = (dy.abs() / ROW_HEIGHT).ceil().max(1.0) as usize;
        let next = if dy < 0.0 {
            (self.scroll + steps).min(self.max_scroll())
        } else {
            self.scroll.saturating_sub(steps)
        };
        if next != self.scroll {
            self.scroll = next;
            self.hover = None;
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.zoom != 0 {
            self.zoom_to(0, cx);
            cx.stop_propagation();
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let mut info = format!(
            "{} · {} {} · {} frames",
            spec.format.name(),
            spec.total(),
            spec.unit,
            spec.nodes.len().saturating_sub(1)
        );
        if spec.hidden > 0 {
            info.push_str(&format!(" · {} tiny hidden", spec.hidden));
        }
        let found = (!self.query.trim().is_empty()).then(|| {
            readout(
                format!(
                    "{} match{} · {:.1}%",
                    self.match_count,
                    if self.match_count == 1 { "" } else { "es" },
                    self.match_share * 100.0
                ),
                cx,
            )
            .debug_selector(|| "flame-matches".to_owned())
        });
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
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(info),
            )
            .children(found)
            .child(
                div()
                    .w(px(200.))
                    .debug_selector(|| "flame-search".to_owned())
                    .child(Input::new(&self.input).small()),
            )
            .when(self.zoom != 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .child(
                            tool_button("flame-reset", "reset", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.zoom_to(0, cx);
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    fn breadcrumb(&self, cx: &App) -> Option<AnyElement> {
        if self.zoom == 0 {
            return None;
        }
        let stack = self.spec.stack(self.zoom).join(" › ");
        let node = self.spec.nodes.get(self.zoom)?;
        Some(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .debug_selector(|| "flame-breadcrumb".to_owned())
                .child(format!(
                    "zoomed: {stack} ({})",
                    share(node.total, self.spec.total())
                ))
                .into_any_element(),
        )
    }

    fn tooltip(&self, cx: &App) -> Option<AnyElement> {
        let (ix, (x, y)) = self.hover?;
        let node = self.spec.nodes.get(ix)?;
        let theme = cx.theme();
        let total = self.spec.total();
        let width = f32::from(self.bounds.get().size.width);
        let left = if x + 14.0 + TOOLTIP_WIDTH > width {
            (x - 14.0 - TOOLTIP_WIDTH).max(0.0)
        } else {
            x + 14.0
        };
        Some(
            v_flex()
                .absolute()
                .left(px(left))
                .top(px(y + 14.0))
                .w(px(TOOLTIP_WIDTH))
                .p_1p5()
                .gap_0p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .text_xs()
                .debug_selector(|| "flame-tooltip".to_owned())
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(node.name.clone())),
                )
                .child(div().text_color(theme.muted_foreground).child(format!(
                    "total {} {} ({}) · self {} ({})",
                    node.total,
                    self.spec.unit,
                    share(node.total, total),
                    node.self_value,
                    share(node.self_value, total)
                )))
                .into_any_element(),
        )
    }

    fn top_table(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let total = self.spec.total();
        let (muted, hover_bg, bar) = (
            theme.muted_foreground,
            theme.secondary_hover,
            theme.chart_1.opacity(0.35),
        );
        let head = h_flex()
            .gap_2()
            .text_xs()
            .text_color(muted)
            .child(div().flex_1().child("function (most self time)"))
            .child(div().w(px(64.)).text_right().child("self"))
            .child(div().w(px(64.)).text_right().child("total"));
        let rows = self.stats.iter().enumerate().map(|(ix, stat)| {
            let name = stat.name.clone();
            #[allow(clippy::cast_precision_loss)]
            let fraction = if total == 0 {
                0.0
            } else {
                stat.self_value as f32 / total as f32
            };
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "flame-top-{ix}"
                ))))
                .debug_selector(move || format!("flame-top-{ix}"))
                .relative()
                .cursor_pointer()
                .rounded_sm()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(gpui::relative(fraction))
                        .rounded_sm()
                        .bg(bar),
                )
                .child(
                    h_flex()
                        .relative()
                        .gap_2()
                        .px_1()
                        .text_xs()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(SharedString::from(stat.name.clone())),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .text_right()
                                .child(share(stat.self_value, total)),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .text_right()
                                .text_color(muted)
                                .child(share(stat.total, total)),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let name = name.clone();
                    this.input
                        .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                    this.set_query(&name, cx);
                }))
        });
        v_flex()
            .gap_0p5()
            .debug_selector(|| "flame-top".to_owned())
            .child(head)
            .children(rows)
            .into_any_element()
    }
}

/// Colors and data one paint needs.
struct Frame {
    spec: Rc<FlameSpec>,
    rows: Rc<Vec<Vec<usize>>>,
    matches: Rc<Vec<bool>>,
    searching: bool,
    zoom: usize,
    window: (f64, f64),
    scroll: usize,
    visible: usize,
    hovered: Option<usize>,
    font: LabelFont,
    text: Hsla,
    matched: Hsla,
    outline: Hsla,
}

impl Frame {
    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let width = f32::from(bounds.size.width);
        let (lo, span) = self.window;
        let zoom_depth = self
            .spec
            .nodes
            .get(self.zoom)
            .map_or(0, |n| n.depth as usize);
        let x_of = |v: f64| ((v - lo) / span) as f32 * width;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for depth in self.scroll..(self.scroll + self.visible).min(self.rows.len()) {
                let y = oy + (depth - self.scroll) as f32 * ROW_HEIGHT;
                let row = &self.rows[depth];
                // First node ending after the window starts.
                let first = row.partition_point(|&ix| {
                    let n = &self.spec.nodes[ix];
                    ((n.start + n.total) as f64) <= lo
                });
                for &ix in &row[first..] {
                    let node = &self.spec.nodes[ix];
                    if node.start as f64 >= lo + span {
                        break;
                    }
                    let x0 = x_of(node.start as f64).max(0.0);
                    let x1 = x_of((node.start + node.total) as f64).min(width);
                    let w = x1 - x0;
                    if w < MIN_WIDTH {
                        continue;
                    }
                    let mut color = frame_color(&node.name);
                    if self.searching {
                        color = if self.matches.get(ix).copied().unwrap_or(false) {
                            self.matched
                        } else {
                            color.opacity(0.3)
                        };
                    }
                    if depth < zoom_depth {
                        color = color.opacity(color.a * 0.55);
                    }
                    let bar = Bounds::new(
                        point(px(ox + x0), px(y + 1.0)),
                        size(px((w - 1.0).max(MIN_WIDTH)), px(ROW_HEIGHT - 2.0)),
                    );
                    if self.hovered == Some(ix) {
                        window.paint_quad(quad(
                            bar,
                            px(2.0),
                            color,
                            px(1.0),
                            self.outline,
                            gpui::BorderStyle::Solid,
                        ));
                    } else {
                        window.paint_quad(fill(bar, color).corner_radii(px(2.0)));
                    }
                    if w >= LABEL_MIN_WIDTH {
                        window.with_content_mask(Some(ContentMask { bounds: bar }), |window| {
                            let label = Label {
                                text: node.name.clone(),
                                x: ox + x0 + 3.0,
                                y: y + 2.0,
                                align: Align::Left,
                            };
                            self.font
                                .paint(&label, self.text, point(px(0.), px(0.)), window, cx);
                        });
                    }
                }
            }
        });
    }
}

impl Render for FlameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let visible = self.visible_rows();
        let height = visible as f32 * ROW_HEIGHT;
        let frame = Frame {
            spec: self.spec.clone(),
            rows: self.rows.clone(),
            matches: self.matches.clone(),
            searching: !self.query.trim().is_empty(),
            zoom: self.zoom,
            window: self.window(),
            scroll: self.scroll,
            visible,
            hovered: self.hover.map(|(ix, _)| ix),
            font: LabelFont {
                font: window.text_style().font(),
                size: window.rem_size() * 0.7,
            },
            text: hsla(0.0, 0.0, 0.08, 1.0),
            matched: hsla(0.83, 0.6, 0.62, 1.0),
            outline: theme.foreground,
        };
        let bounds_cell = self.bounds.clone();
        let surface = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, cx| frame.paint(bounds, window, cx),
        )
        .size_full();
        let more = self.max_scroll() > 0;
        let canvas = div()
            .id("flame-canvas")
            .debug_selector(|| "flame-canvas".to_owned())
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .h(px(height))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_key_down(cx.listener(Self::key_down))
            .child(surface)
            .children(self.tooltip(cx));
        let note = more.then(|| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "depth {}–{} of {} · wheel to scroll",
                    self.scroll,
                    self.scroll + visible - 1,
                    self.rows.len() - 1
                ))
        });
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "flame".to_owned())
            // A press inside is not a click on the timeline row.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.header(cx))
            .children(self.breadcrumb(cx))
            .child(canvas)
            .children(note)
            .child(self.top_table(cx))
    }
}

/// The flame graph entity of block `id`, in sync with `spec`.
pub fn view(id: BlockId, spec: &FlameSpec, window: &mut Window, cx: &mut App) -> Entity<FlameView> {
    let view = super::block_state(id, window, cx, |window, cx| {
        FlameView::new(spec, window, cx)
    });
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::FlameGraph(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::flame::{FlameTree, ProfileFormat};
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> FlameSpec {
        let mut tree = FlameTree::new();
        tree.add(["main", "parse", "lex"], 30);
        tree.add(["main", "eval", "add"], 60);
        tree.add(["idle"], 10);
        let mut spec = tree.finish(ProfileFormat::Folded, "samples");
        spec.path = Some("prof/cpu.folded".into());
        spec
    }

    #[test]
    fn hits_pick_the_frame_under_the_cursor() {
        let spec = spec();
        // all, idle, main, eval, add, parse, lex
        let depth1 = [1, 2];
        assert_eq!(hit(&spec, &depth1, 0.0, 100.0, 100.0, 5.0), Some(1));
        assert_eq!(hit(&spec, &depth1, 0.0, 100.0, 100.0, 50.0), Some(2));
        assert_eq!(hit(&spec, &[5], 0.0, 100.0, 100.0, 50.0), None);
        // Zoomed into `main` (10..100): x = 0 is value 10.
        assert_eq!(hit(&spec, &[3, 5], 10.0, 90.0, 90.0, 1.0), Some(3));
        assert_eq!(share(1, 3), "33.3%");
        assert_ne!(frame_color("a"), frame_color("b"));
        assert_eq!(frame_color("a"), frame_color("a"));
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let at = cx
            .debug_bounds(selector)
            .map(|b| b.center())
            .unwrap_or_else(|| panic!("no {selector}"));
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn zooms_searches_and_lists_hot_functions(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::FlameGraph(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(800.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "flame",
            "flame-canvas",
            "flame-search",
            "flame-top",
            "flame-top-0",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let flame = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        // A click on the right half of the depth-1 row hits `main`.
        let canvas = cx.debug_bounds("flame-canvas").unwrap_or_default();
        let at = point(
            canvas.origin.x + canvas.size.width * 0.6,
            canvas.origin.y + px(ROW_HEIGHT * 1.5),
        );
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        let main = flame.read_with(cx, |f, _| f.zoom());
        assert_eq!(
            flame.read_with(cx, |f, _| f.spec().nodes[main].name.clone()),
            "main"
        );
        assert!(cx.debug_bounds("flame-breadcrumb").is_some());
        click(cx, "flame-reset");
        assert_eq!(flame.read_with(cx, |f, _| f.zoom()), 0);
        // The top function is `add`; a click searches for it.
        click(cx, "flame-top-0");
        let (count, share) = flame.read_with(cx, |f, _| f.matches());
        assert_eq!(count, 1);
        assert!((share - 0.6).abs() < 1e-9, "{share}");
        assert!(cx.debug_bounds("flame-matches").is_some());
    }
}
