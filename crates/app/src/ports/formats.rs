//! Parsers and renderers of file formats behind the report and lab
//! blocks: test reports (JUnit XML, libtest JSON), static-analysis findings (SARIF),
//! timing diagrams (WaveDrom WaveJSON), dataset previews (CSV, TSV, JSON
//! lines, Parquet) and 2D molecules (SMILES).
//!
//! The tools read the files through the workspace guard and hand the
//! bytes over; a [`FormatsPort`] only parses. Locations come back as the
//! report wrote them (a `file://` URI already turned into a path); making
//! them relative to the workspace is the tool's job. Parsing is
//! CPU-bound and synchronous: async callers run it on a blocking thread.

use domain::dataset::{DatasetFormat, DatasetSpec};
use domain::report::{FindingsSpec, TestReportSpec};
use domain::science::MoleculeSpec;

/// What [`FormatsPort::preview_dataset`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetRequest {
    /// Workspace-relative path, for the block and messages.
    pub path: String,
    /// How to read the bytes.
    pub format: DatasetFormat,
    /// Rows to put in the preview table.
    pub preview_rows: usize,
    /// The bytes are only the head of a larger text file: the last line
    /// may be cut, the total row count is unknown and the statistics are
    /// a sample.
    pub truncated: bool,
}

/// Why a file could not be parsed or a diagram rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The input is malformed or not in the expected format.
    #[error("{0}")]
    Invalid(String),
}

/// Parses report files and renders diagrams.
pub trait FormatsPort: Send + Sync {
    /// A test report: JUnit XML (`<testsuites>` or a single
    /// `<testsuite>`), or the JSON lines of libtest / nextest
    /// (`--format json`, `--message-format libtest-json`), told apart by
    /// content. `path` names the file in messages and the title.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed XML or a file with no suite
    /// or test result.
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

    /// The schema, per-column statistics and first rows of a dataset file.
    /// Statistics cover at most a fixed number of rows (then `sampled` is
    /// set); `rows` is the total when it is known.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the bytes are not in `request.format`
    /// or the format is not supported.
    fn preview_dataset(
        &self,
        bytes: &[u8],
        request: &DatasetRequest,
    ) -> Result<DatasetSpec, FormatError>;

    /// A molecule from SMILES: canonical SMILES, Hill formula, average
    /// molecular weight, net charge and a 2D SVG depiction, compared with
    /// `claimed_formula` when given. A SMILES that does not parse comes
    /// back with [`MoleculeSpec::error`] set and nothing computed.
    fn depict_molecule(&self, smiles: &str, claimed_formula: Option<&str>) -> MoleculeSpec;
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
