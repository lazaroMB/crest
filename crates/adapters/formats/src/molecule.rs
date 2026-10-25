//! 2D molecules from SMILES with `chematic` (pure Rust): canonical
//! SMILES, Hill formula, average molecular weight, net charge and an SVG
//! depiction.
//!
//! The depiction keeps chematic's light style (white background, black
//! bonds and carbons, CPK colours for heteroatom labels); the block draws
//! it on a light rounded card, so it stays legible in a dark theme.

use chematic::chem;
use chematic::depict::{self, RenderOptions};
use chematic::smiles;
use domain::science::{MoleculeSpec, formulas_match};

/// Longest SMILES accepted, in bytes.
pub const MAX_SMILES_BYTES: usize = 4_096;

/// Parses `smiles` and fills in everything computed from it; a SMILES
/// that does not parse comes back with `error` set.
#[must_use]
pub fn depict(smiles_text: &str, claimed_formula: Option<&str>) -> MoleculeSpec {
    let mut spec = MoleculeSpec {
        smiles: smiles_text.to_owned(),
        name: None,
        svg: None,
        formula: None,
        mw: None,
        charge: None,
        canonical: None,
        error: None,
        claimed_formula: claimed_formula.map(str::to_owned),
        claimed_matches: None,
    };
    let source = smiles_text.trim();
    if source.len() > MAX_SMILES_BYTES {
        spec.error = Some(format!(
            "SMILES is too long ({} bytes, limit {MAX_SMILES_BYTES})",
            source.len()
        ));
        return spec;
    }
    let mol = match smiles::parse(source) {
        Ok(mol) => mol,
        Err(e) => {
            spec.error = Some(format!("invalid SMILES: {e}"));
            return spec;
        }
    };
    let formula = neutral_formula(&chem::calc_mol_formula(&mol));
    spec.claimed_matches = claimed_formula.and_then(|claimed| formulas_match(claimed, &formula));
    spec.formula = Some(formula);
    spec.mw = Some(chem::molecular_weight(&mol));
    spec.charge = Some(chem::formal_charge_sum(&mol));
    spec.canonical = Some(smiles::canonical_smiles(&mol));
    // Kekulé bonds read better than chematic's dashed aromatic style.
    let options = RenderOptions {
        kekulize: true,
        ..RenderOptions::default()
    };
    spec.svg = Some(depict::depict_svg_opts(&mol, &options));
    spec
}

/// `formula` without the net charge RDKit-style formulas end with
/// (`C7H5O2-` → `C7H5O2`, `O4S-2` → `O4S`).
fn neutral_formula(formula: &str) -> String {
    let body = formula.trim_end_matches(|c: char| c.is_ascii_digit());
    match body.strip_suffix(['+', '-']) {
        Some(neutral) => neutral.to_owned(),
        None => formula.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_are_cut_from_formulas() {
        assert_eq!(neutral_formula("C7H5O2-"), "C7H5O2");
        assert_eq!(neutral_formula("O4S-2"), "O4S");
        assert_eq!(neutral_formula("H4N+"), "H4N");
        assert_eq!(neutral_formula("C2H6O"), "C2H6O");
    }
}
