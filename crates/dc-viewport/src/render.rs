//! GPU side of the viewport: pipelines, per-mesh buffers, offscreen targets
//! and the egui paint callback that records the 3D pass.
//!
//! The scene is drawn into an offscreen colour + depth target (with MSAA when
//! the adapter supports it) during [`egui_wgpu::CallbackTrait::prepare`], in
//! egui's command encoder, before egui's own render pass. The resolved image
//! is registered as an egui native texture and shown with the painter, so the
//! viewport works whatever depth/MSAA settings the host app gives egui.

use std::sync::{Arc, OnceLock};

use bytemuck::{Pod, Zeroable};
use dc_mesh::TriMesh;
use dc_scene::{Color, LabelPalette, MeshColoring};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use glam::{Mat4, Vec3};

pub(crate) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// View format egui samples: egui expects gamma-encoded ("non sRGB-aware") textures.
const SAMPLE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Selection mask: red where the selected mesh is the front surface, green
/// its whole silhouette.
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
pub(crate) const PALETTE_SIZE: usize = 32;
/// Widest selection outline (pixels).
const MAX_OUTLINE_PX: f32 = 8.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct CameraUniform {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub params: [f32; 4],
    pub wire_color: [f32; 4],
    pub back_color: [f32; 4],
    /// Tint of the mesh under the pointer (linear rgb, a = strength).
    pub hover_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshUniform {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
    /// x: 1 under the pointer.
    pub extra: [f32; 4],
    pub palette: [[f32; 4]; PALETTE_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct OutlineUniform {
    /// Linear rgb, a = opacity.
    pub color: [f32; 4],
    /// x: radius (pixels), y: strength around the hidden parts.
    pub params: [f32; 4],
}

/// Colour mode values understood by the shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorMode {
    Uniform = 0,
    PerVertex = 1,
    Labels = 2,
}

/// Per-vertex `u32` attribute and shader parameters derived from a colouring.
pub(crate) struct ColorData {
    pub mode: ColorMode,
    pub base: Color,
    pub attr: Vec<u32>,
    pub palette: [[f32; 4]; PALETTE_SIZE],
    pub palette_len: u32,
}

fn srgb_byte(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn palette_array(p: &LabelPalette) -> ([[f32; 4]; PALETTE_SIZE], u32) {
    let mut out = [[0.0; 4]; PALETTE_SIZE];
    out[0] = p.unlabeled.to_array();
    let n = p.colors.len().min(PALETTE_SIZE - 1);
    for (dst, c) in out[1..].iter_mut().zip(&p.colors) {
        *dst = c.to_array();
    }
    (out, n as u32)
}

impl ColorData {
    /// Builds the vertex attribute for `coloring`; falls back to a uniform
    /// colour when per-vertex data is missing or has the wrong length.
    pub fn new(mesh: &TriMesh, coloring: &MeshColoring) -> Self {
        let nv = mesh.positions.len();
        let uniform = |base: Color| Self {
            mode: ColorMode::Uniform,
            base,
            attr: vec![0; nv],
            palette: [[0.0; 4]; PALETTE_SIZE],
            palette_len: 0,
        };
        let labels = |labels: &[u16], palette: &LabelPalette| {
            let (pal, n) = palette_array(palette);
            Self {
                mode: ColorMode::Labels,
                base: Color::WHITE,
                attr: labels.iter().map(|&l| u32::from(l)).collect(),
                palette: pal,
                palette_len: n,
            }
        };
        match coloring {
            MeshColoring::Uniform(c) => uniform(*c),
            MeshColoring::PerVertex(colors) if colors.len() == nv => Self {
                mode: ColorMode::PerVertex,
                base: Color::WHITE,
                attr: colors
                    .iter()
                    .map(|c| {
                        u32::from_le_bytes([
                            srgb_byte(c.r),
                            srgb_byte(c.g),
                            srgb_byte(c.b),
                            (c.a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        ])
                    })
                    .collect(),
                palette: [[0.0; 4]; PALETTE_SIZE],
                palette_len: 0,
            },
            MeshColoring::VertexLabels(p) => match &mesh.vertex_labels {
                Some(l) if l.len() == nv => labels(l, p),
                _ => uniform(p.unlabeled),
            },
            MeshColoring::Labels { labels: l, palette } if l.len() == nv => labels(l, palette),
            MeshColoring::PerVertex(_) | MeshColoring::Labels { .. } => uniform(Color::TOOTH),
        }
    }
}

/// Shared pipelines and layouts of one viewport.
pub(crate) struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub sample_count: u32,
    mesh_bgl: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    camera_bg: wgpu::BindGroup,
    opaque: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    wire: wgpu::RenderPipeline,
    /// The selected meshes where they are the front surface (mask red).
    mask_visible: wgpu::RenderPipeline,
    /// Their whole silhouette (mask green).
    mask_full: wgpu::RenderPipeline,
    /// The outline over the resolved image, from the mask.
    outline: wgpu::RenderPipeline,
    outline_bgl: wgpu::BindGroupLayout,
    outline_buf: wgpu::Buffer,
}

/// Geometry and per-mesh uniforms on the GPU.
pub(crate) struct GpuMesh {
    vertices: wgpu::Buffer,
    attr: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    edges: OnceLock<(wgpu::Buffer, u32)>,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Offscreen render targets.
pub(crate) struct Targets {
    pub size: [u32; 2],
    msaa: Option<wgpu::TextureView>,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
    pub sample_view: wgpu::TextureView,
    /// The selection mask, multisampled like the scene (resolved into `mask`).
    mask_msaa: Option<wgpu::TextureView>,
    mask: wgpu::TextureView,
    /// The mask and the outline uniform, for the outline pass.
    outline_bg: wgpu::BindGroup,
}

impl Gpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, adapter: &wgpu::Adapter) -> Self {
        let sample_count = [4u32, 2]
            .into_iter()
            .find(|&n| {
                let c = adapter.get_texture_format_features(COLOR_FORMAT);
                let d = adapter.get_texture_format_features(DEPTH_FORMAT);
                let m = adapter.get_texture_format_features(MASK_FORMAT);
                let resolves = wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE;
                c.flags.sample_count_supported(n)
                    && d.flags.sample_count_supported(n)
                    && m.flags.sample_count_supported(n)
                    && c.flags.contains(resolves)
                    && m.flags.contains(resolves)
            })
            .unwrap_or(1);

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dc_viewport_mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });
        let uniform_entry = |size: usize| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dc_viewport_camera"),
            entries: &[uniform_entry(size_of::<CameraUniform>())],
        });
        let mesh_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dc_viewport_mesh"),
            entries: &[uniform_entry(size_of::<MeshUniform>())],
        });
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dc_viewport_camera"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dc_viewport_camera"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dc_viewport"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&mesh_bgl)],
            immediate_size: 0,
        });
        let vertex_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 4,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![2 => Uint32],
            }),
        ];
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        topology: wgpu::PrimitiveTopology,
                        blend: Option<wgpu::BlendState>,
                        depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &vertex_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let tri = wgpu::PrimitiveTopology::TriangleList;
        // the selection mask: same meshes, uniforms and depth as the scene
        let mask = |label: &str, compare: wgpu::CompareFunction, write: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_mask"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &vertex_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: tri,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_mask"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: MASK_FORMAT,
                        blend: None,
                        write_mask: write,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let outline_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dc_viewport_outline"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });
        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dc_viewport_outline"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<OutlineUniform>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dc_viewport_outline"),
            bind_group_layouts: &[Some(&outline_bgl)],
            immediate_size: 0,
        });
        let outline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dc_viewport_outline"),
            layout: Some(&outline_layout),
            vertex: wgpu::VertexState {
                module: &outline_module,
                entry_point: Some("vs_full"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &outline_module,
                entry_point: Some("fs_outline"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: alpha,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let outline_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dc_viewport_outline"),
            size: size_of::<OutlineUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            opaque: pipeline("dc_viewport_opaque", "vs_main", "fs_main", tri, None, true),
            transparent: pipeline(
                "dc_viewport_transparent",
                "vs_main",
                "fs_main",
                tri,
                alpha,
                false,
            ),
            wire: pipeline(
                "dc_viewport_wire",
                "vs_wire",
                "fs_wire",
                wgpu::PrimitiveTopology::LineList,
                alpha,
                false,
            ),
            mask_visible: mask(
                "dc_viewport_mask_visible",
                wgpu::CompareFunction::LessEqual,
                wgpu::ColorWrites::RED,
            ),
            mask_full: mask(
                "dc_viewport_mask_full",
                wgpu::CompareFunction::Always,
                wgpu::ColorWrites::GREEN,
            ),
            outline,
            outline_bgl,
            outline_buf,
            device: device.clone(),
            queue: queue.clone(),
            sample_count,
            mesh_bgl,
            camera_buf,
            camera_bg,
        }
    }

    /// Uploads a mesh (positions, normals, colour attribute, indices).
    pub fn upload(&self, mesh: &TriMesh, colors: &ColorData) -> GpuMesh {
        let normals = match &mesh.normals {
            Some(n) if n.len() == mesh.positions.len() => None,
            _ => Some(mesh.compute_vertex_normals()),
        };
        let normals = normals
            .as_deref()
            .or(mesh.normals.as_deref())
            .unwrap_or(&[]);
        let mut interleaved = Vec::with_capacity(mesh.positions.len() * 6);
        for (p, n) in mesh.positions.iter().zip(normals) {
            interleaved.extend_from_slice(&[p.x, p.y, p.z, n.x, n.y, n.z]);
        }
        let nv = mesh.positions.len() as u32;
        let indices: Vec<u32> = mesh
            .tris
            .iter()
            .filter(|t| t.iter().all(|&i| i < nv))
            .flatten()
            .copied()
            .collect();
        let vertices = self.buffer_init(
            "dc_viewport_vertices",
            &interleaved,
            wgpu::BufferUsages::VERTEX,
        );
        let attr = self.buffer_init(
            "dc_viewport_attr",
            &colors.attr,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let index_buf =
            self.buffer_init("dc_viewport_indices", &indices, wgpu::BufferUsages::INDEX);
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dc_viewport_mesh_uniform"),
            size: size_of::<MeshUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dc_viewport_mesh"),
            layout: &self.mesh_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        GpuMesh {
            vertices,
            attr,
            indices: index_buf,
            index_count: indices.len() as u32,
            edges: OnceLock::new(),
            uniform,
            bind_group,
        }
    }

    /// Replaces the colour attribute of an uploaded mesh (same vertex count).
    pub fn update_colors(&self, gpu: &GpuMesh, colors: &ColorData) {
        if !colors.attr.is_empty() {
            self.queue
                .write_buffer(&gpu.attr, 0, bytemuck::cast_slice(&colors.attr));
        }
    }

    /// Builds the unique-edge index buffer used by the wireframe overlay.
    pub fn ensure_edges(&self, gpu: &GpuMesh, mesh: &TriMesh) {
        if gpu.edges.get().is_some() {
            return;
        }
        let nv = mesh.positions.len() as u32;
        let mut keys: Vec<u64> = Vec::with_capacity(mesh.tris.len() * 3);
        for t in mesh.tris.iter().filter(|t| t.iter().all(|&i| i < nv)) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                keys.push((u64::from(lo) << 32) | u64::from(hi));
            }
        }
        keys.sort_unstable();
        keys.dedup();
        let idx: Vec<u32> = keys
            .iter()
            .flat_map(|k| [(k >> 32) as u32, *k as u32])
            .collect();
        let buf = self.buffer_init("dc_viewport_edges", &idx, wgpu::BufferUsages::INDEX);
        let _ = gpu.edges.set((buf, idx.len() as u32));
    }

    fn buffer_init<T: Pod>(
        &self,
        label: &str,
        data: &[T],
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        // zero-sized buffers are not allowed as vertex/index bindings: pad
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let padded;
        let contents = if bytes.is_empty() {
            padded = [0u8; 16];
            &padded[..]
        } else {
            bytes
        };
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
    }

    /// Creates offscreen targets of `size` pixels.
    pub fn targets(&self, size: [u32; 2]) -> Targets {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let tex = |label, format, samples, usage, view_formats: &[wgpu::TextureFormat]| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats,
            })
        };
        let resolve = tex(
            "dc_viewport_color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            &[SAMPLE_FORMAT],
        );
        let msaa = (self.sample_count > 1).then(|| {
            tex(
                "dc_viewport_color_msaa",
                COLOR_FORMAT,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                &[],
            )
            .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let depth = tex(
            "dc_viewport_depth",
            DEPTH_FORMAT,
            self.sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            &[],
        );
        let mask = tex(
            "dc_viewport_mask",
            MASK_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            &[],
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let mask_msaa = (self.sample_count > 1).then(|| {
            tex(
                "dc_viewport_mask_msaa",
                MASK_FORMAT,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                &[],
            )
            .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let outline_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dc_viewport_outline"),
            layout: &self.outline_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.outline_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
            ],
        });
        Targets {
            size,
            msaa,
            sample_view: resolve.create_view(&wgpu::TextureViewDescriptor {
                format: Some(SAMPLE_FORMAT),
                ..Default::default()
            }),
            resolve: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            mask_msaa,
            mask,
            outline_bg,
        }
    }
}

/// One mesh to draw this frame.
pub(crate) struct DrawItem {
    pub mesh: Arc<GpuMesh>,
    pub uniform: MeshUniform,
    pub transparent: bool,
    pub wireframe: bool,
    /// View-space depth of the mesh centre (sorting of transparent meshes).
    pub depth: f32,
}

/// Builds the per-mesh uniform; `hovered` tints it with the hover colour.
pub(crate) fn mesh_uniform(
    model: Mat4,
    view: Mat4,
    colors: &ColorData,
    opacity: f32,
    backface: u32,
    hovered: bool,
) -> MeshUniform {
    let normal_mat = (view * model).inverse().transpose();
    let base = colors.base;
    MeshUniform {
        model: model.to_cols_array_2d(),
        normal_mat: normal_mat.to_cols_array_2d(),
        color: base.to_array(),
        params: [
            colors.mode as u32 as f32,
            backface as f32,
            opacity.clamp(0.0, 1.0),
            colors.palette_len as f32,
        ],
        extra: [f32::from(u8::from(hovered)), 0.0, 0.0, 0.0],
        palette: colors.palette,
    }
}

/// The selection outline of a frame.
pub(crate) struct OutlinePass {
    /// The selected meshes (their uniforms are written by the scene pass).
    pub meshes: Vec<Arc<GpuMesh>>,
    pub uniform: OutlineUniform,
    /// Pixels the outline can touch: `[x, y, width, height]`.
    pub scissor: [u32; 4],
}

impl OutlinePass {
    /// The outline of `meshes` in `color` (linear), `width_px` wide, at
    /// `hidden` strength around the parts other meshes cover; `bbox` is the
    /// pixel box of the meshes (`None`: the whole target of `size`).
    pub fn new(
        meshes: Vec<Arc<GpuMesh>>,
        color: Color,
        width_px: f32,
        hidden: f32,
        bbox: Option<[f32; 4]>,
        size: [u32; 2],
    ) -> Self {
        let r = width_px.clamp(1.0, MAX_OUTLINE_PX);
        Self {
            meshes,
            uniform: OutlineUniform {
                color: color.to_array(),
                params: [r, hidden.clamp(0.0, 1.0), 0.0, 0.0],
            },
            scissor: scissor(bbox, r + 2.0, size),
        }
    }
}

/// `[x, y, width, height]` of `bbox` (`[min x, min y, max x, max y]`,
/// pixels) grown by `pad` and clipped to a target of `size`; the whole target
/// without a box.
pub(crate) fn scissor(bbox: Option<[f32; 4]>, pad: f32, size: [u32; 2]) -> [u32; 4] {
    let (w, h) = (size[0] as f32, size[1] as f32);
    let [x0, y0, x1, y1] = bbox.unwrap_or([0.0, 0.0, w, h]);
    let x0 = (x0 - pad).floor().clamp(0.0, w);
    let y0 = (y0 - pad).floor().clamp(0.0, h);
    let x1 = (x1 + pad).ceil().clamp(0.0, w);
    let y1 = (y1 + pad).ceil().clamp(0.0, h);
    [
        x0 as u32,
        y0 as u32,
        (x1 - x0).max(0.0) as u32,
        (y1 - y0).max(0.0) as u32,
    ]
}

/// Everything the paint callback needs to render one frame.
pub(crate) struct FrameCallback {
    pub gpu: Arc<Gpu>,
    pub targets: Arc<Targets>,
    pub camera: CameraUniform,
    pub clear: Color,
    pub items: Vec<DrawItem>,
    /// The selection outline, if something is selected and shown.
    pub outline: Option<OutlinePass>,
}

impl FrameCallback {
    fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let gpu = &self.gpu;
        gpu.queue
            .write_buffer(&gpu.camera_buf, 0, bytemuck::bytes_of(&self.camera));
        for it in &self.items {
            gpu.queue
                .write_buffer(&it.mesh.uniform, 0, bytemuck::bytes_of(&it.uniform));
        }
        let t = &self.targets;
        let (view, resolve_target) = match &t.msaa {
            Some(m) => (m, Some(&t.resolve)),
            None => (&t.resolve, None),
        };
        let c = self.clear;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dc_viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(c.r),
                        g: f64::from(c.g),
                        b: f64::from(c.b),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    // the selection mask tests it
                    store: if self.outline.is_some() {
                        wgpu::StoreOp::Store
                    } else {
                        wgpu::StoreOp::Discard
                    },
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &gpu.camera_bg, &[]);
        let draw = |pass: &mut wgpu::RenderPass<'_>, it: &DrawItem| {
            pass.set_bind_group(1, &it.mesh.bind_group, &[]);
            pass.set_vertex_buffer(0, it.mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, it.mesh.attr.slice(..));
            pass.set_index_buffer(it.mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..it.mesh.index_count, 0, 0..1);
        };
        let draw_wire = |pass: &mut wgpu::RenderPass<'_>, it: &DrawItem| {
            if let (true, Some((edges, n))) = (it.wireframe, it.mesh.edges.get()) {
                pass.set_bind_group(1, &it.mesh.bind_group, &[]);
                pass.set_vertex_buffer(0, it.mesh.vertices.slice(..));
                pass.set_vertex_buffer(1, it.mesh.attr.slice(..));
                pass.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*n, 0, 0..1);
            }
        };
        pass.set_pipeline(&gpu.opaque);
        for it in self
            .items
            .iter()
            .filter(|i| !i.transparent && i.mesh.index_count > 0)
        {
            draw(&mut pass, it);
        }
        pass.set_pipeline(&gpu.wire);
        for it in self.items.iter().filter(|i| !i.transparent) {
            draw_wire(&mut pass, it);
        }
        // transparent meshes back to front (more negative view z = farther)
        let mut order: Vec<&DrawItem> = self
            .items
            .iter()
            .filter(|i| i.transparent && i.mesh.index_count > 0)
            .collect();
        order.sort_by(|a, b| a.depth.total_cmp(&b.depth));
        for it in order {
            pass.set_pipeline(&gpu.transparent);
            draw(&mut pass, it);
            pass.set_pipeline(&gpu.wire);
            draw_wire(&mut pass, it);
        }
        drop(pass);
        if let Some(o) = &self.outline {
            self.record_outline(encoder, o);
        }
    }

    /// The selection mask of the selected meshes against the depth of the
    /// scene, then the outline over the resolved image.
    fn record_outline(&self, encoder: &mut wgpu::CommandEncoder, o: &OutlinePass) {
        let gpu = &self.gpu;
        let t = &self.targets;
        if o.meshes.is_empty() || o.scissor[2] == 0 || o.scissor[3] == 0 {
            return;
        }
        let (view, resolve_target) = match &t.mask_msaa {
            Some(m) => (m, Some(&t.mask)),
            None => (&t.mask, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dc_viewport_mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &gpu.camera_bg, &[]);
        for pipeline in [&gpu.mask_visible, &gpu.mask_full] {
            pass.set_pipeline(pipeline);
            for m in o.meshes.iter().filter(|m| m.index_count > 0) {
                pass.set_bind_group(1, &m.bind_group, &[]);
                pass.set_vertex_buffer(0, m.vertices.slice(..));
                pass.set_vertex_buffer(1, m.attr.slice(..));
                pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..m.index_count, 0, 0..1);
            }
        }
        drop(pass);
        gpu.queue
            .write_buffer(&gpu.outline_buf, 0, bytemuck::bytes_of(&o.uniform));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dc_viewport_outline"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.resolve,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let [x, y, w, h] = o.scissor;
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&gpu.outline);
        pass.set_bind_group(0, &t.outline_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl egui_wgpu::CallbackTrait for FrameCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.record(egui_encoder);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
        // the image is drawn by egui from the offscreen texture
    }
}

/// World-space bounding box of a local box under `xform`.
pub(crate) fn transform_box(min: Vec3, max: Vec3, xform: Mat4) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for i in 0..8 {
        let p = Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        );
        let q = xform.transform_point3(p);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layouts_are_16_byte_aligned() {
        assert_eq!(size_of::<CameraUniform>() % 16, 0);
        assert_eq!(size_of::<MeshUniform>() % 16, 0);
        assert_eq!(size_of::<OutlineUniform>() % 16, 0);
        assert_eq!(size_of::<CameraUniform>(), 64 * 2 + 16 * 4);
        assert_eq!(
            size_of::<MeshUniform>(),
            64 * 2 + 16 * 3 + 16 * PALETTE_SIZE
        );
    }

    #[test]
    fn the_outline_touches_the_box_of_the_meshes_and_its_width_only() {
        // a box grown by the pad, inside the target
        assert_eq!(
            scissor(Some([100.0, 50.0, 200.0, 80.0]), 5.0, [800, 600]),
            [95, 45, 110, 40]
        );
        // clipped at the edges; without a box, the whole target
        assert_eq!(
            scissor(Some([-30.0, 590.0, 10.0, 700.0]), 5.0, [800, 600]),
            [0, 585, 15, 15]
        );
        assert_eq!(scissor(None, 5.0, [800, 600]), [0, 0, 800, 600]);
        // off the target: nothing to draw
        assert_eq!(
            scissor(Some([900.0, 0.0, 950.0, 10.0]), 5.0, [800, 600])[2],
            0
        );
        // the width is kept to a sane range
        let o = OutlinePass::new(Vec::new(), Color::WHITE, 40.0, 2.0, None, [10, 10]);
        assert_eq!(o.uniform.params[0], MAX_OUTLINE_PX);
        assert_eq!(o.uniform.params[1], 1.0);
    }

    #[test]
    fn color_data_modes_and_fallbacks() {
        let mut m = TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]);
        let d = ColorData::new(&m, &MeshColoring::Uniform(Color::TOOTH));
        assert_eq!(d.mode, ColorMode::Uniform);
        assert_eq!(d.attr.len(), 3);
        // vertex labels missing: uniform with the unlabeled colour
        let d = ColorData::new(&m, &MeshColoring::VertexLabels(LabelPalette::default()));
        assert_eq!(d.mode, ColorMode::Uniform);
        m.vertex_labels = Some(vec![0, 11, 31]);
        let d = ColorData::new(&m, &MeshColoring::VertexLabels(LabelPalette::default()));
        assert_eq!(d.mode, ColorMode::Labels);
        assert_eq!(d.attr, vec![0, 11, 31]);
        assert_eq!(d.palette_len, 12);
        assert_eq!(d.palette[0], Color::GINGIVA.to_array());
        let colors = Arc::new(vec![Color::rgb(1.0, 0.0, 0.0); 3]);
        let d = ColorData::new(&m, &MeshColoring::PerVertex(colors));
        assert_eq!(d.mode, ColorMode::PerVertex);
        assert_eq!(d.attr[0].to_le_bytes(), [255, 0, 0, 255]);
        // wrong length: uniform fallback
        let d = ColorData::new(&m, &MeshColoring::PerVertex(Arc::new(vec![Color::WHITE])));
        assert_eq!(d.mode, ColorMode::Uniform);
    }

    #[test]
    fn shaders_parse_and_validate() {
        use wgpu::naga;
        for (source, entries) in [
            (
                include_str!("mesh.wgsl"),
                &[
                    "vs_main", "fs_main", "vs_wire", "fs_wire", "vs_mask", "fs_mask",
                ][..],
            ),
            (include_str!("outline.wgsl"), &["vs_full", "fs_outline"][..]),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .expect("valid shader");
            for entry in entries {
                assert!(
                    module.entry_points.iter().any(|e| e.name == *entry),
                    "{entry}"
                );
            }
        }
    }

    #[test]
    fn srgb_encoding() {
        assert_eq!(srgb_byte(0.0), 0);
        assert_eq!(srgb_byte(1.0), 255);
        assert_eq!(srgb_byte(0.214_041_14), 128);
    }

    #[test]
    fn transformed_box_contains_rotated_corners() {
        let x =
            Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2) * Mat4::from_translation(Vec3::X);
        let (lo, hi) = transform_box(Vec3::ZERO, Vec3::ONE, x);
        assert!((lo - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-5);
        assert!((hi - Vec3::new(0.0, 2.0, 1.0)).length() < 1e-5);
    }
}
