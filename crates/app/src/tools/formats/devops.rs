//! Developer and operations viewers: `show_trace` (a distributed trace
//! as a waterfall), `show_flamegraph` (a profile as a flame graph) and
//! `show_logs` (a log file on a filterable list).
//!
//! Each reads a workspace file through the guard of `read_file`, parses
//! it on the [`Offload`](crate::usecases::compute::Offload) and puts the
//! block in the timeline. The model gets a summary, never the data: span
//! counts, the critical path, the spans with the most self time and the
//! errors; the hottest path and the frames with the most self time; the
//! level counts and the error messages grouped by shape.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::flame::FlameSpec;
use domain::devops::logs::{LogFormat, LogSpec, MAX_LINES};
use domain::devops::trace::{STATUS_MESSAGE, TraceSpec};
use domain::permission::ToolEffect;
use domain::widget::timeline::{TimeUnit, format_duration};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, mib, one_line, read};
use crate::ports::formats::{LogRequest, ProfileRequest};
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// How much of a log file `show_logs` reads; a larger file is shown from
/// its head and marked cut.
pub const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
/// Entries of the critical path listed to the model.
pub const MAX_PATH_LISTED: usize = 12;
/// Spans, frames or patterns listed to the model in a "top" list.
pub const MAX_TOP: usize = 8;

/// A non-empty title, if given.
fn title(input: Option<String>) -> Option<String> {
    input.filter(|t| !t.trim().is_empty())
}

fn ms(value: f64) -> String {
    format_duration(value, TimeUnit::Ms)
}

fn percent(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".into();
    }
    #[allow(clippy::cast_precision_loss)]
    let share = part as f64 * 100.0 / total as f64;
    format!("{share:.1}%")
}

// show_trace

/// Input of `show_trace`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTrace {
    /// A trace file of the workspace: OTLP JSON (an
    /// `ExportTraceServiceRequest`, or the collector file exporter's JSON
    /// lines), a Jaeger JSON download or Zipkin v2 JSON.
    pub path: String,
    /// Title shown above the waterfall; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_trace`: a trace file as a waterfall.
#[derive(Clone)]
pub struct ShowTraceTool {
    ports: FormatToolPorts,
}

impl ShowTraceTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_trace`.
pub const SHOW_TRACE: &str = "show_trace";

#[async_trait]
impl TypedTool for ShowTraceTool {
    type Input = ShowTrace;

    fn name(&self) -> &str {
        SHOW_TRACE
    }

    fn description(&self) -> &str {
        "Shows a distributed trace file of the workspace (OTLP JSON, Jaeger \
         JSON or Zipkin v2 JSON) as a waterfall in the user's timeline: one \
         row per span under its parent, colored by service, failed spans in \
         red, events as ticks, the critical path on a toggle and each \
         span's attributes on hover. Returns the span and service counts, \
         the critical path, the spans with the most self time and the \
         errors: do not repeat the trace in your reply."
    }

    fn effect(&self, _input: &ShowTrace) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowTrace,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_TRACE, "`path` is empty"));
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
                offloaded(&ports.offload, move || formats.parse_trace(&bytes, &path)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the trace parser did not run")),
            };
            spec.title = title(input.title);
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = trace_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Trace(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_trace`.
#[must_use]
pub fn trace_summary(path: &str, spec: &TraceSpec) -> String {
    let errors = spec.errors();
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut out = format!(
        "Trace `{path}` ({}) shown to the user: {}, {}",
        spec.format.name(),
        plural(spec.traces, "trace", "traces"),
        plural(spec.span_count(), "span", "spans"),
    );
    if spec.total_spans > spec.span_count() {
        let _ = write!(out, " (of {} in the file)", spec.total_spans);
    }
    if !spec.services.is_empty() {
        let _ = write!(
            out,
            ", {} ({})",
            plural(spec.services.len(), "service", "services"),
            spec.services.join(", ")
        );
    }
    let _ = write!(
        out,
        ", {} end to end, {}.",
        ms(spec.duration()),
        plural(errors.len(), "error", "errors")
    );
    let times = spec.times(None);
    if !spec.critical_path.is_empty() {
        let steps: Vec<String> = spec
            .critical_path
            .iter()
            .filter_map(|id| spec.timeline.rows.iter().position(|r| &r.id == id))
            .take(MAX_PATH_LISTED)
            .map(|ix| format!("{} {}", spec.span_name(ix), ms(times[ix].duration)))
            .collect();
        let _ = write!(out, "\nCritical path: {}", steps.join(" → "));
        if spec.critical_path.len() > MAX_PATH_LISTED {
            let _ = write!(
                out,
                " → … {} more",
                spec.critical_path.len() - MAX_PATH_LISTED
            );
        }
    }
    let top = spec.times(Some(MAX_TOP));
    if !top.is_empty() {
        out.push_str("\nMost self time:");
        for t in top {
            let _ = write!(
                out,
                "\n- {}: {} self of {}",
                spec.span_name(t.row),
                ms(t.self_time),
                ms(t.duration)
            );
        }
    }
    if !errors.is_empty() {
        out.push_str("\nErrors:");
        for &ix in errors.iter().take(MAX_LISTED) {
            let _ = write!(
                out,
                "\n- {} ({})",
                spec.span_name(ix),
                ms(times[ix].duration)
            );
            if let Some(message) = spec.attr(ix, STATUS_MESSAGE) {
                let _ = write!(out, ": {}", one_line(&message, 160));
            }
        }
        if errors.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more", errors.len() - MAX_LISTED);
        }
    }
    out
}

// show_flamegraph

/// Input of `show_flamegraph`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowFlamegraph {
    /// A profile of the workspace: folded stacks (`a;b;c 12` lines from
    /// `stackcollapse-perf.pl`, `cargo flamegraph`, py-spy `--format raw`,
    /// async-profiler `collapsed`) or a pprof profile (`.pb.gz`, `.pprof`).
    pub path: String,
    /// The pprof sample type to draw, e.g. `cpu`, `alloc_space`,
    /// `inuse_space`; the profile's default when omitted.
    #[serde(default)]
    pub sample_type: Option<String>,
    /// Title shown above the flame graph; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_flamegraph`: a profile as a flame graph.
#[derive(Clone)]
pub struct ShowFlamegraphTool {
    ports: FormatToolPorts,
}

impl ShowFlamegraphTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_flamegraph`.
pub const SHOW_FLAMEGRAPH: &str = "show_flamegraph";

#[async_trait]
impl TypedTool for ShowFlamegraphTool {
    type Input = ShowFlamegraph;

    fn name(&self) -> &str {
        SHOW_FLAMEGRAPH
    }

    fn description(&self) -> &str {
        "Shows a CPU or memory profile of the workspace (folded stacks or \
         pprof) as a flame graph in the user's timeline: callers above \
         callees, widths by value, click a frame to zoom, search to \
         highlight, and a table of the functions with the most self time. \
         Returns the total, the hottest path and the top functions by self \
         time: do not repeat the profile in your reply."
    }

    fn effect(&self, _input: &ShowFlamegraph) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowFlamegraph,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_FLAMEGRAPH, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let request = ProfileRequest {
                path: input.path.clone(),
                sample_type: input.sample_type.filter(|s| !s.trim().is_empty()),
            };
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_profile(&bytes, &request)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the profile parser did not run")),
            };
            spec.title = title(input.title);
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = flame_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::FlameGraph(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_flamegraph`.
#[must_use]
pub fn flame_summary(path: &str, spec: &FlameSpec) -> String {
    let total = spec.total();
    let mut out = format!(
        "Flame graph of `{path}` ({}) shown to the user: {total} {} in total, {} frames, \
         stacks up to {} deep",
        spec.format.name(),
        spec.unit,
        spec.nodes.len().saturating_sub(1),
        spec.max_depth(),
    );
    if spec.hidden > 0 {
        let _ = write!(out, " ({} tiny frames hidden)", spec.hidden);
    }
    out.push('.');
    let hot = spec.hottest_path();
    if !hot.is_empty() {
        let steps: Vec<String> = hot
            .iter()
            .take(MAX_PATH_LISTED)
            .map(|&ix| {
                let node = &spec.nodes[ix];
                format!("{} {}", node.name, percent(node.total, total))
            })
            .collect();
        let _ = write!(out, "\nHottest path: {}", steps.join(" → "));
        if hot.len() > MAX_PATH_LISTED {
            let _ = write!(out, " → … {} more", hot.len() - MAX_PATH_LISTED);
        }
    }
    let stats = spec.frame_stats();
    if !stats.is_empty() {
        out.push_str("\nMost self time:");
        for stat in stats.iter().take(MAX_TOP) {
            let _ = write!(
                out,
                "\n- {}: {} self, {} total",
                one_line(&stat.name, 120),
                percent(stat.self_value, total),
                percent(stat.total, total)
            );
        }
    }
    out
}

// show_logs

/// How `show_logs` reads the lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogFormatInput {
    /// One JSON object per line.
    Json,
    /// `key=value` pairs.
    Logfmt,
    /// Plain text.
    Text,
}

/// Input of `show_logs`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowLogs {
    /// A log file of the workspace: JSON lines (pino, bunyan, zap, slog,
    /// tracing JSON), logfmt or plain text (log4j/logback, Python logging,
    /// env_logger, tracing's default format).
    pub path: String,
    /// How the lines are written; told from the first lines when omitted.
    #[serde(default)]
    pub format: Option<LogFormatInput>,
}

/// `show_logs`: a log file on a filterable list.
#[derive(Clone)]
pub struct ShowLogsTool {
    ports: FormatToolPorts,
}

impl ShowLogsTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_logs`.
pub const SHOW_LOGS: &str = "show_logs";

/// Reads the head of a log file (up to [`MAX_LOG_BYTES`]): the bytes and
/// whether the file is longer.
///
/// # Errors
///
/// A message for the caller: a bad path, a file the guard refuses or one
/// that cannot be read.
pub async fn read_log(fs: &dyn FileSystemPort, path: &str) -> Result<(Vec<u8>, bool), String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("log path `{path}`: {e}"))?;
    let (bytes, size) = fs
        .read_head(&parsed, MAX_LOG_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    let truncated = (bytes.len() as u64) < size;
    Ok((bytes, truncated))
}

impl From<LogFormatInput> for LogFormat {
    fn from(format: LogFormatInput) -> Self {
        match format {
            LogFormatInput::Json => Self::Json,
            LogFormatInput::Logfmt => Self::Logfmt,
            LogFormatInput::Text => Self::Text,
        }
    }
}

#[async_trait]
impl TypedTool for ShowLogsTool {
    type Input = ShowLogs;

    fn name(&self) -> &str {
        SHOW_LOGS
    }

    fn description(&self) -> &str {
        "Shows a log file of the workspace (JSON lines, logfmt or plain \
         text) in the user's timeline as a filterable list: level colors, \
         chips by level and by fields such as the service, a text or \
         /regex/ filter, a histogram of lines over time, and each line's \
         fields and stack trace on click. Returns the line count, the \
         counts per level, the facets and the error messages grouped by \
         shape: do not repeat the log in your reply."
    }

    fn effect(&self, _input: &ShowLogs) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowLogs,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_LOGS, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let (bytes, truncated) = match read_log(ports.fs.as_ref(), &input.path).await {
                Ok(read) => read,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let request = LogRequest {
                path: input.path.clone(),
                format: input.format.map(LogFormat::from),
                truncated,
            };
            let parsed =
                offloaded(&ports.offload, move || formats.parse_logs(&bytes, &request)).await;
            let spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the log parser did not run")),
            };
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = logs_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Logs(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_logs`.
#[must_use]
pub fn logs_summary(path: &str, spec: &LogSpec) -> String {
    let items = &spec.list.items;
    let levels: Vec<String> = spec
        .level_counts()
        .into_iter()
        .map(|(level, n)| {
            if level == "-" {
                format!("{n} without level")
            } else {
                format!("{level} {n}")
            }
        })
        .collect();
    let mut out = format!(
        "Log `{path}` ({}) shown to the user: {} lines",
        spec.format.name(),
        spec.total_lines
    );
    if !levels.is_empty() {
        let _ = write!(out, " ({})", levels.join(", "));
    }
    let first = items.iter().find_map(|i| i.ts.as_deref());
    let last = items.iter().rev().find_map(|i| i.ts.as_deref());
    if let (Some(first), Some(last)) = (first, last) {
        let _ = write!(out, ", from {first} to {last}");
    }
    out.push('.');
    if spec.total_lines > items.len() {
        let _ = write!(
            out,
            " Only the first {} lines are in the block (at most {MAX_LINES}).",
            items.len()
        );
    }
    if spec.truncated {
        let _ = write!(
            out,
            " The file is larger than {}: only its head was read.",
            mib(MAX_LOG_BYTES)
        );
    }
    for facet in &spec.list.facets {
        let mut values: Vec<(String, usize)> = Vec::new();
        for item in items {
            if let Some(value) = item.field_text(facet) {
                match values.iter_mut().find(|(v, _)| *v == value) {
                    Some((_, n)) => *n += 1,
                    None => values.push((value, 1)),
                }
            }
        }
        values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let shown: Vec<String> = values
            .iter()
            .take(MAX_TOP)
            .map(|(v, n)| format!("{v} {n}"))
            .collect();
        let _ = write!(out, "\n{facet}: {}", shown.join(", "));
    }
    let patterns = spec.error_patterns();
    if !patterns.is_empty() {
        out.push_str("\nErrors by message shape:");
        for (pattern, count, first) in patterns.iter().take(MAX_LISTED) {
            let item = &items[*first];
            let _ = write!(out, "\n- {count}× {}", one_line(pattern, 160));
            match &item.ts {
                Some(ts) => {
                    let _ = write!(out, " (first at {ts}, record {})", first + 1);
                }
                None => {
                    let _ = write!(out, " (first is record {})", first + 1);
                }
            }
        }
        if patterns.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more shapes", patterns.len() - MAX_LISTED);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root())
            .with_file("traces/t.json", "{}")
            .with_file("prof/cpu.folded", "main 1\n")
            .with_file("logs/app.log", "x\n")
            .with_file(
                "logs/huge.log",
                "x\n".repeat(usize::try_from(MAX_LOG_BYTES).unwrap_or(0)),
            );
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
    fn definitions() {
        let ports = ports(Arc::new(FakeFormats::new()));
        insta::assert_json_snapshot!(
            "show_trace_definition",
            definition(ShowTraceTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!(
            "show_flamegraph_definition",
            definition(ShowFlamegraphTool::new(ports.clone()))
        );
        insta::assert_json_snapshot!("show_logs_definition", definition(ShowLogsTool::new(ports)));
    }

    #[tokio::test]
    async fn a_trace_is_a_block_and_the_model_gets_its_shape() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowTraceTool::new(ports(Arc::clone(&formats))));
        let out = call(
            &tool,
            json!({ "path": "traces/t.json", "title": "checkout" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Trace `traces/t.json` (OTLP) shown to the user: 1 trace, 3 spans, 3 services \
             (api, payments, db), 100 ms end to end, 1 error.\n\
             Critical path: api: GET /checkout 100 ms → payments: charge 80 ms → db: SELECT cards 20 ms\n\
             Most self time:\n\
             - payments: charge: 60 ms self of 80 ms\n\
             - api: GET /checkout: 20 ms self of 100 ms\n\
             - db: SELECT cards: 20 ms self of 20 ms\n\
             Errors:\n\
             - payments: charge (80 ms): card declined"
        );
        let Some(BlockKind::Trace(spec)) = out.block_update else {
            panic!("expected a trace, got {:?}", out.block_update);
        };
        assert_eq!(spec.title.as_deref(), Some("checkout"));
        assert_eq!(
            formats.calls(),
            [("parse_trace".to_owned(), "traces/t.json".to_owned())]
        );
    }

    #[tokio::test]
    async fn a_profile_reports_the_hot_path_and_top_frames() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowFlamegraphTool::new(ports(Arc::clone(&formats))));
        let out = call(
            &tool,
            json!({ "path": "prof/cpu.folded", "sample_type": "cpu" }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Flame graph of `prof/cpu.folded` (folded stacks) shown to the user: 100 samples in \
             total, 3 frames, stacks up to 2 deep.\n\
             Hottest path: main 100.0% → work 70.0%\n\
             Most self time:\n\
             - work: 70.0% self, 70.0% total\n\
             - idle: 30.0% self, 30.0% total\n\
             - main: 0.0% self, 100.0% total"
        );
        assert!(matches!(out.block_update, Some(BlockKind::FlameGraph(_))));
    }

    #[tokio::test]
    async fn logs_report_levels_facets_and_error_shapes() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowLogsTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "logs/app.log", "format": "logfmt" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Log `logs/app.log` (logfmt) shown to the user: 3 lines (error 2, info 1), from \
             2026-10-24T13:05:07Z to 2026-10-24T13:05:09Z.\n\
             service: db 3\n\
             Errors by message shape:\n\
             - 2× db down after <n> tries (first at 2026-10-24T13:05:08Z, record 2)"
        );
        let Some(BlockKind::Logs(spec)) = out.block_update else {
            panic!("expected logs, got {:?}", out.block_update);
        };
        assert!(!spec.truncated);
        let big = call(&tool, json!({ "path": "logs/huge.log" }))
            .await
            .unwrap();
        assert!(
            big.text_content().contains("only its head was read"),
            "{}",
            big.text_content()
        );
    }

    #[tokio::test]
    async fn bad_inputs_and_files_are_errors() {
        let tool = Typed(ShowTraceTool::new(ports(Arc::new(FakeFormats::new()))));
        let empty = tool.prepare(json!({ "path": " " }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
        let missing = call(&tool, json!({ "path": "nope.json" })).await.unwrap();
        assert!(missing.is_error);
        let failing = Typed(ShowLogsTool::new(ports(Arc::new(FakeFormats::failing(
            "`logs/app.log` has no log lines",
        )))));
        let out = call(&failing, json!({ "path": "logs/app.log" }))
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.block_update.is_none());
        let unknown = Typed(ShowLogsTool::new(ports(Arc::new(FakeFormats::new()))))
            .prepare(json!({ "path": "a", "format": "xml" }), &ctx())
            .await;
        assert!(matches!(unknown, Err(ToolError::InvalidInput { .. })));
    }
}
