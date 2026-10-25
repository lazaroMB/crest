//! SVG pictures of `image` blocks and image nodes, rasterized with resvg
//! off the UI thread.
//!
//! GPUI can decode an SVG itself, but on the UI thread and at a fixed
//! scale. Here the bytes (inline or read from the path) are parsed and
//! drawn on the background executor at the window's scale factor, times a
//! power-of-two zoom bucket so a zoomed modal stays sharp. Results are
//! kept in a global cache keyed by (data hash, raster scale); a request
//! that is still running shows as [`SvgState::Loading`] and the windows
//! are refreshed when it lands. Nothing is tinted: the SVG's own colors
//! are drawn.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, LazyLock};

use base64::Engine as _;
use domain::block::ImageData;
use gpui::{App, Global, RenderImage, Window};
use resvg::tiny_skia;

/// The media type of SVG.
pub const SVG_MEDIA_TYPE: &str = "image/svg+xml";

/// Largest side of a rasterized SVG, in device pixels.
const MAX_PIXELS: f32 = 8192.0;
/// Largest zoom bucket; zooming further scales the bitmap up.
const MAX_BUCKET: f32 = 8.0;

/// Whether an image of `media_type` (or at `path`) is an SVG.
#[must_use]
pub fn is_svg(media_type: &str, data: &ImageData) -> bool {
    let media_type = media_type.trim().to_ascii_lowercase();
    media_type == SVG_MEDIA_TYPE
        || media_type == "image/svg"
        || matches!(data, ImageData::Path { path } if path.to_ascii_lowercase().ends_with(".svg"))
}

/// A rasterized SVG.
#[derive(Debug)]
pub struct Rasterized {
    /// The bitmap, in device pixels.
    pub image: Arc<RenderImage>,
    /// Natural size of the SVG in logical pixels (its width and height).
    pub size: (f32, f32),
}

/// Where an SVG is in the cache.
#[derive(Debug, Clone)]
pub enum SvgState {
    /// Being rasterized.
    Loading,
    /// Ready to draw.
    Ready(Arc<Rasterized>),
    /// It could not be read or parsed.
    Failed(String),
}

/// The fonts SVG text is drawn with, loaded once, on first use.
static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

/// Parses `bytes` and draws them at `scale` device pixels per logical
/// pixel.
///
/// # Errors
///
/// The SVG does not parse or has an empty size.
pub fn rasterize(bytes: &[u8], scale: f32) -> Result<Rasterized, String> {
    let options = usvg::Options {
        font_family: "sans-serif".to_owned(),
        fontdb: Arc::clone(&FONTS),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|e| format!("invalid SVG: {e}"))?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    // Keep the bitmap within MAX_PIXELS on its longer side.
    let scale = scale.max(0.1).min(MAX_PIXELS / w.max(h).max(1.0));
    let (pw, ph) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
    let mut pixmap =
        tiny_skia::Pixmap::new(pw.max(1), ph.max(1)).ok_or_else(|| "empty SVG".to_owned())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let (pw, ph) = (pixmap.width(), pixmap.height());
    let mut pixels = pixmap.take();
    // tiny-skia draws premultiplied RGBA; GPUI wants straight BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        let alpha = f32::from(pixel[3]);
        if alpha > 0.0 && alpha < 255.0 {
            for channel in &mut pixel[..3] {
                *channel = (f32::from(*channel) * 255.0 / alpha).round().min(255.0) as u8;
            }
        }
    }
    let buffer = image::RgbaImage::from_raw(pw, ph, pixels)
        .ok_or_else(|| "SVG bitmap has the wrong size".to_owned())?;
    let frame = image::Frame::new(buffer);
    Ok(Rasterized {
        image: Arc::new(RenderImage::new(vec![frame])),
        size: (w, h),
    })
}

/// The raster scale for a window scale factor and a zoom level: the scale
/// factor times the zoom rounded up to a power of two (1 to
/// [`MAX_BUCKET`]).
#[must_use]
pub fn raster_scale(scale_factor: f32, zoom: f32) -> f32 {
    let zoom = if zoom.is_finite() { zoom } else { 1.0 };
    let bucket = zoom.clamp(1.0, MAX_BUCKET).log2().ceil().exp2();
    scale_factor.max(1.0) * bucket
}

/// Cache key: the data hash and the raster scale (to the hundredth).
type Key = (u64, u32);

#[derive(Default)]
struct SvgCache(HashMap<Key, SvgState>);

impl Global for SvgCache {}

fn key(data: &ImageData, scale: f32) -> Key {
    let mut hasher = DefaultHasher::new();
    match data {
        ImageData::Base64 { data } => ("b64", data).hash(&mut hasher),
        ImageData::Path { path } => ("path", path).hash(&mut hasher),
    }
    (hasher.finish(), (scale * 100.0).round() as u32)
}

/// The SVG bytes of `data`: decoded base64, or the file.
fn bytes(data: &ImageData) -> Result<Vec<u8>, String> {
    match data {
        ImageData::Base64 { data } => base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| format!("invalid base64 image data: {e}")),
        ImageData::Path { path } => {
            std::fs::read(path).map_err(|e| format!("cannot load {path}: {e}"))
        }
    }
}

/// The SVG of `data` drawn for this window at `zoom` (1.0 when fitted),
/// starting the rasterization on first use.
pub fn use_svg(data: &ImageData, zoom: f32, window: &Window, cx: &mut App) -> SvgState {
    let scale = raster_scale(window.scale_factor(), zoom);
    let key = key(data, scale);
    if let Some(state) = cx.default_global::<SvgCache>().0.get(&key) {
        return state.clone();
    }
    cx.default_global::<SvgCache>()
        .0
        .insert(key, SvgState::Loading);
    let data = data.clone();
    let task = cx
        .background_executor()
        .spawn(async move { bytes(&data).and_then(|bytes| rasterize(&bytes, scale)) });
    cx.spawn(async move |cx| {
        let state = match task.await {
            Ok(raster) => SvgState::Ready(Arc::new(raster)),
            Err(err) => SvgState::Failed(err),
        };
        cx.update(|cx| {
            cx.default_global::<SvgCache>().0.insert(key, state);
            cx.refresh_windows();
        });
    })
    .detach();
    SvgState::Loading
}

/// The cached state of `data` at `zoom` for this window, without starting
/// anything (tests).
#[cfg(test)]
pub(crate) fn cached(
    data: &ImageData,
    zoom: f32,
    window: &Window,
    cx: &mut App,
) -> Option<SvgState> {
    let key = key(data, raster_scale(window.scale_factor(), zoom));
    cx.default_global::<SvgCache>().0.get(&key).cloned()
}

/// A 40×20 red rectangle, for tests.
#[cfg(test)]
pub(crate) const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20"><rect width="40" height="20" fill="#ff0000"/></svg>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_at_the_scale_with_the_natural_size() {
        let raster = rasterize(SQUARE.as_bytes(), 2.0).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(raster.size, (40.0, 20.0));
        let pixels = raster.image.size(0);
        assert_eq!((pixels.width.0, pixels.height.0), (80, 40));
        // Opaque red, as BGRA.
        let bytes = raster.image.as_bytes(0).unwrap_or_default();
        assert_eq!(&bytes[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn bad_svg_is_an_error() {
        assert!(rasterize(b"<svg", 1.0).is_err());
        assert!(rasterize(b"not xml at all", 1.0).is_err());
    }

    #[test]
    fn detection_and_scale_buckets() {
        let inline = ImageData::Base64 {
            data: String::new(),
        };
        assert!(is_svg("image/svg+xml", &inline));
        assert!(!is_svg("image/png", &inline));
        let path = ImageData::Path {
            path: "out/wave.SVG".into(),
        };
        assert!(is_svg("", &path));
        assert_eq!(raster_scale(2.0, 1.0), 2.0);
        assert_eq!(raster_scale(1.0, 0.3), 1.0);
        assert_eq!(raster_scale(1.0, 3.0), 4.0);
        assert_eq!(raster_scale(1.5, 100.0), 12.0);
        assert_eq!(raster_scale(1.0, f32::NAN), 1.0);
    }
}
