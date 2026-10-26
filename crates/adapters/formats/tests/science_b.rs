//! Science B viewers on fixture files (all generated for these tests):
//! GeoJSON maps, Gantt charts and HDF5 / NetCDF-3 files.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_formats::Formats;
use application::ports::formats::{
    ArrayRequest, FormatsPort, GanttRequest, GanttSource, GanttTaskInput, MapRequest,
};
use domain::gantt::Calendar;
use domain::science::arrays::{ArrayFormat, ArrayNodeKind, PreviewKind};
use domain::science::geomap::{CrsSource, GeometryKind, to_lon_lat};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/science/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn map(name: &str, crs: Option<&str>) -> Result<domain::science::geomap::MapSpec, String> {
    Formats::new()
        .parse_geojson(
            &fixture(name),
            &MapRequest {
                path: Some(format!("geo/{name}")),
                crs: crs.map(str::to_owned),
                color_by: Some("kind".into()),
            },
        )
        .map_err(|e| e.to_string())
}

#[test]
fn geojson_features_are_measured_geodesically() {
    let spec = map("parks.geojson", None).unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.crs.epsg, Some(4326));
    assert_eq!(spec.crs.source, CrsSource::Default);
    assert_eq!(spec.total_features, 8);
    assert_eq!(spec.without_geometry, 1);
    assert_eq!(spec.features.len(), 7);
    assert_eq!(spec.color_by.as_deref(), Some("kind"));
    // A 1° × 1° cell on the equator is 12 308.78 km² on WGS 84.
    let cell = spec.features[0].area_m2.unwrap();
    assert!((cell / 1e6 - 12_308.78).abs() < 0.05, "{cell}");
    // The hole is taken out: 0.01° squares at 50° N, the hole 4 % of it.
    let ring = &spec.features[1];
    assert_eq!(ring.id.as_deref(), Some("7"));
    assert_eq!(ring.polygons[0].len(), 2);
    let full = 0.01_f64.to_radians().powi(2) * 6_371_000f64.powi(2) * 50.005_f64.to_radians().cos();
    let area = ring.area_m2.unwrap();
    assert!(
        (area / (full * 0.96) - 1.0).abs() < 0.01,
        "{area} vs {full}"
    );
    // The unclosed pond ring was closed.
    assert_eq!(spec.features[2].kind, GeometryKind::MultiPolygon);
    assert_eq!(spec.features[2].polygons[1][0].len(), 5);
    // One degree along the equator: 111 319.49 m (a·π/180); this line is
    // at 0.5° S, a geodesic slightly shorter than the parallel.
    let trail = spec.features[3].length_m.unwrap();
    assert!((trail - 111_319.0).abs() < 30.0, "{trail}");
    // A meridian degree near the equator is 110 574 m; the walks are 1.5°.
    let walks = spec.features[4].length_m.unwrap();
    assert!((walks - 110_574.4 * 1.5).abs() < 20.0, "{walks}");
    assert!(spec.features[0].perimeter_m.unwrap() > 4.0 * 110_000.0);
    // Display coordinates are Web Mercator.
    let gate = spec.features[5].points[0];
    let (lon, lat) = to_lon_lat(gate);
    assert!((lon - 10.005).abs() < 1e-9 && (lat - 50.0).abs() < 1e-9);
    let m = &spec.measures;
    assert_eq!(m.values["features"].v, 7.0);
    assert!((m.values["bbox.west"].v - 0.0).abs() < 1e-12);
    assert!((m.values["bbox.north"].v - 50.01).abs() < 1e-9);
    assert!(m.values.contains_key("feature.2.area_m2"));
    assert!(m.method.why.as_deref().unwrap().contains("Karney"));
    assert_eq!(m.engine.name, "crest-geo");
}

#[test]
fn legacy_crs_member_and_given_crs_reproject() {
    let spec = map("utm_parcel.geojson", None).unwrap();
    assert_eq!(spec.crs.epsg, Some(32_632));
    assert_eq!(spec.crs.source, CrsSource::Member);
    assert_eq!(spec.crs.name, "WGS 84 / UTM zone 32N");
    // 100 m grid square on the central meridian: UTM's scale factor 0.9996
    // makes it 100.04 m on the ground each way.
    let area = spec.features[0].area_m2.unwrap();
    assert!((area - 10_008.0).abs() < 2.0, "{area}");
    let [w, s, ..] = spec.features[0].bbox.unwrap();
    assert!((w - 9.0).abs() < 1e-6, "central meridian of zone 32: {w}");
    assert!((s - 50.0).abs() < 0.05, "{s}");
    // A given CRS overrides the member, and says so.
    let given = map("utm_parcel.geojson", Some("EPSG:32633")).unwrap();
    assert_eq!(given.crs.source, CrsSource::Given);
    assert!(given.warnings.iter().any(|w| w.contains("overridden")));
    let [w, ..] = given.features[0].bbox.unwrap();
    assert!((w - 15.0).abs() < 1e-6, "{w}");
    // Unknown codes and broken files are refused with a hint.
    assert!(
        map("utm_parcel.geojson", Some("EPSG:9999"))
            .unwrap_err()
            .contains("PROJ string")
    );
    assert!(
        Formats::new()
            .parse_geojson(b"{\"type\": \"Nope\"}", &MapRequest::default())
            .is_err()
    );
}

fn gantt(name: &str, calendar: Calendar) -> Result<domain::gantt::GanttSpec, String> {
    Formats::new()
        .schedule_gantt(&GanttRequest {
            path: Some(format!("plan/{name}")),
            source: GanttSource::File(fixture(name)),
            start: None,
            calendar,
        })
        .map_err(|e| e.to_string())
}

#[test]
fn cpm_textbook_network() {
    let spec = gantt("project.csv", Calendar::AllDays).unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.project_duration, 14.0);
    assert_eq!(spec.critical_path, vec!["A", "B", "D", "F"]);
    let row = |id: &str| spec.tasks.iter().find(|t| t.id == id).unwrap();
    let times = |id: &str| {
        let t = row(id);
        (
            t.early_start,
            t.early_finish,
            t.late_start,
            t.late_finish,
            t.total_slack,
            t.free_slack,
        )
    };
    assert_eq!(times("A"), (0.0, 3.0, 0.0, 3.0, 0.0, 0.0));
    assert_eq!(times("C"), (3.0, 5.0, 7.0, 9.0, 4.0, 0.0));
    assert_eq!(times("E"), (5.0, 8.0, 9.0, 12.0, 4.0, 4.0));
    assert_eq!(times("F"), (12.0, 14.0, 12.0, 14.0, 0.0, 0.0));
    assert_eq!(row("B").group.as_deref(), Some("build"));
    assert_eq!(row("C").name.as_deref(), Some("Order steel"));
    assert!(spec.start.is_none(), "no dates: offsets only");
    let env = &spec.schedule;
    assert_eq!(env.values["project_duration"].v, 14.0);
    assert_eq!(env.values["E.total_slack"].v, 4.0);
    assert_eq!(env.values["critical_tasks"].v, 4.0);
    assert_eq!(env.engine.name, "crest-plan");
}

#[test]
fn cpm_with_lags_milestones_and_a_weekday_calendar() {
    let spec = gantt("release.json", Calendar::Weekdays).unwrap();
    spec.validate().unwrap();
    assert_eq!(
        spec.start.as_deref(),
        Some("2026-11-02"),
        "the earliest task start"
    );
    assert_eq!(spec.project_duration, 13.0);
    let row = |id: &str| spec.tasks.iter().find(|t| t.id == id).unwrap();
    assert_eq!(
        (row("build").early_start, row("build").early_finish),
        (2.0, 12.0)
    );
    // FF+1: test finishes a day after build, so it starts at 13 - 4.
    assert_eq!(
        (row("test").early_start, row("test").late_finish),
        (9.0, 13.0)
    );
    assert_eq!(
        (row("docs").total_slack, row("docs").free_slack),
        (5.0, 5.0)
    );
    assert!(row("release").milestone && row("release").critical);
    assert_eq!(
        spec.critical_path,
        vec!["design", "build", "test", "release"]
    );
    // 12 working days after Monday 2 November: Wednesday 18 November.
    assert_eq!(spec.finish_text(spec.project_duration), "2026-11-18");
    assert_eq!(
        spec.schedule.method.params["finish_date"],
        serde_json::json!("2026-11-18")
    );
}

#[test]
fn cycles_and_unknown_tasks_are_named() {
    let task = |id: &str, deps: &[&str]| GanttTaskInput {
        id: id.into(),
        duration: Some(1.0),
        depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
        ..GanttTaskInput::default()
    };
    let run = |tasks: Vec<GanttTaskInput>| {
        Formats::new()
            .schedule_gantt(&GanttRequest {
                path: None,
                source: GanttSource::Tasks(tasks),
                start: Some("2026-11-07".into()),
                calendar: Calendar::Weekdays,
            })
            .map_err(|e| e.to_string())
    };
    let err = run(vec![
        task("a", &[]),
        task("b", &["a", "d"]),
        task("c", &["b"]),
        task("d", &["c"]),
    ])
    .unwrap_err();
    assert!(
        err.contains("cycle") && err.contains("b → c → d → b"),
        "{err}"
    );
    assert!(
        run(vec![task("a", &["zz"])])
            .unwrap_err()
            .contains("unknown task")
    );
    // A Saturday start moves to Monday, with a note.
    let ok = run(vec![task("a", &[])]).unwrap();
    assert_eq!(ok.start.as_deref(), Some("2026-11-09"));
    assert!(ok.warnings[0].contains("not a working day"));
}

fn arrays(
    name: &str,
    variable: Option<&str>,
) -> Result<domain::science::arrays::ArrayFileSpec, String> {
    Formats::new()
        .inspect_array_file(
            &fixture(name),
            &ArrayRequest {
                path: format!("data/{name}"),
                variable: variable.map(str::to_owned),
            },
        )
        .map_err(|e| e.to_string())
}

#[test]
fn hdf5_structure_and_bounded_previews() {
    let spec = arrays("arrays.h5", None).unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, ArrayFormat::Hdf5);
    let paths: Vec<&str> = spec.nodes.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/",
            "/scalar",
            "/series",
            "/sim",
            "/sim/field",
            "/sim/stack"
        ]
    );
    let root = &spec.nodes[0];
    assert_eq!(root.attr("title"), Some("crest array fixture"));
    let field = spec.node("/sim/field").unwrap();
    assert_eq!(field.kind, ArrayNodeKind::Variable);
    assert_eq!(field.dtype.as_deref(), Some("f32"));
    assert_eq!(field.shape, vec![40, 30]);
    assert_eq!(field.chunks, Some(vec![10, 10]));
    assert_eq!(field.filters.len(), 2, "{:?}", field.filters);
    assert!(
        field.filters.iter().any(|f| f.starts_with("deflate")),
        "{:?}",
        field.filters
    );
    assert_eq!(field.attr("units"), Some("K"));
    // Previews: the 2D and 3D first, then the 1D, then the scalar.
    let order: Vec<&str> = spec.previews.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        order,
        vec!["/sim/field", "/sim/stack", "/series", "/scalar"]
    );
    let heat = &spec.previews[0];
    assert_eq!(
        (heat.kind, heat.rows, heat.cols),
        (PreviewKind::Heatmap, 40, 30)
    );
    assert_eq!(heat.at(3, 7), Some(307.0));
    assert_eq!(heat.selection, "[0:40, 0:30]");
    let st = &heat.stats.values;
    assert_eq!((st["n"].v, st["min"].v, st["max"].v), (1200.0, 0.0, 3929.0));
    // Mean of r*100 + c over 40 x 30: 19.5*100 + 14.5.
    assert!((st["mean"].v - 1964.5).abs() < 1e-9);
    assert!(
        heat.stats
            .method
            .why
            .as_deref()
            .unwrap()
            .contains("preview only")
    );
    let stack = &spec.previews[1];
    assert_eq!(stack.selection, "[0, 0:8, 0:6]");
    assert_eq!((stack.rows, stack.cols, stack.at(7, 5)), (8, 6, Some(75.0)));
    let series = &spec.previews[2];
    assert_eq!((series.kind, series.cols), (PreviewKind::Line, 2000));
    assert_eq!(series.values[100], Some(3.0));
    assert_eq!(spec.previews[3].values, vec![Some(2.5)]);
    // One variable on request, by name or path.
    let one = arrays("arrays.h5", Some("stack")).unwrap();
    assert_eq!(one.previews.len(), 1);
    assert!(
        arrays("arrays.h5", Some("nope"))
            .unwrap_err()
            .contains("/sim/field")
    );
}

#[test]
fn netcdf3_classic_with_records_and_cf_decoding() {
    let spec = arrays("grid.nc", None).unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, ArrayFormat::Netcdf3Classic);
    assert_eq!(spec.nodes[0].attr("Conventions"), Some("CF-1.8"));
    let temp = spec.node("/temp").unwrap();
    assert_eq!(temp.shape, vec![3, 4, 5], "3 records");
    assert_eq!(temp.dims, vec!["time", "lat", "lon"]);
    assert_eq!(temp.dtype.as_deref(), Some("i16"));
    assert_eq!(temp.shape_text(), "time=3 × lat=4 × lon=5");
    let time = spec.node("/time").unwrap();
    assert_eq!(time.attr("units"), Some("hours since 2026-01-01"));
    let p = spec.previews.iter().find(|p| p.path == "/temp").unwrap();
    assert_eq!(p.selection, "[0, 0:4, 0:5]");
    // stored × 0.5 + 250; the fill value is missing.
    assert_eq!(p.at(0, 0), Some(250.0));
    assert_eq!(p.at(3, 4), Some(250.0 + 34.0 * 0.5));
    assert_eq!(p.at(1, 2), None);
    assert_eq!(p.stats.values["missing"].v, 1.0);
    assert_eq!(p.stats.values["n"].v, 19.0);
    assert!(p.decoding.as_deref().unwrap().contains("scale_factor"));
    // A record variable's later rows come from their own records.
    let times = spec.previews.iter().find(|p| p.path == "/time").unwrap();
    assert_eq!(times.values, vec![Some(0.0), Some(6.0), Some(12.0)]);
    let lat = spec.previews.iter().find(|p| p.path == "/lat").unwrap();
    assert_eq!(
        lat.values,
        vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0)]
    );
    assert!(
        Formats::new()
            .inspect_array_file(b"not an array file", &ArrayRequest::default())
            .is_err()
    );
}
