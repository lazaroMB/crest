//! The image node of `Custom` blocks and modals: a picture with fit, 1:1
//! and zoom levels, and drag to pan when zoomed.
//!
//! *Fit* shows the image at its natural size, scaled down to the width
//! available. Any zoom level (the `+`/`−` buttons, ctrl/cmd + wheel, or
//! with the viewport focused `+`, `-`, `1` for 1:1 and `0` for fit) draws
//! it scaled from its natural size inside a viewport that clips it; drag
//! pans. The natural size comes from GPUI's image cache once decoded, or
//! for SVG from the document itself ([`super::svg`]).

use std::cell::Cell;
use std::rc::Rc;

use domain::block::{BlockId, ImageData, ImageFit};
use gpui::{
    AnyElement, App, Context, FocusHandle, Hsla, ImageSource, ImgResourceLoader,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Pixels, Point, Render, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::image::{ImageError, image_source};
use super::svg::SvgState;

/// Zoom step of the keys, the buttons and one wheel notch.
const ZOOM_STEP: f32 = 1.25;
/// Zoom limits, relative to the natural size.
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 32.0;
/// Tallest the zoomed viewport gets; the image is clipped and panned.
const MAX_VIEWPORT: f32 = 560.0;
/// Shortest zoomed viewport, so a thin image can still be dragged.
const MIN_VIEWPORT: f32 = 120.0;

/// Clamps a zoom level to the supported range.
pub fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// Per-node state: the decoded source, the zoom level and the pan.
pub struct ImageView {
    data: ImageData,
    media_type: Option<String>,
    fit: ImageFit,
    decoded: Result<ImageSource, ImageError>,
    alt: Option<String>,
    /// `None` is fit; otherwise the scale of the natural size.
    zoom: Option<f32>,
    offset: Point<Pixels>,
    drag: Option<Point<Pixels>>,
    /// Scale of the natural size the fit view drew last, to zoom from it.
    fit_scale: Rc<Cell<f32>>,
    focus: FocusHandle,
}

fn initial_zoom(fit: ImageFit) -> Option<f32> {
    match fit {
        ImageFit::Contain => None,
        ImageFit::Actual => Some(1.0),
    }
}

impl ImageView {
    fn new(
        data: &ImageData,
        media_type: Option<&str>,
        alt: Option<&str>,
        fit: ImageFit,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            data: data.clone(),
            media_type: media_type.map(str::to_owned),
            fit,
            decoded: image_source(media_type.unwrap_or_default(), data),
            alt: alt.map(str::to_owned),
            zoom: initial_zoom(fit),
            offset: Point::default(),
            drag: None,
            fit_scale: Rc::new(Cell::new(1.0)),
            focus: cx.focus_handle(),
        }
    }

    /// Decodes again and resets the view when the node changed.
    fn sync(
        &mut self,
        data: &ImageData,
        media_type: Option<&str>,
        alt: Option<&str>,
        fit: ImageFit,
        cx: &mut Context<Self>,
    ) {
        if &self.data != data || self.media_type.as_deref() != media_type || self.fit != fit {
            *self = Self {
                focus: self.focus.clone(),
                ..Self::new(data, media_type, alt, fit, cx)
            };
            cx.notify();
        } else if self.alt.as_deref() != alt {
            self.alt = alt.map(str::to_owned);
            cx.notify();
        }
    }

    /// The zoom level, `None` while fitted.
    pub fn zoom(&self) -> Option<f32> {
        self.zoom
    }

    /// The pan offset of the zoomed view.
    pub fn offset(&self) -> Point<Pixels> {
        self.offset
    }

    /// Multiplies the zoom by `factor`, starting from the fitted scale.
    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let current = self.zoom.unwrap_or_else(|| self.fit_scale.get());
        self.zoom = Some(clamp_zoom(current * factor));
        cx.notify();
    }

    /// Natural size (1:1), centered.
    pub fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.zoom = Some(1.0);
        self.offset = Point::default();
        cx.notify();
    }

    /// Back to fit.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = None;
        self.offset = Point::default();
        cx.notify();
    }

    /// Moves the zoomed image by `delta`.
    pub fn pan_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.offset += delta;
        cx.notify();
    }

    /// Natural size of the decoded image, once GPUI loaded it.
    fn natural_size(&self, window: &mut Window, cx: &mut App) -> Option<(f32, f32)> {
        let image = match self.decoded.as_ref().ok()? {
            ImageSource::Image(image) => image.clone().use_render_image(window, cx)?,
            ImageSource::Resource(resource) => {
                window.use_asset::<ImgResourceLoader>(resource, cx)?.ok()?
            }
            ImageSource::Render(image) => image.clone(),
            ImageSource::Custom(_) => return None,
        };
        let size = image.size(0);
        let (w, h) = (size.width.0 as f32, size.height.0 as f32);
        (w > 0.0 && h > 0.0).then_some((w, h))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(event.position);
        window.focus(&self.focus, cx);
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        self.drag = Some(event.position);
        self.pan_by(event.position - last, cx);
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let dy = f32::from(event.delta.pixel_delta(px(20.0)).y);
        self.zoom_by(ZOOM_STEP.powf(dy / 20.0), cx);
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
            "1" => self.actual_size(cx),
            "0" => self.fit(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .bg(theme.secondary)
                .hover(|style| style.bg(theme.secondary_hover))
                .cursor_pointer()
                .child(label)
        };
        let level = match self.zoom {
            None => "fit".to_owned(),
            Some(zoom) => format!("{:.0}%", zoom * 100.0),
        };
        let label = self.alt.clone().unwrap_or_else(|| match &self.data {
            ImageData::Path { path } => path
                .rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
                .next()
                .unwrap_or(path)
                .to_owned(),
            ImageData::Base64 { .. } => "image".to_owned(),
        });
        h_flex()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(SharedString::from(label)),
            )
            .child(SharedString::from(level))
            .child(button("image-fit", "fit").on_click(cx.listener(|this, _, _, cx| this.fit(cx))))
            .child(
                button("image-actual", "1:1")
                    .on_click(cx.listener(|this, _, _, cx| this.actual_size(cx))),
            )
            .child(
                button("image-zoom-out", "−")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.0 / ZOOM_STEP, cx))),
            )
            .child(
                button("image-zoom-in", "+")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(ZOOM_STEP, cx))),
            )
            .into_any_element()
    }

    fn fallback(&self, danger: Hsla, border: Hsla) -> impl Fn() -> AnyElement + 'static {
        let message = match (&self.alt, &self.data) {
            (Some(alt), _) => format!("cannot load image: {alt}"),
            (None, ImageData::Path { path }) => format!("cannot load {path}"),
            (None, ImageData::Base64 { .. }) => "cannot load inline image".to_owned(),
        };
        move || error_view(message.clone(), danger, border)
    }
}

fn error_view(message: String, danger: Hsla, border: Hsla) -> AnyElement {
    div()
        .p_2()
        .border_1()
        .border_color(border)
        .rounded_md()
        .text_color(danger)
        .child(SharedString::from(message))
        .into_any_element()
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (danger, border, radius) = (theme.danger, theme.border, theme.radius);
        let media_type = self.media_type.as_deref().unwrap_or_default();
        let tint = super::svg::wants_tint(media_type).then(|| super::svg::Tint::from_theme(cx));
        let svg = super::svg::is_svg(media_type, &self.data).then(|| {
            super::svg::use_svg_tinted(&self.data, self.zoom.unwrap_or(1.0), tint, window, cx)
        });
        let is_svg = svg.is_some();
        let (source, natural) = match (svg, &self.decoded) {
            // An SVG rasterized for the zoom level, drawn at its natural
            // (logical) size times the zoom.
            (Some(SvgState::Ready(raster)), _) => {
                (ImageSource::Render(raster.image.clone()), Some(raster.size))
            }
            (Some(SvgState::Loading), _) => {
                return v_flex()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("rendering SVG…")
                    .into_any_element();
            }
            (Some(SvgState::Failed(err)), _) => {
                return v_flex()
                    .child(error_view(err, danger, border))
                    .into_any_element();
            }
            (None, Ok(source)) => (source.clone(), self.natural_size(window, cx)),
            (None, Err(err)) => {
                return v_flex()
                    .child(error_view(err.to_string(), danger, border))
                    .into_any_element();
            }
        };
        let fallback = self.fallback(danger, border);
        let body = match (self.zoom, natural) {
            (Some(zoom), Some((w, h))) => {
                let (w, h) = (w * zoom, h * zoom);
                let height = h.clamp(MIN_VIEWPORT, MAX_VIEWPORT);
                div()
                    .id("image-viewport")
                    .track_focus(&self.focus)
                    .relative()
                    .w_full()
                    .h(px(height))
                    .overflow_hidden()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
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
                    .child(
                        img(source)
                            .absolute()
                            .left(self.offset.x)
                            .top(self.offset.y)
                            .w(px(w))
                            .h(px(h))
                            .with_fallback(fallback),
                    )
                    .into_any_element()
            }
            _ => {
                let fit_scale = Rc::clone(&self.fit_scale);
                let natural_width = natural.map(|(w, _)| w);
                div()
                    .id("image-fit")
                    .track_focus(&self.focus)
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
                    )
                    .on_scroll_wheel(cx.listener(Self::scroll))
                    .on_key_down(cx.listener(Self::key_down))
                    .child(
                        div()
                            .w_full()
                            .on_children_prepainted(move |bounds, _, _| {
                                if let (Some(shown), Some(natural)) =
                                    (bounds.first(), natural_width)
                                {
                                    fit_scale.set(f32::from(shown.size.width) / natural);
                                }
                            })
                            .child(match natural.filter(|_| is_svg) {
                                // An SVG bitmap has more pixels than its
                                // size; draw it at the natural size.
                                Some((w, h)) => img(source)
                                    .w(px(w))
                                    .max_w_full()
                                    .aspect_ratio(w / h)
                                    .with_fallback(fallback),
                                None => img(source).max_w_full().with_fallback(fallback),
                            }),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .gap_1()
            .w_full()
            .child(self.toolbar(cx))
            .child(body)
            .into_any_element()
    }
}

/// The view of the image node `id`, in sync with its data.
pub fn image_view(
    id: BlockId,
    data: &ImageData,
    media_type: Option<&str>,
    alt: Option<&str>,
    fit: ImageFit,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<ImageView> {
    let view = super::block_state(id, window, cx, |_, cx| {
        ImageView::new(data, media_type, alt, fit, cx)
    });
    let focus = view.read(cx).focus.clone();
    crate::blocks::remember_focus(id, &focus, cx);
    view.update(cx, |view, cx| view.sync(data, media_type, alt, fit, cx));
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, point};

    /// A 1×1 transparent PNG.
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[test]
    fn zoom_is_clamped() {
        assert_eq!(clamp_zoom(100.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(0.0), MIN_ZOOM);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(2.0), 2.0);
    }

    #[gpui::test]
    fn buttons_and_drag_change_zoom_and_pan(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let cx = cx.add_empty_window();
        let data = ImageData::Base64 { data: PIXEL.into() };
        let id = BlockId::new();
        let view = cx.update(|window, cx| {
            image_view(
                id,
                &data,
                Some("image/png"),
                None,
                ImageFit::Contain,
                window,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            assert_eq!(view.zoom(), None);
            view.zoom_by(2.0, cx);
            assert_eq!(view.zoom(), Some(2.0));
            view.pan_by(point(px(5.), px(-3.)), cx);
            assert_eq!(view.offset(), point(px(5.), px(-3.)));
            view.actual_size(cx);
            assert_eq!((view.zoom(), view.offset()), (Some(1.0), Point::default()));
            view.fit(cx);
            assert_eq!(view.zoom(), None);
        });
        // The same node keeps its state; a changed fit resets it.
        view.update(cx, |view, cx| view.zoom_by(3.0, cx));
        let again = cx.update(|window, cx| {
            image_view(
                id,
                &data,
                Some("image/png"),
                None,
                ImageFit::Contain,
                window,
                cx,
            )
        });
        assert_eq!(again, view);
        view.read_with(cx, |view, _| assert_eq!(view.zoom(), Some(3.0)));
        cx.update(|window, cx| {
            image_view(
                id,
                &data,
                Some("image/png"),
                None,
                ImageFit::Actual,
                window,
                cx,
            )
        });
        view.read_with(cx, |view, _| assert_eq!(view.zoom(), Some(1.0)));
    }

    #[gpui::test]
    fn svg_nodes_are_rasterized(cx: &mut TestAppContext) {
        use base64::Engine as _;

        use crate::blocks::svg::{SQUARE, cached};

        cx.update(gpui_component::init);
        let data = ImageData::Base64 {
            data: base64::engine::general_purpose::STANDARD.encode(SQUARE),
        };
        let id = BlockId::new();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = image_view(
                id,
                &data,
                Some("image/svg+xml"),
                None,
                ImageFit::Contain,
                window,
                cx,
            );
            Wrapper(view)
        });
        cx.run_until_parked();
        view.update_in(cx, |_, window, cx| {
            window.refresh();
            let state = cached(&data, 1.0, window, cx);
            assert!(
                matches!(state, Some(SvgState::Ready(ref r)) if r.size == (40.0, 20.0)),
                "{state:?}"
            );
        });
    }

    struct Wrapper(gpui::Entity<ImageView>);

    impl Render for Wrapper {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }
}
