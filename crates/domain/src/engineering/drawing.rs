//! A 2D drawing read from a DXF file (`show_dxf`): its layers and the
//! entities the viewer draws, in drawing units, with block inserts already
//! expanded.
//!
//! Shapes are kept as the file has them where the viewer can draw them
//! exactly (lines, circles, arcs, polylines with bulges, text); ellipses
//! and splines come as polylines through points the reader computed or
//! the file gave, and say so in the warnings. [`flatten`] turns any shape
//! into the points the painter strokes; [`DrawingSpec::computed_extents`]
//! is the box of those points. Colors are AutoCAD Color Index numbers
//! ([`aci_rgb`]); the UI maps them onto the theme.

use std::f64::consts::TAU;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::cad::csv_field;
use super::invalid;
use crate::block::BlockError;

/// Most entities a drawing keeps (after expanding inserts).
pub const MAX_ENTITIES: usize = 200_000;
/// Most layers.
pub const MAX_LAYERS: usize = 10_000;
/// Segments of a full circle when flattened.
pub const CIRCLE_SEGMENTS: usize = 72;

/// A point in drawing units.
pub type P2 = [f64; 2];

/// A polyline vertex: its point and the bulge of the segment that starts
/// at it (tan of a quarter of the arc's angle; 0 for a straight segment,
/// positive counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PolyVertex {
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
    /// Bulge of the segment to the next vertex.
    #[serde(default)]
    pub bulge: f64,
}

/// What an entity draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// A segment.
    Line {
        /// Start.
        a: P2,
        /// End.
        b: P2,
    },
    /// A full circle.
    Circle {
        /// Centre.
        center: P2,
        /// Radius.
        radius: f64,
    },
    /// A counter-clockwise arc from `start` to `end` degrees.
    Arc {
        /// Centre.
        center: P2,
        /// Radius.
        radius: f64,
        /// Start angle in degrees.
        start: f64,
        /// End angle in degrees.
        end: f64,
    },
    /// A polyline (LWPOLYLINE, POLYLINE; ellipses and splines as
    /// approximations).
    Polyline {
        /// Vertices.
        vertices: Vec<PolyVertex>,
        /// Whether the last vertex joins the first.
        #[serde(default)]
        closed: bool,
    },
    /// A single-line or multi-line text.
    Text {
        /// Insertion point (lower left of the first line).
        at: P2,
        /// Character height in drawing units.
        height: f64,
        /// Rotation in degrees (the viewer draws text unrotated).
        #[serde(default)]
        rotation: f64,
        /// The text, formatting codes removed; lines split by `\n`.
        text: String,
    },
    /// A point marker.
    Point {
        /// Where.
        at: P2,
    },
}

impl Shape {
    /// The entity kind it came from, for counts (`line`, `arc`, …).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Line { .. } => "line",
            Self::Circle { .. } => "circle",
            Self::Arc { .. } => "arc",
            Self::Polyline { .. } => "polyline",
            Self::Text { .. } => "text",
            Self::Point { .. } => "point",
        }
    }

    fn coordinates(&self) -> Vec<f64> {
        match self {
            Self::Line { a, b } => vec![a[0], a[1], b[0], b[1]],
            Self::Circle { center, radius } => vec![center[0], center[1], *radius],
            Self::Arc {
                center,
                radius,
                start,
                end,
            } => vec![center[0], center[1], *radius, *start, *end],
            Self::Polyline { vertices, .. } => {
                vertices.iter().flat_map(|v| [v.x, v.y, v.bulge]).collect()
            }
            Self::Text {
                at,
                height,
                rotation,
                ..
            } => vec![at[0], at[1], *height, *rotation],
            Self::Point { at } => vec![at[0], at[1]],
        }
    }
}

/// An entity: a shape on a layer with its own color, or the layer's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DrawEntity {
    /// Index of its layer in [`DrawingSpec::layers`].
    pub layer: usize,
    /// ACI color (1–255); `None` takes the layer's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    /// What it draws.
    #[serde(flatten)]
    pub shape: Shape,
}

/// A layer of the drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DrawingLayer {
    /// Its name.
    pub name: String,
    /// ACI color (1–255).
    pub color: u8,
    /// Turned off in the file (shown off at first).
    #[serde(default)]
    pub off: bool,
    /// Entities on it.
    #[serde(default)]
    pub entities: usize,
}

impl DrawingLayer {
    /// Whether the file shows it.
    #[must_use]
    pub fn visible(&self) -> bool {
        !self.off
    }
}

/// A box in drawing units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Extents {
    /// Lower left.
    pub min: P2,
    /// Upper right.
    pub max: P2,
}

impl Extents {
    /// Width and height.
    #[must_use]
    pub fn size(&self) -> P2 {
        [self.max[0] - self.min[0], self.max[1] - self.min[1]]
    }

    fn valid(&self) -> bool {
        self.min.iter().chain(&self.max).all(|c| c.is_finite())
            && self.max[0] >= self.min[0]
            && self.max[1] >= self.min[1]
    }
}

/// Entities of one kind left out of the view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkippedKind {
    /// The DXF entity name (`HATCH`, `DIMENSION`, …).
    pub kind: String,
    /// How many.
    pub count: usize,
}

/// A DXF drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DrawingSpec {
    /// Title shown above the view; defaults to the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The file, relative to the workspace.
    pub path: String,
    /// The AutoCAD release of the file (`R2000`, `R2018`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The insertion units of `$INSUNITS` (`mm`, `in`), when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The extents of `$EXTMIN`/`$EXTMAX`, when the file has valid ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_extents: Option<Extents>,
    /// The layers, in table order (layers only entities name come last).
    pub layers: Vec<DrawingLayer>,
    /// What is drawn.
    pub entities: Vec<DrawEntity>,
    /// Block inserts expanded into their entities.
    #[serde(default)]
    pub inserts: usize,
    /// Entities left out, by kind.
    #[serde(default)]
    pub skipped: Vec<SkippedKind>,
    /// Whether entities were cut at [`MAX_ENTITIES`].
    #[serde(default)]
    pub truncated: bool,
    /// Reader notes.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl DrawingSpec {
    /// The title, or the file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                self.path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&self.path)
                    .to_owned()
            })
    }

    /// The box of every entity's flattened points (text by its insertion
    /// point), or `None` without entities.
    #[must_use]
    pub fn computed_extents(&self) -> Option<Extents> {
        extents_of(self.entities.iter())
    }

    /// The box of the entities on the layers `visible` accepts.
    #[must_use]
    pub fn extents_where(&self, visible: impl Fn(usize) -> bool) -> Option<Extents> {
        extents_of(self.entities.iter().filter(|e| visible(e.layer)))
    }

    /// Entities by kind, most first.
    #[must_use]
    pub fn kind_counts(&self) -> Vec<(&'static str, usize)> {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for e in &self.entities {
            match counts.iter_mut().find(|(k, _)| *k == e.shape.kind()) {
                Some((_, n)) => *n += 1,
                None => counts.push((e.shape.kind(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        counts
    }

    /// The ACI color an entity is drawn in.
    #[must_use]
    pub fn color_of(&self, entity: &DrawEntity) -> u8 {
        entity
            .color
            .or_else(|| self.layers.get(entity.layer).map(|l| l.color))
            .unwrap_or(7)
    }

    /// The text entities, in file order.
    #[must_use]
    pub fn texts(&self) -> Vec<&str> {
        self.entities
            .iter()
            .filter_map(|e| match &e.shape {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The layers as CSV: name, color, on, entities.
    #[must_use]
    pub fn layers_csv(&self) -> String {
        let mut out = String::from("layer,color,on,entities\n");
        for l in &self.layers {
            out.push_str(&format!(
                "{},{},{},{}\n",
                csv_field(&l.name),
                l.color,
                !l.off,
                l.entities
            ));
        }
        out
    }

    /// Checks layer indices, colors and coordinates.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        const KIND: &str = "drawing";
        if self.path.trim().is_empty() {
            return Err(invalid(KIND, "the path is empty"));
        }
        if self.entities.is_empty() {
            return Err(invalid(KIND, "the drawing has nothing to show"));
        }
        if self.entities.len() > MAX_ENTITIES {
            return Err(invalid(KIND, format!("more than {MAX_ENTITIES} entities")));
        }
        if self.layers.len() > MAX_LAYERS {
            return Err(invalid(KIND, format!("more than {MAX_LAYERS} layers")));
        }
        if let Some(l) = self.layers.iter().find(|l| l.color == 0) {
            return Err(invalid(KIND, format!("layer `{}` has color 0", l.name)));
        }
        for (i, e) in self.entities.iter().enumerate() {
            if e.layer >= self.layers.len() {
                return Err(invalid(KIND, format!("entity {i} names a missing layer")));
            }
            if e.color == Some(0) {
                return Err(invalid(KIND, format!("entity {i} has color 0")));
            }
            if e.shape.coordinates().iter().any(|c| !c.is_finite()) {
                return Err(invalid(KIND, format!("entity {i} has a non-finite number")));
            }
            let bad = match &e.shape {
                Shape::Circle { radius, .. } | Shape::Arc { radius, .. } => *radius <= 0.0,
                Shape::Polyline { vertices, .. } => vertices.len() < 2,
                Shape::Text { height, .. } => *height < 0.0,
                Shape::Line { .. } | Shape::Point { .. } => false,
            };
            if bad {
                return Err(invalid(
                    KIND,
                    format!("entity {i} ({}) is degenerate", e.shape.kind()),
                ));
            }
        }
        if self.header_extents.is_some_and(|x| !x.valid()) {
            return Err(invalid(KIND, "the header extents are not a box"));
        }
        Ok(())
    }
}

fn extents_of<'a>(entities: impl Iterator<Item = &'a DrawEntity>) -> Option<Extents> {
    let mut out: Option<Extents> = None;
    for e in entities {
        for p in flatten(&e.shape) {
            let x = out.get_or_insert(Extents { min: p, max: p });
            for (k, c) in p.iter().enumerate() {
                x.min[k] = x.min[k].min(*c);
                x.max[k] = x.max[k].max(*c);
            }
        }
    }
    out
}

/// The points to stroke for `shape`, in order (a closed figure repeats
/// its first point; text is its insertion point).
#[must_use]
pub fn flatten(shape: &Shape) -> Vec<P2> {
    match shape {
        Shape::Line { a, b } => vec![*a, *b],
        Shape::Circle { center, radius } => arc_points(*center, *radius, 0.0, TAU),
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => {
            let (a, mut b) = (start.to_radians(), end.to_radians());
            while b <= a {
                b += TAU;
            }
            arc_points(*center, *radius, a, b)
        }
        Shape::Polyline { vertices, closed } => {
            let mut out = Vec::with_capacity(vertices.len() + 1);
            let n = vertices.len();
            let segments = if *closed { n } else { n.saturating_sub(1) };
            if let Some(first) = vertices.first() {
                out.push([first.x, first.y]);
            }
            for i in 0..segments {
                let (v, w) = (vertices[i], vertices[(i + 1) % n]);
                let (a, b) = ([v.x, v.y], [w.x, w.y]);
                if v.bulge.abs() < 1e-12 {
                    out.push(b);
                } else {
                    out.extend(bulge_points(a, b, v.bulge).into_iter().skip(1));
                }
            }
            out
        }
        Shape::Text { at, .. } | Shape::Point { at } => vec![*at],
    }
}

/// The circular arc a polyline segment from `a` to `b` with `bulge`
/// follows: centre, radius, start angle and signed sweep (radians).
#[must_use]
pub fn bulge_arc(a: P2, b: P2, bulge: f64) -> Option<(P2, f64, f64, f64)> {
    let chord = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    if chord <= 0.0 || bulge.abs() < 1e-12 {
        return None;
    }
    let sweep = 4.0 * bulge.atan();
    let radius = chord / (2.0 * (sweep / 2.0).sin().abs());
    // The centre is on the chord's bisector, at the sagitta-adjusted
    // distance, to the left of a→b for a positive bulge.
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let d = radius * (sweep / 2.0).cos();
    let n = [-(b[1] - a[1]) / chord, (b[0] - a[0]) / chord];
    let s = if bulge > 0.0 { 1.0 } else { -1.0 };
    let center = [mid[0] + s * d * n[0], mid[1] + s * d * n[1]];
    let start = (a[1] - center[1]).atan2(a[0] - center[0]);
    Some((center, radius, start, sweep))
}

/// Points along a bulged segment from `a` to `b` (both included).
#[must_use]
pub fn bulge_points(a: P2, b: P2, bulge: f64) -> Vec<P2> {
    let Some((center, radius, start, sweep)) = bulge_arc(a, b, bulge) else {
        return vec![a, b];
    };
    let mut out = arc_points(center, radius, start, start + sweep);
    // Land exactly on the vertices.
    if let Some(first) = out.first_mut() {
        *first = a;
    }
    if let Some(last) = out.last_mut() {
        *last = b;
    }
    out
}

/// Points on the circle `center`, `radius` from angle `a` to `b`
/// (radians, either direction), about [`CIRCLE_SEGMENTS`] per turn.
fn arc_points(center: P2, radius: f64, a: f64, b: f64) -> Vec<P2> {
    let sweep = b - a;
    // Rounded up from a share of a full turn: a small positive count.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let steps = ((sweep.abs() / TAU * CIRCLE_SEGMENTS as f64).ceil() as usize).clamp(2, 4096);
    (0..=steps)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = a + sweep * i as f64 / steps as f64;
            [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
        })
        .collect()
}

/// The standard RGB of AutoCAD Color Index `index` (1–255; 7 is white,
/// drawn in the foreground colour by viewers on light backgrounds).
#[must_use]
pub fn aci_rgb(index: u8) -> [u8; 3] {
    const BASE: [[u8; 3]; 10] = [
        [0, 0, 0],
        [255, 0, 0],
        [255, 255, 0],
        [0, 255, 0],
        [0, 255, 255],
        [0, 0, 255],
        [255, 0, 255],
        [255, 255, 255],
        [128, 128, 128],
        [192, 192, 192],
    ];
    const GRAYS: [u8; 6] = [51, 91, 132, 173, 214, 255];
    const VALUES: [f64; 5] = [255.0, 204.0, 153.0, 127.0, 76.0];
    match index {
        0..=9 => BASE[index as usize],
        250..=255 => {
            let g = GRAYS[(index - 250) as usize];
            [g, g, g]
        }
        _ => {
            let hue = f64::from(index / 10 - 1) * 15.0;
            let offset = index % 10;
            let v = VALUES[(offset / 2) as usize];
            let s = if offset.is_multiple_of(2) { 1.0 } else { 0.5 };
            hsv(hue, s, v)
        }
    }
}

/// HSV (hue in degrees, value 0–255) to RGB, truncated as AutoCAD's table.
fn hsv(hue: f64, s: f64, v: f64) -> [u8; 3] {
    let h = (hue % 360.0) / 60.0;
    let f = h - h.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    // `h` is in [0, 6).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (r, g, b) = match h.floor() as u8 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    // Each channel is within 0..=255.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = |c: f64| c.floor().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

/// Angle in degrees normalised to `[0, 360)`.
#[must_use]
pub fn normal_degrees(d: f64) -> f64 {
    let r = d.rem_euclid(360.0);
    if (r - 360.0).abs() < 1e-12 { 0.0 } else { r }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;

    fn entity(layer: usize, shape: Shape) -> DrawEntity {
        DrawEntity {
            layer,
            color: None,
            shape,
        }
    }

    fn spec() -> DrawingSpec {
        DrawingSpec {
            title: None,
            path: "dwg/plate.dxf".into(),
            version: Some("R2000".into()),
            units: Some("mm".into()),
            header_extents: None,
            layers: vec![
                DrawingLayer {
                    name: "0".into(),
                    color: 7,
                    off: false,
                    entities: 2,
                },
                DrawingLayer {
                    name: "Holes, M6".into(),
                    color: 1,
                    off: true,
                    entities: 1,
                },
            ],
            entities: vec![
                entity(
                    0,
                    Shape::Line {
                        a: [0.0, 0.0],
                        b: [100.0, 0.0],
                    },
                ),
                DrawEntity {
                    color: Some(3),
                    ..entity(
                        0,
                        Shape::Text {
                            at: [10.0, 10.0],
                            height: 2.5,
                            rotation: 0.0,
                            text: "PLATE".into(),
                        },
                    )
                },
                entity(
                    1,
                    Shape::Circle {
                        center: [50.0, 20.0],
                        radius: 5.0,
                    },
                ),
            ],
            inserts: 0,
            skipped: Vec::new(),
            truncated: false,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_drawing_is_valid_and_measured() {
        let spec = spec();
        assert_eq!(spec.validate(), Ok(()));
        let x = spec.computed_extents().unwrap();
        assert!((x.min[0] - 0.0).abs() < 1e-9 && (x.max[0] - 100.0).abs() < 1e-9);
        assert!((x.max[1] - 25.0).abs() < 1e-9, "{x:?}");
        let visible = spec.extents_where(|l| spec.layers[l].visible()).unwrap();
        assert!(
            (visible.max[1] - 10.0).abs() < 1e-9,
            "the circle's layer is off"
        );
        assert_eq!(spec.color_of(&spec.entities[1]), 3);
        assert_eq!(spec.color_of(&spec.entities[2]), 1, "by layer");
        assert_eq!(spec.texts(), ["PLATE"]);
        assert_eq!(spec.kind_counts()[0], ("circle", 1));
        assert_eq!(
            spec.layers_csv(),
            "layer,color,on,entities\n0,7,true,2\n\"Holes, M6\",1,false,1\n"
        );
    }

    #[test]
    fn broken_drawings_are_refused() {
        let mut layer = spec();
        layer.entities[0].layer = 9;
        assert!(layer.validate().is_err());
        let mut radius = spec();
        radius.entities[2].shape = Shape::Circle {
            center: [0.0; 2],
            radius: 0.0,
        };
        assert!(radius.validate().is_err());
        let mut nan = spec();
        nan.entities[0].shape = Shape::Line {
            a: [f64::NAN, 0.0],
            b: [0.0; 2],
        };
        assert!(nan.validate().is_err());
        let mut empty = spec();
        empty.entities.clear();
        assert!(empty.validate().is_err());
    }

    #[test]
    fn a_bulge_of_one_is_a_half_circle() {
        // From (0,0) to (2,0) with bulge 1: a CCW half circle of radius 1
        // around (1,0), below the chord (to the right of a→b going CCW).
        let (center, radius, start, sweep) = bulge_arc([0.0, 0.0], [2.0, 0.0], 1.0).unwrap();
        assert!(
            (center[0] - 1.0).abs() < 1e-12 && center[1].abs() < 1e-12,
            "{center:?}"
        );
        assert!((radius - 1.0).abs() < 1e-12);
        assert!((sweep - PI).abs() < 1e-12);
        assert!(
            (start.cos() + 1.0).abs() < 1e-12,
            "starts at the left: {start}"
        );
        let points = bulge_points([0.0, 0.0], [2.0, 0.0], 1.0);
        assert_eq!(points.first(), Some(&[0.0, 0.0]));
        assert_eq!(points.last(), Some(&[2.0, 0.0]));
        let lowest = points.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!(
            (lowest + 1.0).abs() < 1e-3,
            "CCW from the left end goes down: {lowest}"
        );
        // A closed square with one bulged side reaches past it.
        let square = Shape::Polyline {
            vertices: vec![
                PolyVertex {
                    x: 0.0,
                    y: 0.0,
                    bulge: 1.0,
                },
                PolyVertex {
                    x: 2.0,
                    y: 0.0,
                    bulge: 0.0,
                },
                PolyVertex {
                    x: 2.0,
                    y: 2.0,
                    bulge: 0.0,
                },
                PolyVertex {
                    x: 0.0,
                    y: 2.0,
                    bulge: 0.0,
                },
            ],
            closed: true,
        };
        let flat = flatten(&square);
        assert_eq!(flat.last(), Some(&[0.0, 0.0]), "closed");
        assert!(flat.iter().any(|p| p[1] < -0.99));
    }

    #[test]
    fn arcs_run_counter_clockwise_across_zero() {
        let arc = Shape::Arc {
            center: [0.0; 2],
            radius: 1.0,
            start: 270.0,
            end: 90.0,
        };
        let points = flatten(&arc);
        // Through 0°: the rightmost point is (1, 0).
        let right = points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        assert!((right - 1.0).abs() < 1e-3);
        assert!(points.iter().all(|p| p[0] >= -1e-9), "{points:?}");
        assert_eq!(normal_degrees(-90.0), 270.0);
        assert_eq!(normal_degrees(720.0), 0.0);
    }

    #[test]
    fn aci_colors_follow_the_table() {
        assert_eq!(aci_rgb(1), [255, 0, 0]);
        assert_eq!(aci_rgb(7), [255, 255, 255]);
        assert_eq!(aci_rgb(10), [255, 0, 0]);
        assert_eq!(aci_rgb(11), [255, 127, 127]);
        assert_eq!(aci_rgb(20), [255, 63, 0]);
        assert_eq!(aci_rgb(21), [255, 159, 127]);
        assert_eq!(aci_rgb(30), [255, 127, 0]);
        assert_eq!(aci_rgb(140), [0, 191, 255]);
        assert_eq!(aci_rgb(250), [51, 51, 51]);
    }
}
