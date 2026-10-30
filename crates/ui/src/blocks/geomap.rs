//! Renderer of the `geo_map` block kind (`show_map`): GeoJSON features
//! on a map.
//!
//! A summary line comes first (feature counts by type, the CRS and where
//! it came from, the engine's totals). The canvas paints the features in
//! Web Mercator as the adapter projected them: polygons filled (holes left
//! open) and outlined, lines stroked, points as dots, in the colour of the
//! property chosen below (categories from the palette, numbers on a one-hue
//! ramp) or the first palette colour. Drag pans, `Ctrl`/`Cmd` + wheel zooms
//! about the cursor (a plain wheel scrolls the chat), `+`/`-`/`0` and the
//! toolbar zoom and fit; a click picks the feature under the cursor and the
//! inspector shows its properties and the engine's measures (geodesic area,
//! length, perimeter, box). The cursor's longitude and latitude, the CRS
//! and a scale bar (ground distance at the centre's latitude) are read out
//! on the canvas. There are no background tiles: the map is the data alone.

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::drawing::Extents;
use domain::science::geomap::{
    Colouring, MapSpec, Xy, area_text, ground_factor, length_text, scale_bar, to_lon_lat,
    value_text,
};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Context, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, point, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::drawing::Viewport;
use crate::theme::token;
use crate::timeline::TimelineEntry;
use crate::widgets::{palette, readout, tool_button};

/// Height of the map canvas, in pixels.
pub const CANVAS_HEIGHT: f32 = 440.0;
/// Zoom step of the keys and buttons.
const ZOOM_STEP: f64 = 1.25;
/// Zoom step of one wheel notch.
const WHEEL_STEP: f64 = 1.15;
/// Pixels around a point or line that still pick it.
const PICK_SLOP: f64 = 5.0;
/// Pixels a press may move and still be a click.
const CLICK_SLOP: f32 = 3.0;
/// Radius of a point, in pixels.
const DOT: f32 = 3.5;
/// Longest the scale bar gets, in pixels.
const SCALE_BAR_PX: f64 = 120.0;
/// Steps of the sequential ramp (features are batched per step).
const RAMP_STEPS: usize = 12;
/// Vertices per painted path (GPUI's tessellator indexes with u16).
const PATH_CHUNK: usize = 4_000;
/// Property keys offered for colouring.
const MAX_KEYS: usize = 16;
/// Properties listed in the inspector.
const MAX_PROPERTIES: usize = 40;
/// Notes listed.
const LISTED: usize = 5;

/// A number for readouts: up to `decimals` decimals, no trailing zeros.
fn number(v: f64, decimals: usize) -> String {
    let text = format!("{v:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    if text == "-0" { "0".to_owned() } else { text }
}

/// The summary text: counts by type, the CRS and the engine's totals.
#[must_use]
pub fn summary_text(spec: &MapSpec) -> String {
    let mut parts: Vec<String> = spec
        .kind_counts()
        .iter()
        .map(|(k, n)| format!("{n} {}", k.name()))
        .collect();
    if spec.without_geometry > 0 {
        parts.push(format!("{} without geometry", spec.without_geometry));
    }
    parts.push(format!("{} ({})", spec.crs.label(), spec.crs.source_text()));
    let values = &spec.measures.values;
    if let Some(v) = values.get("area_m2") {
        parts.push(format!("area {}", area_text(v.v)));
    }
    if let Some(v) = values.get("length_m") {
        parts.push(format!("lines {}", length_text(v.v)));
    }
    parts.join(" · ")
}

/// Colour slot of feature `ix` under `colouring`: a category index, a
/// ramp step offset by the categories' room, or `None` (the base colour).
fn class_of(colouring: Option<&Colouring>, ix: usize) -> Option<usize> {
    match colouring? {
        Colouring::Categorical { of, .. } => of.get(ix).copied().flatten(),
        Colouring::Sequential { of, .. } => of.get(ix).copied().flatten().map(|t| {
            // Ramp positions to steps 0..RAMP_STEPS.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let step = (t.clamp(0.0, 1.0) * (RAMP_STEPS - 1) as f64).round() as usize;
            step
        }),
    }
}

/// Colours the canvas paints with.
#[derive(Clone)]
struct Colors {
    palette: [Hsla; 5],
    base: Hsla,
    missing: Hsla,
    foreground: Hsla,
    background: Hsla,
    dark: bool,
}

impl Colors {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            palette: [0, 1, 2, 3, 4].map(|ix| palette(ix, cx)),
            base: palette(0, cx),
            missing: theme.muted_foreground,
            foreground: theme.foreground,
            background: token(cx, "canvas.bg"),
            dark: theme.background.l < 0.5,
        }
    }

    /// The colour of class `class` under `colouring`.
    fn of(&self, colouring: Option<&Colouring>, class: Option<usize>) -> Hsla {
        match (colouring, class) {
            (None, _) => self.base,
            (Some(_), None) => self.missing,
            (Some(Colouring::Categorical { .. }), Some(c)) => category_color(&self.palette, c),
            (Some(Colouring::Sequential { .. }), Some(step)) =>
            {
                #[allow(clippy::cast_precision_loss)]
                ramp_color(self.base, step as f32 / (RAMP_STEPS - 1) as f32, self.dark)
            }
        }
    }
}

/// Category `c`'s colour: the five palette colours, then the same a step
/// lighter and a step darker.
fn category_color(palette: &[Hsla; 5], c: usize) -> Hsla {
    let mut color = palette[c % 5];
    match c / 5 {
        0 => {}
        1 => color.l = (color.l + 0.18).min(0.85),
        _ => color.l = (color.l - 0.18).max(0.2),
    }
    color
}

/// A one-hue ramp: light (low) to dark (high) on a light theme, the other
/// way round on a dark one, so high values always stand out.
fn ramp_color(base: Hsla, t: f32, dark: bool) -> Hsla {
    let (lo, hi) = if dark { (0.25, 0.8) } else { (0.88, 0.3) };
    Hsla {
        l: lo + (hi - lo) * t,
        s: base.s.max(0.45),
        ..base
    }
}

/// Per-block state: the spec, the colouring, the viewport, the cursor and
/// the picked feature.
pub struct MapView {
    spec: Rc<MapSpec>,
    /// Display box of each feature, to skip what is off screen.
    boxes: Rc<Vec<(Xy, Xy)>>,
    color_by: Option<String>,
    colouring: Rc<Option<Colouring>>,
    view: Rc<Cell<Option<Viewport>>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<(Point<Pixels>, f32)>,
    cursor: Option<Xy>,
    selected: Option<usize>,
    focus: FocusHandle,
}

fn feature_boxes(spec: &MapSpec) -> Vec<(Xy, Xy)> {
    spec.features
        .iter()
        .map(|f| {
            f.vertices().fold(
                ([f64::MAX, f64::MAX], [f64::MIN, f64::MIN]),
                |(lo, hi), p| {
                    (
                        [lo[0].min(p[0]), lo[1].min(p[1])],
                        [hi[0].max(p[0]), hi[1].max(p[1])],
                    )
                },
            )
        })
        .collect()
}

impl MapView {
    fn new(spec: &MapSpec, cx: &mut Context<Self>) -> Self {
        let color_by = spec.color_by.clone();
        Self {
            colouring: Rc::new(color_by.as_deref().and_then(|k| spec.colouring(k))),
            color_by,
            boxes: Rc::new(feature_boxes(spec)),
            spec: Rc::new(spec.clone()),
            view: Rc::default(),
            bounds: Rc::default(),
            drag: None,
            cursor: None,
            selected: None,
            focus: cx.focus_handle(),
        }
    }

    fn sync(&mut self, spec: &MapSpec, cx: &mut Context<Self>) {
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

    fn extents(&self) -> Option<Extents> {
        self.spec.extent().map(|(min, max)| Extents { min, max })
    }

    /// The viewport (fitted to the features until zoomed or panned).
    pub fn viewport(&self) -> Viewport {
        self.view
            .get()
            .unwrap_or_else(|| Viewport::fit(self.extents(), self.size()))
    }

    /// Zooms by `factor` about the canvas centre.
    pub fn zoom_by(&mut self, factor: f64, cx: &mut Context<Self>) {
        let size = self.size();
        let mut view = self.viewport();
        view.zoom_about(factor, (size.0 / 2.0, size.1 / 2.0), size);
        self.view.set(Some(view));
        cx.notify();
    }

    /// Fits the features again.
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

    /// The property the features are coloured by.
    pub fn color_by(&self) -> Option<&str> {
        self.color_by.as_deref()
    }

    /// Colours by property `key`, or by nothing.
    pub fn set_color_by(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.colouring = Rc::new(key.as_deref().and_then(|k| self.spec.colouring(k)));
        self.color_by = key;
        cx.notify();
    }

    /// The picked feature.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Picks feature `ix` (or none).
    pub fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix.filter(|ix| *ix < self.spec.features.len());
        cx.notify();
    }

    /// The map point (Web Mercator) under the mouse, if it is over the
    /// canvas.
    pub fn cursor(&self) -> Option<Xy> {
        self.cursor
    }

    /// Picks the feature at canvas pixel `at`.
    pub fn pick_at(&mut self, at: (f64, f64), cx: &mut Context<Self>) {
        let view = self.viewport();
        let p = view.to_world(at, self.size());
        let hit = self.spec.pick(p, PICK_SLOP / view.scale);
        self.select(hit, cx);
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
        self.drag = Some((event.position, 0.0));
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let size = self.size();
        if self.bounds.get().contains(&event.position) {
            self.cursor = Some(self.viewport().to_world(self.local(event.position), size));
        }
        if let Some((last, moved)) = self.drag {
            if event.pressed_button.is_none() {
                self.drag = None;
            } else {
                let d = event.position - last;
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                self.pan_by(f64::from(dx), f64::from(dy), cx);
                self.drag = Some((event.position, moved + dx.abs() + dy.abs()));
            }
        }
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, moved)) = self.drag.take()
            && moved <= CLICK_SLOP
        {
            self.pick_at(self.local(event.position), cx);
        }
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
            "escape" => self.select(None, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn canvas(&self, cx: &App) -> impl IntoElement + use<> {
        let colors = Colors::new(cx);
        let spec = Rc::clone(&self.spec);
        let boxes = Rc::clone(&self.boxes);
        let colouring = Rc::clone(&self.colouring);
        let selected = self.selected;
        let (view_cell, bounds_cell) = (Rc::clone(&self.view), Rc::clone(&self.bounds));
        let extents = self.extents();
        canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |bounds, (), window, _| {
                let size = (
                    f64::from(f32::from(bounds.size.width)).max(1.0),
                    f64::from(f32::from(bounds.size.height)).max(1.0),
                );
                let view = view_cell
                    .get()
                    .unwrap_or_else(|| Viewport::fit(extents, size));
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(gpui::fill(bounds, colors.background));
                    let painter = Painter {
                        spec: &spec,
                        boxes: &boxes,
                        colouring: colouring.as_ref().as_ref(),
                        view,
                        size,
                        bounds,
                        colors: &colors,
                    };
                    painter.paint(selected, window);
                });
            },
        )
        .size_full()
    }

    fn legend(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let colors = Colors::new(cx);
        let keys: Vec<String> = self
            .spec
            .property_keys()
            .into_iter()
            .take(MAX_KEYS)
            .collect();
        let current = self.color_by.clone();
        let chips = std::iter::once(None)
            .chain(keys.into_iter().map(Some))
            .enumerate()
            .map(|(i, key)| {
                let on = key == current;
                let label = key.clone().unwrap_or_else(|| "none".to_owned());
                div()
                    .id(("map-color", i))
                    .debug_selector(move || format!("map-color-{i}"))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(token(cx, "accent.bg"))
                            .text_color(token(cx, "accent.fg"))
                    })
                    .when(!on, |d| d.text_color(muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_color_by(key.clone(), cx);
                    }))
                    .child(SharedString::from(label))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let swatch = |color: Hsla| div().size_3().rounded_sm().bg(color);
        let scale: Vec<AnyElement> = match self.colouring.as_ref() {
            None => Vec::new(),
            Some(
                c @ Colouring::Categorical {
                    categories, other, ..
                },
            ) => {
                let mut items: Vec<AnyElement> = categories
                    .iter()
                    .enumerate()
                    .map(|(ix, (name, n))| {
                        h_flex()
                            .gap_1()
                            .child(swatch(colors.of(Some(c), Some(ix))))
                            .child(SharedString::from(format!("{name} ({n})")))
                            .into_any_element()
                    })
                    .collect();
                if *other > 0 {
                    items.push(
                        h_flex()
                            .gap_1()
                            .child(swatch(colors.missing))
                            .child(format!("other or none ({other}+)"))
                            .into_any_element(),
                    );
                }
                items
            }
            Some(c @ Colouring::Sequential { min, max, .. }) => {
                let steps = (0..RAMP_STEPS)
                    .step_by(2)
                    .map(|s| swatch(colors.of(Some(c), Some(s))));
                vec![
                    h_flex()
                        .gap_1()
                        .child(SharedString::from(number(*min, 4)))
                        .children(steps)
                        .child(SharedString::from(number(*max, 4)))
                        .into_any_element(),
                ]
            }
        };
        v_flex()
            .gap_1()
            .text_xs()
            .debug_selector(|| "map-legend".to_owned())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(div().text_color(muted).child("Colour by"))
                    .children(chips),
            )
            .when(!scale.is_empty(), |d| {
                d.child(h_flex().flex_wrap().gap_x_3().gap_y_1().children(scale))
            })
    }

    fn inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ix = self.selected?;
        let feature = self.spec.features.get(ix)?;
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let mut measures = Vec::new();
        if let Some(a) = feature.area_m2 {
            measures.push(format!("area {} ({} m²)", area_text(a), number(a, 1)));
        }
        if let Some(l) = feature.length_m {
            measures.push(format!("length {} ({} m)", length_text(l), number(l, 1)));
        }
        if let Some(p) = feature.perimeter_m {
            measures.push(format!("perimeter {}", length_text(p)));
        }
        if let Some([w, s, e, n]) = feature.bbox {
            measures.push(format!(
                "box {}, {} – {}, {}",
                number(w, 6),
                number(s, 6),
                number(e, 6),
                number(n, 6)
            ));
        }
        let rows = feature
            .properties
            .iter()
            .take(MAX_PROPERTIES)
            .map(|(k, v)| {
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .min_w(px(120.0))
                            .text_color(muted)
                            .child(SharedString::from(k.clone())),
                    )
                    .child(SharedString::from(value_text(v)))
            });
        let more = feature.properties.len().saturating_sub(MAX_PROPERTIES);
        Some(
            v_flex()
                .gap_0p5()
                .text_xs()
                .border_t_1()
                .border_color(border)
                .pt_1()
                .debug_selector(|| "map-inspector".to_owned())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(SharedString::from(feature.label(ix))),
                        )
                        .child(div().text_color(muted).child(format!(
                            "{} · feature {}",
                            feature.kind.name(),
                            ix + 1
                        )))
                        .child(
                            tool_button("map-inspector-close", "×", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.select(None, cx))),
                        ),
                )
                .when(!measures.is_empty(), |d| {
                    d.child(div().text_color(muted).child(format!(
                        "Engine (geodesic, WGS 84): {}",
                        measures.join(" · ")
                    )))
                })
                .children(rows)
                .when(more > 0, |d| {
                    d.child(div().text_color(muted).child(format!("… {more} more")))
                })
                .into_any_element(),
        )
    }

    /// The scale bar at the centre's latitude: its pixel length and label.
    fn scale_bar(&self) -> Option<(f32, String)> {
        let view = self.viewport();
        let (_, lat) = to_lon_lat(view.center);
        let metres_per_px = ground_factor(lat) / view.scale;
        let (metres, label) = scale_bar(SCALE_BAR_PX * metres_per_px)?;
        #[allow(clippy::cast_possible_truncation)]
        Some(((metres / metres_per_px) as f32, label))
    }
}

/// Everything one paint reads.
struct Painter<'a> {
    spec: &'a MapSpec,
    boxes: &'a [(Xy, Xy)],
    colouring: Option<&'a Colouring>,
    view: Viewport,
    size: (f64, f64),
    bounds: Bounds<Pixels>,
    colors: &'a Colors,
}

impl Painter<'_> {
    fn at(&self, p: Xy) -> Point<Pixels> {
        let (x, y) = self.view.to_screen(p, self.size);
        // Screen coordinates fit an f32.
        #[allow(clippy::cast_possible_truncation)]
        point(
            self.bounds.origin.x + px(x as f32),
            self.bounds.origin.y + px(y as f32),
        )
    }

    fn visible(&self, ix: usize) -> bool {
        let a = self.view.to_world((0.0, self.size.1), self.size);
        let b = self.view.to_world((self.size.0, 0.0), self.size);
        self.boxes.get(ix).is_some_and(|(lo, hi)| {
            hi[0] >= a[0] && lo[0] <= b[0] && hi[1] >= a[1] && lo[1] <= b[1]
        })
    }

    /// Paints `rings` (closed) into chunked paths: filled, or stroked.
    fn rings<'r>(
        &self,
        rings: impl Iterator<Item = &'r [Xy]>,
        fill: bool,
        width: f32,
        color: Hsla,
        window: &mut Window,
    ) {
        let new = || {
            if fill {
                PathBuilder::fill()
            } else {
                PathBuilder::stroke(px(width))
            }
        };
        let mut path = new();
        let mut count = 0;
        for ring in rings {
            let mut points = ring.iter();
            let Some(first) = points.next() else {
                continue;
            };
            if count > 0 && count + ring.len() > PATH_CHUNK {
                if let Ok(built) = std::mem::replace(&mut path, new()).build() {
                    window.paint_path(built, color);
                }
                count = 0;
            }
            path.move_to(self.at(*first));
            for p in points {
                path.line_to(self.at(*p));
            }
            if fill {
                path.close();
            }
            count += ring.len();
        }
        if count > 0
            && let Ok(built) = path.build()
        {
            window.paint_path(built, color);
        }
    }

    fn paint(&self, selected: Option<usize>, window: &mut Window) {
        let features = &self.spec.features;
        let mut classes: Vec<Option<usize>> = (0..features.len())
            .map(|ix| class_of(self.colouring, ix))
            .collect::<Vec<_>>();
        classes.sort_unstable();
        classes.dedup();
        let in_class = |class: Option<usize>| {
            (0..features.len())
                .filter(move |ix| class_of(self.colouring, *ix) == class)
                .filter(|ix| self.visible(*ix))
        };
        // Polygons first (fill, then outline), lines above, points on top.
        for &class in &classes {
            let color = self.colors.of(self.colouring, class);
            let fill = color.opacity(0.35);
            let rings = || {
                in_class(class)
                    .flat_map(|ix| features[ix].polygons.iter().flatten().map(Vec::as_slice))
            };
            self.rings(rings(), true, 0.0, fill, window);
            self.rings(rings(), false, 1.0, color, window);
        }
        for &class in &classes {
            let color = self.colors.of(self.colouring, class);
            let lines = in_class(class).flat_map(|ix| features[ix].lines.iter().map(Vec::as_slice));
            self.rings(lines, false, 2.0, color, window);
        }
        for &class in &classes {
            let color = self.colors.of(self.colouring, class);
            for ix in in_class(class) {
                for p in &features[ix].points {
                    self.dot(*p, DOT, color, window);
                }
            }
        }
        if let Some(ix) = selected.filter(|ix| *ix < features.len()) {
            let f = &features[ix];
            let fg = self.colors.foreground;
            self.rings(
                f.polygons.iter().flatten().map(Vec::as_slice),
                false,
                2.5,
                fg,
                window,
            );
            self.rings(f.lines.iter().map(Vec::as_slice), false, 3.5, fg, window);
            for p in &f.points {
                self.dot(*p, DOT + 2.0, fg, window);
            }
        }
    }

    fn dot(&self, p: Xy, r: f32, color: Hsla, window: &mut Window) {
        let c = self.at(p);
        let r = px(r);
        window.paint_quad(
            gpui::fill(
                Bounds::new(point(c.x - r, c.y - r), gpui::size(r * 2.0, r * 2.0)),
                color,
            )
            .corner_radii(r),
        );
    }
}

impl Render for MapView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.canvas(cx);
        let legend = self.legend(cx);
        let inspector = self.inspector(cx);
        let theme = cx.theme();
        let (muted, radius, foreground) = (theme.muted_foreground, theme.radius, theme.foreground);
        let cursor = self.cursor.map(|p| {
            let (lon, lat) = to_lon_lat(p);
            format!("lon {} · lat {}", number(lon, 6), number(lat, 6))
        });
        let crs = format!("{} → Web Mercator", self.spec.crs.label());
        let bar = self.scale_bar();
        let warnings = self.spec.warnings.clone();
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "map".to_owned())
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
                    .debug_selector(|| "map-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .child(
                div()
                    .id("map-canvas")
                    .debug_selector(|| "map-canvas".to_owned())
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
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
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
                            .child(tool_button("map-zoom-out", "−", cx).on_click(
                                cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, cx)),
                            ))
                            .child(tool_button("map-zoom-in", "+", cx).on_click(
                                cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, cx)),
                            ))
                            .child(
                                tool_button("map-fit", "fit", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                            ),
                    )
                    .child(
                        v_flex()
                            .absolute()
                            .left_1()
                            .bottom_1()
                            .gap_0p5()
                            .text_xs()
                            .when_some(bar, |d, (width, label)| {
                                d.child(
                                    v_flex()
                                        .debug_selector(|| "map-scale".to_owned())
                                        .child(div().text_color(foreground).child(label))
                                        .child(
                                            div()
                                                .w(px(width))
                                                .h(px(4.0))
                                                .border_1()
                                                .border_t_0()
                                                .border_color(foreground),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .debug_selector(|| "map-crs".to_owned())
                                    .child(readout(crs, cx)),
                            )
                            .when_some(cursor, |d, text| {
                                d.child(
                                    div()
                                        .debug_selector(|| "map-cursor".to_owned())
                                        .child(readout(text, cx)),
                                )
                            }),
                    ),
            )
            .child(legend)
            .children(inspector)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "map-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The map entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &MapSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<MapView> {
    let view = super::block_state(id, window, cx, |_, cx| MapView::new(spec, cx));
    let focus = view.read(cx).focus.clone();
    crate::blocks::remember_focus(id, &focus, cx);
    view.update(cx, |view, cx| view.sync(spec, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::GeoMap(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::computed::{Computed, Engine};
    use domain::science::geomap::{CrsSource, GeometryKind, MapCrs, MapFeature};
    use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, size};
    use serde_json::json;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Xy> {
        vec![
            [x0, y0],
            [x0 + s, y0],
            [x0 + s, y0 + s],
            [x0, y0 + s],
            [x0, y0],
        ]
    }

    fn spec() -> MapSpec {
        let feature = |kind, props: serde_json::Value| MapFeature {
            id: None,
            kind,
            points: Vec::new(),
            lines: Vec::new(),
            polygons: Vec::new(),
            properties: props.as_object().cloned().unwrap_or_default(),
            bbox: Some([0.0, 0.0, 0.01, 0.01]),
            area_m2: None,
            length_m: None,
            perimeter_m: None,
        };
        let mut park = feature(
            GeometryKind::Polygon,
            json!({"name": "Park", "kind": "green"}),
        );
        park.polygons = vec![vec![square(0.0, 0.0, 1000.0), square(400.0, 400.0, 200.0)]];
        park.area_m2 = Some(960_000.0);
        let mut road = feature(
            GeometryKind::LineString,
            json!({"name": "Road", "kind": "grey"}),
        );
        road.lines = vec![vec![[0.0, 1200.0], [1000.0, 1200.0]]];
        road.length_m = Some(1000.0);
        MapSpec {
            title: None,
            path: Some("geo/parks.geojson".into()),
            crs: MapCrs {
                epsg: Some(4326),
                name: "WGS 84".into(),
                source: CrsSource::Default,
                definition: "+proj=longlat +datum=WGS84 +no_defs".into(),
            },
            features: vec![park, road],
            total_features: 2,
            without_geometry: 0,
            color_by: Some("kind".into()),
            measures: Computed::new(
                "ge_1",
                "geo.measures",
                "m",
                Engine {
                    name: "crest-geo".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: vec!["note".into()],
        }
    }

    #[test]
    fn summary_classes_and_ramp() {
        assert_eq!(
            summary_text(&spec()),
            "1 LineString · 1 Polygon · EPSG:4326 · WGS 84 (default (RFC 7946))"
        );
        let s = spec();
        let c = s.colouring("kind");
        assert_eq!(class_of(c.as_ref(), 0), Some(0));
        assert_eq!(class_of(None, 0), None);
        let light = ramp_color(gpui::red(), 0.0, false);
        let dark = ramp_color(gpui::red(), 1.0, false);
        assert!(light.l > dark.l, "high values are darker on a light theme");
        let palette = [gpui::red(); 5];
        assert!(category_color(&palette, 5).l > category_color(&palette, 0).l);
    }

    #[gpui::test]
    fn draws_picks_and_recolours(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (timeline, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::GeoMap(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
            div().size_full().child(timeline.clone())
        });
        cx.run_until_parked();
        for selector in [
            "map",
            "map-summary",
            "map-canvas",
            "map-legend",
            "map-color-0",
            "map-crs",
            "map-scale",
            "map-warnings",
            "map-fit",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let fitted = view.read_with(cx, |v, _| v.viewport());
        assert!((fitted.center[0] - 500.0).abs() < 1e-6, "{fitted:?}");
        // A click inside the park (not the hole) picks it.
        let canvas = cx.debug_bounds("map-canvas").unwrap();
        let at = view.read_with(cx, |v, _| v.viewport().to_screen([100.0, 100.0], v.size()));
        #[allow(clippy::cast_possible_truncation)]
        let click = canvas.origin + point(px(at.0 as f32), px(at.1 as f32));
        cx.simulate_click(click, Modifiers::none());
        assert_eq!(view.read_with(cx, |v, _| v.selected()), Some(0));
        assert!(cx.debug_bounds("map-inspector").is_some());
        let hole = view.read_with(cx, |v, _| v.viewport().to_screen([500.0, 500.0], v.size()));
        view.update(cx, |v, cx| v.pick_at(hole, cx));
        assert_eq!(view.read_with(cx, |v, _| v.selected()), None);
        // Colour by nothing, then zoom with the wheel; the cursor reads out.
        let none = cx.debug_bounds("map-color-0").unwrap().center();
        cx.simulate_click(none, Modifiers::none());
        assert_eq!(
            view.read_with(cx, |v, _| v.color_by().map(str::to_owned)),
            None
        );
        cx.simulate_mouse_move(canvas.center(), None, Modifiers::none());
        cx.simulate_event(ScrollWheelEvent {
            position: canvas.center(),
            delta: ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers: Modifiers::control(),
            touch_phase: TouchPhase::Moved,
        });
        assert!(view.read_with(cx, |v, _| v.viewport()).scale > fitted.scale);
        assert!(view.read_with(cx, |v, _| v.cursor()).is_some());
        assert!(cx.debug_bounds("map-cursor").is_some());
    }

    #[gpui::test]
    fn a_plain_wheel_scrolls_the_chat_and_ctrl_wheel_zooms(cx: &mut TestAppContext) {
        use crate::blocks::wheel_host::{host, wheel};
        cx.update(gpui_component::init);
        let (timeline, wheels, cx) = host(cx, |_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::GeoMap(Box::new(spec())));
        let id = block.id;
        timeline.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let view = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let fitted = view.read_with(cx, |v, _| v.viewport());
        let canvas = cx.debug_bounds("map-canvas").expect("canvas").center();
        wheel(cx, canvas, Modifiers::none());
        assert_eq!(view.read_with(cx, |v, _| v.viewport()), fitted, "no zoom");
        assert_eq!(wheels.get(), 1, "the wheel went on to the chat");
        wheel(cx, canvas, Modifiers::control());
        assert!(view.read_with(cx, |v, _| v.viewport()).scale > fitted.scale);
        assert_eq!(wheels.get(), 1, "Ctrl + wheel stays in the map");
    }
}
