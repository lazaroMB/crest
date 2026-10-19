//! STL, ASCII or binary, through [`stl_io`] (which merges equal vertices).

use std::io::Cursor;

use domain::model3d::{ModelMesh, UpAxis};

use super::{ModelError, check_triangles, index};

pub(super) fn parse(bytes: &[u8]) -> Result<ModelMesh, ModelError> {
    let stl = stl_io::read_stl(&mut Cursor::new(bytes))
        .map_err(|err| ModelError::Parse(format!("STL: {err}")))?;
    check_triangles(stl.faces.len())?;
    let triangles = stl
        .faces
        .iter()
        .map(|face| {
            let [a, b, c] = face.vertices;
            Ok([index(a)?, index(b)?, index(c)?])
        })
        .collect::<Result<_, ModelError>>()?;
    Ok(ModelMesh {
        positions: stl.vertices.iter().map(|v| v.0).collect(),
        triangles,
        up: UpAxis::Z,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A binary STL of `triangles`: 80-byte header, count, then per
    /// triangle a normal, three vertices and the attribute count.
    fn binary(triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut out = vec![0_u8; 80];
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for t in triangles {
            for _ in 0..3 {
                out.extend_from_slice(&0_f32.to_le_bytes());
            }
            for v in t {
                for c in v {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&0_u16.to_le_bytes());
        }
        out
    }

    #[test]
    fn binary_files_share_their_vertices() {
        let (o, x, y, z) = ([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let mesh = parse(&binary(&[[o, y, x], [o, x, z], [o, z, y], [x, y, z]])).unwrap();
        assert_eq!(mesh.triangles.len(), 4);
        assert_eq!(mesh.positions.len(), 4, "equal vertices are merged");
        assert_eq!(mesh.up, UpAxis::Z);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn truncated_files_are_parse_errors() {
        let mut bytes = binary(&[[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]]);
        bytes.truncate(100);
        assert!(matches!(parse(&bytes), Err(ModelError::Parse(_))));
        assert!(matches!(parse(b""), Err(ModelError::Parse(_))));
    }
}
