//! `show_step`: a STEP part (AP203, AP214, AP242) tessellated into the 3D
//! view, with its solids.
//!
//! The model gets what the file says (schema, CAD tool, length unit,
//! products), the part's box, and each solid's faces, triangles and box,
//! with the faces that could not be meshed. The mesh stays in the block.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::cad::CadModelSpec;
use domain::permission::ToolEffect;
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, one_line};
use crate::ports::formats::StepRequest;
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest STEP file read, in bytes (as the other 3D models).
pub const MAX_STEP_BYTES: u64 = crate::usecases::model3d::MAX_MODEL_BYTES;
/// Extensions of STEP files, lowercase.
pub const STEP_EXTENSIONS: [&str; 2] = ["step", "stp"];

/// Input of `show_step`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowStep {
    /// A STEP file of the workspace (`.step`, `.stp`; AP203, AP214 or
    /// AP242 B-rep), e.g. an export of CadQuery, build123d, FreeCAD,
    /// Onshape or KiCad.
    pub path: String,
    /// Chord tolerance of the tessellation in the file's length unit;
    /// smaller is finer. By default 0.05% of the part's size.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// Title shown above the view; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_step`: a STEP part in the 3D view.
#[derive(Clone)]
pub struct ShowStepTool {
    ports: FormatToolPorts,
}

impl ShowStepTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_step`.
pub const SHOW_STEP: &str = "show_step";

/// Reads a STEP file of the workspace (up to [`MAX_STEP_BYTES`]) through
/// the guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_step(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("STEP path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_STEP_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowStepTool {
    type Input = ShowStep;

    fn name(&self) -> &str {
        SHOW_STEP
    }

    fn description(&self) -> &str {
        "Shows a STEP part (`path`, AP203/AP214/AP242 B-rep) in a 3D view in \
         the user's timeline: tessellated solids with orbit, pan and zoom, a \
         list of solids the user can hide, a section plane and \
         point-to-point measuring in the file's unit. Returns the schema, \
         CAD tool, length unit, products, the part's box and each solid's \
         faces, triangles and box, with the faces that could not be meshed: \
         cite those sizes instead of guessing from the file text, and do \
         not repeat the list in your reply."
    }

    fn effect(&self, _input: &ShowStep) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowStep,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_STEP, "`path` is empty"));
        }
        if let Some(t) = input.tolerance
            && !(t.is_finite() && t > 0.0)
        {
            return Err(invalid(SHOW_STEP, "`tolerance` must be a positive number"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_step(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let request = StepRequest {
                path: input.path.clone(),
                tolerance: input.tolerance,
            };
            let parsed =
                offloaded(&ports.offload, move || formats.parse_step(&bytes, &request)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the STEP reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = step_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::CadModel(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// A length for the summary: up to 4 significant decimals.
fn length(v: f32) -> String {
    let text = format!("{v:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn size_text(size: [f32; 3], unit: &str) -> String {
    format!(
        "{} × {} × {} {unit}",
        length(size[0]),
        length(size[1]),
        length(size[2])
    )
}

/// What the model gets back from `show_step`.
#[must_use]
pub fn step_summary(spec: &CadModelSpec) -> String {
    let unit = spec.unit_label();
    let mut out = format!("STEP part `{}`", spec.path);
    match (spec.protocol(), &spec.schema) {
        (Some(ap), _) => {
            let _ = write!(out, " ({ap})");
        }
        (None, Some(schema)) => {
            let _ = write!(out, " (schema {})", one_line(schema, 60));
        }
        (None, None) => {}
    }
    if let Some(app) = &spec.application {
        let _ = write!(out, ", written by {}", one_line(app, 60));
    }
    let _ = write!(
        out,
        ", shown to the user: {} solids, {} faces, {} triangles (tolerance {} {unit})",
        spec.solids.len(),
        spec.faces(),
        spec.mesh.triangles.len(),
        length(spec.tolerance as f32),
    );
    match &spec.unit {
        Some(u) => {
            let _ = write!(out, "; length unit {u}");
        }
        None => out.push_str("; the file declares no length unit"),
    }
    out.push('.');
    if let Some((min, max)) = spec.bounds() {
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let _ = write!(
            out,
            "\nBox: {} (from {}, {}, {} to {}, {}, {})",
            size_text(size, unit),
            length(min[0]),
            length(min[1]),
            length(min[2]),
            length(max[0]),
            length(max[1]),
            length(max[2])
        );
    }
    if !spec.products.is_empty() {
        let listed: Vec<String> = spec
            .products
            .iter()
            .take(MAX_LISTED)
            .map(|p| one_line(p, 60))
            .collect();
        let _ = write!(out, "\nProducts: {}", listed.join(", "));
        if spec.products.len() > MAX_LISTED {
            let _ = write!(out, " and {} more", spec.products.len() - MAX_LISTED);
        }
    }
    let failed = spec.failed_faces();
    if failed > 0 {
        let _ = write!(
            out,
            "\n{failed} faces could not be meshed and are missing from the view; \
             sizes are of what was meshed."
        );
    }
    out.push_str("\nSolids:");
    for s in spec.solids.iter().take(MAX_LISTED) {
        let _ = write!(out, "\n- {}", one_line(&s.name, 60));
        if let Some(p) = &s.product {
            let _ = write!(out, " [{}]", one_line(p, 60));
        }
        let _ = write!(
            out,
            ": {} faces, {} triangles, {}",
            s.faces,
            s.triangles,
            size_text(s.size(), unit)
        );
        if s.failed_faces > 0 {
            let _ = write!(out, ", {} faces not meshed", s.failed_faces);
        }
    }
    if spec.solids.len() > MAX_LISTED {
        let _ = write!(
            out,
            "\n… {} more solids (the user sees them all)",
            spec.solids.len() - MAX_LISTED
        );
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
        let fs = InMemoryFs::new(test_root()).with_file("cad/block.step", "ISO-10303-21;\n");
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
        let tool = Typed(ShowStepTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_step_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_sizes_and_missing_faces() {
        let tool = Typed(ShowStepTool::new(ports()));
        let out = call(&tool, json!({ "path": "cad/block.step" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "STEP part `cad/block.step` (AP214), written by fake CAD, shown to the user: \
             1 solids, 4 faces, 3 triangles (tolerance 0.01 mm); length unit mm.\n\
             Box: 10 × 10 × 10 mm (from 0, 0, 0 to 10, 10, 10)\n\
             Products: Block\n\
             1 faces could not be meshed and are missing from the view; sizes are of what \
             was meshed.\n\
             Solids:\n\
             - Body [Block]: 4 faces, 3 triangles, 10 × 10 × 10 mm, 1 faces not meshed"
        );
        assert!(matches!(out.block_update, Some(BlockKind::CadModel(_))));
        let missing = call(&tool, json!({ "path": "cad/none.step" }))
            .await
            .unwrap();
        assert!(missing.is_error);
        let bad = tool
            .prepare(json!({ "path": "cad/block.step", "tolerance": 0 }), &ctx())
            .await;
        assert!(matches!(bad, Err(ToolError::InvalidInput { .. })));
    }
}
