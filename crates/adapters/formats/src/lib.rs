//! [`FormatsPort`] adapter: developer file formats behind the report
//! blocks.
//!
//! - test reports: JUnit XML ([`junit`], quick-xml) and the libtest /
//!   nextest JSON event stream ([`libtest`], serde), told apart by
//!   content;
//! - SARIF 2.1.0 static-analysis logs ([`sarif`], serde);
//! - WaveDrom WaveJSON timing diagrams to SVG ([`wave`], `wavedrom`);
//! - dataset previews of CSV, TSV, JSON lines and Parquet ([`dataset`],
//!   `parquet` without arrow);
//! - 2D molecules from SMILES ([`molecule`], `chematic`);
//! - distributed traces from OTLP, Jaeger and Zipkin JSON ([`trace`],
//!   serde);
//! - profiles from folded stacks and pprof ([`flame`], prost + flate2);
//! - log files in JSON lines, logfmt or text ([`logs`]);
//! - PostgreSQL `EXPLAIN (FORMAT JSON)` plans ([`query_plan`], serde_json).

pub mod dataset;
pub mod flame;
pub mod junit;
pub mod libtest;
pub mod logs;
pub mod molecule;
pub mod query_plan;
pub mod sarif;
pub mod trace;
pub mod wave;

use application::ports::formats::{
    DatasetRequest, FormatError, FormatsPort, LogRequest, ProfileRequest,
};
use domain::dataset::DatasetSpec;
use domain::devops::flame::FlameSpec;
use domain::devops::logs::LogSpec;
use domain::devops::query_plan::QueryPlanSpec;
use domain::devops::trace::TraceSpec;
use domain::report::{FindingsSpec, TestReportSpec};
use domain::science::MoleculeSpec;

/// The formats adapter. Stateless.
#[derive(Debug, Clone, Copy, Default)]
pub struct Formats;

impl Formats {
    /// The adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl FormatsPort for Formats {
    fn parse_test_report(&self, bytes: &[u8], path: &str) -> Result<TestReportSpec, FormatError> {
        if libtest::is_json_stream(bytes) {
            libtest::parse(bytes, path)
        } else {
            junit::parse(bytes, path)
        }
        .map_err(FormatError::Invalid)
    }

    fn parse_sarif(&self, bytes: &[u8], path: &str) -> Result<FindingsSpec, FormatError> {
        sarif::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn render_wavedrom(&self, source: &str) -> Result<String, FormatError> {
        wave::render(source).map_err(FormatError::Invalid)
    }

    fn preview_dataset(
        &self,
        bytes: &[u8],
        request: &DatasetRequest,
    ) -> Result<DatasetSpec, FormatError> {
        dataset::preview(bytes, request).map_err(FormatError::Invalid)
    }

    fn depict_molecule(&self, smiles: &str, claimed_formula: Option<&str>) -> MoleculeSpec {
        molecule::depict(smiles, claimed_formula)
    }

    fn parse_trace(&self, bytes: &[u8], path: &str) -> Result<TraceSpec, FormatError> {
        trace::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn parse_profile(
        &self,
        bytes: &[u8],
        request: &ProfileRequest,
    ) -> Result<FlameSpec, FormatError> {
        flame::parse(bytes, &request.path, request.sample_type.as_deref())
            .map_err(FormatError::Invalid)
    }

    fn parse_logs(&self, bytes: &[u8], request: &LogRequest) -> Result<LogSpec, FormatError> {
        logs::parse(bytes, request).map_err(FormatError::Invalid)
    }

    fn parse_query_plan(
        &self,
        bytes: &[u8],
        path: Option<&str>,
    ) -> Result<QueryPlanSpec, FormatError> {
        query_plan::parse(bytes, path).map_err(FormatError::Invalid)
    }
}
