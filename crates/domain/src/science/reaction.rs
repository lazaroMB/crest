//! A reaction scheme with its balance check (`show_reaction`): the
//! reactants, agents and products of a reaction SMILES drawn in a row,
//! each with its computed formula, and a [`Computed`] envelope saying
//! whether every element and the charge balance.
//!
//! The chemistry adapter parses and depicts the molecules, counts atoms
//! (implicit hydrogens included) and, when the given coefficients do not
//! balance, looks for the smallest positive integer coefficients that do.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::MoleculeSpec;
use crate::block::BlockError;
use crate::computed::Computed;

/// Most molecules on one side.
pub const MAX_COMPONENTS: usize = 16;
/// Largest coefficient suggested.
pub const MAX_COEFFICIENT: u64 = 1_000;

/// The side of the arrow a molecule is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReactionRole {
    /// Left of the arrow.
    Reactant,
    /// Over the arrow (catalysts, solvents): not counted.
    Agent,
    /// Right of the arrow.
    Product,
}

/// One molecule of the scheme.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionComponent {
    /// Its side.
    pub role: ReactionRole,
    /// The molecule (SMILES, depiction, computed formula, mass, charge).
    pub molecule: MoleculeSpec,
    /// Its coefficient as given (1 when none was).
    #[serde(default = "one")]
    pub coefficient: u32,
    /// Atom counts of one molecule by element, implicit hydrogens
    /// included.
    #[serde(default)]
    pub atoms: BTreeMap<String, u32>,
}

fn one() -> u32 {
    1
}

/// One element's count on each side, with the coefficients given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ElementBalance {
    /// Element symbol.
    pub element: String,
    /// Atoms on the left.
    pub reactants: u64,
    /// Atoms on the right.
    pub products: u64,
}

impl ElementBalance {
    /// Products minus reactants.
    #[must_use]
    pub fn delta(&self) -> i64 {
        i64::try_from(self.products).unwrap_or(i64::MAX)
            - i64::try_from(self.reactants).unwrap_or(i64::MAX)
    }
}

/// What the coefficient search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stoichiometry {
    /// The given coefficients balance.
    Balanced,
    /// These smallest integer coefficients (reactants then products, in
    /// order) balance it.
    Suggested {
        /// One per counted molecule.
        coefficients: Vec<u32>,
    },
    /// No positive coefficients balance these molecules: something is
    /// missing (a by-product, a counter-ion) or a structure is wrong.
    Impossible,
    /// More than one independent set of coefficients balances it (two
    /// reactions in one); none is suggested.
    Ambiguous,
    /// The search was not run (a molecule did not parse).
    NotChecked,
}

/// A reaction drawn from reaction SMILES with its balance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionSpec {
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Reaction SMILES as given (`A.B>agent>C.D`).
    pub smiles: String,
    /// Conditions written over the arrow (`Δ, 2 h`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<String>,
    /// The molecules, reactants then agents then products.
    pub components: Vec<ReactionComponent>,
    /// Element counts per side with the given coefficients (implicit H
    /// included), by symbol.
    #[serde(default)]
    pub elements: Vec<ElementBalance>,
    /// Net charge on the left and on the right.
    #[serde(default)]
    pub charge: (i64, i64),
    /// The coefficient search.
    pub stoichiometry: Stoichiometry,
    /// The balance check as a computed envelope (values `balanced`,
    /// `charge_delta`, `delta.<element>`, `suggested.<n>`).
    pub balance: Computed,
    /// Why the reaction could not be read, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ReactionSpec {
    /// Checks that there is a reaction to show and the envelope is sound.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] for an empty SMILES, too many molecules or
    /// a malformed envelope.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "reaction",
            reason,
        };
        if self.smiles.trim().is_empty() {
            return Err(invalid("the reaction SMILES is empty".into()));
        }
        if self.components.len() > MAX_COMPONENTS * 3 {
            return Err(invalid(format!(
                "{} molecules; at most {} per side",
                self.components.len(),
                MAX_COMPONENTS
            )));
        }
        self.balance
            .validate()
            .map_err(|e| invalid(format!("balance envelope: {e}")))
    }

    /// The molecules of one side.
    pub fn side(&self, role: ReactionRole) -> impl Iterator<Item = &ReactionComponent> {
        self.components.iter().filter(move |c| c.role == role)
    }

    /// Whether every element and the charge balance with the given
    /// coefficients.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.error.is_none()
            && self.elements.iter().all(|e| e.delta() == 0)
            && self.charge.0 == self.charge.1
    }

    /// The elements that do not balance.
    #[must_use]
    pub fn unbalanced(&self) -> Vec<&ElementBalance> {
        self.elements.iter().filter(|e| e.delta() != 0).collect()
    }

    /// The scheme as text, with coefficients: `2 H2 + O2 → 2 H2O`
    /// (`coefficients` replaces the given ones, reactants then products).
    #[must_use]
    pub fn equation(&self, coefficients: Option<&[u32]>) -> String {
        let counted: Vec<&ReactionComponent> = self
            .components
            .iter()
            .filter(|c| c.role != ReactionRole::Agent)
            .collect();
        let coefficient = |i: usize, c: &ReactionComponent| {
            coefficients
                .and_then(|cs| cs.get(i).copied())
                .unwrap_or(c.coefficient)
        };
        let side = |role: ReactionRole| {
            counted
                .iter()
                .enumerate()
                .filter(|(_, c)| c.role == role)
                .map(|(i, c)| {
                    let n = coefficient(i, c);
                    let name = c
                        .molecule
                        .formula
                        .clone()
                        .unwrap_or_else(|| c.molecule.smiles.clone());
                    if n == 1 { name } else { format!("{n} {name}") }
                })
                .collect::<Vec<_>>()
                .join(" + ")
        };
        format!(
            "{} → {}",
            side(ReactionRole::Reactant),
            side(ReactionRole::Product)
        )
    }

    /// The balance table as CSV: element, reactants, products, delta.
    #[must_use]
    pub fn balance_csv(&self) -> String {
        let mut out = String::from("element,reactants,products,delta\n");
        for e in &self.elements {
            out.push_str(&format!(
                "{},{},{},{}\n",
                e.element,
                e.reactants,
                e.products,
                e.delta()
            ));
        }
        out.push_str(&format!(
            "charge,{},{},{}\n",
            self.charge.0,
            self.charge.1,
            self.charge.1 - self.charge.0
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn component(role: ReactionRole, formula: &str, coefficient: u32) -> ReactionComponent {
        ReactionComponent {
            role,
            molecule: MoleculeSpec {
                smiles: formula.into(),
                name: None,
                svg: None,
                formula: Some(formula.into()),
                mw: None,
                charge: Some(0),
                canonical: None,
                error: None,
                claimed_formula: None,
                claimed_matches: None,
            },
            coefficient,
            atoms: BTreeMap::new(),
        }
    }

    #[test]
    fn equation_and_balance_text() {
        let spec = ReactionSpec {
            title: None,
            smiles: "[H][H].O=O>>O".into(),
            conditions: None,
            components: vec![
                component(ReactionRole::Reactant, "H2", 1),
                component(ReactionRole::Reactant, "O2", 1),
                component(ReactionRole::Agent, "Pt", 1),
                component(ReactionRole::Product, "H2O", 1),
            ],
            elements: vec![
                ElementBalance {
                    element: "H".into(),
                    reactants: 2,
                    products: 2,
                },
                ElementBalance {
                    element: "O".into(),
                    reactants: 2,
                    products: 1,
                },
            ],
            charge: (0, 0),
            stoichiometry: Stoichiometry::Suggested {
                coefficients: vec![2, 1, 2],
            },
            balance: Computed::new(
                "ch_1",
                "chem.reaction_balance",
                "balance",
                Engine {
                    name: "crest-chem".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            error: None,
        };
        spec.validate().unwrap();
        assert!(!spec.is_balanced());
        assert_eq!(spec.unbalanced()[0].element, "O");
        assert_eq!(spec.equation(None), "H2 + O2 → H2O");
        assert_eq!(spec.equation(Some(&[2, 1, 2])), "2 H2 + O2 → 2 H2O");
        assert!(spec.balance_csv().contains("O,2,1,-1\ncharge,0,0,0\n"));
    }
}
