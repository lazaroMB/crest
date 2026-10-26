//! `show_spectrum`: an IR, Raman, UV-Vis, NMR or mass spectrum on the
//! plot painter with the peaks the engine picked.
//!
//! The model gets the kind, axes, range, the picked peaks (positions and
//! heights, strongest first) and the envelope id to cite them by, never
//! the points.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::spectrum::{MAX_PEAKS, PeakSense, SpectrumKind, SpectrumSpec};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line};
use crate::ports::formats::SpectrumRequest;
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest spectrum file read, in bytes.
pub const MAX_SPECTRUM_BYTES: u64 = 128 * 1024 * 1024;

/// Input of `show_spectrum`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowSpectrum {
    /// A spectrum file of the workspace: JCAMP-DX (`.jdx`, `.dx`, `.jcamp`:
    /// IR, Raman, UV-Vis, NMR, MS peak tables), mzML or MGF (mass spectra).
    pub path: String,
    /// The data block (JCAMP-DX link files) or scan (mzML, MGF) to show,
    /// 1-based (default 1).
    #[serde(default)]
    pub block: Option<usize>,
    /// Smallest peak kept, as a share of the y range (or of the base peak
    /// for sticks), 0 to 1; default 0.05.
    #[serde(default)]
    pub threshold: Option<f64>,
    /// Most peaks listed, 1 to 100; default 20.
    #[serde(default)]
    pub max_peaks: Option<usize>,
    /// Title shown above the plot.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_spectrum`: a spectrum with its peaks.
#[derive(Clone)]
pub struct ShowSpectrumTool {
    ports: FormatToolPorts,
}

impl ShowSpectrumTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_spectrum`.
pub const SHOW_SPECTRUM: &str = "show_spectrum";

/// Reads a spectrum file of the workspace (up to [`MAX_SPECTRUM_BYTES`])
/// through the guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_spectrum(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("spectrum path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_SPECTRUM_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowSpectrumTool {
    type Input = ShowSpectrum;

    fn name(&self) -> &str {
        SHOW_SPECTRUM
    }

    fn description(&self) -> &str {
        "Shows a spectrum file (`path`) in the user's timeline on a plot with \
         zoom: JCAMP-DX for IR, Raman, UV-Vis and NMR (wavenumber and ppm \
         axes run right to left), mzML or MGF for mass spectra. The engine \
         picks the peaks (prominence for traces, minima for IR \
         transmittance, strongest sticks for MS). Returns the peaks with \
         their positions and the envelope id; cite these instead of reading \
         values off the data, and do not assign peaks the list does not \
         have."
    }

    fn effect(&self, _input: &ShowSpectrum) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowSpectrum,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_SPECTRUM, "`path` is empty"));
        }
        let block = match input.block {
            Some(0) => return Err(invalid(SHOW_SPECTRUM, "`block` counts from 1")),
            Some(b) => b - 1,
            None => 0,
        };
        if input.threshold.is_some_and(|t| !(0.0..1.0).contains(&t)) {
            return Err(invalid(SHOW_SPECTRUM, "`threshold` is a share from 0 to 1"));
        }
        if input.max_peaks.is_some_and(|m| m == 0 || m > MAX_PEAKS) {
            return Err(invalid(
                SHOW_SPECTRUM,
                format!("`max_peaks` is 1 to {MAX_PEAKS}"),
            ));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_spectrum(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let request = SpectrumRequest {
                path: input.path.clone(),
                block,
                threshold: input.threshold,
                max_peaks: input.max_peaks,
            };
            let formats = Arc::clone(&ports.formats);
            let parsed =
                offloaded(&ports.offload, move || formats.parse_spectrum(&bytes, &request)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => {
                    return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
                }
                None => return Ok(ToolOutput::error("the spectrum reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = spectrum_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Spectrum(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// `v` with the decimals peak positions of `kind` use.
fn position(v: f64, kind: SpectrumKind) -> String {
    match kind {
        SpectrumKind::Infrared | SpectrumKind::Raman | SpectrumKind::UvVis => format!("{v:.1}"),
        SpectrumKind::Mass => format!("{v:.4}"),
        SpectrumKind::Nmr | SpectrumKind::Other => format!("{v:.3}"),
    }
}

/// `v` with 4 significant digits.
fn height(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let digits = (3 - v.abs().log10().floor() as i32).clamp(0, 8) as usize;
    format!("{v:.digits$}")
}

/// What the model gets back from `show_spectrum`.
#[must_use]
pub fn spectrum_summary(spec: &SpectrumSpec) -> String {
    let mut out = format!(
        "{} spectrum `{}` shown to the user",
        spec.kind.name(),
        one_line(&spec.display_title(), 80)
    );
    if spec.blocks > 1 {
        let _ = write!(out, " (block {} of {})", spec.block + 1, spec.blocks);
    }
    let _ = write!(out, ": {} {}", spec.x.len(), if spec.sticks { "sticks" } else { "points" });
    if let Some((lo, hi)) = spec.x_range() {
        let _ = write!(
            out,
            ", {} from {} to {}",
            spec.x_label,
            position(lo, spec.kind),
            position(hi, spec.kind)
        );
    }
    let _ = write!(out, ", y: {}.", spec.y_label);
    if !spec.metadata.is_empty() {
        let meta: Vec<String> = spec
            .metadata
            .iter()
            .map(|(k, v)| format!("{k} {}", one_line(v, 60)))
            .collect();
        let _ = write!(out, " {}.", meta.join("; "));
    }
    let what = match (spec.sticks, spec.sense) {
        (true, _) => "strongest sticks",
        (false, PeakSense::Minima) => "minima (transmittance dips)",
        (false, PeakSense::Maxima) => "maxima",
    };
    if spec.peaks.is_empty() {
        let _ = write!(out, "\nNo peaks above the threshold (envelope `{}`).", spec.picking.id);
    } else {
        let _ = write!(
            out,
            "\n{} peaks picked as {what}, strongest first (envelope `{}`, keys \
             `peak.<n>.x`, `peak.<n>.y`):",
            spec.peaks.len(),
            spec.picking.id
        );
        for (i, p) in spec.peaks.iter().enumerate() {
            let _ = write!(
                out,
                "\n{}. {} → {}",
                i + 1,
                position(p.x, spec.kind),
                height(p.y)
            );
            if let Some(prom) = p.prominence {
                let _ = write!(out, " (prominence {})", height(prom));
            }
        }
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 200));
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

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("spectra/acetone.jdx", "##TITLE=x\n");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
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
        let tool = Typed(ShowSpectrumTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_spectrum_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        assert_eq!(height(0.2), "0.2000");
        assert_eq!(height(1234.5), "1234");
    }

    #[tokio::test]
    async fn the_model_gets_the_peaks() {
        let tool = Typed(ShowSpectrumTool::new(ports()));
        let out = call(&tool, json!({ "path": "spectra/acetone.jdx" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "IR spectrum `acetone` shown to the user: 5 points, wavenumber (cm⁻¹) from 400.0 to \
             4000.0, y: transmittance. origin crest.\n\
             1 peaks picked as minima (transmittance dips), strongest first (envelope \
             `ch_peaks`, keys `peak.<n>.x`, `peak.<n>.y`):\n\
             1. 1715.0 → 0.2000 (prominence 0.7000)"
        );
        assert!(matches!(out.block_update, Some(BlockKind::Spectrum(_))));
        assert!(call(&tool, json!({ "path": "x.jdx", "block": 0 })).await.is_err());
        assert!(call(&tool, json!({ "path": "x.jdx", "threshold": 2 })).await.is_err());
        let missing = call(&tool, json!({ "path": "spectra/none.jdx" })).await.unwrap();
        assert!(missing.is_error);
    }
}
