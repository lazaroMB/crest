//! Reaction schemes for `show_reaction`: reaction SMILES read with
//! chematic's reaction parser, each molecule depicted like
//! `show_molecule`, and the atom and charge balance checked.
//!
//! Atoms are counted from each molecule's computed formula, implicit
//! hydrogens included; agents (over the arrow) are not counted. When the
//! given coefficients do not balance, the null space of the
//! element-by-molecule matrix (charge as one more row) is found in exact
//! rational arithmetic: one dimension with all coefficients of one sign
//! gives the smallest integer coefficients; none means the molecules
//! cannot balance; more than one means two reactions are mixed.

use std::collections::{BTreeMap, BTreeSet};

use chematic::rxn;
use chematic::smiles;
use domain::computed::{Computed, ComputedStatus, ComputedValue, ComputedWarning, WarningSeverity};
use domain::science::parse_formula;
use domain::science::reaction::{
    ElementBalance, MAX_COEFFICIENT, MAX_COMPONENTS, ReactionComponent, ReactionRole, ReactionSpec,
    Stoichiometry,
};
use serde_json::json;

use crate::chem;
use crate::molecule::{self, MAX_SMILES_BYTES};

/// Kind of the balance envelope.
pub const KIND: &str = "chem.reaction_balance";
/// Longest reaction SMILES accepted, in bytes.
pub const MAX_REACTION_BYTES: usize = 4 * MAX_SMILES_BYTES;

/// The scheme of `smiles` (`A.B>agents>C.D`) with its balance;
/// `coefficients` (reactants then products) default to 1 each. A
/// reaction that cannot be read comes back with `error` set and an error
/// envelope.
#[must_use]
pub fn depict(smiles_text: &str, coefficients: Option<&[u32]>) -> ReactionSpec {
    let source = smiles_text.trim();
    let mut spec = ReactionSpec {
        title: None,
        smiles: source.to_owned(),
        conditions: None,
        components: Vec::new(),
        elements: Vec::new(),
        charge: (0, 0),
        stoichiometry: Stoichiometry::NotChecked,
        balance: envelope(source, coefficients),
        error: None,
    };
    if let Err(error) = read(&mut spec, source, coefficients) {
        spec.balance.status = ComputedStatus::Error;
        spec.balance.error = Some(error.clone());
        spec.error = Some(error);
        return spec;
    }
    balance(&mut spec);
    spec
}

fn envelope(source: &str, coefficients: Option<&[u32]>) -> Computed {
    let bytes = serde_json::to_vec(&json!({ "smiles": source, "coefficients": coefficients }))
        .unwrap_or_default();
    let mut computed = Computed::new(
        chem::derive_id(KIND, &bytes),
        KIND,
        "Atom and charge balance",
        chem::engine(chem::CHEMATIC),
    );
    computed.method.why = Some(
        "Atoms of each molecule counted from its structure (implicit hydrogens included); \
         agents are not counted."
            .into(),
    );
    computed.method.alternatives = vec!["smallest integer coefficients (exact null space)".into()];
    computed.inputs = vec![chem::input("reaction SMILES", "param", &bytes, None)];
    computed.reproduce.lua = Some(format!("app.formats.reaction{{ smiles = {source:?} }}"));
    computed
}

fn read(spec: &mut ReactionSpec, source: &str, coefficients: Option<&[u32]>) -> Result<(), String> {
    if source.len() > MAX_REACTION_BYTES {
        return Err(format!(
            "the reaction SMILES is too long ({} bytes, limit {MAX_REACTION_BYTES})",
            source.len()
        ));
    }
    if source.matches('>').count() < 2 {
        return Err(
            "not a reaction SMILES: write reactants>agents>products, e.g. CCO.O=O>>CC=O.O".into(),
        );
    }
    let reaction =
        rxn::parse_reaction(source).map_err(|e| format!("invalid reaction SMILES: {e}"))?;
    if reaction.reactants.is_empty() || reaction.products.is_empty() {
        return Err("the reaction needs at least one reactant and one product".into());
    }
    for (role, mols) in [
        (ReactionRole::Reactant, &reaction.reactants),
        (ReactionRole::Agent, &reaction.agents),
        (ReactionRole::Product, &reaction.products),
    ] {
        if mols.len() > MAX_COMPONENTS {
            return Err(format!(
                "{} molecules on one side; at most {MAX_COMPONENTS}",
                mols.len()
            ));
        }
        for mol in mols {
            let text = smiles::canonical_smiles(mol);
            let molecule = molecule::depict_mol(mol, &text);
            let atoms = molecule
                .formula
                .as_deref()
                .and_then(parse_formula)
                .unwrap_or_default();
            spec.components.push(ReactionComponent {
                role,
                molecule,
                coefficient: 1,
                atoms,
            });
        }
    }
    let counted = spec
        .components
        .iter()
        .filter(|c| c.role != ReactionRole::Agent)
        .count();
    if let Some(given) = coefficients {
        if given.len() != counted {
            return Err(format!(
                "{} coefficients given for {counted} reactants and products",
                given.len()
            ));
        }
        if given.contains(&0) {
            return Err("coefficients must be at least 1".into());
        }
        let mut given = given.iter();
        for c in spec
            .components
            .iter_mut()
            .filter(|c| c.role != ReactionRole::Agent)
        {
            c.coefficient = given.next().copied().unwrap_or(1);
        }
    }
    Ok(())
}

fn balance(spec: &mut ReactionSpec) {
    let counted: Vec<&ReactionComponent> = spec
        .components
        .iter()
        .filter(|c| c.role != ReactionRole::Agent)
        .collect();
    let elements: BTreeSet<String> = counted
        .iter()
        .flat_map(|c| c.atoms.keys().cloned())
        .collect();
    let mut table = Vec::new();
    for element in &elements {
        let (mut left, mut right) = (0u64, 0u64);
        for c in &counted {
            let n = u64::from(*c.atoms.get(element).unwrap_or(&0)) * u64::from(c.coefficient);
            match c.role {
                ReactionRole::Reactant => left += n,
                ReactionRole::Product => right += n,
                ReactionRole::Agent => {}
            }
        }
        table.push(ElementBalance {
            element: element.clone(),
            reactants: left,
            products: right,
        });
    }
    let charge_of = |c: &ReactionComponent| {
        i64::from(c.molecule.charge.unwrap_or(0)) * i64::from(c.coefficient)
    };
    let left: i64 = counted
        .iter()
        .filter(|c| c.role == ReactionRole::Reactant)
        .map(|c| charge_of(c))
        .sum();
    let right: i64 = counted
        .iter()
        .filter(|c| c.role == ReactionRole::Product)
        .map(|c| charge_of(c))
        .sum();
    // The matrix: a row per element and one for the charge, reactant
    // columns positive and product columns negative.
    let mut matrix: Vec<Vec<i64>> = elements
        .iter()
        .map(|element| {
            counted
                .iter()
                .map(|c| {
                    let n = i64::from(*c.atoms.get(element).unwrap_or(&0));
                    if c.role == ReactionRole::Product {
                        -n
                    } else {
                        n
                    }
                })
                .collect()
        })
        .collect();
    matrix.push(
        counted
            .iter()
            .map(|c| {
                let q = i64::from(c.molecule.charge.unwrap_or(0));
                if c.role == ReactionRole::Product {
                    -q
                } else {
                    q
                }
            })
            .collect(),
    );
    spec.elements = table;
    spec.charge = (left, right);
    spec.stoichiometry = if spec.is_balanced() {
        Stoichiometry::Balanced
    } else {
        solve(&matrix)
    };
    fill_envelope(spec);
}

fn fill_envelope(spec: &mut ReactionSpec) {
    let balanced = spec.is_balanced();
    let mut values = BTreeMap::new();
    let mut flag = ComputedValue::new(if balanced { 1.0 } else { 0.0 });
    flag.display = Some(if balanced { "yes" } else { "no" }.into());
    flag.label = Some("balanced".into());
    values.insert("balanced".to_owned(), flag);
    let mut charge = ComputedValue::new((spec.charge.1 - spec.charge.0) as f64);
    charge.decimals = Some(0);
    values.insert("charge_delta".to_owned(), charge);
    for e in &spec.elements {
        let mut v = ComputedValue::new(e.delta() as f64);
        v.decimals = Some(0);
        values.insert(format!("delta.{}", e.element), v);
    }
    let mut warnings = Vec::new();
    match &spec.stoichiometry {
        Stoichiometry::Suggested { coefficients } => {
            for (i, c) in coefficients.iter().enumerate() {
                let mut v = ComputedValue::new(f64::from(*c));
                v.decimals = Some(0);
                values.insert(format!("suggested.{}", i + 1), v);
            }
            spec.balance
                .method
                .params
                .insert("suggested".into(), json!(spec.equation(Some(coefficients))));
        }
        Stoichiometry::Impossible => warnings.push(ComputedWarning {
            code: "cannot_balance".into(),
            severity: WarningSeverity::Warn,
            text: "No positive coefficients balance these molecules: a by-product, \
                   counter-ion or electron is missing, or a structure is wrong."
                .into(),
        }),
        Stoichiometry::Ambiguous => warnings.push(ComputedWarning {
            code: "several_balances".into(),
            severity: WarningSeverity::Info,
            text: "More than one independent set of coefficients balances it (two \
                   reactions in one); none is suggested."
                .into(),
        }),
        Stoichiometry::Balanced | Stoichiometry::NotChecked => {}
    }
    if !balanced {
        let missing: Vec<String> = spec
            .unbalanced()
            .iter()
            .map(|e| format!("{} {:+}", e.element, e.delta()))
            .collect();
        let mut text = if missing.is_empty() {
            String::new()
        } else {
            format!("Atoms (products − reactants): {}.", missing.join(", "))
        };
        if spec.charge.0 != spec.charge.1 {
            text.push_str(&format!(" Charge {} → {}.", spec.charge.0, spec.charge.1));
        }
        warnings.insert(
            0,
            ComputedWarning {
                code: "unbalanced".into(),
                severity: WarningSeverity::Warn,
                text: text.trim().to_owned(),
            },
        );
    }
    spec.balance.status = if warnings.is_empty() {
        ComputedStatus::Ok
    } else {
        ComputedStatus::Warning
    };
    spec.balance.values = values;
    spec.balance.warnings = warnings;
    spec.balance
        .method
        .params
        .insert("equation".into(), json!(spec.equation(None)));
}

/// An exact fraction `n / d` with `d > 0`, reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Q(i128, i128);

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Q {
    fn new(n: i128, d: i128) -> Self {
        let g = gcd(n, d).max(1);
        let s = if d < 0 { -1 } else { 1 };
        Self(s * n / g, s * d / g)
    }
    fn int(n: i64) -> Self {
        Self(i128::from(n), 1)
    }
    fn is_zero(self) -> bool {
        self.0 == 0
    }
    fn sub(self, o: Self) -> Self {
        Self::new(self.0 * o.1 - o.0 * self.1, self.1 * o.1)
    }
    fn mul(self, o: Self) -> Self {
        Self::new(self.0 * o.0, self.1 * o.1)
    }
    fn div(self, o: Self) -> Self {
        Self::new(self.0 * o.1, self.1 * o.0)
    }
}

/// The smallest positive integer vector `x` with `matrix · x = 0`, when
/// the null space has one dimension and a positive direction.
fn solve(matrix: &[Vec<i64>]) -> Stoichiometry {
    let cols = matrix.first().map_or(0, Vec::len);
    if cols == 0 {
        return Stoichiometry::Impossible;
    }
    let mut m: Vec<Vec<Q>> = matrix
        .iter()
        .map(|row| row.iter().map(|&v| Q::int(v)).collect())
        .collect();
    // Reduced row echelon form.
    let mut pivots = Vec::new();
    let mut r = 0;
    for c in 0..cols {
        let Some(p) = (r..m.len()).find(|&i| !m[i][c].is_zero()) else {
            continue;
        };
        m.swap(r, p);
        let lead = m[r][c];
        for v in &mut m[r] {
            *v = v.div(lead);
        }
        let pivot_row = m[r].clone();
        for (i, row) in m.iter_mut().enumerate() {
            if i != r && !row[c].is_zero() {
                let f = row[c];
                for (v, p) in row.iter_mut().zip(&pivot_row) {
                    *v = v.sub(p.mul(f));
                }
            }
        }
        pivots.push(c);
        r += 1;
        if r == m.len() {
            break;
        }
    }
    let free: Vec<usize> = (0..cols).filter(|c| !pivots.contains(c)).collect();
    match free.len() {
        0 => return Stoichiometry::Impossible,
        1 => {}
        _ => return Stoichiometry::Ambiguous,
    }
    let f = free[0];
    let mut x = vec![Q::int(0); cols];
    x[f] = Q::int(1);
    for (row, &c) in pivots.iter().enumerate() {
        x[c] = Q::int(0).sub(m[row][f]);
    }
    // Clear the denominators and the common factor.
    let lcm = x
        .iter()
        .fold(1i128, |acc, q| acc / gcd(acc, q.1).max(1) * q.1);
    let ints: Vec<i128> = x.iter().map(|q| q.0 * (lcm / q.1)).collect();
    let g = ints.iter().fold(0i128, |acc, &v| gcd(acc, v)).max(1);
    let mut ints: Vec<i128> = ints.iter().map(|v| v / g).collect();
    if ints.iter().all(|&v| v <= 0) {
        for v in &mut ints {
            *v = -*v;
        }
    }
    if ints.iter().any(|&v| v <= 0) {
        return Stoichiometry::Impossible;
    }
    let coefficients: Option<Vec<u32>> = ints
        .iter()
        .map(|&v| {
            u64::try_from(v)
                .ok()
                .filter(|&v| v <= MAX_COEFFICIENT)
                .and_then(|v| u32::try_from(v).ok())
        })
        .collect();
    coefficients.map_or(Stoichiometry::Impossible, |coefficients| {
        Stoichiometry::Suggested { coefficients }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_solver_finds_the_smallest_coefficients() {
        // H2 + O2 -> H2O: rows H, O, charge.
        let m = vec![vec![2, 0, -2], vec![0, 2, -1], vec![0, 0, 0]];
        assert_eq!(
            solve(&m),
            Stoichiometry::Suggested {
                coefficients: vec![2, 1, 2]
            }
        );
        // C -> O cannot balance.
        assert_eq!(solve(&[vec![1, 0], vec![0, -1]]), Stoichiometry::Impossible);
        // Two independent reactions: A -> B and C -> D with A=B, C=D.
        let two = vec![vec![1, 0, -1, 0], vec![0, 1, 0, -1]];
        assert_eq!(solve(&two), Stoichiometry::Ambiguous);
    }
}
