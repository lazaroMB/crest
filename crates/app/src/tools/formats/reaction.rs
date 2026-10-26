//! `show_reaction`: a reaction scheme from reaction SMILES with its atom
//! and charge balance.
//!
//! The model gets the equation with the computed formulas, whether it
//! balances, the elements and charge that do not, and the smallest
//! integer coefficients that would balance it, or why none exist.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::reaction::{ReactionRole, ReactionSpec, Stoichiometry};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, invalid, one_line};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Input of `show_reaction`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowReaction {
    /// Reaction SMILES: reactants, agents (catalysts, solvents; may be
    /// empty) and products separated by `>`, molecules by `.`, e.g.
    /// `CCO.O=O>>CC(=O)O.O`.
    pub smiles: String,
    /// Stoichiometric coefficients, one per reactant then per product (not
    /// agents), e.g. `[1, 2, 1, 2]`; 1 each when absent.
    #[serde(default)]
    pub coefficients: Option<Vec<u32>>,
    /// Conditions written over the arrow, e.g. `H2SO4, reflux, 2 h`.
    #[serde(default)]
    pub conditions: Option<String>,
    /// Names of the molecules in order (reactants, agents, products), shown
    /// under each drawing.
    #[serde(default)]
    pub names: Vec<String>,
    /// Title shown above the scheme.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_reaction`: a reaction scheme with its balance.
#[derive(Clone)]
pub struct ShowReactionTool {
    ports: FormatToolPorts,
}

impl ShowReactionTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_reaction`.
pub const SHOW_REACTION: &str = "show_reaction";

#[async_trait]
impl TypedTool for ShowReactionTool {
    type Input = ShowReaction;

    fn name(&self) -> &str {
        SHOW_REACTION
    }

    fn description(&self) -> &str {
        "Draws a reaction scheme in the user's timeline from reaction SMILES \
         (`reactants>agents>products`): every molecule with its computed \
         formula, and an engine check that every element and the charge \
         balance (implicit hydrogens counted, agents not). When it does not \
         balance it returns what is off and the smallest integer \
         coefficients that balance it, or that none exist (a missing \
         by-product). Cite this result; never claim a reaction is balanced \
         without it."
    }

    fn effect(&self, _input: &ShowReaction) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowReaction,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.smiles.trim().is_empty() {
            return Err(invalid(SHOW_REACTION, "`smiles` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.smiles.clone();
        let run = async move {
            let formats = Arc::clone(&ports.formats);
            let smiles = input.smiles.trim().to_owned();
            let coefficients = input.coefficients.clone();
            let depicted = offloaded(&ports.offload, move || {
                formats.depict_reaction(&smiles, coefficients.as_deref())
            })
            .await;
            let Some(mut spec) = depicted else {
                return Ok(ToolOutput::error("the reaction renderer did not run"));
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            spec.conditions = input.conditions.filter(|c| !c.trim().is_empty());
            let mut names = input.names.into_iter();
            for c in &mut spec.components {
                c.molecule.name = names.next().filter(|n| !n.trim().is_empty());
            }
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = reaction_summary(&spec);
            let output = if spec.error.is_some() {
                ToolOutput::error(summary)
            } else {
                ToolOutput::text(summary)
            };
            Ok(output.with_block_update(BlockKind::Reaction(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_reaction`.
#[must_use]
pub fn reaction_summary(spec: &ReactionSpec) -> String {
    if let Some(error) = &spec.error {
        return format!(
            "The reaction could not be drawn: {error}. Fix the reaction SMILES `{}` and call again.",
            spec.smiles
        );
    }
    let id = &spec.balance.id;
    let mut out = format!(
        "Reaction shown to the user (envelope `{id}`): {}.",
        spec.equation(None)
    );
    let agents: Vec<String> = spec
        .side(ReactionRole::Agent)
        .map(|c| {
            c.molecule
                .formula
                .clone()
                .unwrap_or_else(|| c.molecule.smiles.clone())
        })
        .collect();
    if !agents.is_empty() {
        let _ = write!(out, " Agents (not counted): {}.", agents.join(", "));
    }
    let masses: Vec<String> = spec
        .components
        .iter()
        .filter_map(|c| {
            let formula = c.molecule.formula.as_deref()?;
            let mw = c.molecule.mw?;
            let name = c
                .molecule
                .name
                .as_deref()
                .map(|n| format!("{} ", one_line(n, 40)))
                .unwrap_or_default();
            Some(format!("{name}{formula} {mw:.2} g/mol"))
        })
        .collect();
    if !masses.is_empty() {
        let _ = write!(out, "\nMolecules: {}.", masses.join("; "));
    }
    if spec.is_balanced() {
        out.push_str("\nBalanced: every element and the charge match.");
    } else {
        let off: Vec<String> = spec
            .unbalanced()
            .iter()
            .map(|e| format!("{} {} → {}", e.element, e.reactants, e.products))
            .collect();
        let _ = write!(out, "\nNot balanced");
        if !off.is_empty() {
            let _ = write!(out, ": {}", off.join(", "));
        }
        if spec.charge.0 != spec.charge.1 {
            let _ = write!(out, "; charge {} → {}", spec.charge.0, spec.charge.1);
        }
        out.push('.');
    }
    match &spec.stoichiometry {
        Stoichiometry::Suggested { coefficients } => {
            let _ = write!(
                out,
                "\nSmallest balancing coefficients: {} ({}).",
                spec.equation(Some(coefficients)),
                coefficients
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Stoichiometry::Impossible => out.push_str(
            "\nNo positive coefficients balance these molecules: a by-product, counter-ion or \
             electron is missing, or a structure is wrong.",
        ),
        Stoichiometry::Ambiguous => out.push_str(
            "\nSeveral independent sets of coefficients balance it (two reactions in one); \
             give the coefficients.",
        ),
        Stoichiometry::Balanced | Stoichiometry::NotChecked => {}
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
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
            fs: Arc::new(InMemoryFs::new(test_root())),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowReactionTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_reaction_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_the_balance_and_a_suggestion() {
        let tool = Typed(ShowReactionTool::new(ports()));
        let out = call(
            &tool,
            json!({ "smiles": "[H][H].O=O>>O", "names": ["hydrogen", "oxygen", "water"] }),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Reaction shown to the user (envelope `ch_fake`): H2 + O2 → H2O.\n\
             Molecules: hydrogen H2 1.00 g/mol; oxygen O2 1.00 g/mol; water H2O 1.00 g/mol.\n\
             Not balanced: O 2 → 1.\n\
             Smallest balancing coefficients: 2 H2 + O2 → 2 H2O (2, 1, 2)."
        );
        let Some(BlockKind::Reaction(spec)) = out.block_update else {
            panic!("no reaction block");
        };
        assert_eq!(spec.components[2].molecule.name.as_deref(), Some("water"));
        let bad = call(&tool, json!({ "smiles": "bad" })).await.unwrap();
        assert!(bad.is_error);
        assert!(call(&tool, json!({ "smiles": " " })).await.is_err());
    }
}
