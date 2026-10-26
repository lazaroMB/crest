//! `show_molecule_grid`: a grid of molecules with computed descriptors
//! and the activities given, for structure–activity (SAR) tables.
//!
//! Molecules come inline or from a CSV/TSV file with a `smiles` column
//! (and optionally `id`, `name` and an activity column). The model gets a
//! line per molecule with the descriptors the engine computed, the
//! envelope id to cite them by, and the SMILES that did not parse.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::molecule_grid::{
    ActivityDirection, MAX_MOLECULES, MoleculeGridSpec, value_key,
};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line, read};
use crate::csv::{delimiter_for, number, records};
use crate::ports::formats::{GridEntry, MoleculeGridRequest};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Molecules listed to the model.
pub const MAX_LISTED_MOLECULES: usize = 60;

/// Input of `show_molecule_grid`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMoleculeGrid {
    /// The molecules: `{id, smiles, name, activity}` each.
    #[serde(default)]
    pub molecules: Vec<GridEntry>,
    /// Instead of `molecules`: a CSV or TSV file of the workspace with a
    /// `smiles` column and optionally `id`, `name` and the activity
    /// column.
    #[serde(default)]
    pub path: Option<String>,
    /// The activity column of the file (default `activity`).
    #[serde(default)]
    pub activity_column: Option<String>,
    /// What the activity is, e.g. `IC50` or `% inhibition at 10 µM`.
    #[serde(default)]
    pub activity_label: Option<String>,
    /// Its unit, e.g. `nM`.
    #[serde(default)]
    pub activity_unit: Option<String>,
    /// `higher` (default) or `lower`: which activity is better, for the
    /// colours (IC50 and Ki: `lower`).
    #[serde(default)]
    pub direction: Option<ActivityDirection>,
    /// Title shown above the grid.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_molecule_grid`: a SAR grid.
#[derive(Clone)]
pub struct ShowMoleculeGridTool {
    ports: FormatToolPorts,
}

impl ShowMoleculeGridTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_molecule_grid`.
pub const SHOW_MOLECULE_GRID: &str = "show_molecule_grid";

/// The molecules of a CSV/TSV `text`: a `smiles` column (any case), `id`
/// (the row number otherwise), `name` and `activity_column`.
///
/// # Errors
///
/// A message for a file without a `smiles` column, or an activity that
/// is not a number.
pub fn grid_entries(
    text: &str,
    delimiter: char,
    activity_column: &str,
) -> Result<Vec<GridEntry>, String> {
    let rows = records(text, delimiter)?;
    let Some((header, body)) = rows.split_first() else {
        return Err("the file is empty".into());
    };
    let find = |name: &str| {
        header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let smiles = find("smiles").ok_or("the file has no `smiles` column")?;
    let (id, name, activity) = (find("id"), find("name"), find(activity_column));
    let mut entries = Vec::new();
    for (i, row) in body.iter().enumerate() {
        let cell = |c: Option<usize>| {
            c.and_then(|c| row.get(c))
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        let Some(s) = cell(Some(smiles)) else {
            continue;
        };
        let activity = match cell(activity) {
            Some(text) => Some(number(&text).ok_or_else(|| {
                format!("row {}: activity `{text}` is not a number", i + 2)
            })?),
            None => None,
        };
        entries.push(GridEntry {
            id: cell(id).unwrap_or_else(|| (i + 1).to_string()),
            smiles: s,
            name: cell(name),
            activity,
        });
    }
    Ok(entries)
}

#[async_trait]
impl TypedTool for ShowMoleculeGridTool {
    type Input = ShowMoleculeGrid;

    fn name(&self) -> &str {
        SHOW_MOLECULE_GRID
    }

    fn description(&self) -> &str {
        "Shows a grid of molecules in the user's timeline for a \
         structure–activity (SAR) table: each SMILES drawn with \
         descriptors the engine computes (MW, heavy atoms, HBD/HBA, \
         rotatable bonds, rings, Crippen cLogP, TPSA, rule-of-five \
         violations) and the activity you give, sortable, filterable and \
         coloured by activity. Give `molecules` inline or a CSV/TSV `path` \
         with a `smiles` column. Returns the descriptors per molecule and \
         the envelope id; cite these, never descriptor values from memory."
    }

    fn effect(&self, _input: &ShowMoleculeGrid) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMoleculeGrid,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input.path.clone().filter(|p| !p.trim().is_empty());
        match (&path, input.molecules.is_empty()) {
            (Some(_), false) => {
                return Err(invalid(
                    SHOW_MOLECULE_GRID,
                    "give `molecules` or `path`, not both",
                ));
            }
            (None, true) => return Err(invalid(SHOW_MOLECULE_GRID, "give `molecules` or `path`")),
            _ => {}
        }
        if input.molecules.len() > MAX_MOLECULES {
            return Err(invalid(
                SHOW_MOLECULE_GRID,
                format!("at most {MAX_MOLECULES} molecules in one grid"),
            ));
        }
        let ports = self.ports.clone();
        let subject = path
            .clone()
            .unwrap_or_else(|| format!("{} molecules", input.molecules.len()));
        let run = async move {
            let molecules = match &path {
                Some(path) => {
                    let bytes = match read(ports.fs.as_ref(), path).await {
                        Ok(bytes) => bytes,
                        Err(message) => return Ok(ToolOutput::error(message)),
                    };
                    let Ok(text) = String::from_utf8(bytes) else {
                        return Ok(ToolOutput::error(format!("`{path}` is not UTF-8 text")));
                    };
                    let column = input.activity_column.as_deref().unwrap_or("activity");
                    match grid_entries(&text, delimiter_for(path), column) {
                        Ok(entries) => entries,
                        Err(message) => return Ok(ToolOutput::error(format!("`{path}`: {message}"))),
                    }
                }
                None => input.molecules,
            };
            let request = MoleculeGridRequest {
                molecules,
                activity_label: input.activity_label.filter(|l| !l.trim().is_empty()),
                activity_unit: input.activity_unit.filter(|u| !u.trim().is_empty()),
                direction: input.direction.unwrap_or_default(),
            };
            let formats = Arc::clone(&ports.formats);
            let computed = offloaded(&ports.offload, move || formats.molecule_grid(&request)).await;
            let mut spec = match computed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the descriptor engine did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = grid_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::MoleculeGrid(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// A number with `decimals`, `-0` as `0`.
fn fixed(v: f64, decimals: usize) -> String {
    let text = format!("{v:.decimals$}");
    if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        format!("{:.decimals$}", 0.0)
    } else {
        text
    }
}

/// What the model gets back from `show_molecule_grid`.
#[must_use]
pub fn grid_summary(spec: &MoleculeGridSpec) -> String {
    let mut out = format!(
        "Molecule grid shown to the user: {} molecules (descriptor envelope `{}`, keys \
         `<id>.mw`, `.logp`, `.tpsa`, `.hbd`, `.hba`, `.rotatable_bonds`, `.rings`, \
         `.heavy_atoms`, `.ro5_violations`).",
        spec.molecules.len(),
        spec.descriptors.id
    );
    let unit = spec
        .activity_unit
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    if let Some((lo, hi)) = spec.activity_range() {
        let label = spec.activity_label.as_deref().unwrap_or("activity");
        let better = match spec.direction {
            ActivityDirection::Higher => "higher",
            ActivityDirection::Lower => "lower",
        };
        let _ = write!(
            out,
            " {label} from {lo}{unit} to {hi}{unit} ({better} is better)."
        );
    }
    for m in spec.molecules.iter().take(MAX_LISTED_MOLECULES) {
        let key = value_key(&m.id);
        let _ = write!(out, "\n- {}", one_line(&m.id, 40));
        if key != m.id {
            let _ = write!(out, " (key `{key}`)");
        }
        if let Some(name) = &m.molecule.name {
            let _ = write!(out, " {}", one_line(name, 40));
        }
        match (&m.descriptors, &m.molecule.error) {
            (Some(d), _) => {
                let _ = write!(
                    out,
                    ": {}, MW {}, cLogP {}, TPSA {}, HBD {}, HBA {}, rot {}, rings {}, Ro5 {}",
                    m.molecule.formula.as_deref().unwrap_or("?"),
                    fixed(d.mw, 2),
                    fixed(d.logp, 2),
                    fixed(d.tpsa, 1),
                    d.hbd,
                    d.hba,
                    d.rotatable_bonds,
                    d.rings,
                    d.lipinski_violations()
                );
            }
            (None, Some(error)) => {
                let _ = write!(out, ": SMILES not parsed ({})", one_line(error, 80));
            }
            (None, None) => out.push_str(": no descriptors"),
        }
        if let Some(a) = m.activity {
            let _ = write!(out, "; activity {a}{unit}");
        }
    }
    if spec.molecules.len() > MAX_LISTED_MOLECULES {
        let _ = write!(
            out,
            "\n… {} more (the user sees them all; the envelope has every value)",
            spec.molecules.len() - MAX_LISTED_MOLECULES
        );
    }
    let failed = spec.failed().len();
    if failed > 0 {
        let _ = write!(out, "\n{failed} SMILES could not be parsed: fix them and call again.");
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
        let fs = InMemoryFs::new(test_root()).with_file(
            "sar/series.csv",
            "ID,SMILES,pIC50\nA-1,CCO,6.5\nA-2,bad,\n",
        );
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
    fn definition_and_csv_entries() {
        let tool = Typed(ShowMoleculeGridTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_molecule_grid_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        let entries = grid_entries("smiles\tname\nCCO\tethanol\n", '\t', "activity").unwrap();
        assert_eq!(entries[0].id, "1");
        assert_eq!(entries[0].name.as_deref(), Some("ethanol"));
        assert!(grid_entries("id\nx\n", ',', "activity").is_err());
        assert!(grid_entries("smiles,activity\nC,high\n", ',', "activity")
            .unwrap_err()
            .contains("not a number"));
    }

    #[tokio::test]
    async fn the_model_gets_the_descriptors_per_molecule() {
        let tool = Typed(ShowMoleculeGridTool::new(ports()));
        let out = call(
            &tool,
            json!({
                "path": "sar/series.csv",
                "activity_column": "pIC50",
                "activity_label": "pIC50"
            }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Molecule grid shown to the user: 2 molecules (descriptor envelope `ch_grid`, keys \
             `<id>.mw`, `.logp`, `.tpsa`, `.hbd`, `.hba`, `.rotatable_bonds`, `.rings`, \
             `.heavy_atoms`, `.ro5_violations`). pIC50 from 6.5 to 6.5 (higher is better).\n\
             - A-1: C2H6O, MW 46.07, cLogP 0.00, TPSA 20.2, HBD 1, HBA 1, rot 0, rings 0, Ro5 0; \
             activity 6.5\n\
             - A-2: SMILES not parsed (invalid SMILES: unexpected character at position 0)\n\
             1 SMILES could not be parsed: fix them and call again."
        );
        assert!(matches!(out.block_update, Some(BlockKind::MoleculeGrid(_))));
        assert!(call(&tool, json!({})).await.is_err());
        let both = json!({ "path": "a.csv", "molecules": [{ "id": "a", "smiles": "C" }] });
        assert!(call(&tool, both).await.is_err());
    }
}
