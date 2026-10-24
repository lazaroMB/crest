//! Mermaid source to a flat list of shapes ready to paint.
//!
//! rusty-mermaid parses and lays the diagram out into a `Scene` of
//! primitives (rects, paths with markers, text, nested groups). This module
//! flattens it once into [`Diagram`]: every primitive becomes a path or a
//! label in diagram coordinates, with group transforms, markers and style
//! defaults already resolved, so painting a frame is only a coordinate
//! mapping. Colors come from the color theme ([`crest_theme`]): the
//! diagram is always painted on the ink background. Parsing never panics: a panic inside the parser is caught and
//! reported like a parse error, and a diagram with nothing to draw is an
//! error too.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::wrap;
use domain::theme::{Appearance, Theme as ColorTheme};
use gpui::Rgba;
use rusty_mermaid_core::{
    Color, FontWeight, MarkerPath, MarkerType, PathSegment, Point, Primitive, Style, TextAnchor,
    TextStyle, Theme, Transform, marker_path, path_end_tangent, path_start_tangent,
};

/// A point in diagram coordinates.
pub type Pt = (f32, f32);

/// One path command, in diagram coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Start a sub-path.
    Move(Pt),
    /// Straight line.
    Line(Pt),
    /// Quadratic curve: control, end.
    Quad(Pt, Pt),
    /// Cubic curve: two controls, end.
    Cubic(Pt, Pt, Pt),
    /// Elliptical arc as in SVG.
    Arc {
        /// Radii.
        radii: Pt,
        /// Rotation of the x axis, in degrees.
        rotation: f32,
        /// SVG large-arc flag.
        large: bool,
        /// SVG sweep flag.
        sweep: bool,
        /// End point.
        to: Pt,
    },
    /// Close the sub-path.
    Close,
}

/// A stroke style.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// Color.
    pub color: Rgba,
    /// Width in diagram units.
    pub width: f32,
    /// Dash pattern in diagram units.
    pub dash: Option<Vec<f32>>,
}

/// Horizontal anchor of a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The position is the left edge.
    Start,
    /// The position is the center.
    Middle,
    /// The position is the right edge.
    End,
}

/// A text label, vertically centered on its position.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    /// Anchor point.
    pub at: Pt,
    /// Lines of text, top to bottom.
    pub lines: Vec<String>,
    /// Horizontal anchor.
    pub anchor: Anchor,
    /// Font size in diagram units.
    pub size: f32,
    /// Color.
    pub color: Rgba,
    /// Bold weight.
    pub bold: bool,
}

/// Something to paint.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A filled and/or stroked path.
    Path {
        /// Commands.
        segments: Vec<Segment>,
        /// Fill color.
        fill: Option<Rgba>,
        /// Outline.
        stroke: Option<Stroke>,
    },
    /// Text.
    Text(Label),
}

/// A laid-out diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    /// Width of the content, in diagram units.
    pub width: f32,
    /// Height of the content.
    pub height: f32,
    /// Margin to keep around the content.
    pub padding: f32,
    /// Background the colors were chosen for.
    pub background: Rgba,
    /// Shapes in paint order.
    pub shapes: Vec<Shape>,
}

/// The Mermaid theme of a color theme: nodes on the muted surface with an
/// accent edge, notes edged in the warning color.
pub fn crest_theme(colors: &ColorTheme) -> Theme {
    let c = |path: &str| color(colors.color(path));
    let base = match colors.appearance {
        Appearance::Dark => Theme::dark(),
        Appearance::Light => Theme::default(),
    };
    Theme {
        node_fill: c("muted.bg"),
        node_stroke: c("accent.bg"),
        node_text: c("foreground"),
        edge_stroke: c("muted.fg"),
        edge_label_text: c("button.fg"),
        edge_label_bg: c("canvas.bg"),
        start_fill: c("accent.bg"),
        end_inner_fill: c("accent.bg"),
        composite_fill: c("popover.bg"),
        composite_stroke: c("input.border"),
        composite_label: c("button.fg"),
        note_fill: c("popover.bg"),
        note_stroke: c("warning.bg"),
        note_text: c("foreground"),
        subgraph_fill: c("popover.bg"),
        subgraph_stroke: c("input.border"),
        subgraph_label: c("button.fg"),
        divider_stroke: c("input.border"),
        region_stroke: c("input.border"),
        lifeline_stroke: c("muted.fg"),
        activation_fill: c("muted.bg"),
        activation_stroke: c("accent.bg"),
        grid_stroke: c("border"),
        muted_text: c("muted.fg"),
        face_fill: c("warning.bg"),
        detail_stroke: c("canvas.bg"),
        background: c("canvas.bg"),
        ..base
    }
}

/// A theme color as a Mermaid color.
fn color(color: domain::theme::Color) -> Color {
    Color::rgba(color.r, color.g, color.b, color.a)
}

/// Parses and lays out `source` in the built-in dark theme.
///
/// # Errors
///
/// As [`parse_with`].
pub fn parse(source: &str) -> Result<Diagram, String> {
    parse_with(source, &crate::theme::default_theme())
}

/// Parses and lays out `source` with the [`crest_theme`] of `colors`.
///
/// # Errors
///
/// The parser's message when the source is not a diagram rusty-mermaid
/// understands, when it panics, or when the result is empty.
pub fn parse_with(source: &str, colors: &ColorTheme) -> Result<Diagram, String> {
    let theme = crest_theme(colors);
    let source = wrap::wrap_labels(source, wrap::LABEL_COLUMNS);
    let scene = catch_unwind(AssertUnwindSafe(|| rusty_mermaid::render(&source, &theme)))
        .map_err(|_| "the Mermaid parser crashed on this input".to_owned())?
        .map_err(|err| format!("cannot parse the diagram: {err}"))?;
    let mut flattener = Flattener {
        theme: &theme,
        shapes: Vec::new(),
    };
    for element in scene.elements() {
        flattener.primitive(&element.primitive, Affine::IDENTITY);
    }
    if flattener.shapes.is_empty() {
        return Err("the diagram has nothing to draw".to_owned());
    }
    let (width, height) = (scene.width as f32, scene.height as f32);
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err("the diagram has no size".to_owned());
    }
    Ok(Diagram {
        width,
        height,
        padding: theme.padding as f32,
        background: rgba(theme.background, 1.0),
        shapes: flattener.shapes,
    })
}

/// Label text as rusty-mermaid measured it: `<br>` (any case and form)
/// is a line break and other HTML tags (`<b>`, `</i>`...) are dropped. A
/// `<` that does not start a tag stays.
fn strip_tags(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let is_tag = tail[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
        match tail.find('>').filter(|_| is_tag) {
            Some(end) => {
                if wrap::find_br(&tail[..=end]) == Some((0, end + 1)) {
                    out.push('\n');
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Source text safe to show: control characters other than newlines and
/// tabs (binary garbage) become U+FFFD.
pub fn printable(source: &str) -> String {
    source
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

fn rgba(color: Color, opacity: f64) -> Rgba {
    Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: f32::from(color.a) / 255.0 * opacity.clamp(0.0, 1.0) as f32,
    }
}

/// 2D affine transform: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Affine {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn from_transform(transform: &Transform) -> Self {
        match *transform {
            Transform::Identity => Self::IDENTITY,
            Transform::Translate(x, y) => Self {
                e: x,
                f: y,
                ..Self::IDENTITY
            },
            Transform::Scale(x, y) => Self {
                a: x,
                d: y,
                ..Self::IDENTITY
            },
            Transform::Rotate { degrees, cx, cy } => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                Self {
                    a: cos,
                    b: sin,
                    c: -sin,
                    d: cos,
                    e: cx - cos * cx + sin * cy,
                    f: cy - sin * cx - cos * cy,
                }
            }
        }
    }

    /// `self` applied after `inner`.
    fn then(self, inner: Self) -> Self {
        Self {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            e: self.a * inner.e + self.c * inner.f + self.e,
            f: self.b * inner.e + self.d * inner.f + self.f,
        }
    }

    fn point(&self, p: Point) -> Pt {
        (
            (self.a * p.x + self.c * p.y + self.e) as f32,
            (self.b * p.x + self.d * p.y + self.f) as f32,
        )
    }

    /// Uniform scale factor, for widths, radii and font sizes.
    fn scale(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }

    /// Rotation of the x axis, in degrees.
    fn rotation(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }
}

struct Flattener<'a> {
    theme: &'a Theme,
    shapes: Vec<Shape>,
}

impl Flattener<'_> {
    fn fill(&self, style: &Style) -> Option<Rgba> {
        let opacity = style.opacity.unwrap_or(1.0);
        style.fill.map(|c| rgba(c, opacity))
    }

    fn stroke(&self, style: &Style, at: &Affine, default: bool) -> Option<Stroke> {
        let (color, width) = match style.resolve_stroke_opt(self.theme) {
            Some(stroke) => stroke,
            None if default => (self.theme.edge_stroke, self.theme.default_stroke_width),
            None => return None,
        };
        let scale = at.scale();
        Some(Stroke {
            color: rgba(color, style.opacity.unwrap_or(1.0)),
            width: (width * scale) as f32,
            dash: style
                .stroke_dasharray
                .as_ref()
                .map(|dash| dash.iter().map(|d| (d * scale) as f32).collect()),
        })
    }

    fn push_path(&mut self, segments: Vec<Segment>, fill: Option<Rgba>, stroke: Option<Stroke>) {
        if !segments.is_empty() && (fill.is_some() || stroke.is_some()) {
            self.shapes.push(Shape::Path {
                segments,
                fill,
                stroke,
            });
        }
    }

    fn arc(
        at: &Affine,
        rx: f64,
        ry: f64,
        rotation: f64,
        large: bool,
        sweep: bool,
        to: Point,
    ) -> Segment {
        let scale = at.scale();
        Segment::Arc {
            radii: ((rx * scale) as f32, (ry * scale) as f32),
            rotation: (rotation + at.rotation()) as f32,
            large,
            sweep,
            to: at.point(to),
        }
    }

    fn ellipse(at: &Affine, center: Point, rx: f64, ry: f64) -> Vec<Segment> {
        let right = Point::new(center.x + rx, center.y);
        let left = Point::new(center.x - rx, center.y);
        vec![
            Segment::Move(at.point(right)),
            Self::arc(at, rx, ry, 0.0, false, true, left),
            Self::arc(at, rx, ry, 0.0, false, true, right),
            Segment::Close,
        ]
    }

    fn rect(at: &Affine, x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> Vec<Segment> {
        let p = |x, y| at.point(Point::new(x, y));
        let rx = rx.clamp(0.0, w / 2.0);
        let ry = if ry > 0.0 { ry } else { rx }.clamp(0.0, h / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return vec![
                Segment::Move(p(x, y)),
                Segment::Line(p(x + w, y)),
                Segment::Line(p(x + w, y + h)),
                Segment::Line(p(x, y + h)),
                Segment::Close,
            ];
        }
        let corner = |to: Point| Self::arc(at, rx, ry, 0.0, false, true, to);
        vec![
            Segment::Move(p(x + rx, y)),
            Segment::Line(p(x + w - rx, y)),
            corner(Point::new(x + w, y + ry)),
            Segment::Line(p(x + w, y + h - ry)),
            corner(Point::new(x + w - rx, y + h)),
            Segment::Line(p(x + rx, y + h)),
            corner(Point::new(x, y + h - ry)),
            Segment::Line(p(x, y + ry)),
            corner(Point::new(x + rx, y)),
            Segment::Close,
        ]
    }

    fn polygon(at: &Affine, points: &[Point], closed: bool) -> Vec<Segment> {
        let mut segments: Vec<Segment> = points
            .iter()
            .enumerate()
            .map(|(ix, &p)| {
                if ix == 0 {
                    Segment::Move(at.point(p))
                } else {
                    Segment::Line(at.point(p))
                }
            })
            .collect();
        if closed && !segments.is_empty() {
            segments.push(Segment::Close);
        }
        segments
    }

    fn segments(at: &Affine, segments: &[PathSegment]) -> Vec<Segment> {
        segments
            .iter()
            .map(|segment| match *segment {
                PathSegment::MoveTo(p) => Segment::Move(at.point(p)),
                PathSegment::LineTo(p) => Segment::Line(at.point(p)),
                PathSegment::QuadTo { cp, to } => Segment::Quad(at.point(cp), at.point(to)),
                PathSegment::CubicTo { cp1, cp2, to } => {
                    Segment::Cubic(at.point(cp1), at.point(cp2), at.point(to))
                }
                PathSegment::ArcTo {
                    rx,
                    ry,
                    rotation,
                    large_arc,
                    sweep,
                    to,
                } => Self::arc(at, rx, ry, rotation, large_arc, sweep, to),
                PathSegment::Close => Segment::Close,
            })
            .collect()
    }

    fn marker(&mut self, marker: MarkerType, tip: Point, angle: f64, stroke: &Stroke, at: &Affine) {
        let color = stroke.color;
        let width = f64::from(stroke.width) / at.scale().max(f64::EPSILON);
        let line = |w: f64| Stroke {
            color,
            width: (w * at.scale()) as f32,
            dash: None,
        };
        match marker_path(marker, tip, angle, width) {
            MarkerPath::FillPolygon { points } => {
                self.push_path(Self::polygon(at, &points, true), Some(color), None);
            }
            MarkerPath::StrokePolyline {
                points,
                width,
                closed,
            } => self.push_path(Self::polygon(at, &points, closed), None, Some(line(width))),
            MarkerPath::FillAndStrokePolygon {
                points,
                stroke_width,
                fill_is_marker_color,
            } => {
                let fill = if fill_is_marker_color {
                    color
                } else {
                    rgba(self.theme.background, 1.0)
                };
                self.push_path(
                    Self::polygon(at, &points, true),
                    Some(fill),
                    Some(line(stroke_width)),
                );
            }
            MarkerPath::FillCircle { center, radius } => {
                self.push_path(Self::ellipse(at, center, radius, radius), Some(color), None);
            }
            MarkerPath::StrokeCurves { curves, width } => {
                for [from, ctrl, to] in curves {
                    let segments = vec![
                        Segment::Move(at.point(from)),
                        Segment::Quad(at.point(ctrl), at.point(to)),
                    ];
                    self.push_path(segments, None, Some(line(width)));
                }
            }
        }
    }

    fn text(
        &mut self,
        at: &Affine,
        position: Point,
        content: &str,
        anchor: TextAnchor,
        style: &TextStyle,
    ) {
        let content = strip_tags(content);
        let lines: Vec<String> = content.lines().map(printable).collect();
        if lines.iter().all(|line| line.trim().is_empty()) {
            return;
        }
        self.shapes.push(Shape::Text(Label {
            at: at.point(position),
            lines,
            anchor: match anchor {
                TextAnchor::Start => Anchor::Start,
                TextAnchor::Middle => Anchor::Middle,
                TextAnchor::End => Anchor::End,
            },
            size: (style.font_size * at.scale()) as f32,
            color: rgba(style.fill.unwrap_or(self.theme.node_text), 1.0),
            bold: style.font_weight == FontWeight::Bold,
        }));
    }

    fn primitive(&mut self, primitive: &Primitive, at: Affine) {
        match primitive {
            Primitive::Rect {
                bbox,
                rx,
                ry,
                style,
            } => {
                let x = bbox.x - bbox.width / 2.0;
                let y = bbox.y - bbox.height / 2.0;
                let segments = Self::rect(&at, x, y, bbox.width, bbox.height, *rx, *ry);
                self.push_path(segments, self.fill(style), self.stroke(style, &at, false));
            }
            Primitive::Circle {
                center,
                radius,
                style,
            } => {
                let segments = Self::ellipse(&at, *center, *radius, *radius);
                self.push_path(segments, self.fill(style), self.stroke(style, &at, false));
            }
            Primitive::Ellipse {
                center,
                rx,
                ry,
                style,
            } => {
                let segments = Self::ellipse(&at, *center, *rx, *ry);
                self.push_path(segments, self.fill(style), self.stroke(style, &at, false));
            }
            Primitive::Polygon { points, style } => {
                let segments = Self::polygon(&at, points, true);
                self.push_path(segments, self.fill(style), self.stroke(style, &at, false));
            }
            Primitive::Path {
                segments,
                style,
                marker_start,
                marker_end,
            } => {
                let stroke = self.stroke(style, &at, true);
                self.push_path(
                    Self::segments(&at, segments),
                    self.fill(style),
                    stroke.clone(),
                );
                let Some(stroke) = stroke else { return };
                if let (Some(marker), Some((tip, angle))) =
                    (marker_start, path_start_tangent(segments))
                {
                    self.marker(*marker, tip, angle, &stroke, &at);
                }
                if let (Some(marker), Some((tip, angle))) = (marker_end, path_end_tangent(segments))
                {
                    self.marker(*marker, tip, angle, &stroke, &at);
                }
            }
            Primitive::Text {
                position,
                content,
                anchor,
                style,
            } => self.text(&at, *position, content, *anchor, style),
            Primitive::Group {
                transform,
                children,
            } => {
                let inner = at.then(Affine::from_transform(transform));
                for child in children {
                    self.primitive(child, inner);
                }
            }
            Primitive::Arc {
                center,
                inner_r,
                outer_r,
                start_angle,
                end_angle,
                style,
            } => {
                let on = |r: f64, angle: f64| {
                    Point::new(center.x + r * angle.cos(), center.y + r * angle.sin())
                };
                let large = (end_angle - start_angle).abs() > std::f64::consts::PI;
                let mut segments = vec![
                    Segment::Move(at.point(on(*outer_r, *start_angle))),
                    Self::arc(
                        &at,
                        *outer_r,
                        *outer_r,
                        0.0,
                        large,
                        true,
                        on(*outer_r, *end_angle),
                    ),
                ];
                if *inner_r > 0.0 {
                    segments.push(Segment::Line(at.point(on(*inner_r, *end_angle))));
                    segments.push(Self::arc(
                        &at,
                        *inner_r,
                        *inner_r,
                        0.0,
                        large,
                        false,
                        on(*inner_r, *start_angle),
                    ));
                    segments.push(Segment::Close);
                }
                self.push_path(segments, self.fill(style), self.stroke(style, &at, true));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOWCHART: &str =
        "flowchart LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]\n  B -.->|no| D((Retry))\n";
    const SEQUENCE: &str =
        "sequenceDiagram\n  Alice->>Bob: Hi\n  Bob-->>Alice: Hello\n  Note over Alice,Bob: chat\n";

    fn labels(diagram: &Diagram) -> Vec<String> {
        diagram
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Text(label) => Some(label.lines.join(" ")),
                Shape::Path { .. } => None,
            })
            .collect()
    }

    #[test]
    fn flowchart_has_nodes_edges_and_arrows() {
        let diagram = parse(FLOWCHART).unwrap_or_else(|err| panic!("{err}"));
        let text = labels(&diagram);
        for label in ["Start", "Ok?", "Done", "Retry", "yes", "no"] {
            assert!(
                text.iter().any(|t| t == label),
                "{label} missing in {text:?}"
            );
        }
        let dashed = diagram.shapes.iter().any(|shape| {
            matches!(
                shape,
                Shape::Path {
                    stroke: Some(Stroke { dash: Some(_), .. }),
                    ..
                }
            )
        });
        assert!(dashed, "the -.-> edge is dashed");
        let arrows = diagram
            .shapes
            .iter()
            .filter(|shape| {
                matches!(
                    shape,
                    Shape::Path {
                        fill: Some(_),
                        stroke: None,
                        ..
                    }
                )
            })
            .count();
        assert!(arrows >= 3, "one filled arrowhead per edge, got {arrows}");
        assert!(diagram.width > 0.0 && diagram.height > 0.0);
    }

    #[test]
    fn the_diagram_uses_the_theme_colors() {
        let theme = crate::theme::default_theme();
        let diagram = parse(FLOWCHART).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(
            diagram.background,
            rgba(color(theme.color("canvas.bg")), 1.0)
        );
        let coral = rgba(color(theme.color("accent.bg")), 1.0);
        let coral_edge = diagram.shapes.iter().any(|shape| {
            matches!(shape, Shape::Path { stroke: Some(Stroke { color, .. }), .. } if *color == coral)
        });
        assert!(coral_edge, "nodes are outlined in the accent");
    }

    #[test]
    fn sequence_diagram_parses() {
        let diagram = parse(SEQUENCE).unwrap_or_else(|err| panic!("{err}"));
        let text = labels(&diagram);
        for label in ["Alice", "Bob", "Hi", "Hello", "chat"] {
            assert!(
                text.iter().any(|t| t == label),
                "{label} missing in {text:?}"
            );
        }
    }

    #[test]
    fn invalid_sources_are_errors_not_panics() {
        let garbage = String::from_utf8_lossy(&[0, 159, 146, 150, 255, 1, 2, 3, 0xfe]).into_owned();
        let random: String = (0..2000u32)
            .map(|i| char::from_u32(i.wrapping_mul(2_654_435_761) % 0x2FFF).unwrap_or('?'))
            .collect();
        let flow_garbage = format!("flowchart TD\n{garbage}\n{random}");
        let seq_garbage = format!("sequenceDiagram\n{random}");
        let inputs = [
            flow_garbage.as_str(),
            seq_garbage.as_str(),
            "",
            "   \n\t\n",
            "not a diagram",
            "flowchart LR\n A --> \n [[[",
            "sequenceDiagram\n ->> ->>",
            "pie\n \"a\": x",
            "graph TD\n A[\"unclosed\n",
            "classDiagram\n class A {",
            "%% only a comment",
            garbage.as_str(),
            random.as_str(),
        ];
        for input in inputs {
            if let Err(err) = parse(input) {
                assert!(!err.is_empty(), "error for {input:?} has a message");
            }
        }
        for input in ["", "   \n", "not a diagram", garbage.as_str()] {
            assert!(parse(input).is_err(), "{input:?} should not parse");
        }
    }

    #[test]
    fn long_labels_wrap_into_narrower_nodes() {
        let long =
            "Validate the user input, check every permission and log the result of the request";
        let source = format!("flowchart LR\n  A[{long}]\n");
        let diagram = parse(&source).unwrap_or_else(|err| panic!("{err}"));
        let label = diagram
            .shapes
            .iter()
            .find_map(|shape| match shape {
                Shape::Text(label) => Some(label.clone()),
                Shape::Path { .. } => None,
            })
            .unwrap_or_else(|| panic!("no label"));
        assert_eq!(label.lines.len(), 3, "{:?}", label.lines);
        assert_eq!(label.lines.join(" "), long);
        let theme = crest_theme(&crate::theme::default_theme());
        let unwrapped =
            rusty_mermaid::render(&source, &theme).unwrap_or_else(|err| panic!("{err}"));
        assert!(
            diagram.width < unwrapped.width as f32 / 1.5,
            "{} vs {}",
            diagram.width,
            unwrapped.width
        );
    }

    #[test]
    fn tags_are_stripped_and_breaks_kept_in_any_case() {
        assert_eq!(strip_tags("a<BR>b<br />c<b>d</b>"), "a\nb\ncd");
        assert_eq!(strip_tags("x < y <3"), "x < y <3");
        assert_eq!(strip_tags("unclosed <b"), "unclosed <b");
    }

    #[test]
    fn printable_replaces_control_characters() {
        assert_eq!(printable("a\u{0}b\n\tc\u{1b}"), "a\u{FFFD}b\n\tc\u{FFFD}");
    }

    #[test]
    fn group_transforms_compose() {
        let translate = Affine::from_transform(&Transform::Translate(10.0, 5.0));
        let scale = Affine::from_transform(&Transform::Scale(2.0, 2.0));
        let both = translate.then(scale);
        assert_eq!(both.point(Point::new(1.0, 1.0)), (12.0, 7.0));
        assert!((both.scale() - 2.0).abs() < 1e-9);
        let rotate = Affine::from_transform(&Transform::Rotate {
            degrees: 90.0,
            cx: 1.0,
            cy: 1.0,
        });
        let (x, y) = rotate.point(Point::new(2.0, 1.0));
        assert!(
            (x - 1.0).abs() < 1e-5 && (y - 2.0).abs() < 1e-5,
            "({x}, {y})"
        );
    }
}
