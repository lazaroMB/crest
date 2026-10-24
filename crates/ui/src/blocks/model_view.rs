//! Viewer of a 3D model (`CustomNode::Model3d`: `/open` of an OBJ, STL or
//! PLY file). The offscreen [`crate::render3d`] renderer draws it when
//! there is a GPU adapter, and [`vector`] paths otherwise, through the same
//! [`OrbitCamera`]: it starts in perspective, fitted to the model.
//!
//! Drag orbits; right-drag, middle-drag or shift-drag pans; the wheel
//! zooms. With the viewport focused (click it) `+`/`-` zoom, `0` fits the
//! model again, `i` is the isometric preset and `u` swaps the up axis (Y
//! or Z); the toolbar does the same. A line under the model gives its size
//! and which renderer draws it.

pub mod vector;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::block::BlockId;
use domain::model3d::{ModelSpec, UpAxis};
use gpui::{
    App, Bounds, Context, Corners, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _,
    Pixels, Point, Render, RenderImage, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::render3d::camera::OrbitCamera;
use crate::render3d::mesh::Mesh;
use crate::sysboard::{Backend, auto_backend};
use crate::theme::token;

/// Zoom step of one wheel notch.
const WHEEL_STEP: f32 = 1.15;
/// Zoom step of the keys and the buttons.
const ZOOM_STEP: f32 = 1.25;
/// Base colour of the model, linear: a neutral clay grey.
const CLAY: [f32; 3] = [0.55, 0.56, 0.6];
/// Height of the viewport at least, and the room left around it in the
/// window (modal header, info line, margins).
const MIN_HEIGHT: f32 = 240.0;
const CHROME: f32 = 150.0;
/// Share of the window height a modal takes (as `/open` opens it).
const MODAL_SHARE: f32 = 0.85;

/// Keys of the uploaded meshes: a new one per built mesh.
static NEXT_MESH: AtomicU64 = AtomicU64::new(1);

/// What a rendered GPU frame showed; a new key renders a new frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameKey {
    size: (u32, u32),
    mesh: u64,
    camera: [u32; 7],
    background: domain::theme::Color,
}

#[derive(Default)]
struct Surface {
    image: Option<Arc<RenderImage>>,
    key: Option<FrameKey>,
    retired: Vec<Arc<RenderImage>>,
    failed: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    last: Point<Pixels>,
    pan: bool,
}

/// Which model a view shows: where its data lives and how big it is. A
/// model opened again (a new spec) has new data, so the view rebuilds.
type Fingerprint = (usize, usize, usize);

fn fingerprint(spec: &ModelSpec) -> Fingerprint {
    (
        spec.mesh.positions.as_ptr() as usize,
        spec.mesh.positions.len(),
        spec.mesh.triangles.len(),
    )
}

/// Per-node state: the model, its mesh and the camera.
pub struct ModelView {
    spec: ModelSpec,
    fingerprint: Fingerprint,
    up: UpAxis,
    mesh: Rc<Mesh>,
    mesh_key: u64,
    fitted: OrbitCamera,
    camera: OrbitCamera,
    backend: Backend,
    surface: Rc<RefCell<Surface>>,
    drag: Option<Drag>,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

impl ModelView {
    fn new(spec: &ModelSpec, cx: &mut Context<Self>) -> Self {
        let up = spec.mesh.up;
        let (mesh, mesh_key, fitted) = build(spec, up);
        Self {
            spec: spec.clone(),
            fingerprint: fingerprint(spec),
            up,
            mesh,
            mesh_key,
            fitted,
            camera: fitted,
            backend: auto_backend(cx),
            surface: Rc::default(),
            drag: None,
            viewport: Rc::default(),
            focus: cx.focus_handle(),
        }
    }

    /// Shows `spec`, unless it is the model already shown.
    fn sync(&mut self, spec: &ModelSpec, cx: &mut Context<Self>) {
        if fingerprint(spec) == self.fingerprint {
            return;
        }
        *self = Self {
            backend: self.backend.clone(),
            ..Self::new(spec, cx)
        };
    }

    /// The camera.
    pub fn camera(&self) -> OrbitCamera {
        self.camera
    }

    /// The axis taken as up.
    pub fn up(&self) -> UpAxis {
        self.up
    }

    /// Whether the GPU renderer draws the model.
    pub fn on_gpu(&self) -> bool {
        matches!(self.backend, Backend::Gpu(_))
    }

    fn viewport_size(&self) -> (f32, f32) {
        let size = self.viewport.get().size;
        (
            f32::from(size.width).max(1.0),
            f32::from(size.height).max(1.0),
        )
    }

    /// Zooms by `factor`.
    pub fn zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.camera.zoom_by(factor);
        cx.notify();
    }

    /// Back to the fitted view.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.camera = self.fitted;
        cx.notify();
    }

    /// The isometric preset.
    pub fn iso(&mut self, cx: &mut Context<Self>) {
        self.camera.iso();
        cx.notify();
    }

    /// Swaps the axis taken as up and fits the model again.
    pub fn toggle_up(&mut self, cx: &mut Context<Self>) {
        self.up = self.up.toggled();
        (self.mesh, self.mesh_key, self.fitted) = build(&self.spec, self.up);
        self.camera = self.fitted;
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.drag = Some(Drag {
            last: event.position,
            pan: event.button != MouseButton::Left || event.modifiers.shift,
        });
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else { return };
        if event.pressed_button.is_none() {
            self.drag = None;
            return;
        }
        let d = event.position - drag.last;
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        if drag.pan {
            self.camera.pan(dx, dy, self.viewport_size());
        } else {
            self.camera.orbit(dx, dy);
        }
        self.drag = Some(Drag {
            last: event.position,
            ..drag
        });
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        self.zoom(WHEEL_STEP.powf(dy / 20.0), cx);
        cx.stop_propagation();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = event
            .keystroke
            .key_char
            .as_deref()
            .unwrap_or(event.keystroke.key.as_str());
        match key {
            "+" | "=" => self.zoom(ZOOM_STEP, cx),
            "-" => self.zoom(1.0 / ZOOM_STEP, cx),
            "0" => self.fit(cx),
            "i" => self.iso(cx),
            "u" => self.toggle_up(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let up = match self.up {
            UpAxis::Y => "Y up",
            UpAxis::Z => "Z up",
        };
        h_flex()
            .absolute()
            .top_1()
            .right_1()
            .gap_1()
            .text_xs()
            .child(
                button("model-zoom-out", "−")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.0 / ZOOM_STEP, cx))),
            )
            .child(
                button("model-zoom-in", "+")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(ZOOM_STEP, cx))),
            )
            .child(button("model-iso", "iso").on_click(cx.listener(|this, _, _, cx| this.iso(cx))))
            .child(
                button("model-up", up).on_click(cx.listener(|this, _, _, cx| this.toggle_up(cx))),
            )
            .child(button("model-fit", "fit").on_click(cx.listener(|this, _, _, cx| this.fit(cx))))
    }

    /// Vertices, triangles, size in file units and the renderer.
    fn info(&self) -> String {
        let mesh = &self.spec.mesh;
        let size = mesh.bounds().map_or_else(String::new, |(min, max)| {
            let d = |axis: usize| number(max[axis] - min[axis]);
            format!(" · {} × {} × {}", d(0), d(1), d(2))
        });
        let triangles = mesh.triangles.len();
        let renderer = match &self.backend {
            Backend::Gpu(_) => match &self.surface.borrow().failed {
                Some(err) => format!("GPU failed: {err}"),
                None => "GPU".to_owned(),
            },
            Backend::Vector(reason) if vector::drawn(triangles) < triangles => format!(
                "vector ({reason}; {} of the triangles drawn)",
                vector::drawn(triangles)
            ),
            Backend::Vector(reason) => format!("vector ({reason})"),
        };
        format!(
            "{} · {} vertices · {triangles} triangles{size} · {renderer}",
            self.spec.name,
            mesh.positions.len()
        )
    }

    fn surface(&self) -> impl IntoElement {
        let (surface, backend, viewport) = (
            Rc::clone(&self.surface),
            self.backend.clone(),
            Rc::clone(&self.viewport),
        );
        let (mesh, mesh_key, camera) = (Rc::clone(&self.mesh), self.mesh_key, self.camera);
        canvas(
            move |bounds, _, _| viewport.set(bounds),
            move |bounds, (), window, cx| {
                let background = crate::theme::crest_theme(cx).color("canvas.bg");
                let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if size.0 <= 0.0 || size.1 <= 0.0 {
                    return;
                }
                let mut s = surface.borrow_mut();
                for old in s.retired.drain(..) {
                    window.drop_image(old).ok();
                }
                if let Backend::Gpu(renderer) = &backend
                    && s.failed.is_none()
                {
                    let scale = window.scale_factor();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let px_size = ((size.0 * scale) as u32, (size.1 * scale) as u32);
                    let key = FrameKey {
                        size: px_size,
                        mesh: mesh_key,
                        camera: camera_bits(&camera),
                        background,
                    };
                    if s.key != Some(key) {
                        renderer.borrow_mut().set_background(background);
                        match renderer.borrow_mut().render_mesh(
                            &mesh,
                            Some(mesh_key),
                            &camera,
                            px_size,
                        ) {
                            Ok(r) => {
                                if let Some(old) = s.image.replace(r.image) {
                                    s.retired.push(old);
                                }
                                s.key = Some(key);
                            }
                            Err(e) => {
                                s.failed = Some(e.to_string());
                                window.refresh();
                            }
                        }
                    }
                    if let (Some(image), None) = (&s.image, &s.failed) {
                        window
                            .paint_image(
                                bounds,
                                bounds,
                                Corners::default(),
                                Arc::clone(image),
                                0,
                                false,
                            )
                            .ok();
                        return;
                    }
                }
                vector::paint(&mesh, &camera, bounds, window);
            },
        )
        .size_full()
    }
}

impl Render for ModelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height =
            (f32::from(window.viewport_size().height) * MODAL_SHARE - CHROME).max(MIN_HEIGHT);
        let theme = cx.theme();
        let (radius, muted) = (theme.radius, theme.muted_foreground);
        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .id("model-viewport")
                    .debug_selector(|| "model-viewport".to_owned())
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
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, _, _, _| this.drag = None),
                    )
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_key_down(cx.listener(Self::key_down))
                    .child(self.surface())
                    .child(self.toolbar(cx))
                    .child(
                        div()
                            .absolute()
                            .left_2()
                            .bottom_1()
                            .text_xs()
                            .text_color(token(cx, "muted.fg"))
                            .child(
                                "drag: orbit · right/shift-drag: pan · wheel/+-: zoom · \
                                 0: fit · i: iso · u: up axis",
                            ),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(self.info())),
            )
    }
}

/// The mesh of `spec` with `up` as up, a new key for it and the camera
/// fitted to it.
fn build(spec: &ModelSpec, up: UpAxis) -> (Rc<Mesh>, u64, OrbitCamera) {
    let (mesh, (min, max)) = Mesh::from_model(&spec.mesh, up, CLAY);
    let key = NEXT_MESH.fetch_add(1, Ordering::Relaxed);
    (Rc::new(mesh), key, OrbitCamera::fit_bounds(min, max))
}

fn camera_bits(camera: &OrbitCamera) -> [u32; 7] {
    [
        camera.yaw.to_bits(),
        camera.pitch.to_bits(),
        camera.zoom.to_bits(),
        camera.target[0].to_bits(),
        camera.target[1].to_bits(),
        camera.target[2].to_bits(),
        u32::from(camera.ortho),
    ]
}

/// A length for the info line: up to 3 significant decimals, no trailing
/// zeros.
fn number(v: f32) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// The viewer of the model node `id`, in sync with `spec`.
pub fn model_view(
    id: BlockId,
    spec: &ModelSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ModelView> {
    let view = super::block_state(id, window, cx, |_, cx| ModelView::new(spec, cx));
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

#[cfg(test)]
mod tests {
    use domain::model3d::ModelMesh;
    use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, point};

    use super::*;

    fn spec() -> ModelSpec {
        ModelSpec {
            name: "tetra.stl".into(),
            mesh: ModelMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [0.0, 0.0, 25.0],
                ],
                triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
                up: UpAxis::Z,
            },
        }
    }

    struct Host {
        view: Entity<ModelView>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.view.clone())
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<ModelView>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sysboard::disable_gpu(cx);
        });
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            view: model_view(BlockId::new(), &spec(), window, cx),
        });
        cx.run_until_parked();
        let view = host.read_with(cx, |host, _| host.view.clone());
        (view, cx)
    }

    #[test]
    fn lengths_are_short() {
        assert_eq!(number(25.0), "25");
        assert_eq!(number(0.125_4), "0.125");
        assert_eq!(number(1e-5), "0");
    }

    #[gpui::test]
    fn the_model_draws_on_the_fallback_with_its_info(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.read_with(cx, |view, _| {
            assert!(!view.on_gpu());
            assert_eq!(view.up(), UpAxis::Z);
            assert!(view.viewport.get().size.width > px(0.0), "painted");
            let info = view.info();
            assert!(
                info.starts_with("tetra.stl · 4 vertices · 4 triangles"),
                "{info}"
            );
            assert!(info.contains("10 × 10 × 25"), "{info}");
            assert!(info.contains("vector"), "{info}");
        });
    }

    #[gpui::test]
    fn drag_orbits_right_drag_pans_and_the_wheel_zooms(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let fitted = view.read_with(cx, |view, _| view.camera());
        let center = view.read_with(cx, |view, _| view.viewport.get().center());
        let moved = point(center.x + px(40.), center.y + px(10.));
        cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(moved, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
        let orbited = view.read_with(cx, |view, _| view.camera());
        assert_ne!(orbited.yaw, fitted.yaw);
        assert_eq!(orbited.target, fitted.target);

        cx.simulate_mouse_down(center, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_move(moved, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(moved, MouseButton::Right, Modifiers::none());
        let panned = view.read_with(cx, |view, _| view.camera());
        assert_eq!(panned.yaw, orbited.yaw);
        assert_ne!(panned.target, orbited.target);

        cx.simulate_event(ScrollWheelEvent {
            position: center,
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        let zoom = view.read_with(cx, |view, _| view.camera().zoom);
        assert!((zoom - WHEEL_STEP).abs() < 1e-3, "zoom {zoom}");
    }

    #[gpui::test]
    fn the_camera_and_the_up_axis_follow_the_controls(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let fitted = view.read_with(cx, |view, _| view.camera());
        view.update(cx, |view, cx| {
            view.zoom(ZOOM_STEP, cx);
            view.toggle_up(cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.up(), UpAxis::Y);
            assert_eq!(view.camera().zoom, 1.0, "the model is fitted again");
        });
        view.update(cx, |view, cx| {
            view.camera.orbit(40.0, 10.0);
            view.iso(cx);
        });
        view.read_with(cx, |view, _| assert!(view.camera().ortho));
        view.update(cx, |view, cx| view.fit(cx));
        view.read_with(cx, |view, _| {
            assert!(!view.camera().ortho);
            assert_eq!(view.camera().yaw, fitted.yaw);
        });
    }

    #[gpui::test]
    fn the_same_spec_keeps_the_view_and_a_new_one_rebuilds_it(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let first = spec();
        view.update(cx, |view, cx| {
            view.sync(&first, cx);
            view.toggle_up(cx);
            let key = view.mesh_key;
            view.sync(&first, cx);
            assert_eq!(view.mesh_key, key, "same data: kept");
            let mut other = spec();
            other.mesh.positions.push([1.0, 1.0, 1.0]);
            view.sync(&other, cx);
            assert_ne!(view.mesh_key, key);
            assert_eq!(view.up(), UpAxis::Z, "back to the file's axis");
        });
    }
}
