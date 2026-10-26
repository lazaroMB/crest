//! Triangles of a board at an instant, built on the CPU from the domain's
//! [`Scene`] and [`Frame`]: bevelled chips (state shown as an emissive
//! top), pins, trace ribbons, emissive packets with trails, queue markers
//! and a selection ring. Rebuilt per rendered frame; boards are small.
//!
//! A 3D model's mesh ([`Mesh::from_model`]) is built once instead: flat
//! shaded, Y up, centered and scaled to a fixed size.

use domain::model3d::{ModelMesh, UpAxis};
use domain::sysmodel::scene::{DOWN, Frame, HIGHLIGHT, PACKET_RADIUS, Rgb, Scene};

use super::math::{self, V3};
use super::section::{self, Section};

/// Floats per vertex: position, normal, base colour, emissive.
pub const FLOATS_PER_VERTEX: usize = 12;
/// Half the diagonal of a model's box once built (about a board's).
pub const MODEL_RADIUS: f32 = 4.0;

/// Vertices and indices ready to upload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    /// `FLOATS_PER_VERTEX` floats per vertex.
    pub vertices: Vec<f32>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

/// Where a model's file coordinates go in the viewer: `up` turned to +Y,
/// centred on the origin and scaled to [`MODEL_RADIUS`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The file's up axis.
    pub up: UpAxis,
    /// Centre of the turned box, before scaling.
    pub center: V3,
    /// World units per file unit.
    pub scale: f32,
    /// The placed box.
    pub bounds: (V3, V3),
}

impl Placement {
    /// The placement of `model`'s box with `up` up (the identity for a
    /// model without triangles).
    #[must_use]
    pub fn of(model: &ModelMesh, up: UpAxis) -> Self {
        let Some((min, max)) = model.bounds() else {
            return Self {
                up,
                center: [0.0; 3],
                scale: 1.0,
                bounds: ([0.0; 3], [0.0; 3]),
            };
        };
        let (min, max) = (turn(up, min), turn(up, max));
        let (min, max) = (
            [min[0].min(max[0]), min[1].min(max[1]), min[2].min(max[2])],
            [min[0].max(max[0]), min[1].max(max[1]), min[2].max(max[2])],
        );
        let center = math::scale(math::add(min, max), 0.5);
        let half = math::length(math::sub(max, min)) / 2.0;
        let scale = if half > 0.0 { MODEL_RADIUS / half } else { 1.0 };
        Self {
            up,
            center,
            scale,
            bounds: (
                math::scale(math::sub(min, center), scale),
                math::scale(math::sub(max, center), scale),
            ),
        }
    }

    /// File point `p` in the viewer's world.
    #[must_use]
    pub fn to_world(&self, p: V3) -> V3 {
        math::scale(math::sub(turn(self.up, p), self.center), self.scale)
    }

    /// World point `w` back in file coordinates.
    #[must_use]
    pub fn to_file(&self, w: V3) -> V3 {
        let t = math::add(math::scale(w, 1.0 / self.scale), self.center);
        match self.up {
            UpAxis::Y => t,
            UpAxis::Z => [t[0], -t[2], t[1]],
        }
    }
}

/// `p` with `up` turned to +Y.
fn turn(up: UpAxis, p: V3) -> V3 {
    match up {
        UpAxis::Y => p,
        UpAxis::Z => [p[0], p[2], -p[1]],
    }
}

impl Mesh {
    /// Number of vertices.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / FLOATS_PER_VERTEX
    }

    fn vertex(&mut self, p: V3, n: V3, color: Rgb, emissive: Rgb) -> u32 {
        let index = u32::try_from(self.vertex_count()).unwrap_or(u32::MAX);
        self.vertices.extend_from_slice(&p);
        self.vertices.extend_from_slice(&n);
        self.vertices.extend_from_slice(&color);
        self.vertices.extend_from_slice(&emissive);
        index
    }

    /// The triangles of `model` with `up` pointing up (+Y), centered at the
    /// origin and scaled so its box's half diagonal is [`MODEL_RADIUS`].
    /// Each triangle gets its own three vertices and face normal, for flat
    /// shading. Returns the mesh and its box.
    #[must_use]
    pub fn from_model(model: &ModelMesh, up: UpAxis, color: Rgb) -> (Self, (V3, V3)) {
        let placement = Placement::of(model, up);
        let mesh = Self::from_model_cut(model, &placement, color, &|_| true, None);
        (mesh, placement.bounds)
    }

    /// The triangles of `model` placed by `placement` (so a cut or a
    /// filtered mesh stays where the whole model is), only those whose
    /// index `keep` accepts, cut by `section` (in file coordinates) when
    /// there is one.
    #[must_use]
    pub fn from_model_cut(
        model: &ModelMesh,
        placement: &Placement,
        color: Rgb,
        keep: &dyn Fn(usize) -> bool,
        section: Option<&Section>,
    ) -> Self {
        Self::from_model_painted(model, placement, &|_| color, keep, section)
    }

    /// [`Self::from_model_cut`] with a colour per triangle (`color` of
    /// the triangle's index), for a model coloured by part (atoms).
    #[must_use]
    pub fn from_model_painted(
        model: &ModelMesh,
        placement: &Placement,
        color: &dyn Fn(usize) -> Rgb,
        keep: &dyn Fn(usize) -> bool,
        section: Option<&Section>,
    ) -> Self {
        let mut mesh = Self {
            vertices: Vec::with_capacity(model.triangles.len() * 3 * FLOATS_PER_VERTEX),
            indices: Vec::with_capacity(model.triangles.len() * 3),
        };
        for (index, t) in model.triangles.iter().enumerate() {
            if !keep(index) {
                continue;
            }
            let Some(file) = t
                .iter()
                .map(|&i| model.positions.get(i as usize).copied())
                .collect::<Option<Vec<_>>>()
                .and_then(|v| <[V3; 3]>::try_from(v).ok())
            else {
                continue;
            };
            let polygon = match section {
                Some(section) => section::clip_triangle(file, section),
                None => file.to_vec(),
            };
            if polygon.len() < 3 {
                continue;
            }
            let placed: Vec<V3> = polygon.iter().map(|&p| placement.to_world(p)).collect();
            let (a, b, c) = (placed[0], placed[1], placed[2]);
            let n = math::cross(math::sub(b, a), math::sub(c, a));
            let n = if math::length(n) > 0.0 {
                math::normalize(n)
            } else {
                [0.0, 1.0, 0.0]
            };
            mesh.polygon(&placed, n, color(index), [0.0; 3]);
        }
        mesh
    }

    fn polygon(&mut self, points: &[V3], n: V3, color: Rgb, emissive: Rgb) {
        let first = self.vertex(points[0], n, color, emissive);
        let mut prev = self.vertex(points[1], n, color, emissive);
        for p in &points[2..] {
            let next = self.vertex(*p, n, color, emissive);
            self.indices.extend_from_slice(&[first, prev, next]);
            prev = next;
        }
    }

    /// A box with chamfered edges; `top` is the emissive of its upper face
    /// and `side` that of the rest.
    pub fn bevelled_box(
        &mut self,
        center: V3,
        size: V3,
        bevel: f32,
        color: Rgb,
        top: Rgb,
        side: Rgb,
    ) {
        let h = [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0];
        let b = bevel.min(h[0]).min(h[1]).min(h[2]).max(0.0);
        let at = |v: V3| math::add(center, v);
        // Faces.
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let corner = |su: f32, sv: f32| {
                    let mut p = [0.0; 3];
                    p[axis] = sign * h[axis];
                    p[u] = su * (h[u] - b);
                    p[v] = sv * (h[v] - b);
                    at(p)
                };
                let mut n = [0.0; 3];
                n[axis] = sign;
                let em = if axis == 1 && sign > 0.0 { top } else { side };
                self.polygon(
                    &[
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ],
                    n,
                    color,
                    em,
                );
            }
        }
        if b <= 0.0 {
            return;
        }
        // Edge chamfers.
        for a in 0..3 {
            for c in (a + 1)..3 {
                let along = 3 - a - c;
                for sa in [-1.0f32, 1.0] {
                    for sc in [-1.0f32, 1.0] {
                        let point = |on_a: bool, t: f32| {
                            let mut p = [0.0; 3];
                            p[a] = sa * if on_a { h[a] } else { h[a] - b };
                            p[c] = sc * if on_a { h[c] - b } else { h[c] };
                            p[along] = t * (h[along] - b);
                            at(p)
                        };
                        let mut n = [0.0; 3];
                        n[a] = sa;
                        n[c] = sc;
                        let n = math::normalize(n);
                        let em = if n[1] > 0.5 { top } else { side };
                        self.polygon(
                            &[
                                point(true, -1.0),
                                point(true, 1.0),
                                point(false, 1.0),
                                point(false, -1.0),
                            ],
                            n,
                            color,
                            em,
                        );
                    }
                }
            }
        }
        // Corner triangles.
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    let s = [sx, sy, sz];
                    let point = |axis: usize| {
                        let mut p = [0.0; 3];
                        for k in 0..3 {
                            p[k] = s[k] * if k == axis { h[k] } else { h[k] - b };
                        }
                        at(p)
                    };
                    let em = if sy > 0.0 { top } else { side };
                    self.polygon(
                        &[point(0), point(1), point(2)],
                        math::normalize(s),
                        color,
                        em,
                    );
                }
            }
        }
    }

    /// A flat ribbon along `points` (on a horizontal plane).
    pub fn ribbon(&mut self, points: &[V3], width: f32, color: Rgb, emissive: Rgb) {
        let w = width / 2.0;
        let up = [0.0, 1.0, 0.0];
        for seg in points.windows(2) {
            let d = math::normalize(math::sub(seg[1], seg[0]));
            let side = math::scale(math::normalize(math::cross(up, d)), w);
            let ext = math::scale(d, w);
            let (a, b) = (math::sub(seg[0], ext), math::add(seg[1], ext));
            self.polygon(
                &[
                    math::add(a, side),
                    math::sub(a, side),
                    math::sub(b, side),
                    math::add(b, side),
                ],
                up,
                color,
                emissive,
            );
        }
    }

    /// A low-poly sphere.
    pub fn sphere(&mut self, center: V3, radius: f32, color: Rgb, emissive: Rgb) {
        const RINGS: usize = 6;
        const SEGMENTS: usize = 10;
        let base = u32::try_from(self.vertex_count()).unwrap_or(u32::MAX);
        for r in 0..=RINGS {
            let phi = std::f32::consts::PI * r as f32 / RINGS as f32;
            for s in 0..=SEGMENTS {
                let theta = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
                let n = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
                self.vertex(
                    math::add(center, math::scale(n, radius)),
                    n,
                    color,
                    emissive,
                );
            }
        }
        let row = u32::try_from(SEGMENTS + 1).unwrap_or(u32::MAX);
        for r in 0..u32::try_from(RINGS).unwrap_or(0) {
            for s in 0..u32::try_from(SEGMENTS).unwrap_or(0) {
                let i = base + r * row + s;
                self.indices
                    .extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
    }

    /// A thin glowing frame on the board around a `half`-sized footprint.
    pub fn ring(&mut self, center: V3, half: [f32; 2], width: f32, emissive: Rgb) {
        let y = center[1];
        let (x0, x1) = (center[0] - half[0], center[0] + half[0]);
        let (z0, z1) = (center[2] - half[1], center[2] + half[1]);
        let pts = [
            [x0, y, z0],
            [x1, y, z0],
            [x1, y, z1],
            [x0, y, z1],
            [x0, y, z0],
        ];
        self.ribbon(&pts, width, [0.0; 3], emissive);
    }
}

fn mix(a: Rgb, s: f32) -> Rgb {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// How much an element that is not related to the selection keeps of its
/// colour.
const DIM: f32 = 0.22;

/// The triangles of `scene` at `frame`, with `selected` highlighted: its
/// traces glow and the chips at their other end stay lit, while the rest
/// of the board dims. A trace with a message in flight glows in the
/// message's colour.
#[must_use]
pub fn build(scene: &Scene, frame: &Frame, selected: Option<&str>) -> Mesh {
    let mut mesh = Mesh::default();
    let related = selected.map(|id| scene.related(id));
    let lit_chip = |id: &str| related.as_ref().is_none_or(|r| r.has_chip(id));
    let lit_ribbon = |id: &str| related.as_ref().is_none_or(|r| r.has_ribbon(id));
    let board = &scene.board;
    let center = [
        (board.min[0] + board.max[0]) / 2.0,
        (board.min[1] + board.max[1]) / 2.0,
        (board.min[2] + board.max[2]) / 2.0,
    ];
    let size = math::sub(board.max, board.min);
    mesh.bevelled_box(
        center,
        size,
        0.12,
        board.material.base,
        board.material.emissive,
        board.material.emissive,
    );
    for ribbon in &scene.ribbons {
        let traffic = frame.packets.iter().find(|p| p.conn == ribbon.id);
        let mut glow = match traffic {
            Some(packet) => mix(packet.color, 0.35),
            None => ribbon.material.emissive,
        };
        let mut base = ribbon.material.base;
        if related.is_some() {
            if lit_ribbon(&ribbon.id) {
                glow = match traffic {
                    Some(packet) => mix(packet.color, 0.9),
                    None => HIGHLIGHT,
                };
            } else {
                glow = mix(glow, DIM);
                base = mix(base, DIM);
            }
        }
        mesh.ribbon(&ribbon.points, ribbon.width, base, glow);
    }
    for chip in &scene.chips {
        let look = frame.chips.iter().find(|c| c.id == chip.id);
        let (state, busy, up, queued) = look.map_or(([0.2; 3], false, true, 0), |l| {
            (l.emissive, l.busy, l.up, l.queued)
        });
        let lit = lit_chip(&chip.id);
        let dim = if lit { 1.0 } else { DIM };
        let top = if up {
            mix(state, if busy { 1.1 } else { 0.35 } * dim)
        } else {
            mix(DOWN, 1.2 * dim)
        };
        let is_selected = selected == Some(chip.id.as_str());
        // A thin coloured edge: the state, or the accent when selected.
        let side = if is_selected {
            mix(HIGHLIGHT, 0.6)
        } else {
            mix(if up { state } else { DOWN }, 0.06 * dim)
        };
        mesh.bevelled_box(
            chip.center,
            chip.size,
            chip.bevel,
            mix(chip.material.base, dim),
            top,
            side,
        );
        for pin in &chip.pins {
            mesh.bevelled_box(
                pin.at,
                [0.30, 0.08, 0.16],
                0.0,
                mix([0.10, 0.10, 0.11], dim),
                [0.0; 3],
                [0.0; 3],
            );
        }
        let top_y = chip.center[1] + chip.size[1] / 2.0;
        for q in 0..queued.min(8) {
            let x = chip.center[0] - chip.size[0] / 2.0 + 0.35 + 0.36 * q as f32;
            mesh.bevelled_box(
                [x, top_y + 0.1, chip.center[2] + chip.size[2] / 2.0 - 0.35],
                [0.26, 0.2, 0.26],
                0.04,
                [0.05, 0.04, 0.02],
                mix([1.6, 0.9, 0.1], dim),
                mix([0.8, 0.45, 0.05], dim),
            );
        }
        if is_selected || (lit && related.is_some()) {
            let (width, glow) = if is_selected {
                (0.12, mix(HIGHLIGHT, 2.0))
            } else {
                (0.06, mix(HIGHLIGHT, 0.8))
            };
            mesh.ring(
                [chip.center[0], 0.04, chip.center[2]],
                [chip.size[0] / 2.0 + 0.35, chip.size[2] / 2.0 + 0.35],
                width,
                glow,
            );
        }
    }
    for packet in &frame.packets {
        let dim = if lit_ribbon(&packet.conn) { 1.0 } else { DIM };
        mesh.sphere(
            packet.at,
            PACKET_RADIUS,
            mix(packet.color, dim),
            mix(packet.color, 2.5 * dim),
        );
        for (i, p) in packet.trail.iter().enumerate() {
            let fade = 1.0 - (i as f32 + 1.0) / (packet.trail.len() as f32 + 1.0);
            mesh.sphere(
                *p,
                PACKET_RADIUS * (0.4 + 0.5 * fade),
                mix(packet.color, dim),
                mix(packet.color, 1.6 * fade * dim),
            );
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bevelled_box_is_closed_and_indexed() {
        let mut m = Mesh::default();
        m.bevelled_box([0.0; 3], [2.0, 1.0, 2.0], 0.1, [1.0; 3], [0.5; 3], [0.0; 3]);
        // 6 faces + 12 chamfers (4 vertices each) + 8 corners (3 each).
        assert_eq!(m.vertex_count(), 6 * 4 + 12 * 4 + 8 * 3);
        assert_eq!(m.indices.len(), (6 * 2 + 12 * 2 + 8) * 3);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertex_count()));
        // Every vertex lies on the box's surface.
        for v in m.vertices.chunks(FLOATS_PER_VERTEX) {
            assert!(
                v[0].abs() <= 1.0 + 1e-6 && v[1].abs() <= 0.5 + 1e-6 && v[2].abs() <= 1.0 + 1e-6
            );
        }
        let mut flat = Mesh::default();
        flat.bevelled_box([0.0; 3], [1.0; 3], 0.0, [1.0; 3], [0.0; 3], [0.0; 3]);
        assert_eq!(flat.vertex_count(), 24);
    }

    #[test]
    fn a_model_is_flat_shaded_centered_and_turned_y_up() {
        let model = ModelMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [0.0, 2.0, 0.0],
                [0.0, 0.0, 6.0],
            ],
            triangles: vec![[0, 1, 2], [0, 1, 3]],
            up: UpAxis::Z,
        };
        let (mesh, (min, max)) = Mesh::from_model(&model, UpAxis::Z, [0.5; 3]);
        assert_eq!(mesh.vertex_count(), 6, "three vertices per triangle");
        assert_eq!(mesh.indices, [0, 1, 2, 3, 4, 5]);
        let half = math::length(math::sub(max, min)) / 2.0;
        assert!((half - MODEL_RADIUS).abs() < 1e-4, "{half}");
        // The file's Z (the tallest side) is the height now.
        let size = math::sub(max, min);
        assert!(size[1] > size[0] && size[1] > size[2], "{size:?}");
        assert!(math::length(math::add(min, max)) < 1e-4, "centered");
        // The first triangle lies in the file's XY plane: its normal is
        // vertical in the world.
        let n = &mesh.vertices[3..6];
        assert!(n[1].abs() > 0.99, "{n:?}");
        let (y_up, _) = Mesh::from_model(&model, UpAxis::Y, [0.5; 3]);
        assert!(y_up.vertices[4].abs() < 1e-6, "Y up keeps the file's axes");
    }

    #[test]
    fn placement_round_trips_and_cuts_stay_in_place() {
        let model = ModelMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [0.0, 2.0, 0.0],
                [0.0, 0.0, 6.0],
            ],
            triangles: vec![[0, 1, 2], [0, 1, 3]],
            up: UpAxis::Z,
        };
        for up in [UpAxis::Z, UpAxis::Y] {
            let placement = Placement::of(&model, up);
            let p = [1.5, -0.5, 4.0];
            let back = placement.to_file(placement.to_world(p));
            assert!(math::length(math::sub(back, p)) < 1e-5, "{up:?}: {back:?}");
        }
        let placement = Placement::of(&model, UpAxis::Z);
        let whole = Mesh::from_model_cut(&model, &placement, [0.5; 3], &|_| true, None);
        assert_eq!(whole, Mesh::from_model(&model, UpAxis::Z, [0.5; 3]).0);
        let second = Mesh::from_model_cut(&model, &placement, [0.5; 3], &|i| i == 1, None);
        assert_eq!(second.vertex_count(), 3, "only the kept triangle");
        // z ≤ 3 leaves the floor triangle and a quad of the wall.
        let cut = Section {
            axis: 2,
            offset: 3.0,
            flip: false,
        };
        let cut = Mesh::from_model_cut(&model, &placement, [0.5; 3], &|_| true, Some(&cut));
        assert_eq!(cut.vertex_count(), 3 + 4);
        assert_eq!(cut.indices.len(), 3 * 3, "a triangle and a quad's two");
        let top = cut
            .vertices
            .chunks(FLOATS_PER_VERTEX)
            .map(|v| placement.to_file([v[0], v[1], v[2]])[2])
            .fold(f32::MIN, f32::max);
        assert!((top - 3.0).abs() < 1e-4, "cut at z = 3: {top}");
    }

    #[test]
    fn ribbons_and_spheres() {
        let mut m = Mesh::default();
        m.ribbon(
            &[[0.0; 3], [2.0, 0.0, 0.0], [2.0, 0.0, 3.0]],
            0.2,
            [1.0; 3],
            [0.0; 3],
        );
        assert_eq!(m.vertex_count(), 8);
        m.sphere([0.0; 3], 1.0, [1.0; 3], [1.0; 3]);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertex_count()));
    }
}
