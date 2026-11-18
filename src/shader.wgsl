struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // x: material id, y: show mask, z: flat shading, w: unused
    params: vec4<f32>,
    wire_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) mask: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) vn: vec3<f32>,
    @location(1) mask: f32,
    @location(2) vpos: vec3<f32>,
};

@vertex
fn vs_mesh(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.vn = (u.view * vec4<f32>(v.nrm, 0.0)).xyz;
    o.vpos = (u.view * vec4<f32>(v.pos, 1.0)).xyz;
    o.mask = v.mask;
    return o;
}

fn srgb(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

// Procedural "matcaps": shading depends only on the view-space normal.
fn studio(n: vec3<f32>, base: vec3<f32>, spec_pow: f32, spec_k: f32, rim_col: vec3<f32>, rim_k: f32, wrap: f32) -> vec3<f32> {
    let key = normalize(vec3<f32>(-0.45, 0.65, 0.62));
    let fill = normalize(vec3<f32>(0.7, -0.15, 0.55));
    let back = normalize(vec3<f32>(0.2, 0.4, -0.9));
    let dk = max((dot(n, key) + wrap) / (1.0 + wrap), 0.0);
    let df = max(dot(n, fill), 0.0);
    let db = max(dot(n, back), 0.0);
    let diffuse = base * (dk * 0.95 + df * 0.28 + 0.10) + base * db * 0.15;
    let h = normalize(key + vec3<f32>(0.0, 0.0, 1.0));
    let spec = pow(max(dot(n, h), 0.0), spec_pow) * spec_k;
    let fres = pow(1.0 - clamp(n.z, 0.0, 1.0), 3.0) * rim_k;
    return diffuse + vec3<f32>(spec) + rim_col * fres;
}

fn chrome(n: vec3<f32>) -> vec3<f32> {
    // reflect the view vector and look up a simple studio environment
    let r = reflect(vec3<f32>(0.0, 0.0, -1.0), n);
    let sky = mix(vec3<f32>(0.08, 0.08, 0.09), vec3<f32>(0.95, 0.96, 1.0), smoothstep(-0.2, 0.6, r.y));
    let horizon = exp(-abs(r.y) * 14.0) * 0.6;
    let softbox = smoothstep(0.85, 0.97, dot(r, normalize(vec3<f32>(-0.5, 0.6, 0.6)))) * 1.5;
    return sky * 0.7 + vec3<f32>(horizon + softbox);
}

fn material(n: vec3<f32>, id: i32) -> vec3<f32> {
    switch id {
        case 0: { // Clay
            return studio(n, srgb(vec3<f32>(0.78, 0.60, 0.50)), 18.0, 0.08, srgb(vec3<f32>(0.9, 0.75, 0.65)), 0.25, 0.25);
        }
        case 1: { // Red wax
            return studio(n, srgb(vec3<f32>(0.62, 0.10, 0.08)), 60.0, 0.45, srgb(vec3<f32>(1.0, 0.45, 0.35)), 0.35, 0.4);
        }
        case 2: { // Pearl
            return studio(n, srgb(vec3<f32>(0.86, 0.86, 0.88)), 30.0, 0.25, srgb(vec3<f32>(0.55, 0.7, 1.0)), 0.6, 0.35);
        }
        case 3: { // Jade
            return studio(n, srgb(vec3<f32>(0.25, 0.58, 0.42)), 80.0, 0.5, srgb(vec3<f32>(0.6, 1.0, 0.8)), 0.4, 0.5);
        }
        case 4: { // Chrome
            return chrome(n);
        }
        default: { // Normals
            return srgb(n * 0.5 + vec3<f32>(0.5));
        }
    }
}

@fragment
fn fs_mesh(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.vn);
    if (u.params.z > 0.5) {
        n = normalize(cross(dpdx(i.vpos), dpdy(i.vpos)));
        if (dot(n, -i.vpos) < 0.0) { n = -n; }
    } else if (!front) {
        n = -n;
    }
    var c = material(n, i32(u.params.x + 0.5));
    if (u.params.y > 0.5) {
        let m = clamp(i.mask, 0.0, 1.0);
        c = mix(c, c * vec3<f32>(0.18, 0.2, 0.28), m * 0.85);
    }
    return vec4<f32>(c, 1.0);
}

@vertex
fn vs_wire(v: VIn) -> @builtin(position) vec4<f32> {
    return u.view_proj * vec4<f32>(v.pos, 1.0);
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return u.wire_color;
}

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) vi: u32) -> BgOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: BgOut;
    o.clip = vec4<f32>(p[vi], 0.0, 1.0);
    o.uv = p[vi] * 0.5 + vec2<f32>(0.5);
    return o;
}

@fragment
fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    let t = clamp(i.uv.y, 0.0, 1.0);
    let vign = 1.0 - 0.25 * length(i.uv - vec2<f32>(0.5, 0.55));
    return vec4<f32>(mix(u.bg_bottom.rgb, u.bg_top.rgb, t) * vign, 1.0);
}
