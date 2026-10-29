//! HTTP archives on a DevTools-style fixture.

use adapter_formats::Formats;
use application::ports::formats::FormatsPort;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/devops/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn a_devtools_export_becomes_sorted_requests_with_phases() {
    let spec = Formats
        .parse_har(&fixture("checkout.har"), "net/checkout.har")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.total_entries, 4);
    assert_eq!(spec.creator.as_deref(), Some("WebInspector 537.36"));
    assert_eq!(spec.started_at, Some(1_792_847_107.0));
    let order: Vec<_> = spec
        .entries
        .iter()
        .map(|e| (e.method.as_str(), e.host(), e.status, e.start.round()))
        .collect();
    assert_eq!(
        order,
        [
            ("GET", "shop.test", 200, 0.0),
            ("GET", "cdn.shop.test", 200, 150.0),
            ("POST", "api.shop.test", 503, 600.0),
            ("GET", "ads.tracker.test", 0, 700.0),
        ]
    );
    let doc = &spec.entries[0];
    // connect (40) included ssl (25).
    assert_eq!(doc.timings.connect, Some(15.0));
    assert_eq!(doc.timings.ssl, Some(25.0));
    assert_eq!(doc.transfer_size, Some(2120));
    assert_eq!(doc.content_size, Some(5120));
    assert_eq!(doc.resource_type, "document");
    assert!(doc.has_credentials());
    let script = &spec.entries[1];
    assert_eq!(script.timings.dns, None);
    assert_eq!(script.transfer_size, Some(35_210));
    let body = script.response_body.clone().unwrap_or_default();
    assert!(body.base64 && body.text.is_none());
    let pay = &spec.entries[2];
    assert_eq!(
        pay.request_body.as_ref().and_then(|b| b.text.as_deref()),
        Some("{\"amount\": 42}")
    );
    assert!(pay.failed());
    let blocked = &spec.entries[3];
    assert_eq!(blocked.error.as_deref(), Some("net::ERR_BLOCKED_BY_CLIENT"));
    assert_eq!(blocked.status_class(), "failed");
    assert_eq!(spec.pages[0].on_load, Some(980.0));
    let timeline = spec.timeline();
    timeline.validate().unwrap();
    let markers: Vec<_> = timeline.rows[0].markers.iter().map(|m| m.at).collect();
    assert_eq!(markers, [420.5, 980.0]);
    assert_eq!(spec.failed(), [2, 3]);
    assert_eq!(
        spec.list().items[0].text,
        "GET https://shop.test/checkout?session_id=<redacted>&step=2"
    );
}

#[test]
fn files_that_are_not_har_are_errors() {
    let err = Formats
        .parse_har(b"{\"entries\": []}", "x.har")
        .unwrap_err();
    assert!(err.to_string().contains("not a HAR file"), "{err}");
    let empty = Formats
        .parse_har(br#"{"log": {"entries": []}}"#, "e.har")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(empty.entries.len(), 0);
}
