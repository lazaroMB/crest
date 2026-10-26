//! Fixed answers of [`super::FakeFormats`] for the science B viewers
//! (maps, Gantt charts, array files, networks, training curves).

use domain::computed::{Computed, ComputedValue, Engine};
use domain::gantt::{Dependency, DependencyKind, GanttSpec, GanttTask};
use domain::science::arrays::{
    ArrayFileSpec, ArrayFormat, ArrayNode, ArrayNodeKind, ArrayPreview, PreviewKind,
};
use domain::science::geomap::{CrsSource, GeometryKind, MapCrs, MapFeature, MapSpec};
use serde_json::{Map, Value, json};

use crate::ports::formats::{ArrayRequest, GanttRequest, MapRequest};

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

/// `design` (3 d) then `build` (2 d), both critical, and `docs` (1 d)
/// after `design` with 1 d of slack.
pub(super) fn gantt(request: &GanttRequest) -> GanttSpec {
    let task = |id: &str, d: f64, es: f64, slack: f64, after: Option<&str>| GanttTask {
        id: id.into(),
        name: None,
        group: None,
        duration: d,
        milestone: false,
        depends_on: after
            .map(|a| Dependency {
                task: a.into(),
                kind: DependencyKind::Fs,
                lag: 0.0,
            })
            .into_iter()
            .collect(),
        not_before: None,
        early_start: es,
        early_finish: es + d,
        late_start: es + slack,
        late_finish: es + d + slack,
        total_slack: slack,
        free_slack: slack,
        critical: slack == 0.0,
    };
    let mut schedule = Computed::new(
        "pl_fake",
        "plan.cpm",
        "Critical path method",
        engine("crest-plan"),
    );
    let mut duration = ComputedValue::new(5.0);
    duration.unit = Some("d".into());
    schedule.values.insert("project_duration".into(), duration);
    GanttSpec {
        title: None,
        path: request.path.clone(),
        start: request.start.clone(),
        calendar: request.calendar,
        tasks: vec![
            task("design", 3.0, 0.0, 0.0, None),
            task("build", 2.0, 3.0, 0.0, Some("design")),
            task("docs", 1.0, 3.0, 1.0, Some("design")),
        ],
        project_duration: 5.0,
        critical_path: vec!["design".into(), "build".into()],
        schedule,
        warnings: Vec::new(),
    }
}

/// HDF5 with `/temp` (2 × 3 f32) and a full preview of it, 1 to 6 with
/// the third value missing.
pub(super) fn arrays(request: &ArrayRequest) -> ArrayFileSpec {
    let mut stats = Computed::new(
        "ar_fake",
        "array.preview_stats",
        "Descriptive statistics of the preview",
        engine("crest-array"),
    );
    for (key, v) in [
        ("n", 5.0),
        ("missing", 1.0),
        ("min", 1.0),
        ("max", 6.0),
        ("mean", 3.6),
    ] {
        stats.values.insert(key.into(), ComputedValue::new(v));
    }
    let node = |path: &str, kind, shape: Vec<u64>| ArrayNode {
        path: path.into(),
        depth: u32::from(path != "/"),
        kind,
        dtype: (kind == ArrayNodeKind::Variable).then(|| "f32".into()),
        shape,
        dims: Vec::new(),
        chunks: None,
        filters: Vec::new(),
        attrs: Vec::new(),
    };
    ArrayFileSpec {
        title: None,
        path: request.path.clone(),
        format: ArrayFormat::Hdf5,
        size: 4096,
        nodes: vec![
            node("/", ArrayNodeKind::Group, Vec::new()),
            node("/temp", ArrayNodeKind::Variable, vec![2, 3]),
        ],
        truncated: false,
        previews: vec![ArrayPreview {
            path: "/temp".into(),
            kind: PreviewKind::Heatmap,
            selection: "[0:2, 0:3]".into(),
            rows: 2,
            cols: 3,
            values: vec![Some(1.0), Some(2.0), None, Some(4.0), Some(5.0), Some(6.0)],
            texts: Vec::new(),
            decoding: None,
            stats,
        }],
        warnings: Vec::new(),
    }
}
