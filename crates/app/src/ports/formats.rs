//! Parsers and renderers of file formats behind the report and lab
//! blocks: test reports (JUnit XML, libtest JSON), static-analysis findings (SARIF),
//! timing diagrams (WaveDrom WaveJSON), dataset previews (CSV, TSV, JSON
//! lines, Parquet), 2D molecules (SMILES), distributed traces (OTLP,
//! Jaeger, Zipkin JSON), profiles (folded stacks, pprof), log files
//! (JSON lines, logfmt, text), SQL query plans (PostgreSQL `EXPLAIN
//! (FORMAT JSON)`), HTTP archives (HAR) and Terraform plans.
//!
//! The tools read the files through the workspace guard and hand the
//! bytes over; a [`FormatsPort`] only parses. Locations come back as the
//! report wrote them (a `file://` URI already turned into a path); making
//! them relative to the workspace is the tool's job. Parsing is
//! CPU-bound and synchronous: async callers run it on a blocking thread.

use domain::dataset::{DatasetFormat, DatasetSpec};
use domain::devops::flame::FlameSpec;
use domain::devops::har::HarSpec;
use domain::devops::logs::{LogFormat, LogSpec};
use domain::devops::query_plan::QueryPlanSpec;
use domain::devops::terraform::TerraformPlanSpec;
use domain::devops::trace::TraceSpec;
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

/// What [`FormatsPort::parse_profile`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRequest {
    /// Workspace-relative path, for the block and messages.
    pub path: String,
    /// The pprof sample type to draw (`cpu`, `alloc_space`...); the
    /// profile's default when absent. Folded stacks have one value.
    pub sample_type: Option<String>,
}

/// What [`FormatsPort::parse_logs`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRequest {
    /// Workspace-relative path, for the block and messages.
    pub path: String,
    /// How the lines are written; told from the first lines when absent.
    pub format: Option<LogFormat>,
    /// The bytes are only the head of a larger file (the last line may be
    /// cut).
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

    /// The spans of a trace file as a waterfall: OTLP JSON (an
    /// `ExportTraceServiceRequest`, or one per line as the collector's
    /// file exporter writes), Jaeger JSON or Zipkin v2 JSON, told apart by
    /// content. At most [`domain::devops::trace::MAX_SPANS`] spans are
    /// kept.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed JSON, an unknown shape or a
    /// file with no span.
    fn parse_trace(&self, bytes: &[u8], path: &str) -> Result<TraceSpec, FormatError>;

    /// A profile as a call tree: folded stacks (`a;b;c 12` lines) or a
    /// pprof `profile.proto`, gzipped or not.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for a file in neither format, an unknown
    /// sample type or a profile with no samples.
    fn parse_profile(
        &self,
        bytes: &[u8],
        request: &ProfileRequest,
    ) -> Result<FlameSpec, FormatError>;

    /// A log file: JSON lines, logfmt or plain text, with levels, times,
    /// fields, facets and a time histogram. At most
    /// [`domain::devops::logs::MAX_LINES`] lines are kept.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for a file that is not text or has no
    /// line.
    fn parse_logs(&self, bytes: &[u8], request: &LogRequest) -> Result<LogSpec, FormatError>;

    /// A SQL query plan: PostgreSQL `EXPLAIN (FORMAT JSON)`, with
    /// `ANALYZE` and `BUFFERS` when given (also pasted from `psql`), with
    /// self times, misestimates and warnings derived
    /// ([`QueryPlanSpec::analyse`]). `path` names the file, when there is
    /// one.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for a text plan, malformed JSON or JSON
    /// without a `Plan`.
    fn parse_query_plan(
        &self,
        bytes: &[u8],
        path: Option<&str>,
    ) -> Result<QueryPlanSpec, FormatError>;

    /// An HTTP archive (HAR 1.2, as DevTools, proxies and clients export
    /// it): requests in start order with timing phases, headers and body
    /// previews. At most [`domain::devops::har::MAX_ENTRIES`] requests are
    /// kept.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed JSON or a file without
    /// `log.entries`.
    fn parse_har(&self, bytes: &[u8], path: &str) -> Result<HarSpec, FormatError>;

    /// A Terraform or OpenTofu plan: `terraform show -json <planfile>`
    /// (resource changes with attribute diffs, sensitive values masked),
    /// or the JSON lines of `terraform plan -json` (actions only).
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for malformed JSON or JSON that is not a
    /// plan.
    fn parse_terraform_plan(
        &self,
        bytes: &[u8],
        path: &str,
    ) -> Result<TerraformPlanSpec, FormatError>;
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
