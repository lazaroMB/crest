//! The fallback of the model viewer for machines without a GPU adapter:
//! the triangles projected with the same [`OrbitCamera`], flat shaded with
//! the renderer's key light and painted back to front as GPUI paths
//! (painter's order by the distance of each centroid to the eye). Above
//! [`VECTOR_MAX_TRIANGLES`] only an even sample of the triangles is drawn.

use gpui::{Bounds, ContentMask, Hsla, PathBuilder, Pixels, Window, point, px};

use crate::render3d::camera::OrbitCamera;
use crate::render3d::math::{self, V3};
use crate::render3d::mesh::{FLOATS_PER_VERTEX, Mesh};

/// Most triangles painted per frame.
pub const VECTOR_MAX_TRIANGLES: usize = 20_000;
/// Direction towards the key light, as the GPU renderer's.
const LIGHT: V3 = [0.45, 1.0, 0.65];
/// The TwoCrest ink behind the model, linear (as the GPU's clear colour).
const BACKGROUND: [f32; 3] = [0.0033, 0.0033, 0.0037];

/// Every how many triangles one is drawn, so at most
/// [`VECTOR_MAX_TRIANGLES`] are.
#[must_use]
pub fn stride(triangles: usize) -> usize {
    triangles.div_ceil(VECTOR_MAX_TRIANGLES).max(1)
}

/// How many of `triangles` are drawn.
#[must_use]
pub fn drawn(triangles: usize) -> usize {
    triangles.div_ceil(stride(triangles))
}

/// A triangle ready to paint: screen corners, depth and colour.
struct Flat {
    corners: [(f32, f32); 3],
    depth: f32,
    color: Hsla,
}

/// The triangles of `mesh` seen through `camera` in a viewport of `size`,
/// farthest first.
fn flats(mesh: &Mesh, camera: &OrbitCamera, size: (f32, f32)) -> Vec<Flat> {
    let view_proj = camera.view_projection(size.0 / size.1);
    let eye = camera.eye();
    let light = math::normalize(LIGHT);
    let vertex = |i: u32| {
        let at = i as usize * FLOATS_PER_VERTEX;
        mesh.vertices.get(at..at + 9)
    };
    let project = |p: V3| {
        let clip = math::transform(&view_proj, p)?;
        (0.0..=1.0).contains(&clip[2]).then(|| {
            (
                (clip[0] + 1.0) / 2.0 * size.0,
                (1.0 - clip[1]) / 2.0 * size.1,
            )
        })
    };
    let triangles = mesh.indices.len() / 3;
    let mut out: Vec<Flat> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .step_by(stride(triangles))
        .filter_map(|t| {
            let [a, b, c] = [vertex(t[0])?, vertex(t[1])?, vertex(t[2])?];
            let pos = |v: &[f32]| [v[0], v[1], v[2]];
            let (pa, pb, pc) = (pos(a), pos(b), pos(c));
            let corners = [project(pa)?, project(pb)?, project(pc)?];
            let centroid = math::scale(math::add(math::add(pa, pb), pc), 1.0 / 3.0);
            let to_eye = math::sub(eye, centroid);
            let mut n = [a[3], a[4], a[5]];
            if math::dot(n, to_eye) < 0.0 {
                n = math::scale(n, -1.0);
            }
            let diffuse = math::dot(n, light).max(0.0);
            let base = [a[6], a[7], a[8]];
            let lit = math::scale(base, 0.28 + 0.72 * diffuse);
            Some(Flat {
                corners,
                depth: math::length(to_eye),
                color: color(lit),
            })
        })
        .collect();
    out.sort_by(|x, y| y.depth.total_cmp(&x.depth));
    out
}

/// A linear colour as an sRGB-ish GPUI colour.
fn color(c: [f32; 3]) -> Hsla {
    gpui::Rgba {
        r: c[0].clamp(0.0, 1.0).sqrt(),
        g: c[1].clamp(0.0, 1.0).sqrt(),
        b: c[2].clamp(0.0, 1.0).sqrt(),
        a: 1.0,
    }
    .into()
}

/// Paints `mesh` through `camera` in `bounds`.
pub fn paint(mesh: &Mesh, camera: &OrbitCamera, bounds: Bounds<Pixels>, window: &mut Window) {
    let size = (
        f32::from(bounds.size.width).max(1.0),
        f32::from(bounds.size.height).max(1.0),
    );
    let origin = bounds.origin;
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        window.paint_quad(gpui::fill(bounds, color(BACKGROUND)));
        for flat in flats(mesh, camera, size) {
            let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
            let mut path = PathBuilder::fill();
            path.move_to(at(flat.corners[0]));
            path.line_to(at(flat.corners[1]));
            path.line_to(at(flat.corners[2]));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, flat.color);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use domain::model3d::{ModelMesh, UpAxis};

    use super::*;

    #[test]
    fn large_models_are_sampled() {
        assert_eq!((stride(10), drawn(10)), (1, 10));
        assert_eq!(drawn(VECTOR_MAX_TRIANGLES), VECTOR_MAX_TRIANGLES);
        assert_eq!(stride(VECTOR_MAX_TRIANGLES + 1), 2);
        assert!(drawn(1_000_000) <= VECTOR_MAX_TRIANGLES);
        assert_eq!(stride(0), 1);
    }

    #[test]
    fn triangles_are_painted_back_to_front_and_lit_from_either_side() {
        // Two parallel squares, one behind the other along the view.
        let model = ModelMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 3.0],
                [1.0, 0.0, 3.0],
                [0.0, 1.0, 3.0],
            ],
            triangles: vec![[0, 1, 2], [5, 4, 3]],
            up: UpAxis::Y,
        };
        let (mesh, (min, max)) = Mesh::from_model(&model, UpAxis::Y, [0.5; 3]);
        let camera = OrbitCamera::fit_bounds(min, max);
        let flats = flats(&mesh, &camera, (400.0, 300.0));
        assert_eq!(flats.len(), 2);
        assert!(flats[0].depth > flats[1].depth, "farthest first");
        // Opposite windings, same facing: the same shade.
        assert_eq!(flats[0].color, flats[1].color);
    }
}
