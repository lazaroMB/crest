//! A CAD part read from a STEP file (`show_step`): its solids tessellated
//! into one triangle mesh for the 3D view, with what the file says about
//! itself (schema, originating system, length unit, product names).
//!
//! Every solid keeps the triangles it owns (a contiguous range of
//! [`CadModelSpec::mesh`]), its face count, the faces the tessellator could
//! not mesh and its box, so the block can list, hide and size each one and
//! the model can be told which parts are incomplete.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;
use crate::model3d::{ModelMesh, ModelSpec};

/// Most solids a part may list.
pub const MAX_SOLIDS: usize = 10_000;

/// A solid (or an open shell) of the part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CadSolid {
    /// Its name in the file (`MANIFOLD_SOLID_BREP('name', …)`), or
    /// `solid N` when it has none.
    pub name: String,
    /// The product it belongs to, when the file links it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Faces of its boundary.
    pub faces: usize,
    /// Faces the tessellator could not mesh (left out of the view).
    #[serde(default)]
    pub failed_faces: usize,
    /// Index of its first triangle in the mesh.
    pub first_triangle: usize,
    /// Its triangles.
    pub triangles: usize,
    /// Corners of its box, in file units (all zero without triangles).
    pub min: [f32; 3],
    /// See [`Self::min`].
    pub max: [f32; 3],
}

impl CadSolid {
    /// Width, depth and height of its box.
    #[must_use]
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// The range of its triangles in the mesh.
    #[must_use]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.first_triangle..self.first_triangle + self.triangles
    }
}

/// A tessellated STEP part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CadModelSpec {
    /// Title shown above the view; defaults to the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The file, relative to the workspace.
    pub path: String,
    /// The schema of `FILE_SCHEMA` (`AUTOMOTIVE_DESIGN`,
    /// `CONFIG_CONTROL_DESIGN`, `AP242_…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The originating system of `FILE_NAME` (the CAD tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    /// The length unit of the coordinates (`mm`, `m`, `in`), when the file
    /// declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Names of the products in the file.
    #[serde(default)]
    pub products: Vec<String>,
    /// The solids, in the order of the mesh.
    pub solids: Vec<CadSolid>,
    /// Every solid's triangles, in file units, Z up.
    pub mesh: ModelMesh,
    /// Chord tolerance of the tessellation, in file units.
    pub tolerance: f64,
    /// Reader notes (shells that could not be converted, skipped
    /// placements).
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl CadModelSpec {
    /// The title, or the file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                self.path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&self.path)
                    .to_owned()
            })
    }

    /// The application protocol the schema names (`AP203`, `AP214`,
    /// `AP242`), if known.
    #[must_use]
    pub fn protocol(&self) -> Option<&'static str> {
        let schema = self.schema.as_deref()?.to_ascii_uppercase();
        if schema.starts_with("AP242") || schema.contains("MANAGED_MODEL_BASED_3D") {
            Some("AP242")
        } else if schema.starts_with("AUTOMOTIVE_DESIGN") {
            Some("AP214")
        } else if schema.starts_with("CONFIG_CONTROL_DESIGN") || schema.starts_with("AP203") {
            Some("AP203")
        } else {
            None
        }
    }

    /// Faces left out of the view, over every solid.
    #[must_use]
    pub fn failed_faces(&self) -> usize {
        self.solids.iter().map(|s| s.failed_faces).sum()
    }

    /// Faces over every solid.
    #[must_use]
    pub fn faces(&self) -> usize {
        self.solids.iter().map(|s| s.faces).sum()
    }

    /// The box of the whole part, in file units.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.mesh.bounds()
    }

    /// The unit for readouts: the file's, or `units` when it has none.
    #[must_use]
    pub fn unit_label(&self) -> &str {
        self.unit.as_deref().unwrap_or("units")
    }

    /// The model the 3D view draws.
    #[must_use]
    pub fn model(&self) -> ModelSpec {
        ModelSpec {
            name: self.display_title(),
            mesh: self.mesh.clone(),
            unit: self.unit.clone(),
        }
    }

    /// The solids as CSV: name, product, faces, failed faces, triangles
    /// and box size.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out =
            String::from("solid,product,faces,failed_faces,triangles,size_x,size_y,size_z\n");
        for s in &self.solids {
            let [x, y, z] = s.size();
            out.push_str(&format!(
                "{},{},{},{},{},{x},{y},{z}\n",
                csv_field(&s.name),
                csv_field(s.product.as_deref().unwrap_or("")),
                s.faces,
                s.failed_faces,
                s.triangles
            ));
        }
        out
    }

    /// Checks the mesh, that every solid's triangles are in it and that
    /// the boxes are finite.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        const KIND: &str = "cad_model";
        if self.path.trim().is_empty() {
            return Err(invalid(KIND, "the path is empty"));
        }
        if self.solids.is_empty() {
            return Err(invalid(KIND, "the part has no solids"));
        }
        if self.solids.len() > MAX_SOLIDS {
            return Err(invalid(KIND, format!("more than {MAX_SOLIDS} solids")));
        }
        self.mesh
            .validate()
            .map_err(|e| invalid(KIND, e.to_string()))?;
        let total = self.mesh.triangles.len();
        for s in &self.solids {
            if s.range().end > total {
                return Err(invalid(
                    KIND,
                    format!("solid `{}` names triangles past the mesh", s.name),
                ));
            }
            if s.failed_faces > s.faces {
                return Err(invalid(
                    KIND,
                    format!("solid `{}` has more failed faces than faces", s.name),
                ));
            }
            if s.min.iter().chain(&s.max).any(|c| !c.is_finite()) {
                return Err(invalid(
                    KIND,
                    format!("solid `{}` has a non-finite box", s.name),
                ));
            }
        }
        if !self.tolerance.is_finite() || self.tolerance <= 0.0 {
            return Err(invalid(KIND, "the tolerance must be positive"));
        }
        Ok(())
    }
}

/// A CSV field, quoted when it holds a comma, quote or line break.
pub(crate) fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model3d::UpAxis;

    fn spec() -> CadModelSpec {
        CadModelSpec {
            title: None,
            path: "parts/bracket.step".into(),
            schema: Some("AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }".into()),
            application: Some("FreeCAD".into()),
            unit: Some("mm".into()),
            products: vec!["Bracket".into()],
            solids: vec![
                CadSolid {
                    name: "Plate".into(),
                    product: Some("Bracket".into()),
                    faces: 6,
                    failed_faces: 0,
                    first_triangle: 0,
                    triangles: 1,
                    min: [0.0; 3],
                    max: [10.0, 10.0, 0.0],
                },
                CadSolid {
                    name: "Boss, M4".into(),
                    product: None,
                    faces: 3,
                    failed_faces: 1,
                    first_triangle: 1,
                    triangles: 1,
                    min: [0.0; 3],
                    max: [1.0, 1.0, 5.0],
                },
            ],
            mesh: ModelMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [0.0, 0.0, 5.0],
                ],
                triangles: vec![[0, 1, 2], [0, 1, 3]],
                up: UpAxis::Z,
            },
            tolerance: 0.05,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_part_is_valid_and_summarised() {
        let spec = spec();
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.protocol(), Some("AP214"));
        assert_eq!(spec.display_title(), "bracket.step");
        assert_eq!((spec.faces(), spec.failed_faces()), (9, 1));
        assert_eq!(spec.solids[1].range(), 1..2);
        assert_eq!(spec.model().unit.as_deref(), Some("mm"));
        assert_eq!(
            spec.to_csv(),
            "solid,product,faces,failed_faces,triangles,size_x,size_y,size_z\n\
             Plate,Bracket,6,0,1,10,10,0\n\"Boss, M4\",,3,1,1,1,1,5\n"
        );
    }

    #[test]
    fn broken_parts_are_refused() {
        let mut past = spec();
        past.solids[1].triangles = 5;
        assert!(past.validate().is_err());
        let mut failed = spec();
        failed.solids[0].failed_faces = 7;
        assert!(failed.validate().is_err());
        let mut empty = spec();
        empty.solids.clear();
        assert!(empty.validate().is_err());
        let mut tolerance = spec();
        tolerance.tolerance = 0.0;
        assert!(tolerance.validate().is_err());
    }

    #[test]
    fn protocols_follow_the_schema() {
        let mut spec = spec();
        spec.schema = Some("CONFIG_CONTROL_DESIGN".into());
        assert_eq!(spec.protocol(), Some("AP203"));
        spec.schema = Some("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF".into());
        assert_eq!(spec.protocol(), Some("AP242"));
        spec.schema = None;
        assert_eq!(spec.protocol(), None);
    }
}
