//! `show_material`: a material data card, each value with its source or
//! an "unverified" badge.
//!
//! The values come from the call. crest has no material database, so the
//! card shows where each number came from instead of vouching for it; the
//! model is told which values carry no source.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::material::{MaterialProperty, MaterialSpec};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{invalid, one_line};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};

/// Input of `show_material`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMaterial {
    /// Material name (`aluminium alloy`, `PEEK`, `structural steel`).
    pub name: String,
    /// Designation or grade (`6061-T6`, `S355JR`, `AISI 304`).
    #[serde(default)]
    pub designation: Option<String>,
    /// Form or condition of the whole card (`plate 6–12 mm`, `annealed`).
    #[serde(default)]
    pub condition: Option<String>,
    /// The properties: `{name, symbol, value, unit, condition, source}`.
    /// `source` names where the value comes from (datasheet with its
    /// revision, standard and table, handbook and edition, URL, or `user`
    /// for a value the user gave); leave it out when you have none and the
    /// value shows as unverified.
    pub properties: Vec<MaterialProperty>,
    /// A source for every property that names none (one datasheet for the
    /// whole card).
    #[serde(default)]
    pub source: Option<String>,
    /// Notes shown under the table.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// `show_material`: a material data card.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowMaterialTool;

/// Name of `show_material`.
pub const SHOW_MATERIAL: &str = "show_material";

#[async_trait]
impl TypedTool for ShowMaterialTool {
    type Input = ShowMaterial;

    fn name(&self) -> &str {
        SHOW_MATERIAL
    }

    fn description(&self) -> &str {
        "Shows a material data card in the user's timeline: a table of the \
         properties you give (density, E, ν, yield and ultimate strength, \
         CTE, conductivity...), each with its unit, condition and source, \
         and an `unverified` badge on every value without a source. crest \
         has no material database: give only values from the user, a file \
         or page you read, or a tool, and cite them; a value from memory \
         must go without a source. Returns which values are unverified."
    }

    fn effect(&self, _input: &ShowMaterial) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMaterial,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let default_source = input.source.filter(|s| !s.trim().is_empty());
        let clean = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        let spec = MaterialSpec {
            name: input.name.trim().to_owned(),
            designation: clean(input.designation),
            condition: clean(input.condition),
            properties: input
                .properties
                .into_iter()
                .map(|p| MaterialProperty {
                    source: clean(p.source).or_else(|| default_source.clone()),
                    symbol: clean(p.symbol),
                    condition: clean(p.condition),
                    ..p
                })
                .collect(),
            notes: input
                .notes
                .into_iter()
                .filter(|n| !n.trim().is_empty())
                .collect(),
        };
        spec.validate()
            .map_err(|err| invalid(SHOW_MATERIAL, err.to_string()))?;
        let subject = spec.full_name();
        let summary = material_summary(&spec);
        let run = async move {
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Material(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_material`.
#[must_use]
pub fn material_summary(spec: &MaterialSpec) -> String {
    let unverified = spec.unverified();
    let mut out = format!(
        "Material card for {} shown to the user: {} properties, {} with a source, {} unverified.",
        one_line(&spec.full_name(), 80),
        spec.properties.len(),
        spec.properties.len() - unverified.len(),
        unverified.len()
    );
    if !unverified.is_empty() {
        let names: Vec<&str> = unverified.iter().map(|p| p.name.as_str()).collect();
        let _ = write!(
            out,
            "\nUnverified (no source; say so if you use them): {}.",
            names.join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::tools::registry::{Tool, Typed};

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowMaterialTool);
        insta::assert_json_snapshot!(
            "show_material_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn sources_default_from_the_card_and_unverified_values_are_named() {
        let tool = Typed(ShowMaterialTool);
        let input = json!({
            "name": "aluminium alloy",
            "designation": "6061-T6",
            "source": "supplier datasheet rev C",
            "properties": [
                { "name": "density", "value": 2700, "unit": "kg/m³" },
                { "name": "Poisson's ratio", "symbol": "ν", "value": 0.33, "unit": "", "source": "" }
            ]
        });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "aluminium alloy 6061-T6");
        let out = prepared.run.await.unwrap();
        assert_eq!(
            out.text_content(),
            "Material card for aluminium alloy 6061-T6 shown to the user: 2 properties, 2 with \
             a source, 0 unverified."
        );
        let Some(BlockKind::Material(spec)) = out.block_update else {
            panic!("not a material card");
        };
        assert_eq!(
            spec.properties[1].source.as_deref(),
            Some("supplier datasheet rev C")
        );
        let bare = json!({
            "name": "PEEK",
            "properties": [{ "name": "density", "value": 1300, "unit": "kg/m³" }]
        });
        let text = tool
            .prepare(bare, &ctx())
            .await
            .unwrap()
            .run
            .await
            .unwrap()
            .text_content();
        assert!(text.contains("Unverified (no source; say so if you use them): density."));
        let empty = tool
            .prepare(json!({ "name": "x", "properties": [] }), &ctx())
            .await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
    }
}
