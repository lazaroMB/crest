//! The formats adapter on fixture files of real tools' dialects.

use adapter_formats::Formats;
use application::ports::formats::FormatsPort;
use domain::block::SourceRef;
use domain::report::{FindingSeverity, TestOutcome};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn pytest_junit_has_outcomes_messages_details_and_locations() {
    let report = Formats
        .parse_test_report(&fixture("pytest.xml"), "out/pytest.xml")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(report.title.as_deref(), Some("pytest tests"));
    assert_eq!(report.suites.len(), 1);
    let suite = &report.suites[0];
    assert_eq!(
        (suite.name.as_str(), suite.time_secs),
        ("pytest", Some(1.25))
    );
    let totals = report.totals();
    assert_eq!(
        (totals.passed, totals.failed, totals.skipped, totals.errored),
        (1, 1, 1, 1)
    );
    let [add, div, slow, open] = &suite.cases[..] else {
        panic!("four cases: {:?}", suite.cases);
    };
    assert_eq!(add.outcome, TestOutcome::Passed);
    assert_eq!(add.details, None);
    assert_eq!(
        add.location,
        Some(SourceRef::File {
            path: "tests/test_math.py".into(),
            line_start: Some(3),
            line_end: None,
        })
    );
    assert_eq!(div.outcome, TestOutcome::Failed);
    assert_eq!(div.message.as_deref(), Some("assert 1 == 2"));
    let details = div.details.as_deref().unwrap_or_default();
    assert!(
        details.starts_with("def test_div():\n>       assert 1 == 2"),
        "{details}"
    );
    assert!(
        details.ends_with("--- stdout ---\ndividing & checking"),
        "{details}"
    );
    assert_eq!(div.time_secs, Some(0.02));
    assert_eq!(div.classname.as_deref(), Some("tests.test_math"));
    assert_eq!(slow.outcome, TestOutcome::Skipped);
    assert_eq!(slow.message.as_deref(), Some("needs network"));
    assert_eq!(open.outcome, TestOutcome::Errored);
    // No message attribute: the type names it.
    assert_eq!(open.message.as_deref(), Some("FileNotFoundError"));
    assert_eq!(open.details.as_deref(), Some("fixture 'tmp' not found"));
}

#[test]
fn surefire_single_suite_with_cdata_and_separators() {
    let report = Formats
        .parse_test_report(&fixture("surefire.xml"), "target/TEST-AppTest.xml")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(report.title.as_deref(), Some("TEST-AppTest.xml"));
    let suite = &report.suites[0];
    assert_eq!(suite.time_secs, Some(1234.5));
    let breaks = &suite.cases[1];
    assert_eq!(breaks.outcome, TestOutcome::Failed);
    assert_eq!(
        breaks.message.as_deref(),
        Some("org.opentest4j.AssertionFailedError")
    );
    let details = breaks.details.as_deref().unwrap_or_default();
    assert!(
        details.starts_with("expected: <1> but was: <2>\n\tat com.example"),
        "{details}"
    );
    assert!(
        details.ends_with("--- stderr ---\nwarning: deprecated"),
        "{details}"
    );
}

#[test]
fn nested_suites_are_flattened_in_order() {
    let report = Formats
        .parse_test_report(&fixture("nested.xml"), "nested.xml")
        .unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<_> = report.suites.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["outer", "outer / inner"]);
    assert_eq!(report.suites[0].cases[0].name, "b");
    assert_eq!(report.suites[0].cases[0].outcome, TestOutcome::Failed);
    assert_eq!(report.suites[1].cases[0].name, "a");
}

#[test]
fn bad_junit_is_an_error() {
    let not_xml = Formats.parse_test_report(b"<testsuite><testcase", "a.xml");
    assert!(not_xml.is_err());
    let other = Formats.parse_test_report(b"<html><body/></html>", "a.xml");
    let message = other.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("not a JUnit report"), "{message}");
}

#[test]
fn sarif_findings_resolve_rules_levels_and_paths() {
    let spec = Formats
        .parse_sarif(&fixture("clippy.sarif"), "clippy.sarif")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(spec.tool.as_deref(), Some("clippy 0.1.95"));
    let [unwrap, ret, pass] = &spec.findings[..] else {
        panic!("three findings: {:?}", spec.findings);
    };
    assert_eq!(unwrap.rule.as_deref(), Some("clippy::unwrap_used"));
    // The rule's default level.
    assert_eq!(unwrap.severity, FindingSeverity::Error);
    assert_eq!(unwrap.message, "used `unwrap()` on a `Result` value");
    assert_eq!(
        unwrap.location,
        Some(SourceRef::File {
            path: "/home/user/proj/src/main.rs".into(),
            line_start: Some(7),
            line_end: None,
        })
    );
    assert_eq!(unwrap.snippet.as_deref(), Some("let x = f().unwrap();"));
    assert!(
        unwrap
            .help_uri
            .as_deref()
            .unwrap_or_default()
            .ends_with("#unwrap_used")
    );
    assert_eq!(ret.rule.as_deref(), Some("clippy::needless_return"));
    assert_eq!(ret.severity, FindingSeverity::Note);
    assert_eq!(ret.message, "unneeded `return` in `main`");
    assert_eq!(
        ret.location,
        Some(SourceRef::File {
            path: "/home/user/proj/src/my lib.rs".into(),
            line_start: Some(3),
            line_end: Some(5),
        })
    );
    assert_eq!(pass.severity, FindingSeverity::None);
    assert_eq!(pass.location, None);
}

#[test]
fn bad_sarif_is_an_error() {
    assert!(Formats.parse_sarif(b"{", "a.sarif").is_err());
    let message = Formats
        .parse_sarif(b"{\"version\": \"2.1.0\"}", "a.sarif")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(message.contains("no `runs`"), "{message}");
}

#[test]
fn wavedrom_renders_json5_wavejson_to_svg() {
    let source = String::from_utf8(fixture("wave.json5")).unwrap_or_default();
    let svg = Formats
        .render_wavedrom(&source)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(svg.starts_with("<svg"), "{}", &svg[..svg.len().min(200)]);
    assert!(svg.contains("clk") && svg.contains("head"), "labels drawn");
    assert!(
        Formats
            .render_wavedrom("{ signal: [ { wave: 3 } ] }")
            .is_err()
    );
    assert!(Formats.render_wavedrom("not json").is_err());
}
