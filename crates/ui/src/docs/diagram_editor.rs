//! The diagram editor: replaces the page while a diagram is written or
//! changed. The drawing is on the left (or on top when the panel is narrow)
//! and its Mermaid code on the right, redrawn 250 ms after the last key.
//!
//! The drawing is the block renderer's own: the code is laid out by
//! [`diagram::parse_with`] and painted by [`paint::paint`], the pieces of
//! `blocks::mermaid`. What this view adds is the viewport the editor needs:
//! "fit" (the default, follows the pane in both directions), a percentage
//! button that goes to 100 %, steps from 25 to 400 %, ctrl/cmd + wheel up
//! to 800 % around the cursor, and drag to pan a drawing larger than the
//! pane.
//!
//! Code that does not parse shows the parser's reason and the code, and
//! Save still keeps it. The splitter between the panes is 7 px, 15 to 85 %,
//! moved with the arrows (5 % a step), reset to 50 % by a double-click, and
//! remembered between openings ([`SplitMemory`]).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Global, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas,
    div, px, relative,
};
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};

use crate::blocks::mermaid::diagram::{self, Diagram};
use crate::blocks::mermaid::paint::{self, Camera};
use crate::kit::tool_button;

/// Quiet time after the last key before the drawing is made again.
/// Action names of the editor's buttons.
pub const SAVE_ACTION: &str = "diagram.save";
pub const CANCEL_ACTION: &str = "diagram.cancel";
pub const ZOOM_IN_ACTION: &str = "diagram.zoom_in";
pub const ZOOM_OUT_ACTION: &str = "diagram.zoom_out";
pub const ZOOM_FIT_ACTION: &str = "diagram.zoom_fit";

pub const LIVE_AFTER_MS: u64 = 250;
/// The zoom steps of the buttons and keys, as scales of the drawing's size.
pub const ZOOM_STEPS: [f32; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// Smallest and largest zoom (the wheel reaches 800 %).
pub const ZOOM_MIN: f32 = 0.1;
/// See [`ZOOM_MIN`].
pub const ZOOM_MAX: f32 = 8.0;
/// Largest scale "fit" gives a small drawing.
const FIT_MAX: f32 = 3.0;
/// Room "fit" leaves around the drawing, in pixels.
const FIT_PAD: f32 = 24.0;
/// Smallest and largest share of the splitter, in percent.
pub const SPLIT_MIN: f32 = 15.0;
/// See [`SPLIT_MIN`].
pub const SPLIT_MAX: f32 = 85.0;
/// Arrow-key step of the splitter, in percent.
pub const SPLIT_STEP: f32 = 5.0;
/// Width of the splitter bar.
pub const SPLITTER_PX: f32 = 7.0;
/// Panels narrower than this stack the panes.
pub const STACK_BELOW: f32 = 560.0;
/// Characters of a source that are drawn; the rest is kept but not drawn.
/// Same limit as the block renderer's.
pub const MAX_SOURCE_CHARS: usize = 8_000;

/// The splitter's share, kept between openings of the editor.
#[derive(Debug, Clone, Copy)]
pub struct SplitMemory(pub f32);

impl Global for SplitMemory {}

/// Keeps a splitter share within [`SPLIT_MIN`] and [`SPLIT_MAX`].
#[must_use]
pub fn clamp_split_percent(percent: f32) -> f32 {
    if percent.is_finite() {
        percent.clamp(SPLIT_MIN, SPLIT_MAX)
    } else {
        50.0
    }
}

/// The next zoom step from scale `current`: up (`dir > 0`) or down. The
/// ends stay where they are.
#[must_use]
pub fn zoom_step(current: f32, dir: i32) -> f32 {
    let first = ZOOM_STEPS[0];
    let last = ZOOM_STEPS[ZOOM_STEPS.len() - 1];
    if dir > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|z| *z > current + 0.001)
            .unwrap_or(last.max(current))
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|z| *z < current - 0.001)
            .unwrap_or(first.min(current))
    }
}

/// What the editor is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramSpec {
    /// The `index`th diagram of the doc.
    Edit {
        /// Position among the doc's diagrams.
        index: usize,
    },
    /// A new diagram after top-level block `after` (`None`: at the end).
    New {
        /// Block the diagram goes after.
        after: Option<usize>,
    },
}

/// What the editor asks of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramEvent {
    /// Save this code into the doc (may be empty).
    Save(String),
    /// Leave with the doc as it was.
    Cancel,
}

/// The scale and origin the drawing has in a pane of `size` for a zoom of
/// `scale` (`None` is "fit") panned by `pan`.
fn camera_for(diagram: &Diagram, size: (f32, f32), scale: Option<f32>, pan: (f32, f32)) -> Camera {
    let fit = Camera::fit(diagram, size).max(1e-3);
    let s = scale.unwrap_or_else(|| fit_scale(diagram, size));
    Camera {
        zoom: s / fit,
        pan: if scale.is_some() { pan } else { (0.0, 0.0) },
    }
}

/// The scale that shows the whole drawing in a pane of `size`.
fn fit_scale(diagram: &Diagram, size: (f32, f32)) -> f32 {
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return 1.0;
    }
    let w = diagram.width + diagram.padding * 2.0;
    let h = diagram.height + diagram.padding * 2.0;
    ((size.0 - FIT_PAD) / w)
        .min((size.1 - FIT_PAD) / h)
        .clamp(ZOOM_MIN, FIT_MAX)
}

/// The diagram editor view.
pub struct DiagramEditor {
    spec: DiagramSpec,
    code: Entity<TextareaState>,
    focus: FocusHandle,
    split_focus: FocusHandle,
    /// The source the layout in flight or on screen is of.
    drawn: String,
    diagram: Option<Result<Rc<Diagram>, String>>,
    colors: crate::app::ColorTheme,
    layout: Option<Task<()>>,
    redraw: Option<Task<()>>,
    cut: bool,
    /// Explicit zoom, or `None` for "fit".
    scale: Option<f32>,
    pan: (f32, f32),
    drag: Option<gpui::Point<Pixels>>,
    pane_bounds: Rc<Cell<Bounds<Pixels>>>,
    body_bounds: Rc<Cell<Bounds<Pixels>>>,
    split: f32,
    split_drag: bool,
    _changes: Subscription,
}

impl EventEmitter<DiagramEvent> for DiagramEditor {}

impl Focusable for DiagramEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DiagramEditor {
    /// An editor for `spec` that starts with `initial` as its code.
    pub fn new(
        spec: DiagramSpec,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let code = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(false));
        code.update(cx, |state, cx| {
            state.set_value(initial.to_owned(), window, cx)
        });
        let changes = cx.subscribe(&code, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_redraw(cx);
            }
        });
        let split = cx
            .try_global::<SplitMemory>()
            .map_or(50.0, |memory| clamp_split_percent(memory.0));
        let mut editor = Self {
            spec,
            code,
            focus: cx.focus_handle(),
            split_focus: cx.focus_handle(),
            drawn: String::new(),
            diagram: None,
            colors: crate::theme::crest_theme(cx),
            layout: None,
            redraw: None,
            cut: false,
            scale: None,
            pan: (0.0, 0.0),
            drag: None,
            pane_bounds: Rc::default(),
            body_bounds: Rc::default(),
            split,
            split_drag: false,
            _changes: changes,
        };
        editor.redraw_now(cx);
        editor
    }

    /// What the editor is for.
    #[must_use]
    pub fn spec(&self) -> DiagramSpec {
        self.spec
    }

    /// The code as typed.
    #[must_use]
    pub fn source(&self, cx: &App) -> String {
        self.code.read(cx).value().to_string()
    }

    /// Replaces the code (a template) and draws it at once.
    pub fn set_code(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.code.update(cx, |state, cx| {
            state.replace_all(text.to_owned(), window, cx)
        });
        self.redraw_now(cx);
        self.code.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// The code editor's focus handle.
    #[must_use]
    pub fn code_focus(&self, cx: &App) -> FocusHandle {
        self.code.read(cx).focus_handle(cx)
    }

    /// The splitter's share, in percent.
    #[must_use]
    pub fn split(&self) -> f32 {
        self.split
    }

    /// Moves the splitter to `percent` (kept within 15-85) and remembers it.
    pub fn set_split(&mut self, percent: f32, cx: &mut Context<Self>) {
        self.split = clamp_split_percent(percent);
        cx.set_global(SplitMemory(self.split));
        cx.notify();
    }

    /// Whether the code is longer than what is drawn.
    #[must_use]
    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// The parse error of the drawn code, if it does not parse.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.diagram.as_ref()?.as_ref().err().map(String::as_str)
    }

    /// Whether a layout is running or a redraw is waiting for its timer.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.layout.is_some() || self.redraw.is_some()
    }

    /// The zoom as a scale of the drawing's size, `None` while it fits.
    #[must_use]
    pub fn zoom(&self) -> Option<f32> {
        self.scale
    }

    fn schedule_redraw(&mut self, cx: &mut Context<Self>) {
        self.redraw = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LIVE_AFTER_MS))
                .await;
            this.update(cx, |this, cx| {
                this.redraw = None;
                this.redraw_now(cx);
            })
            .ok();
        }));
    }

    /// Lays the code out again when it changed.
    fn redraw_now(&mut self, cx: &mut Context<Self>) {
        let source = self.source(cx);
        if source == self.drawn && self.diagram.is_some() {
            return;
        }
        self.drawn.clone_from(&source);
        self.cut = source.chars().count() > MAX_SOURCE_CHARS;
        self.start_layout(cx);
        cx.notify();
    }

    fn start_layout(&mut self, cx: &mut Context<Self>) {
        let source: String = self.drawn.chars().take(MAX_SOURCE_CHARS).collect();
        let colors = self.colors.clone();
        let task = cx
            .background_executor()
            .spawn(async move { diagram::parse_with(&source, &colors) });
        self.layout = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.diagram = Some(result.map(Rc::new));
                this.layout = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn pane_size(&self) -> (f32, f32) {
        let size = self.pane_bounds.get().size;
        (f32::from(size.width), f32::from(size.height))
    }

    fn drawing(&self) -> Option<Rc<Diagram>> {
        match &self.diagram {
            Some(Ok(d)) => Some(d.clone()),
            _ => None,
        }
    }

    /// The scale the drawing has now.
    #[must_use]
    pub fn effective_scale(&self) -> Option<f32> {
        let d = self.drawing()?;
        let size = self.pane_size();
        Some(self.scale.unwrap_or_else(|| fit_scale(&d, size)))
    }

    /// Zooms to scale `target` around `anchor` (window coordinates) or the
    /// middle of the pane.
    pub fn zoom_to(
        &mut self,
        target: f32,
        anchor: Option<gpui::Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = self.drawing() else { return };
        let size = self.pane_size();
        let bounds = self.pane_bounds.get();
        let anchor = anchor.map_or((size.0 / 2.0, size.1 / 2.0), |p| {
            (
                f32::from(p.x - bounds.origin.x),
                f32::from(p.y - bounds.origin.y),
            )
        });
        let old = camera_for(&d, size, self.scale, self.pan);
        let (old_scale, old_origin) = (old.scale(&d, size), old.origin(&d, size));
        let under = (
            (anchor.0 - old_origin.0) / old_scale,
            (anchor.1 - old_origin.1) / old_scale,
        );
        let target = target.clamp(ZOOM_MIN, ZOOM_MAX);
        let base = Camera {
            zoom: target / Camera::fit(&d, size).max(1e-3),
            pan: (0.0, 0.0),
        }
        .origin(&d, size);
        self.pan = (
            anchor.0 - base.0 - under.0 * target,
            anchor.1 - base.1 - under.1 * target,
        );
        self.scale = Some(target);
        cx.notify();
    }

    /// One zoom step up or down.
    pub fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        if let Some(current) = self.effective_scale() {
            self.zoom_to(zoom_step(current, dir), None, cx);
        }
    }

    /// Back to "fit".
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.scale = None;
        self.pan = (0.0, 0.0);
        cx.notify();
    }

    /// Whether the zoomed drawing is larger than the pane.
    fn overflows(&self) -> bool {
        let (Some(d), Some(scale)) = (self.drawing(), self.scale) else {
            return false;
        };
        let size = self.pane_size();
        (d.width + d.padding * 2.0) * scale > size.0
            || (d.height + d.padding * 2.0) * scale > size.1
    }

    fn pane_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if event.click_count == 2 {
            self.fit(cx);
        } else if self.overflows() {
            self.drag = Some(event.position);
        }
    }

    fn pane_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let delta = event.position - last;
        self.pan = (
            self.pan.0 + f32::from(delta.x),
            self.pan.1 + f32::from(delta.y),
        );
        self.drag = Some(event.position);
        cx.notify();
    }

    fn pane_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let Some(current) = self.effective_scale() else {
            return;
        };
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        self.zoom_to(current * (dy * 0.01).exp(), Some(event.position), cx);
        cx.stop_propagation();
    }

    fn splitter_percent(&self, position: gpui::Point<Pixels>, stacked: bool) -> f32 {
        let bounds = self.body_bounds.get();
        let (offset, extent) = if stacked {
            (position.y - bounds.origin.y, bounds.size.height)
        } else {
            (position.x - bounds.origin.x, bounds.size.width)
        };
        if f32::from(extent) <= 0.0 {
            return self.split;
        }
        f32::from(offset) / f32::from(extent) * 100.0
    }

    fn is_stacked(&self) -> bool {
        let width = f32::from(self.body_bounds.get().size.width);
        width > 0.0 && width < STACK_BELOW
    }

    /// Puts the code in the doc and leaves.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        cx.emit(DiagramEvent::Save(self.source(cx)));
    }

    /// Leaves with the doc as it was.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(DiagramEvent::Cancel);
    }

    /// Runs one of the editor's named actions (`diagram.save`, `.cancel`,
    /// `.zoom_in`, `.zoom_out`, `.zoom_fit`); false for any other name.
    pub fn handle_action(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        match name {
            SAVE_ACTION => self.save(cx),
            CANCEL_ACTION => self.cancel(cx),
            ZOOM_IN_ACTION => self.step(1, cx),
            ZOOM_OUT_ACTION => self.step(-1, cx),
            ZOOM_FIT_ACTION => self.fit(cx),
            _ => return false,
        }
        true
    }

    /// Handles the editor's keys; true when it did.
    pub fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let stroke = &event.keystroke;
        let key = stroke.key.as_str();
        let modified = stroke.modifiers.secondary();
        if key == "escape" {
            self.cancel(cx);
        } else if modified && (key == "enter" || key == "s") {
            self.save(cx);
        } else if modified && matches!(key, "=" | "+" | "-" | "0") {
            match key {
                "0" => self.fit(cx),
                "-" => self.step(-1, cx),
                _ => self.step(1, cx),
            }
        } else if key == "tab" && !stroke.modifiers.shift && !modified && !stroke.modifiers.alt {
            // Two spaces at the caret, as the mock does. A selection is left
            // to the editor's own Tab, which indents its lines.
            let caret_only = self.code.read(cx).selected_range().is_empty();
            if !caret_only || !self.code.read(cx).focus_handle(cx).is_focused(window) {
                return false;
            }
            self.code
                .update(cx, |state, cx| state.insert("  ", window, cx));
        } else {
            return false;
        }
        true
    }

    fn head(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, popover_fg, border, bar_bg) = (
            theme.muted_foreground,
            theme.popover_foreground,
            theme.border,
            theme.muted,
        );
        let (fg, hover_border) = (theme.foreground, theme.accent);
        let is_new = matches!(self.spec, DiagramSpec::New { .. });
        let mut templates = h_flex().gap_1().items_center().min_w_0().overflow_hidden();
        for &ix in drawable_templates() {
            let (name, source) = &domain::doc::DIAGRAM_TEMPLATES[ix];
            let source = (*source).to_owned();
            let id = TEMPLATE_IDS[ix % TEMPLATE_IDS.len()];
            templates = templates.child(
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px_2()
                    .rounded(px(11.))
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(move |style| style.text_color(fg).border_color(hover_border))
                    .child(*name)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_code(&source, window, cx)),
                    ),
            );
        }
        h_flex()
            .id("diagram-head")
            .debug_selector(|| "diagram-head".to_owned())
            .flex_none()
            .gap_2()
            .items_center()
            .px_2p5()
            .py_1p5()
            .overflow_hidden()
            .bg(bar_bg)
            .border_b_1()
            .border_color(border)
            .text_sm()
            .text_color(popover_fg)
            .child(
                Icon::new(gpui_kit_assets::IconName::Workflow)
                    .xsmall()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if is_new {
                        "New diagram"
                    } else {
                        "Edit diagram"
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .child("start from"),
            )
            .child(templates)
            .child(div().flex_1())
            .child(
                div()
                    .id("diagram-cancel")
                    .debug_selector(|| "diagram-cancel".to_owned())
                    .flex_none()
                    .px_1p5()
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(move |style| style.text_color(fg))
                    .child("cancel")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DiagramEvent::Cancel))),
            )
            .child(
                tool_button(
                    "diagram-save",
                    if is_new { "Add to doc" } else { "Save" },
                    cx,
                )
                .flex_none()
                .py_0p5()
                .text_xs()
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .into_any_element()
    }

    fn zoom_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted, fg, hover) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.list_active,
        );
        let percent = self
            .effective_scale()
            .map_or_else(|| "–".to_owned(), |s| format!("{:.0}%", s * 100.0));
        let button = |id: &'static str, label: SharedString, min: f32| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .flex()
                .items_center()
                .justify_center()
                .h(px(20.))
                .min_w(px(min))
                .px_1p5()
                .rounded_sm()
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(move |style| style.text_color(fg).bg(hover))
                .child(label)
        };
        h_flex()
            .id("diagram-zoom")
            .debug_selector(|| "diagram-zoom".to_owned())
            .flex_none()
            .gap_0p5()
            .p(px(1.))
            .rounded_sm()
            .border_1()
            .border_color(border)
            .child(
                button("diagram-zoom-out", "−".into(), 22.)
                    .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
            )
            .child(
                button("diagram-zoom-level", percent.into(), 44.)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_to(1.0, None, cx))),
            )
            .child(
                button("diagram-zoom-in", "+".into(), 22.)
                    .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
            )
            .child(
                button("diagram-zoom-fit", "fit".into(), 22.)
                    .border_l_1()
                    .border_color(border)
                    .rounded_r_sm()
                    .rounded_l_none()
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
            .into_any_element()
    }

    fn drawing_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (danger, muted_bg, mono) = (theme.danger, theme.muted, theme.mono_font_family.clone());
        // Another theme: lay the drawing out in its colors.
        let colors = crate::theme::crest_theme(cx);
        if !std::sync::Arc::ptr_eq(&colors, &self.colors) {
            self.colors = colors;
            self.start_layout(cx);
        }
        match self.diagram.clone() {
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("Drawing the diagram…")
                .into_any_element(),
            Some(Err(reason)) => {
                let mut shown: String = diagram::printable(&self.drawn)
                    .chars()
                    .take(MAX_SOURCE_CHARS)
                    .collect();
                if shown.trim().is_empty() {
                    shown = "(empty source)".to_owned();
                }
                v_flex()
                    .size_full()
                    .gap_1()
                    .p_2()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_color(danger)
                            .child(SharedString::from(diagram::printable(&reason))),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(muted_bg)
                            .font_family(mono)
                            .text_sm()
                            .child(SharedString::from(shown)),
                    )
                    .into_any_element()
            }
            Some(Ok(painted)) => {
                let (bounds_cell, scale, pan) = (self.pane_bounds.clone(), self.scale, self.pan);
                let editor = cx.entity();
                canvas(
                    move |bounds, window, cx| {
                        if bounds_cell.replace(bounds).size != bounds.size {
                            // The percentage and the fit follow the pane: draw this
                            // view again (a cached parent only redraws when told).
                            editor.update(cx, |_, cx| cx.notify());
                            window.refresh();
                        }
                    },
                    move |bounds, (), window, cx| {
                        let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                        let camera = camera_for(&painted, size, scale, pan);
                        paint::paint(&painted, camera, bounds, &mono, window, cx);
                    },
                )
                .size_full()
                .into_any_element()
            }
        }
    }
}

/// Ids of the template buttons (debug selectors are `&'static`).
const TEMPLATE_IDS: [&str; 6] = [
    "diagram-template-0",
    "diagram-template-1",
    "diagram-template-2",
    "diagram-template-3",
    "diagram-template-4",
    "diagram-template-5",
];

/// Indexes (into `items`) of the templates `draws` accepts, in order.
pub(super) fn filter_drawable(items: &[(&str, &str)], draws: impl Fn(&str) -> bool) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, (_, source))| draws(source))
        .map(|(ix, _)| ix)
        .collect()
}

/// Original indexes of the starting points crest can draw, parsed once:
/// whether a source parses does not depend on the theme's colors.
pub(super) fn drawable_templates() -> &'static [usize] {
    static DRAWABLE: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    DRAWABLE.get_or_init(|| {
        let colors = crate::theme::default_theme();
        filter_drawable(domain::doc::DIAGRAM_TEMPLATES, |source| {
            diagram::parse_with(source, &colors).is_ok()
        })
    })
}

impl Render for DiagramEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.accent);
        let mono = theme.mono_font_family.clone();
        let stacked = self.is_stacked();
        let head = self.head(cx);
        let preview = self.drawing_view(cx);
        let zoom_bar = self.zoom_bar(cx);
        let body_bounds = self.body_bounds.clone();
        let editor = cx.entity();
        let split = self.split / 100.0;
        let cut = self.cut;
        let flow = |el: gpui::Div| {
            if stacked {
                el.flex_col()
            } else {
                el.flex_row()
            }
        };

        let drawing_area = div()
            .id("diagram-preview")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .rounded_md()
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::pane_down))
            .on_mouse_move(cx.listener(Self::pane_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag = None),
            )
            .on_scroll_wheel(cx.listener(Self::pane_scroll))
            .child(preview);

        // The label and the zoom pill make one row; the drawing, the
        // parser's error and the cut notice are under it.
        let pane = v_flex()
            .id("diagram-pane")
            .flex_none()
            .overflow_hidden()
            .gap_1p5()
            .px_3()
            .py_2p5()
            .map_axis(stacked, split)
            .child(
                h_flex()
                    .min_h(px(22.))
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().text_color(muted).child("preview"))
                    .child(zoom_bar),
            )
            .child(drawing_area);

        let bar = div()
            .id("diagram-splitter")
            .debug_selector(|| "diagram-splitter".to_owned())
            .track_focus(&self.split_focus)
            .flex_none()
            .bg(border)
            .hover(move |style| style.bg(accent))
            .map_bar(stacked)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.split_focus, cx);
                    if event.click_count == 2 {
                        this.set_split(50.0, cx);
                    } else {
                        this.split_drag = true;
                    }
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let delta = match event.keystroke.key.as_str() {
                    "left" | "up" => -SPLIT_STEP,
                    "right" | "down" => SPLIT_STEP,
                    _ => return,
                };
                this.set_split(this.split + delta, cx);
                cx.stop_propagation();
            }));

        let hint = self.code.read(cx).value().trim().is_empty().then_some(
            if matches!(self.spec, DiagramSpec::New { .. }) {
                "empty: nothing will be added"
            } else {
                "empty: saving removes this diagram"
            },
        );
        let (inset, warning) = (cx.theme().secondary, cx.theme().warning);
        let code = v_flex()
            .id("diagram-code-pane")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1p5()
            .px_3()
            .py_2p5()
            .child(
                h_flex()
                    .min_h(px(22.))
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .gap_2()
                    .child("mermaid code")
                    .children(cut.then(|| {
                        div()
                            .text_color(warning)
                            .child("drawing cut at 8,000 characters")
                    })),
            )
            .child(
                div()
                    .id("diagram-code")
                    .debug_selector(|| "diagram-code".to_owned())
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(inset)
                    .child(
                        Textarea::new(&self.code)
                            .h_full()
                            .appearance(false)
                            .bordered(false)
                            .font_family(mono),
                    ),
            )
            .children(hint.map(|text| {
                div()
                    .id("diagram-hint")
                    .debug_selector(|| "diagram-hint".to_owned())
                    .text_xs()
                    .text_color(muted)
                    .child(text)
            }));

        let body = flow(div())
            .id("diagram-body")
            .debug_selector(|| "diagram-body".to_owned())
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(border)
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if !this.split_drag {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    this.split_drag = false;
                    return;
                }
                let stacked = this.is_stacked();
                let percent = this.splitter_percent(event.position, stacked);
                this.set_split(percent, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.split_drag = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.split_drag = false),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let old = body_bounds.replace(bounds);
                        let (a, b) = (f32::from(old.size.width), f32::from(bounds.size.width));
                        if (a < STACK_BELOW) != (b < STACK_BELOW) || old.size != bounds.size {
                            editor.update(cx, |_, cx| cx.notify());
                            window.refresh();
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(pane)
            .child(bar)
            .child(code);

        v_flex()
            .id("diagram-editor")
            .debug_selector(|| "diagram-editor".to_owned())
            .track_focus(&self.focus)
            .size_full()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(head)
            .child(body)
    }
}

/// Sizing of the two panes and the bar for a row or a column.
trait AxisExt: Sized {
    fn map_axis(self, stacked: bool, share: f32) -> Self;
    fn map_bar(self, stacked: bool) -> Self;
}

impl AxisExt for gpui::Stateful<gpui::Div> {
    fn map_axis(self, stacked: bool, share: f32) -> Self {
        if stacked {
            self.w_full().h(relative(share))
        } else {
            self.h_full().w(relative(share))
        }
    }

    fn map_bar(self, stacked: bool) -> Self {
        if stacked {
            self.w_full().h(px(SPLITTER_PX)).cursor_row_resize()
        } else {
            self.h_full().w(px(SPLITTER_PX)).cursor_col_resize()
        }
    }
}
