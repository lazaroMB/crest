//! Viewer of a 3D model (`CustomNode::Model3d`: `/open` of an OBJ, STL,
//! PLY or STEP file; the `cad_model` block of `show_step`). The offscreen
//! [`crate::render3d`] renderer draws it when there is a GPU adapter, and
//! [`vector`] paths otherwise, through the same [`OrbitCamera`]: it starts
//! in perspective, fitted to the model.
//!
//! Drag orbits; right-drag, middle-drag or shift-drag pans; the wheel
//! zooms. With the viewport focused (click it) `+`/`-` zoom, `0` fits the
//! model again, `i` is the isometric preset and `u` swaps the up axis (Y
//! or Z); the toolbar does the same.
//!
//! A section plane cuts the model across the file's x, y or z axis (`s`
//! cycles off → x → y → z, `f` flips the kept side, `[`/`]` move it by a
//! twentieth of the model; the second toolbar does the same). The cut is
//! made on the CPU before drawing ([`crate::render3d::section`]), so both
//! renderers show it; the cut is open (no cap). Measuring (`m`) picks
//! points on the drawn surface with a click: two give their distance and
//! the difference per axis in the file's unit (`mm` from STEP; plain
//! "units" for OBJ/STL/PLY, which do not say), `Esc` clears them. A line
//! under the model gives its size and which renderer draws it.

pub mod vector;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::block::BlockId;
use domain::model3d::{ModelSpec, UpAxis};
use gpui::{
    App, Bounds, Context, Corners, Entity, FocusHandle, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, PathBuilder, Pixels, Point, Render, RenderImage, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::render3d::camera::OrbitCamera;
use crate::render3d::math::V3;
use domain::sysmodel::scene::Rgb;
use crate::render3d::mesh::{Mesh, Placement};
use crate::render3d::section::{self, Section};
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
/// Steps of the section plane across the model.
pub const SECTION_STEPS: f32 = 20.0;
/// A press that moves less than this (pixels) is a click, not a drag.
const CLICK_SLOP: f32 = 4.0;
/// Radius of a measured point's dot, in pixels.
const DOT: f32 = 4.0;

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
    start: Point<Pixels>,
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
    placement: Placement,
    mesh: Rc<Mesh>,
    mesh_key: u64,
    fitted: OrbitCamera,
    camera: OrbitCamera,
    backend: Backend,
    surface: Rc<RefCell<Surface>>,
    drag: Option<Drag>,
    viewport: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    /// Triangle ranges left out (hidden solids).
    hidden: Vec<Range<usize>>,
    section: Option<Section>,
    measuring: bool,
    /// Measured points, in file coordinates (at most two).
    points: Vec<V3>,
    /// Fixed viewport height when shown inline; the modal share otherwise.
    height: Option<f32>,
    /// A colour per triangle (linear RGB), instead of clay.
    colors: Option<Rc<[Rgb]>>,
    /// Points (file coordinates) a measured point snaps to, e.g. atom
    /// centres.
    snap: Rc<[V3]>,
}

impl ModelView {
    fn new(spec: &ModelSpec, cx: &mut Context<Self>) -> Self {
        let up = spec.mesh.up;
        let placement = Placement::of(&spec.mesh, up);
        let mut view = Self {
            spec: spec.clone(),
            fingerprint: fingerprint(spec),
            up,
            placement,
            mesh: Rc::default(),
            mesh_key: 0,
            fitted: OrbitCamera::fit_bounds(placement.bounds.0, placement.bounds.1),
            camera: OrbitCamera::fit_bounds(placement.bounds.0, placement.bounds.1),
            backend: auto_backend(cx),
            surface: Rc::default(),
            drag: None,
            viewport: Rc::default(),
            focus: cx.focus_handle(),
            hidden: Vec::new(),
            section: None,
            measuring: false,
            points: Vec::new(),
            height: None,
            colors: None,
            snap: Rc::from(Vec::new()),
        };
        view.rebuild();
        view
    }

    /// Shows `spec`, unless it is the model already shown.
    fn sync(&mut self, spec: &ModelSpec, cx: &mut Context<Self>) {
        if fingerprint(spec) == self.fingerprint {
            return;
        }
        let (height, colors, snap) = (self.height, self.colors.clone(), Rc::clone(&self.snap));
        *self = Self {
            backend: self.backend.clone(),
            ..Self::new(spec, cx)
        };
        self.height = height;
        self.snap = snap;
        if colors.is_some() {
            self.colors = colors;
            self.rebuild();
        }
    }

    /// Builds the drawn mesh again (hidden ranges, section), keeping the
    /// placement and the camera.
    fn rebuild(&mut self) {
        let hidden = self.hidden.clone();
        let keep = move |i: usize| !hidden.iter().any(|r| r.contains(&i));
        let colors = self.colors.clone();
        let color = move |i: usize| {
            colors
                .as_ref()
                .and_then(|c| c.get(i).copied())
                .unwrap_or(CLAY)
        };
        self.mesh = Rc::new(Mesh::from_model_painted(
            &self.spec.mesh,
            &self.placement,
            &color,
            &keep,
            self.section.as_ref(),
        ));
        self.mesh_key = NEXT_MESH.fetch_add(1, Ordering::Relaxed);
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

    /// The mesh drawn (cut and filtered).
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// Shows the viewport `height` pixels tall (inline in a block) instead
    /// of the modal's share of the window.
    pub fn set_height(&mut self, height: Option<f32>) {
        self.height = height;
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
        self.placement = Placement::of(&self.spec.mesh, self.up);
        self.fitted = OrbitCamera::fit_bounds(self.placement.bounds.0, self.placement.bounds.1);
        self.camera = self.fitted;
        self.rebuild();
        cx.notify();
    }

    /// Colours the triangles one by one (linear RGB; clay past the end), or
    /// all clay again with `None`.
    pub fn set_colors(&mut self, colors: Option<Rc<[Rgb]>>, cx: &mut Context<Self>) {
        let same = match (&self.colors, &colors) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.colors = colors;
            self.rebuild();
            cx.notify();
        }
    }

    /// Measured points snap to the nearest of `points` (file coordinates):
    /// atom centres for a molecule. Empty: no snapping.
    pub fn set_snap(&mut self, points: Rc<[V3]>) {
        self.snap = points;
    }

    /// Leaves the triangles in `ranges` out (hidden solids).
    pub fn set_hidden(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        if ranges != self.hidden {
            self.hidden = ranges;
            self.rebuild();
            cx.notify();
        }
    }

    /// The section plane, if any.
    pub fn section(&self) -> Option<Section> {
        self.section
    }

    /// The model's extent along file axis `axis`.
    fn extent(&self, axis: usize) -> (f32, f32) {
        self.spec
            .mesh
            .bounds()
            .map_or((0.0, 0.0), |(min, max)| (min[axis], max[axis]))
    }

    /// Sets the section plane (or removes it) and cuts the model again.
    pub fn set_section(&mut self, section: Option<Section>, cx: &mut Context<Self>) {
        if section != self.section {
            self.section = section;
            self.rebuild();
            cx.notify();
        }
    }

    /// Off → x → y → z → off; a new axis starts through the middle of the
    /// model.
    pub fn cycle_section(&mut self, cx: &mut Context<Self>) {
        let next = match self.section {
            None => Some(0),
            Some(s) if s.axis < 2 => Some(s.axis + 1),
            Some(_) => None,
        };
        let section = next.map(|axis| {
            let (lo, hi) = self.extent(axis);
            Section {
                axis,
                offset: (lo + hi) / 2.0,
                flip: self.section.is_some_and(|s| s.flip),
            }
        });
        self.set_section(section, cx);
    }

    /// Keeps the other side of the plane.
    pub fn flip_section(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.section {
            self.set_section(Some(Section { flip: !s.flip, ..s }), cx);
        }
    }

    /// Moves the plane by `steps` twentieths of the model, within it.
    pub fn move_section(&mut self, steps: f32, cx: &mut Context<Self>) {
        let Some(s) = self.section else { return };
        let (lo, hi) = self.extent(s.axis);
        let step = (hi - lo) / SECTION_STEPS;
        let offset = (s.offset + steps * step).clamp(lo, hi);
        self.set_section(Some(Section { offset, ..s }), cx);
    }

    /// Whether clicks measure.
    pub fn measuring(&self) -> bool {
        self.measuring
    }

    /// Turns measuring on or off (off clears the points).
    pub fn set_measuring(&mut self, on: bool, cx: &mut Context<Self>) {
        self.measuring = on;
        if !on {
            self.points.clear();
        }
        cx.notify();
    }

    /// The measured points, in file coordinates.
    pub fn points(&self) -> &[V3] {
        &self.points
    }

    /// The distance between the two measured points, in file units.
    pub fn distance(&self) -> Option<f32> {
        match self.points.as_slice() {
            [a, b] => Some(section::distance(*a, *b)),
            _ => None,
        }
    }

    /// The surface point (file coordinates) under window position `at`,
    /// if the drawn mesh is there.
    pub fn pick_at(&self, at: Point<Pixels>) -> Option<V3> {
        let bounds = self.viewport.get();
        let x = f32::from(at.x - bounds.origin.x);
        let y = f32::from(at.y - bounds.origin.y);
        let (origin, dir) = self.camera.ray(x, y, self.viewport_size())?;
        let hit = section::pick(&self.mesh, origin, dir)?;
        Some(self.placement.to_file(hit))
    }

    /// Adds the point under `at` to the measurement (a third starts a new
    /// one); `false` when nothing is there.
    pub fn measure_at(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(p) = self.pick_at(at) else {
            return false;
        };
        let p = self
            .snap
            .iter()
            .copied()
            .min_by(|a, b| section::distance(*a, p).total_cmp(&section::distance(*b, p)))
            .unwrap_or(p);
        if self.points.len() >= 2 {
            self.points.clear();
        }
        self.points.push(p);
        cx.notify();
        true
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.drag = Some(Drag {
            start: event.position,
            last: event.position,
            pan: event.button != MouseButton::Left || event.modifiers.shift,
        });
        cx.stop_propagation();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        let moved = event.position - drag.start;
        let click = f32::from(moved.x).abs() < CLICK_SLOP && f32::from(moved.y).abs() < CLICK_SLOP;
        if click && self.measuring && event.button == MouseButton::Left && !drag.pan {
            self.measure_at(event.position, cx);
        }
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
            "s" => self.cycle_section(cx),
            "f" => self.flip_section(cx),
            "[" => self.move_section(-1.0, cx),
            "]" => self.move_section(1.0, cx),
            "m" => self.set_measuring(!self.measuring, cx),
            _ if event.keystroke.key == "escape" && !self.points.is_empty() => {
                self.points.clear();
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn button(
        id: &'static str,
        label: impl Into<SharedString>,
        on: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (accent, accent_hover, accent_fg) = (
            token(cx, "accent.bg"),
            token(cx, "accent.hover.bg"),
            token(cx, "accent.fg"),
        );
        let (bg, fg) = if on {
            (cx.theme().primary, cx.theme().primary_foreground)
        } else {
            (accent, accent_fg)
        };
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .px_1p5()
            .rounded_sm()
            .bg(bg)
            .hover(move |style| style.bg(accent_hover))
            .text_color(fg)
            .font_weight(FontWeight::SEMIBOLD)
            .child(label.into())
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                Self::button("model-zoom-out", "−", false, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(1.0 / ZOOM_STEP, cx))),
            )
            .child(
                Self::button("model-zoom-in", "+", false, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom(ZOOM_STEP, cx))),
            )
            .child(
                Self::button("model-iso", "iso", false, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.iso(cx))),
            )
            .child(
                Self::button("model-up", up, false, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_up(cx))),
            )
            .child(
                Self::button("model-fit", "fit", false, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }

    /// Section and measuring controls, top left.
    fn tools(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let cut = match self.section {
            None => "cut: off".to_owned(),
            Some(s) => format!("cut: {}", s.axis_name()),
        };
        let has_cut = self.section.is_some();
        let flip = if self.section.is_some_and(|s| s.flip) {
            "keep ≥"
        } else {
            "keep ≤"
        };
        h_flex()
            .absolute()
            .top_1()
            .left_1()
            .gap_1()
            .text_xs()
            .child(
                Self::button("model-section", cut, has_cut, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_section(cx))),
            )
            .when(has_cut, |row| {
                row.child(
                    Self::button("model-section-back", "◀", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.move_section(-1.0, cx))),
                )
                .child(
                    Self::button("model-section-forward", "▶", false, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.move_section(1.0, cx))),
                )
                .child(
                    Self::button("model-section-flip", flip, false, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.flip_section(cx))),
                )
            })
            .child(
                Self::button("model-measure", "measure", self.measuring, cx).on_click(
                    cx.listener(|this, _, _, cx| this.set_measuring(!this.measuring, cx)),
                ),
            )
    }

    fn unit(&self) -> &str {
        self.spec.unit.as_deref().unwrap_or("units")
    }

    /// Vertices, triangles, size in file units and the renderer.
    fn info(&self) -> String {
        let mesh = &self.spec.mesh;
        let unit = self.unit();
        let size = mesh.bounds().map_or_else(String::new, |(min, max)| {
            let d = |axis: usize| number(max[axis] - min[axis]);
            format!(" · {} × {} × {} {unit}", d(0), d(1), d(2))
        });
        let triangles = mesh.triangles.len();
        let drawn = self.mesh.indices.len() / 3;
        let renderer = match &self.backend {
            Backend::Gpu(_) => match &self.surface.borrow().failed {
                Some(err) => format!("GPU failed: {err}"),
                None => "GPU".to_owned(),
            },
            Backend::Vector(reason) if vector::drawn(drawn) < drawn => format!(
                "vector ({reason}; {} of the triangles drawn)",
                vector::drawn(drawn)
            ),
            Backend::Vector(reason) => format!("vector ({reason})"),
        };
        let cut = self.section.map_or_else(String::new, |s| {
            format!(
                " · cut {} {} {} {unit}",
                s.axis_name(),
                if s.flip { "≥" } else { "≤" },
                number(s.offset)
            )
        });
        format!(
            "{} · {} vertices · {triangles} triangles{size}{cut} · {renderer}",
            self.spec.name,
            mesh.positions.len()
        )
    }

    /// The measuring line: what to do, or the distance.
    pub fn measure_text(&self) -> Option<String> {
        if !self.measuring {
            return None;
        }
        let unit = self.unit();
        let at = |p: V3| format!("({}, {}, {})", number(p[0]), number(p[1]), number(p[2]));
        Some(match self.points.as_slice() {
            [] => "measure: click a point on the model".to_owned(),
            [a] => format!("measure: A {} — click a second point", at(*a)),
            [a, b, ..] => format!(
                "A {} → B {}: {} {unit} (Δx {}, Δy {}, Δz {})",
                at(*a),
                at(*b),
                number(section::distance(*a, *b)),
                number(b[0] - a[0]),
                number(b[1] - a[1]),
                number(b[2] - a[2])
            ),
        })
    }

    fn surface(&self, cx: &App) -> impl IntoElement + use<> {
        let (surface, backend, viewport) = (
            Rc::clone(&self.surface),
            self.backend.clone(),
            Rc::clone(&self.viewport),
        );
        let (mesh, mesh_key, camera) = (Rc::clone(&self.mesh), self.mesh_key, self.camera);
        let marks: Vec<V3> = self
            .points
            .iter()
            .map(|&p| self.placement.to_world(p))
            .collect();
        let mark_color = cx.theme().primary;
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
                let mut painted = false;
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
                        painted = true;
                    }
                }
                if !painted {
                    vector::paint(&mesh, &camera, bounds, window);
                }
                paint_marks(&marks, &camera, bounds, mark_color, window);
            },
        )
        .size_full()
    }
}

/// The measured points as dots joined by a line, over the model.
fn paint_marks(
    marks: &[V3],
    camera: &OrbitCamera,
    bounds: Bounds<Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    let size = (
        f32::from(bounds.size.width).max(1.0),
        f32::from(bounds.size.height).max(1.0),
    );
    let screen: Vec<Point<Pixels>> = marks
        .iter()
        .filter_map(|&p| camera.project(p, size))
        .map(|(x, y)| gpui::point(bounds.origin.x + px(x), bounds.origin.y + px(y)))
        .collect();
    if let [a, b] = screen.as_slice() {
        let mut path = PathBuilder::stroke(px(2.0));
        path.move_to(*a);
        path.line_to(*b);
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
    for at in screen {
        let dot = Bounds::new(
            gpui::point(at.x - px(DOT), at.y - px(DOT)),
            gpui::size(px(DOT * 2.0), px(DOT * 2.0)),
        );
        window.paint_quad(gpui::fill(dot, color).corner_radii(px(DOT)));
    }
}

impl Render for ModelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.height.unwrap_or_else(|| {
            (f32::from(window.viewport_size().height) * MODAL_SHARE - CHROME).max(MIN_HEIGHT)
        });
        let theme = cx.theme();
        let (radius, muted, primary) = (theme.radius, theme.muted_foreground, theme.primary);
        let surface = self.surface(cx);
        let toolbar = self.toolbar(cx);
        let tools = self.tools(cx);
        let cursor_measure = self.measuring;
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
                    .map(|d| {
                        if cursor_measure {
                            d.cursor_crosshair()
                        } else {
                            d.cursor_grab()
                        }
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                    .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_key_down(cx.listener(Self::key_down))
                    .child(surface)
                    .child(tools)
                    .child(toolbar)
                    .child(
                        div()
                            .absolute()
                            .left_2()
                            .bottom_1()
                            .text_xs()
                            .text_color(token(cx, "muted.fg"))
                            .child(
                                "drag: orbit · right/shift-drag: pan · wheel/+-: zoom · \
                                 0: fit · i: iso · u: up axis · s/f/[ ]: cut · m: measure",
                            ),
                    ),
            )
            .when_some(self.measure_text(), |d, text| {
                d.child(
                    div()
                        .debug_selector(|| "model-measure-readout".to_owned())
                        .text_xs()
                        .text_color(primary)
                        .child(text),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(self.info())),
            )
    }
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
    if text.is_empty() || text == "-" || text == "-0" {
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

/// [`model_view`] with a viewport `height` pixels tall, for a block in the
/// timeline.
pub fn model_view_inline(
    id: BlockId,
    spec: &ModelSpec,
    height: f32,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ModelView> {
    let view = model_view(id, spec, window, cx);
    view.update(cx, |view, _| view.set_height(Some(height)));
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
            unit: None,
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
        open_spec(cx, spec())
    }

    fn open_spec(
        cx: &mut TestAppContext,
        spec: ModelSpec,
    ) -> (Entity<ModelView>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sysboard::disable_gpu(cx);
        });
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            view: model_view(BlockId::new(), &spec, window, cx),
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
        assert_eq!(number(-1e-5), "0");
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
            assert!(info.contains("10 × 10 × 25 units"), "{info}");
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

    #[gpui::test]
    fn the_section_plane_cycles_moves_flips_and_cuts(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        let whole = view.read_with(cx, |view, _| view.mesh().vertex_count());
        let camera = view.read_with(cx, |view, _| view.camera());
        view.update(cx, |view, cx| view.cycle_section(cx));
        view.read_with(cx, |view, _| {
            let s = view.section().unwrap();
            assert_eq!(
                (s.axis, s.offset, s.flip),
                (0, 5.0, false),
                "x through the middle"
            );
            assert_ne!(view.mesh().vertex_count(), whole, "cut");
            assert_eq!(view.camera(), camera, "the camera stays");
            assert!(view.info().contains("cut x ≤ 5 units"), "{}", view.info());
        });
        view.update(cx, |view, cx| {
            view.cycle_section(cx);
            view.cycle_section(cx);
            view.move_section(1.0, cx);
            view.flip_section(cx);
        });
        view.read_with(cx, |view, _| {
            let s = view.section().unwrap();
            assert_eq!(s.axis, 2);
            assert!(
                (s.offset - (12.5 + 25.0 / SECTION_STEPS)).abs() < 1e-4,
                "{s:?}"
            );
            assert!(s.flip);
        });
        view.update(cx, |view, cx| view.move_section(100.0, cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.section().unwrap().offset),
            25.0,
            "kept within the model"
        );
        view.update(cx, |view, cx| view.cycle_section(cx));
        view.read_with(cx, |view, _| {
            assert_eq!(view.section(), None);
            assert_eq!(view.mesh().vertex_count(), whole);
        });
        assert!(cx.debug_bounds("model-section").is_some());
    }

    #[gpui::test]
    fn hidden_ranges_leave_triangles_out(cx: &mut TestAppContext) {
        let (view, cx) = open(cx);
        view.update(cx, |view, cx| {
            view.set_hidden(std::iter::once(0..2).collect(), cx)
        });
        assert_eq!(
            view.read_with(cx, |view, _| view.mesh().indices.len()),
            2 * 3
        );
    }

    #[gpui::test]
    fn clicks_measure_between_two_points_on_the_surface(cx: &mut TestAppContext) {
        // A flat 10 × 10 mm square at z = 0, seen from the top (Y up file).
        let square = ModelSpec {
            name: "plate.step".into(),
            mesh: ModelMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 0.0, 10.0],
                    [0.0, 0.0, 10.0],
                ],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
                up: UpAxis::Y,
            },
            unit: Some("mm".into()),
        };
        let (view, cx) = open_spec(cx, square);
        view.update(cx, |view, cx| {
            view.camera.pitch = 1.48;
            view.camera.yaw = 0.0;
            view.set_measuring(true, cx);
        });
        cx.run_until_parked();
        let bounds = view.read_with(cx, |view, _| view.viewport.get());
        let center = bounds.center();
        // Nothing outside the model.
        assert!(
            view.read_with(cx, |view, _| view.pick_at(bounds.origin))
                .is_none()
        );
        cx.simulate_click(center, Modifiers::none());
        let a = view.read_with(cx, |view, _| view.points().to_vec());
        assert_eq!(a.len(), 1, "a click picks a point");
        assert!(
            (a[0][0] - 5.0).abs() < 0.1 && (a[0][2] - 5.0).abs() < 0.1,
            "{a:?}"
        );
        assert!(a[0][1].abs() < 1e-3, "on the surface");
        let right = point(center.x + px(40.0), center.y);
        cx.simulate_click(right, Modifiers::none());
        view.read_with(cx, |view, _| {
            let d = view.distance().unwrap();
            assert!(d > 0.5, "{d}");
            let text = view.measure_text().unwrap();
            assert!(text.contains(" mm (Δx"), "{text}");
        });
        assert!(cx.debug_bounds("model-measure-readout").is_some());
        // A drag orbits and measures nothing.
        cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(right, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(right, MouseButton::Left, Modifiers::none());
        assert_eq!(view.read_with(cx, |view, _| view.points().len()), 2);
        view.update(cx, |view, cx| view.set_measuring(false, cx));
        view.read_with(cx, |view, _| {
            assert!(view.points().is_empty());
            assert_eq!(view.measure_text(), None);
        });
    }
}
