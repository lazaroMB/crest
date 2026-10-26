//! A map of GeoJSON vector data (`show_map`).
//!
//! The adapter reads the features, reprojects them from their coordinate
//! reference system ([`MapCrs`]: WGS 84 by default, as RFC 7946 says, or
//! the legacy `crs` member, or one the caller gave) to Web Mercator for
//! display, and measures them on the WGS 84 ellipsoid: the box, the
//! geodesic area of polygons and the geodesic length of lines, per
//! feature and in total, the totals in a [`Computed`] envelope. The UI
//! paints the display coordinates as they are, and colours features by a
//! property with [`MapSpec::colouring`].

use std::collections::BTreeMap;
use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::block::BlockError;
use crate::computed::Computed;

/// Most features kept.
pub const MAX_FEATURES: usize = 50_000;
/// Most vertices (all features together) kept.
pub const MAX_VERTICES: usize = 2_000_000;
/// Radius of the Web Mercator sphere (EPSG:3857), in metres.
pub const EARTH_RADIUS: f64 = 6_378_137.0;
/// Northernmost latitude Web Mercator shows (and its mirror south).
pub const MAX_LATITUDE: f64 = 85.051_128_779_806_59;
/// Categories a categorical colouring tells apart; the rest are "other".
pub const MAX_CATEGORIES: usize = 10;

/// A point in Web Mercator metres (x east, y north).
pub type Xy = [f64; 2];

/// The GeoJSON geometry type of a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeometryKind {
    /// `Point`.
    Point,
    /// `MultiPoint`.
    MultiPoint,
    /// `LineString`.
    LineString,
    /// `MultiLineString`.
    MultiLineString,
    /// `Polygon`.
    Polygon,
    /// `MultiPolygon`.
    MultiPolygon,
    /// `GeometryCollection`.
    GeometryCollection,
}

impl GeometryKind {
    /// The GeoJSON name (`MultiPolygon`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::MultiPoint => "MultiPoint",
            Self::LineString => "LineString",
            Self::MultiLineString => "MultiLineString",
            Self::Polygon => "Polygon",
            Self::MultiPolygon => "MultiPolygon",
            Self::GeometryCollection => "GeometryCollection",
        }
    }
}

/// One feature, in display coordinates, with what the engine measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapFeature {
    /// The feature's `id`, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Its geometry type.
    pub kind: GeometryKind,
    /// Points (Web Mercator).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Xy>,
    /// Lines, each a vertex list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<Vec<Xy>>,
    /// Polygons, each a list of closed rings: the exterior, then holes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygons: Vec<Vec<Vec<Xy>>>,
    /// The feature's properties as written.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub properties: Map<String, Value>,
    /// Box in degrees: west, south, east, north (WGS 84).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    /// Geodesic area of its polygons, holes taken out, in m².
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_m2: Option<f64>,
    /// Geodesic length of its lines, in m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_m: Option<f64>,
    /// Geodesic perimeter of its polygons (holes included), in m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perimeter_m: Option<f64>,
}

impl MapFeature {
    /// The vertices of every part.
    pub fn vertices(&self) -> impl Iterator<Item = &Xy> {
        self.points
            .iter()
            .chain(self.lines.iter().flatten())
            .chain(self.polygons.iter().flatten().flatten())
    }

    /// The label for lists: its `name` (or `title`, `label`) property,
    /// its id, or `#n`.
    #[must_use]
    pub fn label(&self, index: usize) -> String {
        for key in ["name", "NAME", "title", "label"] {
            if let Some(Value::String(s)) = self.properties.get(key)
                && !s.trim().is_empty()
            {
                return s.clone();
            }
        }
        self.id.clone().unwrap_or_else(|| format!("#{}", index + 1))
    }

    /// Whether `p` (Web Mercator) is on the feature: within `tolerance`
    /// metres of a point or line, or inside a polygon (holes excluded).
    #[must_use]
    pub fn hit(&self, p: Xy, tolerance: f64) -> bool {
        let t2 = tolerance * tolerance;
        if self.points.iter().any(|q| distance2(p, *q) <= t2) {
            return true;
        }
        if self.lines.iter().any(|line| {
            line.windows(2)
                .any(|s| segment_distance2(p, s[0], s[1]) <= t2)
        }) {
            return true;
        }
        self.polygons.iter().any(|rings| {
            // Even-odd over all rings: inside the exterior, outside holes.
            let inside = rings.iter().filter(|ring| in_ring(p, ring)).count() % 2 == 1;
            inside
                || rings.iter().any(|ring| {
                    ring.windows(2)
                        .any(|s| segment_distance2(p, s[0], s[1]) <= t2)
                })
        })
    }
}

fn distance2(a: Xy, b: Xy) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn segment_distance2(p: Xy, a: Xy, b: Xy) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance2(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Ray casting: whether `p` is inside the closed `ring`.
fn in_ring(p: Xy, ring: &[Xy]) -> bool {
    let mut inside = false;
    for s in ring.windows(2) {
        let (a, b) = (s[0], s[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Where the coordinate reference system came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CrsSource {
    /// None was named: WGS 84 longitude/latitude, as RFC 7946 requires.
    #[default]
    Default,
    /// The file's legacy (2008) `crs` member.
    Member,
    /// The caller gave it.
    Given,
}

/// The coordinate reference system of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapCrs {
    /// EPSG code, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epsg: Option<u32>,
    /// Name to show (`WGS 84`, `WGS 84 / UTM zone 32N`, or the proj
    /// string given).
    pub name: String,
    /// Where it came from.
    #[serde(default)]
    pub source: CrsSource,
    /// The PROJ definition used to reproject it.
    pub definition: String,
}

impl MapCrs {
    /// `EPSG:32632 · WGS 84 / UTM zone 32N`, or the name alone.
    #[must_use]
    pub fn label(&self) -> String {
        match self.epsg {
            Some(code) => format!("EPSG:{code} · {}", self.name),
            None => self.name.clone(),
        }
    }

    /// Where it came from, in words.
    #[must_use]
    pub fn source_text(&self) -> &'static str {
        match self.source {
            CrsSource::Default => "default (RFC 7946)",
            CrsSource::Member => "from the file's crs member",
            CrsSource::Given => "given",
        }
    }
}

/// A map of features.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MapSpec {
    /// Title shown above the map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Coordinate reference system of the source.
    pub crs: MapCrs,
    /// The features with geometry, in file order.
    pub features: Vec<MapFeature>,
    /// Features in the file (more than `features` when capped or some had
    /// no geometry).
    pub total_features: usize,
    /// Features without geometry, not drawn.
    #[serde(default)]
    pub without_geometry: usize,
    /// The property to colour by at first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_by: Option<String>,
    /// The measures (box, area, length) as an envelope.
    pub measures: Computed,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// How features are coloured by a property.
#[derive(Debug, Clone, PartialEq)]
pub enum Colouring {
    /// Text (or few) values: a palette slot per category, the most common
    /// first; `None` for "other" or a missing value.
    Categorical {
        /// Category values and their feature counts.
        categories: Vec<(String, usize)>,
        /// Category index per feature.
        of: Vec<Option<usize>>,
        /// Values beyond [`MAX_CATEGORIES`], counted as other.
        other: usize,
    },
    /// Numbers: a position from 0 (min) to 1 (max) per feature.
    Sequential {
        /// Smallest value.
        min: f64,
        /// Largest value.
        max: f64,
        /// Position per feature; `None` when missing or not a number.
        of: Vec<Option<f64>>,
    },
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "map",
        reason: reason.into(),
    }
}

/// A property value as text (strings without quotes).
#[must_use]
pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Longitude and latitude (degrees) of a Web Mercator point.
#[must_use]
pub fn to_lon_lat(p: Xy) -> (f64, f64) {
    let lon = (p[0] / EARTH_RADIUS).to_degrees();
    let lat = (2.0 * (p[1] / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    (lon, lat)
}

/// Ground metres per Web Mercator metre at latitude `lat` (degrees): the
/// projection stretches distances by `1 / cos(lat)`.
#[must_use]
pub fn ground_factor(lat: f64) -> f64 {
    lat.to_radians().cos().max(1e-9)
}

/// The longest "nice" distance (1, 2 or 5 × 10ⁿ metres) not over
/// `max_metres`, and its label (`500 m`, `2 km`).
#[must_use]
pub fn scale_bar(max_metres: f64) -> Option<(f64, String)> {
    if !(max_metres.is_finite() && max_metres > 0.0) {
        return None;
    }
    let exp = max_metres.log10().floor();
    let base = 10f64.powf(exp);
    let step = [5.0, 2.0, 1.0]
        .into_iter()
        .map(|m| m * base)
        .find(|v| *v <= max_metres)
        .unwrap_or(base);
    let label = if step >= 1000.0 {
        format!("{} km", trim(step / 1000.0))
    } else if step >= 1.0 {
        format!("{} m", trim(step))
    } else {
        format!("{} cm", trim(step * 100.0))
    };
    Some((step, label))
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// An area for people: `850 m²`, `12.4 ha`, `3.21 km²`.
#[must_use]
pub fn area_text(m2: f64) -> String {
    if m2 >= 1e6 {
        format!("{} km²", significant(m2 / 1e6))
    } else if m2 >= 1e4 {
        format!("{} ha", significant(m2 / 1e4))
    } else {
        format!("{} m²", significant(m2))
    }
}

/// A length for people: `850 m`, `12.4 km`.
#[must_use]
pub fn length_text(m: f64) -> String {
    if m >= 1000.0 {
        format!("{} km", significant(m / 1000.0))
    } else {
        format!("{} m", significant(m))
    }
}

/// `v` with 3 significant digits (at least the integer part).
fn significant(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let digits = (2 - v.abs().log10().floor() as i32).clamp(0, 6) as usize;
    format!("{v:.digits$}")
}

impl MapSpec {
    /// Checks sizes, coordinates and the envelope.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.features.len() > MAX_FEATURES {
            return Err(invalid(format!(
                "{} features; at most {MAX_FEATURES}",
                self.features.len()
            )));
        }
        let vertices = self.vertex_count();
        if vertices > MAX_VERTICES {
            return Err(invalid(format!(
                "{vertices} vertices; at most {MAX_VERTICES}"
            )));
        }
        for (ix, f) in self.features.iter().enumerate() {
            if f.vertices()
                .any(|p| !(p[0].is_finite() && p[1].is_finite()))
            {
                return Err(invalid(format!(
                    "feature {} has a non-finite coordinate",
                    ix + 1
                )));
            }
            if f.polygons.iter().flatten().any(|ring| ring.len() < 4) {
                return Err(invalid(format!(
                    "feature {} has a polygon ring with fewer than 4 positions",
                    ix + 1
                )));
            }
            if f.lines.iter().any(|line| line.len() < 2) {
                return Err(invalid(format!(
                    "feature {} has a line with fewer than 2 positions",
                    ix + 1
                )));
            }
        }
        if self.crs.definition.trim().is_empty() {
            return Err(invalid("the CRS has no definition"));
        }
        self.measures
            .validate()
            .map_err(|e| invalid(format!("measures envelope: {e}")))
    }

    /// Vertices of all features.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.features.iter().map(|f| f.vertices().count()).sum()
    }

    /// The title, the file name or `Map`.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map_or_else(|| "Map".to_owned(), str::to_owned)
    }

    /// The display box (Web Mercator): lowest and highest corner.
    #[must_use]
    pub fn extent(&self) -> Option<(Xy, Xy)> {
        self.features
            .iter()
            .flat_map(MapFeature::vertices)
            .fold(None, |acc, p| match acc {
                None => Some((*p, *p)),
                Some((lo, hi)) => Some((
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )),
            })
    }

    /// Features per geometry type, in type order.
    #[must_use]
    pub fn kind_counts(&self) -> Vec<(GeometryKind, usize)> {
        let mut counts: BTreeMap<u8, (GeometryKind, usize)> = BTreeMap::new();
        for f in &self.features {
            let entry = counts.entry(f.kind as u8).or_insert((f.kind, 0));
            entry.1 += 1;
        }
        counts.into_values().collect()
    }

    /// Property keys of the features, in first-seen order.
    #[must_use]
    pub fn property_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for f in &self.features {
            for k in f.properties.keys() {
                if !keys.iter().any(|seen| seen == k) {
                    keys.push(k.clone());
                }
            }
        }
        keys
    }

    /// The topmost feature at `p` (later features are drawn above, and
    /// points and lines above polygons), within `tolerance` metres.
    #[must_use]
    pub fn pick(&self, p: Xy, tolerance: f64) -> Option<usize> {
        let mut polygon = None;
        for (ix, f) in self.features.iter().enumerate().rev() {
            if !f.hit(p, tolerance) {
                continue;
            }
            if f.points.is_empty() && f.lines.is_empty() {
                polygon.get_or_insert(ix);
            } else {
                return Some(ix);
            }
        }
        polygon
    }

    /// The colouring by property `key`: sequential when every value set
    /// is a number (and there are more than [`MAX_CATEGORIES`] or they
    /// are not whole), categorical otherwise. `None` when no feature has
    /// it.
    #[must_use]
    pub fn colouring(&self, key: &str) -> Option<Colouring> {
        let values: Vec<Option<&Value>> = self
            .features
            .iter()
            .map(|f| f.properties.get(key).filter(|v| !v.is_null()))
            .collect();
        if values.iter().all(Option::is_none) {
            return None;
        }
        let numbers: Vec<Option<f64>> = values.iter().map(|v| v.and_then(Value::as_f64)).collect();
        let all_numbers = values
            .iter()
            .zip(&numbers)
            .all(|(v, n)| v.is_none() || n.is_some());
        let mut distinct: Vec<f64> = numbers.iter().flatten().copied().collect();
        distinct.sort_by(f64::total_cmp);
        distinct.dedup();
        let few_whole =
            distinct.len() <= MAX_CATEGORIES && distinct.iter().all(|v| v.fract() == 0.0);
        if all_numbers && !few_whole {
            let (min, max) = (distinct[0], distinct[distinct.len() - 1]);
            let span = max - min;
            let of = numbers
                .iter()
                .map(|n| n.map(|v| if span > 0.0 { (v - min) / span } else { 0.5 }))
                .collect();
            return Some(Colouring::Sequential { min, max, of });
        }
        let texts: Vec<Option<String>> = values.iter().map(|v| v.map(value_text)).collect();
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for t in texts.iter().flatten() {
            *counts.entry(t.as_str()).or_default() += 1;
        }
        let mut ranked: Vec<(String, usize)> =
            counts.into_iter().map(|(k, n)| (k.to_owned(), n)).collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let other = ranked.iter().skip(MAX_CATEGORIES).map(|(_, n)| n).sum();
        ranked.truncate(MAX_CATEGORIES);
        let of = texts
            .iter()
            .map(|t| {
                t.as_ref()
                    .and_then(|t| ranked.iter().position(|(k, _)| k == t))
            })
            .collect();
        Some(Colouring::Categorical {
            categories: ranked,
            of,
            other,
        })
    }

    /// The features as CSV: index, id, label, type, area (m²), length
    /// and perimeter (m), box and every property.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let keys = self.property_keys();
        let mut out =
            String::from("index,id,label,type,area_m2,length_m,perimeter_m,west,south,east,north");
        for k in &keys {
            out.push(',');
            out.push_str(&csv_field(k));
        }
        out.push('\n');
        let num = |v: Option<f64>| v.map(|v| format!("{v}")).unwrap_or_default();
        for (ix, f) in self.features.iter().enumerate() {
            let b = f.bbox;
            let _ = write!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{}",
                ix + 1,
                csv_field(f.id.as_deref().unwrap_or("")),
                csv_field(&f.label(ix)),
                f.kind.name(),
                num(f.area_m2),
                num(f.length_m),
                num(f.perimeter_m),
                num(b.map(|b| b[0])),
                num(b.map(|b| b[1])),
                num(b.map(|b| b[2])),
                num(b.map(|b| b[3])),
            );
            for k in &keys {
                out.push(',');
                if let Some(v) = f.properties.get(k).filter(|v| !v.is_null()) {
                    out.push_str(&csv_field(&value_text(v)));
                }
            }
            out.push('\n');
        }
        out
    }
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::computed::Engine;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Xy> {
        vec![
            [x0, y0],
            [x0 + s, y0],
            [x0 + s, y0 + s],
            [x0, y0 + s],
            [x0, y0],
        ]
    }

    fn feature(kind: GeometryKind, props: Value) -> MapFeature {
        MapFeature {
            id: None,
            kind,
            points: Vec::new(),
            lines: Vec::new(),
            polygons: Vec::new(),
            properties: props.as_object().cloned().unwrap_or_default(),
            bbox: None,
            area_m2: None,
            length_m: None,
            perimeter_m: None,
        }
    }

    fn spec() -> MapSpec {
        let mut park = feature(
            GeometryKind::Polygon,
            json!({"name": "Park", "kind": "green", "pop": 10.5}),
        );
        park.polygons = vec![vec![square(0.0, 0.0, 100.0), square(40.0, 40.0, 20.0)]];
        let mut road = feature(
            GeometryKind::LineString,
            json!({"kind": "road", "pop": 3.25}),
        );
        road.lines = vec![vec![[0.0, 120.0], [100.0, 120.0]]];
        let mut well = feature(GeometryKind::Point, json!({"kind": "green"}));
        well.points = vec![[10.0, 10.0]];
        MapSpec {
            title: None,
            path: Some("geo/parks.geojson".into()),
            crs: MapCrs {
                epsg: Some(4326),
                name: "WGS 84".into(),
                source: CrsSource::Default,
                definition: "+proj=longlat +datum=WGS84 +no_defs".into(),
            },
            features: vec![park, road, well],
            total_features: 3,
            without_geometry: 0,
            color_by: None,
            measures: Computed::new(
                "ge_1",
                "geo.measures",
                "Geodesic measures",
                Engine {
                    name: "crest-geo".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn picks_points_lines_and_polygons_but_not_holes() {
        let s = spec();
        s.validate().unwrap();
        assert_eq!(
            s.pick([10.0, 10.0], 2.0),
            Some(2),
            "the point is above the park"
        );
        assert_eq!(s.pick([20.0, 80.0], 2.0), Some(0));
        assert_eq!(s.pick([50.0, 50.0], 2.0), None, "inside the hole");
        assert_eq!(s.pick([50.0, 121.0], 2.0), Some(1));
        assert_eq!(s.pick([500.0, 500.0], 2.0), None);
        assert_eq!(s.extent(), Some(([0.0, 0.0], [100.0, 120.0])));
        assert_eq!(s.vertex_count(), 13);
        assert_eq!(s.features[1].label(1), "#2");
        assert_eq!(s.features[0].label(0), "Park");
    }

    #[test]
    fn colours_by_category_or_sequence() {
        let s = spec();
        let Some(Colouring::Categorical {
            categories,
            of,
            other,
        }) = s.colouring("kind")
        else {
            panic!("categorical");
        };
        assert_eq!(categories, vec![("green".into(), 2), ("road".into(), 1)]);
        assert_eq!(of, vec![Some(0), Some(1), Some(0)]);
        assert_eq!(other, 0);
        let Some(Colouring::Sequential { min, max, of }) = s.colouring("pop") else {
            panic!("sequential");
        };
        assert_eq!((min, max), (3.25, 10.5));
        assert_eq!(of, vec![Some(1.0), Some(0.0), None]);
        assert!(s.colouring("missing").is_none());
        assert_eq!(s.property_keys(), vec!["name", "kind", "pop"]);
    }

    #[test]
    fn mercator_scale_and_texts() {
        let (lon, lat) = to_lon_lat([1_113_194.907_932_735_7, 6_446_275.841_017_158]);
        assert!(
            (lon - 10.0).abs() < 1e-9 && (lat - 50.0).abs() < 1e-9,
            "{lon} {lat}"
        );
        assert_eq!(scale_bar(730.0), Some((500.0, "500 m".into())));
        assert_eq!(scale_bar(2_500.0), Some((2_000.0, "2 km".into())));
        assert_eq!(scale_bar(0.3).map(|s| s.1), Some("20 cm".into()));
        assert_eq!(area_text(12_345_678.0), "12.3 km²");
        assert_eq!(area_text(54_321.0), "5.43 ha");
        assert_eq!(length_text(850.0), "850 m");
        assert!((ground_factor(60.0) - 0.5).abs() < 1e-12);
        let csv = spec().to_csv();
        assert!(csv.starts_with(
            "index,id,label,type,area_m2,length_m,perimeter_m,west,south,east,north,name,kind,pop\n"
        ));
        assert!(
            csv.contains("\n2,,#2,LineString,,,,,,,,,road,3.25\n"),
            "{csv}"
        );
    }

    #[test]
    fn rejects_short_rings() {
        let mut s = spec();
        s.features[0].polygons[0][1].truncate(3);
        assert!(
            s.validate()
                .unwrap_err()
                .to_string()
                .contains("fewer than 4")
        );
    }
}
