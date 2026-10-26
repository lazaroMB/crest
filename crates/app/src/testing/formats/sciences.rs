//! Fixed answers of [`super::FakeFormats`] for the science B viewers
//! (maps, Gantt charts, array files, networks, training curves).

use domain::computed::{Computed, ComputedValue, Engine};
use domain::science::geomap::{CrsSource, GeometryKind, MapCrs, MapFeature, MapSpec};
use serde_json::{Map, Value, json};

use crate::ports::formats::MapRequest;

fn engine(name: &str) -> Engine {
    Engine {
        name: name.into(),
        version: "0.0.0".into(),
        backend: None,
        platform: None,
    }
}

fn props(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// One park polygon (12.5 ha) and one 1.2 km trail in WGS 84.
pub(super) fn map(request: &MapRequest) -> MapSpec {
    let mut measures = Computed::new(
        "ge_fake",
        "geo.measures",
        "Geodesic measures on the WGS 84 ellipsoid",
        engine("crest-geo"),
    );
    for (key, v) in [
        ("features", 2.0),
        ("area_m2", 125_000.0),
        ("length_m", 1_200.0),
        ("bbox.west", 13.40),
        ("bbox.south", 52.50),
        ("bbox.east", 13.42),
        ("bbox.north", 52.52),
    ] {
        measures.values.insert(key.into(), ComputedValue::new(v));
    }
    let ring = vec![
        [1_491_000.0, 6_891_000.0],
        [1_492_000.0, 6_891_000.0],
        [1_492_000.0, 6_892_000.0],
        [1_491_000.0, 6_891_000.0],
    ];
    MapSpec {
        title: None,
        path: request.path.clone(),
        crs: MapCrs {
            epsg: Some(4326),
            name: "WGS 84".into(),
            source: CrsSource::Default,
            definition: "+proj=longlat +datum=WGS84 +no_defs".into(),
        },
        features: vec![
            MapFeature {
                id: Some("park".into()),
                kind: GeometryKind::Polygon,
                points: Vec::new(),
                lines: Vec::new(),
                polygons: vec![vec![ring]],
                properties: props(json!({"name": "Tiergarten", "kind": "park"})),
                bbox: Some([13.40, 52.50, 13.41, 52.51]),
                area_m2: Some(125_000.0),
                length_m: None,
                perimeter_m: Some(1_500.0),
            },
            MapFeature {
                id: None,
                kind: GeometryKind::LineString,
                points: Vec::new(),
                lines: vec![vec![[1_491_000.0, 6_893_000.0], [1_492_200.0, 6_893_000.0]]],
                polygons: Vec::new(),
                properties: props(json!({"name": "Trail", "kind": "path"})),
                bbox: Some([13.40, 52.51, 13.42, 52.52]),
                area_m2: None,
                length_m: Some(1_200.0),
                perimeter_m: None,
            },
        ],
        total_features: 2,
        without_geometry: 0,
        color_by: request.color_by.clone(),
        measures,
        warnings: Vec::new(),
    }
}
