//! The board camera: an orthographic isometric preset (yaw 45°, pitch 30°)
//! and free orbit, zoom and pan in perspective, plus projection of world
//! points to the screen (labels) and rays from the screen (picking).

use domain::sysmodel::scene::Scene;

use super::math::{self, M4, V3};

/// Yaw of the isometric preset.
pub const ISO_YAW: f32 = std::f32::consts::FRAC_PI_4;
/// Pitch of the isometric preset (30°).
pub const ISO_PITCH: f32 = std::f32::consts::FRAC_PI_6;
/// Vertical field of view in perspective.
const FOV: f32 = 0.6;
/// Zoom limits.
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;

/// Where the board is seen from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    /// Point looked at.
    pub target: V3,
    /// Rotation around the vertical axis, radians.
    pub yaw: f32,
    /// Elevation above the board, radians.
    pub pitch: f32,
    /// Zoom over the fitted view.
    pub zoom: f32,
    /// Orthographic (the isometric preset) or perspective.
    pub ortho: bool,
    /// Radius of the scene, for fitting.
    pub radius: f32,
    center: V3,
}

impl OrbitCamera {
    /// The isometric preset fitted to `scene`.
    #[must_use]
    pub fn fit(scene: &Scene) -> Self {
        let (min, max) = (scene.board.min, scene.board.max);
        let center = [(min[0] + max[0]) / 2.0, 0.0, (min[2] + max[2]) / 2.0];
        let radius = (math::length(math::sub(max, min)) / 2.0).max(1.0);
        Self {
            target: center,
            yaw: ISO_YAW,
            pitch: ISO_PITCH,
            zoom: 1.0,
            ortho: true,
            radius,
            center,
        }
    }

    /// Back to the isometric preset.
    pub fn iso(&mut self) {
        self.yaw = ISO_YAW;
        self.pitch = ISO_PITCH;
        self.zoom = 1.0;
        self.ortho = true;
        self.target = self.center;
    }

    fn distance(&self) -> f32 {
        self.radius * 2.6 / self.zoom
    }

    /// Height of the visible area at the target, in world units.
    #[must_use]
    pub fn view_height(&self) -> f32 {
        if self.ortho {
            self.radius * 1.25 / self.zoom
        } else {
            2.0 * self.distance() * (FOV / 2.0).tan()
        }
    }

    /// Camera position.
    #[must_use]
    pub fn eye(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let d = if self.ortho {
            self.radius * 4.0
        } else {
            self.distance()
        };
        math::add(self.target, [d * cp * sy, d * sp, d * cp * cy])
    }

    /// World to view.
    #[must_use]
    pub fn view(&self) -> M4 {
        math::look_at(self.eye(), self.target, [0.0, 1.0, 0.0])
    }

    /// View to clip, for a viewport of `aspect` (width / height).
    #[must_use]
    pub fn projection(&self, aspect: f32) -> M4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        if self.ortho {
            let h = self.view_height();
            math::orthographic(h * aspect, h, 0.1, self.radius * 10.0)
        } else {
            math::perspective(FOV, aspect, 0.1, self.radius * 12.0)
        }
    }

    /// World to clip.
    #[must_use]
    pub fn view_projection(&self, aspect: f32) -> M4 {
        math::mul(&self.projection(aspect), &self.view())
    }

    /// Orbits by a mouse drag of `(dx, dy)` pixels; leaves the preset for
    /// perspective.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.ortho = false;
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(0.08, 1.48);
    }

    /// Moves the target by a drag of `(dx, dy)` pixels in a viewport
    /// `height` pixels tall.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        let per_px = self.view_height() / height.max(1.0);
        let (sy, cy) = self.yaw.sin_cos();
        let right = [cy, 0.0, -sy];
        let forward = [-sy, 0.0, -cy];
        self.target = math::add(
            self.target,
            math::add(
                math::scale(right, -dx * per_px),
                math::scale(forward, dy * per_px),
            ),
        );
    }

    /// Multiplies the zoom by `factor`, within limits.
    pub fn zoom_by(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        }
    }

    /// Pixel position of world point `p` in a `size` viewport, if in
    /// front of the camera.
    #[must_use]
    pub fn project(&self, p: V3, size: (f32, f32)) -> Option<(f32, f32)> {
        let clip = math::transform(&self.view_projection(size.0 / size.1), p)?;
        (clip[2] >= 0.0 && clip[2] <= 1.0).then(|| {
            (
                (clip[0] + 1.0) / 2.0 * size.0,
                (1.0 - clip[1]) / 2.0 * size.1,
            )
        })
    }

    /// The ray through pixel `(x, y)`: origin and unit direction.
    #[must_use]
    pub fn ray(&self, x: f32, y: f32, size: (f32, f32)) -> Option<(V3, V3)> {
        let inv = math::invert(&self.view_projection(size.0 / size.1))?;
        let nx = x / size.0 * 2.0 - 1.0;
        let ny = 1.0 - y / size.1 * 2.0;
        let near = math::transform(&inv, [nx, ny, 0.0])?;
        let far = math::transform(&inv, [nx, ny, 1.0])?;
        Some((near, math::normalize(math::sub(far, near))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::sysmodel::scene::{Material, Slab};

    fn scene() -> Scene {
        Scene {
            board: Slab {
                min: [-10.0, -0.4, -6.0],
                max: [10.0, 0.0, 6.0],
                material: Material {
                    base: [0.0; 3],
                    emissive: [0.0; 3],
                    metallic: 0.0,
                    roughness: 1.0,
                },
            },
            chips: Vec::new(),
            ribbons: Vec::new(),
            labels: Vec::new(),
        }
    }

    #[test]
    fn the_preset_centers_the_board() {
        let cam = OrbitCamera::fit(&scene());
        let size = (800.0, 600.0);
        let (x, y) = cam.project([0.0, 0.0, 0.0], size).unwrap();
        assert!(
            (x - 400.0).abs() < 1.0 && (y - 300.0).abs() < 1.0,
            "{x} {y}"
        );
        // Higher points appear higher on screen.
        let (_, y_up) = cam.project([0.0, 2.0, 0.0], size).unwrap();
        assert!(y_up < y);
    }

    #[test]
    fn rays_hit_what_projects_there() {
        for ortho in [true, false] {
            let mut cam = OrbitCamera::fit(&scene());
            if !ortho {
                cam.orbit(30.0, -10.0);
            }
            let size = (640.0, 480.0);
            let p = [3.0, 0.5, -2.0];
            let (x, y) = cam.project(p, size).unwrap();
            let (origin, dir) = cam.ray(x, y, size).unwrap();
            let to_p = math::sub(p, origin);
            let along = math::dot(to_p, dir);
            let closest = math::add(origin, math::scale(dir, along));
            assert!(math::length(math::sub(closest, p)) < 1e-2, "ortho {ortho}");
        }
    }

    #[test]
    fn orbit_zoom_and_pan_stay_sane() {
        let mut cam = OrbitCamera::fit(&scene());
        cam.orbit(10_000.0, 10_000.0);
        assert!(!cam.ortho);
        assert!(cam.pitch <= 1.48);
        cam.zoom_by(1000.0);
        assert_eq!(cam.zoom, 8.0);
        cam.zoom_by(f32::NAN);
        assert_eq!(cam.zoom, 8.0);
        let before = cam.target;
        cam.pan(50.0, 0.0, 600.0);
        assert_ne!(before, cam.target);
        cam.iso();
        assert!(cam.ortho);
        assert_eq!(cam.target, OrbitCamera::fit(&scene()).target);
    }
}
