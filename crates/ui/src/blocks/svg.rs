//! SVG pictures of `image` blocks and image nodes, rasterized with resvg
//! off the UI thread.
//!
//! GPUI can decode an SVG itself, but on the UI thread and at a fixed
//! scale. Here the bytes (inline or read from the path) are parsed and
//! drawn on the background executor at the window's scale factor, times a
//! power-of-two zoom bucket so a zoomed modal stays sharp. Results are
//! kept in a global cache keyed by (data hash, raster scale, tint); a
//! request that is still running shows as [`SvgState::Loading`] and the
//! windows are refreshed when it lands. The cache keeps at most
//! [`CACHE_BYTES`] of bitmaps, dropping the least recently drawn first.
//!
//! SVGs drawn for white paper (molecule depictions, WaveDrom diagrams,
//! images whose media type carries [`TINT_PARAM`]) can be tinted to the
//! theme first ([`tint_svg`]): black strokes and text take the theme's
//! foreground, the white background becomes transparent and other white
//! fills (label masks) the surface color, light grey grid lines the
//! border color. Every other color (CPK heteroatoms) is kept. Other SVGs
//! are drawn with their own colors.

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

/// Media type parameter asking for the SVG to be tinted to the theme,
/// e.g. `image/svg+xml; tint=theme`.
pub const TINT_PARAM: &str = "tint=theme";

/// Largest side of a rasterized SVG, in device pixels.
const MAX_PIXELS: f32 = 8192.0;
/// Largest zoom bucket; zooming further scales the bitmap up.
const MAX_BUCKET: f32 = 8.0;

/// Whether an image of `media_type` (or at `path`) is an SVG.
#[must_use]
pub fn is_svg(media_type: &str, data: &ImageData) -> bool {
    let base = media_type.split(';').next().unwrap_or_default();
    let media_type = base.trim().to_ascii_lowercase();
    media_type == SVG_MEDIA_TYPE
        || media_type == "image/svg"
        || matches!(data, ImageData::Path { path } if path.to_ascii_lowercase().ends_with(".svg"))
}

/// Whether `media_type` asks for the theme tint ([`TINT_PARAM`]).
#[must_use]
pub fn wants_tint(media_type: &str) -> bool {
    media_type
        .split(';')
        .skip(1)
        .any(|p| p.trim().eq_ignore_ascii_case(TINT_PARAM))
}

/// The colors [`tint_svg`] paints with, as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tint {
    /// Strokes and text drawn black.
    pub foreground: String,
    /// Fills drawn white, other than the background.
    pub surface: String,
    /// Light grey lines (grids).
    pub muted: String,
}

impl Tint {
    /// The tint of the current theme.
    pub fn from_theme(cx: &App) -> Self {
        let theme = gpui_component::ActiveTheme::theme(cx);
        Self {
            foreground: hex(theme.foreground),
            surface: hex(theme.background),
            muted: hex(theme.border),
        }
    }
}

/// `color` as `#rrggbb`, alpha dropped.
fn hex(color: gpui::Hsla) -> String {
    let rgba = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b)
    )
}

/// What a paint value of an SVG drawn for white paper is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paint {
    Black,
    White,
    LightGrey,
    Other,
}

fn paint(value: &str) -> Paint {
    match value.trim().to_ascii_lowercase().as_str() {
        "black" | "#000" | "#000000" | "rgb(0,0,0)" => Paint::Black,
        "white" | "#fff" | "#ffffff" | "rgb(255,255,255)" => Paint::White,
        "#ccc" | "#cccccc" | "lightgrey" | "lightgray" => Paint::LightGrey,
        _ => Paint::Other,
    }
}

/// `svg` with black, white and light grey paints (in `fill` and `stroke`
/// attributes and style declarations) swapped for `tint`'s colors; the
/// first white `rect`, the background, becomes `none`.
#[must_use]
pub fn tint_svg(svg: &str, tint: &Tint) -> String {
    let mut out = String::with_capacity(svg.len() + 64);
    let mut background_done = false;
    let mut rest = svg;
    let mut consumed = 0;
    while let Some((start, name_len)) = next_paint(rest) {
        let after_name = &rest[start + name_len..];
        // `fill="v"`, `fill='v'` or `fill: v` (style).
        let (value_start, end) = match after_name.as_bytes().first() {
            Some(b'=') => {
                let quote = after_name.as_bytes().get(1).copied();
                let Some(quote @ (b'"' | b'\'')) = quote else {
                    out.push_str(&rest[..start + name_len]);
                    consumed += start + name_len;
                    rest = &svg[consumed..];
                    continue;
                };
                let body = &after_name[2..];
                let len = body.find(char::from(quote)).unwrap_or(body.len());
                (start + name_len + 2, start + name_len + 2 + len)
            }
            Some(b':') => {
                let body = &after_name[1..];
                let len = body.find([';', '"', '\'', '}']).unwrap_or(body.len());
                (start + name_len + 1, start + name_len + 1 + len)
            }
            _ => {
                out.push_str(&rest[..start + name_len]);
                consumed += start + name_len;
                rest = &svg[consumed..];
                continue;
            }
        };
        out.push_str(&rest[..value_start]);
        let value = &rest[value_start..end];
        let element_is_rect = rest[..start]
            .rfind('<')
            .map(|lt| rest[lt..].starts_with("<rect"))
            .or_else(|| out.rfind('<').map(|lt| out[lt..].starts_with("<rect")))
            .unwrap_or(false);
        let replaced = match paint(value) {
            Paint::Black => tint.foreground.as_str(),
            Paint::White if !background_done && element_is_rect => {
                background_done = true;
                "none"
            }
            Paint::White => {
                background_done = true;
                tint.surface.as_str()
            }
            Paint::LightGrey => tint.muted.as_str(),
            Paint::Other => value,
        };
        out.push_str(replaced);
        consumed += end;
        rest = &svg[consumed..];
    }
    out.push_str(rest);
    out
}

/// The next `fill` or `stroke` property name in `text` that starts a
/// name (not inside `fill-opacity` or `stroke-width`): its offset and
/// length.
fn next_paint(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut from = 0;
    loop {
        let hit = [("fill", 4), ("stroke", 6)]
            .into_iter()
            .filter_map(|(name, len)| text[from..].find(name).map(|at| (from + at, len)))
            .min_by_key(|(at, _)| *at)?;
        let (at, len) = hit;
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + len).copied();
        let starts =
            before.is_none_or(|b| b.is_ascii_whitespace() || b == b';' || b == b'"' || b == b'{');
        let separated = matches!(after, Some(b'=' | b':'));
        if starts && separated {
            return Some(hit);
        }
        from = at + len;
    }
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

/// Cache key: the data and tint hash and the raster scale (to the
/// hundredth).
type Key = (u64, u32);

/// Most bytes of bitmaps the cache keeps.
pub const CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Bytes a cache entry holds: its bitmap's, 0 while loading or failed.
fn state_bytes(state: &SvgState) -> usize {
    match state {
        SvgState::Ready(raster) => {
            let size = raster.image.size(0);
            usize::try_from(size.width.0).unwrap_or(0)
                * usize::try_from(size.height.0).unwrap_or(0)
                * 4
        }
        SvgState::Loading | SvgState::Failed(_) => 0,
    }
}

/// A map bounded by the bytes of its values: inserting past `cap` drops
/// the least recently used entries that have bytes (never the one just
/// inserted, and never an entry of 0 bytes, such as one still loading).
#[derive(Debug)]
pub(crate) struct Lru<K, V> {
    cap: usize,
    bytes: usize,
    tick: u64,
    entries: HashMap<K, (V, usize, u64)>,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
        }
    }

    /// The value of `key`, marked as just used.
    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|(value, _, used)| {
            *used = tick;
            value.clone()
        })
    }

    /// Stores `value` of `size` bytes under `key`, then evicts down to the
    /// cap.
    pub(crate) fn insert(&mut self, key: K, value: V, size: usize) {
        self.tick += 1;
        if let Some((_, old, _)) = self.entries.insert(key.clone(), (value, size, self.tick)) {
            self.bytes -= old;
        }
        self.bytes += size;
        while self.bytes > self.cap {
            let victim = self
                .entries
                .iter()
                .filter(|(k, (_, size, _))| *size > 0 && **k != key)
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(victim) = victim else {
                break;
            };
            if let Some((_, size, _)) = self.entries.remove(&victim) {
                self.bytes -= size;
            }
        }
    }

    /// Bytes held.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }
}

struct SvgCache(Lru<Key, SvgState>);

impl Default for SvgCache {
    fn default() -> Self {
        Self(Lru::new(CACHE_BYTES))
    }
}

impl Global for SvgCache {}

fn key(data: &ImageData, scale: f32, tint: Option<&Tint>) -> Key {
    let mut hasher = DefaultHasher::new();
    match data {
        ImageData::Base64 { data } => ("b64", data).hash(&mut hasher),
        ImageData::Path { path } => ("path", path).hash(&mut hasher),
    }
    tint.hash(&mut hasher);
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
    use_svg_tinted(data, zoom, None, window, cx)
}

/// [`use_svg`], tinted with `tint` first when given ([`tint_svg`]).
pub fn use_svg_tinted(
    data: &ImageData,
    zoom: f32,
    tint: Option<Tint>,
    window: &Window,
    cx: &mut App,
) -> SvgState {
    let scale = raster_scale(window.scale_factor(), zoom);
    let key = key(data, scale, tint.as_ref());
    if let Some(state) = cx.default_global::<SvgCache>().0.get(&key) {
        return state;
    }
    cx.default_global::<SvgCache>()
        .0
        .insert(key, SvgState::Loading, 0);
    let data = data.clone();
    let task = cx.background_executor().spawn(async move {
        let bytes = bytes(&data)?;
        match &tint {
            Some(tint) => {
                let text = String::from_utf8_lossy(&bytes);
                rasterize(tint_svg(&text, tint).as_bytes(), scale)
            }
            None => rasterize(&bytes, scale),
        }
    });
    cx.spawn(async move |cx| {
        let state = match task.await {
            Ok(raster) => SvgState::Ready(Arc::new(raster)),
            Err(err) => SvgState::Failed(err),
        };
        cx.update(|cx| {
            let size = state_bytes(&state);
            cx.default_global::<SvgCache>().0.insert(key, state, size);
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
    let key = key(data, raster_scale(window.scale_factor(), zoom), None);
    cx.default_global::<SvgCache>().0.get(&key)
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

    fn tint() -> Tint {
        Tint {
            foreground: "#eeeeee".into(),
            surface: "#111111".into(),
            muted: "#444444".into(),
        }
    }

    #[test]
    fn tinting_recolors_black_white_and_grey_only() {
        let molecule = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
  <rect x="0" y="0" width="40" height="20" fill="white"/>
  <line x1="0" y1="0" x2="10" y2="10" stroke="black" stroke-width="1.5" fill="none"/>
  <rect x="5" y="5" width="8" height="8" fill="white"/>
  <text fill="#FF0D0D" stroke-width="0">O</text>
  <path style="fill:#000;stroke:#CCC" fill-opacity="1"/>
</svg>"##;
        let tinted = tint_svg(molecule, &tint());
        assert!(tinted.contains(r#"height="20" fill="none"/>"#), "{tinted}");
        assert!(tinted.contains(r##"stroke="#eeeeee" stroke-width="1.5" fill="none""##));
        assert!(tinted.contains(r##"height="8" fill="#111111""##));
        assert!(tinted.contains(r##"fill="#FF0D0D""##), "CPK kept");
        assert!(tinted.contains("fill:#eeeeee;stroke:#444444"), "{tinted}");
        assert!(tinted.contains(r#"fill-opacity="1""#));
        assert!(!tinted.contains("black") && !tinted.contains("white"));
        // Still a valid SVG.
        assert!(rasterize(tinted.as_bytes(), 1.0).is_ok());
        // The WaveDrom background.
        let wave = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="100%" height="100%" fill="#FFF"/><text fill="#000">clk</text></svg>"##;
        let tinted = tint_svg(wave, &tint());
        assert!(tinted.contains(r#"height="100%" fill="none""#), "{tinted}");
        assert!(tinted.contains(r##"<text fill="#eeeeee">"##), "{tinted}");
        assert!(wants_tint("image/svg+xml; tint=theme"));
        assert!(!wants_tint("image/svg+xml"));
        assert!(is_svg(
            "image/svg+xml; tint=theme",
            &ImageData::Base64 {
                data: String::new()
            }
        ));
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_bitmaps_past_its_cap() {
        let mut lru: Lru<&str, u8> = Lru::new(100);
        lru.insert("a", 1, 40);
        lru.insert("b", 2, 40);
        lru.insert("loading", 0, 0);
        assert_eq!(lru.get(&"a"), Some(1));
        // 120 bytes: `b`, used least recently, goes.
        lru.insert("c", 3, 40);
        assert!(!lru.contains(&"b") && lru.contains(&"a") && lru.contains(&"c"));
        assert!(lru.contains(&"loading"));
        assert_eq!(lru.bytes(), 80);
        // Replacing an entry counts its new size only.
        lru.insert("a", 4, 10);
        assert_eq!(lru.bytes(), 50);
        // One entry over the cap stays: nothing else to drop.
        lru.insert("huge", 5, 500);
        assert!(lru.contains(&"huge"));
        assert_eq!(lru.bytes(), 500);
        let raster = rasterize(SQUARE.as_bytes(), 1.0).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(state_bytes(&SvgState::Ready(Arc::new(raster))), 40 * 20 * 4);
        assert_eq!(state_bytes(&SvgState::Loading), 0);
    }

    #[test]
    fn a_landed_bitmap_evicts_older_ones_but_not_itself() {
        let mut lru: Lru<&str, u8> = Lru::new(100);
        lru.insert("old", 1, 60);
        lru.insert("new", 0, 0);
        assert_eq!(lru.bytes(), 60);
        // The placeholder becomes a bitmap: the older one makes room.
        lru.insert("new", 2, 60);
        assert!(!lru.contains(&"old"));
        assert_eq!(lru.get(&"new"), Some(2));
        assert_eq!(lru.get(&"missing"), None);
        assert_eq!(lru.bytes(), 60);
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
