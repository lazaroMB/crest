//! Molecule grids for `show_molecule_grid`: each SMILES depicted like
//! `show_molecule`, with chematic's descriptors.
//!
//! The descriptors and their definitions: average molecular weight;
//! heavy atoms; H-bond donors and acceptors with RDKit's SMARTS
//! (`CalcNumHBD`/`CalcNumHBA`); rotatable bonds; rings (SSSR size) and
//! aromatic rings; Crippen logP (Wildman & Crippen 1999, RDKit's atom
//! types); TPSA (Ertl 2000, with the S and P contributions, as RDKit's
//! `includeSandP=True`). Each value also goes into the envelope as
//! `<id>.<descriptor>`.

use std::collections::{BTreeMap, HashSet};

use application::ports::formats::{GridEntry, MoleculeGridRequest};
use chematic::chem;
use chematic::smiles;
use domain::computed::{
    Computed, ComputedStatus, ComputedValue, ComputedWarning, WarningSeverity,
};
use domain::science::molecule_grid::{
    DESCRIPTORS, Descriptors, GridMolecule, MAX_MOLECULES, MoleculeGridSpec, value_key,
};

use crate::chem as envelope;
use crate::molecule::{self, MAX_SMILES_BYTES};

/// Kind of the descriptor envelope.
pub const KIND: &str = "chem.descriptors";

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The molecules of `request` depicted with their descriptors.
///
/// # Errors
///
/// A message for an empty or oversized grid, an empty id or a repeated
/// one.
pub fn grid(request: &MoleculeGridRequest) -> Result<MoleculeGridSpec, String> {
    if request.molecules.is_empty() {
        return Err("no molecules given".into());
    }
    if request.molecules.len() > MAX_MOLECULES {
        return Err(format!(
            "{} molecules; at most {MAX_MOLECULES} in one grid",
            request.molecules.len()
        ));
    }
    let mut seen = HashSet::new();
    for m in &request.molecules {
        if m.id.trim().is_empty() {
            return Err(format!("the molecule `{}` has no id", m.smiles));
        }
        if !seen.insert(m.id.trim()) {
            return Err(format!("id `{}` is used twice", m.id.trim()));
        }
    }
    let bytes = serde_json::to_vec(&request.molecules).unwrap_or_default();
    let mut computed = Computed::new(
        envelope::derive_id(KIND, &bytes),
        KIND,
        "Molecular descriptors",
        envelope::engine(envelope::CHEMATIC),
    );
    computed.method.why = Some(
        "MW (average), heavy atoms, HBD/HBA (RDKit SMARTS), rotatable bonds, rings (SSSR), \
         Crippen logP (Wildman & Crippen 1999), TPSA (Ertl 2000, N, O, S and P), \
         rule-of-five violations"
            .into(),
    );
    computed.inputs = vec![envelope::input("molecules", "data", &bytes, None)];
    let mut values = BTreeMap::new();
    let mut molecules = Vec::with_capacity(request.molecules.len());
    let mut used_keys = HashSet::new();
    for entry in &request.molecules {
        let item = one(entry);
        if let Some(d) = &item.descriptors {
            // Two ids that differ only in punctuation share a key: the
            // second gets its position.
            let mut key = value_key(&item.id);
            if !used_keys.insert(key.clone()) {
                key = format!("{key}_{}", molecules.len() + 1);
                used_keys.insert(key.clone());
            }
            for (name, _) in DESCRIPTORS {
                if let Some(v) = d.get(name) {
                    let mut value = ComputedValue::new(v);
                    value.decimals = Some(match name {
                        "mw" | "logp" | "tpsa" => 2,
                        _ => 0,
                    });
                    value.unit = match name {
                        "mw" => Some("g/mol".into()),
                        "tpsa" => Some("Å²".into()),
                        _ => None,
                    };
                    values.insert(format!("{key}.{name}"), value);
                }
            }
        }
        molecules.push(item);
    }
    let failed = molecules
        .iter()
        .filter(|m: &&GridMolecule| m.molecule.error.is_some())
        .count();
    if failed > 0 {
        computed.status = ComputedStatus::Warning;
        computed.warnings.push(ComputedWarning {
            code: "smiles_not_parsed".into(),
            severity: WarningSeverity::Warn,
            text: format!("{failed} SMILES could not be parsed and have no descriptors."),
        });
    }
    computed.values = values;
    Ok(MoleculeGridSpec {
        title: None,
        activity_label: request.activity_label.clone(),
        activity_unit: request.activity_unit.clone(),
        direction: request.direction,
        molecules,
        descriptors: computed,
    })
}

fn one(entry: &GridEntry) -> GridMolecule {
    let source = entry.smiles.trim();
    let mut item = GridMolecule {
        id: entry.id.trim().to_owned(),
        molecule: molecule::depict(source, None),
        activity: entry.activity.filter(|a| a.is_finite()),
        descriptors: None,
    };
    item.molecule.name = entry.name.clone().filter(|n| !n.trim().is_empty());
    if item.molecule.error.is_some() || source.len() > MAX_SMILES_BYTES {
        return item;
    }
    let Ok(mol) = smiles::parse(source) else {
        return item;
    };
    item.descriptors = Some(Descriptors {
        mw: chem::molecular_weight(&mol),
        heavy_atoms: count(chem::heavy_atom_count(&mol)),
        hbd: count(chem::hbd_count(&mol)),
        hba: count(chem::hba_count(&mol)),
        rotatable_bonds: count(chem::rotatable_bond_count(&mol)),
        rings: count(chem::ring_count(&mol)),
        aromatic_rings: count(chem::aromatic_ring_count(&mol)),
        logp: chem::logp_crippen(&mol),
        tpsa: chem::tpsa(&mol),
    });
    item
}
