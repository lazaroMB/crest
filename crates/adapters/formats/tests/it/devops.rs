//! Traces, profiles and logs on fixture files of real tools' dialects.

use adapter_formats::Formats;
use application::ports::formats::{FormatsPort, LogRequest, ProfileRequest};
use domain::devops::logs::LogFormat;
use domain::devops::trace::{KIND, SERVICE, STATUS_MESSAGE, TraceFormat};
use domain::widget::timeline::SpanStatus;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/devops/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn otlp_json_becomes_a_tree_with_services_status_and_events() {
    let spec = Formats
        .parse_trace(&fixture("otlp.json"), "traces/checkout.json")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.format, TraceFormat::Otlp);
    assert_eq!((spec.traces, spec.total_spans), (1, 4));
    assert_eq!(spec.services, ["frontend", "payments"]);
    assert_eq!(spec.started_at, Some(1_792_847_107.0));
    let rows: Vec<_> = spec
        .timeline
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.depth, r.spans[0].start, r.spans[0].end))
        .collect();
    assert_eq!(
        rows,
        [
            ("GET /checkout", 0, 0.0, 250.0),
            ("charge", 1, 10.0, 190.0),
            ("SELECT cards", 2, 20.0, 120.0),
            ("render", 1, 200.0, 240.0),
        ]
    );
    let charge = &spec.timeline.rows[1].spans[0];
    assert_eq!(charge.status, Some(SpanStatus::Error));
    assert_eq!(charge.attrs[SERVICE], "payments");
    assert_eq!(charge.attrs[KIND], "client");
    assert_eq!(charge.attrs[STATUS_MESSAGE], "card declined");
    assert_eq!(charge.attrs["retry"], true);
    assert_eq!(charge.attrs["amount"], 12.5);
    assert_eq!(charge.attrs["tags"], serde_json::json!(["a", 2]));
    assert_eq!(
        spec.timeline.rows[0].spans[0].attrs["http.status_code"],
        500
    );
    assert_eq!(spec.timeline.rows[3].spans[0].attrs[KIND], "internal");
    let marker = &spec.timeline.rows[1].markers[0];
    assert_eq!(
        (marker.at, marker.label.as_deref()),
        (150.0, Some("exception"))
    );
    assert_eq!(
        spec.critical_path,
        [
            "eee19b7ec3c1b174",
            "bbb19b7ec3c1b176",
            "ccc19b7ec3c1b177",
            "aaa19b7ec3c1b175"
        ]
    );
    assert_eq!(spec.errors(), [0, 1]);
}

#[test]
fn otlp_json_lines_merge_requests() {
    let spec = Formats
        .parse_trace(&fixture("otlp-lines.jsonl"), "otel.jsonl")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((spec.traces, spec.total_spans), (2, 5));
    assert_eq!(spec.services, ["frontend", "payments", "late"]);
    // Two traces: rows are banded by trace.
    assert_eq!(spec.timeline.rows[0].group.as_deref(), Some("5b8efff7"));
    assert_eq!(spec.timeline.rows[4].label, "batch");
    assert_eq!(spec.timeline.rows[4].spans[0].start, 1_000.0);
}

#[test]
fn jaeger_json_uses_references_processes_and_error_tags() {
    let spec = Formats
        .parse_trace(&fixture("jaeger.json"), "jaeger.json")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.format, TraceFormat::Jaeger);
    assert_eq!(spec.services, ["api", "db"]);
    let [root, query] = &spec.timeline.rows[..] else {
        panic!("{:?}", spec.timeline.rows);
    };
    assert_eq!((root.label.as_str(), root.depth), ("HTTP GET /users", 0));
    assert_eq!((query.label.as_str(), query.depth), ("query", 1));
    assert_eq!((query.spans[0].start, query.spans[0].end), (1.0, 5.0));
    assert_eq!(query.spans[0].status, Some(SpanStatus::Error));
    assert_eq!(query.spans[0].attrs[KIND], "client");
    assert_eq!(query.markers[0].label.as_deref(), Some("timeout"));
    assert_eq!(query.markers[0].at, 3.0);
}

#[test]
fn zipkin_v2_reads_parents_errors_and_annotations() {
    let spec = Formats
        .parse_trace(&fixture("zipkin.json"), "zipkin.json")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.format, TraceFormat::Zipkin);
    assert_eq!(spec.traces, 2);
    let labels: Vec<_> = spec
        .timeline
        .rows
        .iter()
        .map(|r| (r.label.as_str(), r.depth))
        .collect();
    assert_eq!(labels, [("get /", 0), ("get /api", 1), ("cron", 0)]);
    let api = &spec.timeline.rows[1].spans[0];
    assert_eq!(api.status, Some(SpanStatus::Error));
    assert_eq!(api.attrs[STATUS_MESSAGE], "connection reset");
    assert_eq!(spec.timeline.rows[1].markers[0].at, 10.0);
    // The longest root's path.
    assert_eq!(spec.critical_path, ["a", "b"]);
}

#[test]
fn bad_trace_files_say_why() {
    let err = |bytes: &[u8]| {
        Formats
            .parse_trace(bytes, "t.json")
            .unwrap_err()
            .to_string()
    };
    assert!(err(b"hello").contains("not JSON"), "{}", err(b"hello"));
    assert!(err(b"{\"x\": 1}").contains("resourceSpans"));
    assert!(err(b"[]").contains("no spans"));
    assert!(err(b"[1, 2]").contains("neither Zipkin"));
}

#[test]
fn folded_profile_has_hot_frames() {
    let spec = Formats
        .parse_profile(
            &fixture("cpu.folded"),
            &ProfileRequest {
                path: "cpu.folded".into(),
                sample_type: None,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.total(), 1_000);
    assert_eq!(spec.unit, "samples");
    let stats = spec.frame_stats();
    assert_eq!((stats[0].name.as_str(), stats[0].self_value), ("add", 500));
    assert_eq!((stats[1].name.as_str(), stats[1].self_value), ("lex", 300));
    let hot: Vec<_> = spec
        .hottest_path()
        .into_iter()
        .map(|ix| spec.nodes[ix].name.as_str())
        .collect();
    assert_eq!(hot, ["main", "eval", "eval", "add"]);
    assert!(
        Formats
            .parse_profile(
                b"just some text\n",
                &ProfileRequest {
                    path: "x".into(),
                    sample_type: None
                }
            )
            .is_err()
    );
}

fn logs(name: &str) -> domain::devops::logs::LogSpec {
    Formats
        .parse_logs(
            &fixture(name),
            &LogRequest {
                path: name.into(),
                format: None,
                truncated: false,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn json_lines_logs_have_levels_facets_and_a_histogram() {
    let spec = logs("app.jsonl");
    spec.validate().unwrap();
    assert_eq!(spec.format, LogFormat::Json);
    assert_eq!(spec.total_lines, 8);
    assert_eq!(spec.list.facets, ["service"]);
    assert_eq!(spec.list.columns, ["service"]);
    assert_eq!(spec.errors(), [3, 4, 7]);
    assert_eq!(
        spec.error_patterns()[0],
        ("connect to <n>:<n> failed after <n> tries".into(), 2, 3)
    );
    let histogram = spec.histogram.unwrap();
    assert_eq!(histogram.width, 1.0);
    assert_eq!(histogram.buckets.len(), 8);
    assert_eq!(histogram.buckets[3].errors, 1);
    assert_eq!(histogram.buckets[2].warnings, 1);
}

#[test]
fn logfmt_logs_read_quoted_messages() {
    let spec = logs("app.logfmt");
    assert_eq!(spec.format, LogFormat::Logfmt);
    assert_eq!(spec.list.items[2].text, "query failed");
    assert_eq!(spec.list.items[2].fields["err"], "pq: deadlock detected");
    assert_eq!(spec.list.facets, ["component"]);
    assert_eq!(spec.level_counts()[0], ("error".into(), 1));
}

#[test]
fn text_logs_fold_stack_traces_into_the_record() {
    let spec = logs("app.log");
    assert_eq!(spec.format, LogFormat::Text);
    assert_eq!(spec.total_lines, 4);
    let failed = &spec.list.items[2];
    assert_eq!(failed.level.as_deref(), Some("error"));
    assert_eq!(failed.ts.as_deref(), Some("2026-10-24 13:05:02,500"));
    assert_eq!(failed.text, "[pool-1] app.Db - query failed");
    assert_eq!(
        failed.fields["more.1"],
        "java.sql.SQLException: deadlock detected"
    );
    assert_eq!(failed.fields.len(), 4);
    assert!(spec.histogram.is_some());
}
