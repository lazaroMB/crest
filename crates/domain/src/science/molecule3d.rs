//! A molecule in 3D as ball and stick (`show_molecule_3d`).
//!
//! The atoms come with coordinates from a structure file (MOL/SDF, XYZ,
//! PDB) or from the chemistry engine's embedding of a SMILES (distance
//! geometry then a force-field minimisation); nothing here makes up a
//! geometry. [`Molecule3dSpec::ball_and_stick`] turns them into a mesh
//! for the 3D view: a sphere per atom in its CPK colour and a cylinder
//! per bond, each half in its atom's colour.

use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;
use crate::model3d::{ModelMesh, ModelSpec, UpAxis};

/// Most atoms drawn.
pub const MAX_ATOMS: usize = 20_000;
/// Radius of a bond cylinder, Å.
pub const BOND_RADIUS: f64 = 0.12;

/// The file the coordinates come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StructureFormat {
    /// MDL molfile (V2000 or V3000).
    Mol,
    /// SD file (its first record).
    Sdf,
    /// XYZ.
    Xyz,
    /// Protein Data Bank.
    Pdb,
}

/// Where the coordinates come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoordinateSource {
    /// Read from a structure file.
    File {
        /// Its format.
        format: StructureFormat,
    },
    /// Embedded from SMILES by the engine (a computed conformer, not an
    /// experimental structure).
    Embedded {
        /// The method, e.g. `distance geometry + DREIDING (chematic 1.2.1)`.
        method: String,
    },
}

/// An atom with its position in Å.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Atom3d {
    /// Element symbol (`C`, `Cl`).
    pub element: String,
    /// Position, Å.
    pub position: [f64; 3],
    /// Atom name from the file (PDB `CA`), when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Residue (`ALA 12 A`), for PDB atoms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residue: Option<String>,
}

/// A bond between two atoms (indices into the atoms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Bond3d {
    /// First atom.
    pub a: u32,
    /// Second atom.
    pub b: u32,
    /// 1, 2 or 3; 4 for aromatic.
    pub order: u8,
}

/// A molecule in 3D.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Molecule3dSpec {
    /// Title shown above the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Name of the molecule (the file's title line, or the call's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The SMILES embedded, when it came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smiles: Option<String>,
    /// The workspace file read, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Where the coordinates come from.
    pub source: CoordinateSource,
    /// The atoms.
    pub atoms: Vec<Atom3d>,
    /// The bonds.
    #[serde(default)]
    pub bonds: Vec<Bond3d>,
    /// Whether the bonds were inferred from distances (XYZ, PDB).
    #[serde(default)]
    pub bonds_inferred: bool,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The CPK colour of `element` (Jmol's palette), as sRGB.
#[must_use]
pub fn cpk_rgb(element: &str) -> [u8; 3] {
    match element {
        "H" => [0xFF, 0xFF, 0xFF],
        "C" => [0x90, 0x90, 0x90],
        "N" => [0x30, 0x50, 0xF8],
        "O" => [0xFF, 0x0D, 0x0D],
        "F" => [0x90, 0xE0, 0x50],
        "Cl" => [0x1F, 0xF0, 0x1F],
        "Br" => [0xA6, 0x29, 0x29],
        "I" => [0x94, 0x00, 0x94],
        "S" => [0xFF, 0xFF, 0x30],
        "P" => [0xFF, 0x80, 0x00],
        "B" => [0xFF, 0xB5, 0xB5],
        "Si" => [0xF0, 0xC8, 0xA0],
        "Li" => [0xCC, 0x80, 0xFF],
        "Na" => [0xAB, 0x5C, 0xF2],
        "K" => [0x8F, 0x40, 0xD4],
        "Mg" => [0x8A, 0xFF, 0x00],
        "Ca" => [0x3D, 0xFF, 0x00],
        "Fe" => [0xE0, 0x66, 0x33],
        "Cu" => [0xC8, 0x80, 0x33],
        "Zn" => [0x7D, 0x80, 0xB0],
        "Se" => [0xFF, 0xA1, 0x00],
        _ => [0xFF, 0x14, 0x93],
    }
}

/// The ball radius drawn for `element`, Å (a fraction of its covalent
/// radius, so bonds show).
#[must_use]
pub fn ball_radius(element: &str) -> f64 {
    match element {
        "H" => 0.22,
        "C" | "N" | "O" | "F" => 0.34,
        "B" | "Si" | "P" | "S" | "Cl" => 0.45,
        "Br" | "Se" => 0.5,
        "I" => 0.56,
        _ => 0.5,
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Mesh being built: positions, triangles and a colour per triangle.
struct Builder {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    colors: Vec<[u8; 3]>,
}

impl Builder {
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        self.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        u32::try_from(self.positions.len() - 1).unwrap_or(u32::MAX)
    }

    fn triangle(&mut self, t: [u32; 3], color: [u8; 3]) {
        self.triangles.push(t);
        self.colors.push(color);
    }

    /// A UV sphere of `radius` at `c`, wound outwards.
    fn sphere(&mut self, c: [f64; 3], radius: f64, color: [u8; 3], (lon, lat): (u32, u32)) {
        let top = self.vertex(add(c, [0.0, 0.0, radius]));
        let mut rings = Vec::new();
        for i in 1..lat {
            let theta = PI * f64::from(i) / f64::from(lat);
            let ring: Vec<u32> = (0..lon)
                .map(|j| {
                    let phi = TAU * f64::from(j) / f64::from(lon);
                    self.vertex(add(
                        c,
                        [
                            radius * theta.sin() * phi.cos(),
                            radius * theta.sin() * phi.sin(),
                            radius * theta.cos(),
                        ],
                    ))
                })
                .collect();
            rings.push(ring);
        }
        let bottom = self.vertex(add(c, [0.0, 0.0, -radius]));
        let n = lon as usize;
        if let Some(first) = rings.first() {
            for j in 0..n {
                let t = [top, first[j], first[(j + 1) % n]];
                self.triangle(t, color);
            }
        }
        for pair in rings.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            for j in 0..n {
                let k = (j + 1) % n;
                self.triangle([a[j], b[j], b[k]], color);
                self.triangle([a[j], b[k], a[k]], color);
            }
        }
        if let Some(last) = rings.last() {
            for j in 0..n {
                self.triangle([bottom, last[(j + 1) % n], last[j]], color);
            }
        }
    }

    /// An open cylinder of `radius` from `a` to `b`.
    fn cylinder(&mut self, a: [f64; 3], b: [f64; 3], radius: f64, color: [u8; 3], sides: u32) {
        let axis = sub(b, a);
        let len = norm(axis);
        if len <= 1e-9 {
            return;
        }
        let dir = scale(axis, 1.0 / len);
        // Any vector not along the axis gives the cross-section's frame.
        let helper = if dir[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let u = cross(dir, helper);
        let u = scale(u, 1.0 / norm(u));
        let v = cross(dir, u);
        let ring = |this: &mut Self, c: [f64; 3]| -> Vec<u32> {
            (0..sides)
                .map(|j| {
                    let phi = TAU * f64::from(j) / f64::from(sides);
                    let off = add(scale(u, radius * phi.cos()), scale(v, radius * phi.sin()));
                    this.vertex(add(c, off))
                })
                .collect()
        };
        let r0 = ring(self, a);
        let r1 = ring(self, b);
        let n = sides as usize;
        for j in 0..n {
            let k = (j + 1) % n;
            self.triangle([r0[j], r0[k], r1[k]], color);
            self.triangle([r0[j], r1[k], r1[j]], color);
        }
    }
}

impl Molecule3dSpec {
    /// Checks the atoms and bonds.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "molecule_3d",
            reason,
        };
        if self.atoms.is_empty() {
            return Err(invalid("no atoms".into()));
        }
        if self.atoms.len() > MAX_ATOMS {
            return Err(invalid(format!(
                "{} atoms; at most {MAX_ATOMS}",
                self.atoms.len()
            )));
        }
        if self
            .atoms
            .iter()
            .any(|a| a.position.iter().any(|v| !v.is_finite()))
        {
            return Err(invalid("an atom position is not finite".into()));
        }
        let n = self.atoms.len();
        if let Some(b) = self
            .bonds
            .iter()
            .find(|b| b.a as usize >= n || b.b as usize >= n || b.a == b.b)
        {
            return Err(invalid(format!("bond {}–{} is not between two atoms", b.a, b.b)));
        }
        Ok(())
    }

    /// The title, else the name, else the SMILES or file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.name.clone())
            .or_else(|| self.smiles.clone())
            .or_else(|| {
                self.path
                    .as_deref()
                    .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
            })
            .unwrap_or_else(|| "molecule".into())
    }

    /// Atom counts by element in Hill order (C, H, then alphabetical; all
    /// alphabetical without carbon), as a formula.
    #[must_use]
    pub fn formula(&self) -> String {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &self.atoms {
            *counts.entry(a.element.as_str()).or_default() += 1;
        }
        let mut out = String::new();
        let mut push = |e: &str, n: usize| {
            out.push_str(e);
            if n > 1 {
                out.push_str(&n.to_string());
            }
        };
        if let Some(c) = counts.remove("C") {
            push("C", c);
            if let Some(h) = counts.remove("H") {
                push("H", h);
            }
        }
        for (e, n) in counts {
            push(e, n);
        }
        out
    }

    /// The box of the atoms (min, max), Å.
    #[must_use]
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = self.atoms.first()?.position;
        Some(self.atoms.iter().fold((first, first), |(lo, hi), a| {
            let p = a.position;
            (
                [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
            )
        }))
    }

    /// Distance between atoms `a` and `b`, Å.
    #[must_use]
    pub fn distance(&self, a: usize, b: usize) -> Option<f64> {
        Some(norm(sub(self.atoms.get(a)?.position, self.atoms.get(b)?.position)))
    }

    /// The label of atom `i`: element and 1-based index, with its name and
    /// residue when the file gives them (`C7`, `CA 12 (ALA 3 A)`).
    #[must_use]
    pub fn atom_label(&self, i: usize) -> String {
        let Some(a) = self.atoms.get(i) else {
            return format!("#{}", i + 1);
        };
        let mut label = format!("{}{}", a.element, i + 1);
        if let Some(name) = &a.name
            && name != &a.element
        {
            label = format!("{label} {name}");
        }
        if let Some(res) = &a.residue {
            label = format!("{label} ({res})");
        }
        label
    }

    /// The ball-and-stick mesh (Z up, Å) and a CPK colour per triangle.
    #[must_use]
    pub fn ball_and_stick(&self) -> (ModelSpec, Vec<[u8; 3]>) {
        let detail = if self.atoms.len() > 2_000 {
            (8, 5)
        } else if self.atoms.len() > 300 {
            (10, 6)
        } else {
            (16, 10)
        };
        let sides = if self.atoms.len() > 2_000 { 5 } else { 8 };
        let mut b = Builder {
            positions: Vec::new(),
            triangles: Vec::new(),
            colors: Vec::new(),
        };
        for a in &self.atoms {
            b.sphere(a.position, ball_radius(&a.element), cpk_rgb(&a.element), detail);
        }
        for bond in &self.bonds {
            let (Some(pa), Some(pb)) = (
                self.atoms.get(bond.a as usize),
                self.atoms.get(bond.b as usize),
            ) else {
                continue;
            };
            let mid = scale(add(pa.position, pb.position), 0.5);
            let radius = if bond.order >= 2 && bond.order != 4 {
                BOND_RADIUS * 1.4
            } else {
                BOND_RADIUS
            };
            b.cylinder(pa.position, mid, radius, cpk_rgb(&pa.element), sides);
            b.cylinder(mid, pb.position, radius, cpk_rgb(&pb.element), sides);
        }
        let model = ModelSpec {
            name: self.display_title(),
            mesh: ModelMesh {
                positions: b.positions,
                triangles: b.triangles,
                up: UpAxis::Z,
            },
            unit: Some("Å".into()),
        };
        (model, b.colors)
    }

    /// The atoms as an XYZ file.
    #[must_use]
    pub fn to_xyz(&self) -> String {
        let mut out = format!("{}\n{}\n", self.atoms.len(), self.display_title());
        for a in &self.atoms {
            out.push_str(&format!(
                "{:<2} {:>12.6} {:>12.6} {:>12.6}\n",
                a.element, a.position[0], a.position[1], a.position[2]
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water() -> Molecule3dSpec {
        Molecule3dSpec {
            title: None,
            name: Some("water".into()),
            smiles: None,
            path: Some("mol/water.xyz".into()),
            source: CoordinateSource::File {
                format: StructureFormat::Xyz,
            },
            atoms: vec![
                Atom3d {
                    element: "O".into(),
                    position: [0.0, 0.0, 0.0],
                    name: None,
                    residue: None,
                },
                Atom3d {
                    element: "H".into(),
                    position: [0.9572, 0.0, 0.0],
                    name: None,
                    residue: None,
                },
                Atom3d {
                    element: "H".into(),
                    position: [-0.24, 0.9266, 0.0],
                    name: None,
                    residue: None,
                },
            ],
            bonds: vec![Bond3d { a: 0, b: 1, order: 1 }, Bond3d { a: 0, b: 2, order: 1 }],
            bonds_inferred: true,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn mesh_formula_and_distances() {
        let w = water();
        w.validate().unwrap();
        assert_eq!(w.formula(), "H2O");
        assert!((w.distance(0, 1).unwrap() - 0.9572).abs() < 1e-12);
        assert_eq!(w.atom_label(2), "H3");
        let (model, colors) = w.ball_and_stick();
        model.mesh.validate().unwrap();
        assert_eq!(model.mesh.triangles.len(), colors.len());
        assert_eq!(model.unit.as_deref(), Some("Å"));
        // Oxygen's sphere comes first, in red; hydrogens are white.
        assert_eq!(colors[0], cpk_rgb("O"));
        assert!(colors.contains(&cpk_rgb("H")));
        let mut bad = water();
        bad.bonds.push(Bond3d { a: 0, b: 9, order: 1 });
        assert!(bad.validate().is_err());
        assert!(w.to_xyz().starts_with("3\nwater\nO "));
    }
}
