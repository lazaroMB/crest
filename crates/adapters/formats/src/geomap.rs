//! GeoJSON for `show_map`: features read with the `geojson` crate,
//! reprojected with `proj4rs` and measured with `geo`.
//!
//! Coordinates are in the request's CRS when it names one, else in the
//! legacy (2008) `crs` member's, else WGS 84 longitude/latitude (RFC
//! 7946). EPSG codes are looked up in a small bundled table ([`epsg`]);
//! any other CRS can be given as a PROJ string. Every position is taken to
//! WGS 84 (lon/lat) and from there to Web Mercator (EPSG:3857) for
//! display, latitudes clamped to ±85.05°. The measures are geodesic on
//! the WGS 84 ellipsoid (Karney 2013, through geo's `GeodesicArea` and
//! `Geodesic` length): the area of polygons with their holes taken out,
//! the length of lines and the perimeter of polygons (holes included).

use std::collections::BTreeMap;

use application::ports::formats::MapRequest;
use domain::computed::{Computed, ComputedValue};
use domain::science::geomap::{
    CrsSource, GeometryKind, MAX_FEATURES, MAX_LATITUDE, MAX_VERTICES, MapCrs, MapFeature, MapSpec,
    Xy, area_text, length_text,
};
use geo::{Geodesic, GeodesicArea, Length, LineString, Polygon};
use geojson::{Feature, GeoJson, Geometry, GeometryValue, JsonObject, Position};
use proj4rs::proj::Proj;
use serde_json::{Map, Value};

use crate::chem;

/// Envelope kind of the measures.
pub const KIND: &str = "geo.measures";
/// Engine name of the map measures.
pub const GEO_ENGINE_NAME: &str = "crest-geo";
/// The libraries underneath.
pub const BACKEND: &str = "geo 0.33.1 (geographiclib), proj4rs 0.2.1";
/// Features whose own measures go into the envelope too.
pub const ENVELOPE_FEATURES: usize = 100;

const WGS84: &str = "+proj=longlat +datum=WGS84 +no_defs";
const WEB_MERCATOR: &str =
    "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +no_defs";

/// The name and PROJ definition of EPSG `code`, for the codes crest
/// bundles: WGS 84 and other geographic systems, Web Mercator, every
/// WGS 84, ETRS89 and NAD83 UTM zone, and common national grids.
#[must_use]
pub fn epsg(code: u32) -> Option<(String, String)> {
    let fixed = |name: &str, def: &str| Some((name.to_owned(), def.to_owned()));
    match code {
        4326 => fixed("WGS 84", WGS84),
        4269 => fixed("NAD83", "+proj=longlat +datum=NAD83 +no_defs"),
        4258 => fixed(
            "ETRS89",
            "+proj=longlat +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +no_defs",
        ),
        4283 => fixed(
            "GDA94",
            "+proj=longlat +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +no_defs",
        ),
        7844 => fixed(
            "GDA2020",
            "+proj=longlat +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +no_defs",
        ),
        3857 | 900_913 | 3785 | 102_100 => fixed("WGS 84 / Pseudo-Mercator", WEB_MERCATOR),
        3395 => fixed(
            "WGS 84 / World Mercator",
            "+proj=merc +lon_0=0 +k=1 +x_0=0 +y_0=0 +datum=WGS84 +units=m +no_defs",
        ),
        32_601..=32_660 => Some((
            format!("WGS 84 / UTM zone {}N", code - 32_600),
            format!(
                "+proj=utm +zone={} +datum=WGS84 +units=m +no_defs",
                code - 32_600
            ),
        )),
        32_701..=32_760 => Some((
            format!("WGS 84 / UTM zone {}S", code - 32_700),
            format!(
                "+proj=utm +zone={} +south +datum=WGS84 +units=m +no_defs",
                code - 32_700
            ),
        )),
        25_828..=25_838 => Some((
            format!("ETRS89 / UTM zone {}N", code - 25_800),
            format!(
                "+proj=utm +zone={} +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
                code - 25_800
            ),
        )),
        26_901..=26_923 => Some((
            format!("NAD83 / UTM zone {}N", code - 26_900),
            format!(
                "+proj=utm +zone={} +datum=NAD83 +units=m +no_defs",
                code - 26_900
            ),
        )),
        3006 => fixed(
            "SWEREF99 TM",
            "+proj=utm +zone=33 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        ),
        3035 => fixed(
            "ETRS89-extended / LAEA Europe",
            "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 \
             +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        ),
        2154 => fixed(
            "RGF93 v1 / Lambert-93",
            "+proj=lcc +lat_0=46.5 +lon_0=3 +lat_1=49 +lat_2=44 +x_0=700000 +y_0=6600000 \
             +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        ),
        27_700 => fixed(
            "OSGB36 / British National Grid",
            "+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 \
             +ellps=airy +towgs84=446.448,-125.157,542.06,0.15,0.247,0.842,-20.489 +units=m \
             +no_defs",
        ),
        2056 => fixed(
            "CH1903+ / LV95",
            "+proj=somerc +lat_0=46.9524055555556 +lon_0=7.43958333333333 +k_0=1 \
             +x_0=2600000 +y_0=1200000 +ellps=bessel +towgs84=674.374,15.056,405.346,0,0,0,0 \
             +units=m +no_defs",
        ),
        28_992 => fixed(
            "Amersfoort / RD New",
            "+proj=sterea +lat_0=52.1561605555556 +lon_0=5.38763888888889 +k=0.9999079 \
             +x_0=155000 +y_0=463000 +ellps=bessel \
             +towgs84=565.417,50.3319,465.552,-0.398957,0.343988,-1.8774,4.0725 +units=m \
             +no_defs",
        ),
        _ => None,
    }
}

/// The EPSG code a CRS name stands for: `EPSG:4326`, `4326`,
/// `urn:ogc:def:crs:EPSG::4326`, `urn:ogc:def:crs:OGC:1.3:CRS84`.
#[must_use]
pub fn epsg_code(name: &str) -> Option<u32> {
    let name = name.trim();
    let upper = name.to_ascii_uppercase();
    if upper.ends_with("CRS84") || upper.ends_with("CRS:84") {
        return Some(4326);
    }
    let digits = upper
        .rsplit([':', '/'])
        .next()
        .filter(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))?;
    let epsg_like = upper.contains("EPSG") || upper.bytes().all(|b| b.is_ascii_digit());
    epsg_like.then(|| digits.parse().ok()).flatten()
}

/// The CRS a request or file names, resolved.
fn resolve(text: &str, source: CrsSource) -> Result<MapCrs, String> {
    let text = text.trim();
    if text.starts_with('+') {
        Proj::from_proj_string(text).map_err(|e| format!("PROJ string `{text}`: {e}"))?;
        return Ok(MapCrs {
            epsg: None,
            name: text.to_owned(),
            source,
            definition: text.to_owned(),
        });
    }
    let code = epsg_code(text).ok_or_else(|| {
        format!("`{text}` is neither an EPSG code (`EPSG:32632`) nor a PROJ string (`+proj=...`)")
    })?;
    let (name, definition) = epsg(code).ok_or_else(|| {
        format!(
            "EPSG:{code} is not in crest's bundled table; give its PROJ string as `crs` \
             (epsg.io lists it)"
        )
    })?;
    Ok(MapCrs {
        epsg: Some(code),
        name,
        source,
        definition,
    })
}

/// The legacy `crs` member's name, and a note when it cannot be used.
fn crs_member(members: Option<&JsonObject>) -> (Option<String>, Option<String>) {
    let Some(crs) = members.and_then(|m| m.get("crs")) else {
        return (None, None);
    };
    let kind = crs.get("type").and_then(Value::as_str).unwrap_or("");
    let name = crs
        .get("properties")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str);
    match (kind, name) {
        ("name", Some(name)) => (Some(name.to_owned()), None),
        _ => (
            None,
            Some(format!(
                "the `crs` member ({}) is not a named CRS; read as WGS 84",
                if kind.is_empty() { "no type" } else { kind }
            )),
        ),
    }
}

/// Reprojection from the source CRS to WGS 84 and on to Web Mercator.
struct Reproject {
    source: Option<Proj>,
    wgs84: Proj,
    mercator: Proj,
}

impl Reproject {
    fn new(crs: &MapCrs) -> Result<Self, String> {
        let parse = |s: &str| Proj::from_proj_string(s).map_err(|e| format!("PROJ `{s}`: {e}"));
        let wgs84 = parse(WGS84)?;
        let source = if crs.epsg == Some(4326) {
            None
        } else {
            Some(parse(&crs.definition)?)
        };
        Ok(Self {
            source,
            wgs84,
            mercator: parse(WEB_MERCATOR)?,
        })
    }

    /// Longitude and latitude (degrees) of a source position.
    fn lon_lat(&self, p: &Position) -> Result<(f64, f64), String> {
        let c = p.as_slice();
        let (x, y) = match c {
            [x, y, ..] => (*x, *y),
            _ => return Err("a position has fewer than 2 coordinates".into()),
        };
        if !(x.is_finite() && y.is_finite()) {
            return Err("a position is not finite".into());
        }
        let Some(source) = &self.source else {
            return Ok((x, y));
        };
        let mut point = if source.is_latlong() {
            (x.to_radians(), y.to_radians(), 0.0)
        } else {
            (x, y, 0.0)
        };
        proj4rs::transform::transform(source, &self.wgs84, &mut point)
            .map_err(|e| format!("cannot reproject ({x}, {y}): {e}"))?;
        Ok((point.0.to_degrees(), point.1.to_degrees()))
    }

    /// The Web Mercator point of a WGS 84 longitude/latitude.
    fn mercator(&self, (lon, lat): (f64, f64)) -> Result<Xy, String> {
        let lat = lat.clamp(-MAX_LATITUDE, MAX_LATITUDE);
        let mut point = (lon.to_radians(), lat.to_radians(), 0.0);
        proj4rs::transform::transform(&self.wgs84, &self.mercator, &mut point)
            .map_err(|e| format!("cannot project ({lon}, {lat}) to Web Mercator: {e}"))?;
        Ok([point.0, point.1])
    }
}

/// A feature's geometry gathered as lon/lat parts.
#[derive(Default)]
struct Parts {
    points: Vec<(f64, f64)>,
    lines: Vec<Vec<(f64, f64)>>,
    polygons: Vec<Vec<Vec<(f64, f64)>>>,
}

impl Parts {
    fn vertices(&self) -> usize {
        self.points.len()
            + self.lines.iter().map(Vec::len).sum::<usize>()
            + self.polygons.iter().flatten().map(Vec::len).sum::<usize>()
    }
}

fn kind_of(value: &GeometryValue) -> GeometryKind {
    match value {
        GeometryValue::Point { .. } => GeometryKind::Point,
        GeometryValue::MultiPoint { .. } => GeometryKind::MultiPoint,
        GeometryValue::LineString { .. } => GeometryKind::LineString,
        GeometryValue::MultiLineString { .. } => GeometryKind::MultiLineString,
        GeometryValue::Polygon { .. } => GeometryKind::Polygon,
        GeometryValue::MultiPolygon { .. } => GeometryKind::MultiPolygon,
        GeometryValue::GeometryCollection { .. } => GeometryKind::GeometryCollection,
    }
}

fn line(r: &Reproject, positions: &[Position]) -> Result<Vec<(f64, f64)>, String> {
    positions.iter().map(|p| r.lon_lat(p)).collect()
}

fn ring(r: &Reproject, positions: &[Position]) -> Result<Vec<(f64, f64)>, String> {
    let mut ring = line(r, positions)?;
    // RFC 7946 rings are closed; close one that is not.
    if let (Some(first), Some(last)) = (ring.first().copied(), ring.last().copied())
        && first != last
    {
        ring.push(first);
    }
    if ring.len() < 4 {
        return Err("a polygon ring has fewer than 4 positions".into());
    }
    Ok(ring)
}

fn gather(
    r: &Reproject,
    value: &GeometryValue,
    parts: &mut Parts,
    depth: usize,
) -> Result<(), String> {
    match value {
        GeometryValue::Point { coordinates } => parts.points.push(r.lon_lat(coordinates)?),
        GeometryValue::MultiPoint { coordinates } => {
            for p in coordinates {
                parts.points.push(r.lon_lat(p)?);
            }
        }
        GeometryValue::LineString { coordinates } => {
            if coordinates.len() >= 2 {
                parts.lines.push(line(r, coordinates)?);
            }
        }
        GeometryValue::MultiLineString { coordinates } => {
            for l in coordinates.iter().filter(|l| l.len() >= 2) {
                parts.lines.push(line(r, l)?);
            }
        }
        GeometryValue::Polygon { coordinates } => {
            let rings: Result<Vec<_>, _> = coordinates.iter().map(|c| ring(r, c)).collect();
            parts.polygons.push(rings?);
        }
        GeometryValue::MultiPolygon { coordinates } => {
            for polygon in coordinates {
                let rings: Result<Vec<_>, _> = polygon.iter().map(|c| ring(r, c)).collect();
                parts.polygons.push(rings?);
            }
        }
        GeometryValue::GeometryCollection { geometries } => {
            if depth > 8 {
                return Err("geometry collections nest more than 8 deep".into());
            }
            for g in geometries {
                gather(r, &g.value, parts, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// The geodesic area (m², holes out) and perimeter (m) of `rings`.
fn polygon_measures(rings: &[Vec<(f64, f64)>]) -> (f64, f64) {
    let mut iter = rings.iter().map(|ring| LineString::from(ring.clone()));
    let Some(exterior) = iter.next() else {
        return (0.0, 0.0);
    };
    let polygon = Polygon::new(exterior, iter.collect());
    let (perimeter, area) = polygon.geodesic_perimeter_area_unsigned();
    (area, perimeter)
}

fn bbox(parts: &Parts) -> Option<[f64; 4]> {
    let all = parts
        .points
        .iter()
        .chain(parts.lines.iter().flatten())
        .chain(parts.polygons.iter().flatten().flatten());
    all.fold(None, |acc, &(x, y)| match acc {
        None => Some([x, y, x, y]),
        Some([w, s, e, n]) => Some([w.min(x), s.min(y), e.max(x), n.max(y)]),
    })
}

fn id_text(feature: &Feature) -> Option<String> {
    feature.id.as_ref().map(|id| match id {
        geojson::feature::Id::String(s) => s.clone(),
        geojson::feature::Id::Number(n) => n.to_string(),
    })
}

/// What one feature became.
struct Measured {
    feature: MapFeature,
    vertices: usize,
}

fn measure(
    r: &Reproject,
    geometry: &Geometry,
    id: Option<String>,
    properties: Map<String, Value>,
) -> Result<Measured, String> {
    let mut parts = Parts::default();
    gather(r, &geometry.value, &mut parts, 0)?;
    let vertices = parts.vertices();
    let (mut area, mut length, mut perimeter) = (None, None, None);
    if !parts.lines.is_empty() {
        let total: f64 = parts
            .lines
            .iter()
            .map(|l| Geodesic.length(&LineString::from(l.clone())))
            .sum();
        length = Some(total);
    }
    if !parts.polygons.is_empty() {
        let (a, p) = parts
            .polygons
            .iter()
            .map(|rings| polygon_measures(rings))
            .fold((0.0, 0.0), |acc, m| (acc.0 + m.0, acc.1 + m.1));
        area = Some(a);
        perimeter = Some(p);
    }
    let to_xy = |pts: &[(f64, f64)]| -> Result<Vec<Xy>, String> {
        pts.iter().map(|p| r.mercator(*p)).collect()
    };
    let feature = MapFeature {
        id,
        kind: kind_of(&geometry.value),
        points: to_xy(&parts.points)?,
        lines: parts
            .lines
            .iter()
            .map(|l| to_xy(l))
            .collect::<Result<_, _>>()?,
        polygons: parts
            .polygons
            .iter()
            .map(|rings| rings.iter().map(|ring| to_xy(ring)).collect())
            .collect::<Result<_, _>>()?,
        properties,
        bbox: bbox(&parts),
        area_m2: area,
        length_m: length,
        perimeter_m: perimeter,
    };
    Ok(Measured { feature, vertices })
}

/// The map of GeoJSON `bytes`.
///
/// # Errors
///
/// A message for malformed GeoJSON, an unknown CRS or coordinates that do
/// not reproject.
pub fn parse(bytes: &[u8], request: &MapRequest) -> Result<MapSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "GeoJSON is not UTF-8".to_owned())?;
    let geojson: GeoJson = text.parse().map_err(|e| format!("not GeoJSON: {e}"))?;
    let (features, members): (Vec<Feature>, Option<JsonObject>) = match geojson {
        GeoJson::FeatureCollection(fc) => (fc.features, fc.foreign_members),
        GeoJson::Feature(f) => {
            let members = f.foreign_members.clone();
            (vec![f], members)
        }
        GeoJson::Geometry(g) => {
            let members = g.foreign_members.clone();
            (
                vec![Feature {
                    bbox: None,
                    geometry: Some(g),
                    id: None,
                    properties: None,
                    foreign_members: None,
                }],
                members,
            )
        }
    };
    let mut warnings = Vec::new();
    let (member, note) = crs_member(members.as_ref());
    warnings.extend(note);
    let crs = match (&request.crs, &member) {
        (Some(given), _) if !given.trim().is_empty() => resolve(given, CrsSource::Given)?,
        (_, Some(name)) => resolve(name, CrsSource::Member)?,
        _ => MapCrs {
            epsg: Some(4326),
            name: "WGS 84".into(),
            source: CrsSource::Default,
            definition: WGS84.into(),
        },
    };
    if let (Some(name), CrsSource::Given) = (&member, crs.source) {
        warnings.push(format!("the file's crs member ({name}) was overridden"));
    }
    let reproject = Reproject::new(&crs)?;
    let total_features = features.len();
    let mut out = Vec::new();
    let mut without_geometry = 0;
    let mut vertices = 0;
    let mut capped = false;
    for (ix, f) in features.iter().enumerate() {
        let Some(geometry) = &f.geometry else {
            without_geometry += 1;
            continue;
        };
        if out.len() >= MAX_FEATURES {
            capped = true;
            break;
        }
        let measured = measure(
            &reproject,
            geometry,
            id_text(f),
            f.properties.clone().unwrap_or_default(),
        )
        .map_err(|e| format!("feature {}: {e}", ix + 1))?;
        if vertices + measured.vertices > MAX_VERTICES {
            capped = true;
            break;
        }
        vertices += measured.vertices;
        out.push(measured.feature);
    }
    if capped {
        warnings.push(format!(
            "only the first {} features are shown (at most {MAX_FEATURES} features and \
             {MAX_VERTICES} vertices)",
            out.len()
        ));
    }
    if out.is_empty() {
        return Err("the GeoJSON has no feature with a geometry".into());
    }
    let color_by = request
        .color_by
        .clone()
        .filter(|k| out.iter().any(|f| f.properties.contains_key(k)));
    if let (Some(key), None) = (&request.color_by, &color_by) {
        warnings.push(format!("no feature has the property `{key}` to colour by"));
    }
    let measures = envelope(bytes, request, &crs, &out);
    Ok(MapSpec {
        title: None,
        path: request.path.clone(),
        crs,
        features: out,
        total_features,
        without_geometry,
        color_by,
        measures,
        warnings,
    })
}

fn value(v: f64, unit: &str, display: Option<String>) -> ComputedValue {
    let mut value = ComputedValue::new(v);
    value.unit = Some(unit.to_owned());
    value.display = display;
    value
}

fn envelope(bytes: &[u8], request: &MapRequest, crs: &MapCrs, features: &[MapFeature]) -> Computed {
    let mut computed = Computed::new(
        derive_id(bytes, crs),
        KIND,
        "Geodesic measures on the WGS 84 ellipsoid",
        crate::sci::engine(GEO_ENGINE_NAME, BACKEND),
    );
    computed.method.why = Some(
        "Areas and perimeters by Karney's (2013) geodesic polygon area (geo GeodesicArea, \
         unsigned, holes subtracted), line lengths as sums of geodesic distances (geo \
         Geodesic), boxes from the WGS 84 longitudes and latitudes; coordinates reprojected \
         to WGS 84 with proj4rs first"
            .into(),
    );
    let mut params = BTreeMap::new();
    params.insert("crs".to_owned(), Value::String(crs.label()));
    params.insert(
        "crs_definition".to_owned(),
        Value::String(crs.definition.clone()),
    );
    params.insert(
        "display".to_owned(),
        Value::String("EPSG:3857 (Web Mercator)".into()),
    );
    computed.method.params = params;
    computed.inputs = vec![chem::input(
        "geojson",
        "data",
        bytes,
        request.path.as_deref(),
    )];
    let v = &mut computed.values;
    #[allow(clippy::cast_precision_loss)]
    v.insert("features".into(), ComputedValue::new(features.len() as f64));
    let areas: Vec<f64> = features.iter().filter_map(|f| f.area_m2).collect();
    if !areas.is_empty() {
        let total: f64 = areas.iter().sum();
        v.insert("area_m2".into(), value(total, "m²", Some(area_text(total))));
    }
    let lengths: Vec<f64> = features.iter().filter_map(|f| f.length_m).collect();
    if !lengths.is_empty() {
        let total: f64 = lengths.iter().sum();
        v.insert(
            "length_m".into(),
            value(total, "m", Some(length_text(total))),
        );
    }
    let boxes = features.iter().filter_map(|f| f.bbox);
    if let Some([w, s, e, n]) = boxes.reduce(|a, b| {
        [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]
    }) {
        for (key, x) in [
            ("bbox.west", w),
            ("bbox.south", s),
            ("bbox.east", e),
            ("bbox.north", n),
        ] {
            let mut value = value(x, "°", None);
            value.decimals = Some(6);
            v.insert(key.into(), value);
        }
    }
    for (ix, f) in features.iter().enumerate().take(ENVELOPE_FEATURES) {
        if let Some(a) = f.area_m2 {
            v.insert(
                format!("feature.{}.area_m2", ix + 1),
                value(a, "m²", Some(area_text(a))),
            );
        }
        if let Some(l) = f.length_m {
            v.insert(
                format!("feature.{}.length_m", ix + 1),
                value(l, "m", Some(length_text(l))),
            );
        }
        if let Some(p) = f.perimeter_m {
            v.insert(
                format!("feature.{}.perimeter_m", ix + 1),
                value(p, "m", Some(length_text(p))),
            );
        }
    }
    computed
}

fn derive_id(bytes: &[u8], crs: &MapCrs) -> String {
    crate::sci::derive_id("ge", &[KIND.as_bytes(), crs.definition.as_bytes(), bytes])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epsg_names_are_read() {
        assert_eq!(epsg_code("EPSG:32632"), Some(32_632));
        assert_eq!(epsg_code("urn:ogc:def:crs:EPSG::3857"), Some(3857));
        assert_eq!(epsg_code("urn:ogc:def:crs:OGC:1.3:CRS84"), Some(4326));
        assert_eq!(epsg_code("4326"), Some(4326));
        assert_eq!(epsg_code("WGS84"), None);
        assert_eq!(
            epsg(32_633).map(|e| e.0).as_deref(),
            Some("WGS 84 / UTM zone 33N")
        );
        assert!(epsg(9999).is_none());
        assert!(
            resolve("EPSG:9999", CrsSource::Given)
                .unwrap_err()
                .contains("PROJ string")
        );
        assert!(resolve("+proj=nonsense", CrsSource::Given).is_err());
    }
}
