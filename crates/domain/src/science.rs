//! Lab blocks: a 2D molecule with its validation strip (`show_molecule`)
//! and a microplate map (`show_plate`).
//!
//! A molecule's formula, mass and charge are computed from its SMILES by
//! the chemistry adapter; when the model also stated a formula
//! (`claimed_formula`), the block shows whether the two agree.

use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;

/// A molecule drawn from SMILES.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MoleculeSpec {
    /// SMILES as given.
    pub smiles: String,
    /// Name to show beside the drawing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 2D depiction, SVG source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
    /// Molecular formula in Hill order, computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Molecular weight in g/mol, computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mw: Option<f64>,
    /// Net formal charge, computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge: Option<i32>,
    /// Canonical SMILES, computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
    /// Why the SMILES could not be parsed, with the column when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Formula the model stated, to compare with the computed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_formula: Option<String>,
    /// Whether `claimed_formula` matches `formula` (see [`formulas_match`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_matches: Option<bool>,
}

impl MoleculeSpec {
    /// Checks that the SMILES is not empty.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] for an empty SMILES.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.smiles.trim().is_empty() {
            return Err(BlockError::Invalid {
                kind: "molecule",
                reason: "smiles is empty".into(),
            });
        }
        Ok(())
    }
}

/// Element counts of a molecular formula such as `C9H8O4` or `Ca(OH)2`.
/// `None` when it does not parse. Charges and hydrates are not supported.
#[must_use]
pub fn parse_formula(formula: &str) -> Option<BTreeMap<String, u32>> {
    let chars: Vec<char> = formula.chars().filter(|c| !c.is_whitespace()).collect();
    let mut stack: Vec<BTreeMap<String, u32>> = vec![BTreeMap::new()];
    let mut i = 0;
    let count_at = |i: &mut usize| -> u32 {
        let start = *i;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        chars[start..*i]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(1)
    };
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_uppercase() {
            let mut symbol = c.to_string();
            i += 1;
            if let Some(lower) = chars.get(i).filter(|c| c.is_ascii_lowercase()) {
                symbol.push(*lower);
                i += 1;
            }
            let n = count_at(&mut i);
            *stack.last_mut()?.entry(symbol).or_default() += n;
        } else if c == '(' {
            stack.push(BTreeMap::new());
            i += 1;
        } else if c == ')' {
            i += 1;
            let n = count_at(&mut i);
            let group = stack.pop()?;
            let outer = stack.last_mut()?;
            for (symbol, k) in group {
                *outer.entry(symbol).or_default() += k * n;
            }
        } else {
            return None;
        }
    }
    let counts = stack.pop()?;
    (stack.is_empty() && !counts.is_empty()).then_some(counts)
}

/// Whether two formulas have the same element counts, whatever their
/// order or grouping (`C2H6O` matches `CH3CH2OH`). `None` when either does
/// not parse.
#[must_use]
pub fn formulas_match(a: &str, b: &str) -> Option<bool> {
    Some(parse_formula(a)? == parse_formula(b)?)
}

/// A microplate map: a value and a label per well, drawn as a heatmap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlateSpec {
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Number of wells: 96 (8×12) or 384 (16×24).
    pub format: u16,
    /// What the values are, e.g. `OD600`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_label: Option<String>,
    /// Wells with data; the others are drawn empty.
    pub wells: Vec<Well>,
}

/// One well of a plate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Well {
    /// Position, row letter and 1-based column: `A1`, `P24`.
    pub well: String,
    /// Sample label, e.g. `blank`, `ctrl+`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Measured value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

/// Rows and columns of a plate format, `None` for an unknown one.
#[must_use]
pub fn plate_dimensions(format: u16) -> Option<(u8, u8)> {
    match format {
        96 => Some((8, 12)),
        384 => Some((16, 24)),
        _ => None,
    }
}

/// Zero-based (row, column) of a well name such as `B12`, if it fits in a
/// plate of `format`.
#[must_use]
pub fn parse_well(well: &str, format: u16) -> Option<(u8, u8)> {
    let (rows, cols) = plate_dimensions(format)?;
    let mut chars = well.trim().chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_uppercase() {
        return None;
    }
    let row = u8::try_from(u32::from(letter) - u32::from('A')).ok()?;
    let col: u8 = chars.as_str().parse().ok()?;
    (row < rows && (1..=cols).contains(&col)).then(|| (row, col - 1))
}

impl PlateSpec {
    /// Checks the format, that every well name fits it and that no well is
    /// listed twice.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "plate",
            reason,
        };
        if plate_dimensions(self.format).is_none() {
            return Err(invalid(format!("format {} is not 96 or 384", self.format)));
        }
        let mut seen = HashSet::new();
        for well in &self.wells {
            let position = parse_well(&well.well, self.format).ok_or_else(|| {
                invalid(format!(
                    "{:?} is not a well of a {}-well plate",
                    well.well, self.format
                ))
            })?;
            if !seen.insert(position) {
                return Err(invalid(format!("well {:?} appears twice", well.well)));
            }
            if well.value.is_some_and(|v| !v.is_finite()) {
                return Err(invalid(format!(
                    "well {:?} has a non-finite value",
                    well.well
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formulas_compare_by_element_counts() {
        let aspirin = parse_formula("C9H8O4").unwrap();
        assert_eq!(aspirin["C"], 9);
        assert_eq!(aspirin["O"], 4);
        assert_eq!(parse_formula("Ca(OH)2").unwrap()["H"], 2);
        assert_eq!(formulas_match("C2H6O", "CH3CH2OH"), Some(true));
        assert_eq!(formulas_match("C9H8O4", "C9H8O3"), Some(false));
        assert_eq!(formulas_match("C9H8O4", "c9"), None);
        assert_eq!(parse_formula("(CH3"), None);
        assert_eq!(parse_formula(""), None);
    }

    #[test]
    fn molecule_roundtrips() {
        let spec: MoleculeSpec = serde_json::from_value(json!({
            "smiles": "CC(=O)Oc1ccccc1C(=O)O", "name": "aspirin", "formula": "C9H8O4",
            "mw": 180.159, "charge": 0, "claimed_formula": "C9H8O4", "claimed_matches": true
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        let back: MoleculeSpec =
            serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn wells_parse_within_the_format() {
        assert_eq!(parse_well("A1", 96), Some((0, 0)));
        assert_eq!(parse_well("h12", 96), Some((7, 11)));
        assert_eq!(parse_well("I1", 96), None);
        assert_eq!(parse_well("A13", 96), None);
        assert_eq!(parse_well("P24", 384), Some((15, 23)));
        assert_eq!(parse_well("A0", 384), None);
        assert_eq!(parse_well("A1", 48), None);
    }

    #[test]
    fn plate_validate() {
        let mut plate: PlateSpec = serde_json::from_value(json!({
            "format": 96, "value_label": "OD600",
            "wells": [{"well": "A1", "label": "blank", "value": 0.04}, {"well": "B2", "value": 1.2}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(plate.validate(), Ok(()));
        plate.wells.push(Well {
            well: "a1".into(),
            label: None,
            value: None,
        });
        assert!(plate.validate().is_err());
        plate.wells.pop();
        plate.format = 48;
        assert!(plate.validate().is_err());
    }
}
