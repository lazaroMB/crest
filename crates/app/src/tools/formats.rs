//! Report and lab tools, thin [`TypedTool`]s over the [`FormatsPort`]:
//! `show_test_report` (a JUnit XML file as a test report block),
//! `show_findings` (a SARIF log as a findings block), `show_wavedrom`
//! (WaveJSON as an SVG timing diagram), `show_dataset` (the schema,
//! column statistics and first rows of a CSV, TSV, JSON lines or Parquet
//! file), `show_molecule` (a SMILES as a 2D structure with its computed
//! formula and mass) and `show_plate` (a 96/384-well plate map, pure),
//! plus the developer/ops viewers of [`devops`] (`show_trace`,
//! `show_flamegraph`, `show_logs`), [`query_plan`] (`show_query_plan`),
//! [`har`] (`show_har`), [`terraform`] (`show_terraform_plan`),
//! [`diagram`] (`show_dot`, `show_er`), [`bench`] (`show_benchmarks`) and
//! the engineering viewers [`waveform`] (`show_waveform`), [`netlist`]
//! (`show_netlist`), [`material`] (`show_material`), [`step`]
//! (`show_step`), [`dxf`] (`show_dxf`) and [`ifc`]
//! (`show_ifc_schedule`), and the science viewers [`sequence`]
//! (`show_sequence`).
//! [`register_format_tools`] adds them and [`FORMAT_TOOLS_PROMPT`] tells
//! the model when to use each.
//!
//! Files are read through the file port, with the workspace and
//! sensitive-path guard of `read_file`, and parsed through the
//! [`Offload`] so a large report never blocks the executor. Locations the
//! report gives as absolute paths inside the workspace are made relative
//! to it, so the block can open them. The model gets the totals and the
//! failing tests or the findings as `file:line` lines (capped), enough to
//! act on, not the whole report; for a dataset, its shape and column
//! types, never the rows. They only read: no call needs approval.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use domain::block::{BlockKind, ImageData, SourceRef};
use domain::dataset::{DatasetFormat, DatasetSpec};
use domain::permission::ToolEffect;
use domain::report::{FindingSeverity, FindingsSpec, TestOutcome, TestReportSpec, apply_edits};
use domain::science::{MoleculeSpec, PlateScale, PlateSpec, Well, plate_dimensions};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use crate::ports::formats::{DatasetRequest, FormatsPort};
use crate::ports::fs::{FileSystemPort, FsError, WriteFileRequest};
use crate::usecases::compute::{Offload, offloaded};

pub mod bench;
pub mod devops;
pub mod diagram;
pub mod dxf;
pub mod har;
pub mod ifc;
pub mod material;
pub mod netlist;
pub mod query_plan;
pub mod sequence;
pub mod step;
pub mod terraform;
pub mod waveform;

pub use bench::{
    DEFAULT_BASELINE, DEFAULT_CRITERION_DIR, SHOW_BENCHMARKS, ShowBenchmarksTool, bench_summary,
    collect_benchmarks,
};
pub use devops::{
    SHOW_FLAMEGRAPH, SHOW_LOGS, SHOW_TRACE, ShowFlamegraphTool, ShowLogsTool, ShowTraceTool,
    flame_summary, logs_summary, read_log, trace_summary,
};
pub use diagram::{SHOW_DOT, SHOW_ER, ShowDotTool, ShowErTool, dot_summary, er_summary};
pub use dxf::{SHOW_DXF, ShowDxfTool, dxf_summary};
pub use har::{MAX_HAR_BYTES, SHOW_HAR, ShowHarTool, har_summary, read_har};
pub use ifc::{MAX_IFC_BYTES, SHOW_IFC_SCHEDULE, ShowIfcScheduleTool, ifc_summary, read_ifc};
pub use material::{SHOW_MATERIAL, ShowMaterialTool, material_summary};
pub use netlist::{SHOW_NETLIST, ShowNetlistTool, netlist_summary};
pub use query_plan::{SHOW_QUERY_PLAN, ShowQueryPlanTool, query_plan_summary};
pub use sequence::{SHOW_SEQUENCE, ShowSequenceTool, sequence_summary};
pub use step::{MAX_STEP_BYTES, SHOW_STEP, STEP_EXTENSIONS, ShowStepTool, read_step, step_summary};
pub use terraform::{SHOW_TERRAFORM_PLAN, ShowTerraformPlanTool, terraform_summary};
pub use waveform::{MAX_WAVE_BYTES, SHOW_WAVEFORM, ShowWaveformTool, read_wave, waveform_summary};

/// Largest report file the tools read, in bytes.
pub const MAX_REPORT_BYTES: u64 = 32 * 1024 * 1024;

/// Most failing tests or findings listed to the model; the block shows
/// all of them.
pub const MAX_LISTED: usize = 20;

/// What the report tools run on.
#[derive(Clone)]
pub struct FormatToolPorts {
    /// The parsers.
    pub formats: Arc<dyn FormatsPort>,
    /// Reads report files of the workspace (with the guard of `read_file`).
    pub fs: Arc<dyn FileSystemPort>,
    /// Runs the parsers off the async executor.
    pub offload: Offload,
}

/// An input error for the model to fix.
fn invalid(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: message.into(),
    }
}

/// Reads `path` of the workspace through the guard.
async fn read(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("report path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_REPORT_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

/// Reads a report, trace or profile file of the workspace (up to
/// [`MAX_REPORT_BYTES`]) through the guard, for callers outside the tools
/// (Lua's `app.formats`).
///
/// # Errors
///
/// A message for the caller: a bad path, a file the guard refuses or one
/// that cannot be read.
pub async fn read_report(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    read(fs, path).await
}

/// `path` relative to `root` when it is an absolute path inside it;
/// otherwise as given, without a leading `./`.
#[must_use]
pub fn relativize(root: &Path, path: &str) -> String {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        && let Ok(rest) = candidate.strip_prefix(root)
    {
        return rest.to_string_lossy().replace('\\', "/");
    }
    path.strip_prefix("./").unwrap_or(path).to_owned()
}

fn relativize_ref(root: &Path, location: &mut Option<SourceRef>) {
    if let Some(SourceRef::File { path, .. }) = location {
        *path = relativize(root, path);
    }
}

/// `text` on one line, cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// `path:line` of a location, or `path`.
#[must_use]
pub fn location_text(location: Option<&SourceRef>) -> Option<String> {
    match location? {
        SourceRef::File {
            path,
            line_start: Some(line),
            ..
        } => Some(format!("{path}:{line}")),
        SourceRef::File { path, .. } => Some(path.clone()),
        _ => None,
    }
}

/// Input of `show_test_report`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTestReport {
    /// A test report of the workspace: JUnit XML, e.g.
    /// `target/nextest/default/junit.xml` or `reports/pytest.xml`, or the
    /// JSON lines of `cargo test -- -Z unstable-options --format json` /
    /// `cargo nextest run --message-format libtest-json` saved to a file.
    pub path: String,
    /// Title shown above the report; defaults to the report's own name or
    /// the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_test_report`: a JUnit XML file as a test report block.
#[derive(Clone)]
pub struct ShowTestReportTool {
    ports: FormatToolPorts,
}

impl ShowTestReportTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_test_report`.
pub const SHOW_TEST_REPORT: &str = "show_test_report";

#[async_trait]
impl TypedTool for ShowTestReportTool {
    type Input = ShowTestReport;

    fn name(&self) -> &str {
        SHOW_TEST_REPORT
    }

    fn description(&self) -> &str {
        "Shows a test report of the workspace (JUnit XML, or libtest / \
         nextest JSON lines) as a test report in the user's timeline: \
         totals by outcome, failures first with their message and stack \
         trace, durations and links to the test files. Most runners write \
         JUnit (cargo nextest `--profile ci`, pytest `--junitxml`, Maven \
         Surefire, Jest/Vitest junit reporters, gotestsum `--junitfile`); \
         for cargo, redirect the JSON event stream to a file. Returns the \
         totals and the failing tests with their locations: do not repeat \
         the report in your reply."
    }

    fn effect(&self, _input: &ShowTestReport) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowTestReport,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_TEST_REPORT, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_test_report(&bytes, &path)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the report parser did not run")),
            };
            let root = ports.fs.root().as_path().to_path_buf();
            for case in spec.suites.iter_mut().flat_map(|s| &mut s.cases) {
                relativize_ref(&root, &mut case.location);
            }
            if let Some(title) = input.title.filter(|t| !t.trim().is_empty()) {
                spec.title = Some(title);
            }
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = test_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::TestReport(spec)))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_test_report`: the totals, the
/// time and the failing tests (capped at [`MAX_LISTED`]).
#[must_use]
pub fn test_summary(path: &str, spec: &TestReportSpec) -> String {
    let totals = spec.totals();
    let time: f64 = spec.suites.iter().filter_map(|s| s.time_secs).sum();
    let mut out = format!(
        "Test report `{path}` shown to the user: {} tests, {} passed, {} failed, {} errored, \
         {} skipped",
        totals.total(),
        totals.passed,
        totals.failed,
        totals.errored,
        totals.skipped
    );
    if time > 0.0 {
        let _ = write!(out, ", {time:.2} s");
    }
    out.push('.');
    let failing: Vec<_> = spec
        .suites
        .iter()
        .flat_map(|s| &s.cases)
        .filter(|c| matches!(c.outcome, TestOutcome::Failed | TestOutcome::Errored))
        .collect();
    if failing.is_empty() {
        return out;
    }
    out.push_str("\nFailing:");
    for case in failing.iter().take(MAX_LISTED) {
        let name = match &case.classname {
            Some(class) => format!("{class}::{}", case.name),
            None => case.name.clone(),
        };
        let _ = write!(out, "\n- {name}");
        if case.outcome == TestOutcome::Errored {
            out.push_str(" (error)");
        }
        if let Some(at) = location_text(case.location.as_ref()) {
            let _ = write!(out, " at {at}");
        }
        if let Some(message) = &case.message {
            let _ = write!(out, ": {}", one_line(message, 160));
        }
    }
    if failing.len() > MAX_LISTED {
        let _ = write!(out, "\n… and {} more", failing.len() - MAX_LISTED);
    }
    out
}

/// Input of `show_findings`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowFindings {
    /// A SARIF 2.1.0 log of the workspace, e.g. `clippy.sarif` (from
    /// `clippy-sarif`), `semgrep.sarif`, CodeQL or ESLint SARIF output.
    pub path: String,
}

/// `show_findings`: a SARIF log as a findings block.
#[derive(Clone)]
pub struct ShowFindingsTool {
    ports: FormatToolPorts,
}

impl ShowFindingsTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_findings`.
pub const SHOW_FINDINGS: &str = "show_findings";

#[async_trait]
impl TypedTool for ShowFindingsTool {
    type Input = ShowFindings;

    fn name(&self) -> &str {
        SHOW_FINDINGS
    }

    fn description(&self) -> &str {
        "Shows a SARIF static-analysis log of the workspace (linters, \
         security scanners) as findings in the user's timeline, grouped by \
         file and severity with the rule, message, snippet and a link to the \
         rule's documentation. Returns the counts and the findings as \
         `file:line: rule message` lines: do not repeat them in your reply."
    }

    fn effect(&self, _input: &ShowFindings) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowFindings,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_FINDINGS, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed =
                offloaded(&ports.offload, move || formats.parse_sarif(&bytes, &path)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the SARIF parser did not run")),
            };
            let root = ports.fs.root().as_path().to_path_buf();
            for finding in &mut spec.findings {
                relativize_ref(&root, &mut finding.location);
                for edit in finding.fixes.iter_mut().flat_map(|f| &mut f.edits) {
                    edit.path = relativize(&root, &edit.path);
                }
            }
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            fix_diffs(ports.fs.as_ref(), &mut spec).await;
            let summary = findings_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Findings(spec)))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Findings whose fix is turned into a diff; the others show the edits.
pub const MAX_FIX_DIFFS: usize = 200;

/// Sets [`Finding::fix`] to a unified diff of the first fix of each
/// finding (up to [`MAX_FIX_DIFFS`]), its edits applied to the files read
/// through `fs`. A file that cannot be read or edits that do not apply
/// leave `fix` empty, and the block lists the edits instead.
pub async fn fix_diffs(fs: &dyn FileSystemPort, spec: &mut FindingsSpec) {
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    for finding in spec
        .findings
        .iter_mut()
        .filter(|f| f.fix.is_none() && !f.fixes.is_empty())
        .take(MAX_FIX_DIFFS)
    {
        let Some(first) = finding.fixes.first() else {
            continue;
        };
        let mut paths: Vec<&str> = Vec::new();
        for edit in &first.edits {
            if !paths.contains(&edit.path.as_str()) {
                paths.push(&edit.path);
            }
        }
        let mut diff = String::new();
        for path in paths {
            if !files.contains_key(path) {
                let text = read(fs, path)
                    .await
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok());
                files.insert(path.to_owned(), text);
            }
            let Some(Some(text)) = files.get(path) else {
                diff.clear();
                break;
            };
            let edits = first.edits.iter().filter(|e| e.path == path);
            let (Ok(new), Ok(parsed)) = (apply_edits(text, edits), WorkspacePath::parse(path))
            else {
                diff.clear();
                break;
            };
            let request = WriteFileRequest {
                path: parsed,
                content: new,
            };
            match fs.prepare_write(request).await {
                Ok(change) => diff.push_str(change.diff_unified()),
                Err(_) => {
                    diff.clear();
                    break;
                }
            }
        }
        if !diff.is_empty() {
            finding.fix = Some(diff);
        }
    }
}

/// Name of a severity as SARIF writes it.
#[must_use]
pub fn severity_name(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::Error => "error",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Note => "note",
        FindingSeverity::None => "none",
    }
}

/// What the model gets back from `show_findings`: counts by severity and
/// the findings, most severe first (capped at [`MAX_LISTED`]).
#[must_use]
pub fn findings_summary(path: &str, spec: &FindingsSpec) -> String {
    let count = |severity| {
        spec.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    };
    let mut out = format!(
        "Findings of `{path}` shown to the user{}: {} findings ({} errors, {} warnings, {} notes).",
        spec.tool
            .as_deref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default(),
        spec.findings.len(),
        count(FindingSeverity::Error),
        count(FindingSeverity::Warning),
        count(FindingSeverity::Note),
    );
    let mut sorted: Vec<_> = spec.findings.iter().collect();
    sorted.sort_by_key(|f| f.severity);
    for finding in sorted.iter().take(MAX_LISTED) {
        let at = location_text(finding.location.as_ref()).unwrap_or_else(|| "?".into());
        let rule = finding
            .rule
            .as_deref()
            .map(|r| format!(" [{r}]"))
            .unwrap_or_default();
        let fix = if finding.fixes.is_empty() {
            ""
        } else {
            " (fix suggested)"
        };
        let _ = write!(
            out,
            "\n- {at}: {}{rule} {}{fix}",
            severity_name(finding.severity),
            one_line(&finding.message, 160)
        );
    }
    if sorted.len() > MAX_LISTED {
        let _ = write!(out, "\n… and {} more", sorted.len() - MAX_LISTED);
    }
    out
}

/// Input of `show_wavedrom`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowWavedrom {
    /// The diagram as WaveJSON, e.g. `{ "signal": [ { "name": "clk",
    /// "wave": "p...." }, { "name": "data", "wave": "x=.=x", "data": ["a",
    /// "b"] } ] }`. Wave characters: `p`/`n` clocks, `0`/`1` levels, `x`
    /// unknown, `z` high impedance, `=` and `2`-`9` data (labels from
    /// `data`), `.` repeats the previous cycle, `|` a gap. JSON5-style
    /// keys without quotes are accepted.
    pub source: String,
}

/// `show_wavedrom`: a WaveDrom timing diagram as an SVG image block.
#[derive(Clone)]
pub struct ShowWavedromTool {
    ports: FormatToolPorts,
}

impl ShowWavedromTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Media type of the WaveDrom image block: SVG to tint to the theme.
pub const WAVEDROM_MEDIA_TYPE: &str = "image/svg+xml; tint=theme";

/// Name of `show_wavedrom`.
pub const SHOW_WAVEDROM: &str = "show_wavedrom";

#[async_trait]
impl TypedTool for ShowWavedromTool {
    type Input = ShowWavedrom;

    fn name(&self) -> &str {
        SHOW_WAVEDROM
    }

    fn description(&self) -> &str {
        "Draws a digital timing diagram (WaveDrom WaveJSON) in the user's \
         timeline: clocks, signals, buses with data labels. Use it for \
         protocols, handshakes, register timing and state sequences over \
         clock cycles. Returns only a short confirmation."
    }

    fn effect(&self, _input: &ShowWavedrom) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowWavedrom,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.source.trim().is_empty() {
            return Err(invalid(SHOW_WAVEDROM, "`source` is empty"));
        }
        // Rendering is quick; a bad source goes back as an input error.
        let svg = self
            .ports
            .formats
            .render_wavedrom(&input.source)
            .map_err(|err| invalid(SHOW_WAVEDROM, err.to_string()))?;
        // Drawn black on white; the parameter asks the UI to tint it to
        // the theme.
        let block = BlockKind::Image {
            media_type: WAVEDROM_MEDIA_TYPE.into(),
            data_or_path: ImageData::Base64 {
                data: base64::engine::general_purpose::STANDARD.encode(svg),
            },
            source: None,
        };
        let output =
            ToolOutput::text("WaveDrom diagram shown to the user.").with_block_update(block);
        Ok(PreparedCall::new(
            "timing diagram".to_owned(),
            futures::future::ready(Ok(output)).boxed(),
        ))
    }
}

/// Largest Parquet file `show_dataset` reads (Parquet needs the whole
/// file), in bytes.
pub const MAX_PARQUET_BYTES: u64 = 256 * 1024 * 1024;

/// How much of a text dataset (CSV, TSV, JSON lines) `show_dataset`
/// reads; a larger file is profiled on its head and marked sampled.
pub const MAX_TEXT_HEAD_BYTES: u64 = 32 * 1024 * 1024;

/// Preview rows by default and at most.
pub const DEFAULT_PREVIEW_ROWS: usize = 20;
/// Most preview rows a call may ask for.
pub const MAX_PREVIEW_ROWS: usize = 200;

/// Most columns described to the model; the block shows all of them.
pub const MAX_LISTED_COLUMNS: usize = 60;

/// The format a dataset file name implies, if it is one `show_dataset`
/// reads.
#[must_use]
pub fn dataset_format(path: &str) -> Option<DatasetFormat> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "csv" => Some(DatasetFormat::Csv),
        "tsv" | "tab" => Some(DatasetFormat::Tsv),
        "jsonl" | "ndjson" => Some(DatasetFormat::Jsonl),
        "parquet" | "pq" => Some(DatasetFormat::Parquet),
        "arrow" | "feather" | "ipc" => Some(DatasetFormat::Arrow),
        _ => None,
    }
}

/// Lower-case name of a dataset format.
#[must_use]
pub fn format_name(format: DatasetFormat) -> &'static str {
    match format {
        DatasetFormat::Csv => "csv",
        DatasetFormat::Tsv => "tsv",
        DatasetFormat::Parquet => "parquet",
        DatasetFormat::Arrow => "arrow",
        DatasetFormat::Jsonl => "jsonl",
    }
}

/// `bytes` in MiB with one decimal.
fn mib(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let mib = bytes as f64 / (1024.0 * 1024.0);
    format!("{mib:.1} MiB")
}

/// Input of `show_dataset`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowDataset {
    /// A data file of the workspace: `.csv`, `.tsv`, `.jsonl`/`.ndjson`
    /// (one JSON object per line) or `.parquet`.
    pub path: String,
    /// Rows in the preview table (default 20, at most 200).
    #[serde(default)]
    pub rows: Option<usize>,
}

/// `show_dataset`: a data file's schema, column statistics and first rows.
#[derive(Clone)]
pub struct ShowDatasetTool {
    ports: FormatToolPorts,
}

impl ShowDatasetTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_dataset`.
pub const SHOW_DATASET: &str = "show_dataset";

/// Reads a dataset: a Parquet file whole (up to [`MAX_PARQUET_BYTES`]),
/// a text file's head (up to [`MAX_TEXT_HEAD_BYTES`]). Returns the bytes
/// and whether they are only the head.
///
/// # Errors
///
/// A message for the caller: a bad path, a file the guard refuses or that
/// cannot be read, or a Parquet file over the limit.
pub async fn read_dataset(
    fs: &dyn FileSystemPort,
    path: &str,
    format: DatasetFormat,
) -> Result<(Vec<u8>, bool), String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("dataset path `{path}`: {e}"))?;
    if format == DatasetFormat::Parquet {
        return match fs.read_bytes(&parsed, MAX_PARQUET_BYTES).await {
            Ok(bytes) => Ok((bytes, false)),
            Err(FsError::TooLarge { size, limit }) => Err(format!(
                "`{path}` is {}; Parquet previews read the whole file and the limit is {}. \
                 Write a smaller sample (e.g. the first row groups) and show that",
                mib(size),
                mib(limit)
            )),
            Err(e) => Err(format!("cannot read `{path}`: {e}")),
        };
    }
    let (bytes, size) = fs
        .read_head(&parsed, MAX_TEXT_HEAD_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    let truncated = (bytes.len() as u64) < size;
    Ok((bytes, truncated))
}

#[async_trait]
impl TypedTool for ShowDatasetTool {
    type Input = ShowDataset;

    fn name(&self) -> &str {
        SHOW_DATASET
    }

    fn description(&self) -> &str {
        "Shows a data file of the workspace (CSV, TSV, JSON lines or \
         Parquet) in the user's timeline: its shape, one row per column \
         with the type, missing share, distinct count, range and a \
         distribution sparkline, then the first rows. Use it to look at a \
         dataset before analysing it. Returns the shape and the column \
         types with their missing shares, never the rows."
    }

    fn effect(&self, _input: &ShowDataset) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowDataset,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_DATASET, "`path` is empty"));
        }
        let Some(format) = dataset_format(&input.path) else {
            return Err(invalid(
                SHOW_DATASET,
                format!(
                    "`{}` is not a dataset file: use .csv, .tsv, .jsonl/.ndjson or .parquet",
                    input.path
                ),
            ));
        };
        let preview_rows = input
            .rows
            .unwrap_or(DEFAULT_PREVIEW_ROWS)
            .min(MAX_PREVIEW_ROWS);
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let (bytes, truncated) =
                match read_dataset(ports.fs.as_ref(), &input.path, format).await {
                    Ok(read) => read,
                    Err(message) => return Ok(ToolOutput::error(message)),
                };
            let request = DatasetRequest {
                path: input.path.clone(),
                format,
                preview_rows,
                truncated,
            };
            let formats = Arc::clone(&ports.formats);
            let parsed = offloaded(&ports.offload, move || {
                formats.preview_dataset(&bytes, &request)
            })
            .await;
            let spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => {
                    return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
                }
                None => return Ok(ToolOutput::error("the dataset reader did not run")),
            };
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = dataset_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Dataset(spec)))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_dataset`: the shape, whether the
/// statistics are a sample, and per column its type, missing share and
/// distinct count (capped at [`MAX_LISTED_COLUMNS`]).
#[must_use]
pub fn dataset_summary(spec: &DatasetSpec) -> String {
    let rows = spec
        .rows
        .map_or_else(|| "an unknown number of".to_owned(), |n| n.to_string());
    let mut out = format!(
        "Dataset `{}` ({}) shown to the user: {rows} rows × {} columns.",
        spec.path,
        format_name(spec.format),
        spec.columns.len()
    );
    if spec.sampled {
        out.push_str(" Column statistics come from the first rows only (sampled).");
    }
    out.push_str("\nColumns:");
    for column in spec.columns.iter().take(MAX_LISTED_COLUMNS) {
        let _ = write!(
            out,
            "\n- {}: {}, {:.1}% missing",
            column.name, column.dtype, column.null_percent
        );
        if let Some(distinct) = column.distinct {
            let at_least = if column.distinct_capped { "≥" } else { "" };
            let _ = write!(out, ", {at_least}{distinct} distinct");
        }
    }
    if spec.columns.len() > MAX_LISTED_COLUMNS {
        let _ = write!(
            out,
            "\n… and {} more",
            spec.columns.len() - MAX_LISTED_COLUMNS
        );
    }
    out
}

/// Input of `show_molecule`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMolecule {
    /// The structure as SMILES, e.g. `CC(=O)Oc1ccccc1C(=O)O` (aspirin).
    pub smiles: String,
    /// Name shown with the drawing, e.g. `aspirin`.
    #[serde(default)]
    pub name: Option<String>,
    /// The molecular formula you believe it has, e.g. `C9H8O4`; it is
    /// checked against the formula computed from the SMILES.
    #[serde(default)]
    pub claimed_formula: Option<String>,
}

/// `show_molecule`: a SMILES as a 2D structure with its computed formula,
/// mass, charge and canonical SMILES.
#[derive(Clone)]
pub struct ShowMoleculeTool {
    ports: FormatToolPorts,
}

impl ShowMoleculeTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_molecule`.
pub const SHOW_MOLECULE: &str = "show_molecule";

#[async_trait]
impl TypedTool for ShowMoleculeTool {
    type Input = ShowMolecule;

    fn name(&self) -> &str {
        SHOW_MOLECULE
    }

    fn description(&self) -> &str {
        "Draws a molecule from SMILES in the user's timeline with its \
         molecular formula, molecular weight, net charge and canonical \
         SMILES, all computed from the structure. Pass `claimed_formula` \
         when you state a formula so the block checks it. Returns the \
         computed values (cite these, not values from memory) or the parse \
         error to fix."
    }

    fn effect(&self, _input: &ShowMolecule) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMolecule,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.smiles.trim().is_empty() {
            return Err(invalid(SHOW_MOLECULE, "`smiles` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| input.smiles.clone());
        let run = async move {
            let formats = Arc::clone(&ports.formats);
            let smiles = input.smiles.trim().to_owned();
            let claimed = input
                .claimed_formula
                .clone()
                .filter(|c| !c.trim().is_empty());
            let depicted = offloaded(&ports.offload, move || {
                formats.depict_molecule(&smiles, claimed.as_deref())
            })
            .await;
            let Some(mut spec) = depicted else {
                return Ok(ToolOutput::error("the molecule renderer did not run"));
            };
            spec.name = input.name.filter(|n| !n.trim().is_empty());
            let summary = molecule_summary(&spec);
            let output = if spec.error.is_some() {
                ToolOutput::error(summary)
            } else {
                ToolOutput::text(summary)
            };
            Ok(output.with_block_update(BlockKind::Molecule(spec)))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_molecule`: the computed values and
/// a notice when its claimed formula disagrees, or the parse error.
#[must_use]
pub fn molecule_summary(spec: &MoleculeSpec) -> String {
    let name = spec
        .name
        .as_deref()
        .map(|n| format!(" {n}"))
        .unwrap_or_default();
    if let Some(error) = &spec.error {
        return format!(
            "Molecule{name} could not be drawn: {error}. Fix the SMILES `{}` and call again.",
            spec.smiles
        );
    }
    let mut out = format!("Molecule{name} shown to the user, computed from the SMILES:");
    if let Some(formula) = &spec.formula {
        let _ = write!(out, " formula {formula}");
    }
    if let Some(mw) = spec.mw {
        let _ = write!(out, ", MW {mw:.2} g/mol");
    }
    if let Some(charge) = spec.charge {
        let _ = write!(out, ", net charge {charge:+}");
    }
    if let Some(canonical) = &spec.canonical {
        let _ = write!(out, ", canonical SMILES `{canonical}`");
    }
    out.push('.');
    match (spec.claimed_matches, &spec.claimed_formula, &spec.formula) {
        (Some(false), Some(claimed), Some(formula)) => {
            let _ = write!(
                out,
                "\nMismatch: you said {claimed}, computed {formula}. Correct the formula in \
                 your reply, or the SMILES if it is not the structure you meant."
            );
        }
        (None, Some(claimed), _) => {
            let _ = write!(
                out,
                "\nThe claimed formula {claimed} could not be read as a formula; it was not checked."
            );
        }
        _ => {}
    }
    out
}

/// Input of `show_plate`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowPlate {
    /// Wells on the plate: 96 (rows A–H, columns 1–12) or 384 (rows A–P,
    /// columns 1–24).
    pub format: u16,
    /// Wells with data, e.g. `{"well": "A1", "label": "blank", "value":
    /// 0.041}`; unlisted wells are drawn empty.
    pub wells: Vec<Well>,
    /// What the values are, e.g. `OD600` or `RFU`.
    #[serde(default)]
    pub value_label: Option<String>,
    /// Title shown above the plate.
    #[serde(default)]
    pub title: Option<String>,
    /// Colour scale: `sequential` (default), `diverging` around `center`
    /// (fold changes, z-scores, % of control) or `log` (concentrations,
    /// counts spanning decades; values at or below 0 are not coloured).
    #[serde(default)]
    pub scale: PlateScale,
    /// Centre of a diverging scale; 0 when the values straddle 0, else
    /// their median.
    #[serde(default)]
    pub center: Option<f64>,
}

/// `show_plate`: a 96- or 384-well plate map coloured by value.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowPlateTool;

/// Name of `show_plate`.
pub const SHOW_PLATE: &str = "show_plate";

#[async_trait]
impl TypedTool for ShowPlateTool {
    type Input = ShowPlate;

    fn name(&self) -> &str {
        SHOW_PLATE
    }

    fn description(&self) -> &str {
        "Shows a 96- or 384-well microplate map in the user's timeline: a \
         heatmap of the well values with a legend, sample labels inside the \
         wells and a tooltip per well. Use it for plate-reader results, \
         layouts of samples and controls, or screening hits. Returns only a \
         short confirmation."
    }

    fn effect(&self, _input: &ShowPlate) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowPlate,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let spec = PlateSpec {
            title: input.title.filter(|t| !t.trim().is_empty()),
            format: input.format,
            value_label: input.value_label.filter(|l| !l.trim().is_empty()),
            wells: input.wells,
            scale: input.scale,
            center: input.center,
        };
        spec.validate()
            .map_err(|err| invalid(SHOW_PLATE, err.to_string()))?;
        let values: Vec<f64> = spec.wells.iter().filter_map(|w| w.value).collect();
        let mut confirmation = format!(
            "{}-well plate map shown to the user ({} wells listed, {} with values",
            spec.format,
            spec.wells.len(),
            values.len()
        );
        if let (Some(lo), Some(hi)) = (
            values.iter().copied().reduce(f64::min),
            values.iter().copied().reduce(f64::max),
        ) {
            let unit = spec
                .value_label
                .as_deref()
                .map(|l| format!(" {l}"))
                .unwrap_or_default();
            let _ = write!(confirmation, ", range {lo}–{hi}{unit}");
        }
        confirmation.push_str(").");
        let (rows, cols) = plate_dimensions(spec.format).unwrap_or_default();
        let subject = spec
            .title
            .clone()
            .unwrap_or_else(|| format!("{rows}×{cols} plate"));
        let output = ToolOutput::text(confirmation).with_block_update(BlockKind::Plate(spec));
        Ok(PreparedCall::new(
            subject,
            futures::future::ready(Ok(output)).boxed(),
        ))
    }
}

/// Tells the model when to use the report and lab tools; appended to the
/// system prompt after the component tools.
pub const FORMAT_TOOLS_PROMPT: &str = "\
# Reports, data files and lab results

- `show_test_report`: after a test run that wrote a JUnit XML report, show \
it instead of pasting the output: the user sees failures first with their \
traces. Ask the runner for JUnit output when it can write it (cargo nextest, \
pytest `--junitxml`, Surefire, Jest/Vitest, gotestsum); a saved libtest or \
nextest JSON event stream works too.
- `show_findings`: show a SARIF log of a linter or scanner (clippy-sarif, \
semgrep, CodeQL, ESLint) grouped by file and severity.
- `show_wavedrom`: draw a digital timing diagram from WaveJSON (clocks, \
handshakes, bus transfers over cycles).
- `show_dataset`: look at a data file (CSV, TSV, JSON lines, Parquet) \
before any statistics on it: its columns, types, missing values and first \
rows. Use the column names and types it returns when you call the \
statistics tools.
- `show_molecule`: whenever you give a chemical structure, draw it from \
SMILES. Never state a formula or molecular weight from memory: cite the \
values the tool computes, and pass `claimed_formula` when you have one in \
mind so a mismatch is caught.
- `show_plate`: a 96- or 384-well plate map of values and sample labels.
- `show_trace`: show a trace file (OTLP JSON, Jaeger or Zipkin JSON) as a \
waterfall when the question is where time went or which call failed.
- `show_flamegraph`: show a profile (folded stacks or pprof) as a flame \
graph; reason from the hottest path and self times it returns.
- `show_logs`: show a log file (JSON lines, logfmt, text) as a filterable \
list instead of pasting lines; it returns the level counts and the error \
messages grouped by shape.
- `show_query_plan`: show a PostgreSQL `EXPLAIN (FORMAT JSON)` plan as a \
tree table when a query is slow; ask for `EXPLAIN (ANALYZE, BUFFERS, FORMAT \
JSON)` to get actual times. Explain from the slowest nodes, misestimates \
and warnings it returns.
- `show_har`: show an HTTP archive (HAR) as a waterfall with a request \
inspector when the question is why a page is slow or which request \
failed. Its summary hides cookies and tokens; never ask the user to paste \
them.
- `show_terraform_plan`: before an apply, show the plan (`terraform show \
-json tfplan > plan.json`) instead of reading the text output; point out \
destroys and replacements and what forces them.
- `show_dot`: draw an existing Graphviz DOT file or output (dependency \
graphs, `terraform graph`, build graphs) as an interactive graph.
- `show_er`: draw the entity-relationship diagram of a schema from its \
SQL DDL (schema dump or migrations) so it matches the real tables. Use \
`show_mermaid` only for diagrams you compose yourself.
- `show_benchmarks`: after `cargo bench`, show criterion's results \
(baseline against new) instead of pasting its output; cite its numbers and \
verdicts, never your own estimates.
- `show_waveform`: show a simulation dump (VCD, FST, GHW) as waves \
instead of reading the dump text; narrow it with `signals` patterns and \
cite the values and clock periods it returns.
- `show_netlist`: show a SPICE deck or KiCad netlist as connectivity \
before reasoning about a circuit's wiring; use the nets it returns \
(including single-pin nets) instead of tracing the text yourself.
- `show_material`: put material properties in a card with the source of \
each value; a value you only remember goes without a source and shows as \
unverified. Never present remembered values as sourced.
- `show_step`: show a STEP part (for example one your CadQuery or \
build123d script exported) in the 3D view so the user can check it; cite \
the boxes, solids and faces it returns, and say when faces were not meshed.
- `show_dxf`: show a 2D DXF drawing with its layers instead of reading the \
DXF text; cite the extents, units, layers and texts it returns.
- `show_ifc_schedule`: show an IFC model's rooms and areas per storey; \
quote only the areas it returns with their source, and say \
`not given` for spaces without one instead of estimating.
- `show_sequence`: show a DNA, RNA or protein sequence (GenBank, FASTA or \
residues) with its features, frames and, for plasmids, a map with \
restriction sites; pass `find` for a motif or primer. Cite the lengths, \
positions, GC content and cutters it returns, never your own counts.

The test report and findings results list what failed with `file:line`, so \
you can open the files and fix them; do not repeat the lists to the user.";

/// Registers `show_test_report`, `show_findings`, `show_wavedrom`,
/// `show_dataset`, `show_molecule`, `show_plate`, `show_trace`,
/// `show_flamegraph`, `show_logs`, `show_query_plan`, `show_har`,
/// `show_terraform_plan`, `show_dot`, `show_er`, `show_benchmarks`,
/// `show_waveform`, `show_netlist`, `show_material`, `show_step`,
/// `show_dxf`, `show_ifc_schedule` and `show_sequence`.
///
/// # Errors
///
/// A name is already registered.
pub fn register_format_tools(
    registry: &mut ToolRegistry,
    ports: &FormatToolPorts,
) -> Result<(), RegisterError> {
    registry.register_typed(ShowTestReportTool::new(ports.clone()))?;
    registry.register_typed(ShowFindingsTool::new(ports.clone()))?;
    registry.register_typed(ShowWavedromTool::new(ports.clone()))?;
    registry.register_typed(ShowDatasetTool::new(ports.clone()))?;
    registry.register_typed(ShowMoleculeTool::new(ports.clone()))?;
    registry.register_typed(ShowPlateTool)?;
    registry.register_typed(ShowTraceTool::new(ports.clone()))?;
    registry.register_typed(ShowFlamegraphTool::new(ports.clone()))?;
    registry.register_typed(ShowLogsTool::new(ports.clone()))?;
    registry.register_typed(ShowQueryPlanTool::new(ports.clone()))?;
    registry.register_typed(ShowHarTool::new(ports.clone()))?;
    registry.register_typed(ShowTerraformPlanTool::new(ports.clone()))?;
    registry.register_typed(ShowDotTool::new(ports.clone()))?;
    registry.register_typed(ShowErTool::new(ports.clone()))?;
    registry.register_typed(ShowBenchmarksTool::new(ports.clone()))?;
    registry.register_typed(ShowWaveformTool::new(ports.clone()))?;
    registry.register_typed(ShowNetlistTool::new(ports.clone()))?;
    registry.register_typed(ShowMaterialTool)?;
    registry.register_typed(ShowStepTool::new(ports.clone()))?;
    registry.register_typed(ShowDxfTool::new(ports.clone()))?;
    registry.register_typed(ShowIfcScheduleTool::new(ports.clone()))?;
    registry.register_typed(ShowSequenceTool::new(ports.clone()))
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::{FAKE_SVG, FakeFormats};
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root())
            .with_file("out/junit.xml", "<testsuites/>")
            .with_file("out/lint.sarif", "{}")
            .with_file("data/cars.csv", "x,label\n1,a\n")
            .with_file("data/big.jsonl", "{}\n".repeat(10))
            .with_file(".env", "SECRET=1");
        FormatToolPorts {
            formats,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    fn definition<T: TypedTool + 'static>(tool: T) -> Value {
        let tool = Typed(tool);
        json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema(),
        })
    }

    #[tokio::test]
    async fn sarif_fixes_become_diffs_of_the_files() {
        use domain::report::{Finding, FindingFix, FixEdit};
        let fs = InMemoryFs::new(test_root())
            .with_file("src/a.rs", "fn main() {\n    let x = f().unwrap();\n}\n");
        let finding = |path: &str| Finding {
            rule: Some("clippy::unwrap_used".into()),
            severity: FindingSeverity::Warning,
            message: "unwrap".into(),
            location: None,
            snippet: None,
            help_uri: None,
            fix: None,
            fixes: vec![FindingFix {
                description: Some("use `?`".into()),
                edits: vec![FixEdit {
                    path: path.into(),
                    start_line: Some(2),
                    start_column: Some(16),
                    end_column: Some(25),
                    inserted: "?".into(),
                    ..FixEdit::default()
                }],
            }],
        };
        let mut spec = FindingsSpec {
            tool: None,
            findings: vec![finding("src/a.rs"), finding("missing.rs")],
        };
        fix_diffs(&fs, &mut spec).await;
        let diff = spec.findings[0].fix.as_deref().unwrap_or_default();
        assert!(diff.contains("--- a/src/a.rs"), "{diff}");
        assert!(diff.contains("-    let x = f().unwrap();"), "{diff}");
        assert!(diff.contains("+    let x = f()?;"), "{diff}");
        // Unreadable: no diff, the edits stay for the block to list.
        assert_eq!(spec.findings[1].fix, None);
        assert_eq!(spec.findings[1].fixes.len(), 1);
        assert!(findings_summary("x.sarif", &spec).contains("(fix suggested)"));
    }

    #[test]
    fn definitions_and_effects() {
        let ports = ports(Arc::new(FakeFormats::new()));
        insta::assert_json_snapshot!(
            "show_test_report_definition",
            definition(ShowTestReportTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "show_findings_definition",
            definition(ShowFindingsTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "show_wavedrom_definition",
            definition(ShowWavedromTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "show_dataset_definition",
            definition(ShowDatasetTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "show_molecule_definition",
            definition(ShowMoleculeTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!("show_plate_definition", definition(ShowPlateTool));
        let tool = Typed(ShowTestReportTool::new(ports));
        assert_eq!(tool.effect(&json!({ "path": "a" })), ToolEffect::Read);
    }

    #[test]
    fn registers_every_format_tool_and_prompts_for_each() {
        let mut registry = ToolRegistry::new();
        register_format_tools(&mut registry, &ports(Arc::new(FakeFormats::new()))).unwrap();
        assert_eq!(registry.names().len(), 22);
        for name in registry.names() {
            assert!(FORMAT_TOOLS_PROMPT.contains(&format!("`{name}`")), "{name}");
        }
    }

    #[tokio::test]
    async fn a_dataset_is_a_block_and_the_model_gets_its_shape() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowDatasetTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "data/cars.csv", "rows": 2 }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Dataset `data/cars.csv` (csv) shown to the user: 3 rows × 2 columns.\n\
             Columns:\n- x: integer, 0.0% missing, 3 distinct\n\
             - label: text, 25.0% missing, 3 distinct"
        );
        let Some(BlockKind::Dataset(spec)) = out.block_update else {
            panic!("expected a dataset, got {:?}", out.block_update);
        };
        assert_eq!(spec.preview.rows.len(), 2);
        assert_eq!(
            formats.calls(),
            [("preview_dataset".to_owned(), "data/cars.csv".to_owned())]
        );
    }

    #[tokio::test]
    async fn dataset_inputs_and_files_are_checked() {
        let tool = Typed(ShowDatasetTool::new(ports(Arc::new(FakeFormats::new()))));
        let not_data = tool.prepare(json!({ "path": "src/lib.rs" }), &ctx()).await;
        assert!(matches!(not_data, Err(ToolError::InvalidInput { .. })));
        let guarded = call(&tool, json!({ "path": "../x.csv" })).await.unwrap();
        assert!(guarded.is_error);
        let bad = Typed(ShowDatasetTool::new(ports(Arc::new(FakeFormats::failing(
            "line 1 is not a JSON object",
        )))));
        let out = call(&bad, json!({ "path": "data/big.jsonl" }))
            .await
            .unwrap();
        assert!(out.is_error);
        assert_eq!(
            out.text_content(),
            "`data/big.jsonl`: line 1 is not a JSON object"
        );
        assert_eq!(dataset_format("a/B.PARQUET"), Some(DatasetFormat::Parquet));
        assert_eq!(dataset_format("x.ndjson"), Some(DatasetFormat::Jsonl));
        assert_eq!(dataset_format("README"), None);
    }

    #[tokio::test]
    async fn the_head_of_a_large_text_dataset_is_a_sample() {
        let fs = InMemoryFs::new(test_root()).with_file(
            "data/huge.csv",
            "x\n".repeat(usize::try_from(MAX_TEXT_HEAD_BYTES).unwrap_or(0)),
        );
        let (bytes, truncated) = read_dataset(&fs, "data/huge.csv", DatasetFormat::Csv)
            .await
            .unwrap();
        assert!(truncated);
        assert_eq!(bytes.len() as u64, MAX_TEXT_HEAD_BYTES);
        let small = InMemoryFs::new(test_root()).with_file("d.parquet", "PAR1");
        let (_, truncated) = read_dataset(&small, "d.parquet", DatasetFormat::Parquet)
            .await
            .unwrap();
        assert!(!truncated);
    }

    #[tokio::test]
    async fn a_molecule_reports_computed_values_and_a_wrong_claim() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowMoleculeTool::new(ports(Arc::clone(&formats))));
        let out = call(
            &tool,
            json!({ "smiles": "CCO", "name": "ethanol", "claimed_formula": "C2H4O" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "Molecule ethanol shown to the user, computed from the SMILES: formula C2H6O, \
             MW 46.07 g/mol, net charge +0, canonical SMILES `CCO`.\n\
             Mismatch: you said C2H4O, computed C2H6O. Correct the formula in your reply, \
             or the SMILES if it is not the structure you meant."
        );
        let Some(BlockKind::Molecule(spec)) = out.block_update else {
            panic!("expected a molecule, got {:?}", out.block_update);
        };
        assert_eq!(spec.name.as_deref(), Some("ethanol"));
        assert_eq!(spec.claimed_matches, Some(false));
        assert_eq!(
            formats.calls(),
            [("depict_molecule".to_owned(), "CCO".to_owned())]
        );
    }

    #[tokio::test]
    async fn a_bad_smiles_is_an_error_with_its_block() {
        let tool = Typed(ShowMoleculeTool::new(ports(Arc::new(FakeFormats::new()))));
        let out = call(&tool, json!({ "smiles": "bad" })).await.unwrap();
        assert!(out.is_error);
        assert!(
            out.text_content()
                .contains("could not be drawn: invalid SMILES"),
            "{}",
            out.text_content()
        );
        assert!(matches!(
            out.block_update,
            Some(BlockKind::Molecule(MoleculeSpec { error: Some(_), .. }))
        ));
        let empty = tool.prepare(json!({ "smiles": "  " }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn a_plate_is_validated_and_confirmed_briefly() {
        let tool = Typed(ShowPlateTool);
        let out = call(
            &tool,
            json!({
                "format": 96, "value_label": "OD600",
                "wells": [{"well": "A1", "label": "blank", "value": 0.04}, {"well": "H12", "value": 1.2}]
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            out.text_content(),
            "96-well plate map shown to the user (2 wells listed, 2 with values, \
             range 0.04–1.2 OD600)."
        );
        assert!(matches!(out.block_update, Some(BlockKind::Plate(_))));
        let outside = tool
            .prepare(json!({ "format": 96, "wells": [{"well": "I1"}] }), &ctx())
            .await;
        assert!(matches!(outside, Err(ToolError::InvalidInput { .. })));
        let format = tool
            .prepare(json!({ "format": 48, "wells": [] }), &ctx())
            .await;
        assert!(matches!(format, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn a_test_report_is_a_block_and_the_model_gets_the_failures() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowTestReportTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "out/junit.xml", "title": "CI run" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert!(
            text.starts_with(
                "Test report `out/junit.xml` shown to the user: 2 tests, 1 passed, 1 failed, \
                 0 errored, 0 skipped, 1.00 s."
            ),
            "{text}"
        );
        assert!(
            text.contains("- fake::fails at src/lib.rs:3: assert 1 == 2"),
            "{text}"
        );
        let Some(BlockKind::TestReport(spec)) = out.block_update else {
            panic!("expected a test report, got {:?}", out.block_update);
        };
        assert_eq!(spec.title.as_deref(), Some("CI run"));
        assert_eq!(
            formats.calls(),
            [("parse_test_report".to_owned(), "out/junit.xml".to_owned())]
        );
    }

    #[tokio::test]
    async fn absolute_locations_inside_the_workspace_become_relative() {
        let root = test_root();
        let inside = root.as_path().join("tests/a.py");
        let formats = Arc::new(FakeFormats::new().with_location(&inside.to_string_lossy()));
        let tool = Typed(ShowFindingsTool::new(ports(formats)));
        let out = call(&tool, json!({ "path": "out/lint.sarif" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert!(
            out.text_content()
                .contains("- tests/a.py:3: warning [fake::rule] something looks off"),
            "{}",
            out.text_content()
        );
        let Some(BlockKind::Findings(spec)) = out.block_update else {
            panic!("expected findings, got {:?}", out.block_update);
        };
        assert_eq!(
            spec.findings[0].location,
            Some(SourceRef::File {
                path: "tests/a.py".into(),
                line_start: Some(3),
                line_end: None,
            })
        );
        assert_eq!(
            relativize(Path::new("/w"), "/elsewhere/x.rs"),
            "/elsewhere/x.rs"
        );
        assert_eq!(relativize(Path::new("/w"), "./src/x.rs"), "src/x.rs");
    }

    #[tokio::test]
    async fn unreadable_guarded_or_bad_files_are_errors() {
        let tool = Typed(ShowTestReportTool::new(ports(Arc::new(FakeFormats::new()))));
        let missing = call(&tool, json!({ "path": "out/none.xml" }))
            .await
            .unwrap();
        assert!(missing.is_error);
        assert!(
            missing.text_content().contains("cannot read"),
            "{}",
            missing.text_content()
        );
        let escaping = call(&tool, json!({ "path": "../x.xml" })).await.unwrap();
        assert!(escaping.is_error);
        let bad = Typed(ShowFindingsTool::new(ports(Arc::new(
            FakeFormats::failing("`out/lint.sarif` is not SARIF JSON"),
        ))));
        let out = call(&bad, json!({ "path": "out/lint.sarif" }))
            .await
            .unwrap();
        assert!(out.is_error);
        assert_eq!(out.text_content(), "`out/lint.sarif` is not SARIF JSON");
        assert!(out.block_update.is_none());
        let empty = tool.prepare(json!({ "path": " " }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn a_wavedrom_diagram_is_an_svg_image() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowWavedromTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "source": "{signal: []}" }))
            .await
            .unwrap();
        let Some(BlockKind::Image {
            media_type,
            data_or_path: ImageData::Base64 { data },
            ..
        }) = out.block_update
        else {
            panic!("expected an image, got {:?}", out.block_update);
        };
        assert_eq!(media_type, WAVEDROM_MEDIA_TYPE);
        let svg = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap_or_default();
        assert_eq!(svg, FAKE_SVG.as_bytes());
        let bad = Typed(ShowWavedromTool::new(ports(Arc::new(
            FakeFormats::failing("invalid WaveJSON"),
        ))));
        let err = bad.prepare(json!({ "source": "x" }), &ctx()).await;
        assert!(
            matches!(err, Err(ToolError::InvalidInput { message, .. }) if message == "invalid WaveJSON")
        );
    }
}
