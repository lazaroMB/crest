//! `show_waveform`: the signals of a VCD, FST or GHW simulation dump as
//! digital waves.
//!
//! The model gets the shape of the dump (format, timescale, time range,
//! signal count, scopes), the clocks with their periods, the signals
//! with unknown or high-impedance bits and each signal's value at the
//! end of the dump, never the full change lists.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::waveform::{Radix, SignalKind, WaveformSpec};
use domain::permission::ToolEffect;
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, one_line};
use crate::ports::formats::WaveRequest;
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest dump read, in bytes.
pub const MAX_WAVE_BYTES: u64 = 256 * 1024 * 1024;
/// Signal patterns a call may give.
pub const MAX_PATTERNS: usize = 64;

/// Input of `show_waveform`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowWaveform {
    /// A waveform dump of the workspace: `.vcd` (Verilator, Icarus,
    /// cocotb, GHDL `--vcd`), `.fst` (GTKWave, Verilator `--trace-fst`)
    /// or `.ghw` (GHDL `--wave`).
    pub path: String,
    /// Signals to show: dotted paths or patterns with `*` (`tb.dut.*`,
    /// `*clk*`). All signals (up to 2000) when empty.
    #[serde(default)]
    pub signals: Vec<String>,
    /// How buses are written at first: `hex` (default), `bin` or `dec`;
    /// the user can switch.
    #[serde(default)]
    pub radix: Radix,
    /// Title shown above the waves; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_waveform`: a simulation dump as digital waves.
#[derive(Clone)]
pub struct ShowWaveformTool {
    ports: FormatToolPorts,
}

impl ShowWaveformTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_waveform`.
pub const SHOW_WAVEFORM: &str = "show_waveform";

/// Reads a dump of the workspace (up to [`MAX_WAVE_BYTES`]) through the
/// guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_wave(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("waveform path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_WAVE_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowWaveformTool {
    type Input = ShowWaveform;

    fn name(&self) -> &str {
        SHOW_WAVEFORM
    }

    fn description(&self) -> &str {
        "Shows a digital simulation dump (VCD, FST or GHW in `path`) as \
         waves in the user's timeline: the signal tree by scope, one-bit \
         signals as high/low traces with x and z marked, buses with their \
         values in hex, binary or decimal, zoom and pan in time and a \
         cursor that reads every signal's value at that time. Returns the \
         timescale, time range, scopes, clocks with their periods, signals \
         with unknown bits and the values at the end: do not repeat the \
         waves in your reply."
    }

    fn effect(&self, _input: &ShowWaveform) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowWaveform,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_WAVEFORM, "`path` is empty"));
        }
        if input.signals.len() > MAX_PATTERNS {
            return Err(invalid(
                SHOW_WAVEFORM,
                format!("at most {MAX_PATTERNS} signal patterns"),
            ));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_wave(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let request = WaveRequest {
                path: input.path.clone(),
                signals: input
                    .signals
                    .iter()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect(),
                radix: input.radix,
            };
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_waveform(&bytes, &request)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the waveform reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = waveform_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Waveform(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_waveform`.
#[must_use]
pub fn waveform_summary(spec: &WaveformSpec) -> String {
    let mut out = format!(
        "{} waveform `{}` shown to the user: timescale {}, {} to {}, {} of {} signals",
        spec.format.label(),
        spec.path,
        spec.timescale.label(),
        spec.time_text(spec.start),
        spec.time_text(spec.end),
        spec.signals.len(),
        spec.total_signals,
    );
    if let Some(generator) = &spec.generator {
        let _ = write!(out, " (written by {})", one_line(generator, 80));
    }
    out.push('.');
    if spec.truncated() {
        out.push_str(" Some signals were cut short to stay under the change limit.");
    }
    let scopes: Vec<&str> = spec
        .scopes()
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if !scopes.is_empty() {
        let listed: Vec<&str> = scopes.iter().take(MAX_LISTED).copied().collect();
        let _ = write!(out, "\nScopes: {}", listed.join(", "));
        if scopes.len() > MAX_LISTED {
            let _ = write!(out, " and {} more", scopes.len() - MAX_LISTED);
        }
    }
    let clocks: Vec<String> = spec
        .signals
        .iter()
        .filter_map(|s| {
            s.clock_period().map(|p| {
                // A tick count fits an f64 for display.
                #[allow(clippy::cast_precision_loss)]
                let hz = 1.0 / (p as f64 * spec.timescale.seconds());
                format!("{} (period {}, {})", s.path(), spec.time_text(p), hertz(hz))
            })
        })
        .take(MAX_LISTED)
        .collect();
    if !clocks.is_empty() {
        let _ = write!(out, "\nClocks: {}", clocks.join(", "));
    }
    let unknown: Vec<String> = spec
        .signals
        .iter()
        .filter(|s| s.has_unknown())
        .map(domain::engineering::waveform::WaveSignal::path)
        .take(MAX_LISTED)
        .collect();
    if !unknown.is_empty() {
        let _ = write!(
            out,
            "\nWith x or z values at some time: {}",
            unknown.join(", ")
        );
    }
    let finals: Vec<String> = spec
        .signals
        .iter()
        .filter(|s| s.kind != SignalKind::Event)
        .take(MAX_LISTED)
        .map(|s| {
            let value = s
                .formatted_at(spec.end, spec.radix)
                .unwrap_or_else(|| "no value".to_owned());
            format!(
                "{} = {} ({} changes)",
                s.path(),
                one_line(&value, 40),
                s.toggles()
            )
        })
        .collect();
    if !finals.is_empty() {
        let _ = write!(
            out,
            "\nAt {} ({}): {}",
            spec.time_text(spec.end),
            spec.radix.label(),
            finals.join("; ")
        );
        if spec.signals.len() > MAX_LISTED {
            let _ = write!(out, "; {} more signals", spec.signals.len() - MAX_LISTED);
        }
    }
    out
}

/// A frequency with an SI prefix (`100 MHz`).
fn hertz(hz: f64) -> String {
    let (scale, suffix) = [(1e9, "GHz"), (1e6, "MHz"), (1e3, "kHz"), (1.0, "Hz")]
        .into_iter()
        .find(|(s, _)| hz >= *s)
        .unwrap_or((1.0, "Hz"));
    let value = format!("{:.3}", hz / scale);
    let value = value.trim_end_matches('0').trim_end_matches('.');
    format!("{value} {suffix}")
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
        let fs = InMemoryFs::new(test_root()).with_file("sim/tb.vcd", "$date $end");
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

    #[test]
    fn definition() {
        let tool = Typed(ShowWaveformTool::new(ports(Arc::new(FakeFormats::new()))));
        insta::assert_json_snapshot!(
            "show_waveform_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_clocks_and_final_values() {
        let tool = Typed(ShowWaveformTool::new(ports(Arc::new(FakeFormats::new()))));
        let out = call(&tool, json!({ "path": "sim/tb.vcd" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "VCD waveform `sim/tb.vcd` shown to the user: timescale 1 ns, 0 to 30 ns, 2 of 2 \
             signals.\n\
             Scopes: tb\n\
             Clocks: tb.clk (period 10 ns, 100 MHz)\n\
             At 30 ns (hex): tb.clk = 1 (5 changes); tb.count[7:0] = 2 (2 changes)"
        );
        assert!(matches!(out.block_update, Some(BlockKind::Waveform(_))));
        let missing = call(&tool, json!({ "path": "sim/none.vcd" }))
            .await
            .unwrap();
        assert!(missing.is_error);
        let empty = tool.prepare(json!({ "path": "" }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
    }
}
