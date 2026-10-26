//! A grid of molecules with their descriptors and activities, for a
//! structure–activity (SAR) table (`show_molecule_grid`).
//!
//! The chemistry adapter depicts each SMILES and computes its descriptors
//! (molecular weight, heavy atoms, H-bond donors and acceptors, rotatable
//! bonds, rings, Crippen logP, TPSA); the activities are the call's. The
//! descriptor values also go into a [`Computed`] envelope, keyed
//! `<id>.<descriptor>`, so the model can cite them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::MoleculeSpec;
use crate::block::BlockError;
use crate::computed::Computed;

/// Most molecules in one grid.
pub const MAX_MOLECULES: usize = 500;

/// Whether a higher activity value is better (potency in %, pIC50) or a
/// lower one (IC50, Ki).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDirection {
    /// Higher is better.
    #[default]
    Higher,
    /// Lower is better.
    Lower,
}

/// Descriptors computed from a structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Descriptors {
    /// Average molecular weight, g/mol.
    pub mw: f64,
    /// Heavy (non-hydrogen) atoms.
    pub heavy_atoms: u32,
    /// Hydrogen-bond donors.
    pub hbd: u32,
    /// Hydrogen-bond acceptors.
    pub hba: u32,
    /// Rotatable bonds.
    pub rotatable_bonds: u32,
    /// Rings (SSSR size).
    pub rings: u32,
    /// Aromatic rings.
    pub aromatic_rings: u32,
    /// Crippen logP (Wildman & Crippen 1999).
    pub logp: f64,
    /// Topological polar surface area, Å² (Ertl 2000, N, O, S and P).
    pub tpsa: f64,
}

impl Descriptors {
    /// Lipinski rule-of-five violations: MW > 500, logP > 5, HBD > 5,
    /// HBA > 10.
    #[must_use]
    pub fn lipinski_violations(&self) -> u32 {
        u32::from(self.mw > 500.0)
            + u32::from(self.logp > 5.0)
            + u32::from(self.hbd > 5)
            + u32::from(self.hba > 10)
    }

    /// The value of descriptor `key` (see [`DESCRIPTORS`]).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<f64> {
        Some(match key {
            "mw" => self.mw,
            "heavy_atoms" => f64::from(self.heavy_atoms),
            "hbd" => f64::from(self.hbd),
            "hba" => f64::from(self.hba),
            "rotatable_bonds" => f64::from(self.rotatable_bonds),
            "rings" => f64::from(self.rings),
            "aromatic_rings" => f64::from(self.aromatic_rings),
            "logp" => self.logp,
            "tpsa" => self.tpsa,
            "ro5_violations" => f64::from(self.lipinski_violations()),
            _ => return None,
        })
    }
}

/// The descriptor keys with their labels, in table order.
pub const DESCRIPTORS: [(&str, &str); 10] = [
    ("mw", "MW"),
    ("heavy_atoms", "heavy atoms"),
    ("hbd", "HBD"),
    ("hba", "HBA"),
    ("rotatable_bonds", "rot. bonds"),
    ("rings", "rings"),
    ("aromatic_rings", "arom. rings"),
    ("logp", "cLogP"),
    ("tpsa", "TPSA"),
    ("ro5_violations", "Ro5 viol."),
];

/// One molecule of the grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GridMolecule {
    /// Its id (compound number), unique in the grid.
    pub id: String,
    /// The structure, depicted (or the parse error).
    pub molecule: MoleculeSpec,
    /// The measured activity, as given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<f64>,
    /// Descriptors, when the SMILES parsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptors: Option<Descriptors>,
}

/// A grid of molecules with descriptors and activities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MoleculeGridSpec {
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// What the activity is (`IC50`, `% inhibition at 10 µM`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_label: Option<String>,
    /// Its unit (`nM`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_unit: Option<String>,
    /// Which way is better, for the colours.
    #[serde(default)]
    pub direction: ActivityDirection,
    /// The molecules in the order given.
    pub molecules: Vec<GridMolecule>,
    /// The descriptors as an envelope (values `<id>.<descriptor>`).
    pub descriptors: Computed,
}

impl MoleculeGridSpec {
    /// Checks the size, unique ids and the envelope.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "molecule_grid",
            reason,
        };
        if self.molecules.is_empty() {
            return Err(invalid("the grid has no molecules".into()));
        }
        if self.molecules.len() > MAX_MOLECULES {
            return Err(invalid(format!(
                "{} molecules; at most {MAX_MOLECULES}",
                self.molecules.len()
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for m in &self.molecules {
            if !seen.insert(m.id.as_str()) {
                return Err(invalid(format!("id `{}` is used twice", m.id)));
            }
            if m.activity.is_some_and(|a| !a.is_finite()) {
                return Err(invalid(format!("activity of `{}` is not finite", m.id)));
            }
        }
        self.descriptors
            .validate()
            .map_err(|e| invalid(format!("descriptor envelope: {e}")))
    }

    /// The range of the activities given, if any.
    #[must_use]
    pub fn activity_range(&self) -> Option<(f64, f64)> {
        let mut values = self.molecules.iter().filter_map(|m| m.activity);
        let first = values.next()?;
        Some(values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }

    /// Where `activity` falls from worst (0) to best (1).
    #[must_use]
    pub fn activity_score(&self, activity: f64) -> Option<f64> {
        let (lo, hi) = self.activity_range()?;
        if hi <= lo {
            return Some(0.5);
        }
        let t = (activity - lo) / (hi - lo);
        Some(match self.direction {
            ActivityDirection::Higher => t,
            ActivityDirection::Lower => 1.0 - t,
        })
    }

    /// Molecules whose SMILES did not parse.
    #[must_use]
    pub fn failed(&self) -> Vec<&GridMolecule> {
        self.molecules
            .iter()
            .filter(|m| m.molecule.error.is_some())
            .collect()
    }

    /// The grid as CSV: id, SMILES, name, activity, formula and every
    /// descriptor.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let mut out = String::from("id,smiles,name,activity,formula");
        for (key, _) in DESCRIPTORS {
            out.push(',');
            out.push_str(key);
        }
        out.push('\n');
        for m in &self.molecules {
            let mut row = vec![
                field(&m.id),
                field(&m.molecule.smiles),
                field(m.molecule.name.as_deref().unwrap_or("")),
                m.activity.map(|a| a.to_string()).unwrap_or_default(),
                m.molecule.formula.clone().unwrap_or_default(),
            ];
            for (key, _) in DESCRIPTORS {
                row.push(
                    m.descriptors
                        .as_ref()
                        .and_then(|d| d.get(key))
                        .map(|v| format!("{}", (v * 1000.0).round() / 1000.0))
                        .unwrap_or_default(),
                );
            }
            out.push_str(&row.join(","));
            out.push('\n');
        }
        out
    }
}

/// `id` made safe as an envelope key: anything but letters, digits, `_`
/// and `-` becomes `_`.
#[must_use]
pub fn value_key(id: &str) -> String {
    let key: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if key.is_empty() { "_".to_owned() } else { key }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn descriptors(mw: f64, logp: f64) -> Descriptors {
        Descriptors {
            mw,
            heavy_atoms: 13,
            hbd: 1,
            hba: 3,
            rotatable_bonds: 2,
            rings: 1,
            aromatic_rings: 1,
            logp,
            tpsa: 63.6,
        }
    }

    fn molecule(id: &str, activity: Option<f64>) -> GridMolecule {
        GridMolecule {
            id: id.into(),
            molecule: MoleculeSpec {
                smiles: "C".into(),
                name: None,
                svg: None,
                formula: Some("CH4".into()),
                mw: Some(16.0),
                charge: Some(0),
                canonical: None,
                error: None,
                claimed_formula: None,
                claimed_matches: None,
            },
            activity,
            descriptors: Some(descriptors(180.16, 1.31)),
        }
    }

    fn spec() -> MoleculeGridSpec {
        MoleculeGridSpec {
            title: None,
            activity_label: Some("IC50".into()),
            activity_unit: Some("nM".into()),
            direction: ActivityDirection::Lower,
            molecules: vec![molecule("a 1", Some(10.0)), molecule("b", Some(1000.0))],
            descriptors: Computed::new(
                "ch_1",
                "chem.descriptors",
                "descriptors",
                Engine {
                    name: "crest-chem".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
        }
    }

    #[test]
    fn scores_violations_and_csv() {
        let spec = spec();
        spec.validate().unwrap();
        // Lower is better: the 10 nM compound scores best.
        assert_eq!(spec.activity_score(10.0), Some(1.0));
        assert_eq!(spec.activity_score(1000.0), Some(0.0));
        assert_eq!(descriptors(600.0, 6.0).lipinski_violations(), 2);
        assert_eq!(value_key("a 1/x"), "a_1_x");
        let csv = spec.to_csv();
        assert!(csv.starts_with("id,smiles,name,activity,formula,mw,"), "{csv}");
        assert!(csv.contains("a 1,C,,10,CH4,180.16,13,1,3,2,1,1,1.31,63.6,0\n"), "{csv}");
        let mut twice = spec.clone();
        twice.molecules[1].id = "a 1".into();
        assert!(twice.validate().is_err());
    }
}
