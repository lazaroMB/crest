//! The wgpu side: an own device (not GPUI's), offscreen targets per size,
//! the passes of `board.wgsl` and the readback of the final image as BGRA
//! bytes for a GPUI `RenderImage`.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use wgpu::util::DeviceExt as _;

use super::mesh::{FLOATS_PER_VERTEX, Mesh};
use super::{RenderError, RenderStats};

const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The final image: BGRA, as GPUI's images are, sRGB-encoded by the GPU.
const OUT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
/// The background around the board: the TwoCrest ink (`#0b0b0c`), linear.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.0033,
    g: 0.0033,
    b: 0.0037,
    a: 1.0,
};

/// What the scene pass needs per frame.
#[derive(Debug, Clone, Copy)]
pub struct CameraUniform {
    /// World to clip.
    pub view_proj: [[f32; 4]; 4],
    /// Camera position.
    pub eye: [f32; 3],
    /// Direction towards the key light.
    pub light: [f32; 3],
    /// `1 / far` for the stored depth.
    pub inv_far: f32,
}

impl CameraUniform {
    fn bytes(&self) -> Vec<u8> {
        let mut f: Vec<f32> = self.view_proj.iter().flatten().copied().collect();
        f.extend_from_slice(&[self.eye[0], self.eye[1], self.eye[2], 1.0]);
        f.extend_from_slice(&[self.light[0], self.light[1], self.light[2], 0.0]);
        f.extend_from_slice(&[self.inv_far, 0.7, 0.0, 0.0]);
        bytemuck::cast_slice(&f).to_vec()
    }
}

/// Offscreen targets of one size.
struct Targets {
    width: u32,
    height: u32,
    samples: u32,
    msaa_color: Option<wgpu::TextureView>,
    msaa_nd: Option<wgpu::TextureView>,
    hdr: wgpu::TextureView,
    nd: wgpu::TextureView,
    depth: wgpu::TextureView,
    lit: wgpu::TextureView,
    bloom_a: wgpu::TextureView,
    bloom_b: wgpu::TextureView,
    out: wgpu::Texture,
    out_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_row: u32,
    edges: wgpu::BindGroup,
    bright: wgpu::BindGroup,
    blur_h: wgpu::BindGroup,
    blur_v: wgpu::BindGroup,
    composite: wgpu::BindGroup,
}

/// The device, its pipelines and the targets of the last size.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `name (backend)` of the adapter.
    pub adapter: String,
    samples: u32,
    scene_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    post_layout: wgpu::BindGroupLayout,
    edges_pipeline: wgpu::RenderPipeline,
    bright_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    vertex_buffer: Option<(wgpu::Buffer, u64)>,
    index_buffer: Option<(wgpu::Buffer, u64)>,
    /// Key of the mesh in the buffers, when the caller gave one.
    uploaded: Option<u64>,
    targets: Option<Targets>,
    error: Arc<Mutex<Option<String>>>,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    (width, height): (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

fn full_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_full"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn supports_msaa(adapter: &wgpu::Adapter) -> bool {
    [HDR, DEPTH].iter().all(|f| {
        let flags = adapter.get_texture_format_features(*f).flags;
        flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4)
    }) && adapter
        .get_texture_format_features(HDR)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
}

impl Gpu {
    /// Opens an adapter and a device of its own.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoAdapter`] when there is no usable GPU (or software)
    /// adapter, [`RenderError::Device`] when the device cannot be created.
    pub fn new() -> Result<Self, RenderError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            futures::executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            }))
            .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) =
            futures::executor::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("crest board"),
                required_limits:
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..wgpu::DeviceDescriptor::default()
            }))
            .map_err(|e| RenderError::Device(e.to_string()))?;
        let error = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&error);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            *sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.to_string());
        }));
        let samples = if supports_msaa(&adapter) { 4 } else { 1 };

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("board.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("board.wgsl"))),
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: 112,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let stride = (FLOATS_PER_VERTEX * 4) as u64;
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3];
        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_scene"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                }],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..wgpu::MultisampleState::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_scene"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: HDR,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: HDR,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
            }),
            multiview_mask: None,
            cache: None,
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let edges_pipeline =
            full_pipeline(&device, &module, &post_pipeline_layout, "fs_edges", HDR);
        let bright_pipeline =
            full_pipeline(&device, &module, &post_pipeline_layout, "fs_bright", HDR);
        let blur_pipeline = full_pipeline(&device, &module, &post_pipeline_layout, "fs_blur", HDR);
        let composite_pipeline =
            full_pipeline(&device, &module, &post_pipeline_layout, "fs_composite", OUT);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let gpu = Self {
            adapter: format!("{} ({:?})", info.name, info.backend),
            device,
            queue,
            samples,
            scene_pipeline,
            camera_buffer,
            camera_group,
            post_layout,
            edges_pipeline,
            bright_pipeline,
            blur_pipeline,
            composite_pipeline,
            sampler,
            vertex_buffer: None,
            index_buffer: None,
            uploaded: None,
            targets: None,
            error,
        };
        gpu.take_error()?;
        Ok(gpu)
    }

    /// MSAA samples of the scene pass.
    #[must_use]
    pub fn samples(&self) -> u32 {
        self.samples
    }

    fn take_error(&self) -> Result<(), RenderError> {
        match self
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(e) => Err(RenderError::Render(e)),
            None => Ok(()),
        }
    }

    fn post_group(
        &self,
        label: &str,
        t0: &wgpu::TextureView,
        t1: &wgpu::TextureView,
        uniform: [f32; 8],
    ) -> wgpu::BindGroup {
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(t0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(t1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn targets(&mut self, width: u32, height: u32) -> &Targets {
        let fits = self
            .targets
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height && t.samples == self.samples);
        if !fits {
            let d = &self.device;
            let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
            let sampled = attach | wgpu::TextureUsages::TEXTURE_BINDING;
            let size = (width, height);
            let half = (width.div_ceil(2).max(1), height.div_ceil(2).max(1));
            let msaa = self.samples > 1;
            let msaa_color =
                msaa.then(|| view(&texture(d, "msaa color", size, HDR, self.samples, attach)));
            let msaa_nd =
                msaa.then(|| view(&texture(d, "msaa nd", size, HDR, self.samples, attach)));
            let hdr = view(&texture(d, "hdr", size, HDR, 1, sampled));
            let nd = view(&texture(d, "nd", size, HDR, 1, sampled));
            let depth = view(&texture(d, "depth", size, DEPTH, self.samples, attach));
            let lit = view(&texture(d, "lit", size, HDR, 1, sampled));
            let bloom_a = view(&texture(d, "bloom a", half, HDR, 1, sampled));
            let bloom_b = view(&texture(d, "bloom b", half, HDR, 1, sampled));
            let out = texture(
                d,
                "out",
                size,
                OUT,
                1,
                attach | wgpu::TextureUsages::COPY_SRC,
            );
            let out_view = view(&out);
            let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let readback = d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: u64::from(padded_row) * u64::from(height.max(1)),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let texel = [1.0 / width.max(1) as f32, 1.0 / height.max(1) as f32];
            let half_texel = [1.0 / half.0 as f32, 1.0 / half.1 as f32];
            let edges = self.post_group(
                "edges",
                &hdr,
                &nd,
                [texel[0], texel[1], 0.0, 0.0, 0.0, 0.0, 1.0, 1.0],
            );
            let bright = self.post_group(
                "bright",
                &lit,
                &lit,
                [texel[0], texel[1], 0.0, 0.0, 0.9, 0.0, 0.0, 1.0],
            );
            let blur_h = self.post_group(
                "blur h",
                &bloom_a,
                &bloom_a,
                [half_texel[0], half_texel[1], 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            );
            let blur_v = self.post_group(
                "blur v",
                &bloom_b,
                &bloom_b,
                [half_texel[0], half_texel[1], 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            );
            let composite = self.post_group(
                "composite",
                &lit,
                &bloom_a,
                [texel[0], texel[1], 0.0, 0.0, 0.0, 0.7, 0.0, 1.0],
            );
            self.targets = Some(Targets {
                width,
                height,
                samples: self.samples,
                msaa_color,
                msaa_nd,
                hdr,
                nd,
                depth,
                lit,
                bloom_a,
                bloom_b,
                out,
                out_view,
                readback,
                padded_row,
                edges,
                bright,
                blur_h,
                blur_v,
                composite,
            });
        }
        self.targets
            .as_ref()
            .unwrap_or_else(|| unreachable!("set above"))
    }

    /// Writes `mesh` to the buffers, unless they already hold the mesh of
    /// `key` (a large model is not uploaded again for every camera move).
    fn upload(&mut self, mesh: &Mesh, key: Option<u64>) {
        if key.is_some() && key == self.uploaded {
            return;
        }
        self.uploaded = key;
        let vbytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let ibytes: &[u8] = bytemuck::cast_slice(&mesh.indices);
        let grow =
            |slot: &mut Option<(wgpu::Buffer, u64)>, bytes: &[u8], usage, device: &wgpu::Device| {
                let need = (bytes.len() as u64).max(16).next_power_of_two();
                if slot.as_ref().is_none_or(|(_, cap)| *cap < need) {
                    *slot = Some((
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("mesh"),
                            size: need,
                            usage: usage | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        need,
                    ));
                }
            };
        grow(
            &mut self.vertex_buffer,
            vbytes,
            wgpu::BufferUsages::VERTEX,
            &self.device,
        );
        grow(
            &mut self.index_buffer,
            ibytes,
            wgpu::BufferUsages::INDEX,
            &self.device,
        );
        if let (Some((vb, _)), Some((ib, _))) = (&self.vertex_buffer, &self.index_buffer) {
            if !vbytes.is_empty() {
                self.queue.write_buffer(vb, 0, vbytes);
            }
            if !ibytes.is_empty() {
                self.queue.write_buffer(ib, 0, ibytes);
            }
        }
    }

    /// Renders `mesh` seen through `camera` at `width × height` and reads
    /// the BGRA bytes back. With a `key`, a mesh already uploaded under it
    /// is not uploaded again.
    ///
    /// # Errors
    ///
    /// [`RenderError::Render`] for a wgpu error or a failed readback.
    pub fn render(
        &mut self,
        mesh: &Mesh,
        key: Option<u64>,
        camera: &CameraUniform,
        width: u32,
        height: u32,
    ) -> Result<(Vec<u8>, RenderStats), RenderError> {
        let started = Instant::now();
        let (width, height) = (width.clamp(1, 8192), height.clamp(1, 8192));
        self.upload(mesh, key);
        self.queue
            .write_buffer(&self.camera_buffer, 0, &camera.bytes());
        self.targets(width, height);
        let t = self
            .targets
            .as_ref()
            .unwrap_or_else(|| unreachable!("created above"));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("board"),
            });
        {
            let (color_view, color_resolve) = match &t.msaa_color {
                Some(msaa) => (msaa, Some(&t.hdr)),
                None => (&t.hdr, None),
            };
            let (nd_view, nd_resolve) = match &t.msaa_nd {
                Some(msaa) => (msaa, Some(&t.nd)),
                None => (&t.nd, None),
            };
            let store = if t.msaa_color.is_some() {
                wgpu::StoreOp::Discard
            } else {
                wgpu::StoreOp::Store
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        depth_slice: None,
                        resolve_target: color_resolve,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR),
                            store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: nd_view,
                        depth_slice: None,
                        resolve_target: nd_resolve,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let (Some((vb, _)), Some((ib, _))) = (&self.vertex_buffer, &self.index_buffer)
                && !mesh.indices.is_empty()
            {
                pass.set_pipeline(&self.scene_pipeline);
                pass.set_bind_group(0, &self.camera_group, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                let count = u32::try_from(mesh.indices.len()).unwrap_or(u32::MAX);
                pass.draw_indexed(0..count, 0, 0..1);
            }
        }
        let passes: [(
            &str,
            &wgpu::RenderPipeline,
            &wgpu::BindGroup,
            &wgpu::TextureView,
        ); 5] = [
            ("edges", &self.edges_pipeline, &t.edges, &t.lit),
            ("bright", &self.bright_pipeline, &t.bright, &t.bloom_a),
            ("blur h", &self.blur_pipeline, &t.blur_h, &t.bloom_b),
            ("blur v", &self.blur_pipeline, &t.blur_v, &t.bloom_a),
            (
                "composite",
                &self.composite_pipeline,
                &t.composite,
                &t.out_view,
            ),
        ];
        for (label, pipeline, group, target) in passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &t.out,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(t.padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        t.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                tx.send(result).ok();
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Render(e.to_string()))?;
        let gpu_done = Instant::now();
        rx.recv()
            .map_err(|e| RenderError::Render(e.to_string()))?
            .map_err(|e| RenderError::Render(e.to_string()))?;
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        {
            let data = t.readback.get_mapped_range(..);
            for row in data.chunks(t.padded_row as usize).take(height as usize) {
                bgra.extend_from_slice(&row[..(width * 4) as usize]);
            }
        }
        t.readback.unmap();
        self.take_error()?;
        let done = Instant::now();
        Ok((
            bgra,
            RenderStats {
                width,
                height,
                vertices: mesh.vertex_count(),
                gpu_ms: (gpu_done - started).as_secs_f32() * 1000.0,
                readback_ms: (done - gpu_done).as_secs_f32() * 1000.0,
            },
        ))
    }
}
