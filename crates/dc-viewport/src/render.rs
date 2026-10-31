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
pub(crate) const PALETTE_SIZE: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct CameraUniform {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub params: [f32; 4],
    pub wire_color: [f32; 4],
    pub back_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshUniform {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
    pub palette: [[f32; 4]; PALETTE_SIZE],
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
}

impl Gpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, adapter: &wgpu::Adapter) -> Self {
        let sample_count = [4u32, 2]
            .into_iter()
            .find(|&n| {
                let c = adapter.get_texture_format_features(COLOR_FORMAT);
                let d = adapter.get_texture_format_features(DEPTH_FORMAT);
                c.flags.sample_count_supported(n)
                    && d.flags.sample_count_supported(n)
                    && c.flags
                        .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
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
        Targets {
            size,
            msaa,
            sample_view: resolve.create_view(&wgpu::TextureViewDescriptor {
                format: Some(SAMPLE_FORMAT),
                ..Default::default()
            }),
            resolve: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
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

/// Builds the per-mesh uniform.
pub(crate) fn mesh_uniform(
    model: Mat4,
    view: Mat4,
    colors: &ColorData,
    opacity: f32,
    backface: u32,
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
        palette: colors.palette,
    }
}

/// Everything the paint callback needs to render one frame.
pub(crate) struct FrameCallback {
    pub gpu: Arc<Gpu>,
    pub targets: Arc<Targets>,
    pub camera: CameraUniform,
    pub clear: Color,
    pub items: Vec<DrawItem>,
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
                    store: wgpu::StoreOp::Discard,
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
        assert_eq!(
            size_of::<MeshUniform>(),
            64 * 2 + 16 * 2 + 16 * PALETTE_SIZE
        );
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
    fn shader_parses_and_validates() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(include_str!("mesh.wgsl"))
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(include_str!("mesh.wgsl"))));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("valid shader");
        for entry in ["vs_main", "fs_main", "vs_wire", "fs_wire"] {
            assert!(
                module.entry_points.iter().any(|e| e.name == entry),
                "{entry}"
            );
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
