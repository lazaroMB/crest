//! `show_molecule_3d`: a molecule in the 3D view as ball and stick, from
//! a structure file's coordinates or the engine's embedding of a SMILES.
//!
//! The model gets the formula, atom and bond counts, where the
//! coordinates come from (an embedded conformer is said to be computed)
//! and the box, never the coordinates.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::molecule3d::{CoordinateSource, Molecule3dSpec};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line, read};
use crate::ports::formats::Molecule3dSource;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Input of `show_molecule_3d`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMolecule3d {
    /// A SMILES to embed in 3D (a computed conformer, hydrogens added).
    #[serde(default)]
    pub smiles: Option<String>,
    /// Instead of `smiles`: a structure file of the workspace with 3D
    /// coordinates (`.mol`, `.sdf`, `.xyz`, `.pdb`).
    #[serde(default)]
    pub path: Option<String>,
    /// Name shown with the model.
    #[serde(default)]
    pub name: Option<String>,
    /// Title shown above the view.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_molecule_3d`: ball and stick in the 3D view.
#[derive(Clone)]
pub struct ShowMolecule3dTool {
    ports: FormatToolPorts,
}

impl ShowMolecule3dTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_molecule_3d`.
pub const SHOW_MOLECULE_3D: &str = "show_molecule_3d";

#[async_trait]
impl TypedTool for ShowMolecule3dTool {
    type Input = ShowMolecule3d;

    fn name(&self) -> &str {
        SHOW_MOLECULE_3D
    }

    fn description(&self) -> &str {
        "Shows a molecule in 3D as ball and stick in the user's timeline \
         (CPK colours, orbit and zoom; the user clicks atoms to read them \
         and measure distances): from a structure file with 3D coordinates \
         (`path`: MOL/SDF, XYZ, PDB) or from `smiles`, which the engine \
         embeds (hydrogens added, distance geometry and a force-field \
         minimisation: a computed conformer, not an experimental \
         structure). Returns the formula, counts and where the coordinates \
         come from; never state geometry the tool did not compute."
    }

    fn effect(&self, _input: &ShowMolecule3d) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMolecule3d,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let smiles = input.smiles.filter(|s| !s.trim().is_empty());
        let path = input.path.filter(|p| !p.trim().is_empty());
        let subject = match (&smiles, &path) {
            (Some(_), Some(_)) => {
                return Err(invalid(
                    SHOW_MOLECULE_3D,
                    "give `smiles` or `path`, not both",
                ));
            }
            (None, None) => return Err(invalid(SHOW_MOLECULE_3D, "give `smiles` or `path`")),
            (Some(s), None) => s.clone(),
            (None, Some(p)) => p.clone(),
        };
        let ports = self.ports.clone();
        let run = async move {
            let source = match (smiles, path) {
                (Some(s), _) => Molecule3dSource::Smiles(s.trim().to_owned()),
                (None, Some(path)) => match read(ports.fs.as_ref(), &path).await {
                    Ok(bytes) => Molecule3dSource::File { path, bytes },
                    Err(message) => return Ok(ToolOutput::error(message)),
                },
                (None, None) => return Ok(ToolOutput::error("nothing to show")),
            };
            let formats = Arc::clone(&ports.formats);
            let built = offloaded(&ports.offload, move || formats.molecule_3d(&source)).await;
            let mut spec = match built {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the 3D builder did not run")),
            };
            if let Some(name) = input.name.filter(|n| !n.trim().is_empty()) {
                spec.name = Some(name);
            }
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = molecule3d_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Molecule3d(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_molecule_3d`.
#[must_use]
pub fn molecule3d_summary(spec: &Molecule3dSpec) -> String {
    let mut out = format!(
        "3D model of `{}` shown to the user: {}, {} atoms, {} bonds",
        one_line(&spec.display_title(), 80),
        spec.formula(),
        spec.atoms.len(),
        spec.bonds.len()
    );
    if spec.bonds_inferred {
        out.push_str(" (bonds inferred from distances)");
    }
    out.push('.');
    match &spec.source {
        CoordinateSource::Embedded { method } => {
            let _ = write!(
                out,
                " Coordinates embedded by the engine ({method}): a computed conformer, not an \
                 experimental structure."
            );
        }
        CoordinateSource::File { .. } => {
            let _ = write!(
                out,
                " Coordinates from `{}`.",
                spec.path.as_deref().unwrap_or("the file")
            );
        }
    }
    if let Some((lo, hi)) = spec.bounds() {
        let _ = write!(
            out,
            " Box {:.2} × {:.2} × {:.2} Å.",
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2]
        );
    }
    out.push_str(" The user can click atoms to read them and measure distances.");
    for w in spec
        .warnings
        .iter()
        .filter(|w| !w.contains("computed conformer"))
        .take(5)
    {
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
        let fs = InMemoryFs::new(test_root()).with_file("mol/water.xyz", "3\nwater\n");
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
        let tool = Typed(ShowMolecule3dTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_molecule_3d_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_learns_where_the_coordinates_come_from() {
        let tool = Typed(ShowMolecule3dTool::new(ports()));
        let out = call(&tool, json!({ "smiles": "O" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "3D model of `water` shown to the user: H2O, 3 atoms, 2 bonds. Coordinates \
             embedded by the engine (fake embedding): a computed conformer, not an \
             experimental structure. Box 1.20 × 0.93 × 0.00 Å. The user can click atoms to \
             read them and measure distances."
        );
        let file = call(&tool, json!({ "path": "mol/water.xyz", "name": "H2O" }))
            .await
            .unwrap();
        assert!(
            file.text_content()
                .contains("Coordinates from `mol/water.xyz`.")
        );
        let Some(BlockKind::Molecule3d(spec)) = file.block_update else {
            panic!("no block");
        };
        assert_eq!(spec.name.as_deref(), Some("H2O"));
        assert!(
            call(&tool, json!({ "smiles": "bad" }))
                .await
                .unwrap()
                .is_error
        );
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "smiles": "O", "path": "a.pdb" }))
                .await
                .is_err()
        );
    }
}
