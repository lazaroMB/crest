//! A 3D model opened from a file (OBJ, STL, PLY, or a tessellated STEP
//! part): a triangle mesh with no materials, which the UI draws with an
//! orbit camera.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The axis that points up in the file's coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    /// `+Y` (OBJ, most real-time tools).
    #[default]
    Y,
    /// `+Z` (STL, PLY, most CAD tools).
    Z,
}

impl UpAxis {
    /// The other axis.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Y => Self::Z,
            Self::Z => Self::Y,
        }
    }
}

/// A triangle mesh.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelMesh {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Triangles, as indices into `positions`.
    pub triangles: Vec<[u32; 3]>,
    /// The axis that points up.
    #[serde(default)]
    pub up: UpAxis,
}

/// Why a [`ModelMesh`] cannot be drawn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshError {
    /// No triangles.
    #[error("the model has no triangles")]
    Empty,
    /// A NaN or infinite coordinate.
    #[error("vertex {index} has a non-finite coordinate")]
    NonFinite {
        /// Zero-based vertex index.
        index: usize,
    },
    /// A triangle that names a vertex that does not exist.
    #[error("triangle {triangle} uses vertex {vertex}, but there are {count} vertices")]
    IndexOutOfRange {
        /// Zero-based triangle index.
        triangle: usize,
        /// The vertex index it uses.
        vertex: u32,
        /// Number of vertices.
        count: usize,
    },
}

impl ModelMesh {
    /// Checks that the mesh has triangles, finite coordinates and indices
    /// in range.
    ///
    /// # Errors
    ///
    /// The first [`MeshError`] found.
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.triangles.is_empty() {
            return Err(MeshError::Empty);
        }
        if let Some(index) = self
            .positions
            .iter()
            .position(|p| p.iter().any(|c| !c.is_finite()))
        {
            return Err(MeshError::NonFinite { index });
        }
        let count = self.positions.len();
        for (triangle, indices) in self.triangles.iter().enumerate() {
            if let Some(&vertex) = indices.iter().find(|&&i| i as usize >= count) {
                return Err(MeshError::IndexOutOfRange {
                    triangle,
                    vertex,
                    count,
                });
            }
        }
        Ok(())
    }

    /// The corners of the box around the vertices the triangles use, in
    /// file coordinates; `None` without triangles.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut used = self
            .triangles
            .iter()
            .flatten()
            .filter_map(|&i| self.positions.get(i as usize));
        let first = *used.next()?;
        Some(used.fold((first, first), |(mut min, mut max), p| {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
            (min, max)
        }))
    }
}

/// A model shown in a modal: the mesh and the file it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSpec {
    /// File name or title.
    pub name: String,
    /// The mesh.
    pub mesh: ModelMesh,
    /// The length unit of the coordinates, when the file says it (`mm`,
    /// `in`; STEP does, OBJ/STL/PLY do not). Measurements show it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra() -> ModelMesh {
        ModelMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [0.0, 3.0, 0.0],
                [0.0, 0.0, -1.0],
            ],
            triangles: vec![[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]],
            up: UpAxis::Z,
        }
    }

    #[test]
    fn a_closed_mesh_is_valid_and_bounded() {
        let mesh = tetra();
        assert_eq!(mesh.validate(), Ok(()));
        assert_eq!(mesh.bounds(), Some(([0.0, 0.0, -1.0], [2.0, 3.0, 0.0])));
    }

    #[test]
    fn broken_meshes_are_explained() {
        assert_eq!(ModelMesh::default().validate(), Err(MeshError::Empty));
        assert_eq!(ModelMesh::default().bounds(), None);
        let mut nan = tetra();
        nan.positions[2][1] = f32::NAN;
        assert_eq!(nan.validate(), Err(MeshError::NonFinite { index: 2 }));
        let mut out = tetra();
        out.triangles[3][2] = 9;
        assert_eq!(
            out.validate(),
            Err(MeshError::IndexOutOfRange {
                triangle: 3,
                vertex: 9,
                count: 4
            })
        );
    }

    #[test]
    fn the_up_axis_toggles() {
        assert_eq!(UpAxis::Y.toggled(), UpAxis::Z);
        assert_eq!(UpAxis::Z.toggled(), UpAxis::Y);
    }
}
