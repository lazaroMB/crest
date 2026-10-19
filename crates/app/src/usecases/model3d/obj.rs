//! Wavefront OBJ, through [`tobj`]. Every object and group goes into one
//! mesh; `mtllib` files are not read.

use std::io::Cursor;

use domain::model3d::{ModelMesh, UpAxis};

use super::{ModelError, check_triangles, index};

pub(super) fn parse(bytes: &[u8]) -> Result<ModelMesh, ModelError> {
    let options = tobj::LoadOptions {
        triangulate: true,
        ignore_points: true,
        ignore_lines: true,
        ..tobj::LoadOptions::default()
    };
    let (models, _materials) = tobj::load_obj_buf(&mut Cursor::new(bytes), &options, |_| {
        Err(tobj::LoadError::OpenFileFailed)
    })
    .map_err(|err| ModelError::Parse(format!("OBJ: {err}")))?;
    let triangles: usize = models.iter().map(|m| m.mesh.indices.len() / 3).sum();
    check_triangles(triangles)?;
    let mut mesh = ModelMesh {
        up: UpAxis::Y,
        ..ModelMesh::default()
    };
    for model in models {
        let offset = mesh.positions.len();
        mesh.positions
            .extend(model.mesh.positions.as_chunks::<3>().0.iter().copied());
        for t in model.mesh.indices.as_chunks::<3>().0 {
            mesh.triangles.push([
                index(offset + t[0] as usize)?,
                index(offset + t[1] as usize)?,
                index(offset + t[2] as usize)?,
            ]);
        }
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_are_triangulated_and_negative_indices_resolved() {
        let mesh = parse(
            b"# a square\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 1\n\
              f -4//1 -3//1 -2//1 -1//1\n",
        )
        .unwrap();
        assert_eq!(mesh.positions.len(), 4);
        assert_eq!(mesh.triangles.len(), 2);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn objects_are_merged_with_their_offsets() {
        let mesh = parse(
            b"o a\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n\
              o b\nv 0 0 1\nv 1 0 1\nv 0 1 1\nf 4 5 6\n",
        )
        .unwrap();
        assert_eq!(mesh.positions.len(), 6);
        assert_eq!(mesh.triangles.len(), 2);
        assert_eq!(mesh.validate(), Ok(()));
        let used: Vec<u32> = mesh.triangles.iter().flatten().copied().collect();
        assert!(used.contains(&5), "{used:?}");
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(matches!(
            parse(b"v 0 zero 0\nf 1 2 3\n"),
            Err(ModelError::Parse(_))
        ));
    }
}
