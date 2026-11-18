//! Offscreen wgpu renderer for the sculpt viewport. The result is shown in
//! egui as a native texture.

use crate::mesh::Mesh;
use crate::sculpt::GpuDirty;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use glam::Mat4;
use std::collections::HashSet;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuVertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    mask: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    params: [f32; 4],
    wire_color: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
}

pub struct ViewSettings {
    pub material: u32,
    pub show_mask: bool,
    pub flat: bool,
    pub wireframe: bool,
}

struct Targets {
    size: [u32; 2],
    resolve_view: wgpu::TextureView,
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    mesh_pipe: wgpu::RenderPipeline,
    wire_pipe: wgpu::RenderPipeline,
    bg_pipe: wgpu::RenderPipeline,
    ubuf: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vbuf: Option<wgpu::Buffer>,
    ibuf: Option<wgpu::Buffer>,
    ebuf: Option<wgpu::Buffer>,
    index_count: u32,
    edge_count: u32,
    edges_stale: bool,
    staging: Vec<GpuVertex>,
    targets: Option<Targets>,
    pub texture_id: Option<egui::TextureId>,
}

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRS,
    }
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("clayform shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
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
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[wgpu::VertexBufferLayout],
                    topology: wgpu::PrimitiveTopology,
                    depth_write: bool,
                    compare: wgpu::CompareFunction,
                    bias: wgpu::DepthBiasState,
                    blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: depth_write,
                    depth_compare: compare,
                    stencil: Default::default(),
                    bias,
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
                multiview: None,
                cache: None,
            })
        };

        let mesh_pipe = make(
            "mesh",
            "vs_mesh",
            "fs_mesh",
            &[vertex_layout()],
            wgpu::PrimitiveTopology::TriangleList,
            true,
            wgpu::CompareFunction::Less,
            wgpu::DepthBiasState { constant: 2, slope_scale: 1.0, clamp: 0.0 },
            None,
        );
        let wire_pipe = make(
            "wire",
            "vs_wire",
            "fs_wire",
            &[vertex_layout()],
            wgpu::PrimitiveTopology::LineList,
            false,
            wgpu::CompareFunction::LessEqual,
            Default::default(),
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let bg_pipe = make(
            "background",
            "vs_bg",
            "fs_bg",
            &[],
            wgpu::PrimitiveTopology::TriangleList,
            false,
            wgpu::CompareFunction::Always,
            Default::default(),
            None,
        );

        Self {
            device: device.clone(),
            queue: queue.clone(),
            mesh_pipe,
            wire_pipe,
            bg_pipe,
            ubuf,
            bind,
            vbuf: None,
            ibuf: None,
            ebuf: None,
            index_count: 0,
            edge_count: 0,
            edges_stale: true,
            staging: Vec::new(),
            targets: None,
            texture_id: None,
        }
    }

    fn ensure_targets(&mut self, size: [u32; 2], egui_renderer: &mut egui_wgpu::Renderer) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        let tex = |label: &str, format, samples, usage| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let resolve_view = tex(
            "viewport",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa_view = tex("msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth_view = tex("depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        match self.texture_id {
            Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &resolve_view,
                wgpu::FilterMode::Linear,
                id,
            ),
            None => {
                self.texture_id =
                    Some(egui_renderer.register_native_texture(&self.device, &resolve_view, wgpu::FilterMode::Linear))
            }
        }
        self.targets = Some(Targets { size, resolve_view, msaa_view, depth_view });
    }

    /// Uploads whatever changed in the mesh since the last frame.
    pub fn sync_mesh(&mut self, mesh: &Mesh, dirty: &mut GpuDirty, need_edges: bool) {
        let n = mesh.positions.len();
        let to_gpu = |i: usize| GpuVertex {
            pos: mesh.positions[i].to_array(),
            nrm: mesh.normals[i].to_array(),
            mask: mesh.mask[i],
            _pad: 0.0,
        };
        if dirty.topology || self.vbuf.is_none() {
            self.staging = (0..n).map(to_gpu).collect();
            let vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size: (n.max(1) * std::mem::size_of::<GpuVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&vbuf, 0, bytemuck::cast_slice(&self.staging));
            let flat: &[u32] = bytemuck::cast_slice(&mesh.tris);
            let ibuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indices"),
                size: (flat.len().max(1) * 4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&ibuf, 0, bytemuck::cast_slice(flat));
            self.vbuf = Some(vbuf);
            self.ibuf = Some(ibuf);
            self.index_count = flat.len() as u32;
            self.edges_stale = true;
            dirty.topology = false;
            dirty.range = None;
        } else if let Some((a, b)) = dirty.range.take().filter(|_| n > 0) {
            let (a, b) = (a as usize, (b as usize).min(n - 1));
            for i in a..=b {
                self.staging[i] = to_gpu(i);
            }
            let off = (a * std::mem::size_of::<GpuVertex>()) as u64;
            self.queue.write_buffer(
                self.vbuf.as_ref().unwrap(),
                off,
                bytemuck::cast_slice(&self.staging[a..=b]),
            );
        }
        if need_edges && self.edges_stale {
            let mut seen: HashSet<(u32, u32)> = HashSet::with_capacity(mesh.tris.len() * 3 / 2);
            let mut edges: Vec<u32> = Vec::with_capacity(mesh.tris.len() * 3);
            for t in &mesh.tris {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    if seen.insert((a.min(b), a.max(b))) {
                        edges.push(a);
                        edges.push(b);
                    }
                }
            }
            let ebuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("edges"),
                size: (edges.len().max(1) * 4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&ebuf, 0, bytemuck::cast_slice(&edges));
            self.ebuf = Some(ebuf);
            self.edge_count = edges.len() as u32;
            self.edges_stale = false;
        }
    }

    pub fn render(
        &mut self,
        size: [u32; 2],
        egui_renderer: &mut egui_wgpu::Renderer,
        view: Mat4,
        proj: Mat4,
        settings: &ViewSettings,
        dark_ui: bool,
    ) {
        self.ensure_targets(size, egui_renderer);
        let (top, bottom) = if dark_ui {
            ([0.075, 0.08, 0.095, 1.0], [0.018, 0.018, 0.022, 1.0])
        } else {
            ([0.55, 0.58, 0.63, 1.0], [0.22, 0.23, 0.26, 1.0])
        };
        let u = Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            params: [
                settings.material as f32,
                settings.show_mask as u32 as f32,
                settings.flat as u32 as f32,
                0.0,
            ],
            wire_color: [0.0, 0.0, 0.0, 0.35],
            bg_top: top,
            bg_bottom: bottom,
        };
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));

        let t = self.targets.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa_view,
                    resolve_target: Some(&t.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_pipeline(&self.bg_pipe);
            pass.draw(0..3, 0..1);
            if let (Some(vb), Some(ib)) = (&self.vbuf, &self.ibuf) {
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_pipeline(&self.mesh_pipe);
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
                if settings.wireframe {
                    if let Some(eb) = &self.ebuf {
                        pass.set_pipeline(&self.wire_pipe);
                        pass.set_index_buffer(eb.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..self.edge_count, 0, 0..1);
                    }
                }
            }
        }
        self.queue.submit(Some(enc.finish()));
    }
}
