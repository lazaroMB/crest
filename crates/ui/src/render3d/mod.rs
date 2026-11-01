//! The offscreen 3D renderer of the simulation board and of the 3D models
//! `/open` shows.
//!
//! GPUI cannot composite an external GPU texture on Linux, so the board is
//! drawn by a `wgpu::Device` of its own into a texture, read back and shown
//! as a GPUI [`RenderImage`]. It renders only when something changed (the
//! caller keys its frames), so an idle board costs nothing.
//!
//! - [`mesh`]: triangles from the domain's renderer-agnostic scene (lit
//!   bevelled chips, ribbons, emissive packets with trails), or from a
//!   model's mesh ([`Mesh::from_model`](mesh::Mesh::from_model));
//! - [`gpu`]: the device, the passes of `board.wgsl` (scene, edge outlines,
//!   bloom, tone-mapped composite) and the readback;
//! - [`camera`]: the isometric preset and orbit/zoom/pan, projection for
//!   the labels GPUI draws on top, and rays for [`pick`]ing on the CPU;
//! - [`section`]: a model cut by an axis-aligned plane and the surface
//!   point under a ray, for the model viewer's section and measuring.
//!
//! [`Renderer3d::new`] fails cleanly when there is no adapter; the board
//! then falls back to its 2.5D vector drawing. It blocks while the driver
//! opens the device, so the app calls it on the background executor
//! ([`crate::sysboard::auto_backend`]).
//!
//! Views do not wait for the GPU either: [`Renderer3d::start_mesh`] submits
//! a frame for an owner (a view), [`Renderer3d::poll`] checks the device
//! without blocking and [`Renderer3d::take`] hands the owner its frame once
//! read back. Meanwhile the view shows its previous image and asks for
//! another frame. One frame is in flight at a time for the whole app.

pub mod camera;
pub mod gpu;
pub mod math;
pub mod mesh;
pub mod pick;
pub mod section;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::sysmodel::scene::{Frame, Scene};
use gpui::RenderImage;

use camera::OrbitCamera;

/// Why the renderer is not available or a frame failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// No GPU or software adapter.
    #[error("no GPU adapter: {0}")]
    NoAdapter(String),
    /// The device could not be created.
    #[error("cannot create the GPU device: {0}")]
    Device(String),
    /// A frame failed.
    #[error("render failed: {0}")]
    Render(String),
}

/// What one frame cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderStats {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Vertices drawn.
    pub vertices: usize,
    /// Upload, encode and wait for the GPU, in milliseconds.
    pub gpu_ms: f32,
    /// Mapping and copying the pixels out, in milliseconds.
    pub readback_ms: f32,
}

/// A rendered frame.
pub struct Rendered {
    /// The image for GPUI.
    pub image: Arc<RenderImage>,
    /// Its cost.
    pub stats: RenderStats,
}

/// Frames read back and not yet taken that are kept at most (one per
/// owner; an owner that went away leaves its frame behind).
const DONE_KEPT: usize = 8;

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// A new owner id for [`Renderer3d::start_mesh`]: one per view.
pub fn next_owner() -> u64 {
    NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
}

/// The renderer: one device for the whole app.
pub struct Renderer3d {
    gpu: gpu::Gpu,
    /// Owner of the frame in flight.
    in_flight: Option<u64>,
    /// Frames read back, by owner, oldest first.
    done: Vec<(u64, Result<Rendered, RenderError>)>,
}

impl std::fmt::Debug for Renderer3d {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer3d")
            .field("adapter", &self.gpu.adapter)
            .finish_non_exhaustive()
    }
}

/// Wraps BGRA bytes as a GPUI image.
#[must_use]
pub fn render_image(bgra: Vec<u8>, width: u32, height: u32) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The camera uniform for an image of `size`.
fn uniform(camera: &OrbitCamera, size: (u32, u32)) -> gpu::CameraUniform {
    let aspect = size.0.max(1) as f32 / size.1.max(1) as f32;
    gpu::CameraUniform {
        view_proj: camera.view_projection(aspect),
        eye: camera.eye(),
        light: math::normalize([0.45, 1.0, 0.65]),
        inv_far: 1.0 / (camera.radius * 12.0),
    }
}

/// The bytes read back, as a frame.
fn rendered(bgra: Vec<u8>, stats: RenderStats) -> Result<Rendered, RenderError> {
    let image = render_image(bgra, stats.width, stats.height)
        .ok_or_else(|| RenderError::Render("readback size mismatch".into()))?;
    Ok(Rendered { image, stats })
}

impl Renderer3d {
    /// Opens a device.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoAdapter`] or [`RenderError::Device`].
    pub fn new() -> Result<Self, RenderError> {
        Ok(Self {
            gpu: gpu::Gpu::new()?,
            in_flight: None,
            done: Vec::new(),
        })
    }

    /// `name (backend)` of the adapter.
    #[must_use]
    pub fn adapter(&self) -> &str {
        &self.gpu.adapter
    }

    /// MSAA samples used.
    #[must_use]
    pub fn samples(&self) -> u32 {
        self.gpu.samples()
    }

    /// Sets the background around the scene (`canvas.bg` of the theme).
    pub fn set_background(&mut self, color: domain::theme::Color) {
        self.gpu.set_background([color.r, color.g, color.b]);
    }

    /// Renders `frame` of `scene` through `camera` at `size` device pixels,
    /// with `selected` highlighted.
    ///
    /// # Errors
    ///
    /// [`RenderError::Render`].
    pub fn render(
        &mut self,
        scene: &Scene,
        frame: &Frame,
        camera: &OrbitCamera,
        selected: Option<&str>,
        size: (u32, u32),
    ) -> Result<Rendered, RenderError> {
        let mesh = mesh::build(scene, frame, selected);
        self.render_mesh(&mesh, None, camera, size)
    }

    /// Renders `mesh` through `camera` at `size` device pixels. A `key`
    /// names the mesh: the same key again reuses the uploaded buffers, so
    /// the caller must change it whenever the mesh changes.
    ///
    /// # Errors
    ///
    /// [`RenderError::Render`].
    pub fn render_mesh(
        &mut self,
        mesh: &mesh::Mesh,
        key: Option<u64>,
        camera: &OrbitCamera,
        size: (u32, u32),
    ) -> Result<Rendered, RenderError> {
        // A frame in flight for a view goes first.
        self.finish(true);
        let uniform = uniform(camera, size);
        let (bgra, stats) = self.gpu.render(mesh, key, &uniform, size.0, size.1)?;
        rendered(bgra, stats)
    }

    /// Whether a frame is in flight (for any owner).
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Submits a frame of `frame` of `scene` for `owner`, as
    /// [`Self::render`] draws it, without waiting for it. Returns `false`
    /// (and submits nothing) while another frame is in flight.
    ///
    /// # Errors
    ///
    /// [`RenderError::Render`].
    pub fn start(
        &mut self,
        owner: u64,
        scene: &Scene,
        frame: &Frame,
        camera: &OrbitCamera,
        selected: Option<&str>,
        size: (u32, u32),
    ) -> Result<bool, RenderError> {
        if self.is_busy() {
            return Ok(false);
        }
        let mesh = mesh::build(scene, frame, selected);
        self.start_mesh(owner, &mesh, None, camera, size)
    }

    /// Submits a frame of `mesh` for `owner` (see [`Self::render_mesh`] for
    /// `key`) without waiting for it: [`Self::poll`] and [`Self::take`]
    /// collect it. Returns `false` (and submits nothing) while another
    /// frame is in flight.
    ///
    /// # Errors
    ///
    /// [`RenderError::Render`].
    pub fn start_mesh(
        &mut self,
        owner: u64,
        mesh: &mesh::Mesh,
        key: Option<u64>,
        camera: &OrbitCamera,
        size: (u32, u32),
    ) -> Result<bool, RenderError> {
        if self.is_busy() {
            return Ok(false);
        }
        let uniform = uniform(camera, size);
        self.gpu.submit(mesh, key, &uniform, size.0, size.1)?;
        self.in_flight = Some(owner);
        Ok(true)
    }

    /// Checks the device without blocking: a frame in flight that the GPU
    /// finished is read back and kept for its owner.
    pub fn poll(&mut self) {
        self.finish(false);
    }

    fn finish(&mut self, wait: bool) {
        let Some(result) = self.gpu.try_finish(wait) else {
            return;
        };
        let Some(owner) = self.in_flight.take() else {
            return;
        };
        let frame = result.and_then(|(bgra, stats)| rendered(bgra, stats));
        self.done.retain(|(o, _)| *o != owner);
        if self.done.len() >= DONE_KEPT {
            drop(self.done.remove(0));
        }
        self.done.push((owner, frame));
    }

    /// The frame read back for `owner`, if one is waiting.
    pub fn take(&mut self, owner: u64) -> Option<Result<Rendered, RenderError>> {
        let at = self.done.iter().position(|(o, _)| *o == owner)?;
        Some(self.done.remove(at).1)
    }

    /// Whether `owner` has a frame in flight.
    #[must_use]
    pub fn is_in_flight_for(&self, owner: u64) -> bool {
        self.in_flight == Some(owner)
    }
}

#[cfg(test)]
mod tests {
    use domain::sysmodel::scene::scene_of;
    use domain::sysmodel::{SysModel, scene};

    use super::*;

    fn model() -> SysModel {
        SysModel::from_value(serde_json::json!({
            "version": "crest.sysmodel/v1",
            "components": [
                { "id": "a", "kind": "client", "ports": [{ "id": "out", "dir": "out" }],
                  "machine": { "initial": "s", "states": ["s"] } },
                { "id": "b", "kind": "db", "ports": [{ "id": "in", "dir": "in" }],
                  "machine": { "initial": "s", "states": ["s"] } }
            ],
            "connections": [{ "id": "c", "from": "a.out", "to": "b.in" }]
        }))
        .unwrap()
    }

    /// Renders a frame when the machine has an adapter; skips otherwise
    /// (CI without a GPU).
    #[test]
    fn renders_a_frame_or_skips_without_an_adapter() {
        let mut renderer = match Renderer3d::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let m = model();
        let scene = scene_of(&m);
        let frame = scene::Frame {
            t_us: 0,
            packets: Vec::new(),
            chips: Vec::new(),
        };
        let camera = OrbitCamera::fit(&scene);
        let out = renderer
            .render(&scene, &frame, &camera, Some("a"), (320, 200))
            .unwrap();
        assert_eq!((out.stats.width, out.stats.height), (320, 200));
        let bytes = out.image.as_bytes(0).unwrap();
        assert_eq!(bytes.len(), 320 * 200 * 4);
        // Something besides the background was drawn in the middle.
        let center = (100 * 320 + 160) * 4;
        let corner = 0;
        assert_ne!(bytes[center..center + 3], bytes[corner..corner + 3]);
        eprintln!(
            "adapter {} ({}x msaa): gpu {:.2} ms, readback {:.2} ms",
            renderer.adapter(),
            renderer.samples(),
            out.stats.gpu_ms,
            out.stats.readback_ms
        );
    }

    /// A model mesh renders through the same passes; the second frame of
    /// the same key reuses the uploaded buffers. Skips without an adapter.
    #[test]
    fn renders_a_model_or_skips_without_an_adapter() {
        use domain::model3d::{ModelMesh, UpAxis};
        let mut renderer = match Renderer3d::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let model = ModelMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            up: UpAxis::Z,
        };
        let (mesh, (min, max)) = mesh::Mesh::from_model(&model, UpAxis::Z, [0.6; 3]);
        let mut camera = OrbitCamera::fit_bounds(min, max);
        for _ in 0..2 {
            let out = renderer
                .render_mesh(&mesh, Some(7), &camera, (200, 160))
                .unwrap();
            let bytes = out.image.as_bytes(0).unwrap();
            let lit = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[..3] != bytes[..3])
                .count();
            // The corner is background; a good share of the frame is model.
            assert!(lit > 200 * 160 / 20, "{lit} pixels drawn");
            camera.orbit(30.0, 0.0);
        }
    }

    /// A frame started for an owner is read back for it alone, without
    /// blocking; a second start waits for the first. Skips without an
    /// adapter.
    #[test]
    fn frames_in_flight_reach_their_owner_or_skips_without_an_adapter() {
        use domain::model3d::{ModelMesh, UpAxis};
        let mut renderer = match Renderer3d::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let model = ModelMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            up: UpAxis::Z,
        };
        let (mesh, (min, max)) = mesh::Mesh::from_model(&model, UpAxis::Z, [0.6; 3]);
        let camera = OrbitCamera::fit_bounds(min, max);
        let (a, b) = (next_owner(), next_owner());
        assert!(
            renderer
                .start_mesh(a, &mesh, Some(1), &camera, (64, 48))
                .unwrap()
        );
        assert!(renderer.is_in_flight_for(a));
        assert!(
            !renderer
                .start_mesh(b, &mesh, Some(1), &camera, (64, 48))
                .unwrap(),
            "one frame at a time"
        );
        let start = std::time::Instant::now();
        while renderer.is_busy() && start.elapsed() < std::time::Duration::from_secs(10) {
            renderer.poll();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(renderer.take(b).is_none());
        let frame = renderer.take(a).unwrap().unwrap();
        assert_eq!((frame.stats.width, frame.stats.height), (64, 48));
        assert!(renderer.take(a).is_none(), "taken once");
        // A blocking render still works after it.
        renderer
            .render_mesh(&mesh, Some(1), &camera, (32, 32))
            .unwrap();
    }

    #[test]
    fn images_need_the_right_byte_count() {
        assert!(render_image(vec![0; 16], 2, 2).is_some());
        assert!(render_image(vec![0; 15], 2, 2).is_none());
    }
}
