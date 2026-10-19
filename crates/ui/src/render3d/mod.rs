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
//!   the labels GPUI draws on top, and rays for [`pick`]ing on the CPU.
//!
//! [`Renderer3d::new`] fails cleanly when there is no adapter; the board
//! then falls back to its 2.5D vector drawing.

pub mod camera;
pub mod gpu;
pub mod math;
pub mod mesh;
pub mod pick;

use std::sync::Arc;

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

/// The renderer: one device for the whole app.
pub struct Renderer3d {
    gpu: gpu::Gpu,
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

impl Renderer3d {
    /// Opens a device.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoAdapter`] or [`RenderError::Device`].
    pub fn new() -> Result<Self, RenderError> {
        Ok(Self {
            gpu: gpu::Gpu::new()?,
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
        let aspect = size.0.max(1) as f32 / size.1.max(1) as f32;
        let uniform = gpu::CameraUniform {
            view_proj: camera.view_projection(aspect),
            eye: camera.eye(),
            light: math::normalize([0.45, 1.0, 0.65]),
            inv_far: 1.0 / (camera.radius * 12.0),
        };
        let (bgra, stats) = self.gpu.render(mesh, key, &uniform, size.0, size.1)?;
        let image = render_image(bgra, stats.width, stats.height)
            .ok_or_else(|| RenderError::Render("readback size mismatch".into()))?;
        Ok(Rendered { image, stats })
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

    #[test]
    fn images_need_the_right_byte_count() {
        assert!(render_image(vec![0; 16], 2, 2).is_some());
        assert!(render_image(vec![0; 15], 2, 2).is_none());
    }
}
