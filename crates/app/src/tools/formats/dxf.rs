//! `show_dxf`: a 2D DXF drawing with its layers, zoom and pan.
//!
//! The model gets the file's release, units and extents, the layers with
//! their entity counts and whether they are on, the entity kinds, the
//! inserts expanded, what was left out and the drawing's texts (a title
//! block's fields), never the geometry.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::drawing::{DrawingSpec, Extents};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, one_line, read_report};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Texts listed to the model.
pub const MAX_TEXTS: usize = 40;

/// Input of `show_dxf`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowDxf {
    /// A DXF file of the workspace (`.dxf`, ASCII or binary, R12 to
    /// R2018), e.g. an export of LibreCAD, QCAD, FreeCAD, KiCad or a
    /// script (ezdxf).
    pub path: String,
    /// Title shown above the drawing; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_dxf`: a DXF drawing in a 2D view.
#[derive(Clone)]
pub struct ShowDxfTool {
    ports: FormatToolPorts,
}

impl ShowDxfTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_dxf`.
pub const SHOW_DXF: &str = "show_dxf";

#[async_trait]
impl TypedTool for ShowDxfTool {
    type Input = ShowDxf;

    fn name(&self) -> &str {
        SHOW_DXF
    }

    fn description(&self) -> &str {
        "Shows a 2D DXF drawing (`path`) in the user's timeline: lines, \
         arcs, circles, polylines, text and expanded block inserts in their \
         layer colors, with a layer list to turn layers on and off, zoom, \
         pan and fit, and the extents and units. Returns the release, \
         units, extents, layers with their entity counts, the entity kinds, \
         what was left out (hatches, dimensions…) and the drawing's texts: \
         cite those instead of reading the DXF text, and do not repeat the \
         lists in your reply."
    }

    fn effect(&self, _input: &ShowDxf) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowDxf, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_DXF, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_report(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed = offloaded(&ports.offload, move || formats.parse_dxf(&bytes, &path)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the DXF reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = dxf_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Drawing(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// A coordinate for the summary: up to 4 decimals.
fn number(v: f64) -> String {
    let text = format!("{v:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn extents_text(x: &Extents, unit: &str) -> String {
    let [w, h] = x.size();
    format!(
        "{} × {} {unit} (from {}, {} to {}, {})",
        number(w),
        number(h),
        number(x.min[0]),
        number(x.min[1]),
        number(x.max[0]),
        number(x.max[1])
    )
}

/// What the model gets back from `show_dxf`.
#[must_use]
pub fn dxf_summary(spec: &DrawingSpec) -> String {
    let unit = spec.units.as_deref().unwrap_or("drawing units");
    let mut out = format!("DXF drawing `{}`", spec.path);
    if let Some(v) = &spec.version {
        let _ = write!(out, " ({v})");
    }
    let _ = write!(
        out,
        " shown to the user: {} entities on {} layers",
        spec.entities.len(),
        spec.layers.len()
    );
    if spec.inserts > 0 {
        let _ = write!(out, ", {} block inserts expanded", spec.inserts);
    }
    match &spec.units {
        Some(u) => {
            let _ = write!(out, "; units {u}");
        }
        None => out.push_str("; the file sets no units ($INSUNITS)"),
    }
    out.push('.');
    if spec.truncated {
        out.push_str(" The drawing was cut at the entity limit.");
    }
    if let Some(x) = spec.computed_extents() {
        let _ = write!(out, "\nExtents of the geometry: {}", extents_text(&x, unit));
    }
    if let Some(x) = &spec.header_extents {
        let _ = write!(out, "\nExtents in the header: {}", extents_text(x, unit));
    }
    let kinds: Vec<String> = spec
        .kind_counts()
        .iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
    let _ = write!(out, "\nEntities: {}", kinds.join(", "));
    if !spec.skipped.is_empty() {
        let skipped: Vec<String> = spec
            .skipped
            .iter()
            .take(MAX_LISTED)
            .map(|s| format!("{} {}", s.count, s.kind))
            .collect();
        let _ = write!(out, "\nNot drawn: {}", skipped.join(", "));
    }
    out.push_str("\nLayers:");
    for l in spec.layers.iter().take(MAX_LISTED) {
        let _ = write!(
            out,
            "\n- {} (color {}{}): {} entities",
            one_line(&l.name, 60),
            l.color,
            if l.off { ", off" } else { "" },
            l.entities
        );
    }
    if spec.layers.len() > MAX_LISTED {
        let _ = write!(out, "\n… {} more layers", spec.layers.len() - MAX_LISTED);
    }
    let texts = spec.texts();
    if !texts.is_empty() {
        let listed: Vec<String> = texts
            .iter()
            .take(MAX_TEXTS)
            .map(|t| format!("\"{}\"", one_line(&t.replace('\n', " / "), 80)))
            .collect();
        let _ = write!(out, "\nTexts: {}", listed.join(", "));
        if texts.len() > MAX_TEXTS {
            let _ = write!(out, " and {} more", texts.len() - MAX_TEXTS);
        }
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 160));
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
        let fs = InMemoryFs::new(test_root()).with_file("dwg/plate.dxf", "0\nEOF\n");
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
        let tool = Typed(ShowDxfTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_dxf_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_layers_extents_and_texts() {
        let tool = Typed(ShowDxfTool::new(ports()));
        let out = call(&tool, json!({ "path": "dwg/plate.dxf" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "DXF drawing `dwg/plate.dxf` (R2000) shown to the user: 3 entities on 2 layers; \
             units mm.\n\
             Extents of the geometry: 100 × 50 mm (from 0, 0 to 100, 50)\n\
             Entities: 1 circle, 1 polyline, 1 text\n\
             Layers:\n\
             - Outline (color 5): 2 entities\n\
             - Holes (color 1, off): 1 entities\n\
             Texts: \"PLATE\""
        );
        assert!(matches!(out.block_update, Some(BlockKind::Drawing(_))));
        let missing = call(&tool, json!({ "path": "dwg/none.dxf" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
