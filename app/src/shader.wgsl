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
@group(1) @binding(0) var ao_tex: texture_2d<f32>;
@group(1) @binding(1) var lin_samp: sampler;
@group(1) @binding(2) var matcap_tex: texture_2d<f32>;
@group(1) @binding(3) var shadow_tex: texture_2d<f32>;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) mask: f32,
    @location(3) cavity: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) vn: vec3<f32>,
    @location(1) mask: f32,
    @location(2) vpos: vec3<f32>,
    @location(3) cavity: f32,
    @location(4) lpos: vec4<f32>,
};

@vertex
fn vs_mesh(v: VIn) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.vn = (u.view * vec4<f32>(v.nrm, 0.0)).xyz;
    o.vpos = (u.view * vec4<f32>(v.pos, 1.0)).xyz;
    o.mask = v.mask;
    o.cavity = v.cavity;
    o.lpos = u.light_vp * vec4<f32>(v.pos + v.nrm * u.lighting.w, 1.0);
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
        case 6: { // Custom matcap image
            let uv = vec2<f32>(n.x, -n.y) * 0.49 + vec2<f32>(0.5);
            return textureSampleLevel(matcap_tex, lin_samp, uv, 0.0).rgb;
        }
        default: { // Normals
            return srgb(n * 0.5 + vec3<f32>(0.5));
        }
    }
}

struct GIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) mask: f32,
    @location(3) cavity: f32,
    @location(4) color: vec3<f32>,
};

struct GOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) vn: vec3<f32>,
    @location(1) mask: f32,
    @location(2) vpos: vec3<f32>,
    @location(3) cavity: f32,
    @location(4) color: vec3<f32>,
    @location(5) lpos: vec4<f32>,
};

@vertex
fn vs_group(v: GIn) -> GOut {
    var o: GOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.vn = (u.view * vec4<f32>(v.nrm, 0.0)).xyz;
    o.vpos = (u.view * vec4<f32>(v.pos, 1.0)).xyz;
    o.mask = v.mask;
    o.cavity = v.cavity;
    o.color = v.color;
    o.lpos = u.light_vp * vec4<f32>(v.pos + v.nrm * u.lighting.w, 1.0);
    return o;
}

@fragment
fn fs_group(i: GOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var v: VOut;
    v.clip = i.clip;
    v.vn = i.vn;
    v.mask = i.mask;
    v.vpos = i.vpos;
    v.cavity = i.cavity;
    v.lpos = i.lpos;
    return vec4<f32>(encode(shade(v, front, i.color, true)), 1.0);
}

struct LitMat {
    base: vec3<f32>,
    rough: f32,
    metal: f32,
    // light wrapping round the terminator (soft materials)
    wrap: f32,
};

fn lit_material(id: i32) -> LitMat {
    switch id {
        case 8: { return LitMat(srgb(vec3<f32>(0.86, 0.62, 0.52)), 0.55, 0.0, 0.35); } // skin
        case 9: { return LitMat(srgb(vec3<f32>(1.0, 0.78, 0.36)), 0.3, 1.0, 0.0); }   // gold
        case 10: { return LitMat(srgb(vec3<f32>(0.78, 0.78, 0.8)), 0.35, 0.0, 0.1); } // plastic
        case 11: { return LitMat(srgb(vec3<f32>(0.93, 0.93, 0.9)), 0.12, 0.0, 0.05); } // ceramic
        case 12: { return LitMat(srgb(vec3<f32>(0.8, 0.5, 0.3)), 0.4, 0.9, 0.0); }    // bronze
        default: { return LitMat(srgb(vec3<f32>(0.78, 0.6, 0.5)), 0.6, 0.0, 0.1); }   // clay
    }
}

fn unpack_depth(e: vec4<f32>) -> f32 {
    return dot(e.rgb, vec3<f32>(1.0, 1.0 / 255.0, 1.0 / 65025.0));
}

// 1 = lit by the key light, 0 = in its shadow (3x3 percentage-closer filter)
fn shadow_factor(lpos: vec4<f32>) -> f32 {
    if (u.lighting.y < 0.5) {
        return 1.0;
    }
    let p = lpos.xyz / lpos.w;
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if (uv.x <= 0.0 || uv.y <= 0.0 || uv.x >= 1.0 || uv.y >= 1.0 || p.z >= 1.0) {
        return 1.0;
    }
    let size = vec2<i32>(textureDimensions(shadow_tex));
    let base = vec2<i32>(uv * vec2<f32>(size));
    let soft = max(u.lighting.z, 0.5);
    var lit = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(base + vec2<i32>(vec2<f32>(f32(x), f32(y)) * soft), vec2<i32>(0), size - vec2<i32>(1));
            let d = unpack_depth(textureLoad(shadow_tex, q, 0));
            if (p.z - 0.0015 <= d) {
                lit += 1.0;
            }
        }
    }
    return lit / 9.0;
}

// filmic curve (ACES fit) so bright lights roll off instead of clipping
fn tonemap(x: vec3<f32>) -> vec3<f32> {
    let a = x * 0.9;
    return clamp((a * (2.51 * a + 0.03)) / (a * (2.43 * a + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

// physically inspired shading with the light rig (lights are in view space)
fn lit_shade(n: vec3<f32>, vpos: vec3<f32>, m: LitMat, shadow: f32) -> vec3<f32> {
    let v = normalize(-vpos);
    let up = normalize((u.view * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz);
    let hemi = mix(0.45, 1.0, dot(n, up) * 0.5 + 0.5);
    let f0 = mix(vec3<f32>(0.04), m.base, m.metal);
    var c = m.base * (1.0 - m.metal * 0.7) * u.lighting.x * hemi;
    // a little environment reflection for metals
    c += f0 * m.metal * u.lighting.x * 1.2 * mix(0.3, 1.0, dot(reflect(-v, n), up) * 0.5 + 0.5);
    let shininess = mix(400.0, 6.0, m.rough);
    for (var i = 0; i < 4; i++) {
        let ld = u.lights[i * 2];
        if (ld.w < 0.5) {
            continue;
        }
        let l = normalize(ld.xyz);
        let col = u.lights[i * 2 + 1].rgb;
        var s = 1.0;
        if (i == 0) {
            s = shadow;
        }
        let ndl = dot(n, l);
        let diffuse = max((ndl + m.wrap) / (1.0 + m.wrap), 0.0);
        let h = normalize(l + v);
        let spec = pow(max(dot(n, h), 0.0), shininess) * (shininess + 8.0) / 25.0 * 2.0;
        let fres = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - max(dot(h, v), 0.0), 5.0);
        c += col * s * (m.base * (1.0 - m.metal) * diffuse / 3.14159 * 2.0 + fres * spec * max(ndl, 0.0) * 0.15);
    }
    return c;
}

@fragment
fn fs_mesh(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(shade(i, front, vec3<f32>(1.0), false)), 1.0);
}

fn shade(i: VOut, front: bool, tint: vec3<f32>, tinted: bool) -> vec3<f32> {
    var n = normalize(i.vn);
    if (u.params.z > 0.5) {
        n = normalize(cross(dpdx(i.vpos), dpdy(i.vpos)));
        if (dot(n, -i.vpos) < 0.0) { n = -n; }
    } else if (!front) {
        n = -n;
    }
    let id = i32(u.params.x + 0.5);
    var c = vec3<f32>(0.0);
    if (id >= 7) {
        var m = lit_material(id);
        if (tinted) {
            m.base = tint;
        }
        c = tonemap(lit_shade(n, i.vpos, m, shadow_factor(i.lpos)));
    } else if (tinted) {
        c = studio(n, tint, 24.0, 0.12, tint, 0.2, 0.3);
    } else {
        c = material(n, id);
    }
    // cavity: darken creases, lift ridges
    let k = u.shading.z;
    if (k > 0.0) {
        let cav = clamp(i.cavity * 7.0, -1.0, 1.0);
        c = c * (1.0 - max(cav, 0.0) * 0.75 * k) + c * max(-cav, 0.0) * 0.35 * k;
    }
    // ambient occlusion (half-resolution buffer, softened with four taps)
    if (u.shading.x > 0.5) {
        let uv = i.clip.xy * u.viewport.zw;
        let o = u.viewport.zw * 1.5;
        let ao = 0.25 * (textureSampleLevel(ao_tex, lin_samp, uv + vec2<f32>(o.x, o.y), 0.0).r
            + textureSampleLevel(ao_tex, lin_samp, uv + vec2<f32>(-o.x, o.y), 0.0).r
            + textureSampleLevel(ao_tex, lin_samp, uv + vec2<f32>(o.x, -o.y), 0.0).r
            + textureSampleLevel(ao_tex, lin_samp, uv + vec2<f32>(-o.x, -o.y), 0.0).r);
        c = c * mix(1.0, ao, u.shading.y);
    }
    if (u.params.y > 0.5) {
        let m = clamp(i.mask, 0.0, 1.0);
        c = mix(c, c * vec3<f32>(0.18, 0.2, 0.28), m * 0.85);
    }
    return c;
}

// Surfaces without an sRGB format (some WebGL setups) need manual encoding.
fn encode(c: vec3<f32>) -> vec3<f32> {
    if (u.params.w > 0.5) {
        return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return c;
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
    return vec4<f32>(encode(mix(u.bg_bottom.rgb, u.bg_top.rgb, t) * vign), 1.0);
}
