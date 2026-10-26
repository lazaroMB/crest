//! `show_ifc_schedule`: the room schedule of an IFC model, storey by
//! storey.
//!
//! The model gets the project, schema and units, each storey's elevation,
//! spaces (name, long name, area and where the file gives it, or "not
//! given") and wall, door and window counts. Areas are never computed:
//! the summary says how many spaces have none.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::ifc::{ElementCounts, IfcScheduleSpec, IfcSpace, NOT_GIVEN};
use domain::permission::ToolEffect;
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, one_line};
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest IFC file read, in bytes.
pub const MAX_IFC_BYTES: u64 = 256 * 1024 * 1024;
/// Spaces listed to the model.
pub const MAX_SPACES_LISTED: usize = 200;

/// Input of `show_ifc_schedule`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowIfcSchedule {
    /// An IFC file of the workspace (`.ifc`, STEP physical file; IFC2X3,
    /// IFC4 or IFC4X3), e.g. an export of Revit, ArchiCAD, BlenderBIM or
    /// IfcOpenShell.
    pub path: String,
    /// Title shown above the schedule; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_ifc_schedule`: an IFC room schedule.
#[derive(Clone)]
pub struct ShowIfcScheduleTool {
    ports: FormatToolPorts,
}

impl ShowIfcScheduleTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_ifc_schedule`.
pub const SHOW_IFC_SCHEDULE: &str = "show_ifc_schedule";

/// Reads an IFC file of the workspace (up to [`MAX_IFC_BYTES`]) through
/// the guard.
///
/// # Errors
///
/// A message for the caller: bad path, unreadable or too large.
pub async fn read_ifc(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("IFC path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_IFC_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowIfcScheduleTool {
    type Input = ShowIfcSchedule;

    fn name(&self) -> &str {
        SHOW_IFC_SCHEDULE
    }

    fn description(&self) -> &str {
        "Shows the room schedule of an IFC building model (`path`) in the \
         user's timeline: storeys by elevation with their spaces (name, \
         long name, floor area and the property or quantity it was read \
         from) and how many walls, doors and windows each storey holds. \
         Areas come only from the file (Qto_SpaceBaseQuantities, \
         Pset_SpaceCommon); a space without one says \"not given\". \
         Returns the schedule: cite its areas and counts, never estimate \
         a missing area, and do not repeat the table in your reply."
    }

    fn effect(&self, _input: &ShowIfcSchedule) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowIfcSchedule,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_IFC_SCHEDULE, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_ifc(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_ifc_schedule(&bytes, &path)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the IFC reader did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = ifc_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::IfcSchedule(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// A number with at most 3 decimals.
fn number(v: f64) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn counts_text(c: &ElementCounts) -> String {
    format!(
        "{} walls, {} doors, {} windows",
        c.walls, c.doors, c.windows
    )
}

fn space_text(s: &IfcSpace, unit: &str) -> String {
    let mut line = one_line(&s.label(), 80);
    match (s.area, &s.area_source) {
        (Some(a), Some(src)) => {
            let _ = write!(line, ": {} {unit} ({src})", number(a));
        }
        _ => {
            let _ = write!(line, ": area {NOT_GIVEN}");
        }
    }
    line
}

/// What the model gets back from `show_ifc_schedule`.
#[must_use]
pub fn ifc_summary(spec: &IfcScheduleSpec) -> String {
    let unit = spec.area_label().to_owned();
    let mut out = format!("{} model `{}`", spec.schema, spec.path);
    if let Some(p) = &spec.project {
        let _ = write!(out, " (project {})", one_line(p, 60));
    }
    let _ = write!(
        out,
        " shown to the user: {} storeys, {} spaces ({} without an area in the file), {}.",
        spec.storeys.len(),
        spec.space_count(),
        spec.missing_areas(),
        counts_text(&spec.element_totals())
    );
    let units: Vec<String> = [
        spec.length_unit.as_ref().map(|u| format!("length {u}")),
        spec.area_unit.as_ref().map(|u| format!("area {u}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !units.is_empty() {
        let _ = write!(out, " Units: {}.", units.join(", "));
    }
    if !spec.buildings.is_empty() {
        let listed: Vec<String> = spec
            .buildings
            .iter()
            .take(MAX_LISTED)
            .map(|b| one_line(b, 60))
            .collect();
        let _ = write!(out, "\nBuildings: {}", listed.join(", "));
    }
    let mut listed = 0;
    for st in &spec.storeys {
        let (given, missing) = st.area();
        let _ = write!(out, "\nStorey {}", one_line(&st.name, 60));
        if let Some(e) = st.elevation {
            let unit = spec
                .length_unit
                .as_deref()
                .map(|u| format!(" {u}"))
                .unwrap_or_default();
            let _ = write!(out, " (elevation {}{unit})", number(e));
        }
        let _ = write!(out, ": {} spaces", st.spaces.len());
        if st.spaces.len() > missing {
            let _ = write!(out, ", given areas sum to {} {unit}", number(given));
            if missing > 0 {
                let _ = write!(out, " ({missing} spaces not given)");
            }
        }
        let _ = write!(out, "; {}", counts_text(&st.elements));
        for s in &st.spaces {
            if listed >= MAX_SPACES_LISTED {
                break;
            }
            listed += 1;
            let _ = write!(out, "\n- {}", space_text(s, &unit));
        }
    }
    if !spec.unplaced_spaces.is_empty() || !spec.unplaced.is_empty() {
        let _ = write!(
            out,
            "\nIn no storey: {} spaces; {}",
            spec.unplaced_spaces.len(),
            counts_text(&spec.unplaced)
        );
        for s in &spec.unplaced_spaces {
            if listed >= MAX_SPACES_LISTED {
                break;
            }
            listed += 1;
            let _ = write!(out, "\n- {}", space_text(s, &unit));
        }
    }
    if spec.space_count() > listed {
        let _ = write!(
            out,
            "\n… {} more spaces (the user sees them all)",
            spec.space_count() - listed
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
        let fs = InMemoryFs::new(test_root()).with_file("bim/house.ifc", "ISO-10303-21;\n");
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
        let tool = Typed(ShowIfcScheduleTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_ifc_schedule_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_areas_with_sources_and_counts() {
        let tool = Typed(ShowIfcScheduleTool::new(ports()));
        let out = call(&tool, json!({ "path": "bim/house.ifc" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "IFC4 model `bim/house.ifc` (project House) shown to the user: 1 storeys, \
             2 spaces (1 without an area in the file), 4 walls, 2 doors, 3 windows. \
             Units: length m, area m².\n\
             Storey Ground floor (elevation 0 m): 2 spaces, given areas sum to 12.5 m² \
             (1 spaces not given); 4 walls, 2 doors, 3 windows\n\
             - 101 Kitchen: 12.5 m² (Qto_SpaceBaseQuantities.NetFloorArea)\n\
             - 102 Store: area not given"
        );
        assert!(matches!(out.block_update, Some(BlockKind::IfcSchedule(_))));
        let missing = call(&tool, json!({ "path": "bim/none.ifc" }))
            .await
            .unwrap();
        assert!(missing.is_error);
    }
}
