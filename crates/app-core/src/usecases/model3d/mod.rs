//! 3D model files to a [`ModelMesh`]: OBJ ([`tobj`]), STL ([`stl_io`],
//! ASCII or binary) and PLY (in-house, ASCII or binary). Only geometry is
//! kept: materials, textures, normals and colours are dropped, and
//! polygons become triangles.

mod obj;
mod ply;
mod stl;

use domain::model3d::{MeshError, ModelMesh};

/// Extensions [`parse`] understands, lowercase.
pub const MODEL_EXTENSIONS: [&str; 3] = ["obj", "stl", "ply"];
/// Largest model file read.
pub const MAX_MODEL_BYTES: u64 = 64 * 1024 * 1024;
/// Most triangles a model may have: drawn flat shaded, a million take
/// 144 MB of vertices, within the GPU buffer limit (256 MB).
pub const MAX_MODEL_TRIANGLES: usize = 1_000_000;

/// Why a model file cannot be shown.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// Not one of [`MODEL_EXTENSIONS`].
    #[error("{0:?} is not a 3D model format (obj, stl, ply)")]
    Unsupported(String),
    /// The file does not follow its format.
    #[error("{0}")]
    Parse(String),
    /// More than [`MAX_MODEL_TRIANGLES`].
    #[error("{count} triangles is over the limit of {limit}")]
    TooManyTriangles {
        /// Triangles found (so far).
        count: usize,
        /// The limit.
        limit: usize,
    },
    /// The geometry is unusable.
    #[error(transparent)]
    Mesh(#[from] MeshError),
}

/// Parses `bytes` as the format of `ext` (lowercase, without the dot) and
/// validates the mesh.
///
/// # Errors
///
/// A [`ModelError`].
pub fn parse(ext: &str, bytes: &[u8]) -> Result<ModelMesh, ModelError> {
    let mesh = match ext {
        "obj" => obj::parse(bytes)?,
        "stl" => stl::parse(bytes)?,
        "ply" => ply::parse(bytes)?,
        other => return Err(ModelError::Unsupported(other.to_owned())),
    };
    mesh.validate()?;
    Ok(mesh)
}

/// Fails once `count` triangles are over [`MAX_MODEL_TRIANGLES`].
fn check_triangles(count: usize) -> Result<(), ModelError> {
    if count > MAX_MODEL_TRIANGLES {
        return Err(ModelError::TooManyTriangles {
            count,
            limit: MAX_MODEL_TRIANGLES,
        });
    }
    Ok(())
}

/// A vertex index as `u32`.
fn index(i: usize) -> Result<u32, ModelError> {
    u32::try_from(i).map_err(|_| ModelError::Parse(format!("vertex index {i} is too large")))
}

#[cfg(test)]
mod tests {
    use domain::model3d::UpAxis;

    use super::*;

    #[test]
    fn every_format_gives_a_valid_mesh() {
        let obj = parse("obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        assert_eq!((obj.triangles.len(), obj.up), (1, UpAxis::Y));
        let stl = parse(
            "stl",
            b"solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\n\
              vertex 0 1 0\nendloop\nendfacet\nendsolid t\n",
        )
        .unwrap();
        assert_eq!((stl.triangles.len(), stl.up), (1, UpAxis::Z));
        let ply = parse(
            "ply",
            b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
              property float z\nelement face 1\nproperty list uchar int vertex_indices\n\
              end_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 2\n",
        )
        .unwrap();
        assert_eq!((ply.triangles.len(), ply.up), (1, UpAxis::Z));
    }

    #[test]
    fn other_formats_and_empty_or_broken_meshes_are_refused() {
        assert_eq!(
            parse("fbx", b""),
            Err(ModelError::Unsupported("fbx".into()))
        );
        assert_eq!(
            parse("obj", b"v 0 0 0\n"),
            Err(ModelError::Mesh(MeshError::Empty))
        );
        assert!(matches!(
            parse("obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n"),
            Err(ModelError::Parse(_) | ModelError::Mesh(_))
        ));
    }

    #[test]
    fn the_triangle_limit_is_enforced() {
        assert_eq!(check_triangles(MAX_MODEL_TRIANGLES), Ok(()));
        assert_eq!(
            check_triangles(MAX_MODEL_TRIANGLES + 1),
            Err(ModelError::TooManyTriangles {
                count: MAX_MODEL_TRIANGLES + 1,
                limit: MAX_MODEL_TRIANGLES
            })
        );
    }
}
