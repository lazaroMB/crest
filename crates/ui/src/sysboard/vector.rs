//! The 2.5D fallback of the board, for machines without a GPU adapter:
//! the same scene drawn as an isometric projection with GPUI paths.
//!
//! `sx = (x − z)·cos 30°`, `sy = (x + z)·sin 30° − y`, scaled to fit the
//! viewport. Chips are boxes drawn as three shaded faces in painter's
//! order (back to front by `x + z`); traces are a dark wide stroke under a
//! thin bright one; packets are glowing dots. Pan and zoom move the
//! projection; picking tests the projected outline of each chip.

use domain::sysmodel::scene::{DOWN, Frame, Rgb, Scene, V3};
use gpui::{Bounds, ContentMask, Hsla, PathBuilder, Pixels, Rgba, Window, point, px};

const COS30: f32 = 0.866_025_4;
const SIN30: f32 = 0.5;

/// Pan and zoom of the fallback view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoView {
    /// Zoom over the fitted scale.
    pub zoom: f32,
    /// Offset in pixels.
    pub pan: (f32, f32),
}

impl Default for IsoView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: (0.0, 0.0),
        }
    }
}

impl IsoView {
    /// Multiplies the zoom by `factor`, within limits.
    pub fn zoom_by(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.zoom = (self.zoom * factor).clamp(0.25, 8.0);
        }
    }
}

/// World to viewport pixels for one viewport size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsoProjector {
    scale: f32,
    offset: (f32, f32),
}

fn raw(p: V3) -> (f32, f32) {
    ((p[0] - p[2]) * COS30, (p[0] + p[2]) * SIN30 - p[1])
}

impl IsoProjector {
    /// Fits `scene` in `size` pixels, then applies `view`.
    #[must_use]
    pub fn new(scene: &Scene, size: (f32, f32), view: IsoView) -> Self {
        let (min, max) = (scene.board.min, scene.board.max);
        let corners = [
            [min[0], 0.0, min[2]],
            [max[0], 0.0, min[2]],
            [min[0], 0.0, max[2]],
            [max[0], 0.0, max[2]],
            [min[0], 3.0, min[2]],
            [max[0], min[1], max[2]],
        ];
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for c in corners {
            let (x, y) = raw(c);
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
        let (w, h) = ((hi.0 - lo.0).max(1.0), (hi.1 - lo.1).max(1.0));
        let scale = (size.0 / w).min(size.1 / h) * 0.92 * view.zoom;
        let center = ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0);
        Self {
            scale,
            offset: (
                size.0 / 2.0 - center.0 * scale + view.pan.0,
                size.1 / 2.0 - center.1 * scale + view.pan.1,
            ),
        }
    }

    /// Pixel position of `p`, relative to the viewport origin.
    #[must_use]
    pub fn project(&self, p: V3) -> (f32, f32) {
        let (x, y) = raw(p);
        (
            x * self.scale + self.offset.0,
            y * self.scale + self.offset.1,
        )
    }
}

fn color(rgb: Rgb, alpha: f32) -> Hsla {
    // Linear to display: a gamma of 2 is close enough for flat shading.
    let c = |v: f32| v.clamp(0.0, 1.0).sqrt();
    Rgba {
        r: c(rgb[0]),
        g: c(rgb[1]),
        b: c(rgb[2]),
        a: alpha,
    }
    .into()
}

fn add(a: Rgb, b: Rgb, s: f32) -> Rgb {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

fn shade(a: Rgb, s: f32) -> Rgb {
    [a[0] * s, a[1] * s, a[2] * s]
}

struct Painter<'a> {
    proj: IsoProjector,
    origin: gpui::Point<Pixels>,
    window: &'a mut Window,
}

impl Painter<'_> {
    fn pt(&self, p: V3) -> gpui::Point<Pixels> {
        let (x, y) = self.proj.project(p);
        point(self.origin.x + px(x), self.origin.y + px(y))
    }

    fn fill(&mut self, points: &[V3], c: Hsla) {
        let mut b = PathBuilder::fill();
        b.move_to(self.pt(points[0]));
        for p in &points[1..] {
            b.line_to(self.pt(*p));
        }
        b.close();
        if let Ok(path) = b.build() {
            self.window.paint_path(path, c);
        }
    }

    fn stroke(&mut self, points: &[V3], width: f32, c: Hsla) {
        let mut b = PathBuilder::stroke(px(width.max(0.5)));
        b.move_to(self.pt(points[0]));
        for p in &points[1..] {
            b.line_to(self.pt(*p));
        }
        if let Ok(path) = b.build() {
            self.window.paint_path(path, c);
        }
    }

    fn dot(&mut self, at: V3, radius: f32, c: Hsla) {
        let center = self.proj.project(at);
        let mut b = PathBuilder::fill();
        for k in 0..=12 {
            let a = std::f32::consts::TAU * k as f32 / 12.0;
            let p = point(
                self.origin.x + px(center.0 + radius * a.cos()),
                self.origin.y + px(center.1 + radius * a.sin()),
            );
            if k == 0 {
                b.move_to(p);
            } else {
                b.line_to(p);
            }
        }
        b.close();
        if let Ok(path) = b.build() {
            self.window.paint_path(path, c);
        }
    }

    /// A box as its three visible faces (top, +x, +z).
    fn block(&mut self, min: V3, max: V3, top: Rgb, side: Rgb) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        self.fill(
            &[[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
            color(shade(side, 0.75), 1.0),
        );
        self.fill(
            &[[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            color(shade(side, 0.5), 1.0),
        );
        self.fill(
            &[[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            color(top, 1.0),
        );
    }
}

/// Paints `frame` of `scene` in `bounds` with `selected` highlighted.
pub fn paint(
    scene: &Scene,
    frame: &Frame,
    selected: Option<&str>,
    proj: IsoProjector,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        window.paint_quad(gpui::fill(bounds, color([0.010, 0.013, 0.020], 1.0)));
        let mut p = Painter {
            proj,
            origin: bounds.origin,
            window,
        };
        let board = &scene.board;
        p.block(
            board.min,
            board.max,
            add(board.material.base, [0.02, 0.05, 0.03], 1.0),
            board.material.base,
        );
        let scale = proj.scale;
        for ribbon in &scene.ribbons {
            if ribbon.points.len() < 2 {
                continue;
            }
            p.stroke(
                &ribbon.points,
                ribbon.width * scale * 1.8,
                color([0.02, 0.03, 0.02], 1.0),
            );
            p.stroke(
                &ribbon.points,
                ribbon.width * scale * 0.7,
                color(ribbon.material.base, 1.0),
            );
        }
        let mut chips: Vec<_> = scene.chips.iter().collect();
        chips.sort_by(|a, b| (a.center[0] + a.center[2]).total_cmp(&(b.center[0] + b.center[2])));
        for chip in chips {
            let look = frame.chips.iter().find(|c| c.id == chip.id);
            let (state, busy, up) =
                look.map_or(([0.2; 3], false, true), |l| (l.emissive, l.busy, l.up));
            let glow = if up { state } else { DOWN };
            let top = add(chip.material.base, glow, if busy { 0.85 } else { 0.4 });
            if selected == Some(chip.id.as_str()) {
                let (lo, hi) = (chip.min(), chip.max());
                let ring = [
                    [lo[0] - 0.35, 0.02, lo[2] - 0.35],
                    [hi[0] + 0.35, 0.02, lo[2] - 0.35],
                    [hi[0] + 0.35, 0.02, hi[2] + 0.35],
                    [lo[0] - 0.35, 0.02, hi[2] + 0.35],
                    [lo[0] - 0.35, 0.02, lo[2] - 0.35],
                ];
                p.stroke(&ring, (0.12 * scale).max(1.5), color([0.5, 0.9, 1.0], 1.0));
            }
            p.block(chip.min(), chip.max(), top, chip.material.base);
            for pin in &chip.pins {
                let h = [0.18, 0.06, 0.11];
                p.block(
                    [pin.at[0] - h[0], pin.at[1] - h[1], pin.at[2] - h[2]],
                    [pin.at[0] + h[0], pin.at[1] + h[1], pin.at[2] + h[2]],
                    [0.8, 0.62, 0.25],
                    [0.5, 0.38, 0.12],
                );
            }
        }
        for packet in &frame.packets {
            for (i, t) in packet.trail.iter().enumerate() {
                let fade = 1.0 - (i as f32 + 1.0) / (packet.trail.len() as f32 + 1.0);
                p.dot(
                    *t,
                    (0.18 * scale).max(1.5) * fade,
                    color(packet.color, 0.6 * fade),
                );
            }
            p.dot(packet.at, (0.5 * scale).max(3.0), color(packet.color, 0.25));
            p.dot(packet.at, (0.28 * scale).max(2.0), color(packet.color, 1.0));
        }
    });
}

/// The chip whose projected outline contains `at` (viewport pixels),
/// front-most first.
#[must_use]
pub fn pick<'a>(scene: &'a Scene, proj: &IsoProjector, at: (f32, f32)) -> Option<&'a str> {
    let mut best: Option<(f32, &str)> = None;
    for chip in &scene.chips {
        let (lo, hi) = (chip.min(), chip.max());
        let mut min = (f32::MAX, f32::MAX);
        let mut max = (f32::MIN, f32::MIN);
        for x in [lo[0], hi[0]] {
            for y in [lo[1], hi[1]] {
                for z in [lo[2], hi[2]] {
                    let (sx, sy) = proj.project([x, y, z]);
                    min = (min.0.min(sx), min.1.min(sy));
                    max = (max.0.max(sx), max.1.max(sy));
                }
            }
        }
        let inside = at.0 >= min.0 && at.0 <= max.0 && at.1 >= min.1 && at.1 <= max.1;
        let depth = chip.center[0] + chip.center[2];
        if inside && best.is_none_or(|(d, _)| depth > d) {
            best = Some((depth, chip.id.as_str()));
        }
    }
    best.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use domain::sysmodel::SysModel;
    use domain::sysmodel::scene::scene_of;

    use super::*;

    fn scene() -> Scene {
        scene_of(
            &SysModel::from_value(serde_json::json!({
                "version": "crest.sysmodel/v1",
                "components": [
                    { "id": "a", "kind": "client", "machine": { "initial": "s", "states": ["s"] } },
                    { "id": "b", "kind": "db", "layout": { "col": 1, "row": 0 },
                      "machine": { "initial": "s", "states": ["s"] } }
                ]
            }))
            .unwrap(),
        )
    }

    #[test]
    fn the_board_fits_and_chips_are_picked() {
        let s = scene();
        let size = (800.0, 500.0);
        let proj = IsoProjector::new(&s, size, IsoView::default());
        for chip in &s.chips {
            let (x, y) = proj.project(chip.center);
            assert!((0.0..=size.0).contains(&x) && (0.0..=size.1).contains(&y));
            assert_eq!(pick(&s, &proj, (x, y)), Some(chip.id.as_str()));
        }
        assert_eq!(pick(&s, &proj, (1.0, 1.0)), None);
        let mut view = IsoView::default();
        view.zoom_by(2.0);
        view.pan = (10.0, 0.0);
        let zoomed = IsoProjector::new(&s, size, view);
        let (a, b) = (
            proj.project(s.chips[1].center),
            zoomed.project(s.chips[1].center),
        );
        assert_ne!(a, b);
    }
}
