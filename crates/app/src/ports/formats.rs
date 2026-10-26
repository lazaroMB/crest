//! Parsers and renderers of file formats behind the report and lab
//! blocks: test reports (JUnit XML, libtest JSON), static-analysis findings (SARIF),
//! timing diagrams (WaveDrom WaveJSON), dataset previews (CSV, TSV, JSON
//! lines, Parquet), 2D molecules (SMILES), distributed traces (OTLP,
//! Jaeger, Zipkin JSON), profiles (folded stacks, pprof), log files
//! (JSON lines, logfmt, text), SQL query plans (PostgreSQL `EXPLAIN
//! (FORMAT JSON)`), HTTP archives (HAR), Terraform plans, diagrams
//! (Graphviz DOT, ER from SQL DDL), criterion benchmark results and
//! engineering files (waveforms, netlists, STEP parts, DXF drawings, IFC
//! room schedules).
//!
//! The tools read the files through the workspace guard and hand the
//! bytes over; a [`FormatsPort`] only parses. Locations come back as the
//! report wrote them (a `file://` URI already turned into a path); making
//! them relative to the workspace is the tool's job. Parsing is
//! CPU-bound and synchronous: async callers run it on a blocking thread.

use domain::dataset::{DatasetFormat, DatasetSpec};
use domain::devops::bench::BenchSpec;
use domain::devops::diagram::DiagramSpec;
use domain::devops::flame::FlameSpec;
use domain::devops::har::HarSpec;
use domain::devops::logs::{LogFormat, LogSpec};
use domain::devops::query_plan::QueryPlanSpec;
use domain::devops::terraform::TerraformPlanSpec;
use domain::devops::trace::TraceSpec;
use domain::engineering::cad::CadModelSpec;
use domain::engineering::drawing::DrawingSpec;
use domain::engineering::ifc::IfcScheduleSpec;
use domain::engineering::netlist::{NetlistFormat, NetlistSpec};
use domain::engineering::waveform::{Radix, WaveformSpec};
use domain::report::{FindingsSpec, TestReportSpec};
use domain::science::MoleculeSpec;
use domain::science::reaction::ReactionSpec;
use domain::science::sequence::{Alphabet, SequenceSpec, Topology};

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

/// The criterion files of one benchmark, for
/// [`FormatsPort::parse_benchmarks`]; a file that is missing is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BenchFiles {
    /// The benchmark's directory, relative to the criterion root.
    pub dir: String,
    /// `new/benchmark.json` (the benchmark's id).
    pub benchmark: Option<Vec<u8>>,
    /// `new/estimates.json`: the last run.
    pub new: Option<Vec<u8>>,
    /// `<baseline>/estimates.json`.
    pub base: Option<Vec<u8>>,
    /// `change/estimates.json`: the relative change against the baseline.
    pub change: Option<Vec<u8>>,
}

/// What [`FormatsPort::parse_benchmarks`] reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BenchRequest {
    /// Workspace path of the criterion directory (`target/criterion`).
    pub path: String,
    /// The baseline compared against (`base`).
    pub baseline: String,
    /// The benchmarks found under it.
    pub benches: Vec<BenchFiles>,
}

/// What [`FormatsPort::parse_waveform`] reads besides the bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaveRequest {
    /// Workspace path of the dump, for the block and messages.
    pub path: String,
    /// Signals to keep: dotted paths or name patterns with `*`
    /// (`tb.cpu.*`, `*clk`); all (up to the cap) when empty.
    pub signals: Vec<String>,
    /// How buses are written at first.
    pub radix: Radix,
}

/// What [`FormatsPort::parse_step`] reads besides the bytes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepRequest {
    /// Workspace path of the file, for the block and messages.
    pub path: String,
    /// Chord tolerance of the tessellation in file units; by default a
    /// fraction of the part's size.
    pub tolerance: Option<f64>,
}

/// What [`FormatsPort::parse_sequence`] reads besides the bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequenceRequest {
    /// Workspace path of the file, for the block and messages; `None` for
    /// residues given inline.
    pub path: Option<String>,
    /// The record to show of a multi-record file, 0-based.
    pub record: usize,
    /// What the residues are; guessed from them when absent (GenBank
    /// says).
    pub alphabet: Option<Alphabet>,
    /// Linear or circular; GenBank's LOCUS line says, otherwise linear.
    pub topology: Option<Topology>,
    /// Search the bundled restriction enzymes (nucleic acids).
    pub enzymes: bool,
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

    /// A Graphviz DOT graph (the first of the text) on the graph widget:
    /// nodes, edges, clusters as groups, `label`, `color` (as the node
    /// kind) and `rankdir`. `path` names the file, when there is one.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for text that is not DOT or breaks its
    /// grammar.
    fn parse_dot(&self, text: &str, path: Option<&str>) -> Result<DiagramSpec, FormatError>;

    /// An ER diagram from SQL DDL: a node per `CREATE TABLE` (columns in
    /// its attributes and in [`DiagramSpec::tables`]), an edge per foreign
    /// key. `dialect` is a `sqlparser` dialect name (generic when
    /// absent); statements that do not parse are skipped with a warning.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for an unknown dialect or SQL without a
    /// table.
    fn parse_ddl(
        &self,
        sql: &str,
        dialect: Option<&str>,
        path: Option<&str>,
    ) -> Result<DiagramSpec, FormatError>;

    /// Criterion results: each benchmark's estimates for the new run and
    /// the baseline and its change, as criterion wrote them, with a
    /// verdict from the change's interval against the noise threshold.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when no benchmark has a readable new run.
    fn parse_benchmarks(&self, request: &BenchRequest) -> Result<BenchSpec, FormatError>;

    /// A VCD, FST or GHW dump: the signals (filtered by the request's
    /// patterns) in hierarchy order with their value changes, capped at
    /// the domain's limits.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] for an unknown or malformed dump, or when
    /// no signal matches.
    fn parse_waveform(
        &self,
        bytes: &[u8],
        request: &WaveRequest,
    ) -> Result<WaveformSpec, FormatError>;

    /// A SPICE deck or KiCad netlist as components and their pin-to-net
    /// assignments; `format` is guessed from the text when `None`.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the text holds no component or is
    /// malformed.
    fn parse_netlist(
        &self,
        text: &str,
        format: Option<NetlistFormat>,
        path: Option<&str>,
    ) -> Result<NetlistSpec, FormatError>;

    /// A STEP part (ISO 10303-21, AP203/AP214/AP242 B-rep): every solid
    /// tessellated into one mesh, with the file's schema, unit and
    /// product names. Runs within a time limit; a reader crash on a file
    /// is an error, not a panic.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the file is not STEP, holds no solid
    /// that could be meshed, is over the triangle limit or took too long.
    fn parse_step(&self, bytes: &[u8], request: &StepRequest) -> Result<CadModelSpec, FormatError>;

    /// A 2D DXF drawing (ASCII or binary): its layers and model-space
    /// entities with block inserts expanded; `path` names it.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the file is not DXF or holds nothing
    /// to draw.
    fn parse_dxf(&self, bytes: &[u8], path: &str) -> Result<DrawingSpec, FormatError>;

    /// The room schedule of an IFC file (IFC2X3, IFC4, IFC4X3): storeys,
    /// their spaces with the areas the file gives (and where), and wall,
    /// door and window counts; `path` names it.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when the file is not IFC or has neither
    /// storeys nor spaces.
    fn parse_ifc_schedule(&self, bytes: &[u8], path: &str) -> Result<IfcScheduleSpec, FormatError>;

    /// A sequence: a GenBank flat file (features, topology, organism),
    /// FASTA or bare residues, told apart by content, with the cut sites
    /// of the bundled enzymes when asked.
    ///
    /// # Errors
    ///
    /// [`FormatError::Invalid`] when nothing reads as a sequence or the
    /// record does not exist.
    fn parse_sequence(
        &self,
        bytes: &[u8],
        request: &SequenceRequest,
    ) -> Result<SequenceSpec, FormatError>;

    /// A reaction scheme from reaction SMILES (`A.B>agents>C.D`): each
    /// molecule depicted with its formula, the atom and charge balance with
    /// `coefficients` (reactants then products; 1 each when absent) and,
    /// when it does not balance, the smallest integer coefficients that do.
    /// A reaction that cannot be read comes back with `error` set.
    fn depict_reaction(&self, smiles: &str, coefficients: Option<&[u32]>) -> ReactionSpec;
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
