//! Report tools, thin [`TypedTool`]s over the [`FormatsPort`]:
//! `show_test_report` (a JUnit XML file as a test report block),
//! `show_findings` (a SARIF log as a findings block) and `show_wavedrom`
//! (WaveJSON as an SVG timing diagram). [`register_format_tools`] adds
//! them and [`FORMAT_TOOLS_PROMPT`] tells the model when to use each.
//!
//! Files are read through the file port, with the workspace and
//! sensitive-path guard of `read_file`, and parsed through the
//! [`Offload`] so a large report never blocks the executor. Locations the
//! report gives as absolute paths inside the workspace are made relative
//! to it, so the block can open them. The model gets the totals and the
//! failing tests or the findings as `file:line` lines (capped), enough to
//! act on, not the whole report. They only read: no call needs approval.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use domain::block::{BlockKind, ImageData, SourceRef};
use domain::permission::ToolEffect;
use domain::report::{FindingSeverity, FindingsSpec, TestOutcome, TestReportSpec};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use crate::ports::formats::FormatsPort;
use crate::ports::fs::FileSystemPort;
use crate::usecases::compute::{Offload, offloaded};

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
    /// A JUnit XML report of the workspace, e.g.
    /// `target/nextest/default/junit.xml` or `reports/pytest.xml`.
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
        "Shows a JUnit XML test report of the workspace as a test report in \
         the user's timeline: totals by outcome, failures first with their \
         message and stack trace, durations and links to the test files. \
         Most runners write one (cargo nextest `--profile ci`, pytest \
         `--junitxml`, Maven Surefire, Jest/Vitest junit reporters, \
         gotestsum `--junitfile`). Returns the totals and the failing tests \
         with their locations: do not repeat the report in your reply."
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
            }
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = findings_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Findings(spec)))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
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
        let _ = write!(
            out,
            "\n- {at}: {}{rule} {}",
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
        let block = BlockKind::Image {
            media_type: "image/svg+xml".into(),
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

/// Tells the model when to use the report tools; appended to the system
/// prompt after the component tools.
pub const FORMAT_TOOLS_PROMPT: &str = "\
# Reports and diagrams

- `show_test_report`: after a test run that wrote a JUnit XML report, show \
it instead of pasting the output: the user sees failures first with their \
traces. Ask the runner for JUnit output when it can write it (cargo nextest, \
pytest `--junitxml`, Surefire, Jest/Vitest, gotestsum).
- `show_findings`: show a SARIF log of a linter or scanner (clippy-sarif, \
semgrep, CodeQL, ESLint) grouped by file and severity.
- `show_wavedrom`: draw a digital timing diagram from WaveJSON (clocks, \
handshakes, bus transfers over cycles).

Their results list what failed with `file:line`, so you can open the files \
and fix them; do not repeat the lists to the user.";

/// Registers `show_test_report`, `show_findings` and `show_wavedrom`.
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
    registry.register_typed(ShowWavedromTool::new(ports.clone()))
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
        let tool = Typed(ShowTestReportTool::new(ports));
        assert_eq!(tool.effect(&json!({ "path": "a" })), ToolEffect::Read);
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
        assert_eq!(media_type, "image/svg+xml");
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
