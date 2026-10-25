//! Renderer of the `image` block kind: the picture, from inline base64 or
//! a file, with its source underneath. An image that cannot be decoded or
//! loaded shows a placeholder with the error instead. SVG is rasterized
//! off the UI thread at the window's scale ([`super::svg`]), tinted to
//! the theme when its media type asks for it (`; tint=theme`).

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use domain::block::{BlockKind, ImageData, SourceRef};
use gpui::{
    AnyElement, App, ElementId, Hsla, Image, ImageFormat, ImageSource, InteractiveElement as _,
    IntoElement as _, ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, px,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::sources::events::{BlockEvent, emit};
use super::sources::{action, activate, entry_label};
use super::svg::SvgState;
use crate::timeline::TimelineEntry;

/// Tallest an image is drawn, in pixels; wider ones scale down to fit.
const MAX_HEIGHT: f32 = 360.0;

/// Why an image block cannot be drawn before loading it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The media type is not one GPUI decodes.
    #[error("unsupported image type `{0}`")]
    UnsupportedType(String),
    /// The inline payload is not valid base64.
    #[error("invalid base64 image data: {0}")]
    Base64(String),
}

/// Turns the block's data into something `img()` can load: decoded bytes
/// for inline data, the path for a file (read by GPUI's asset loader).
pub fn image_source(media_type: &str, data: &ImageData) -> Result<ImageSource, ImageError> {
    match data {
        ImageData::Base64 { data } => {
            let format = ImageFormat::from_mime_type(media_type)
                .ok_or_else(|| ImageError::UnsupportedType(media_type.to_owned()))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map_err(|e| ImageError::Base64(e.to_string()))?;
            Ok(Arc::new(Image::from_bytes(format, bytes)).into())
        }
        ImageData::Path { path } => Ok(PathBuf::from(path).into()),
    }
}

/// Short description of where the bytes come from, for the error view.
fn origin(data: &ImageData) -> String {
    match data {
        ImageData::Base64 { .. } => "inline image".to_owned(),
        ImageData::Path { path } => path.clone(),
    }
}

fn error_view(message: impl Into<SharedString>, danger: Hsla, border: Hsla) -> AnyElement {
    div()
        .p_2()
        .border_1()
        .border_color(border)
        .rounded_md()
        .text_color(danger)
        .child(message.into())
        .into_any_element()
}

/// The decoded source of one block, kept across frames so inline data is
/// decoded once rather than on every repaint.
struct Decoded(Result<ImageSource, ImageError>);

fn source_line(entry: &TimelineEntry, source: &SourceRef, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = div()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(format!(
            "source: {}",
            entry_label(source)
        )));
    let Some(event) = action(source) else {
        return label.into_any_element();
    };
    let id = ElementId::Name(SharedString::from(format!(
        "image-source-{}",
        entry.block.id
    )));
    label
        .id(id)
        .debug_selector(|| "image-source".to_owned())
        .cursor_pointer()
        .hover(|style| style.underline())
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            activate(&event, cx);
        })
        .into_any_element()
}

/// The picture, which a click opens in a modal to zoom in.
fn clickable(entry: &TimelineEntry, picture: AnyElement, inspect: BlockEvent) -> AnyElement {
    div()
        .id(ElementId::Name(SharedString::from(format!(
            "image-{}",
            entry.block.id
        ))))
        .debug_selector(|| "image-picture".to_owned())
        .cursor_pointer()
        .child(picture)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            emit(inspect.clone(), cx);
        })
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Image {
        media_type,
        data_or_path,
        source,
    } = &entry.block.kind
    else {
        return super::placeholder(entry, cx);
    };
    let (mime, data) = (media_type.clone(), data_or_path.clone());
    let decoded = super::block_state(entry.block.id, window, cx, move |_, _| {
        Decoded(image_source(&mime, &data))
    });
    let theme = cx.theme();
    let (danger, border) = (theme.danger, theme.border);
    let tint = super::svg::wants_tint(media_type).then(|| super::svg::Tint::from_theme(cx));
    let svg = super::svg::is_svg(media_type, data_or_path)
        .then(|| super::svg::use_svg_tinted(data_or_path, 1.0, tint, window, cx));
    let inspect = BlockEvent::InspectImage {
        block: entry.block.id,
        media_type: media_type.clone(),
        data: data_or_path.clone(),
    };
    let picture = match (svg, &decoded.read(cx).0) {
        (Some(SvgState::Ready(raster)), _) => {
            // Its natural size, scaled down to MAX_HEIGHT; the column
            // width shrinks it further, keeping the aspect ratio.
            let (w, h) = raster.size;
            let scale = (MAX_HEIGHT / h).min(1.0);
            clickable(
                entry,
                img(ImageSource::Render(raster.image.clone()))
                    .debug_selector(|| "svg-picture".to_owned())
                    .w(px(w * scale))
                    .max_w_full()
                    .aspect_ratio(w / h)
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                inspect,
            )
        }
        (Some(SvgState::Loading), _) => div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("rendering SVG…")
            .into_any_element(),
        (Some(SvgState::Failed(err)), _) => error_view(err, danger, border),
        (None, Ok(image)) => {
            let failed = format!("cannot load {}", origin(data_or_path));
            // Kept at its aspect ratio within the column and MAX_HEIGHT.
            clickable(
                entry,
                img(image.clone())
                    .max_w_full()
                    .max_h(px(MAX_HEIGHT))
                    .with_fallback(move || error_view(failed.clone(), danger, border))
                    .into_any_element(),
                inspect,
            )
        }
        (None, Err(err)) => error_view(err.to_string(), danger, border),
    };
    let mut block = v_flex().gap_1().child(picture);
    if let Some(source) = source {
        block = block.child(source_line(entry, source, cx));
    }
    block.into_any_element()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{Modifiers, TestAppContext};

    use super::*;
    use crate::blocks::sources::{BlockEvent, block_events};
    use crate::timeline::TimelineView;

    /// A 1×1 transparent PNG.
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn inline(data: &str) -> ImageData {
        ImageData::Base64 { data: data.into() }
    }

    #[test]
    fn inline_data_is_decoded_and_paths_pass_through() {
        assert!(matches!(
            image_source("image/png", &inline(PIXEL)),
            Ok(ImageSource::Image(_))
        ));
        let path = ImageData::Path {
            path: "shot.png".into(),
        };
        assert!(matches!(
            image_source("image/png", &path),
            Ok(ImageSource::Resource(_))
        ));
    }

    #[test]
    fn bad_inline_data_is_an_error() {
        assert_eq!(
            image_source("image/x-unknown", &inline(PIXEL)).err(),
            Some(ImageError::UnsupportedType("image/x-unknown".into()))
        );
        assert!(matches!(
            image_source("image/png", &inline("not base64!")),
            Err(ImageError::Base64(_))
        ));
    }

    #[gpui::test]
    fn draws_images_and_errors_and_the_source_links(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let image = |media_type: &str, data, source| {
            Block::new(BlockKind::Image {
                media_type: media_type.into(),
                data_or_path: data,
                source,
            })
        };
        let blocks = [
            image(
                "image/png",
                inline(PIXEL),
                Some(SourceRef::Url {
                    url: "https://example.com/shot".into(),
                    title: None,
                }),
            ),
            image("image/png", inline("%%%"), None),
            image(
                "image/png",
                ImageData::Path {
                    path: "/nonexistent/shot.png".into(),
                },
                None,
            ),
        ];
        view.update(cx, |view, cx| {
            view.apply_all(blocks.map(|block| DomainEvent::BlockAdded { block }), cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&events);
        cx.update(|_, cx| {
            let emitter = block_events(cx);
            cx.subscribe(&emitter, move |_, event: &BlockEvent, _| {
                recorded.borrow_mut().push(event.clone());
            })
            .detach();
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("image-source")
            .unwrap_or_else(|| panic!("source line not drawn"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/shot"));
        assert_eq!(events.borrow().len(), 1);
        // A click on the picture asks to inspect it.
        let picture = cx
            .debug_bounds("image-picture")
            .unwrap_or_else(|| panic!("picture not drawn"));
        cx.simulate_click(picture.center(), Modifiers::none());
        assert!(matches!(
            events.borrow().last(),
            Some(BlockEvent::InspectImage { media_type, .. }) if media_type == "image/png"
        ));
    }

    #[gpui::test]
    fn svg_is_rasterized_off_thread_at_its_natural_size(cx: &mut TestAppContext) {
        use crate::blocks::svg::{SQUARE, SvgState, cached};

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let data = inline(&base64::engine::general_purpose::STANDARD.encode(SQUARE));
        let block = Block::new(BlockKind::Image {
            media_type: "image/svg+xml".into(),
            data_or_path: data.clone(),
            source: None,
        });
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(800.), px(600.)),
                |_, _| div().size_full().child(view.clone()),
            );
        };
        draw(cx);
        cx.run_until_parked();
        draw(cx);
        let state = cx.update(|window, cx| cached(&data, 1.0, window, cx));
        let Some(SvgState::Ready(raster)) = state else {
            panic!("SVG not rasterized: {state:?}");
        };
        assert_eq!(raster.size, (40.0, 20.0));
        let picture = cx
            .debug_bounds("svg-picture")
            .unwrap_or_else(|| panic!("picture not drawn"));
        assert_eq!(picture.size.width, px(40.));
        assert_eq!(picture.size.height, px(20.));
    }
}
