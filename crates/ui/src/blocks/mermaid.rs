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

pub mod diagram;
pub mod paint;
pub mod wrap;

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockKind, MermaidSpec};
use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    Pixels, Render, Rgba, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, canvas, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::theme::palette::{ACCENT, INK, TEXT_2};
use crate::timeline::TimelineEntry;
use diagram::Diagram;
use paint::Camera;

/// Viewport height limits.
const MIN_HEIGHT: f32 = 140.0;
const MAX_HEIGHT: f32 = 640.0;
/// The coral of the buttons under the pointer (`primary.hover` of the theme).
const ACCENT_HOVER: &str = "#e2745f";
/// Ink at 70 %, behind the zoom level.
const INK_CHIP: &str = "#0b0b0cb3";
/// Zoom step of the keys, the buttons and one wheel notch.
const ZOOM_STEP: f32 = 1.25;
/// Characters of a source shown in the error view; garbage can be huge.
const MAX_SOURCE_CHARS: usize = 8_000;

/// Per-block state: the parsed diagram and its camera.
pub struct MermaidView {
    source: String,
    diagram: Result<Rc<Diagram>, String>,
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
        Self {
            source: source.to_owned(),
            diagram: diagram::parse(source).map(Rc::new),
            show_source: false,
            camera: Camera::default(),
            drag: None,
            bounds: Rc::default(),
            focus: cx.focus_handle(),
        }
    }

    /// Reparses when the source changed (resetting the camera) and switches
    /// between diagram and source.
    fn sync(&mut self, source: &str, show_source: bool, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source.to_owned();
            self.diagram = diagram::parse(source).map(Rc::new);
            self.camera = Camera::default();
            cx.notify();
        }
        if self.show_source != show_source {
            self.show_source = show_source;
            cx.notify();
        }
    }

    /// The parse error, when the source is not a diagram.
    pub fn error(&self) -> Option<&str> {
        self.diagram.as_ref().err().map(String::as_str)
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
        let Ok(diagram) = &self.diagram else { return };
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
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .bg(palette_color(ACCENT))
                .hover(|style| style.bg(palette_color(ACCENT_HOVER)))
                .text_color(palette_color(INK))
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
                    .bg(palette_color(INK_CHIP))
                    .text_color(palette_color(TEXT_2))
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
            .bg(palette_color(INK))
            .border_1()
            .border_color(palette_color(ACCENT))
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
            Ok(_) if self.show_source => self.render_source(cx),
            Ok(diagram) => {
                let diagram = diagram.clone();
                self.render_diagram(&diagram, cx)
            }
            Err(err) => {
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
    let view = super::block_state(entry.block.id, window, cx, |_, cx| {
        MermaidView::new(&spec.source, cx)
    });
    view.update(cx, |view, cx| {
        view.sync(&spec.source, entry.alt_view, cx);
    });
    view
}

/// A `#rrggbb` or `#rrggbbaa` color of the TwoCrest palette; the values are
/// static, so a bad one only falls back to transparent.
pub(crate) fn palette_color(hex: &str) -> Hsla {
    Rgba::try_from(hex).map(Hsla::from).unwrap_or_default()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Mermaid(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let view = mermaid_view(entry, spec, window, cx);
    let theme = cx.theme();
    let state = view.read(cx);
    let hint = match (state.error(), entry.alt_view) {
        (Some(_), _) => "diagram · invalid",
        (None, true) => "diagram · source · t: diagram",
        (None, false) => "diagram · drag to pan · ctrl+wheel, +/- to zoom · t: source",
    };
    let title = spec
        .title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Mermaid");
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
    use application::keymap::Action;
    use application::ports::events::DomainEvent;
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
        };
        let spec = MermaidSpec {
            source: source.to_owned(),
            title: Some("diagram".into()),
        };
        cx.update(|window, cx| mermaid_view(&entry, &spec, window, cx))
    }

    fn draw(cx: &mut gpui::VisualTestContext, view: &gpui::Entity<TimelineView>) {
        cx.draw(point(px(0.), px(0.)), size(px(800.), px(900.)), |_, _| {
            div().size_full().child(view.clone())
        });
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

    #[test]
    fn palette_colors_parse() {
        for hex in [ACCENT, ACCENT_HOVER, INK, INK_CHIP, TEXT_2] {
            assert!(Rgba::try_from(hex).is_ok(), "{hex}");
        }
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
