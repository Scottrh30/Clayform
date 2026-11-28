//! wgpu (WebGL 2) renderer drawing the sculpt straight into the viewport canvas.

use bytemuck::{Pod, Zeroable};
use clayform_core::glam::{Mat4, Vec3};
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

/// Polygroup colour in linear RGB.
pub fn group_color(g: u32) -> [f32; 3] {
    clayform_core::poly::group_color(g).map(|c| c.powf(2.2))
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
    /// Per light: view-space direction + on, then colour * intensity.
    lights: [[f32; 4]; 8],
    light_vp: [[f32; 4]; 4],
    /// x ambient, y shadows on, z shadow softness (texels), w unused.
    lighting: [f32; 4],
    /// x focus depth (0..1 of the far plane), y max blur (px), z blur per depth, w unused.
    dof: [f32; 4],
}

/// A directional light. Azimuth 0 shines from the front, elevation 90 from above.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    pub on: bool,
    pub azimuth: f32,
    pub elevation: f32,
    /// sRGB colour.
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Light {
    /// Direction towards the light (in the camera's frame or the world's).
    pub fn dir(&self) -> Vec3 {
        let (a, e) = (self.azimuth.to_radians(), self.elevation.to_radians());
        Vec3::new(e.cos() * a.sin(), e.sin(), e.cos() * a.cos())
    }
}

/// The light rig used by the Lit materials.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Lighting {
    pub lights: [Light; 4],
    /// Lights turn with the camera (otherwise they stay put in the scene).
    pub follow_camera: bool,
    pub ambient: f32,
    /// The first light casts shadows.
    pub shadows: bool,
    pub softness: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        let l = |on, azimuth, elevation, color, intensity| Light { on, azimuth, elevation, color, intensity };
        Self {
            lights: [
                l(true, -60.0, 45.0, [1.0, 0.96, 0.9], 2.6),
                l(true, 55.0, 10.0, [0.75, 0.82, 1.0], 0.35),
                l(true, 170.0, 25.0, [1.0, 1.0, 1.0], 1.0),
                l(false, 0.0, -60.0, [1.0, 0.85, 0.7], 0.5),
            ],
            follow_camera: true,
            ambient: 0.18,
            shadows: true,
            softness: 1.5,
        }
    }
}

pub const LIGHT_NAMES: [&str; 4] = ["Key", "Fill", "Rim", "Bounce"];

/// Depth of field for rendered images.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DepthOfField {
    /// Focus distance from the camera, world units.
    pub focus: f32,
    /// Blur at the far plane / near objects, as a fraction of the image width.
    pub amount: f32,
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
    pub lighting: Lighting,
}

/// Background presets: (name, top colour, bottom colour), linear RGB.
pub const BACKGROUNDS: [(&str, [f32; 3], [f32; 3]); 4] = [
    ("Dark gradient", [0.075, 0.08, 0.095], [0.018, 0.018, 0.022]),
    ("Studio grey", [0.30, 0.31, 0.33], [0.08, 0.085, 0.09]),
    ("Light", [0.72, 0.74, 0.78], [0.42, 0.44, 0.48]),
    ("Black", [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
];

pub const MATERIALS: [&str; 13] = [
    "Clay",
    "Red wax",
    "Pearl",
    "Jade",
    "Chrome",
    "Normals",
    "Custom matcap",
    "Lit clay",
    "Lit skin",
    "Lit gold",
    "Lit plastic",
    "Lit ceramic",
    "Lit bronze",
];
/// Materials from this index on use the light rig.
pub const FIRST_LIT: u32 = 7;
const SHADOW_SIZE: u32 = 2048;
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
    dof_bgl: wgpu::BindGroupLayout,
    dof_pipe: wgpu::RenderPipeline,
    shadow_pipe: wgpu::RenderPipeline,
    shadow_view: wgpu::TextureView,
    shadow_zbuf: wgpu::TextureView,
    shadow_ubuf: wgpu::Buffer,
    shadow_bind: wgpu::BindGroup,
    /// Bounds of the mesh (for fitting the shadow map).
    bounds: (Vec3, Vec3),
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
                tex_entry(3, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let dof_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dof"),
            entries: &[
                tex_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
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
        let dof_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dof layout"),
            bind_group_layouts: &[&bgl, &dof_bgl],
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
        let shadow_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&depth_layout),
            vertex: wgpu::VertexState {
                module: &ssao_shader,
                entry_point: Some("vs_depth"),
                compilation_options: Default::default(),
                buffers: &[vertex_layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &ssao_shader,
                entry_point: Some("fs_shadow"),
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
        let dof_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dof shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dof.wgsl").into()),
        });
        let dof_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dof"),
            layout: Some(&dof_layout),
            vertex: wgpu::VertexState {
                module: &dof_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &dof_shader,
                entry_point: Some("fs_dof"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
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
        let plain = |label: &str, format, size: u32| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let shadow_view = plain("shadow map", PACKED_DEPTH_FORMAT, SHADOW_SIZE);
        let shadow_zbuf = plain("shadow zbuf", DEPTH_FORMAT, SHADOW_SIZE);
        let shadow_ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow bind"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: shadow_ubuf.as_entire_binding() }],
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
            dof_bgl,
            dof_pipe,
            shadow_pipe,
            shadow_view,
            shadow_zbuf,
            shadow_ubuf,
            shadow_bind,
            bounds: (Vec3::splat(-1.0), Vec3::splat(1.0)),
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
        self.targets = Some(self.make_targets(size));
    }

    /// Multisampled colour and depth plus the AO buffers for an image of `size`.
    fn make_targets(&self, size: [u32; 2]) -> Targets {
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
        let ao_depth = self.plain_texture("ao depth", PACKED_DEPTH_FORMAT, ao_size, false);
        let ao_zbuf = self.plain_texture("ao zbuf", DEPTH_FORMAT, ao_size, false);
        let ao_view = self.plain_texture("ao", AO_FORMAT, ao_size, false);
        let ssao_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ssao bind"),
            layout: &self.ssao_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&ao_depth) }],
        });
        let main_bind = self.main_bind_group(&ao_view);
        Targets { size, msaa_view, depth_view, ao_depth, ao_zbuf, ao_view, main_bind, ssao_bind }
    }

    fn plain_texture(&self, label: &str, format: wgpu::TextureFormat, size: [u32; 2], copy: bool) -> wgpu::TextureView {
        self.plain_texture_raw(label, format, size, copy).create_view(&Default::default())
    }

    fn plain_texture_raw(&self, label: &str, format: wgpu::TextureFormat, size: [u32; 2], copy: bool) -> wgpu::Texture {
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        if copy {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    fn main_bind_group(&self, ao_view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("main bind"),
            layout: &self.main_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(ao_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.matcap_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.shadow_view) },
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
        if let Some(t) = self.targets.as_ref() {
            let bind = self.main_bind_group(&t.ao_view);
            self.targets.as_mut().unwrap().main_bind = bind;
        }
    }

    /// Largest image `render_image` can make (per side).
    pub fn max_image_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d.min(8192)
    }

    /// Uploads whatever changed in the mesh since the last frame.
    pub fn sync_mesh(&mut self, mesh: &Mesh, dirty: &mut GpuDirty, need_edges: bool, need_groups: bool) {
        if dirty.topology || dirty.range.is_some() {
            self.groups_stale = true;
        }
        if dirty.topology || self.vbuf.is_none() {
            self.bounds = mesh.bounds();
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
            dirty.blocks.clear();
        } else if dirty.range.is_some() && n > 0 {
            // only the blocks of vertices that changed
            for (a, b) in dirty.take_runs(n) {
                let (a, b) = (a as usize, b as usize);
                for i in a..=b {
                    self.staging[i] = to_gpu(i);
                    // grow the shadow bounds with moved vertices (cheaper than a full pass)
                    let p = mesh.positions[i];
                    self.bounds = (self.bounds.0.min(p), self.bounds.1.max(p));
                }
                let off = (a * std::mem::size_of::<GpuVertex>()) as u64;
                self.queue.write_buffer(self.vbuf.as_ref().unwrap(), off, bytemuck::cast_slice(&self.staging[a..=b]));
            }
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

    fn uniforms(&self, size: [u32; 2], view: Mat4, proj: Mat4, s: &ViewSettings, ao_radius: f32, dof: Option<(f32, f32, f32)>) -> (Uniforms, Mat4) {
        let (_, top, bottom) = BACKGROUNDS[(s.background as usize).min(BACKGROUNDS.len() - 1)];
        let lit = s.material >= FIRST_LIT;
        let rig = &s.lighting;
        let inv_view = view.inverse();
        let mut lights = [[0f32; 4]; 8];
        for (i, l) in rig.lights.iter().enumerate() {
            let d = l.dir();
            let dv = if rig.follow_camera { d } else { view.transform_vector3(d).normalize_or_zero() };
            let c = l.color.map(|c| c.powf(2.2) * l.intensity);
            lights[i * 2] = [dv.x, dv.y, dv.z, l.on as u32 as f32];
            lights[i * 2 + 1] = [c[0], c[1], c[2], 0.0];
        }
        // the key light's shadow map covers the mesh's bounding sphere
        let key = rig.lights[0];
        let world_dir = if rig.follow_camera { inv_view.transform_vector3(key.dir()).normalize_or_zero() } else { key.dir() };
        let (mn, mx) = self.bounds;
        let c = (mn + mx) * 0.5;
        let r = ((mx - mn) * 0.5).length().max(1e-3) * 1.05;
        let up = if world_dir.y.abs() > 0.95 { Vec3::Z } else { Vec3::Y };
        let lview = Mat4::look_at_rh(c + world_dir * r * 2.0, c, up);
        let lproj = Mat4::orthographic_rh(-r, r, -r, r, r * 0.5, r * 3.5);
        let light_vp = lproj * lview;
        let shadows = lit && rig.shadows && key.on && self.vbuf.is_some();
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
            lights,
            light_vp: light_vp.to_cols_array_2d(),
            // w: how far to push points along their normal before the shadow lookup (no acne)
            lighting: [rig.ambient, shadows as u32 as f32, rig.softness, 3.0 * 2.0 * r / SHADOW_SIZE as f32],
            dof: match dof {
                Some((f, b, k)) => [f, b, k, 1.0],
                None => [0.0; 4],
            },
        };
        (u, light_vp)
    }

    /// Records the frame: shadow map, AO, then the scene resolved into `target`.
    #[allow(clippy::too_many_arguments)]
    fn encode_scene(
        &self,
        enc: &mut wgpu::CommandEncoder,
        t: &Targets,
        target: &wgpu::TextureView,
        s: &ViewSettings,
        u: &Uniforms,
        light_vp: Mat4,
        transparent: bool,
    ) {
        if u.lighting[1] > 0.5 {
            if let (Some(vb), Some(ib)) = (&self.vbuf, &self.ibuf) {
                // the shadow pass reuses the depth shader with the light's matrices
                let mut su = *u;
                su.view_proj = light_vp.to_cols_array_2d();
                self.queue.write_buffer(&self.shadow_ubuf, 0, bytemuck::bytes_of(&su));
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.shadow_view,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadow_zbuf,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.shadow_pipe);
                pass.set_bind_group(0, &self.shadow_bind, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }
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
        let clear = if transparent { wgpu::Color::TRANSPARENT } else { wgpu::Color::BLACK };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.msaa_view,
                resolve_target: Some(target),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Discard },
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
        if !transparent {
            pass.set_pipeline(&self.bg_pipe);
            pass.draw(0..3, 0..1);
        }
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

    /// `ao_radius` is the ambient-occlusion sampling radius in world units.
    pub fn render(&mut self, size_px: [u32; 2], view: Mat4, proj: Mat4, s: &ViewSettings, ao_radius: f32) {
        self.ensure_size(size_px);
        let size = self.targets.as_ref().unwrap().size;
        let (u, light_vp) = self.uniforms(size, view, proj, s, ao_radius, None);
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
        self.encode_scene(&mut enc, t, &target, s, &u, light_vp, false);
        self.queue.submit(Some(enc.finish()));
        frame.present();
    }

    /// Renders an image off screen and returns it as RGBA8 (sRGB) rows.
    /// `dof`: depth of field, with `focus_depth` the focus distance along the view.
    #[allow(clippy::too_many_arguments)]
    pub async fn render_image(
        &mut self,
        size: [u32; 2],
        view: Mat4,
        proj: Mat4,
        far: f32,
        s: &ViewSettings,
        ao_radius: f32,
        dof: Option<DepthOfField>,
        transparent: bool,
    ) -> Result<Vec<u8>, String> {
        let max = self.max_image_size();
        let size = [size[0].clamp(1, max), size[1].clamp(1, max)];
        let t = self.make_targets(size);
        let fmt = self.config.format;
        let scene = self.plain_texture_raw("render", fmt, size, true);
        let scene_view = scene.create_view(&Default::default());
        let dof_params = dof.map(|d| {
            // blur in pixels per unit of normalised depth difference
            let max_px = (d.amount * size[0] as f32).clamp(0.0, 64.0);
            (d.focus / far, max_px, 1.0 / (d.focus / far).max(1e-4))
        });
        let (u, light_vp) = self.uniforms(size, view, proj, s, ao_radius, dof_params);
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_scene(&mut enc, &t, &scene_view, s, &u, light_vp, transparent);
        let mut out_tex = scene;
        if let (Some(_), Some(vb), Some(ib)) = (dof_params, &self.vbuf, &self.ibuf) {
            // full-resolution linear depth for the blur
            let depth = self.plain_texture("dof depth", PACKED_DEPTH_FORMAT, size, false);
            let zbuf = self.plain_texture("dof zbuf", DEPTH_FORMAT, size, false);
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("dof depth"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &depth,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &zbuf,
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
            let blurred = self.plain_texture_raw("dof out", fmt, size, true);
            let blurred_view = blurred.create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("dof bind"),
                layout: &self.dof_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&scene_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&depth) },
                ],
            });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("dof"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &blurred_view,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.dof_pipe);
                pass.set_bind_group(0, &self.bind, &[]);
                pass.set_bind_group(1, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            out_tex = blurred;
        }
        // read back
        let row = (size[0] * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * size[1]) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &out_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size[1]) },
            },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let done: std::rc::Rc<std::cell::Cell<Option<bool>>> = Default::default();
        let flag = done.clone();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| flag.set(Some(r.is_ok())));
        let mut waited = 0;
        loop {
            let _ = self.device.poll(wgpu::Maintain::Poll);
            if let Some(ok) = done.get() {
                if !ok {
                    return Err("Could not read the image back from the GPU".into());
                }
                break;
            }
            waited += 1;
            if waited > 3000 {
                return Err("The GPU took too long to render the image".into());
            }
            sleep(5).await;
        }
        let data = buf.slice(..).get_mapped_range();
        let bgra = matches!(fmt, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] as usize {
            let line = &data[y * row as usize..][..size[0] as usize * 4];
            for px in line.chunks_exact(4) {
                let a = if transparent { px[3] } else { 255 };
                if bgra {
                    out.extend_from_slice(&[px[2], px[1], px[0], a]);
                } else {
                    out.extend_from_slice(&[px[0], px[1], px[2], a]);
                }
            }
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

async fn sleep(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}
