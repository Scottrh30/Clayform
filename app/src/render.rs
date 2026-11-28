//! wgpu (WebGL 2) renderer drawing the sculpt straight into the viewport canvas.

use bytemuck::{Pod, Zeroable};
use clayform_core::glam::Mat4;
use clayform_core::mesh::{Mesh, NONE};
use clayform_core::sculpt::GpuDirty;
use std::collections::HashSet;
use web_sys::HtmlCanvasElement;

/// First GPU error reported by wgpu (shown in the viewport).
pub static GPU_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuVertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    mask: f32,
    /// Local concavity (+ in creases, - on ridges) for cavity shading.
    cavity: f32,
}

/// How far a vertex sits below (+) or above (-) the average of its
/// neighbours, relative to their spacing.
fn cavity(mesh: &Mesh, i: usize) -> f32 {
    let nb = mesh.neighbors(i as u32);
    if nb.is_empty() {
        return 0.0;
    }
    let p = mesh.positions[i];
    let mut avg = clayform_core::glam::Vec3::ZERO;
    let mut len = 0.0;
    for &u in nb {
        let q = mesh.positions[u as usize];
        avg += q;
        len += q.distance(p);
    }
    let k = nb.len() as f32;
    let l = len / k;
    if l <= 0.0 { 0.0 } else { mesh.normals[i].dot(avg / k - p) / l }
}

/// Vertex of the expanded (one per triangle corner) polygroup view.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GroupVertex {
    pos: [f32; 3],
    nrm: [f32; 3],
    mask: f32,
    cavity: f32,
    color: [f32; 3],
}

/// A distinct, stable colour for polygroup `g` (golden-ratio hues).
pub fn group_color(g: u32) -> [f32; 3] {
    let h = (g as f32 * 0.618_034 + 0.12).fract() * 6.0;
    let (s, v) = (0.55 + 0.25 * ((g / 7) % 2) as f32, 0.95 - 0.2 * ((g / 3) % 2) as f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, gg, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    // to linear
    [(r + m).powf(2.2), (gg + m).powf(2.2), (b + m).powf(2.2)]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    inv_proj: [[f32; 4]; 4],
    params: [f32; 4],
    wire_color: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    viewport: [f32; 4],
    shading: [f32; 4],
}

#[derive(Clone, Copy, PartialEq)]
pub struct ViewSettings {
    pub material: u32,
    pub show_mask: bool,
    pub flat: bool,
    pub wireframe: bool,
    pub background: u32,
    /// Screen-space ambient occlusion.
    pub ao: bool,
    pub ao_strength: f32,
    /// Cavity shading (darken creases, lighten ridges).
    pub cavity: bool,
    pub cavity_strength: f32,
    /// Colour faces by polygroup.
    pub polygroups: bool,
}

/// Background presets: (name, top colour, bottom colour), linear RGB.
pub const BACKGROUNDS: [(&str, [f32; 3], [f32; 3]); 4] = [
    ("Dark gradient", [0.075, 0.08, 0.095], [0.018, 0.018, 0.022]),
    ("Studio grey", [0.30, 0.31, 0.33], [0.08, 0.085, 0.09]),
    ("Light", [0.72, 0.74, 0.78], [0.42, 0.44, 0.48]),
    ("Black", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
];

pub const MATERIALS: [&str; 7] = ["Clay", "Red wax", "Pearl", "Jade", "Chrome", "Normals", "Custom matcap"];
/// Index of the "Custom matcap" material (a loaded image).
pub const CUSTOM_MATCAP: u32 = 6;
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const PACKED_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Targets {
    size: [u32; 2],
    msaa_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// Half-resolution packed linear depth for SSAO, its z-buffer, and the AO result.
    ao_depth: wgpu::TextureView,
    ao_zbuf: wgpu::TextureView,
    ao_view: wgpu::TextureView,
    main_bind: wgpu::BindGroup,
    ssao_bind: wgpu::BindGroup,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    srgb_surface: bool,
    mesh_pipe: wgpu::RenderPipeline,
    wire_pipe: wgpu::RenderPipeline,
    group_pipe: wgpu::RenderPipeline,
    gbuf: Option<wgpu::Buffer>,
    group_count: u32,
    groups_stale: bool,
    bg_pipe: wgpu::RenderPipeline,
    depth_pipe: wgpu::RenderPipeline,
    ssao_pipe: wgpu::RenderPipeline,
    main_bgl: wgpu::BindGroupLayout,
    ssao_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    matcap_view: wgpu::TextureView,
    /// Why ambient occlusion is unavailable on this GPU, if it is.
    pub ao_error: Option<String>,
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

fn group_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32, 4 => Float32x3
    ];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GroupVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRS,
    }
}

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32];
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
        device.on_uncaptured_error(Box::new(|e| {
            if let Ok(mut slot) = GPU_ERROR.lock() {
                if slot.is_none() {
                    *slot = Some(e.to_string());
                }
            }
        }));
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
        let tex_entry = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let main_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("main textures"),
            entries: &[
                tex_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2, wgpu::TextureSampleType::Float { filterable: true }),
            ],
        });
        let ssao_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao depth"),
            entries: &[tex_entry(0, wgpu::TextureSampleType::Float { filterable: false })],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[&bgl, &main_bgl],
            push_constant_ranges: &[],
        });
        let ssao_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ssao layout"),
            bind_group_layouts: &[&bgl, &ssao_bgl],
            push_constant_ranges: &[],
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        // If this GPU can't build the AO passes, ambient occlusion turns
        // itself off instead of breaking the view.
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let ssao_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ssao shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ssao.wgsl").into()),
        });
        let depth_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ao depth"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &ssao_shader,
                entry_point: Some("vs_depth"),
                compilation_options: Default::default(),
                buffers: &[vertex_layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &ssao_shader,
                entry_point: Some("fs_depth"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: PACKED_DEPTH_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let ssao_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ssao"),
            layout: Some(&ssao_layout),
            vertex: wgpu::VertexState {
                module: &ssao_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &ssao_shader,
                entry_point: Some("fs_ssao"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: AO_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let ao_error = device.pop_error_scope().await;
        if let Some(e) = &ao_error {
            web_sys::console::warn_1(&format!("Ambient occlusion unavailable: {e}").into());
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let matcap_view = Self::make_texture(&device, &queue, 1, 1, &[255, 255, 255, 255]);

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
        let group_pipe = make(
            "polygroups",
            "vs_group",
            "fs_group",
            &[group_layout()],
            wgpu::PrimitiveTopology::TriangleList,
            true,
            wgpu::CompareFunction::Less,
            wgpu::DepthBiasState { constant: 2, slope_scale: 1.0, clamp: 0.0 },
            None,
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
            group_pipe,
            gbuf: None,
            group_count: 0,
            groups_stale: true,
            bg_pipe,
            depth_pipe,
            ssao_pipe,
            main_bgl,
            ssao_bgl,
            sampler,
            matcap_view,
            ao_error: ao_error.map(|e| e.to_string()),
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
        let ao_size = [(size[0] / 2).max(1), (size[1] / 2).max(1)];
        let half = |label: &str, format| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: ao_size[0], height: ao_size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let ao_depth = half("ao depth", PACKED_DEPTH_FORMAT);
        let ao_zbuf = half("ao zbuf", DEPTH_FORMAT);
        let ao_view = half("ao", AO_FORMAT);
        let ssao_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ssao bind"),
            layout: &self.ssao_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&ao_depth) }],
        });
        let main_bind = self.main_bind_group(&ao_view);
        self.targets = Some(Targets { size, msaa_view, depth_view, ao_depth, ao_zbuf, ao_view, main_bind, ssao_bind });
    }

    fn main_bind_group(&self, ao_view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("main bind"),
            layout: &self.main_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(ao_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.matcap_view) },
            ],
        })
    }

    fn make_texture(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("matcap"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        tex.create_view(&Default::default())
    }

    /// Uses an RGBA8 image (sRGB) as the custom matcap.
    pub fn set_matcap(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.matcap_view = Self::make_texture(&self.device, &self.queue, w, h, rgba);
        if let Some(t) = self.targets.as_mut() {
            let bind = {
                let ao = &t.ao_view;
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("main bind"),
                    layout: &self.main_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(ao) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.matcap_view) },
                    ],
                })
            };
            t.main_bind = bind;
        }
    }

    /// Uploads whatever changed in the mesh since the last frame.
    pub fn sync_mesh(&mut self, mesh: &Mesh, dirty: &mut GpuDirty, need_edges: bool, need_groups: bool) {
        if dirty.topology || dirty.range.is_some() {
            self.groups_stale = true;
        }
        let n = mesh.positions.len();
        let to_gpu = |i: usize| GpuVertex {
            pos: mesh.positions[i].to_array(),
            nrm: mesh.normals[i].to_array(),
            mask: mesh.mask[i],
            cavity: cavity(mesh, i),
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
            for (f, t) in mesh.tris.iter().enumerate().filter(|(_, t)| t[0] != NONE) {
                // the diagonal shared with a quad partner isn't drawn
                let p = mesh.partner.get(f).copied().unwrap_or(NONE);
                let pt = if p != NONE { mesh.tris[p as usize] } else { [NONE; 3] };
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    if p != NONE && pt.contains(&a) && pt.contains(&b) {
                        continue;
                    }
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
        if need_groups && self.groups_stale {
            let mut verts: Vec<GroupVertex> = Vec::with_capacity(mesh.tris.len() * 3);
            for (f, t) in mesh.tris.iter().enumerate().filter(|(_, t)| t[0] != NONE) {
                let color = group_color(mesh.groups.get(f).copied().unwrap_or(0));
                for &v in t {
                    let g = &self.staging[v as usize];
                    verts.push(GroupVertex { pos: g.pos, nrm: g.nrm, mask: g.mask, cavity: g.cavity, color });
                }
            }
            let bytes = (verts.len().max(1) * std::mem::size_of::<GroupVertex>()) as u64;
            if self.gbuf.as_ref().is_none_or(|b| b.size() < bytes) {
                self.gbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("polygroup vertices"),
                    size: bytes + bytes / 4,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            if !verts.is_empty() {
                self.queue.write_buffer(self.gbuf.as_ref().unwrap(), 0, bytemuck::cast_slice(&verts));
            }
            self.group_count = verts.len() as u32;
            self.groups_stale = false;
        }
    }

    /// `ao_radius` is the ambient-occlusion sampling radius in world units.
    pub fn render(&mut self, size_px: [u32; 2], view: Mat4, proj: Mat4, s: &ViewSettings, ao_radius: f32) {
        self.ensure_size(size_px);
        let size = self.targets.as_ref().unwrap().size;
        let (_, top, bottom) = BACKGROUNDS[(s.background as usize).min(BACKGROUNDS.len() - 1)];
        let u = Uniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            inv_proj: proj.inverse().to_cols_array_2d(),
            params: [
                s.material as f32,
                s.show_mask as u32 as f32,
                s.flat as u32 as f32,
                (!self.srgb_surface) as u32 as f32,
            ],
            wire_color: [0.0, 0.0, 0.0, 0.35],
            bg_top: [top[0], top[1], top[2], 1.0],
            bg_bottom: [bottom[0], bottom[1], bottom[2], 1.0],
            viewport: [size[0] as f32, size[1] as f32, 1.0 / size[0] as f32, 1.0 / size[1] as f32],
            shading: [
                (s.ao && self.vbuf.is_some() && self.ao_error.is_none()) as u32 as f32,
                s.ao_strength,
                if s.cavity { s.cavity_strength } else { 0.0 },
                ao_radius,
            ],
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
        if s.ao && self.ao_error.is_none() {
            if let (Some(vb), Some(ib)) = (&self.vbuf, &self.ibuf) {
                // 1. depth of the sculpt at half resolution
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ao depth"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &t.ao_depth,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &t.ao_zbuf,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.depth_pipe);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
            // 2. occlusion from that depth
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.ao_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if self.vbuf.is_some() {
                pass.set_pipeline(&self.ssao_pipe);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.set_bind_group(1, &t.ssao_bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
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
            pass.set_bind_group(1, &t.main_bind, &[]);
            pass.set_pipeline(&self.bg_pipe);
            pass.draw(0..3, 0..1);
            if let (Some(vb), Some(ib)) = (&self.vbuf, &self.ibuf) {
                match (&self.gbuf, s.polygroups) {
                    (Some(gb), true) => {
                        pass.set_vertex_buffer(0, gb.slice(..));
                        pass.set_pipeline(&self.group_pipe);
                        pass.draw(0..self.group_count, 0..1);
                    }
                    _ => {
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_pipeline(&self.mesh_pipe);
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..self.index_count, 0, 0..1);
                    }
                }
                pass.set_vertex_buffer(0, vb.slice(..));
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
