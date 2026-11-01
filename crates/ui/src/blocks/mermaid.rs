//! Renderer of the `mermaid` block kind: the diagram, laid out by
//! rusty-mermaid ([`diagram`]) and painted with GPUI ([`paint`]), in a
//! viewport with pan and zoom. The viewport is a TwoCrest card in both app
//! themes: ink background, coral border and coral buttons. The diagram
//! fills its width; a taller one starts at the top and is dragged.
//!
//! Drag pans; ctrl/cmd + wheel zooms around the cursor; with the viewport
//! focused (click it) `+`/`-` zoom and `0` resets; the toolbar does the
//! same. A plain wheel is left to the timeline so it keeps scrolling. When
//! the source does not parse, the block shows the error and the source
//! instead of a diagram, and `t` shows the source of a valid one.
//!
//! Parsing and layout run on the background executor, as the graph widget
//! does: a placeholder shows until the first layout lands, and a new
//! source or theme keeps the diagram on screen until its new layout is
//! ready.

pub mod diagram;
pub mod paint;
pub mod wrap;

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind, BlockTag, MermaidSpec};
use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    Pixels, Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Task, Window, canvas, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::BlockView;
use super::copy::CopyFormat;
use crate::theme::token;
use crate::timeline::TimelineEntry;
use diagram::Diagram;
use paint::Camera;

/// Viewport height limits.
const MIN_HEIGHT: f32 = 140.0;
const MAX_HEIGHT: f32 = 640.0;
/// Zoom step of the keys, the buttons and one wheel notch.
const ZOOM_STEP: f32 = 1.25;
/// Characters of a source shown in the error view; garbage can be huge.
const MAX_SOURCE_CHARS: usize = 8_000;

/// Per-block state: the parsed diagram and its camera.
pub struct MermaidView {
    source: String,
    /// The last layout that landed; `None` before the first one.
    diagram: Option<Result<Rc<Diagram>, String>>,
    /// The color theme the diagram is (being) laid out with.
    colors: crate::app::ColorTheme,
    /// The layout running in the background, if any.
    layout: Option<Task<()>>,
    /// Whether the layout running is of a new source, which starts from
    /// the default camera when it lands.
    reset_camera: bool,
    show_source: bool,
    camera: Camera,
    /// Last mouse position of a drag in progress.
    drag: Option<gpui::Point<Pixels>>,
    /// Viewport bounds of the last paint, to zoom around the cursor.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

impl MermaidView {
    fn new(source: &str, cx: &mut Context<Self>) -> Self {
        let colors = crate::theme::crest_theme(cx);
        let mut view = Self {
            source: source.to_owned(),
            diagram: None,
            colors,
            layout: None,
            reset_camera: false,
            show_source: false,
            camera: Camera::default(),
            drag: None,
            bounds: Rc::default(),
            focus: cx.focus_handle(),
        };
        view.start_layout(cx);
        view
    }

    /// Parses and lays out the source in the current colors on the
    /// background executor. A layout still running is dropped.
    fn start_layout(&mut self, cx: &mut Context<Self>) {
        let (source, colors) = (self.source.clone(), self.colors.clone());
        let task = cx
            .background_executor()
            .spawn(async move { diagram::parse_with(&source, &colors) });
        self.layout = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.diagram = Some(result.map(Rc::new));
                if std::mem::take(&mut this.reset_camera) {
                    this.camera = Camera::default();
                }
                this.layout = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Lays the source out again when it changed; the camera is reset
    /// when the new layout lands.
    fn sync_source(&mut self, source: &str, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source.to_owned();
            self.reset_camera = true;
            self.start_layout(cx);
            cx.notify();
        }
    }

    /// Whether a layout is running.
    pub fn is_laying_out(&self) -> bool {
        self.layout.is_some()
    }

    /// Follows the theme and switches between diagram and source, on
    /// every frame.
    fn sync_view(&mut self, show_source: bool, cx: &mut Context<Self>) {
        // Another theme (a `/theme` preview): lay it out in its colors.
        let colors = crate::theme::crest_theme(cx);
        if !std::sync::Arc::ptr_eq(&colors, &self.colors) {
            self.colors = colors;
            self.start_layout(cx);
        }
        if self.show_source != show_source {
            self.show_source = show_source;
            cx.notify();
        }
    }

    /// The parse error, when the source is not a diagram.
    pub fn error(&self) -> Option<&str> {
        self.diagram.as_ref()?.as_ref().err().map(String::as_str)
    }

    /// Current pan and zoom.
    pub fn camera(&self) -> Camera {
        self.camera
    }

    fn viewport_size(&self) -> (f32, f32) {
        let size = self.bounds.get().size;
        (f32::from(size.width), f32::from(size.height))
    }

    /// Zooms by `factor` around `anchor` (window coordinates), or the
    /// viewport center.
    pub fn zoom(
        &mut self,
        factor: f32,
        anchor: Option<gpui::Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let Some(Ok(diagram)) = &self.diagram else {
            return;
        };
        let bounds = self.bounds.get();
        let size = self.viewport_size();
        let anchor = anchor.map_or((size.0 / 2.0, size.1 / 2.0), |p| {
            (
                f32::from(p.x - bounds.origin.x),
                f32::from(p.y - bounds.origin.y),
            )
        });
        self.camera.zoom_at(factor, anchor, diagram, size);
        cx.notify();
    }

    /// Back to the fitted, centered view.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.camera = Camera::default();
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(event.position);
        window.focus(&self.focus, cx);
        // Keep the timeline row from treating the drag as a click.
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let delta = event.position - last;
        self.camera.pan_by((f32::from(delta.x), f32::from(delta.y)));
        self.drag = Some(event.position);
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        let factor = ZOOM_STEP.powf(dy / 20.0);
        self.zoom(factor, Some(event.position), cx);
        cx.stop_propagation();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event
            .keystroke
            .key_char
            .as_deref()
            .unwrap_or(event.keystroke.key.as_str());
        match key {
            "+" | "=" => self.zoom(ZOOM_STEP, None, cx),
            "-" => self.zoom(1.0 / ZOOM_STEP, None, cx),
            "0" => self.reset(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_source(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mut source: String = diagram::printable(&self.source)
            .chars()
            .take(MAX_SOURCE_CHARS)
            .collect();
        if self.source.chars().count() > MAX_SOURCE_CHARS {
            source.push_str("\n…");
        }
        if source.trim().is_empty() {
            source = "(empty source)".to_owned();
        }
        div()
            .p_2()
            .rounded(theme.radius)
            .bg(theme.muted)
            .font_family(theme.mono_font_family.clone())
            .text_sm()
            .child(SharedString::from(source))
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let (accent, accent_hover, accent_fg) = (
            token(cx, "accent.bg"),
            token(cx, "accent.hover.bg"),
            token(cx, "accent.fg"),
        );
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .bg(accent)
                .hover(move |style| style.bg(accent_hover))
                .text_color(accent_fg)
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        h_flex()
            .absolute()
            .top_1()
            .right_1()
            .gap_1()
            .text_xs()
            .child(
                div()
                    .px_1()
                    .rounded_sm()
                    .bg(token(cx, "overlay"))
                    .text_color(token(cx, "button.fg"))
                    .child(format!("{:.0}%", self.camera.zoom * 100.0)),
            )
            .child(
                button("mermaid-zoom-out", "−")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.0 / ZOOM_STEP, None, cx))),
            )
            .child(
                button("mermaid-zoom-in", "+")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(ZOOM_STEP, None, cx))),
            )
            .child(
                button("mermaid-reset", "fit")
                    .on_click(cx.listener(|this, _, _, cx| this.reset(cx))),
            )
            .into_any_element()
    }

    /// The viewport before the first layout lands.
    fn render_pending(&self, cx: &App) -> AnyElement {
        let radius = cx.theme().radius;
        div()
            .debug_selector(|| "mermaid-pending".to_owned())
            .w_full()
            .h(px(MIN_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius)
            .bg(token(cx, "canvas.bg"))
            .border_1()
            .border_color(token(cx, "accent.bg"))
            .text_sm()
            .text_color(token(cx, "muted.fg"))
            .child("Laying out the diagram…")
            .into_any_element()
    }

    /// The diagram's height at the scale that fits the last painted width
    /// (its natural height before the first paint), within the limits.
    fn viewport_height(&self, diagram: &Diagram) -> f32 {
        let natural = diagram.height + diagram.padding * 2.0;
        let width = self.viewport_size().0;
        let fitted = if width > 0.0 {
            natural * Camera::fit(diagram, (width, 0.0))
        } else {
            natural
        };
        fitted.clamp(MIN_HEIGHT, MAX_HEIGHT)
    }

    fn render_diagram(&self, diagram: &Rc<Diagram>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let height = self.viewport_height(diagram);
        let family = theme.mono_font_family.clone();
        let radius = theme.radius;
        let (painted, bounds_cell, camera) = (diagram.clone(), self.bounds.clone(), self.camera);
        let surface = canvas(
            move |bounds, window, _| {
                // The height follows the width: draw again when it changes.
                if bounds_cell.replace(bounds).size.width != bounds.size.width {
                    window.refresh();
                }
            },
            move |bounds, (), window, cx| {
                paint::paint(&painted, camera, bounds, &family, window, cx);
            },
        )
        .size_full();
        div()
            .id("mermaid-viewport")
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .h(px(height))
            .overflow_hidden()
            .rounded(radius)
            .bg(token(cx, "canvas.bg"))
            .border_1()
            .border_color(token(cx, "accent.bg"))
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_key_down(cx.listener(Self::key_down))
            .child(surface)
            .child(self.toolbar(cx))
            .into_any_element()
    }
}

impl Render for MermaidView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.diagram {
            Some(Ok(_)) | None if self.show_source => self.render_source(cx),
            None => self.render_pending(cx),
            Some(Ok(diagram)) => {
                let diagram = diagram.clone();
                self.render_diagram(&diagram, cx)
            }
            Some(Err(err)) => {
                let theme = cx.theme();
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_color(theme.danger)
                            .child(SharedString::from(diagram::printable(err))),
                    )
                    .child(self.render_source(cx))
                    .into_any_element()
            }
        };
        div().w_full().child(body)
    }
}

/// The diagram view of block `id`, in sync with `spec`.
pub fn mermaid_view(
    entry: &TimelineEntry,
    spec: &MermaidSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<MermaidView> {
    source_view(
        entry.block.id,
        &spec.source,
        entry.alt_view,
        entry.rev,
        window,
        cx,
    )
}

/// The diagram view of `id` for `source` at revision `rev`: the source is
/// compared only when the revision is new.
fn source_view(
    id: BlockId,
    source: &str,
    show_source: bool,
    rev: u64,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<MermaidView> {
    let view = super::synced_state(
        id,
        rev,
        window,
        cx,
        |_, cx| MermaidView::new(source, cx),
        |view, _, cx| view.sync_source(source, cx),
    );
    let focus = view.read(cx).focus.clone();
    crate::blocks::remember_focus(id, &focus, cx);
    view.update(cx, |view, cx| view.sync_view(show_source, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Mermaid(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    render_source(
        entry.block.id,
        &spec.source,
        spec.title.as_deref(),
        entry.alt_view,
        entry.rev,
        window,
        cx,
    )
}

/// The registry entry of mermaid blocks ([`super::BlockView`]).
pub struct View;

impl BlockView for View {
    fn tag(&self) -> BlockTag {
        BlockTag::Mermaid
    }

    fn render(&self, entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
        render(entry, window, cx)
    }

    fn has_alt_view(&self) -> bool {
        true
    }

    fn copy_formats(&self, kind: &BlockKind) -> Vec<CopyFormat> {
        let BlockKind::Mermaid(spec) = kind else {
            return Vec::new();
        };
        vec![CopyFormat::new("source", "source", spec.source.clone())]
    }
}

/// Draws `source` as the diagram of `id` (a block, or a node of a custom
/// block at its block's revision `rev`), titled `title`.
pub fn render_source(
    id: BlockId,
    source: &str,
    title: Option<&str>,
    show_source: bool,
    rev: u64,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let view = source_view(id, source, show_source, rev, window, cx);
    let theme = cx.theme();
    let state = view.read(cx);
    let hint = match (state.error(), show_source) {
        (Some(_), _) => "diagram · invalid",
        (None, true) => "diagram · source · t: diagram",
        (None, false) => "diagram · drag to pan · ctrl+wheel, +/- to zoom · t: source",
    };
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or("Mermaid");
    v_flex()
        .gap_1()
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
                        .child(hint),
                ),
        )
        .child(view)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application_core::keymap::Action;
    use application_core::ports::events::DomainEvent;
    use domain::block::{Block, BlockId};
    use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, point, size};

    use crate::timeline::TimelineView;

    const FLOWCHART: &str = "flowchart TD\n  A[Inicio] --> B{¿Válido?}\n  B -->|sí| C[Fin]\n";
    const SEQUENCE: &str = "sequenceDiagram\n  Alice->>Bob: Hola\n  Bob-->>Alice: Qué tal\n";

    fn block(source: &str) -> Block {
        Block::new(BlockKind::Mermaid(MermaidSpec {
            source: source.to_owned(),
            title: Some("diagram".into()),
        }))
    }

    fn view_of(
        cx: &mut gpui::VisualTestContext,
        id: BlockId,
        source: &str,
    ) -> gpui::Entity<MermaidView> {
        let entry = TimelineEntry {
            block: Block {
                id,
                ..block(source)
            },
            expanded: true,
            alt_view: false,
            numbers: Default::default(),
            rev: crate::timeline::NO_REV,
        };
        let spec = MermaidSpec {
            source: source.to_owned(),
            title: Some("diagram".into()),
        };
        cx.update(|window, cx| mermaid_view(&entry, &spec, window, cx))
    }

    /// Draws the timeline, lets the layouts it started land and draws it
    /// again with them.
    fn draw(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<TimelineView>) {
        for _ in 0..2 {
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(900.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn invalid_diagrams_show_source_and_error(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let garbage = String::from_utf8_lossy(&[0, 0xff, 0xfe, 7, 27, b'x', 0x80]).into_owned();
        let sources = [
            FLOWCHART.to_owned(),
            SEQUENCE.to_owned(),
            String::new(),
            "flowchart LR\n A --> \n [[[".to_owned(),
            garbage,
            "x".repeat(20_000),
        ];
        let blocks: Vec<Block> = sources.iter().map(|s| block(s)).collect();
        view.update(cx, |view, cx| {
            for block in &blocks {
                view.apply(
                    &DomainEvent::BlockAdded {
                        block: block.clone(),
                    },
                    cx,
                );
            }
        });
        for ix in 0..blocks.len() {
            view.update(cx, |view, cx| {
                view.handle_action(&Action::builtin("block.first"), cx);
                for _ in 0..ix {
                    view.handle_action(&Action::builtin("block.next"), cx);
                }
            });
            draw(cx, &view);
        }
        for (ix, (block, source)) in blocks.iter().zip(&sources).enumerate() {
            let mermaid = view_of(cx, block.id, source);
            let error = mermaid.read_with(cx, |m, _| m.error().map(str::to_owned));
            assert_eq!(error.is_none(), ix < 2, "block {ix}: {error:?}");
        }
    }

    #[gpui::test]
    fn layouts_land_in_the_background_and_keep_the_old_diagram_meanwhile(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let id = BlockId::new();
        let mermaid = view_of(cx, id, FLOWCHART);
        assert!(mermaid.read_with(cx, |m, _| m.diagram.is_none() && m.is_laying_out()));
        cx.run_until_parked();
        let first = mermaid.read_with(cx, |m, _| match &m.diagram {
            Some(Ok(d)) => Rc::clone(d),
            _ => panic!("the flowchart lays out"),
        });

        mermaid.update(cx, |m, cx| {
            m.camera.pan_by((5.0, 5.0));
            m.sync_source(SEQUENCE, cx);
        });
        mermaid.read_with(cx, |m, _| {
            assert!(m.is_laying_out());
            assert!(
                matches!(&m.diagram, Some(Ok(d)) if Rc::ptr_eq(d, &first)),
                "the old diagram stays until the new one lands"
            );
            assert_eq!(m.camera.pan, (5.0, 5.0));
        });
        cx.run_until_parked();
        mermaid.read_with(cx, |m, _| {
            assert!(!m.is_laying_out());
            assert!(matches!(&m.diagram, Some(Ok(d)) if !Rc::ptr_eq(d, &first)));
            assert_eq!(m.camera, Camera::default(), "a new source starts fitted");
        });
    }

    #[gpui::test]
    fn toggles_to_source_with_t(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = block(FLOWCHART);
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx);
            view.handle_action(&Action::builtin("block.last"), cx);
            view.handle_action(&Action::builtin("block.toggle_view"), cx);
        });
        draw(cx, &view);
        let mermaid = view_of(cx, id, FLOWCHART);
        assert!(mermaid.read_with(cx, |m, _| m.show_source));
    }

    #[gpui::test]
    fn drag_pans_and_ctrl_wheel_zooms(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = block(FLOWCHART);
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx)
        });
        draw(cx, &view);
        let mermaid = view_of(cx, id, FLOWCHART);
        let center = mermaid.read_with(cx, |m, _| m.bounds.get().center());
        assert!(f32::from(center.y) > 0.0, "the viewport was painted");

        cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
        let moved = point(center.x + px(30.), center.y + px(10.));
        cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
        let camera = mermaid.read_with(cx, |m, _| m.camera());
        assert_eq!(camera.pan, (30.0, 10.0));
        assert!(
            view.read_with(cx, |v, _| v.state().entries()[0].expanded),
            "the drag is not a click that collapses the row"
        );

        draw(cx, &view);
        cx.simulate_event(ScrollWheelEvent {
            position: center,
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers: Modifiers::control(),
            touch_phase: TouchPhase::Moved,
        });
        let zoom = mermaid.read_with(cx, |m, _| m.camera().zoom);
        assert!((zoom - ZOOM_STEP).abs() < 1e-3, "zoom {zoom}");

        mermaid.update(cx, |m, cx| m.reset(cx));
        assert_eq!(mermaid.read_with(cx, |m, _| m.camera()), Camera::default());
    }
}
