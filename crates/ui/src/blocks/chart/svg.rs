//! A chart block as a picture for a document: [`to_svg`] writes the marks
//! the GPUI painter draws (grid, baseline, bars, lines and areas, markers,
//! labels, legend) plus the title, and [`to_png`] rasterizes it with resvg.
//!
//! Text is measured with an average glyph advance instead of the window's
//! text system, so the picture needs no window and is the same on every
//! run. Pies are drawn by gpui-component in the app, not from a layout, so
//! they are not pictured ([`to_png`] says so and the caller falls back to
//! the table).

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use domain::block::{ChartKind, ChartSpec};
use domain::theme::{Appearance, Color, Theme};

use super::layout::{
    Align, BarMark, ChartData, LABEL_HEIGHT, Label, MARKER_SIZE, Orientation, Rect, plan,
};
use super::paint::{AREA_OPACITY, BAR_RADIUS, LINE_WIDTH, MARKER_RING};

/// Font of every text of the picture.
const FONT_FAMILY: &str = "Helvetica, Arial, sans-serif";
/// Size of the labels, as in the app (`rem * 0.75`).
const LABEL_SIZE: f32 = 12.0;
/// Size of the title.
const TITLE_SIZE: f32 = 15.0;
/// Average advance of a glyph, in ems.
const ADVANCE: f32 = 0.6;
/// Distance from the middle of a text line to its baseline, in ems.
const BASELINE_FROM_CENTER: f32 = 0.35;
/// Height of the title band.
const TITLE_BAND: f32 = 32.0;
/// Height of a legend row.
const LEGEND_ROW: f32 = 20.0;
/// Side of a legend swatch.
const SWATCH: f32 = 10.0;
/// Space between a legend swatch and its name, and between entries.
const LEGEND_GAP: f32 = 6.0;
const LEGEND_SPACE: f32 = 16.0;

/// The colors of a picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Page color; also the ring around markers.
    pub background: Color,
    /// Title and value labels.
    pub strong: Color,
    /// Tick, category and legend labels.
    pub muted: Color,
    /// Grid lines.
    pub grid: Color,
    /// The baseline at zero.
    pub baseline: Color,
    /// Series colors, cycled.
    pub series: [Color; 5],
}

impl Palette {
    /// The light, print-friendly palette, derived from the light theme
    /// defaults whatever the app's current theme.
    #[must_use]
    pub fn light() -> &'static Palette {
        static PALETTE: OnceLock<Palette> = OnceLock::new();
        PALETTE.get_or_init(|| {
            let theme = Theme::derive(
                "export",
                Appearance::Light,
                &Default::default(),
                &Default::default(),
            );
            let opaque = |path: &str| Color {
                a: 255,
                ..theme.color(path)
            };
            Palette {
                background: opaque("background"),
                strong: opaque("foreground"),
                muted: opaque("muted.fg"),
                grid: opaque("chart.grid"),
                baseline: opaque("border"),
                series: ["chart.1", "chart.2", "chart.3", "chart.4", "chart.5"].map(opaque),
            }
        })
    }

    fn series(&self, ix: usize) -> Color {
        self.series[ix % self.series.len()]
    }
}

fn rgb(c: Color) -> String {
    format!("rgb({},{},{})", c.r, c.g, c.b)
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
}

fn measure(text: &str) -> f32 {
    text.chars().count() as f32 * LABEL_SIZE * ADVANCE
}

fn text(
    out: &mut String,
    x: f32,
    baseline: f32,
    anchor: &str,
    (size, bold): (f32, bool),
    color: Color,
    s: &str,
) {
    let _ = write!(
        out,
        "<text x=\"{x}\" y=\"{baseline}\" font-family=\"{FONT_FAMILY}\" font-size=\"{size}\" text-anchor=\"{anchor}\" xml:space=\"preserve\" fill=\"{}\"",
        rgb(color)
    );
    if bold {
        out.push_str(" font-weight=\"700\"");
    }
    out.push('>');
    escape(s, out);
    out.push_str("</text>");
}

fn label(out: &mut String, l: &Label, dy: f32, color: Color) {
    if l.text.is_empty() {
        return;
    }
    let anchor = match l.align {
        Align::Left => "start",
        Align::Center => "middle",
        Align::Right => "end",
    };
    let baseline = l.y + dy + LABEL_HEIGHT / 2.0 + LABEL_SIZE * BASELINE_FROM_CENTER;
    text(
        out,
        l.x,
        baseline,
        anchor,
        (LABEL_SIZE, false),
        color,
        &l.text,
    );
}

/// A rectangle as a path with the given corner radii (tl, tr, br, bl).
fn rounded(r: Rect, dy: f32, radii: [f32; 4]) -> String {
    let [tl, tr, br, bl] = radii;
    let (x0, y0, x1, y1) = (r.x, r.y + dy, r.right(), r.bottom() + dy);
    format!(
        "M{} {y0}H{}A{tr} {tr} 0 0 1 {x1} {}V{}A{br} {br} 0 0 1 {} {y1}H{}A{bl} {bl} 0 0 1 {x0} {}V{}A{tl} {tl} 0 0 1 {} {y0}Z",
        x0 + tl,
        x1 - tr,
        y0 + tr,
        y1 - br,
        x1 - br,
        x0 + bl,
        y1 - bl,
        y0 + tl,
        x0 + tl,
    )
}

/// Corner radii of a bar, rounded only at the data end (as
/// [`super::paint::bar_corners`]).
fn corners(bar: &BarMark, orientation: Orientation) -> [f32; 4] {
    let r = BAR_RADIUS
        .min(bar.rect.w / 2.0)
        .min(bar.rect.h / 2.0)
        .max(0.0);
    match (orientation, bar.negative) {
        (Orientation::Vertical, false) => [r, r, 0.0, 0.0],
        (Orientation::Vertical, true) => [0.0, 0.0, r, r],
        (Orientation::Horizontal, false) => [0.0, r, r, 0.0],
        (Orientation::Horizontal, true) => [r, 0.0, 0.0, r],
    }
}

fn legend_names(spec: &ChartSpec) -> Vec<String> {
    spec.series
        .iter()
        .map(|s| {
            if s.name.trim().is_empty() {
                "value".to_owned()
            } else {
                s.name.clone()
            }
        })
        .collect()
}

/// Rows of legend entries wrapped at `width`: `(x, row, entry)`.
fn legend_rows(names: &[String], width: f32) -> (Vec<(f32, usize, usize)>, usize) {
    let mut placed = Vec::new();
    let (mut x, mut row) = (0.0f32, 0usize);
    for (ix, name) in names.iter().enumerate() {
        let w = SWATCH + LEGEND_GAP + measure(name);
        if x > 0.0 && x + w > width {
            x = 0.0;
            row += 1;
        }
        placed.push((x, row, ix));
        x += w + LEGEND_SPACE;
    }
    (placed, row + 1)
}

/// The chart as an SVG document `width` px wide: a title, the plot as the
/// GPUI painter draws it, and a legend for two or more series. Every string
/// of `spec` is escaped.
#[must_use]
pub fn to_svg(spec: &ChartSpec, width: f32, palette: &Palette) -> String {
    let width = width.max(1.0);
    let data = ChartData::from_spec(spec);
    let layout = plan(&data, width, &measure);
    let title = if spec.title.trim().is_empty() {
        "Chart"
    } else {
        spec.title.trim()
    };
    let dy = TITLE_BAND;
    let names = legend_names(spec);
    let (placed, rows) = if spec.series.len() >= 2 {
        legend_rows(&names, width - 2.0 * super::layout::PAD)
    } else {
        (Vec::new(), 0)
    };
    let legend_h = if rows > 0 {
        8.0 + rows as f32 * LEGEND_ROW
    } else {
        0.0
    };
    let height = dy + layout.height + legend_h + super::layout::PAD;

    let mut out = String::with_capacity(8192);
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
    );
    let _ = write!(
        out,
        "<rect width=\"{width}\" height=\"{height}\" fill=\"{}\"/>",
        rgb(palette.background)
    );
    text(
        &mut out,
        super::layout::PAD,
        TITLE_BAND / 2.0 + TITLE_SIZE * BASELINE_FROM_CENTER + 2.0,
        "start",
        (TITLE_SIZE, true),
        palette.strong,
        title,
    );

    // Grid and baseline.
    let plot = &layout.plot;
    let line = |out: &mut String, pos: f32, color: Color| {
        let r = match layout.orientation {
            Orientation::Vertical => Rect::new(plot.x, pos.round(), plot.w, 1.0),
            Orientation::Horizontal => Rect::new(pos.round(), plot.y, 1.0, plot.h),
        };
        let _ = write!(
            out,
            "<rect class=\"grid\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
            r.x,
            r.y + dy,
            r.w,
            r.h,
            rgb(color)
        );
    };
    for tick in &layout.ticks {
        line(&mut out, tick.pos, palette.grid);
    }
    if let Some(base) = layout.baseline {
        line(&mut out, base, palette.baseline);
    }

    // Marks.
    match data.kind {
        ChartKind::Line | ChartKind::Area => {
            let filled = data.kind == ChartKind::Area;
            let base = layout.baseline.unwrap_or(plot.bottom());
            for (series, points) in layout.points.iter().enumerate() {
                let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
                    continue;
                };
                let color = palette.series(series);
                if filled && points.len() > 1 {
                    let mut d = format!("M{} {}", first.0, base + dy);
                    for &(x, y) in points {
                        let _ = write!(d, "L{x} {}", y + dy);
                    }
                    let _ = write!(d, "L{} {}Z", last.0, base + dy);
                    let _ = write!(
                        out,
                        "<path class=\"area\" d=\"{d}\" fill=\"{}\" fill-opacity=\"{AREA_OPACITY}\"/>",
                        rgb(color)
                    );
                }
                if points.len() > 1 {
                    let mut d = String::new();
                    for (i, &(x, y)) in points.iter().enumerate() {
                        let _ = write!(d, "{}{x} {}", if i == 0 { "M" } else { "L" }, y + dy);
                    }
                    let _ = write!(
                        out,
                        "<path class=\"line\" d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{LINE_WIDTH}\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>",
                        rgb(color)
                    );
                }
                if layout.markers {
                    let outer = MARKER_SIZE + 2.0 * MARKER_RING;
                    for &(x, y) in points {
                        let _ = write!(
                            out,
                            "<circle class=\"marker\" cx=\"{x}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/><circle cx=\"{x}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
                            y + dy,
                            outer / 2.0,
                            rgb(palette.background),
                            y + dy,
                            MARKER_SIZE / 2.0,
                            rgb(color)
                        );
                    }
                }
            }
        }
        ChartKind::Bar | ChartKind::Pie => {
            for bar in &layout.bars {
                if bar.rect.w <= 0.0 || bar.rect.h <= 0.0 {
                    continue;
                }
                let _ = write!(
                    out,
                    "<path class=\"bar\" d=\"{}\" fill=\"{}\"/>",
                    rounded(bar.rect, dy, corners(bar, layout.orientation)),
                    rgb(palette.series(bar.series))
                );
            }
        }
    }

    for l in layout.tick_labels.iter().chain(&layout.category_labels) {
        label(&mut out, l, dy, palette.muted);
    }
    for l in &layout.value_labels {
        label(&mut out, l, dy, palette.strong);
    }

    // Legend.
    let top = dy + layout.height + 8.0;
    for (x, row, ix) in placed {
        let y = top + row as f32 * LEGEND_ROW;
        let x = x + super::layout::PAD;
        let _ = write!(
            out,
            "<rect class=\"swatch\" x=\"{x}\" y=\"{}\" width=\"{SWATCH}\" height=\"{SWATCH}\" rx=\"2\" fill=\"{}\"/>",
            y + (LEGEND_ROW - SWATCH) / 2.0,
            rgb(palette.series(ix))
        );
        text(
            &mut out,
            x + SWATCH + LEGEND_GAP,
            y + LEGEND_ROW / 2.0 + LABEL_SIZE * BASELINE_FROM_CENTER,
            "start",
            (LABEL_SIZE, false),
            palette.muted,
            &names[ix],
        );
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

/// The chart as PNG bytes, `width` logical px wide at `scale` device pixels
/// per px, in the light palette.
///
/// # Errors
///
/// Why the chart cannot be pictured: a pie, an invalid or empty spec, or a
/// failure of the rasterizer.
pub fn to_png(spec: &ChartSpec, width: f32, scale: f32) -> Result<Vec<u8>, String> {
    if spec.kind == ChartKind::Pie {
        return Err("pie charts are not pictured".to_owned());
    }
    spec.validate().map_err(|err| err.to_string())?;
    if spec.labels.is_empty() {
        return Err("no data to plot".to_owned());
    }
    let svg = to_svg(spec, width, Palette::light());
    let mut options = usvg::Options {
        fontdb: Arc::clone(fontdb()),
        ..usvg::Options::default()
    };
    options.font_family = "Helvetica".to_owned();
    let tree = usvg::Tree::from_str(&svg, &options)
        .map_err(|err| format!("cannot draw the chart: {err}"))?;
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let size = tree.size();
    let px_w = ((size.width() * scale).round() as u32).clamp(1, 16_384);
    let px_h = ((size.height() * scale).round() as u32).clamp(1, 16_384);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px_w, px_h)
        .ok_or_else(|| "the chart is too large to draw".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            px_w as f32 / size.width(),
            px_h as f32 / size.height(),
        ),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::Series;

    fn spec(kind: ChartKind, series: usize) -> ChartSpec {
        ChartSpec {
            title: "Files <by> & \"type\"".into(),
            kind,
            labels: ["rs", "t<oml", "md", "lua"].map(String::from).to_vec(),
            series: (0..series)
                .map(|s| Series {
                    name: format!("s{s} & co"),
                    values: vec![4.0 + s as f64, -1.0, 2.5, 0.5],
                })
                .collect(),
        }
    }

    fn count(svg: &str, needle: &str) -> usize {
        svg.matches(needle).count()
    }

    #[test]
    fn a_bar_chart_has_one_shape_per_bar_and_escaped_text() {
        let svg = to_svg(&spec(ChartKind::Bar, 1), 720.0, Palette::light());
        assert!(svg.starts_with("<svg"));
        assert_eq!(count(&svg, "class=\"bar\""), 4);
        assert_eq!(count(&svg, "class=\"swatch\""), 0, "one series: no legend");
        assert!(svg.contains("Files &lt;by&gt; &amp; &quot;type&quot;"));
        assert!(svg.contains("t&lt;oml"));
        assert!(!svg.contains("<script"));
        assert!(!svg.contains("t<oml"));
    }

    #[test]
    fn several_series_get_a_legend_and_lines_get_marks() {
        let svg = to_svg(&spec(ChartKind::Bar, 2), 720.0, Palette::light());
        assert_eq!(count(&svg, "class=\"swatch\""), 2);
        assert!(svg.contains("s1 &amp; co"));
        let area = to_svg(&spec(ChartKind::Area, 2), 720.0, Palette::light());
        assert_eq!(count(&area, "class=\"line\""), 2);
        assert_eq!(count(&area, "class=\"area\""), 2);
        assert_eq!(count(&area, "class=\"marker\""), 8);
        let line = to_svg(&spec(ChartKind::Line, 1), 720.0, Palette::light());
        assert_eq!(count(&line, "class=\"area\""), 0);
        assert!(count(&line, "class=\"grid\"") >= 3);
    }

    #[test]
    fn the_png_has_the_expected_size_and_is_not_blank() {
        let png = to_png(&spec(ChartKind::Bar, 2), 720.0, 2.0).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let svg = to_svg(&spec(ChartKind::Bar, 2), 720.0, Palette::light());
        let height: f32 = svg
            .split("height=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse().ok())
            .unwrap();
        assert_eq!(img.width(), 1440);
        assert_eq!(img.height(), (height * 2.0).round() as u32);
        let first = img.pixels().next().unwrap();
        assert!(img.pixels().any(|p| p != first), "the picture is blank");
    }

    #[test]
    fn pies_and_invalid_charts_are_not_pictured() {
        assert!(to_png(&spec(ChartKind::Pie, 1), 720.0, 2.0).is_err());
        let mut bad = spec(ChartKind::Bar, 1);
        bad.series.clear();
        assert!(to_png(&bad, 720.0, 2.0).is_err());
    }

    /// Writes sample pictures to look at by hand.
    #[test]
    #[ignore = "writes sample files; set CHART_SAMPLE_DIR"]
    fn write_sample_pictures() {
        let Ok(dir) = std::env::var("CHART_SAMPLE_DIR") else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        for (name, kind, series) in [
            ("chart-bar.png", ChartKind::Bar, 2),
            ("chart-line.png", ChartKind::Line, 2),
        ] {
            let png = to_png(&spec(kind, series), 720.0, 2.0).unwrap();
            std::fs::write(format!("{dir}/{name}"), png).unwrap();
        }
    }
}
