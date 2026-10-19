//! Painting a [`Diagram`] with GPUI under a pan and zoom [`Camera`].

use gpui::{
    App, Bounds, ContentMask, FontWeight, Hsla, PathBuilder, Pixels, SharedString, TextAlign,
    TextRun, Window, font, point, px,
};

use super::diagram::{Anchor, Diagram, Label, Pt, Segment, Shape};

/// Zoom limits, relative to the fitted size.
pub const MIN_ZOOM: f32 = 0.2;
/// See [`MIN_ZOOM`].
pub const MAX_ZOOM: f32 = 8.0;
/// Largest fitted scale: a small diagram grows up to this to fill the width.
pub const MAX_FIT: f32 = 1.5;
/// Labels smaller than this on screen are skipped; they would be noise.
const MIN_FONT_PX: f32 = 2.0;

/// How the diagram sits in its viewport: `zoom` 1 fits it to the width (up
/// to [`MAX_FIT`]), centered, or aligned to the top when it is taller than
/// the viewport; `pan` moves it, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Zoom over the fitted scale.
    pub zoom: f32,
    /// Offset from the centered position, in pixels.
    pub pan: Pt,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
        }
    }
}

impl Camera {
    /// Scale that fits the width of `diagram` (with its padding) in
    /// `size`, at most [`MAX_FIT`]. A taller diagram is panned, not shrunk.
    pub fn fit(diagram: &Diagram, size: Pt) -> f32 {
        let width = diagram.width + diagram.padding * 2.0;
        (size.0 / width).clamp(0.01, MAX_FIT)
    }

    /// Diagram units to pixels.
    pub fn scale(&self, diagram: &Diagram, size: Pt) -> f32 {
        Self::fit(diagram, size) * self.zoom
    }

    /// Where the diagram's origin lands, relative to the viewport origin.
    pub fn origin(&self, diagram: &Diagram, size: Pt) -> Pt {
        let base = Self::unpanned_origin(diagram, size, self.scale(diagram, size));
        (base.0 + self.pan.0, base.1 + self.pan.1)
    }

    /// The origin at `scale` without pan: centered, or with the top padding
    /// at the top of the viewport when the diagram is taller than it.
    fn unpanned_origin(diagram: &Diagram, size: Pt, scale: f32) -> Pt {
        let x = size.0 / 2.0 - diagram.width / 2.0 * scale;
        let y = if (diagram.height + diagram.padding * 2.0) * scale > size.1 {
            diagram.padding * scale
        } else {
            size.1 / 2.0 - diagram.height / 2.0 * scale
        };
        (x, y)
    }

    /// Moves the diagram by `delta` pixels.
    pub fn pan_by(&mut self, delta: Pt) {
        self.pan = (self.pan.0 + delta.0, self.pan.1 + delta.1);
    }

    /// Multiplies the zoom by `factor` (clamped to the limits), keeping the
    /// diagram point under `anchor` (relative to the viewport origin) in
    /// place.
    pub fn zoom_at(&mut self, factor: f32, anchor: Pt, diagram: &Diagram, size: Pt) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let before = self.scale(diagram, size);
        let origin = self.origin(diagram, size);
        let under = (
            (anchor.0 - origin.0) / before,
            (anchor.1 - origin.1) / before,
        );
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.scale(diagram, size);
        let base = Self::unpanned_origin(diagram, size, after);
        self.pan = (
            anchor.0 - base.0 - under.0 * after,
            anchor.1 - base.1 - under.1 * after,
        );
    }
}

fn finite(segments: &[Segment]) -> bool {
    let ok = |p: &Pt| p.0.is_finite() && p.1.is_finite();
    segments.iter().all(|segment| match segment {
        Segment::Move(p) | Segment::Line(p) => ok(p),
        Segment::Quad(c, p) => ok(c) && ok(p),
        Segment::Cubic(a, b, p) => ok(a) && ok(b) && ok(p),
        Segment::Arc { radii, to, .. } => ok(radii) && ok(to),
        Segment::Close => true,
    })
}

fn build(
    mut builder: PathBuilder,
    segments: &[Segment],
    map: &impl Fn(Pt) -> gpui::Point<Pixels>,
    scale: f32,
) -> Option<gpui::Path<Pixels>> {
    for segment in segments {
        match *segment {
            Segment::Move(p) => builder.move_to(map(p)),
            Segment::Line(p) => builder.line_to(map(p)),
            Segment::Quad(c, p) => builder.curve_to(map(p), map(c)),
            Segment::Cubic(a, b, p) => builder.cubic_bezier_to(map(p), map(a), map(b)),
            Segment::Arc {
                radii,
                rotation,
                large,
                sweep,
                to,
            } => builder.arc_to(
                point(px(radii.0 * scale), px(radii.1 * scale)),
                px(rotation),
                large,
                sweep,
                map(to),
            ),
            Segment::Close => builder.close(),
        }
    }
    builder.build().ok()
}

fn paint_label(
    label: &Label,
    family: &SharedString,
    map: &impl Fn(Pt) -> gpui::Point<Pixels>,
    scale: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let size = label.size * scale;
    if !size.is_finite() || size < MIN_FONT_PX {
        return;
    }
    let line_height = label.size * 1.2;
    let first = label.at.1 - line_height * (label.lines.len().saturating_sub(1)) as f32 / 2.0;
    let color: Hsla = label.color.into();
    let mut run_font = font(family.clone());
    if label.bold {
        run_font.weight = FontWeight::BOLD;
    }
    for (ix, line) in label.lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let run = TextRun {
            len: line.len(),
            font: run_font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(line.clone()),
            px(size),
            &[run],
            None,
        );
        let center = map((label.at.0, first + line_height * ix as f32));
        let x = match label.anchor {
            Anchor::Start => center.x,
            Anchor::Middle => center.x - shaped.width / 2.0,
            Anchor::End => center.x - shaped.width,
        };
        let height = px(size * 1.2);
        let origin = point(x, center.y - height / 2.0);
        // A failed glyph raster only loses that line.
        shaped
            .paint(origin, height, TextAlign::Left, None, window, cx)
            .ok();
    }
}

/// Paints `diagram` in `bounds` (clipped to them) under `camera`, with
/// labels in `family`.
pub fn paint(
    diagram: &Diagram,
    camera: Camera,
    bounds: Bounds<Pixels>,
    family: &SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return;
    }
    let scale = camera.scale(diagram, size);
    let origin = camera.origin(diagram, size);
    let base = bounds.origin;
    let map = move |p: Pt| {
        point(
            base.x + px(origin.0 + p.0 * scale),
            base.y + px(origin.1 + p.1 * scale),
        )
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        window.paint_quad(gpui::fill(bounds, Hsla::from(diagram.background)));
        for shape in &diagram.shapes {
            match shape {
                Shape::Path {
                    segments,
                    fill,
                    stroke,
                } => {
                    if !finite(segments) {
                        continue;
                    }
                    if let Some(fill) = fill
                        && let Some(path) = build(PathBuilder::fill(), segments, &map, scale)
                    {
                        window.paint_path(path, Hsla::from(*fill));
                    }
                    if let Some(stroke) = stroke {
                        let width = (stroke.width * scale).max(0.5);
                        let mut builder = PathBuilder::stroke(px(width));
                        if let Some(dash) = &stroke.dash {
                            let dash: Vec<Pixels> =
                                dash.iter().map(|d| px((d * scale).max(0.5))).collect();
                            builder = builder.dash_array(&dash);
                        }
                        if let Some(path) = build(builder, segments, &map, scale) {
                            window.paint_path(path, Hsla::from(stroke.color));
                        }
                    }
                }
                Shape::Text(label) => paint_label(label, family, &map, scale, window, cx),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Rgba;

    fn diagram() -> Diagram {
        Diagram {
            width: 200.0,
            height: 100.0,
            padding: 0.0,
            background: Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            shapes: Vec::new(),
        }
    }

    #[test]
    fn fits_the_width_up_to_max_fit_and_centers() {
        let d = diagram();
        let camera = Camera::default();
        assert_eq!(Camera::fit(&d, (100.0, 100.0)), 0.5);
        assert_eq!(
            Camera::fit(&d, (250.0, 10.0)),
            1.25,
            "the height does not shrink it"
        );
        assert_eq!(Camera::fit(&d, (1000.0, 1000.0)), MAX_FIT);
        assert_eq!(camera.origin(&d, (200.0, 200.0)), (0.0, 50.0));
    }

    #[test]
    fn a_tall_diagram_starts_at_the_top() {
        let d = Diagram {
            height: 1000.0,
            padding: 10.0,
            ..diagram()
        };
        let camera = Camera::default();
        // Fit 1.0: 1020 px high in a 300 px viewport.
        assert_eq!(camera.origin(&d, (220.0, 300.0)), (10.0, 10.0));
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let tall = Diagram {
            height: 1000.0,
            ..diagram()
        };
        for d in [diagram(), tall] {
            let size = (400.0, 200.0);
            let mut camera = Camera::default();
            let anchor = (150.0, 60.0);
            let to_diagram = |c: &Camera| {
                let (o, s) = (c.origin(&d, size), c.scale(&d, size));
                ((anchor.0 - o.0) / s, (anchor.1 - o.1) / s)
            };
            let before = to_diagram(&camera);
            camera.zoom_at(2.0, anchor, &d, size);
            assert_eq!(camera.zoom, 2.0);
            let after = to_diagram(&camera);
            assert!(
                (before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3,
                "{before:?} vs {after:?}"
            );
        }
    }

    #[test]
    fn zoom_is_clamped_and_ignores_bad_factors() {
        let d = diagram();
        let mut camera = Camera::default();
        camera.zoom_at(1000.0, (0.0, 0.0), &d, (100.0, 100.0));
        assert_eq!(camera.zoom, MAX_ZOOM);
        camera.zoom_at(1e-6, (0.0, 0.0), &d, (100.0, 100.0));
        assert_eq!(camera.zoom, MIN_ZOOM);
        camera.zoom_at(f32::NAN, (0.0, 0.0), &d, (100.0, 100.0));
        camera.zoom_at(-1.0, (0.0, 0.0), &d, (100.0, 100.0));
        assert_eq!(camera.zoom, MIN_ZOOM);
        camera.pan_by((5.0, -3.0));
        assert!(camera.pan.0.is_finite());
    }
}
