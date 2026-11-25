// Screen-space ambient occlusion, computed at half resolution. A first pass
// writes linear depth packed into an RGBA8 texture (depth textures can't be
// read with textureLoad under WebGL); the second pass computes occlusion.
// Same uniform block as shader.wgsl.

struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    params: vec4<f32>,
    wire_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    viewport: vec4<f32>,
    shading: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var depth_tex: texture_2d<f32>;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) mask: f32,
    @location(3) cavity: f32,
};

struct DOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) vz: f32,
};

// far plane of the perspective projection (glam perspective_rh, 0..1 depth)
fn far_plane() -> f32 {
    let r = u.proj[2][2];
    let n = u.proj[3][2] / r;
    return r * n / (1.0 + r);
}

@vertex
fn vs_depth(v: VIn) -> DOut {
    var o: DOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.vz = (u.view * vec4<f32>(v.pos, 1.0)).z;
    return o;
}

@fragment
fn fs_depth(i: DOut) -> @location(0) vec4<f32> {
    let d = clamp(-i.vz / far_plane(), 0.0, 0.9999);
    var e = fract(d * vec3<f32>(1.0, 255.0, 65025.0));
    e = e - vec3<f32>(e.y, e.z, 0.0) / 255.0;
    return vec4<f32>(e, 1.0);
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vi], 0.0, 1.0);
}

fn linear_depth(px: vec2<i32>, size: vec2<i32>) -> f32 {
    let q = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    let e = textureLoad(depth_tex, q, 0).rgb;
    return dot(e, vec3<f32>(1.0, 1.0 / 255.0, 1.0 / 65025.0));
}

fn view_pos(px: vec2<i32>, size: vec2<i32>) -> vec3<f32> {
    let q = clamp(px, vec2<i32>(0), size - vec2<i32>(1));
    let z = -linear_depth(q, size) * far_plane();
    let uv = (vec2<f32>(q) + vec2<f32>(0.5)) / vec2<f32>(size);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return vec3<f32>(ndc.x * -z / u.proj[0][0], ndc.y * -z / u.proj[1][1], z);
}

@fragment
fn fs_ssao(@builtin(position) fc: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(depth_tex));
    let px = vec2<i32>(fc.xy);
    if (linear_depth(px, size) >= 0.999) {
        return vec4<f32>(1.0); // background
    }
    let p = view_pos(px, size);
    // normal from neighbouring depths, picking the smoother side at edges
    let pr = view_pos(px + vec2<i32>(1, 0), size);
    let pl = view_pos(px - vec2<i32>(1, 0), size);
    let pd = view_pos(px + vec2<i32>(0, 1), size);
    let pu = view_pos(px - vec2<i32>(0, 1), size);
    var dx = pr - p;
    if (abs(p.z - pl.z) < abs(pr.z - p.z)) { dx = p - pl; }
    var dy = pd - p;
    if (abs(p.z - pu.z) < abs(pd.z - p.z)) { dy = p - pu; }
    var n = normalize(cross(dx, dy));
    if (dot(n, -p) < 0.0) { n = -n; }

    // per-pixel rotation (interleaved gradient noise) to break up banding
    let ang = 6.2831853 * fract(52.9829189 * fract(0.06711056 * fc.x + 0.00583715 * fc.y));
    let rv = vec3<f32>(cos(ang), sin(ang), 0.0);
    let t = normalize(rv - n * dot(rv, n) + vec3<f32>(1e-4, 0.0, 0.0));
    let b = cross(n, t);

    var kernel = array<vec3<f32>, 12>(
        vec3<f32>(0.53, 0.14, 0.33), vec3<f32>(-0.27, 0.42, 0.21), vec3<f32>(0.10, -0.55, 0.40),
        vec3<f32>(-0.48, -0.20, 0.25), vec3<f32>(0.30, 0.62, 0.55), vec3<f32>(-0.66, 0.31, 0.62),
        vec3<f32>(0.71, -0.38, 0.50), vec3<f32>(-0.12, -0.80, 0.45), vec3<f32>(0.20, 0.12, 0.92),
        vec3<f32>(-0.35, 0.10, 0.85), vec3<f32>(0.85, 0.30, 0.30), vec3<f32>(-0.80, -0.50, 0.20)
    );
    let radius = u.shading.w;
    var occ = 0.0;
    for (var k = 0; k < 12; k++) {
        let kv = kernel[k];
        let s = p + (t * kv.x + b * kv.y + n * kv.z) * radius;
        let c = u.proj * vec4<f32>(s, 1.0);
        let uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
        let sp = view_pos(vec2<i32>(uv * vec2<f32>(size)), size);
        let range = smoothstep(0.0, 1.0, radius / max(abs(p.z - sp.z), 1e-5));
        if (sp.z >= s.z + radius * 0.03) {
            occ += range;
        }
    }
    let ao = clamp(1.0 - occ / 12.0, 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}
