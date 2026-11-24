//! wgpu (WebGL 2) renderer drawing the sculpt straight into the viewport canvas.

use bytemuck::{Pod, Zeroable};
use clayform_core::glam::Mat4;
use clayform_core::mesh::{Mesh, NONE};
use clayform_core::sculpt::GpuDirty;
use std::collections::HashSet;
use web_sys::HtmlCanvasElement;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuVertex {
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

#[derive(Clone, Copy, PartialEq)]
pub struct ViewSettings {
    pub material: u32,
    pub show_mask: bool,
    pub flat: bool,
    pub wireframe: bool,
    pub background: u32,
}

/// Background presets: (name, top colour, bottom colour), linear RGB.
pub const BACKGROUNDS: [(&str, [f32; 3], [f32; 3]); 4] = [
    ("Dark gradient", [0.075, 0.08, 0.095], [0.018, 0.018, 0.022]),
    ("Studio grey", [0.30, 0.31, 0.33], [0.08, 0.085, 0.09]),
    ("Light", [0.72, 0.74, 0.78], [0.42, 0.44, 0.48]),
    ("Black", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
];

pub const MATERIALS: [&str; 6] = ["Clay", "Red wax", "Pearl", "Jade", "Chrome", "Normals"];

struct Targets {
    size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    srgb_surface: bool,
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
    index_staging: Vec<u32>,
    targets: Option<Targets>,
    pub adapter_name: String,
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
    pub async fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|e| format!("Could not create a WebGL 2 surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or("No WebGL 2 graphics adapter is available")?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("clayform"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("Could not open the graphics device: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: canvas.width().max(1),
            height: canvas.height().max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

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
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
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

        Ok(Self {
            adapter_name: adapter.get_info().name,
            surface,
            config,
            srgb_surface: format.is_srgb(),
            device,
            queue,
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
            index_staging: Vec::new(),
            targets: None,
        })
    }

    fn ensure_size(&mut self, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        self.config.width = size[0];
        self.config.height = size[1];
        self.surface.configure(&self.device, &self.config);
        let tex = |label: &str, format| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: SAMPLES,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let msaa_view = tex("msaa", self.config.format);
        let depth_view = tex("depth", DEPTH_FORMAT);
        self.targets = Some(Targets { size, msaa_view, depth_view });
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
            // Dynamic topology re-uploads every frame while sculpting, so
            // buffers grow with headroom and are reused when big enough.
            self.staging.clear();
            self.staging.extend((0..n).map(to_gpu));
            self.index_staging.clear();
            for t in &mesh.tris {
                if t[0] != NONE {
                    self.index_staging.extend_from_slice(t);
                }
            }
            let vbytes = (self.staging.len().max(1) * std::mem::size_of::<GpuVertex>()) as u64;
            if self.vbuf.as_ref().is_none_or(|b| b.size() < vbytes) {
                self.vbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("vertices"),
                    size: vbytes + vbytes / 2,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let ibytes = (self.index_staging.len().max(3) * 4) as u64;
            if self.ibuf.as_ref().is_none_or(|b| b.size() < ibytes) {
                self.ibuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("indices"),
                    size: (ibytes + ibytes / 2 + 3) & !3,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            if !self.staging.is_empty() {
                self.queue.write_buffer(self.vbuf.as_ref().unwrap(), 0, bytemuck::cast_slice(&self.staging));
            }
            if !self.index_staging.is_empty() {
                self.queue.write_buffer(self.ibuf.as_ref().unwrap(), 0, bytemuck::cast_slice(&self.index_staging));
            }
            self.index_count = self.index_staging.len() as u32;
            self.edges_stale = true;
            dirty.topology = false;
            dirty.range = None;
        } else if let Some((a, b)) = dirty.range.take().filter(|_| n > 0) {
            let (a, b) = (a as usize, (b as usize).min(n - 1));
            for i in a..=b {
                self.staging[i] = to_gpu(i);
            }
            let off = (a * std::mem::size_of::<GpuVertex>()) as u64;
            self.queue.write_buffer(self.vbuf.as_ref().unwrap(), off, bytemuck::cast_slice(&self.staging[a..=b]));
        }
        if need_edges && self.edges_stale {
            let mut seen: HashSet<(u32, u32)> = HashSet::with_capacity(mesh.tris.len() * 3 / 2);
            let mut edges: Vec<u32> = Vec::with_capacity(mesh.tris.len() * 3);
            for t in mesh.tris.iter().filter(|t| t[0] != NONE) {
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

    pub fn render(&mut self, size_px: [u32; 2], view: Mat4, proj: Mat4, s: &ViewSettings) {
        self.ensure_size(size_px);
        let (_, top, bottom) = BACKGROUNDS[(s.background as usize).min(BACKGROUNDS.len() - 1)];
        let u = Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            params: [
                s.material as f32,
                s.show_mask as u32 as f32,
                s.flat as u32 as f32,
                (!self.srgb_surface) as u32 as f32,
            ],
            wire_color: [0.0, 0.0, 0.0, 0.35],
            bg_top: [top[0], top[1], top[2], 1.0],
            bg_bottom: [bottom[0], bottom[1], bottom[2], 1.0],
        };
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let target = frame.texture.create_view(&Default::default());
        let t = self.targets.as_ref().unwrap();
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa_view,
                    resolve_target: Some(&target),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
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
                if s.wireframe {
                    if let Some(eb) = &self.ebuf {
                        pass.set_pipeline(&self.wire_pipe);
                        pass.set_index_buffer(eb.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..self.edge_count, 0, 0..1);
                    }
                }
            }
        }
        self.queue.submit(Some(enc.finish()));
        frame.present();
    }
}
