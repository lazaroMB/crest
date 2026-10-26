//! Section planes and picking on a model's triangles, on the CPU: the
//! model viewer cuts its mesh with an axis-aligned plane before it is
//! drawn (so the GPU and the vector fallback show the same cut) and finds
//! the surface point under the cursor for measuring.
//!
//! A [`Section`] keeps the side of the plane where the chosen coordinate
//! is at most the offset (or at least it, flipped); triangles that cross
//! it are cut along it. [`ray_triangle`] is Möller–Trumbore, two-sided;
//! [`pick`] is the nearest hit of a ray on a built [`Mesh`].

use super::math::{self, V3};
use super::mesh::{FLOATS_PER_VERTEX, Mesh};

/// Below this a ray is taken as parallel to a triangle.
const PARALLEL: f32 = 1e-9;

/// An axis-aligned clip plane in file coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// The coordinate cut: 0 = x, 1 = y, 2 = z.
    pub axis: usize,
    /// Where the plane is along that axis, in file units.
    pub offset: f32,
    /// Keep the far side (coordinate ≥ offset) instead of the near one.
    pub flip: bool,
}

impl Section {
    /// Signed distance of `p` past the plane: positive on the side that is
    /// cut away.
    #[must_use]
    pub fn signed(&self, p: V3) -> f32 {
        let d = p[self.axis.min(2)] - self.offset;
        if self.flip { -d } else { d }
    }

    /// Whether `p` is on the kept side (the plane itself is kept).
    #[must_use]
    pub fn keeps(&self, p: V3) -> bool {
        self.signed(p) <= 0.0
    }

    /// The axis' letter.
    #[must_use]
    pub fn axis_name(&self) -> &'static str {
        ["x", "y", "z"][self.axis.min(2)]
    }
}

/// The part of triangle `t` on the kept side of `section`, as a convex
/// polygon in the same winding: empty (also when it only touches the
/// plane), the triangle itself, a smaller triangle or a quad
/// (Sutherland–Hodgman against one plane).
#[must_use]
pub fn clip_triangle(t: [V3; 3], section: &Section) -> Vec<V3> {
    let d = t.map(|p| section.signed(p));
    if d.iter().all(|&x| x <= 0.0) {
        return t.to_vec();
    }
    if d.iter().all(|&x| x >= 0.0) {
        // On the plane at most along an edge: nothing of area is kept.
        return Vec::new();
    }
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (p, q) = (t[i], t[j]);
        let (dp, dq) = (d[i], d[j]);
        if dp <= 0.0 {
            out.push(p);
        }
        // Only a strict crossing adds a point: a vertex on the plane is
        // already in the polygon.
        if (dp < 0.0 && dq > 0.0) || (dp > 0.0 && dq < 0.0) {
            let s = dp / (dp - dq);
            out.push(math::add(p, math::scale(math::sub(q, p), s)));
        }
    }
    out
}

/// Distance along the ray `origin + s·dir` to triangle `[a, b, c]`, if it
/// is hit in front of the origin (either face).
#[must_use]
pub fn ray_triangle(origin: V3, dir: V3, [a, b, c]: [V3; 3]) -> Option<f32> {
    let e1 = math::sub(b, a);
    let e2 = math::sub(c, a);
    let p = math::cross(dir, e2);
    let det = math::dot(e1, p);
    if det.abs() < PARALLEL {
        return None;
    }
    let inv = 1.0 / det;
    let t = math::sub(origin, a);
    let u = math::dot(t, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = math::cross(t, e1);
    let v = math::dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let s = math::dot(e2, q) * inv;
    (s >= 0.0).then_some(s)
}

/// The nearest point of `mesh` the ray hits, in the mesh's coordinates.
#[must_use]
pub fn pick(mesh: &Mesh, origin: V3, dir: V3) -> Option<V3> {
    let vertex = |i: u32| -> Option<V3> {
        let at = i as usize * FLOATS_PER_VERTEX;
        let v = mesh.vertices.get(at..at + 3)?;
        Some([v[0], v[1], v[2]])
    };
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|t| {
            let tri = [vertex(t[0])?, vertex(t[1])?, vertex(t[2])?];
            ray_triangle(origin, dir, tri)
        })
        .min_by(f32::total_cmp)
        .map(|s| math::add(origin, math::scale(dir, s)))
}

/// Distance between two points.
#[must_use]
pub fn distance(a: V3, b: V3) -> f32 {
    math::length(math::sub(b, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRI: [V3; 3] = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];

    fn area(poly: &[V3]) -> f32 {
        // Shoelace in the xy plane, for flat test polygons.
        let n = poly.len();
        (0..n)
            .map(|i| {
                let (p, q) = (poly[i], poly[(i + 1) % n]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f32>()
            / 2.0
    }

    #[test]
    fn a_plane_keeps_cuts_or_drops_a_triangle() {
        let far = Section {
            axis: 0,
            offset: 5.0,
            flip: false,
        };
        assert_eq!(clip_triangle(TRI, &far), TRI.to_vec());
        let flipped = Section { flip: true, ..far };
        assert!(clip_triangle(TRI, &flipped).is_empty());

        // x ≤ 1 keeps a quad of area 1.5 out of the triangle's 2.
        let half = Section {
            axis: 0,
            offset: 1.0,
            flip: false,
        };
        let quad = clip_triangle(TRI, &half);
        assert_eq!(quad.len(), 4);
        assert!((area(&quad) - 1.5).abs() < 1e-6, "{quad:?}");
        assert!(quad.iter().all(|p| p[0] <= 1.0 + 1e-6));
        // x ≥ 1 keeps the tip: a triangle of area 0.5, same winding.
        let tip = clip_triangle(TRI, &Section { flip: true, ..half });
        assert_eq!(tip.len(), 3);
        assert!((area(&tip) - 0.5).abs() < 1e-6, "{tip:?}");
    }

    #[test]
    fn a_vertex_on_the_plane_is_kept_and_a_touching_edge_is_not() {
        let through_tip = Section {
            axis: 1,
            offset: 2.0,
            flip: false,
        };
        assert_eq!(clip_triangle(TRI, &through_tip), TRI.to_vec());
        let along_base = Section {
            offset: 0.0,
            ..through_tip
        };
        assert!(clip_triangle(TRI, &along_base).is_empty());
        // y ≥ 0 keeps it whole (the base lies on the plane).
        assert_eq!(
            clip_triangle(
                TRI,
                &Section {
                    flip: true,
                    ..along_base
                }
            ),
            TRI.to_vec()
        );
        assert!(along_base.keeps([9.0, 0.0, 9.0]));
        assert_eq!(along_base.axis_name(), "y");
    }

    #[test]
    fn rays_hit_triangles_from_either_side_and_miss_outside() {
        let down = [0.0, 0.0, -1.0];
        assert_eq!(ray_triangle([0.5, 0.5, 3.0], down, TRI), Some(3.0));
        assert_eq!(
            ray_triangle([0.5, 0.5, -3.0], [0.0, 0.0, 1.0], TRI),
            Some(3.0)
        );
        assert_eq!(
            ray_triangle([1.5, 1.5, 3.0], down, TRI),
            None,
            "past the hypotenuse"
        );
        assert_eq!(
            ray_triangle([0.5, 0.5, -3.0], down, TRI),
            None,
            "behind the origin"
        );
        assert_eq!(
            ray_triangle([0.5, 0.5, 3.0], [1.0, 0.0, 0.0], TRI),
            None,
            "parallel"
        );
    }

    #[test]
    fn picking_finds_the_nearest_surface() {
        let mut mesh = Mesh::default();
        // Two parallel squares at z = 0 and z = 1, seen from above.
        for z in [0.0, 1.0] {
            for t in [
                [[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z]],
                [[0.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
            ] {
                let base = u32::try_from(mesh.vertex_count()).unwrap();
                for p in t {
                    mesh.vertices.extend_from_slice(&p);
                    mesh.vertices
                        .extend_from_slice(&[0.0; FLOATS_PER_VERTEX - 3]);
                }
                mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
        }
        let hit = pick(&mesh, [0.25, 0.75, 5.0], [0.0, 0.0, -1.0]).unwrap();
        assert!((hit[2] - 1.0).abs() < 1e-6, "{hit:?}");
        assert_eq!(pick(&mesh, [3.0, 3.0, 5.0], [0.0, 0.0, -1.0]), None);
        assert!((distance([0.0; 3], [3.0, 4.0, 0.0]) - 5.0).abs() < 1e-6);
    }
}
