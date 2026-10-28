//! The JSON event stream of Rust's test harness: `cargo test -- -Z
//! unstable-options --format json` (libtest) and `cargo nextest run
//! --message-format libtest-json`, one JSON object per line.
//!
//! `suite` events open and close a suite (one per test binary); `test`
//! events with `ok`, `failed`, `ignored` or `timeout` become cases, their
//! `exec_time` (seconds, as a number or `"0.01s"`) the time. nextest
//! writes names as `binary$test`: the binary names the suite. A failed
//! case takes its message and location from the panic in its `stdout`
//! (`panicked at src/lib.rs:10:5:` then the message, or the older
//! `panicked at 'message', src/lib.rs:10:5`), and keeps the output as its
//! details. Lines that are not JSON objects (cargo's own output) are
//! skipped.

use domain::block::SourceRef;
use domain::report::{TestCase, TestOutcome, TestReportSpec, TestSuite};
use serde_json::Value;

use crate::junit::MAX_DETAILS;

/// Whether `bytes` look like a JSON event stream rather than XML: the
/// first non-blank character opens an object.
#[must_use]
pub fn is_json_stream(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'{')
}

/// Parses `bytes`; `path` names the file in errors and the title.
///
/// # Errors
///
/// Text that is not UTF-8, or no test event at all.
pub fn parse(bytes: &[u8], path: &str) -> Result<TestReportSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))?;
    let mut suites: Vec<TestSuite> = Vec::new();
    // The suite libtest events go to: the last one a `suite started` opened.
    let mut current: Option<usize> = None;
    let mut any = false;
    for line in text.lines() {
        let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let field = |key: &str| event.get(key).and_then(Value::as_str);
        match (field("type"), field("event")) {
            (Some("suite"), Some("started")) => {
                suites.push(TestSuite {
                    name: String::new(),
                    time_secs: None,
                    cases: Vec::new(),
                });
                current = Some(suites.len() - 1);
            }
            (Some("suite"), Some(_)) => {
                if let Some(ix) = current {
                    suites[ix].time_secs = event.get("exec_time").and_then(seconds);
                }
                current = None;
            }
            (Some("test"), Some(outcome)) => {
                let outcome = match outcome {
                    "ok" => TestOutcome::Passed,
                    "failed" => TestOutcome::Failed,
                    "ignored" => TestOutcome::Skipped,
                    "timeout" => TestOutcome::Errored,
                    _ => continue,
                };
                any = true;
                let full = field("name").unwrap_or("?");
                let (binary, name) = match full.split_once('$') {
                    Some((binary, name)) => (Some(binary), name),
                    None => (None, full),
                };
                let mut case = TestCase {
                    name: name.to_owned(),
                    classname: None,
                    outcome,
                    time_secs: event.get("exec_time").and_then(seconds),
                    message: field("message").map(str::to_owned),
                    details: None,
                    location: None,
                };
                if outcome != TestOutcome::Passed {
                    let output = field("stdout").unwrap_or_default();
                    if let Some((message, location)) = panic(output) {
                        if case.message.is_none() {
                            case.message = message;
                        }
                        case.location = location;
                    }
                    if outcome == TestOutcome::Errored && case.message.is_none() {
                        case.message = Some("timed out".into());
                    }
                    let output = output.trim_end();
                    if !output.trim().is_empty() {
                        case.details = Some(cut(output));
                    }
                }
                let ix = match (binary, current) {
                    (Some(binary), _) => suite_named(&mut suites, binary),
                    (None, Some(ix)) => ix,
                    (None, None) => suite_named(&mut suites, "tests"),
                };
                suites[ix].cases.push(case);
            }
            _ => {}
        }
    }
    if !any {
        return Err(format!(
            "`{path}` has no test results: expected the JSON lines of `cargo test -- -Z \
             unstable-options --format json` or `cargo nextest run --message-format libtest-json`"
        ));
    }
    suites.retain(|s| !s.cases.is_empty());
    let unnamed = suites.iter().filter(|s| s.name.is_empty()).count();
    let mut n = 0;
    for suite in suites.iter_mut().filter(|s| s.name.is_empty()) {
        n += 1;
        suite.name = if unnamed == 1 {
            "tests".to_owned()
        } else {
            format!("test binary {n}")
        };
    }
    Ok(TestReportSpec {
        title: path.rsplit(['/', '\\']).next().map(str::to_owned),
        suites,
    })
}

/// The index of the suite called `name`, added when missing.
fn suite_named(suites: &mut Vec<TestSuite>, name: &str) -> usize {
    if let Some(ix) = suites.iter().position(|s| s.name == name) {
        return ix;
    }
    suites.push(TestSuite {
        name: name.to_owned(),
        time_secs: None,
        cases: Vec::new(),
    });
    suites.len() - 1
}

/// Seconds from `exec_time`: a number or a string like `0.25s`.
fn seconds(value: &Value) -> Option<f64> {
    let secs = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().trim_end_matches('s').parse().ok()?,
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

/// The message and location of the first panic in `output`.
fn panic(output: &str) -> Option<(Option<String>, Option<SourceRef>)> {
    let mut lines = output.lines();
    while let Some(line) = lines.next() {
        let Some((_, rest)) = line.split_once("panicked at ") else {
            continue;
        };
        // Older harnesses: `panicked at 'message', src/lib.rs:3:5`.
        if let Some(quoted) = rest.strip_prefix('\'')
            && let Some((message, at)) = quoted.rsplit_once("', ")
        {
            return Some((Some(message.to_owned()), location(at)));
        }
        // Since Rust 1.73: `panicked at src/lib.rs:3:5:` then the message.
        let at = rest.trim_end().trim_end_matches(':');
        let message = lines
            .by_ref()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_owned);
        return Some((message, location(at)));
    }
    None
}

/// `src/lib.rs:3:5` as a file location at line 3.
fn location(at: &str) -> Option<SourceRef> {
    let mut parts = at.trim().rsplitn(3, ':');
    let _column = parts.next()?;
    let line: u32 = parts.next()?.parse().ok().filter(|&l| l > 0)?;
    let path = parts.next()?.trim();
    (!path.is_empty()).then(|| SourceRef::File {
        path: path.to_owned(),
        line_start: Some(line),
        line_end: None,
    })
}

/// `text` cut to [`MAX_DETAILS`] bytes on a character boundary.
fn cut(text: &str) -> String {
    if text.len() <= MAX_DETAILS {
        return text.to_owned();
    }
    let mut end = MAX_DETAILS;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBTEST: &str = r#"
{ "type": "suite", "event": "started", "test_count": 3 }
{ "type": "test", "event": "started", "name": "tests::adds" }
{ "type": "test", "name": "tests::adds", "event": "ok", "exec_time": 0.001 }
{ "type": "test", "event": "started", "name": "tests::fails" }
{ "type": "test", "name": "tests::fails", "event": "failed", "exec_time": "0.002s", "stdout": "\nthread 'tests::fails' panicked at src/lib.rs:12:9:\nassertion `left == right` failed\n  left: 1\n right: 2\n" }
{ "type": "test", "name": "tests::later", "event": "ignored", "message": "slow" }
{ "type": "suite", "event": "failed", "passed": 1, "failed": 1, "ignored": 1, "exec_time": 0.01 }
   Doc-tests demo
{ "type": "suite", "event": "started", "test_count": 1 }
{ "type": "test", "name": "src/lib.rs - add (line 3)", "event": "failed", "stdout": "thread 'main' panicked at 'boom', src/lib.rs:5:1\n" }
{ "type": "suite", "event": "failed", "passed": 0, "failed": 1, "ignored": 0 }
"#;

    #[test]
    fn libtest_events_become_suites_of_cases() {
        assert!(is_json_stream(LIBTEST.as_bytes()));
        assert!(!is_json_stream(b"  <testsuites/>"));
        let spec = parse(LIBTEST.as_bytes(), "target/test.json").unwrap();
        assert_eq!(spec.title.as_deref(), Some("test.json"));
        let names: Vec<&str> = spec.suites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["test binary 1", "test binary 2"]);
        assert_eq!(spec.suites[0].time_secs, Some(0.01));
        let [adds, fails, later] = &spec.suites[0].cases[..] else {
            panic!("three cases: {:?}", spec.suites[0].cases);
        };
        assert_eq!(
            (adds.outcome, adds.time_secs),
            (TestOutcome::Passed, Some(0.001))
        );
        assert_eq!(fails.outcome, TestOutcome::Failed);
        assert_eq!(fails.time_secs, Some(0.002));
        assert_eq!(
            fails.message.as_deref(),
            Some("assertion `left == right` failed")
        );
        assert_eq!(
            fails.location,
            Some(SourceRef::File {
                path: "src/lib.rs".into(),
                line_start: Some(12),
                line_end: None,
            })
        );
        assert!(
            fails
                .details
                .as_deref()
                .unwrap_or_default()
                .contains("right: 2")
        );
        assert_eq!(
            (later.outcome, later.message.as_deref()),
            (TestOutcome::Skipped, Some("slow"))
        );
        let doc = &spec.suites[1].cases[0];
        assert_eq!(doc.message.as_deref(), Some("boom"));
        assert!(matches!(
            doc.location,
            Some(SourceRef::File {
                line_start: Some(5),
                ..
            })
        ));
        assert_eq!(spec.totals().failed, 2);
    }

    #[test]
    fn nextest_names_pick_the_suite() {
        let stream = r#"{"type":"suite","event":"started","test_count":2}
{"type":"test","event":"ok","name":"demo::bin/demo$tests::a","exec_time":0.1}
{"type":"test","event":"timeout","name":"demo$tests::slow"}
{"type":"suite","event":"ok"}"#;
        let spec = parse(stream.as_bytes(), "nextest.json").unwrap();
        let names: Vec<&str> = spec.suites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["demo::bin/demo", "demo"]);
        assert_eq!(spec.suites[0].cases[0].name, "tests::a");
        let slow = &spec.suites[1].cases[0];
        assert_eq!(
            (slow.outcome, slow.message.as_deref()),
            (TestOutcome::Errored, Some("timed out"))
        );
    }

    #[test]
    fn a_stream_without_tests_is_an_error() {
        let err = parse(br#"{"type":"suite","event":"started"}"#, "x.json").unwrap_err();
        assert!(err.contains("no test results"), "{err}");
        assert!(parse(&[0xff, 0xfe], "x.json").is_err());
    }
}
