//! `show_array_file`: an HDF5, NetCDF-4 or NetCDF-3 file's groups and
//! variables with bounded previews.
//!
//! The model gets the format, the variables with their types and shapes
//! (capped), and for each preview its slice and the statistics of that
//! slice with the envelope id, never the values.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::arrays::{ArrayFileSpec, ArrayNodeKind, PreviewKind};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line};
use crate::ports::formats::ArrayRequest;
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest array file read, in bytes: the reader needs the whole file in
/// memory (previews decode only their rows).
pub const MAX_ARRAY_BYTES: u64 = 512 * 1024 * 1024;
/// Variables listed to the model.
const LISTED: usize = 30;

/// Input of `show_array_file`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowArrayFile {
    /// An HDF5 (`.h5`, `.hdf5`, `.he5`), NetCDF-4 or NetCDF-3 (`.nc`) file
    /// of the workspace.
    pub path: String,
    /// The variable to preview (`/group/name` or a NetCDF name); the
    /// first few numeric ones when absent.
    #[serde(default)]
    pub variable: Option<String>,
    /// Title shown above the inspector.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_array_file`: an array file inspector.
#[derive(Clone)]
pub struct ShowArrayFileTool {
    ports: FormatToolPorts,
}

impl ShowArrayFileTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_array_file`.
pub const SHOW_ARRAY_FILE: &str = "show_array_file";

/// Reads an array file of the workspace (up to [`MAX_ARRAY_BYTES`])
/// through the guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_array(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("array file `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_ARRAY_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowArrayFileTool {
    type Input = ShowArrayFile;

    fn name(&self) -> &str {
        SHOW_ARRAY_FILE
    }

    fn description(&self) -> &str {
        "Inspects an HDF5, NetCDF-4 or NetCDF-3 file (`path`) in the user's \
         timeline: the group and variable tree with types, shapes, \
         dimension names, chunking, compression and attributes, and bounded \
         previews (the first values of a 1D variable as a line, the first \
         2D slice of a higher one as a heatmap) of the first few numeric \
         variables or of `variable`. Statistics are computed on the preview \
         only and say which slice; never present them as the whole \
         variable's. Returns the structure and the preview statistics."
    }

    fn effect(&self, _input: &ShowArrayFile) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowArrayFile,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_ARRAY_FILE, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_array(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let request = ArrayRequest {
                path: input.path.clone(),
                variable: input.variable.filter(|v| !v.trim().is_empty()),
            };
            let formats = Arc::clone(&ports.formats);
            let parsed = offloaded(&ports.offload, move || {
                formats.inspect_array_file(&bytes, &request)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => {
                    return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
                }
                None => return Ok(ToolOutput::error("the array reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = array_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::ArrayFile(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// `v` with up to 6 significant digits.
fn number(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return v.to_string();
    }
    let digits = (5 - v.abs().log10().floor() as i32).clamp(0, 10) as usize;
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// What the model gets back from `show_array_file`.
#[must_use]
pub fn array_summary(spec: &ArrayFileSpec) -> String {
    let groups = spec
        .nodes
        .iter()
        .filter(|n| n.kind == ArrayNodeKind::Group)
        .count();
    let variables: Vec<_> = spec.variables().collect();
    let mut out = format!(
        "{} file `{}` shown to the user ({} bytes): {groups} groups, {} variables{}.",
        spec.format.name(),
        one_line(&spec.display_title(), 80),
        spec.size,
        variables.len(),
        if spec.truncated { " (listing cut)" } else { "" }
    );
    out.push_str("\nVariables:");
    for n in variables.iter().take(LISTED) {
        let _ = write!(
            out,
            "\n- {} {} [{}]",
            n.path,
            n.dtype.as_deref().unwrap_or("?"),
            n.shape_text()
        );
        if let Some(c) = &n.chunks {
            let c: Vec<String> = c.iter().map(u64::to_string).collect();
            let _ = write!(out, " chunks {}", c.join("×"));
        }
        if !n.filters.is_empty() {
            let _ = write!(out, " {}", n.filters.join("+"));
        }
        for key in ["units", "long_name"] {
            if let Some(v) = n.attr(key) {
                let _ = write!(out, " {key}={}", one_line(v, 40));
            }
        }
    }
    if variables.len() > LISTED {
        let _ = write!(out, "\n… {} more", variables.len() - LISTED);
    }
    for p in &spec.previews {
        let v = &p.stats.values;
        let _ = write!(
            out,
            "\nPreview of {} {} ({}; envelope `{}`, statistics of this slice only",
            p.path,
            p.selection,
            match p.kind {
                PreviewKind::Heatmap => "heatmap",
                PreviewKind::Line => "line",
                PreviewKind::Scalar => "value",
                PreviewKind::Text => "strings",
            },
            p.stats.id
        );
        out.push_str("):");
        if p.kind == PreviewKind::Text {
            let shown: Vec<String> = p.texts.iter().take(5).map(|t| one_line(t, 40)).collect();
            let _ = write!(out, " {}", shown.join(" | "));
            continue;
        }
        for key in ["n", "missing", "min", "max", "mean", "sd"] {
            if let Some(x) = v.get(key) {
                let _ = write!(out, " {key} {}", number(x.v));
            }
        }
        if let Some(d) = &p.decoding {
            let _ = write!(out, "; {d}");
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
        let fs = InMemoryFs::new(test_root()).with_file("data/run.h5", "\u{89}HDF");
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
        let tool = Typed(ShowArrayFileTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_array_file_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        assert_eq!(number(1964.5), "1964.5");
        assert_eq!(number(0.000_123_456_78), "0.000123457");
    }

    #[tokio::test]
    async fn the_model_gets_structure_and_preview_statistics() {
        let tool = Typed(ShowArrayFileTool::new(ports()));
        let out = call(&tool, json!({ "path": "data/run.h5" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "HDF5 file `run.h5` shown to the user (4096 bytes): 1 groups, 1 variables.\n\
             Variables:\n\
             - /temp f32 [2 × 3]\n\
             Preview of /temp [0:2, 0:3] (heatmap; envelope `ar_fake`, statistics of this \
             slice only): n 5 missing 1 min 1 max 6 mean 3.6"
        );
        assert!(matches!(out.block_update, Some(BlockKind::ArrayFile(_))));
        assert!(call(&tool, json!({ "path": " " })).await.is_err());
        let missing = call(&tool, json!({ "path": "data/none.h5" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
