//! The simulation board of a Spec, opened in a full-size modal by
//! `/simulate` and the `simulate_solution` tool.
//!
//! - Centre: the board, drawn by the offscreen [`crate::render3d`] renderer
//!   (isometric preset, drag or `Shift`+arrows to orbit, right-drag,
//!   shift-drag or `w`/`a`/`s`/`d` to pan, wheel or `+`/`-` to zoom, `i`
//!   back to the preset), or by the 2.5D [`vector`] fallback when there is
//!   no GPU adapter. Labels are GPUI text placed at the projected chip
//!   positions. Click a chip, or `Tab`/`Shift+Tab` through them, to
//!   inspect it.
//! - Bottom: the timeline: play/pause (`space`), step to the previous or
//!   next event (`←`/`→`), speed, and a scrub bar with a red marker per
//!   failed assertion. Packet positions are a pure function of the
//!   simulated time, so scrubbing is free.
//! - Right: the selected component (state, variables, queue and its
//!   machine as a Mermaid state diagram with the current state lit) and
//!   the pass/fail list of the scenario; a failure jumps to its
//!   counterexample.
//! - Top: the scenario picker and the seed; changing the seed re-runs the
//!   scenario here (the simulator is pure).

pub mod inspector;
pub mod playback;
pub mod vector;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use domain::block::{Block, BlockId, BlockKind, MermaidSpec};
use domain::modal::ModalId;
use domain::sysmodel::check::run_scenario_with_seed;
use domain::sysmodel::scene::{Frame, Scene, frame_at, scene_of};
use domain::sysmodel::{Board, ScenarioRun};
use gpui::{
    AnyElement, App, Bounds, Context, Corners, FocusHandle, Focusable, FontWeight, Global, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, point, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};

use crate::render3d::camera::OrbitCamera;
use crate::render3d::{RenderStats, Renderer3d};
use crate::timeline::TimelineEntry;
use playback::{Marker, Playback, failure_markers, time_label};
use vector::{IsoProjector, IsoView};

/// Environment variable that turns the 3D renderer off (`off`).
pub const RENDER3D_ENV: &str = "CREST_RENDER3D";
/// Width of the side panel.
const PANEL_WIDTH: f32 = 340.0;
/// Zoom step of one wheel notch.
const ZOOM_STEP: f32 = 1.15;
/// Pixels of drag one keyboard orbit or pan step stands for.
const KEY_STEP: f32 = 40.0;
/// A press that moves less than this is a click.
const CLICK_SLOP: f32 = 4.0;

/// Which renderer draws the board.
#[derive(Clone)]
pub enum Backend {
    /// The offscreen wgpu renderer.
    Gpu(Rc<RefCell<Renderer3d>>),
    /// The 2.5D vector drawing, and why.
    Vector(String),
}

/// The app's renderer: one device, opened on first use.
#[derive(Default)]
struct SharedRenderer {
    tried: bool,
    disabled: bool,
    renderer: Option<Rc<RefCell<Renderer3d>>>,
    error: Option<String>,
}

impl Global for SharedRenderer {}

/// Keeps every board of this app on the 2.5D fallback (tests).
pub fn disable_gpu(cx: &mut App) {
    cx.default_global::<SharedRenderer>().disabled = true;
}

/// The 3D renderer if an adapter is available, else the fallback with the
/// reason.
pub fn auto_backend(cx: &mut App) -> Backend {
    let off = std::env::var(RENDER3D_ENV).is_ok_and(|v| v.eq_ignore_ascii_case("off"));
    let shared = cx.default_global::<SharedRenderer>();
    if shared.disabled || off {
        return Backend::Vector("3D rendering is off".into());
    }
    if !shared.tried {
        shared.tried = true;
        match Renderer3d::new() {
            Ok(r) => shared.renderer = Some(Rc::new(RefCell::new(r))),
            Err(e) => shared.error = Some(e.to_string()),
        }
    }
    match (&shared.renderer, &shared.error) {
        (Some(r), _) => Backend::Gpu(Rc::clone(r)),
        (None, Some(e)) => Backend::Vector(e.clone()),
        (None, None) => Backend::Vector("no renderer".into()),
    }
}

/// What a rendered GPU frame showed; a new key renders a new frame.
#[derive(Debug, Clone, PartialEq)]
struct FrameKey {
    background: domain::theme::Color,
    size: (u32, u32),
    scenario: usize,
    seed: u64,
    t_us: u64,
    camera: [u32; 7],
    selected: Option<String>,
}

#[derive(Default)]
struct Surface {
    image: Option<Arc<RenderImage>>,
    key: Option<FrameKey>,
    retired: Vec<Arc<RenderImage>>,
    stats: Option<RenderStats>,
    frames: u64,
    failed: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    start: Point<Pixels>,
    last: Point<Pixels>,
    pan: bool,
}

/// The board view.
pub struct SysBoardView {
    board: Board,
    runs: Vec<ScenarioRun>,
    scene: Rc<Scene>,
    scenario: usize,
    playback: Playback,
    selected: Option<String>,
    backend: Backend,
    camera: OrbitCamera,
    iso: IsoView,
    surface: Rc<RefCell<Surface>>,
    board_bounds: Rc<Cell<Bounds<Pixels>>>,
    scrub_bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    scrubbing: bool,
    last_tick: Option<Instant>,
    focus: FocusHandle,
}

fn f(v: Pixels) -> f32 {
    f32::from(v)
}

fn rgb(c: [f32; 3]) -> Hsla {
    gpui::Rgba {
        r: c[0].clamp(0.0, 1.0).sqrt(),
        g: c[1].clamp(0.0, 1.0).sqrt(),
        b: c[2].clamp(0.0, 1.0).sqrt(),
        a: 1.0,
    }
    .into()
}

impl SysBoardView {
    /// A board drawn by the best available renderer.
    pub fn new(board: Board, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = auto_backend(cx);
        Self::with_backend(board, backend, window, cx)
    }

    /// A board drawn by `backend`.
    pub fn with_backend(
        board: Board,
        backend: Backend,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let scene = Rc::new(scene_of(&board.model));
        let camera = OrbitCamera::fit(&scene);
        let runs = board.runs.clone();
        let playback = runs
            .first()
            .map_or_else(|| Playback::new(1), |r| Playback::for_trace(&r.trace));
        Self {
            board,
            runs,
            camera,
            scene,
            scenario: 0,
            playback,
            selected: None,
            backend,
            iso: IsoView::default(),
            surface: Rc::default(),
            board_bounds: Rc::default(),
            scrub_bounds: Rc::default(),
            drag: None,
            scrubbing: false,
            last_tick: None,
            focus: cx.focus_handle(),
        }
    }

    /// `3D · <adapter>` or `2.5D · <reason>`.
    pub fn backend_name(&self) -> String {
        match &self.backend {
            Backend::Gpu(r) => format!("3D · {}", r.borrow().adapter()),
            Backend::Vector(reason) => format!("2.5D · {reason}"),
        }
    }

    /// Whether the GPU renderer draws the board.
    pub fn is_gpu(&self) -> bool {
        matches!(self.backend, Backend::Gpu(_))
    }

    /// The board shown.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The run of the shown scenario.
    pub fn run(&self) -> Option<&ScenarioRun> {
        self.runs.get(self.scenario)
    }

    /// The timeline.
    pub fn playback(&self) -> &Playback {
        &self.playback
    }

    /// The selected component.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Cost of the last GPU frame and frames rendered so far.
    pub fn render_stats(&self) -> (Option<RenderStats>, u64) {
        let s = self.surface.borrow();
        (s.stats, s.frames)
    }

    /// The camera of the 3D view.
    pub fn camera(&self) -> OrbitCamera {
        self.camera
    }

    /// Moving parts at the current time.
    pub fn frame(&self) -> Frame {
        match self.run() {
            Some(run) => frame_at(
                &self.board.model,
                &self.scene,
                &run.trace,
                self.playback.t_us,
            ),
            None => Frame {
                t_us: 0,
                packets: Vec::new(),
                chips: Vec::new(),
            },
        }
    }

    /// Red markers of the shown scenario.
    pub fn markers(&self) -> Vec<Marker> {
        self.run().map(failure_markers).unwrap_or_default()
    }

    /// Shows scenario `ix` from its start.
    pub fn select_scenario(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.runs.len() {
            self.scenario = ix;
            self.playback = Playback::for_trace(&self.runs[ix].trace);
            self.last_tick = None;
            cx.notify();
        }
    }

    /// Re-runs the shown scenario with its seed moved by `delta`.
    pub fn reseed(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get(self.scenario) else {
            return;
        };
        let Some(scenario) = self.board.model.scenario(&run.scenario).cloned() else {
            return;
        };
        let seed = run.trace.seed.wrapping_add_signed(delta);
        if let Ok(new) = run_scenario_with_seed(&self.board.model, &scenario, seed) {
            let t = self.playback.t_us;
            // The new run has other events: pace by them.
            let mut playback = Playback::for_trace(&new.trace);
            playback.seek(t);
            self.runs[self.scenario] = new;
            self.playback = playback;
            cx.notify();
        }
    }

    /// Play or pause.
    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        self.playback.toggle();
        self.last_tick = None;
        cx.notify();
    }

    /// Jumps to the next event.
    pub fn step_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.get(self.scenario) {
            self.playback.step_forward(&run.trace);
            cx.notify();
        }
    }

    /// Jumps to the previous event.
    pub fn step_back(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.get(self.scenario) {
            self.playback.step_back(&run.trace);
            cx.notify();
        }
    }

    /// Jumps to `fraction` of the scenario.
    pub fn scrub(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.playback.scrub(fraction);
        cx.notify();
    }

    /// Jumps to `t_us`, paused.
    pub fn seek(&mut self, t_us: u64, cx: &mut Context<Self>) {
        self.playback.playing = false;
        self.playback.seek(t_us);
        cx.notify();
    }

    /// Selects component `id` (or nothing).
    pub fn select(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        self.selected = id
            .filter(|id| self.board.model.component(id).is_some())
            .map(str::to_owned);
        cx.notify();
    }

    fn board_size(&self) -> (f32, f32) {
        let b = self.board_bounds.get();
        (f(b.size.width).max(1.0), f(b.size.height).max(1.0))
    }

    /// Pixel position of a world point in the board viewport.
    fn project(&self, p: [f32; 3]) -> Option<(f32, f32)> {
        let size = self.board_size();
        match self.backend {
            Backend::Gpu(_) => self.camera.project(p, size),
            Backend::Vector(_) => Some(IsoProjector::new(&self.scene, size, self.iso).project(p)),
        }
    }

    /// The chip under viewport pixel `at`.
    pub fn pick(&self, at: (f32, f32)) -> Option<String> {
        let size = self.board_size();
        match self.backend {
            Backend::Gpu(_) => {
                let (origin, dir) = self.camera.ray(at.0, at.1, size)?;
                crate::render3d::pick::pick(&self.scene, origin, dir).map(str::to_owned)
            }
            Backend::Vector(_) => {
                let proj = IsoProjector::new(&self.scene, size, self.iso);
                vector::pick(&self.scene, &proj, at).map(str::to_owned)
            }
        }
    }

    fn local(&self, p: Point<Pixels>) -> (f32, f32) {
        let o = self.board_bounds.get().origin;
        (f(p.x - o.x), f(p.y - o.y))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let pan = event.button != MouseButton::Left || event.modifiers.shift;
        self.drag = Some(Drag {
            start: event.position,
            last: event.position,
            pan,
        });
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag else { return };
        if event.pressed_button.is_none() {
            self.drag = None;
            return;
        }
        let d = event.position - drag.last;
        let (dx, dy) = (f(d.x), f(d.y));
        let size = self.board_size();
        match (&self.backend, drag.pan) {
            (Backend::Gpu(_), false) => self.camera.orbit(dx, dy),
            (Backend::Gpu(_), true) => self.camera.pan(dx, dy, size),
            (Backend::Vector(_), _) => self.iso.pan = (self.iso.pan.0 + dx, self.iso.pan.1 + dy),
        }
        drag.last = event.position;
        self.drag = Some(drag);
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        let moved = event.position - drag.start;
        if f(moved.x).abs() < CLICK_SLOP && f(moved.y).abs() < CLICK_SLOP {
            let hit = self.pick(self.local(event.position));
            self.selected = hit;
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f(event.delta.pixel_delta(px(20.0)).y);
        let factor = ZOOM_STEP.powf(dy / 20.0);
        match self.backend {
            Backend::Gpu(_) => self.camera.zoom_by(factor),
            Backend::Vector(_) => self.iso.zoom_by(factor),
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Orbits (or pans, with `pan`) the board by a keyboard step.
    fn nudge(&mut self, dx: f32, dy: f32, pan: bool, cx: &mut Context<Self>) {
        let size = self.board_size();
        match (&self.backend, pan) {
            (Backend::Gpu(_), false) => self.camera.orbit(dx, dy),
            (Backend::Gpu(_), true) => self.camera.pan(dx, dy, size),
            (Backend::Vector(_), _) => self.iso.pan = (self.iso.pan.0 + dx, self.iso.pan.1 + dy),
        }
        cx.notify();
    }

    /// Selects the next (or previous) component, in model order.
    fn cycle_selection(&mut self, back: bool, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .board
            .model
            .components
            .iter()
            .map(|c| c.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let len = ids.len();
        let at = self
            .selected
            .as_ref()
            .and_then(|s| ids.iter().position(|id| id == s));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => len - 1,
            (Some(ix), false) => (ix + 1) % len,
            (Some(ix), true) => (ix + len - 1) % len,
        };
        self.select(Some(&ids[next]), cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        if k.modifiers.shift {
            match k.key.as_str() {
                "left" => self.nudge(-KEY_STEP, 0.0, false, cx),
                "right" => self.nudge(KEY_STEP, 0.0, false, cx),
                "up" => self.nudge(0.0, -KEY_STEP, false, cx),
                "down" => self.nudge(0.0, KEY_STEP, false, cx),
                "tab" => self.cycle_selection(true, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        match k.key.as_str() {
            "w" => self.nudge(0.0, -KEY_STEP, true, cx),
            "s" => self.nudge(0.0, KEY_STEP, true, cx),
            "a" => self.nudge(-KEY_STEP, 0.0, true, cx),
            "d" => self.nudge(KEY_STEP, 0.0, true, cx),
            "tab" => self.cycle_selection(false, cx),
            "space" => self.toggle_play(cx),
            "right" => self.step_forward(cx),
            "left" => self.step_back(cx),
            "i" => {
                self.camera.iso();
                self.iso = IsoView::default();
                cx.notify();
            }
            "+" | "=" => {
                self.camera.zoom_by(ZOOM_STEP);
                self.iso.zoom_by(ZOOM_STEP);
                cx.notify();
            }
            "-" => {
                self.camera.zoom_by(1.0 / ZOOM_STEP);
                self.iso.zoom_by(1.0 / ZOOM_STEP);
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn scrub_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let b = self.scrub_bounds.get();
        let w = f(b.size.width);
        if w > 0.0 {
            self.scrub(f(x - b.origin.x) / w, cx);
        }
    }

    fn tick(&mut self, window: &mut Window) {
        if self.playback.playing {
            let now = Instant::now();
            if let Some(last) = self.last_tick {
                self.playback.advance(now - last);
            }
            self.last_tick = Some(now);
            window.request_animation_frame();
        } else {
            self.last_tick = None;
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let passed = self.runs.iter().filter(|r| r.passed).count();
        let mut row = h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border);
        for (ix, run) in self.runs.iter().enumerate() {
            let mark = if run.passed { "✓" } else { "✗" };
            let button = Button::new(("sysboard-scenario", ix))
                .small()
                .label(format!("{mark} {}", run.scenario))
                .selected(ix == self.scenario);
            let button = if run.passed {
                button.ghost()
            } else {
                button.danger()
            };
            row = row.child(
                button.on_click(cx.listener(move |this, _, _, cx| this.select_scenario(ix, cx))),
            );
        }
        let seed = self.run().map_or(0, |r| r.trace.seed);
        row.child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!(
                        "{passed}/{} pass · seed {seed}",
                        self.runs.len()
                    ))),
            )
            .child(
                Button::new("sysboard-seed-down")
                    .ghost()
                    .xsmall()
                    .label("−")
                    .on_click(cx.listener(|this, _, _, cx| this.reseed(-1, cx))),
            )
            .child(
                Button::new("sysboard-seed-up")
                    .ghost()
                    .xsmall()
                    .label("+")
                    .on_click(cx.listener(|this, _, _, cx| this.reseed(1, cx))),
            )
            .child(
                Button::new("sysboard-iso")
                    .ghost()
                    .xsmall()
                    .label("iso")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.camera.iso();
                        this.iso = IsoView::default();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(self.backend_name())),
            )
            .into_any_element()
    }

    fn render_board(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(reason) = self.surface.borrow().failed.clone()
            && self.is_gpu()
        {
            self.backend = Backend::Vector(reason);
        }
        let frame = self.frame();
        let scene = Rc::clone(&self.scene);
        let selected = self.selected.clone();
        let bounds_cell = Rc::clone(&self.board_bounds);
        let surface = Rc::clone(&self.surface);
        let backend = self.backend.clone();
        let camera = self.camera;
        let iso = self.iso;
        let key_base = FrameKey {
            background: crate::theme::crest_theme(cx).color("canvas.bg"),
            size: (0, 0),
            scenario: self.scenario,
            seed: self.run().map_or(0, |r| r.trace.seed),
            t_us: self.playback.t_us,
            camera: [
                camera.yaw.to_bits(),
                camera.pitch.to_bits(),
                camera.zoom.to_bits(),
                camera.target[0].to_bits(),
                camera.target[1].to_bits(),
                camera.target[2].to_bits(),
                u32::from(camera.ortho),
            ],
            selected: selected.clone(),
        };
        let surface_canvas = canvas(
            move |bounds, window, _| {
                if bounds_cell.get() != bounds {
                    bounds_cell.set(bounds);
                    // Labels are placed with the size of the last paint.
                    window.refresh();
                }
            },
            move |bounds, (), window, _| {
                let size = (f(bounds.size.width), f(bounds.size.height));
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
                        ..key_base
                    };
                    if s.key.as_ref() != Some(&key) {
                        renderer.borrow_mut().set_background(key.background);
                        let result = renderer.borrow_mut().render(
                            &scene,
                            &frame,
                            &camera,
                            selected.as_deref(),
                            px_size,
                        );
                        match result {
                            Ok(r) => {
                                if let Some(old) = s.image.replace(r.image) {
                                    s.retired.push(old);
                                }
                                s.key = Some(key);
                                s.stats = Some(r.stats);
                                s.frames += 1;
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
                let proj = IsoProjector::new(&scene, size, iso);
                vector::paint(&scene, &frame, selected.as_deref(), proj, bounds, window);
            },
        )
        .size_full();
        let theme = cx.theme();
        let looks = self.frame().chips;
        let label_color = crate::theme::token(cx, "foreground");
        let labels: Vec<AnyElement> = self
            .scene
            .labels
            .iter()
            .filter_map(|label| {
                let (x, y) = self.project(label.at)?;
                let state = looks.iter().find(|l| l.id == label.chip);
                let sub = state.map(|l| l.state.clone()).unwrap_or_default();
                let glow = state.map_or(theme.muted_foreground, |l| rgb(l.emissive));
                Some(
                    v_flex()
                        .absolute()
                        .left(px(x - 70.0))
                        .top(px(y - 30.0))
                        .w(px(140.0))
                        .items_center()
                        .debug_selector(|| "sysboard-label".to_owned())
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(label_color)
                                .child(SharedString::from(label.text.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(glow)
                                .child(SharedString::from(sub)),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        self.tick(window);
        div()
            .id("sysboard-board")
            .debug_selector(|| "sysboard-canvas".to_owned())
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(surface_canvas)
            .children(labels)
            .child(
                div()
                    .absolute()
                    .left_2()
                    .bottom_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        "drag/Shift+arrows: orbit · right-drag/wasd: pan · wheel/+-: zoom · \
                         i: iso · click/Tab: inspect",
                    ),
            )
            .into_any_element()
    }

    fn render_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut panel = v_flex()
            .id("sysboard-panel")
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .gap_2()
            .p_2()
            .border_l_1()
            .border_color(theme.border);
        let snap = self.run().map(|r| r.trace.snapshot_at(self.playback.t_us));
        match self
            .selected
            .as_deref()
            .and_then(|id| self.board.model.component(id))
        {
            None => {
                panel = panel.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Click a chip to inspect it."),
                );
            }
            Some(comp) => {
                let state = snap
                    .as_ref()
                    .and_then(|s| s.components.get(&comp.id))
                    .cloned();
                let mut section = v_flex()
                    .gap_1()
                    .debug_selector(|| "sysboard-inspector".to_owned())
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from(format!(
                                "{} · {}",
                                comp.name(),
                                comp.kind.as_str()
                            ))),
                    );
                if let Some(state) = &state {
                    section = section.child(
                        div()
                            .text_sm()
                            .child(SharedString::from(inspector::status_line(state))),
                    );
                    for (var, value) in &state.vars {
                        section = section.child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .child(SharedString::from(format!("{var} = {value}"))),
                        );
                    }
                    if !state.queue.is_empty() {
                        let kinds: Vec<&str> =
                            state.queue.iter().map(|q| q.kind.as_str()).collect();
                        section = section.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(format!("queue: {}", kinds.join(", ")))),
                        );
                    }
                }
                let links = inspector::connections(&self.scene, &comp.id);
                if !links.is_empty() {
                    section = section.child(
                        div()
                            .pt_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Connected to (lit on the board)"),
                    );
                    for link in links {
                        section = section.child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .child(SharedString::from(link)),
                        );
                    }
                }
                if let Some(run) = self.run() {
                    let recent = inspector::activity(&run.trace, &comp.id, self.playback.t_us, 6);
                    if !recent.is_empty() {
                        section = section.child(
                            div()
                                .pt_1()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Last activity"),
                        );
                        for line in recent {
                            section = section.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(line)),
                            );
                        }
                    }
                }
                if !comp.cites.is_empty() {
                    section =
                        section.child(div().text_xs().text_color(theme.muted_foreground).child(
                            SharedString::from(format!("cites {}", comp.cites.join(", "))),
                        ));
                }
                let source =
                    inspector::machine_diagram(comp, state.as_ref().map(|s| s.state.as_str()));
                let entry = TimelineEntry {
                    block: Block {
                        id: machine_block_id(&self.board.spec, &comp.id),
                        kind: BlockKind::Mermaid(MermaidSpec {
                            source,
                            title: Some(format!("{} machine", comp.id)),
                        }),
                    },
                    expanded: true,
                    alt_view: false,
                    numbers: Default::default(),
                };
                section = section.child(crate::blocks::mermaid::render(&entry, window, cx));
                panel = panel.child(section);
            }
        }
        let theme = cx.theme();
        let mut checks = v_flex()
            .gap_1()
            .debug_selector(|| "sysboard-checks".to_owned())
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Checks"));
        if let Some(run) = self.run() {
            for (ix, v) in run.verdicts.iter().enumerate() {
                let color = if v.passed {
                    theme.success
                } else {
                    theme.danger
                };
                let at = v.counterexample.as_ref().map(|c| c.at_us);
                let mut row =
                    v_flex()
                        .id(("sysboard-check", ix))
                        .p_1()
                        .rounded_sm()
                        .hover(|s| s.bg(theme.secondary))
                        .child(div().text_xs().text_color(color).child(SharedString::from(
                            format!("{} {}", if v.passed { "✓" } else { "✗" }, v.property),
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(v.detail.clone())),
                        );
                if let Some(at) = at {
                    row = row
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.seek(at, cx);
                        }));
                }
                checks = checks.child(row);
            }
        }
        panel.child(checks).into_any_element()
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let p = self.playback.clone();
        let markers = self.markers();
        let bounds_cell = Rc::clone(&self.scrub_bounds);
        let (track, head, danger) = (theme.secondary, theme.primary, theme.danger);
        let fraction = p.fraction();
        let marks: Vec<f32> = markers.iter().map(|m| m.fraction).collect();
        let bar = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, _| {
                let (x, y) = (bounds.origin.x, bounds.origin.y);
                let (w, h) = (bounds.size.width, bounds.size.height);
                let mid = y + h / 2.0;
                window.paint_quad(gpui::fill(
                    Bounds::new(point(x, mid - px(2.0)), gpui::size(w, px(4.0))),
                    track,
                ));
                window.paint_quad(gpui::fill(
                    Bounds::new(point(x, mid - px(2.0)), gpui::size(w * fraction, px(4.0))),
                    head,
                ));
                for m in &marks {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            point(x + w * *m - px(1.5), y + px(2.0)),
                            gpui::size(px(3.0), h - px(4.0)),
                        ),
                        danger,
                    ));
                }
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        point(x + w * fraction - px(5.0), mid - px(5.0)),
                        gpui::size(px(10.0), px(10.0)),
                    ),
                    head,
                ));
            },
        )
        .size_full();
        h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new("sysboard-back")
                    .ghost()
                    .small()
                    .label("⏮")
                    .on_click(cx.listener(|this, _, _, cx| this.step_back(cx))),
            )
            .child(
                Button::new("sysboard-play")
                    .small()
                    .label(if p.playing { "⏸" } else { "▶" })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx))),
            )
            .child(
                Button::new("sysboard-forward")
                    .ghost()
                    .small()
                    .label("⏭")
                    .on_click(cx.listener(|this, _, _, cx| this.step_forward(cx))),
            )
            .child(
                Button::new("sysboard-slower")
                    .ghost()
                    .xsmall()
                    .label("−")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.playback.slower();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .w(px(36.0))
                    .child(SharedString::from(format!("{}×", p.speed()))),
            )
            .child(
                Button::new("sysboard-faster")
                    .ghost()
                    .xsmall()
                    .label("+")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.playback.faster();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .w(px(190.0))
                    .debug_selector(|| "sysboard-time".to_owned())
                    .child(SharedString::from(format!(
                        "{} / {}",
                        time_label(p.t_us),
                        time_label(p.until_us)
                    ))),
            )
            .child(
                div()
                    .id("sysboard-scrub")
                    .debug_selector(|| "sysboard-scrub".to_owned())
                    .flex_1()
                    .h(px(22.0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.scrubbing = true;
                            this.scrub_at(e.position.x, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                        if this.scrubbing && e.pressed_button == Some(MouseButton::Left) {
                            this.scrub_at(e.position.x, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.scrubbing = false),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.scrubbing = false),
                    )
                    .child(bar),
            )
            .into_any_element()
    }
}

/// Stable block id of a component's machine diagram.
fn machine_block_id(spec: &str, comp: &str) -> BlockId {
    crate::modal::modal_node_id(&ModalId::new(format!("sysboard/{spec}/{comp}")))
}

impl Focusable for SysBoardView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SysBoardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx);
        let board = self.render_board(window, cx);
        let panel = self.render_panel(window, cx);
        let timeline = self.render_timeline(cx);
        v_flex()
            .id("sysboard")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(board)
                    .child(panel),
            )
            .child(timeline)
    }
}
