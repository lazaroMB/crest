//! 2D DXF drawings (`show_dxf`) on ixmilia's `dxf` crate (ASCII and
//! binary DXF, R12 to R2018).
//!
//! Model-space entities become [`DrawingSpec`] shapes in drawing units:
//! lines, circles, arcs, LWPOLYLINE and 2D POLYLINE (bulges kept), TEXT
//! and MTEXT (formatting codes removed), points, ellipses (sampled) and
//! splines (evaluated with de Boor from their control points and knots, or
//! through their fit points). Block inserts are expanded with their
//! position, scale, rotation and row/column arrays, up to [`MAX_DEPTH`]
//! levels; entities on layer `0` take the insert's layer, `BYBLOCK`
//! colors the insert's color. Under a non-uniform scale or a mirror,
//! circles, arcs and bulges become polylines. Everything else (hatches,
//! dimensions, solids, images, 3D meshes, paper space) is counted by kind
//! and left out. Z is dropped and extrusion directions are not applied.

use std::collections::HashMap;
use std::f64::consts::TAU;

use domain::engineering::drawing::{
    DrawEntity, DrawingLayer, DrawingSpec, Extents, MAX_ENTITIES, P2, PolyVertex, Shape,
    SkippedKind, flatten,
};
use dxf::entities::{Entity, EntityType};
use dxf::enums::Units;
use dxf::{Block, Color, Drawing};

/// How deep inserts are expanded.
pub const MAX_DEPTH: usize = 8;
/// Samples of a full ellipse or of a spline span.
const SAMPLES: usize = 72;
/// Most copies of one array insert.
const MAX_ARRAY: usize = 10_000;

/// Reads `bytes` as a DXF drawing; `path` names it.
///
/// # Errors
///
/// A message when the file is not DXF or has nothing to draw.
pub fn parse(bytes: &[u8], path: &str) -> Result<DrawingSpec, String> {
    let mut reader = bytes;
    // dxf 0.6.2 drops a `\` escape shorter than six characters at the end
    // of a string when it reads Windows-1252 (`\P5 mm` loses its tail);
    // its UTF-8 path skips that step, so a file that is valid UTF-8 (every
    // ASCII file, and R2007+) is read that way and `\U+` decoded here.
    let loaded = if std::str::from_utf8(bytes).is_ok() {
        Drawing::load_with_encoding(&mut reader, encoding_rs::UTF_8)
    } else {
        Drawing::load(&mut reader)
    };
    let drawing = loaded.map_err(|e| format!("`{path}` is not a readable DXF file: {e}"))?;
    let mut reader = Reader::new(&drawing);
    for entity in drawing.entities() {
        if reader.full() {
            break;
        }
        if entity.common.is_in_paper_space {
            reader.skip("paper space");
            continue;
        }
        reader.add(entity, &Transform::IDENTITY, None, 0);
    }
    let Reader {
        mut layers,
        entities,
        inserts,
        skipped,
        truncated,
        approximated,
        ..
    } = reader;
    if entities.is_empty() {
        return Err(format!(
            "`{path}` has nothing the viewer can draw in model space{}",
            if skipped.is_empty() {
                String::new()
            } else {
                format!(
                    " (only {})",
                    skipped
                        .iter()
                        .map(|(k, n)| format!("{n} {k}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ));
    }
    for e in &entities {
        if let Some(layer) = layers.get_mut(e.layer) {
            layer.entities += 1;
        }
    }
    let mut skipped: Vec<SkippedKind> = skipped
        .into_iter()
        .map(|(kind, count)| SkippedKind { kind, count })
        .collect();
    skipped.sort_by(|a, b| b.count.cmp(&a.count).then(a.kind.cmp(&b.kind)));
    let mut warnings = Vec::new();
    if approximated > 0 {
        warnings.push(format!(
            "{approximated} ellipses, splines or scaled arcs are drawn as polylines"
        ));
    }
    let header = &drawing.header;
    let (min, max) = (
        &header.minimum_drawing_extents,
        &header.maximum_drawing_extents,
    );
    let extents = Extents {
        min: [min.x, min.y],
        max: [max.x, max.y],
    };
    let header_extents = (extents
        .min
        .iter()
        .chain(&extents.max)
        .all(|c| c.is_finite() && c.abs() < 1e19)
        && extents.max[0] >= extents.min[0]
        && extents.max[1] >= extents.min[1]
        && (extents.max[0] > extents.min[0] || extents.max[1] > extents.min[1]))
        .then_some(extents);
    Ok(DrawingSpec {
        title: None,
        path: path.to_owned(),
        version: Some(format!("{:?}", header.version)),
        units: unit_symbol(header.default_drawing_units),
        header_extents,
        layers,
        entities,
        inserts,
        skipped,
        truncated,
        warnings,
    })
}

/// The symbol of `$INSUNITS`, `None` when unitless.
fn unit_symbol(units: Units) -> Option<String> {
    let symbol = match units {
        Units::Unitless => return None,
        Units::Inches => "in",
        Units::Feet => "ft",
        Units::Miles => "mi",
        Units::Millimeters => "mm",
        Units::Centimeters => "cm",
        Units::Meters => "m",
        Units::Kilometers => "km",
        Units::Microinches => "µin",
        Units::Mils => "mil",
        Units::Yards => "yd",
        Units::Angstroms => "Å",
        Units::Nanometers => "nm",
        Units::Microns => "µm",
        Units::Decimeters => "dm",
        Units::Decameters => "dam",
        Units::Hectometers => "hm",
        Units::Gigameters => "Gm",
        Units::AstronomicalUnits => "au",
        Units::LightYears => "ly",
        Units::Parsecs => "pc",
        Units::USSurveyFeet => "ft (US survey)",
        Units::USSurveyInch => "in (US survey)",
        Units::USSurveyYard => "yd (US survey)",
        Units::USSurveyMile => "mi (US survey)",
    };
    Some(symbol.to_owned())
}

/// An affine map of the plane: `x' = a·x + b·y + c`, `y' = d·x + e·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Transform {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 1.0,
        f: 0.0,
    };

    /// Translate by `t`, rotate by `degrees`, scale by `(sx, sy)`, in
    /// that order outermost first (as an insert places its block).
    fn placing(t: P2, degrees: f64, sx: f64, sy: f64, base: P2) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        let (a, b, d, e) = (c * sx, -s * sy, s * sx, c * sy);
        Self {
            a,
            b,
            c: t[0] - (a * base[0] + b * base[1]),
            d,
            e,
            f: t[1] - (d * base[0] + e * base[1]),
        }
    }

    /// `self` after `inner`.
    fn then(&self, inner: &Self) -> Self {
        Self {
            a: self.a * inner.a + self.b * inner.d,
            b: self.a * inner.b + self.b * inner.e,
            c: self.a * inner.c + self.b * inner.f + self.c,
            d: self.d * inner.a + self.e * inner.d,
            e: self.d * inner.b + self.e * inner.e,
            f: self.d * inner.c + self.e * inner.f + self.f,
        }
    }

    fn apply(&self, p: P2) -> P2 {
        [
            self.a * p[0] + self.b * p[1] + self.c,
            self.d * p[0] + self.e * p[1] + self.f,
        ]
    }

    fn determinant(&self) -> f64 {
        self.a * self.e - self.b * self.d
    }

    /// The uniform scale and rotation (degrees) when the map is a
    /// similarity without a mirror.
    fn similarity(&self) -> Option<(f64, f64)> {
        (self.conformal() && self.determinant() > 0.0)
            .then(|| (self.a.hypot(self.d), self.d.atan2(self.a).to_degrees()))
    }

    /// Whether circles stay circles (a uniform scale, mirrored or not).
    fn conformal(&self) -> bool {
        let sx = self.a.hypot(self.d);
        let sy = self.b.hypot(self.e);
        (sx - sy).abs() < 1e-9 * sx.max(1.0)
            && (self.a * self.b + self.d * self.e).abs() < 1e-9 * sx.max(sy).max(1.0)
    }

    /// The mean scale (for text heights).
    fn scale(&self) -> f64 {
        self.determinant().abs().sqrt()
    }

    /// The rotation of the x axis, in degrees.
    fn rotation(&self) -> f64 {
        self.d.atan2(self.a).to_degrees()
    }
}

struct Reader<'a> {
    blocks: HashMap<&'a str, &'a Block>,
    layer_index: HashMap<String, usize>,
    layers: Vec<DrawingLayer>,
    entities: Vec<DrawEntity>,
    inserts: usize,
    skipped: HashMap<String, usize>,
    truncated: bool,
    approximated: usize,
}

/// What an insert passes on to its block's entities.
#[derive(Clone)]
struct Parent {
    layer: usize,
    color: Option<u8>,
}

impl<'a> Reader<'a> {
    fn new(drawing: &'a Drawing) -> Self {
        let mut reader = Self {
            blocks: drawing.blocks().map(|b| (b.name.as_str(), b)).collect(),
            layer_index: HashMap::new(),
            layers: Vec::new(),
            entities: Vec::new(),
            inserts: 0,
            skipped: HashMap::new(),
            truncated: false,
            approximated: 0,
        };
        for layer in drawing.layers() {
            if reader.layer_index.contains_key(&layer.name) {
                continue;
            }
            reader
                .layer_index
                .insert(layer.name.clone(), reader.layers.len());
            reader.layers.push(DrawingLayer {
                name: layer.name.clone(),
                color: index(&layer.color).unwrap_or(7),
                off: !layer.is_layer_on,
                entities: 0,
            });
        }
        reader
    }

    fn full(&mut self) -> bool {
        if self.entities.len() >= MAX_ENTITIES {
            self.truncated = true;
        }
        self.truncated
    }

    fn skip(&mut self, kind: &str) {
        *self.skipped.entry(kind.to_owned()).or_insert(0) += 1;
    }

    fn layer(&mut self, name: &str) -> usize {
        if let Some(&i) = self.layer_index.get(name) {
            return i;
        }
        let i = self.layers.len();
        self.layer_index.insert(name.to_owned(), i);
        self.layers.push(DrawingLayer {
            name: name.to_owned(),
            color: 7,
            off: false,
            entities: 0,
        });
        i
    }

    fn push(&mut self, layer: usize, color: Option<u8>, shape: Shape) {
        if !self.full() {
            self.entities.push(DrawEntity {
                layer,
                color,
                shape,
            });
        }
    }

    fn add(&mut self, entity: &'a Entity, at: &Transform, parent: Option<&Parent>, depth: usize) {
        let common = &entity.common;
        let layer = match parent {
            Some(p) if common.layer == "0" || common.layer.is_empty() => p.layer,
            _ => self.layer(&common.layer),
        };
        let color = if common.color.is_by_block() {
            parent.and_then(|p| p.color)
        } else {
            index(&common.color)
        };
        match &entity.specific {
            EntityType::Line(line) => {
                let shape = Shape::Line {
                    a: at.apply([line.p1.x, line.p1.y]),
                    b: at.apply([line.p2.x, line.p2.y]),
                };
                self.push(layer, color, shape);
            }
            EntityType::Circle(circle) => {
                let shape = Shape::Circle {
                    center: [circle.center.x, circle.center.y],
                    radius: circle.radius,
                };
                self.curve(layer, color, shape, at);
            }
            EntityType::Arc(arc) => {
                let shape = Shape::Arc {
                    center: [arc.center.x, arc.center.y],
                    radius: arc.radius,
                    start: arc.start_angle,
                    end: arc.end_angle,
                };
                self.curve(layer, color, shape, at);
            }
            EntityType::LwPolyline(poly) => {
                let vertices = poly
                    .vertices
                    .iter()
                    .map(|v| PolyVertex {
                        x: v.x,
                        y: v.y,
                        bulge: v.bulge,
                    })
                    .collect();
                self.polyline(layer, color, vertices, poly.is_closed(), at);
            }
            EntityType::Polyline(poly) => {
                if poly.is_polyface_mesh() || poly.is_3d_polygon_mesh() {
                    self.skip("3D mesh");
                    return;
                }
                let vertices = poly
                    .vertices()
                    .map(|v| PolyVertex {
                        x: v.location.x,
                        y: v.location.y,
                        bulge: v.bulge,
                    })
                    .collect();
                self.polyline(layer, color, vertices, poly.is_closed(), at);
            }
            EntityType::Text(text) => {
                let shape = Shape::Text {
                    at: at.apply([text.location.x, text.location.y]),
                    height: text.text_height.abs() * at.scale(),
                    rotation: text.rotation + at.rotation(),
                    text: plain_text(&text.value),
                };
                self.push(layer, color, shape);
            }
            EntityType::MText(text) => {
                let mut raw = text.extended_text.concat();
                raw.push_str(&text.text);
                let height = text.initial_text_height.abs();
                // The insertion point is the top left corner (the default
                // attachment); the first baseline is about a line lower.
                let corner = [text.insertion_point.x, text.insertion_point.y - height];
                let shape = Shape::Text {
                    at: at.apply(corner),
                    height: height * at.scale(),
                    rotation: text.rotation_angle + at.rotation(),
                    text: mtext_plain(&raw),
                };
                self.push(layer, color, shape);
            }
            EntityType::ModelPoint(point) => {
                let shape = Shape::Point {
                    at: at.apply([point.location.x, point.location.y]),
                };
                self.push(layer, color, shape);
            }
            EntityType::Ellipse(ellipse) => {
                let points = ellipse_points(
                    [ellipse.center.x, ellipse.center.y],
                    [ellipse.major_axis.x, ellipse.major_axis.y],
                    ellipse.minor_axis_ratio,
                    ellipse.start_parameter,
                    ellipse.end_parameter,
                );
                self.approximated += 1;
                self.points(layer, color, &points, false, at);
            }
            EntityType::Spline(spline) => {
                let points = spline_points(
                    usize::try_from(spline.degree_of_curve).unwrap_or(3),
                    &spline
                        .control_points
                        .iter()
                        .map(|p| [p.x, p.y])
                        .collect::<Vec<_>>(),
                    &spline.knot_values,
                    &spline.weight_values,
                )
                .unwrap_or_else(|| spline.fit_points.iter().map(|p| [p.x, p.y]).collect());
                if points.len() < 2 {
                    self.skip("degenerate spline");
                    return;
                }
                self.approximated += 1;
                self.points(layer, color, &points, spline.is_closed(), at);
            }
            EntityType::Insert(insert) => {
                let Some(block) = self.blocks.get(insert.name.as_str()).copied() else {
                    self.skip("insert of a missing block");
                    return;
                };
                if depth >= MAX_DEPTH {
                    self.skip("nested insert too deep");
                    return;
                }
                self.inserts += 1;
                let parent = Parent { layer, color };
                let base = [block.base_point.x, block.base_point.y];
                let (sx, sy) = (
                    nonzero(insert.x_scale_factor),
                    nonzero(insert.y_scale_factor),
                );
                let columns = usize::try_from(insert.column_count.max(1)).unwrap_or(1);
                let rows = usize::try_from(insert.row_count.max(1)).unwrap_or(1);
                let (s, c) = insert.rotation.to_radians().sin_cos();
                for (row, column) in (0..rows)
                    .flat_map(|r| (0..columns).map(move |k| (r, k)))
                    .take(MAX_ARRAY)
                {
                    // Array offsets are along the insert's rotated axes.
                    #[allow(clippy::cast_precision_loss)]
                    let (dx, dy) = (
                        column as f64 * insert.column_spacing,
                        row as f64 * insert.row_spacing,
                    );
                    let origin = [
                        insert.location.x + dx * c - dy * s,
                        insert.location.y + dx * s + dy * c,
                    ];
                    let place = at.then(&Transform::placing(origin, insert.rotation, sx, sy, base));
                    for inner in &block.entities {
                        if self.full() {
                            return;
                        }
                        self.add(inner, &place, Some(&parent), depth + 1);
                    }
                }
            }
            other => self.skip(&kind_name(other)),
        }
    }

    /// A circle or an arc: kept as one under a similarity, else drawn as
    /// a polyline.
    fn curve(&mut self, layer: usize, color: Option<u8>, shape: Shape, at: &Transform) {
        match (at.similarity(), shape) {
            (Some((scale, _)), Shape::Circle { center, radius }) => self.push(
                layer,
                color,
                Shape::Circle {
                    center: at.apply(center),
                    radius: radius * scale,
                },
            ),
            (
                Some((scale, turn_by)),
                Shape::Arc {
                    center,
                    radius,
                    start,
                    end,
                },
            ) => {
                self.push(
                    layer,
                    color,
                    Shape::Arc {
                        center: at.apply(center),
                        radius: radius * scale,
                        start: start + turn_by,
                        end: end + turn_by,
                    },
                );
            }
            (_, shape) => {
                self.approximated += 1;
                let closed = matches!(shape, Shape::Circle { .. });
                let points = flatten(&shape);
                self.points(layer, color, &points, closed, at);
            }
        }
    }

    fn polyline(
        &mut self,
        layer: usize,
        color: Option<u8>,
        vertices: Vec<PolyVertex>,
        closed: bool,
        at: &Transform,
    ) {
        if vertices.len() < 2 {
            self.skip("degenerate polyline");
            return;
        }
        let bulged = vertices.iter().any(|v| v.bulge.abs() > 1e-12);
        if !bulged || at.conformal() {
            let flip = if at.determinant() < 0.0 { -1.0 } else { 1.0 };
            let vertices = vertices
                .iter()
                .map(|v| {
                    let [x, y] = at.apply([v.x, v.y]);
                    PolyVertex {
                        x,
                        y,
                        bulge: v.bulge * flip,
                    }
                })
                .collect();
            self.push(layer, color, Shape::Polyline { vertices, closed });
        } else {
            // A bulge under a non-uniform scale is no longer circular.
            self.approximated += 1;
            let points = flatten(&Shape::Polyline { vertices, closed });
            self.points(layer, color, &points, false, at);
        }
    }

    /// A polyline through `points` mapped by `at`.
    fn points(
        &mut self,
        layer: usize,
        color: Option<u8>,
        points: &[P2],
        closed: bool,
        at: &Transform,
    ) {
        if points.len() < 2 {
            return;
        }
        let vertices = points
            .iter()
            .map(|&p| {
                let [x, y] = at.apply(p);
                PolyVertex { x, y, bulge: 0.0 }
            })
            .collect();
        self.push(layer, color, Shape::Polyline { vertices, closed });
    }
}

fn nonzero(scale: f64) -> f64 {
    if scale.is_finite() && scale != 0.0 {
        scale
    } else {
        1.0
    }
}

/// An ACI index of 1–255, `None` for BYLAYER, BYBLOCK or true color.
fn index(color: &Color) -> Option<u8> {
    color.index().filter(|&i| i > 0)
}

/// The DXF name of an entity kind the viewer leaves out.
fn kind_name(entity: &EntityType) -> String {
    let debug = format!("{entity:?}");
    let name = debug.split(['(', ' ', '{']).next().unwrap_or("other");
    let mut out = String::new();
    let mut previous: Option<char> = None;
    for ch in name.chars() {
        if ch.is_uppercase() && previous.is_some_and(char::is_lowercase) {
            out.push(' ');
        }
        out.extend(ch.to_lowercase());
        previous = Some(ch);
    }
    out
}

/// Points of an elliptical arc from parameter `t0` to `t1` (radians).
fn ellipse_points(center: P2, major: P2, ratio: f64, t0: f64, mut t1: f64) -> Vec<P2> {
    while t1 <= t0 {
        t1 += TAU;
    }
    let minor = [-major[1] * ratio, major[0] * ratio];
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let steps = (((t1 - t0) / TAU * SAMPLES as f64).ceil() as usize).max(4);
    (0..=steps)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let t = t0 + (t1 - t0) * i as f64 / steps as f64;
            let (s, c) = t.sin_cos();
            [
                center[0] + major[0] * c + minor[0] * s,
                center[1] + major[1] * c + minor[1] * s,
            ]
        })
        .collect()
}

/// Points of a (rational) B-spline of `degree` from its control points,
/// knots and weights (de Boor), sampled per knot span; `None` when the
/// knots do not fit the control points.
fn spline_points(degree: usize, control: &[P2], knots: &[f64], weights: &[f64]) -> Option<Vec<P2>> {
    let n = control.len();
    if degree == 0 || n <= degree || knots.len() != n + degree + 1 {
        return None;
    }
    let weight = |i: usize| weights.get(i).copied().filter(|w| *w > 0.0).unwrap_or(1.0);
    let (lo, hi) = (knots[degree], knots[n]);
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let spans = n - degree;
    let steps = (spans * SAMPLES / 4).clamp(8, 4096);
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        #[allow(clippy::cast_precision_loss)]
        let t = lo + (hi - lo) * i as f64 / steps as f64;
        // The span k with knots[k] ≤ t < knots[k+1], within degree..n.
        let k = (degree..n).rev().find(|&k| knots[k] <= t).unwrap_or(degree);
        // Homogeneous points of the span.
        let mut d: Vec<[f64; 3]> = (0..=degree)
            .map(|j| {
                let idx = k - degree + j;
                let w = weight(idx);
                [control[idx][0] * w, control[idx][1] * w, w]
            })
            .collect();
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let idx = k - degree + j;
                let denom = knots[idx + degree + 1 - r] - knots[idx];
                let alpha = if denom.abs() < 1e-300 {
                    0.0
                } else {
                    (t - knots[idx]) / denom
                };
                let previous = d[j - 1];
                for (c, value) in d[j].iter_mut().enumerate() {
                    *value = (1.0 - alpha) * previous[c] + alpha * *value;
                }
            }
        }
        let [x, y, w] = d[degree];
        if w.abs() < 1e-300 {
            return None;
        }
        out.push([x / w, y / w]);
    }
    Some(out)
}

/// TEXT special codes: `%%d` °, `%%p` ±, `%%c` ⌀, `%%u`/`%%o` dropped,
/// `\U+XXXX` the character.
fn plain_text(text: &str) -> String {
    let text = unicode_escapes(text);
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("%%") {
        out.push_str(&rest[..at]);
        let code = rest[at + 2..].chars().next();
        match code.map(|c| c.to_ascii_lowercase()) {
            Some('d') => out.push('°'),
            Some('p') => out.push('±'),
            Some('c') => out.push('⌀'),
            Some('%') => out.push('%'),
            Some('u' | 'o') => {}
            Some(other) => {
                out.push_str("%%");
                out.push(other);
            }
            None => out.push_str("%%"),
        }
        rest = &rest[at + 2 + code.map_or(0, char::len_utf8)..];
    }
    out.push_str(rest);
    out
}

/// `\U+XXXX` escapes as their characters.
fn unicode_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\\U+") {
        out.push_str(&rest[..at]);
        let hex = rest.get(at + 3..at + 7);
        match hex
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32)
        {
            Some(c) => {
                out.push(c);
                rest = &rest[at + 7..];
            }
            None => {
                out.push_str("\\U+");
                rest = &rest[at + 3..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// MTEXT without its formatting: `\P` is a line break, `\~` a space,
/// `\S a^b;` a stacked `a/b`, braces and the `\f…;`, `\H…;`, `\C…;`
/// style codes dropped, `\\`, `\{`, `\}` kept as the character.
fn mtext_plain(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' | '}' => {}
            '\\' => match chars.next() {
                Some('P' | 'p' | 'X') => out.push('\n'),
                Some('~') => out.push(' '),
                Some(c @ ('\\' | '{' | '}')) => out.push(c),
                Some('S') => {
                    let mut stacked = String::new();
                    for c in chars.by_ref() {
                        if c == ';' {
                            break;
                        }
                        stacked.push(c);
                    }
                    out.push_str(&stacked.replace(['^', '#'], "/"));
                }
                Some('L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'N') => {}
                Some('U') if chars.peek() == Some(&'+') => {
                    chars.next();
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(_) => {
                    // A code with an argument up to `;`.
                    for c in chars.by_ref() {
                        if c == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            other => out.push(other),
        }
    }
    plain_text(out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtext_and_text_codes_are_removed() {
        assert_eq!(
            mtext_plain("{\\fArial|b1;Thickness}\\P5 mm"),
            "Thickness\n5 mm"
        );
        assert_eq!(mtext_plain("\\H2.5x;Ø\\S1^2; in\\~x"), "Ø1/2 in x");
        assert_eq!(mtext_plain("a\\\\b \\{c\\}"), "a\\b {c}");
        assert_eq!(mtext_plain("\\U+00B0C"), "°C");
        assert_eq!(plain_text("45%%d %%p0.1 %%c6 100%%%"), "45° ±0.1 ⌀6 100%");
        assert_eq!(plain_text("\\U+00D8 6 \\U+zz"), "Ø 6 \\U+zz");
    }

    #[test]
    fn transforms_compose_and_detect_similarities() {
        let t = Transform::placing([10.0, 0.0], 90.0, 2.0, 2.0, [1.0, 0.0]);
        let p = t.apply([2.0, 0.0]);
        assert!(
            (p[0] - 10.0).abs() < 1e-9 && (p[1] - 2.0).abs() < 1e-9,
            "{p:?}"
        );
        let (scale, turn) = t.similarity().unwrap();
        assert!((scale - 2.0).abs() < 1e-9 && (turn - 90.0).abs() < 1e-9);
        let stretched = Transform::placing([0.0; 2], 0.0, 2.0, 1.0, [0.0; 2]);
        assert!(stretched.similarity().is_none());
        let mirrored = Transform::placing([0.0; 2], 0.0, -1.0, 1.0, [0.0; 2]);
        assert!(mirrored.similarity().is_none());
        let both = t.then(&stretched);
        let q = both.apply([1.0, 0.0]);
        let direct = t.apply(stretched.apply([1.0, 0.0]));
        assert!((q[0] - direct[0]).abs() < 1e-9 && (q[1] - direct[1]).abs() < 1e-9);
    }

    #[test]
    fn a_quadratic_bezier_spline_is_evaluated() {
        // Clamped degree 2 with three control points: a parabola from
        // (0,0) to (2,0) through (1, 0.5) at its middle.
        let points = spline_points(
            2,
            &[[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]],
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &[],
        )
        .unwrap();
        assert_eq!(points.first(), Some(&[0.0, 0.0]));
        let last = points.last().unwrap();
        assert!((last[0] - 2.0).abs() < 1e-9 && last[1].abs() < 1e-9);
        let top = points.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        assert!((top - 0.5).abs() < 1e-3, "{top}");
        assert!(spline_points(3, &[[0.0; 2]; 3], &[0.0; 7], &[]).is_none());
    }

    #[test]
    fn ellipses_are_sampled_on_their_axes() {
        let points = ellipse_points([0.0, 0.0], [4.0, 0.0], 0.5, 0.0, TAU);
        let right = points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        let top = points.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        assert!(
            (right - 4.0).abs() < 1e-9 && (top - 2.0).abs() < 1e-2,
            "{right} {top}"
        );
    }

    #[test]
    fn kinds_are_named_in_words() {
        let solid = EntityType::Solid3D(dxf::entities::Solid3D::default());
        assert_eq!(kind_name(&solid), "solid3d");
        let face = EntityType::Face3D(dxf::entities::Face3D::default());
        assert_eq!(kind_name(&face), "face3d");
    }

    #[test]
    fn other_bytes_are_refused() {
        assert!(parse(b"hello", "a.dxf").is_err());
    }
}
