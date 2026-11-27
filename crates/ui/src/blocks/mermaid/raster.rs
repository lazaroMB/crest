//! Mermaid source to a bitmap: the [`Diagram`] of [`super::diagram`] written
//! as SVG ([`to_svg`]), rasterized with resvg ([`rasterize`]) and handed to
//! GPUI as a [`RenderImage`], with a small LRU of results ([`RasterCache`]).
//!
//! The bitmap is premultiplied BGRA, like the images GPUI makes from SVG
//! itself: tiny-skia draws premultiplied RGBA and only the red and blue
//! channels are swapped.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, OnceLock};

use gpui::{RenderImage, Rgba};
use image::{Frame, RgbaImage};

use super::diagram::{Anchor, Diagram, Label, Segment, Shape, parse_with};
use crate::app::ColorTheme;

/// Below this natural width a diagram keeps its size instead of shrinking.
const SMALL_WIDTH: f32 = 240.0;
/// Entries kept by a [`RasterCache`].
const CACHE_CAP: usize = 64;
/// Line height of a label, as in the GPUI painter.
const LINE_HEIGHT: f32 = 1.2;
/// Distance from the vertical center of a text line to its baseline, in
/// ems: the GPUI painter centers the font's ascent..descent box on the
/// line, which for the usual UI fonts is about this far above the baseline.
const BASELINE_FROM_CENTER: f32 = 0.35;

/// A rasterized diagram.
#[derive(Clone)]
pub struct Raster {
    /// The bitmap (premultiplied BGRA), `round(width * scale)` pixels wide.
    pub image: Arc<RenderImage>,
    /// Logical width, in px.
    pub width: f32,
    /// Logical height, in px.
    pub height: f32,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `fill="..." fill-opacity="..."` style attributes for `color`.
fn paint_attr(out: &mut String, name: &str, color: Rgba) {
    let _ = write!(
        out,
        " {name}=\"rgb({},{},{})\"",
        byte(color.r),
        byte(color.g),
        byte(color.b)
    );
    if color.a < 1.0 {
        let _ = write!(out, " {name}-opacity=\"{:.3}\"", color.a.clamp(0.0, 1.0));
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
}

fn path_data(segments: &[Segment]) -> String {
    let mut d = String::new();
    for segment in segments {
        let _ = match *segment {
            Segment::Move(p) => write!(d, "M{} {} ", p.0, p.1),
            Segment::Line(p) => write!(d, "L{} {} ", p.0, p.1),
            Segment::Quad(c, p) => write!(d, "Q{} {} {} {} ", c.0, c.1, p.0, p.1),
            Segment::Cubic(a, b, p) => {
                write!(d, "C{} {} {} {} {} {} ", a.0, a.1, b.0, b.1, p.0, p.1)
            }
            Segment::Arc {
                radii,
                rotation,
                large,
                sweep,
                to,
            } => write!(
                d,
                "A{} {} {} {} {} {} {} ",
                radii.0,
                radii.1,
                rotation,
                u8::from(large),
                u8::from(sweep),
                to.0,
                to.1
            ),
            Segment::Close => write!(d, "Z "),
        };
    }
    d.truncate(d.trim_end().len());
    d
}

fn finite(segments: &[Segment]) -> bool {
    let ok = |p: (f32, f32)| p.0.is_finite() && p.1.is_finite();
    segments.iter().all(|segment| match *segment {
        Segment::Move(p) | Segment::Line(p) => ok(p),
        Segment::Quad(c, p) => ok(c) && ok(p),
        Segment::Cubic(a, b, p) => ok(a) && ok(b) && ok(p),
        Segment::Arc { radii, to, .. } => ok(radii) && ok(to),
        Segment::Close => true,
    })
}

fn text(label: &Label, family: &str, out: &mut String) {
    let lines = label.lines.len();
    if lines == 0 || !label.size.is_finite() || !label.at.0.is_finite() || !label.at.1.is_finite() {
        return;
    }
    let line_height = label.size * LINE_HEIGHT;
    // `label.at` is the vertical center of the whole block.
    let first_center = label.at.1 - line_height * (lines - 1) as f32 / 2.0;
    let anchor = match label.anchor {
        Anchor::Start => "start",
        Anchor::Middle => "middle",
        Anchor::End => "end",
    };
    let _ = write!(
        out,
        "<text xml:space=\"preserve\" font-family=\"{}\" font-size=\"{}\" text-anchor=\"{anchor}\"",
        family.replace('&', "&amp;").replace('"', "&quot;"),
        label.size
    );
    if label.bold {
        out.push_str(" font-weight=\"700\"");
    }
    paint_attr(out, "fill", label.color);
    out.push('>');
    for (ix, line) in label.lines.iter().enumerate() {
        let y = first_center + line_height * ix as f32 + label.size * BASELINE_FROM_CENTER;
        let _ = write!(out, "<tspan x=\"{}\" y=\"{y}\">", label.at.0);
        escape(line, out);
        out.push_str("</tspan>");
    }
    out.push_str("</text>");
}

/// An SVG document that draws `diagram` as the GPUI painter does, with
/// labels in `font_family`. The view box covers the content plus its
/// padding on every side; the content origin is `(0, 0)`.
#[must_use]
pub fn to_svg(diagram: &Diagram, font_family: &str) -> String {
    let pad = diagram.padding;
    let (x, y) = (-pad, -pad);
    let (w, h) = (diagram.width + pad * 2.0, diagram.height + pad * 2.0);
    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"{x} {y} {w} {h}\">"
    );
    let _ = write!(
        out,
        "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\""
    );
    paint_attr(&mut out, "fill", diagram.background);
    out.push_str("/>");
    for shape in &diagram.shapes {
        match shape {
            Shape::Path {
                segments,
                fill,
                stroke,
            } => {
                if segments.is_empty() || !finite(segments) {
                    continue;
                }
                let _ = write!(out, "<path d=\"{}\"", path_data(segments));
                match fill {
                    Some(fill) => paint_attr(&mut out, "fill", *fill),
                    None => out.push_str(" fill=\"none\""),
                }
                if let Some(stroke) = stroke {
                    paint_attr(&mut out, "stroke", stroke.color);
                    let _ = write!(out, " stroke-width=\"{}\"", stroke.width);
                    if let Some(dash) = &stroke.dash {
                        let list: Vec<String> = dash.iter().map(ToString::to_string).collect();
                        let _ = write!(out, " stroke-dasharray=\"{}\"", list.join(" "));
                    }
                }
                out.push_str("/>");
            }
            Shape::Text(label) => text(label, font_family, &mut out),
        }
    }
    out.push_str("</svg>");
    out
}

fn fontdb() -> &'static Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    })
}

/// Logical size of a diagram of natural size `natural` in at most
/// `max_width`: shrunk to fit, never enlarged, and left alone when small.
fn logical_size(natural: (f32, f32), max_width: f32) -> (f32, f32) {
    let width = if natural.0 < SMALL_WIDTH || max_width.is_nan() || max_width <= 0.0 {
        natural.0
    } else {
        natural.0.min(max_width)
    };
    (width, natural.1 * width / natural.0)
}

/// Rasterizes the Mermaid `source` in `colors` and `font_family` at
/// `scale` device pixels per logical px, at most `max_width` wide.
///
/// # Errors
///
/// The parser's message ([`parse_with`]), or why the SVG could not be drawn.
pub fn rasterize(
    source: &str,
    colors: &ColorTheme,
    font_family: &str,
    max_width: f32,
    scale: f32,
) -> Result<Raster, String> {
    let diagram = parse_with(source, colors)?;
    let svg = to_svg(&diagram, font_family);
    let mut options = usvg::Options {
        fontdb: Arc::clone(fontdb()),
        ..usvg::Options::default()
    };
    options.font_family = font_family.to_owned();
    let tree = usvg::Tree::from_str(&svg, &options)
        .map_err(|err| format!("cannot draw the diagram: {err}"))?;

    let natural = (
        diagram.width + diagram.padding * 2.0,
        diagram.height + diagram.padding * 2.0,
    );
    let (width, height) = logical_size(natural, max_width);
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let px_w = ((width * scale).round() as u32).clamp(1, 16_384);
    let px_h = ((height * scale).round() as u32).clamp(1, 16_384);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px_w, px_h)
        .ok_or_else(|| "the diagram is too large to draw".to_owned())?;
    let size = tree.size();
    let transform = resvg::tiny_skia::Transform::from_scale(
        px_w as f32 / size.width(),
        px_h as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut data = pixmap.take();
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let buffer = RgbaImage::from_raw(px_w, px_h, data)
        .ok_or_else(|| "the bitmap has the wrong size".to_owned())?;
    Ok(Raster {
        image: Arc::new(RenderImage::new([Frame::new(buffer)])),
        width,
        height,
    })
}

/// A hash of every color of `colors` (and its appearance): two themes with
/// the same fingerprint paint a diagram the same way.
#[must_use]
pub fn fingerprint(colors: &ColorTheme) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    colors.appearance.hash(&mut hasher);
    for (path, color) in colors.colors() {
        path.hash(&mut hasher);
        color.hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(PartialEq, Eq)]
struct Key {
    source: String,
    colors: u64,
    family: String,
    max_width: i32,
    scale: u32,
}

/// The last rasterized diagrams, least recently used dropped first.
#[derive(Default)]
pub struct RasterCache {
    /// Most recently used last.
    entries: VecDeque<(Key, Raster)>,
}

impl RasterCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The raster of `source`, drawn now (synchronously) when it is not
    /// cached. Errors are not cached.
    ///
    /// # Errors
    ///
    /// As [`rasterize`].
    pub fn get(
        &mut self,
        source: &str,
        colors: &ColorTheme,
        font_family: &str,
        max_width: f32,
        scale: f32,
    ) -> Result<Raster, String> {
        let key = Key {
            source: source.to_owned(),
            colors: fingerprint(colors),
            family: font_family.to_owned(),
            max_width: max_width.round() as i32,
            scale: (scale * 100.0).round() as u32,
        };
        if let Some(ix) = self.entries.iter().position(|(k, _)| *k == key)
            && let Some(entry) = self.entries.remove(ix)
        {
            let raster = entry.1.clone();
            self.entries.push_back(entry);
            return Ok(raster);
        }
        let raster = rasterize(source, colors, font_family, max_width, scale)?;
        if self.entries.len() >= CACHE_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back((key, raster.clone()));
        Ok(raster)
    }

    /// Forgets every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Entries held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = "flowchart LR\n  start[Idea] --> draft[Draft]\n  draft --> review{Review}\n  review -->|ok| publish[Publish]\n  review -->|changes| draft";
    const FAMILY: &str = "Helvetica Neue";

    fn colors() -> ColorTheme {
        crate::theme::default_theme()
    }

    fn template(name: &str) -> &'static str {
        domain::doc::DIAGRAM_TEMPLATES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
            .expect("template")
    }

    #[test]
    fn svg_has_background_paths_and_text() {
        let diagram = parse_with(FLOW, &colors()).unwrap();
        let svg = to_svg(&diagram, FAMILY);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<rect"), "{svg}");
        assert!(svg.contains("<path"));
        assert!(svg.contains("<text"));
        assert!(svg.contains(">Review</tspan>"));
        assert!(svg.contains("text-anchor=\"middle\""));
    }

    #[test]
    fn svg_escapes_text() {
        let diagram = Diagram {
            width: 10.0,
            height: 10.0,
            padding: 1.0,
            background: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            shapes: vec![Shape::Text(Label {
                at: (5.0, 5.0),
                lines: vec!["a<b & \"c\"".into(), "d".into()],
                anchor: Anchor::Start,
                size: 10.0,
                color: Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.5,
                },
                bold: true,
            })],
        };
        let svg = to_svg(&diagram, "A \"B\"");
        assert!(svg.contains("a&lt;b &amp; &quot;c&quot;"));
        assert!(svg.contains("font-weight=\"700\""));
        assert!(svg.contains("fill-opacity=\"0.500\""));
        assert_eq!(svg.matches("<tspan").count(), 2);
        usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("valid svg");
    }

    #[test]
    fn rasterizes_to_the_scaled_size() {
        let colors = colors();
        let r = rasterize(FLOW, &colors, FAMILY, 400.0, 2.0).unwrap();
        assert!(r.width <= 400.0 + 0.01);
        let size = r.image.size(0);
        assert_eq!(size.width.0, (r.width * 2.0).round() as i32);
        assert_eq!(size.height.0, (r.height * 2.0).round() as i32);
        let bytes = r.image.as_bytes(0).unwrap();
        let first = &bytes[..4];
        assert!(
            bytes.as_chunks::<4>().0.iter().any(|p| p[..] != *first),
            "all background"
        );
        // Background is the theme's canvas in BGRA.
        let bg = parse_with(FLOW, &colors).unwrap().background;
        assert_eq!(first[0], byte(bg.b));
        assert_eq!(first[2], byte(bg.r));
        assert_eq!(first[3], 255);
    }

    #[test]
    fn small_diagrams_keep_their_size() {
        assert_eq!(logical_size((200.0, 100.0), 100.0), (200.0, 100.0));
        assert_eq!(logical_size((800.0, 400.0), 400.0), (400.0, 200.0));
        assert_eq!(logical_size((300.0, 100.0), 1000.0), (300.0, 100.0));
    }

    #[test]
    fn templates_rasterize() {
        let colors = colors();
        for name in ["Use case", "Bar chart", "Sequence", "Pie", "Timeline"] {
            rasterize(template(name), &colors, FAMILY, 600.0, 2.0)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
        }
    }

    #[test]
    fn invalid_source_is_an_error() {
        assert!(rasterize("not a diagram at all {{", &colors(), FAMILY, 400.0, 2.0).is_err());
    }

    #[test]
    fn cache_returns_the_same_image_until_cleared() {
        let colors = colors();
        let mut cache = RasterCache::new();
        let a = cache.get(FLOW, &colors, FAMILY, 400.0, 2.0).unwrap();
        let b = cache.get(FLOW, &colors, FAMILY, 400.0, 2.0).unwrap();
        assert!(Arc::ptr_eq(&a.image, &b.image));
        let other = cache.get(FLOW, &colors, FAMILY, 300.0, 2.0).unwrap();
        assert!(!Arc::ptr_eq(&a.image, &other.image));
        cache.clear();
        assert!(cache.is_empty());
        let c = cache.get(FLOW, &colors, FAMILY, 400.0, 2.0).unwrap();
        assert!(!Arc::ptr_eq(&a.image, &c.image));
        assert!(cache.get("???", &colors, FAMILY, 400.0, 2.0).is_err());
    }

    #[test]
    fn cache_is_capped() {
        let colors = colors();
        let mut cache = RasterCache::new();
        for ix in 0..CACHE_CAP + 3 {
            cache
                .get(
                    &format!("flowchart LR\n  a --> b{ix}"),
                    &colors,
                    FAMILY,
                    300.0,
                    1.0,
                )
                .unwrap();
        }
        assert_eq!(cache.len(), CACHE_CAP);
    }

    /// Writes the flowchart to the scratchpad and times a cache miss.
    #[test]
    #[ignore = "writes a PNG and prints timings"]
    fn write_png() {
        let colors = colors();
        let ten = "flowchart TD\n  A[Start] --> B[One]\n  B --> C{Two}\n  C --> D[Three]\n  C --> E[Four]\n  D --> F[Five]\n  E --> F\n  F --> G[Six]\n  G --> H[Seven]\n  H --> I[Eight]\n  I --> J[Done]";
        for (name, source) in [("flowchart", FLOW), ("ten", ten)] {
            let t = std::time::Instant::now();
            let r = rasterize(source, &colors, FAMILY, 900.0, 2.0).unwrap();
            println!("{name}: {:?} ({}x{})", t.elapsed(), r.width, r.height);
            let t = std::time::Instant::now();
            rasterize(source, &colors, FAMILY, 900.0, 2.0).unwrap();
            println!("{name} second: {:?}", t.elapsed());
            let size = r.image.size(0);
            let mut bytes = r.image.as_bytes(0).unwrap().to_vec();
            for p in bytes.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            let dir = "/private/tmp/claude-501/-Users-alex-dev-crest/2b40e5dd-b00c-48ba-a8a5-865daa8d33de/scratchpad/shots";
            std::fs::create_dir_all(dir).unwrap();
            RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes)
                .unwrap()
                .save(format!("{dir}/raster-{name}.png"))
                .unwrap();
        }
    }
}
