//! Molecules in 3D for `show_molecule_3d`.
//!
//! From SMILES, chematic adds the hydrogens and embeds the molecule
//! (distance geometry, then a DREIDING minimisation): a computed
//! conformer, said so in the block. From files: MOL/SDF V2000 with our
//! own atom and bond block reader (chematic's V2000 reader keeps only x
//! and y), V3000 through chematic, XYZ through chematic with bonds
//! inferred from covalent radii, PDB `ATOM`/`HETATM` records through
//! chematic (bonds from distances too). A molfile whose z coordinates
//! are all zero (and whose header does not say 3D) is a 2D drawing and is
//! refused rather than shown flat.

use chematic::chem;
use chematic::core::{BondOrder, Molecule};
use chematic::smiles;
use chematic::threed::{self, Coords3D, Point3};
use domain::science::molecule3d::{
    Atom3d, Bond3d, CoordinateSource, MAX_ATOMS, Molecule3dSpec, StructureFormat,
};

use crate::molecule::MAX_SMILES_BYTES;

/// Most heavy atoms embedded from SMILES.
pub const MAX_EMBED_HEAVY_ATOMS: usize = 250;
/// Extra distance over the covalent radii that still makes a bond, Å.
const BOND_TOLERANCE: f64 = 0.4;
/// The embedding, as the block names it.
pub const EMBED_METHOD: &str = "distance geometry + DREIDING minimisation (chematic 1.2.1)";

fn order(o: BondOrder) -> u8 {
    match o {
        BondOrder::Double => 2,
        BondOrder::Triple => 3,
        BondOrder::Aromatic => 4,
        _ => 1,
    }
}

fn spec_of(
    mol: &Molecule,
    coords: &Coords3D,
    source: CoordinateSource,
) -> Result<Molecule3dSpec, String> {
    if coords.points.len() != mol.atom_count() {
        return Err(format!(
            "{} coordinates for {} atoms",
            coords.points.len(),
            mol.atom_count()
        ));
    }
    let atoms: Vec<Atom3d> = mol
        .atoms()
        .zip(&coords.points)
        .map(|((_, a), p)| Atom3d {
            element: a.element.symbol().to_owned(),
            position: [p.x, p.y, p.z],
            name: None,
            residue: None,
        })
        .collect();
    let bonds = mol
        .bonds()
        .map(|(_, b)| Bond3d {
            a: b.atom1.0,
            b: b.atom2.0,
            order: order(b.order),
        })
        .collect();
    Ok(Molecule3dSpec {
        title: None,
        name: None,
        smiles: None,
        path: None,
        source,
        atoms,
        bonds,
        bonds_inferred: false,
        warnings: Vec::new(),
    })
}

fn check(spec: &Molecule3dSpec) -> Result<(), String> {
    if spec.atoms.len() > MAX_ATOMS {
        return Err(format!(
            "{} atoms; at most {MAX_ATOMS} can be drawn",
            spec.atoms.len()
        ));
    }
    if spec
        .atoms
        .iter()
        .any(|a| a.position.iter().any(|v| !v.is_finite()))
    {
        return Err("a coordinate is not finite".into());
    }
    Ok(())
}

/// The engine's conformer of `smiles_text`, hydrogens included.
///
/// # Errors
///
/// A message when the SMILES does not parse, is too large to embed, or
/// the embedding fails.
pub fn embed(smiles_text: &str) -> Result<Molecule3dSpec, String> {
    let source = smiles_text.trim();
    if source.len() > MAX_SMILES_BYTES {
        return Err(format!(
            "SMILES is too long (limit {MAX_SMILES_BYTES} bytes)"
        ));
    }
    let mol = smiles::parse(source).map_err(|e| format!("invalid SMILES: {e}"))?;
    let heavy = chem::heavy_atom_count(&mol);
    if heavy > MAX_EMBED_HEAVY_ATOMS {
        return Err(format!(
            "{heavy} heavy atoms; at most {MAX_EMBED_HEAVY_ATOMS} are embedded from SMILES (give a \
             structure file instead)"
        ));
    }
    let with_h = chem::add_hydrogens(&mol);
    let coords = threed::generate_and_minimize_dreiding(&with_h);
    if !coords.is_finite() {
        return Err("the embedding did not converge (coordinates are not finite)".into());
    }
    let mut spec = spec_of(
        &with_h,
        &coords,
        CoordinateSource::Embedded {
            method: EMBED_METHOD.into(),
        },
    )?;
    spec.smiles = Some(source.to_owned());
    spec.warnings
        .push("A computed conformer from the SMILES, not an experimental structure.".into());
    check(&spec)?;
    Ok(spec)
}

/// The format of a structure file, from its name or content.
#[must_use]
pub fn format_of(path: &str, text: &str) -> Option<StructureFormat> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "mol" => return Some(StructureFormat::Mol),
        "sdf" | "sd" => return Some(StructureFormat::Sdf),
        "xyz" => return Some(StructureFormat::Xyz),
        "pdb" | "ent" => return Some(StructureFormat::Pdb),
        _ => {}
    }
    if text.contains("$$$$") {
        Some(StructureFormat::Sdf)
    } else if text.contains("V2000") || text.contains("V3000") {
        Some(StructureFormat::Mol)
    } else if text
        .lines()
        .any(|l| l.starts_with("ATOM  ") || l.starts_with("HETATM"))
    {
        Some(StructureFormat::Pdb)
    } else if text
        .lines()
        .next()
        .is_some_and(|l| l.trim().parse::<usize>().is_ok())
    {
        Some(StructureFormat::Xyz)
    } else {
        None
    }
}

/// The molecule of a structure file `path`.
///
/// # Errors
///
/// A message when the format is not known, the file does not read, or it
/// holds no 3D coordinates.
pub fn read(bytes: &[u8], path: &str) -> Result<Molecule3dSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())?;
    let format = format_of(path, text)
        .ok_or("not a structure file: give a .mol, .sdf, .xyz or .pdb file")?;
    let mut spec = match format {
        StructureFormat::Mol | StructureFormat::Sdf => molfile(text, format)?,
        StructureFormat::Xyz => xyz(text)?,
        StructureFormat::Pdb => pdb(text)?,
    };
    spec.path = Some(path.to_owned());
    check(&spec)?;
    // A molfile with every z at 0 is a drawing, unless its header line
    // says 3D (a planar molecule); XYZ and PDB are always 3D data.
    let header_3d = text
        .lines()
        .nth(1)
        .and_then(|l| l.get(20..22))
        .is_some_and(|d| d == "3D");
    if matches!(format, StructureFormat::Mol | StructureFormat::Sdf)
        && !header_3d
        && spec.atoms.len() > 1
        && spec.atoms.iter().all(|a| a.position[2].abs() < 1e-6)
    {
        return Err(
            "the file has only 2D coordinates (every z is 0): it is a drawing, not a 3D \
             structure; show it with show_molecule, or give 3D coordinates"
                .into(),
        );
    }
    Ok(spec)
}

/// A V2000 counts line's `aaa` and `bbb`.
fn counts(line: &str) -> Option<(usize, usize)> {
    let field = |a: usize, b: usize| line.get(a..b).and_then(|s| s.trim().parse().ok());
    Some((field(0, 3)?, field(3, 6)?))
}

fn molfile(text: &str, format: StructureFormat) -> Result<Molecule3dSpec, String> {
    // The first record of an SD file.
    let record = text.split("$$$$").next().unwrap_or(text);
    let records = text.split("$$$$").filter(|r| !r.trim().is_empty()).count();
    let lines: Vec<&str> = record.lines().collect();
    let name = lines
        .first()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    let counts_line = lines.get(3).ok_or("the molfile has no counts line")?;
    let mut spec = if counts_line.contains("V3000") {
        let (mol, _, coords) = chematic::mol::parse_mol_v3000_with_coords_3d(record)
            .map_err(|e| format!("invalid V3000 molfile: {e}"))?;
        let coords = Coords3D {
            points: coords
                .into_iter()
                .map(|(x, y, z)| Point3 { x, y, z })
                .collect(),
        };
        spec_of(&mol, &coords, CoordinateSource::File { format })?
    } else {
        v2000(&lines, format)?
    };
    spec.name = name;
    if records > 1 {
        spec.warnings.push(format!(
            "{records} records in the SD file; showing the first"
        ));
    }
    Ok(spec)
}

/// Our V2000 reader: the atom block (x, y, z, symbol) and the bond block
/// (atoms, order); the properties block is not read (charges are not
/// drawn).
fn v2000(lines: &[&str], format: StructureFormat) -> Result<Molecule3dSpec, String> {
    let (atoms_n, bonds_n) = lines
        .get(3)
        .and_then(|l| counts(l))
        .ok_or("the molfile's counts line does not read")?;
    if atoms_n > MAX_ATOMS {
        return Err(format!("{atoms_n} atoms; at most {MAX_ATOMS}"));
    }
    let mut atoms = Vec::with_capacity(atoms_n);
    for i in 0..atoms_n {
        let line = lines
            .get(4 + i)
            .ok_or_else(|| format!("the atom block ends after {i} atoms"))?;
        let num = |a: usize, b: usize| -> Result<f64, String> {
            line.get(a..b)
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| format!("atom line {} does not read", i + 1))
        };
        let element = line
            .get(31..34)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("atom line {} has no symbol", i + 1))?;
        atoms.push(Atom3d {
            element: element.to_owned(),
            position: [num(0, 10)?, num(10, 20)?, num(20, 30)?],
            name: None,
            residue: None,
        });
    }
    let mut bonds = Vec::with_capacity(bonds_n);
    for i in 0..bonds_n {
        let line = lines
            .get(4 + atoms_n + i)
            .ok_or_else(|| format!("the bond block ends after {i} bonds"))?;
        let field = |a: usize, b: usize| -> Result<u32, String> {
            line.get(a..b)
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| format!("bond line {} does not read", i + 1))
        };
        let (a, b, t) = (field(0, 3)?, field(3, 6)?, field(6, 9)?);
        if a == 0 || b == 0 || a as usize > atoms_n || b as usize > atoms_n {
            return Err(format!(
                "bond line {} names an atom that is not there",
                i + 1
            ));
        }
        bonds.push(Bond3d {
            a: a - 1,
            b: b - 1,
            order: u8::try_from(t.clamp(1, 4)).unwrap_or(1),
        });
    }
    Ok(Molecule3dSpec {
        title: None,
        name: None,
        smiles: None,
        path: None,
        source: CoordinateSource::File { format },
        atoms,
        bonds,
        bonds_inferred: false,
        warnings: Vec::new(),
    })
}

fn xyz(text: &str) -> Result<Molecule3dSpec, String> {
    let (mol, coords) = threed::parse_xyz(text).map_err(|e| format!("invalid XYZ: {e}"))?;
    let name = text
        .lines()
        .nth(1)
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty());
    let atoms: Vec<(chematic::core::Element, Point3)> = mol
        .atoms()
        .zip(&coords.points)
        .map(|((_, a), p)| (a.element, *p))
        .collect();
    let bonded = threed::determine_bonds(&atoms, BOND_TOLERANCE).map_err(|e| e.to_string())?;
    let mut spec = spec_of(
        &bonded,
        &coords,
        CoordinateSource::File {
            format: StructureFormat::Xyz,
        },
    )?;
    spec.name = name;
    spec.bonds_inferred = true;
    Ok(spec)
}

fn pdb(text: &str) -> Result<Molecule3dSpec, String> {
    let atoms = threed::parse_pdb_atoms(text);
    if atoms.is_empty() {
        return Err("the PDB file has no ATOM or HETATM records".into());
    }
    let (mol, coords) = threed::pdb_to_molecule(&atoms);
    let mut spec = spec_of(
        &mol,
        &coords,
        CoordinateSource::File {
            format: StructureFormat::Pdb,
        },
    )?;
    for (a, p) in spec.atoms.iter_mut().zip(&atoms) {
        let name = p.name.trim();
        a.name = (!name.is_empty()).then(|| name.to_owned());
        a.residue = Some(
            format!("{} {} {}", p.res_name.trim(), p.res_seq, p.chain_id)
                .trim()
                .to_owned(),
        );
    }
    let record = |tag: &str| {
        text.lines()
            .find(|l| l.starts_with(tag))
            .map(|l| l.get(10..).unwrap_or("").trim().to_owned())
            .filter(|l| !l.is_empty())
    };
    spec.name = record("COMPND").or_else(|| record("HEADER"));
    spec.bonds_inferred = true;
    if atoms.len() >= 10_000 {
        spec.warnings
            .push("only the first 10 000 atoms of the PDB file are read".into());
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_from_names_and_content() {
        assert_eq!(format_of("a/b.SDF", ""), Some(StructureFormat::Sdf));
        assert_eq!(
            format_of("x", "3\nwater\nO 0 0 0\n"),
            Some(StructureFormat::Xyz)
        );
        assert_eq!(format_of("x", "HETATM    1  O"), Some(StructureFormat::Pdb));
        assert_eq!(format_of("x", "hello"), None);
        assert_eq!(
            counts("  3  2  0  0  0  0  0  0  0  0999 V2000"),
            Some((3, 2))
        );
    }
}
