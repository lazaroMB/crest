//! [`FakeFormats`]: a [`FormatsPort`] with scripted answers, for tests of
//! the report tools without the real parsers.
//!
//! By default every test report is one suite `fake` with a passed and a
//! failed case (at `src/lib.rs:3`, or the absolute path given with
//! [`FakeFormats::with_location`]), every SARIF log is one warning and
//! every diagram is a tiny SVG. [`FakeFormats::failing`] makes every call
//! fail. Calls are recorded as `(method, path or source)`.

use std::sync::{Mutex, PoisonError};

use domain::block::SourceRef;
use domain::report::{
    Finding, FindingSeverity, FindingsSpec, TestCase, TestOutcome, TestReportSpec, TestSuite,
};

use crate::ports::formats::{FormatError, FormatsPort};

/// The SVG [`FakeFormats::render_wavedrom`] returns.
pub const FAKE_SVG: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#;

/// A scripted formats port.
#[derive(Debug, Default)]
pub struct FakeFormats {
    calls: Mutex<Vec<(String, String)>>,
    error: Option<String>,
    location: Option<String>,
}

impl FakeFormats {
    /// A port answering the defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A port whose every call fails with `message`.
    #[must_use]
    pub fn failing(message: &str) -> Self {
        Self {
            error: Some(message.to_owned()),
            ..Self::default()
        }
    }

    /// Locations point at `path` instead of `src/lib.rs`.
    #[must_use]
    pub fn with_location(mut self, path: &str) -> Self {
        self.location = Some(path.to_owned());
        self
    }

    /// The calls so far, as `(method, path or source)`.
    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, method: &str, arg: &str) -> Result<(), FormatError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_owned(), arg.to_owned()));
        match &self.error {
            Some(message) => Err(FormatError::Invalid(message.clone())),
            None => Ok(()),
        }
    }

    fn location(&self) -> SourceRef {
        SourceRef::File {
            path: self.location.clone().unwrap_or_else(|| "src/lib.rs".into()),
            line_start: Some(3),
            line_end: None,
        }
    }
}

impl FormatsPort for FakeFormats {
    fn parse_test_report(&self, _bytes: &[u8], path: &str) -> Result<TestReportSpec, FormatError> {
        self.record("parse_test_report", path)?;
        let case = |name: &str, outcome, message: Option<&str>| TestCase {
            name: name.to_owned(),
            classname: Some("fake".into()),
            outcome,
            time_secs: Some(0.5),
            message: message.map(str::to_owned),
            details: None,
            location: Some(self.location()),
        };
        Ok(TestReportSpec {
            title: Some(path.to_owned()),
            suites: vec![TestSuite {
                name: "fake".into(),
                time_secs: Some(1.0),
                cases: vec![
                    case("passes", TestOutcome::Passed, None),
                    case("fails", TestOutcome::Failed, Some("assert 1 == 2")),
                ],
            }],
        })
    }

    fn parse_sarif(&self, _bytes: &[u8], path: &str) -> Result<FindingsSpec, FormatError> {
        self.record("parse_sarif", path)?;
        Ok(FindingsSpec {
            tool: Some("fake-lint".into()),
            findings: vec![Finding {
                rule: Some("fake::rule".into()),
                severity: FindingSeverity::Warning,
                message: "something looks off".into(),
                location: Some(self.location()),
                snippet: None,
                help_uri: None,
                fix: None,
            }],
        })
    }

    fn render_wavedrom(&self, source: &str) -> Result<String, FormatError> {
        self.record("render_wavedrom", source)?;
        Ok(FAKE_SVG.to_owned())
    }
}
