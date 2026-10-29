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
//! - PostgreSQL `EXPLAIN (FORMAT JSON)` plans ([`query_plan`], serde_json);
//! - HTTP archives ([`har`], own serde);
//! - Terraform / OpenTofu plans ([`terraform`], serde_json);
//! - Graphviz DOT ([`dot`], own reduced parser) and ER diagrams from SQL
//!   DDL ([`er`], sqlparser);
//! - criterion benchmark estimates ([`bench`], serde);
//! - digital waveforms ([`waveform`], wellen: VCD, FST, GHW);
//! - netlists ([`netlist`], own SPICE and KiCad readers);
//! - STEP parts ([`step`], truck-stepio + truck-meshalgo);
//! - 2D DXF drawings ([`drawing`], the `dxf` crate);
//! - IFC room schedules ([`ifc`], ifc-lite-core).

pub mod bench;
pub mod dataset;
pub mod dot;
pub mod drawing;
pub mod er;
pub mod flame;
pub mod har;
pub mod ifc;
pub mod junit;
pub mod libtest;
pub mod logs;
pub mod molecule;
pub mod netlist;
pub mod query_plan;
pub mod sarif;
pub mod step;
pub mod terraform;
pub mod trace;
pub mod wave;
pub mod waveform;

use application::ports::formats::{
    BenchRequest, DatasetRequest, FormatError, FormatsPort, LogRequest, ProfileRequest,
    StepRequest, WaveRequest,
};
use domain::dataset::DatasetSpec;
use domain::devops::bench::BenchSpec;
use domain::devops::diagram::DiagramSpec;
use domain::devops::flame::FlameSpec;
use domain::devops::har::HarSpec;
use domain::devops::logs::LogSpec;
use domain::devops::query_plan::QueryPlanSpec;
use domain::devops::terraform::TerraformPlanSpec;
use domain::devops::trace::TraceSpec;
use domain::engineering::cad::CadModelSpec;
use domain::engineering::drawing::DrawingSpec;
use domain::engineering::ifc::IfcScheduleSpec;
use domain::engineering::netlist::{NetlistFormat, NetlistSpec};
use domain::engineering::waveform::WaveformSpec;
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

    fn parse_har(&self, bytes: &[u8], path: &str) -> Result<HarSpec, FormatError> {
        har::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn parse_terraform_plan(
        &self,
        bytes: &[u8],
        path: &str,
    ) -> Result<TerraformPlanSpec, FormatError> {
        terraform::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn parse_dot(&self, text: &str, path: Option<&str>) -> Result<DiagramSpec, FormatError> {
        dot::parse(text, path).map_err(FormatError::Invalid)
    }

    fn parse_ddl(
        &self,
        sql: &str,
        dialect: Option<&str>,
        path: Option<&str>,
    ) -> Result<DiagramSpec, FormatError> {
        er::parse(sql, dialect, path).map_err(FormatError::Invalid)
    }

    fn parse_benchmarks(&self, request: &BenchRequest) -> Result<BenchSpec, FormatError> {
        bench::parse(request).map_err(FormatError::Invalid)
    }

    fn parse_waveform(
        &self,
        bytes: &[u8],
        request: &WaveRequest,
    ) -> Result<WaveformSpec, FormatError> {
        waveform::parse(bytes, request).map_err(FormatError::Invalid)
    }

    fn parse_netlist(
        &self,
        text: &str,
        format: Option<NetlistFormat>,
        path: Option<&str>,
    ) -> Result<NetlistSpec, FormatError> {
        netlist::parse(text, format, path).map_err(FormatError::Invalid)
    }

    fn parse_step(&self, bytes: &[u8], request: &StepRequest) -> Result<CadModelSpec, FormatError> {
        step::parse(bytes, request).map_err(FormatError::Invalid)
    }

    fn parse_dxf(&self, bytes: &[u8], path: &str) -> Result<DrawingSpec, FormatError> {
        drawing::parse(bytes, path).map_err(FormatError::Invalid)
    }

    fn parse_ifc_schedule(&self, bytes: &[u8], path: &str) -> Result<IfcScheduleSpec, FormatError> {
        ifc::parse(bytes, path).map_err(FormatError::Invalid)
    }
}
