// Depth of field for rendered images: a disc blur whose size follows each
// pixel's distance from the focus plane.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    // x: material id, y: show mask, z: flat shading, w: encode sRGB in the shader
    params: vec4<f32>,
    wire_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    // xy: viewport size in pixels, zw: 1 / size
    viewport: vec4<f32>,
    // x: ambient occlusion on, y: AO strength, z: cavity strength, w: AO radius
    shading: vec4<f32>,
    // per light: view-space direction + on, then colour * intensity
    lights: array<vec4<f32>, 8>,
    light_vp: mat4x4<f32>,
    // x: ambient, y: shadows on, z: shadow softness (texels)
    lighting: vec4<f32>,
    // x: focus depth (0..1 of far), y: max blur (px), z: blur per depth, w: on
    dof: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var color_tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var depth_tex: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vi], 0.0, 1.0);
}

fn depth_at(px: vec2<i32>) -> f32 {
    let size = vec2<i32>(textureDimensions(depth_tex));
    let q = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    return dot(textureLoad(depth_tex, q, 0).rgb, vec3<f32>(1.0, 1.0 / 255.0, 1.0 / 65025.0));
}

// blur radius in pixels for a normalised depth
fn coc(d: f32) -> f32 {
    return clamp(abs(d - u.dof.x) * u.dof.z, 0.0, 1.0) * u.dof.y;
}

@fragment
fn fs_dof(@builtin(position) fc: vec4<f32>) -> @location(0) vec4<f32> {
    let size = u.viewport.xy;
    let uv = fc.xy / size;
    let d0 = depth_at(vec2<i32>(fc.xy));
    let r0 = coc(d0);
    var sum = textureSampleLevel(color_tex, samp, uv, 0.0);
    var wsum = 1.0;
    let n = 40;
    for (var i = 0; i < n; i++) {
        let fi = f32(i) + 0.5;
        let r = sqrt(fi / f32(n)) * max(r0, 0.0);
        let a = fi * 2.39996323;
        let off = vec2<f32>(cos(a), sin(a)) * r;
        let p = fc.xy + off;
        let ds = depth_at(vec2<i32>(p));
        let rs = coc(ds);
        // a sharp sample in front of this pixel must not spill onto it
        var w = 1.0;
        if (ds < d0) {
            w = smoothstep(r - 1.0, r + 1.0, rs);
        }
        sum += textureSampleLevel(color_tex, samp, p / size, 0.0) * w;
        wsum += w;
    }
    return sum / wsum;
}
