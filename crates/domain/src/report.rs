//! Reports from developer tools: test runs (JUnit XML, `show_test_report`)
//! and static-analysis findings (SARIF, `show_findings`).
//!
//! Adapters parse the files; these types are what the blocks draw.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::{BlockError, SourceRef};

/// A test run: suites of cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestReportSpec {
    /// Title, e.g. the report file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Suites, in report order.
    pub suites: Vec<TestSuite>,
}

/// A test suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestSuite {
    /// Suite name.
    pub name: String,
    /// Wall time of the suite, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_secs: Option<f64>,
    /// Cases, in report order.
    pub cases: Vec<TestCase>,
}

/// One test case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestCase {
    /// Case name.
    pub name: String,
    /// Class or module, when the report has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classname: Option<String>,
    /// Outcome.
    pub outcome: TestOutcome,
    /// Wall time, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_secs: Option<f64>,
    /// Failure or skip message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Stack trace or captured output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Where the test or the failure is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceRef>,
}

/// Outcome of a [`TestCase`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    /// Passed.
    Passed,
    /// An assertion failed.
    Failed,
    /// The test could not run to completion (panic, setup error).
    Errored,
    /// Not run.
    Skipped,
}

/// Counts of a report by outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestTotals {
    /// Passed cases.
    pub passed: usize,
    /// Failed cases.
    pub failed: usize,
    /// Errored cases.
    pub errored: usize,
    /// Skipped cases.
    pub skipped: usize,
}

impl TestTotals {
    /// All cases.
    #[must_use]
    pub fn total(self) -> usize {
        self.passed + self.failed + self.errored + self.skipped
    }
}

impl TestReportSpec {
    /// Counts the cases by outcome.
    #[must_use]
    pub fn totals(&self) -> TestTotals {
        let mut totals = TestTotals::default();
        for case in self.suites.iter().flat_map(|s| &s.cases) {
            match case.outcome {
                TestOutcome::Passed => totals.passed += 1,
                TestOutcome::Failed => totals.failed += 1,
                TestOutcome::Errored => totals.errored += 1,
                TestOutcome::Skipped => totals.skipped += 1,
            }
        }
        totals
    }

    /// Checks the case locations.
    ///
    /// # Errors
    ///
    /// The first [`BlockError`] of a location.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.suites
            .iter()
            .flat_map(|s| &s.cases)
            .filter_map(|c| c.location.as_ref())
            .try_for_each(SourceRef::validate)
    }
}

/// Static-analysis findings, SARIF-like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FindingsSpec {
    /// Tool that produced them, e.g. `clippy 0.1.95`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The findings, in report order.
    pub findings: Vec<Finding>,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// Rule id, e.g. `clippy::unwrap_used`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Severity (SARIF `level`).
    pub severity: FindingSeverity,
    /// What is wrong.
    pub message: String,
    /// Where; usually a [`SourceRef::File`] with lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceRef>,
    /// Source excerpt of the location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Link to the rule's documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_uri: Option<String>,
    /// Suggested fix as a unified diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Severity of a [`Finding`], the SARIF levels.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// A problem.
    Error,
    /// A possible problem.
    Warning,
    /// Information.
    Note,
    /// No severity given.
    None,
}

impl FindingsSpec {
    /// Checks the finding locations.
    ///
    /// # Errors
    ///
    /// The first [`BlockError`] of a location.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.findings
            .iter()
            .filter_map(|f| f.location.as_ref())
            .try_for_each(SourceRef::validate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_report_totals_and_roundtrip() {
        let spec: TestReportSpec = serde_json::from_value(json!({
            "suites": [{"name": "unit", "time_secs": 0.5, "cases": [
                {"name": "a", "outcome": "passed"},
                {"name": "b", "outcome": "failed", "message": "assert", "time_secs": 0.1,
                 "location": {"type": "file", "path": "src/a.rs", "line_start": 3}},
                {"name": "c", "outcome": "skipped"},
                {"name": "d", "outcome": "errored"}
            ]}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        let totals = spec.totals();
        assert_eq!((totals.passed, totals.failed, totals.total()), (1, 1, 4));
        let back: TestReportSpec =
            serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn findings_validate_locations() {
        let mut spec: FindingsSpec = serde_json::from_value(json!({
            "tool": "clippy",
            "findings": [{"rule": "clippy::unwrap_used", "severity": "warning",
                          "message": "used unwrap",
                          "location": {"type": "file", "path": "src/a.rs", "line_start": 4, "line_end": 4}}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        assert!(FindingSeverity::Error < FindingSeverity::Note);
        spec.findings[0].location = Some(SourceRef::File {
            path: "a".into(),
            line_start: Some(0),
            line_end: None,
        });
        assert!(spec.validate().is_err());
    }
}
