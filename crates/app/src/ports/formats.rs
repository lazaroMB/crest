//! Parsers and renderers of developer file formats behind the report
//! blocks: test reports (JUnit XML), static-analysis findings (SARIF) and
//! timing diagrams (WaveDrom WaveJSON).
//!
//! The tools read the files through the workspace guard and hand the
//! bytes over; a [`FormatsPort`] only parses. Locations come back as the
//! report wrote them (a `file://` URI already turned into a path); making
//! them relative to the workspace is the tool's job. Parsing is
//! CPU-bound and synchronous: async callers run it on a blocking thread.

use domain::report::{FindingsSpec, TestReportSpec};

/// Why a file could not be parsed or a diagram rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The input is malformed or not in the expected format.
    #[error("{0}")]
    Invalid(String),
}

/// Parses report files and renders diagrams.
pub trait FormatsPort: Send + Sync {
    /// A JUnit XML report (`<testsuites>` or a single `<testsuite>`).
    /// `path` names the file in messages and the title.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed XML or a file with no suite.
    fn parse_test_report(&self, bytes: &[u8], path: &str) -> Result<TestReportSpec, FormatError>;

    /// A SARIF 2.1.0 log: every result of every run.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed JSON or a log with no runs.
    fn parse_sarif(&self, bytes: &[u8], path: &str) -> Result<FindingsSpec, FormatError>;

    /// A WaveDrom diagram (WaveJSON, JSON5-style keys allowed) as SVG.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the source does not parse.
    fn render_wavedrom(&self, source: &str) -> Result<String, FormatError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn FormatsPort>>) {}
        assert_dyn(None);
    }
}
