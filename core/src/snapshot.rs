//! Snapshot 3D: turn an image's silhouette into a solid (extruded or
//! inflated), to start a sculpt or to add to / cut from one.

use crate::mesh::{Mesh, MeshBuilder};
use glam::{Mat3, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Flat front and back, straight sides.
    Extrude,
    /// Rounded like a pillow, thickest in the middle.
    Inflate,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SnapshotSettings {
    /// Brightness (0..1) below which a pixel is part of the shape.
    pub threshold: f32,
    /// Use light pixels instead of dark ones.
    pub invert: bool,
    pub style: Style,
    /// Thickness as a fraction of the image's longer side.
    pub depth: f32,
    /// Voxels across the longer side.
    pub resolution: u32,
    pub smooth: u32,
}

impl Default for SnapshotSettings {
    fn default() -> Self {
        Self { threshold: 0.5, invert: false, style: Style::Inflate, depth: 0.25, resolution: 160, smooth: 2 }
    }
}

/// Max pixels on the longer side used for the silhouette.
const MAX_SIDE: usize = 384;

/// The image's shape: `true` = inside. Transparent images use their alpha;
/// opaque ones use brightness. Returns (mask, width, height) after scaling
/// down to at most `MAX_SIDE` pixels.
pub fn silhouette(w: usize, h: usize, rgba: &[u8], s: &SnapshotSettings) -> (Vec<bool>, usize, usize) {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return (Vec::new(), 0, 0);
    }
    let step = (w.max(h) as f32 / MAX_SIDE as f32).max(1.0);
    let (ow, oh) = (((w as f32) / step).round().max(1.0) as usize, ((h as f32) / step).round().max(1.0) as usize);
    let has_alpha = rgba.chunks_exact(4).any(|p| p[3] < 250);
    let mut mask = vec![false; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            // average the source block
            let (x0, x1) = ((x as f32 * step) as usize, (((x + 1) as f32 * step) as usize).clamp(1, w));
            let (y0, y1) = ((y as f32 * step) as usize, (((y + 1) as f32 * step) as usize).clamp(1, h));
            let (mut lum, mut alpha, mut n) = (0.0, 0.0, 0.0);
            for sy in y0.min(h - 1)..y1.max(y0 + 1).min(h) {
                for sx in x0.min(w - 1)..x1.max(x0 + 1).min(w) {
                    let p = &rgba[(sy * w + sx) * 4..][..4];
                    lum += (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
                    alpha += p[3] as f32 / 255.0;
                    n += 1.0;
                }
            }
            let (lum, alpha) = (lum / n, alpha / n);
            let inside = if has_alpha { alpha >= 0.5 } else { lum < s.threshold } ^ s.invert;
            mask[y * ow + x] = inside;
        }
    }
    (mask, ow, oh)
}

/// 1-D squared distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: replace
                v[0] = q;
                z[0] = f32::NEG_INFINITY;
                z[1] = f32::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let d = q as f32 - p as f32;
        *o = d * d + f[p];
    }
}

/// Distance (pixels) from each pixel to the nearest pixel where `seed` is true.
fn edt(seed: &[bool], w: usize, h: usize) -> Vec<f32> {
    const BIG: f32 = 1e12;
    let mut g: Vec<f32> = seed.iter().map(|&s| if s { 0.0 } else { BIG }).collect();
    let n = w.max(h);
    let (mut f, mut out, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0f32; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = g[y * w + x];
        }
        edt_1d(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            g[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut out[..w], &mut v, &mut z);
        for x in 0..w {
            g[y * w + x] = out[x].sqrt();
        }
    }
    g
}

/// Signed distance in pixels: negative inside the shape.
pub fn signed_distance(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    let outside: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let to_inside = edt(mask, w, h);
    let to_outside = edt(&outside, w, h);
    (0..mask.len())
        .map(|i| if mask[i] { -(to_outside[i] - 0.5) } else { to_inside[i] - 0.5 })
        .collect()
}

/// Dense-grid surface nets: the zero level of `f` inside the box
/// `[min, max]`, cubes of side `h`. `f` must be positive on the box faces.
pub fn surface_nets(min: Vec3, max: Vec3, h: f32, f: &dyn Fn(Vec3) -> f32) -> Result<Mesh, String> {
    let dims = ((max - min) / h).ceil().as_uvec3() + glam::UVec3::ONE;
    let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
    if nx * ny * nz > 40_000_000 {
        return Err("Resolution too high".into());
    }
    let node = |i: usize, j: usize, k: usize| min + Vec3::new(i as f32, j as f32, k as f32) * h;
    let idx = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
    let mut val = vec![0f32; nx * ny * nz];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                val[idx(i, j, k)] = f(node(i, j, k));
            }
        }
    }
    // one vertex per cell the surface passes through
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cidx = |i: usize, j: usize, k: usize| i + cx * (j + cy * k);
    let mut cell_vert = vec![u32::MAX; cx * cy * cz];
    let mut pos = Vec::new();
    const CORNERS: [[usize; 3]; 8] =
        [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
    const EDGES: [(usize, usize); 12] =
        [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    for k in 0..cz {
        for j in 0..cy {
            for i in 0..cx {
                let d: Vec<f32> = CORNERS.iter().map(|c| val[idx(i + c[0], j + c[1], k + c[2])]).collect();
                let inside = d.iter().filter(|&&x| x < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                // dual contouring: the point that best fits the surface's
                // tangent planes at the crossings, so sharp edges and corners
                // stay sharp instead of being cut off
                let mut sum = Vec3::ZERO;
                let mut n = 0.0;
                let mut ata = glam::Mat3::ZERO;
                let mut atb = Vec3::ZERO;
                let base = node(i, j, k);
                for &(a, b) in &EDGES {
                    if (d[a] < 0.0) != (d[b] < 0.0) {
                        let t = d[a] / (d[a] - d[b]);
                        let pa = Vec3::new(CORNERS[a][0] as f32, CORNERS[a][1] as f32, CORNERS[a][2] as f32);
                        let pb = Vec3::new(CORNERS[b][0] as f32, CORNERS[b][1] as f32, CORNERS[b][2] as f32);
                        let local = pa.lerp(pb, t);
                        sum += local;
                        n += 1.0;
                        let q = base + local * h;
                        let g = gradient(f, q, h * 0.5);
                        if g != Vec3::ZERO {
                            ata += glam::Mat3::from_cols(g * g.x, g * g.y, g * g.z);
                            atb += g * g.dot(local);
                        }
                    }
                }
                let mass = sum / n;
                // a little pull to the mass point keeps flat areas tidy
                let lambda = 0.05;
                let m = ata + glam::Mat3::from_diagonal(Vec3::splat(lambda));
                let rhs = atb + mass * lambda;
                let mut local = if m.determinant().abs() > 1e-9 { m.inverse() * rhs } else { mass };
                if !local.is_finite() {
                    local = mass;
                }
                let local = local.clamp(Vec3::splat(-0.25), Vec3::splat(1.25));
                cell_vert[cidx(i, j, k)] = pos.len() as u32;
                pos.push(base + local * h);
            }
        }
    }
    let mut tris = Vec::new();
    for k in 1..nz - 1 {
        for j in 1..ny - 1 {
            for i in 1..nx - 1 {
                let d0 = val[idx(i, j, k)];
                for axis in 0..3 {
                    let mut e = [i, j, k];
                    e[axis] += 1;
                    if e[axis] >= [nx, ny, nz][axis] {
                        continue;
                    }
                    let d1 = val[idx(e[0], e[1], e[2])];
                    if (d0 < 0.0) == (d1 < 0.0) {
                        continue;
                    }
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    let mut quad = [0u32; 4];
                    let mut ok = true;
                    for (n, (du, dv)) in [(1usize, 1usize), (0, 1), (0, 0), (1, 0)].into_iter().enumerate() {
                        let mut c = [i, j, k];
                        c[u] -= du;
                        c[v] -= dv;
                        let cv = cell_vert[cidx(c[0], c[1], c[2])];
                        ok &= cv != u32::MAX;
                        quad[n] = cv;
                    }
                    if !ok {
                        continue;
                    }
                    if d1 < 0.0 {
                        quad.reverse();
                    }
                    let [a, b, c, d] = quad;
                    if pos[a as usize].distance_squared(pos[c as usize]) <= pos[b as usize].distance_squared(pos[d as usize]) {
                        tris.push([a, b, c]);
                        tris.push([a, c, d]);
                    } else {
                        tris.push([a, b, d]);
                        tris.push([b, c, d]);
                    }
                }
            }
        }
    }
    if tris.is_empty() {
        return Err("No shape found in the image; try another threshold or Invert".into());
    }
    Ok(MeshBuilder::new(pos, tris).finish())
}

/// Unit normal of the field (central differences).
fn gradient(f: &dyn Fn(Vec3) -> f32, p: Vec3, e: f32) -> Vec3 {
    let g = Vec3::new(
        f(p + Vec3::X * e) - f(p - Vec3::X * e),
        f(p + Vec3::Y * e) - f(p - Vec3::Y * e),
        f(p + Vec3::Z * e) - f(p - Vec3::Z * e),
    );
    g.normalize_or_zero()
}

/// Evens out the spacing while keeping sharp edges: vertices where faces
/// meet at a crease stay put, the rest relax and snap back onto the surface.
fn relax(mesh: &mut Mesh, passes: u32, f: &dyn Fn(Vec3) -> f32, h: f32) {
    mesh.compute_all_normals();
    let cos = 35f32.to_radians().cos();
    let sharp: Vec<bool> = (0..mesh.positions.len())
        .map(|v| {
            let fs = mesh.vert_faces(v as u32);
            fs.iter().any(|&a| {
                let na = mesh.face_normals[a as usize].normalize_or_zero();
                fs.iter().any(|&b| na.dot(mesh.face_normals[b as usize].normalize_or_zero()) < cos)
            })
        })
        .collect();
    for _ in 0..passes {
        let prev = mesh.positions.clone();
        for v in 0..prev.len() {
            if sharp[v] {
                continue;
            }
            let nb = mesh.neighbors(v as u32);
            if nb.is_empty() {
                continue;
            }
            let avg: Vec3 = nb.iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nb.len() as f32;
            let mut p = prev[v].lerp(avg, 0.5);
            // back onto the surface (two Newton steps)
            for _ in 0..2 {
                let g = gradient(f, p, h * 0.25);
                p -= g * f(p);
            }
            mesh.positions[v] = p;
        }
    }
    mesh.compute_all_normals();
}

/// Builds the solid for an image, about two units across, facing +Z.
pub fn build(w: usize, h: usize, rgba: &[u8], s: &SnapshotSettings) -> Result<Mesh, String> {
    let (mask, iw, ih) = silhouette(w, h, rgba, s);
    if !mask.iter().any(|&m| m) {
        return Err("No shape found in the image; try another threshold or Invert".into());
    }
    if mask.iter().all(|&m| m) {
        return Err("The whole image counts as the shape; try another threshold or Invert".into());
    }
    // pad so the shape never touches the border
    let pad = 4;
    let (pw, ph) = (iw + 2 * pad, ih + 2 * pad);
    let mut pm = vec![false; pw * ph];
    for y in 0..ih {
        for x in 0..iw {
            pm[(y + pad) * pw + x + pad] = mask[y * iw + x];
        }
    }
    let mut sd = signed_distance(&pm, pw, ph);
    // soften the pixel staircase of the outline (a distance field blurs well)
    for _ in 0..4 {
        let prev = sd.clone();
        for y in 1..ph - 1 {
            for x in 1..pw - 1 {
                let mut acc = 0.0;
                for dy in [-1i32, 0, 1] {
                    for dx in [-1i32, 0, 1] {
                        acc += prev[((y as i32 + dy) as usize) * pw + (x as i32 + dx) as usize];
                    }
                }
                sd[y * pw + x] = acc / 9.0;
            }
        }
    }
    let px = 2.0 / iw.max(ih) as f32; // world size of a pixel
    let dmax = sd.iter().copied().fold(0.0f32, |a, d| a.max(-d)) * px;
    // deep shapes are built shallow and stretched, so the grid stays small
    let wanted = s.depth.clamp(0.02, 8.0);
    let half_depth = wanted.min(0.6);
    let sample = |x: f32, y: f32| -> f32 {
        // world -> pixel (y up), bilinear
        let fx = x / px + pw as f32 * 0.5 - 0.5;
        let fy = ph as f32 * 0.5 - y / px - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |xi: f32, yi: f32| -> f32 {
            if xi < 0.0 || yi < 0.0 || xi >= pw as f32 || yi >= ph as f32 {
                return pad as f32;
            }
            sd[yi as usize * pw + xi as usize]
        };
        let a = at(x0, y0) * (1.0 - tx) + at(x0 + 1.0, y0) * tx;
        let b = at(x0, y0 + 1.0) * (1.0 - tx) + at(x0 + 1.0, y0 + 1.0) * tx;
        (a * (1.0 - ty) + b * ty) * px
    };
    let style = s.style;
    let field = move |p: Vec3| -> f32 {
        let d2 = sample(p.x, p.y);
        let hz = match style {
            Style::Extrude => half_depth,
            Style::Inflate => {
                let t = ((-d2) / dmax.max(1e-6)).clamp(0.0, 1.0);
                half_depth * (1.0 - (1.0 - t) * (1.0 - t)).sqrt()
            }
        };
        d2.max(p.z.abs() - hz)
    };
    let hcell = 2.0 / s.resolution.clamp(32, 400) as f32;
    let ext = Vec3::new(pw as f32 * px * 0.5, ph as f32 * px * 0.5, half_depth + 3.0 * hcell);
    let mut m = surface_nets(-ext, ext, hcell, &field)?;
    relax(&mut m, s.smooth, &field, hcell);
    if wanted > half_depth {
        let k = wanted / half_depth;
        for p in &mut m.positions {
            p.z *= k;
        }
        m.compute_all_normals();
    }
    Ok(m)
}

/// Places a snapshot solid (built facing +Z, two units across) so it faces
/// along `facing`, `size` units across, centred on `center`.
pub fn place(m: &mut Mesh, center: Vec3, size: f32, basis: Mat3) {
    let k = size * 0.5;
    for p in &mut m.positions {
        *p = center + basis * (*p * k);
    }
    m.compute_all_normals();
}

/// One mesh holding both parts; with `subtract`, `b` is turned inside out
/// so a voxel remesh cuts it away from `a`.
pub fn combine(a: &Mesh, b: &Mesh, subtract: bool) -> Mesh {
    let n = a.positions.len() as u32;
    let mut pos = a.positions.clone();
    pos.extend(b.positions.iter().copied());
    let mut tris: Vec<[u32; 3]> = a.tris.iter().copied().filter(|t| t[0] != crate::mesh::NONE).collect();
    for t in b.tris.iter().filter(|t| t[0] != crate::mesh::NONE) {
        let t = [t[0] + n, t[1] + n, t[2] + n];
        tris.push(if subtract { [t[0], t[2], t[1]] } else { t });
    }
    let mut m = MeshBuilder::new(pos, tris).finish();
    m.export_center = a.export_center;
    m.export_scale = a.export_scale;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc_image(n: usize) -> Vec<u8> {
        let mut img = vec![255u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                let (dx, dy) = (x as f32 - n as f32 / 2.0, y as f32 - n as f32 / 2.0);
                if dx * dx + dy * dy < (n as f32 * 0.3).powi(2) {
                    img[(y * n + x) * 4..][..3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        img
    }

    #[test]
    fn distance_transform_is_exact_on_a_line() {
        let w = 9;
        let mask: Vec<bool> = (0..w).map(|x| x == 4).collect();
        let d = edt(&mask, w, 1);
        assert_eq!(d, vec![4.0, 3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn disc_becomes_a_closed_lens() {
        let img = disc_image(64);
        for style in [Style::Extrude, Style::Inflate] {
            let s = SnapshotSettings { style, resolution: 64, ..Default::default() };
            let m = build(64, 64, &img, &s).unwrap();
            let mut c: std::collections::HashMap<(u32, u32), i32> = Default::default();
            for t in &m.tris {
                for e in 0..3 {
                    *c.entry((t[e], t[(e + 1) % 3])).or_default() += 1;
                }
            }
            let bad = c.iter().filter(|(&(a, b), &n)| n != 1 || c.get(&(b, a)) != Some(&1)).count();
            assert!(bad * 100 < c.len(), "{style:?}: {bad} bad edges");
            let (mn, mx) = m.bounds();
            // radius 0.3 of the image width (2 units) = 0.6
            assert!((mx.x - 0.6).abs() < 0.08 && (mn.x + 0.6).abs() < 0.08, "{mn:?} {mx:?}");
            assert!(mx.z > 0.15 && mx.z < 0.3, "{style:?} depth {}", mx.z);
            // outward: the front-most vertex's normal faces +z
            let front = (0..m.positions.len()).max_by(|&a, &b| m.positions[a].z.total_cmp(&m.positions[b].z)).unwrap();
            assert!(m.normals[front].z > 0.5);
        }
    }

    #[test]
    fn extruded_edges_stay_sharp_and_round_outlines_smooth() {
        let img = disc_image(96);
        let s = SnapshotSettings { style: Style::Extrude, resolution: 96, ..Default::default() };
        let m = build(96, 96, &img, &s).unwrap();
        let (_, mx) = m.bounds();
        // vertices on the flat front reach right out to the rim (not chamfered)
        let front_r = m.positions.iter().filter(|p| (p.z - mx.z).abs() < 0.004).map(|p| (p.x * p.x + p.y * p.y).sqrt()).fold(0.0f32, f32::max);
        assert!(front_r > 0.56, "front face reaches {front_r}");
        // few faces slope between the front and the side: the rim isn't bevelled
        let slanted = (0..m.tris.len())
            .filter(|&f| {
                let nz = m.face_normals[f].normalize_or_zero().z.abs();
                nz > 0.35 && nz < 0.94
            })
            .count();
        assert!(slanted * 100 < m.tris.len() * 3, "{slanted} of {} faces bevelled", m.tris.len());
        // the side wall is round: rim radii vary little
        let ring: Vec<Vec3> = m.positions.iter().copied().filter(|p| p.z.abs() < 0.05).collect();
        let c = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
        let side: Vec<f32> = ring.iter().map(|p| ((p.x - c.x).powi(2) + (p.y - c.y).powi(2)).sqrt()).collect();
        let mean = side.iter().sum::<f32>() / side.len() as f32;
        let dev = (side.iter().map(|r| (r - mean) * (r - mean)).sum::<f32>() / side.len() as f32).sqrt();
        assert!(dev < 0.0035, "rim wobble {dev}");
    }

    #[test]
    fn deep_shapes_are_stretched() {
        let img = disc_image(48);
        let s = SnapshotSettings { style: Style::Extrude, resolution: 48, depth: 3.0, ..Default::default() };
        let m = build(48, 48, &img, &s).unwrap();
        let (mn, mx) = m.bounds();
        assert!((mx.z - 3.0).abs() < 0.3 && (mn.z + 3.0).abs() < 0.3, "{mn:?} {mx:?}");
    }

    #[test]
    fn blank_image_is_an_error() {
        let img = vec![255u8; 16 * 16 * 4];
        assert!(build(16, 16, &img, &SnapshotSettings::default()).is_err());
    }
}
