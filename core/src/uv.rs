//! Automatic UV unwrapping (like UV Master): the surface is split into
//! charts of faces facing roughly one way, each flattened with least-squares
//! conformal maps (LSCM), scaled to its true size and packed into the square.

use crate::mesh::{Mesh, NONE};
use crate::sculpt::Sculptor;
use glam::{Vec2, Vec3};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UnwrapSettings {
    /// Faces in one chart stay within this angle of its first face (degrees).
    pub max_angle: f32,
    /// Charts never cross polygroup borders.
    pub by_groups: bool,
    /// Space between charts, as a fraction of the square.
    pub margin: f32,
}

impl Default for UnwrapSettings {
    fn default() -> Self {
        Self { max_angle: 66.0, by_groups: false, margin: 0.006 }
    }
}

/// Groups faces into charts. Quad halves stay together.
fn charts(m: &Mesh, s: &UnwrapSettings) -> Vec<Vec<u32>> {
    let nt = m.tris.len();
    let mut edge_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE {
            continue;
        }
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(f as u32);
        }
    }
    let cos = s.max_angle.to_radians().cos();
    let mut chart_of = vec![u32::MAX; nt];
    let mut out: Vec<Vec<u32>> = Vec::new();
    // seed from the largest faces first so charts grow from flat areas
    let mut order: Vec<u32> = (0..nt as u32).filter(|&f| m.tris[f as usize][0] != NONE).collect();
    let area = |f: u32| m.face_normal_raw(f as usize).length();
    order.sort_by(|a, b| area(*b).total_cmp(&area(*a)));
    for seed in order {
        if chart_of[seed as usize] != u32::MAX {
            continue;
        }
        let id = out.len() as u32;
        let n0 = m.face_normals[seed as usize].normalize_or_zero();
        let g0 = m.groups[seed as usize];
        let mut chart = Vec::new();
        let mut stack = vec![seed];
        chart_of[seed as usize] = id;
        while let Some(f) = stack.pop() {
            chart.push(f);
            let t = m.tris[f as usize];
            let mut cand: Vec<u32> = Vec::with_capacity(4);
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if let Some(fs) = edge_faces.get(&(a.min(b), a.max(b))) {
                    cand.extend(fs.iter().copied().filter(|&g| g != f));
                }
            }
            for g in cand {
                if chart_of[g as usize] != u32::MAX {
                    continue;
                }
                let partner = m.partner[f as usize] == g;
                let ok = (!s.by_groups || m.groups[g as usize] == g0) && (partner || m.face_normals[g as usize].normalize_or_zero().dot(n0) >= cos);
                if ok {
                    chart_of[g as usize] = id;
                    stack.push(g);
                }
            }
        }
        out.push(chart);
    }
    out
}

/// Flattens one chart: (local vertex list, per-face local corner indices,
/// uv per local vertex).
fn flatten(m: &Mesh, faces: &[u32]) -> (Vec<u32>, Vec<[usize; 3]>, Vec<Vec2>) {
    let mut local: HashMap<u32, usize> = HashMap::new();
    let mut verts = Vec::new();
    let tris: Vec<[usize; 3]> = faces
        .iter()
        .map(|&f| {
            m.tris[f as usize].map(|v| {
                *local.entry(v).or_insert_with(|| {
                    verts.push(v);
                    verts.len() - 1
                })
            })
        })
        .collect();
    let n = verts.len();
    let pos: Vec<Vec3> = verts.iter().map(|&v| m.positions[v as usize]).collect();
    // planar projection, used to start and as the fallback
    let nrm: Vec3 = faces.iter().map(|&f| m.face_normal_raw(f as usize)).sum::<Vec3>().normalize_or(Vec3::Z);
    let t1 = nrm.any_orthonormal_vector();
    let t2 = nrm.cross(t1);
    let planar: Vec<Vec2> = pos.iter().map(|p| Vec2::new(p.dot(t1), p.dot(t2))).collect();
    if n < 4 || faces.len() < 2 {
        return (verts, tris, planar);
    }
    // pin the two vertices farthest apart (in the planar view)
    let (mut pa, mut pb) = (0usize, 0usize);
    for _ in 0..2 {
        let far = |from: usize| (0..n).max_by(|&x, &y| planar[x].distance_squared(planar[from]).total_cmp(&planar[y].distance_squared(planar[from]))).unwrap();
        pa = far(pb);
        pb = far(pa);
    }
    if pa == pb {
        return (verts, tris, planar);
    }
    let pin_a = planar[pa];
    let pin_b = planar[pb];
    // unknowns: u then v of every free vertex
    let mut idx = vec![usize::MAX; n];
    let mut free = 0;
    for i in 0..n {
        if i != pa && i != pb {
            idx[i] = free;
            free += 1;
        }
    }
    let unknowns = free * 2;
    // rows of the least-squares system: (entries, rhs)
    let mut rows: Vec<(Vec<(usize, f32)>, f32)> = Vec::with_capacity(tris.len() * 2);
    for t in &tris {
        let (p0, p1, p2) = (pos[t[0]], pos[t[1]], pos[t[2]]);
        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let x1 = e1.length();
        if x1 < 1e-12 {
            continue;
        }
        let ax = e1 / x1;
        let ay = (e2 - ax * e2.dot(ax)).normalize_or_zero();
        let (x2, y2) = (e2.dot(ax), e2.dot(ay));
        let d = (x1 * y2).abs();
        if d < 1e-14 {
            continue;
        }
        let w = 1.0 / d.sqrt();
        // W_j for corners (0, 1, 2) of the local triangle (0,0), (x1,0), (x2,y2)
        let wr = [(x2 - x1) * w, (0.0 - x2) * w, (x1 - 0.0) * w];
        let wi = [(y2 - 0.0) * w, (0.0 - y2) * w, 0.0];
        for row in 0..2 {
            let mut e = Vec::with_capacity(6);
            let mut rhs = 0.0;
            for j in 0..3 {
                let v = t[j];
                // row 0: Re(W u) = wr*u - wi*v ; row 1: Im = wi*u + wr*v
                let (cu, cv) = if row == 0 { (wr[j], -wi[j]) } else { (wi[j], wr[j]) };
                if v == pa || v == pb {
                    let p = if v == pa { pin_a } else { pin_b };
                    rhs -= cu * p.x + cv * p.y;
                } else {
                    e.push((idx[v] * 2, cu));
                    e.push((idx[v] * 2 + 1, cv));
                }
            }
            rows.push((e, rhs));
        }
    }
    // conjugate gradients on the normal equations, from the planar guess
    let mut x = vec![0.0f32; unknowns];
    for i in 0..n {
        if idx[i] != usize::MAX {
            x[idx[i] * 2] = planar[i].x;
            x[idx[i] * 2 + 1] = planar[i].y;
        }
    }
    let ata = |x: &[f32], out: &mut Vec<f32>| {
        out.iter_mut().for_each(|o| *o = 0.0);
        for (e, _) in &rows {
            let r: f32 = e.iter().map(|&(k, c)| c * x[k]).sum();
            for &(k, c) in e {
                out[k] += c * r;
            }
        }
    };
    let mut atb = vec![0.0f32; unknowns];
    for (e, rhs) in &rows {
        for &(k, c) in e {
            atb[k] += c * rhs;
        }
    }
    let mut ax = vec![0.0f32; unknowns];
    ata(&x, &mut ax);
    let mut r: Vec<f32> = atb.iter().zip(&ax).map(|(b, a)| b - a).collect();
    let mut p = r.clone();
    let mut rs: f32 = r.iter().map(|v| v * v).sum();
    let rs0 = rs.max(1e-30);
    let mut ap = vec![0.0f32; unknowns];
    for _ in 0..(unknowns * 2).clamp(50, 1500) {
        if rs <= rs0 * 1e-12 {
            break;
        }
        ata(&p, &mut ap);
        let pap: f32 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-30 {
            break;
        }
        let alpha = rs / pap;
        for k in 0..unknowns {
            x[k] += alpha * p[k];
            r[k] -= alpha * ap[k];
        }
        let rs_new: f32 = r.iter().map(|v| v * v).sum();
        let beta = rs_new / rs;
        for k in 0..unknowns {
            p[k] = r[k] + beta * p[k];
        }
        rs = rs_new;
    }
    let uv: Vec<Vec2> = (0..n)
        .map(|i| if i == pa { pin_a } else if i == pb { pin_b } else { Vec2::new(x[idx[i] * 2], x[idx[i] * 2 + 1]) })
        .collect();
    // reject a solve that folded over or blew up
    let signed = |u: &[Vec2], t: &[usize; 3]| (u[t[1]] - u[t[0]]).perp_dot(u[t[2]] - u[t[0]]);
    let pos_area: f32 = tris.iter().map(|t| signed(&uv, t).max(0.0)).sum();
    let neg_area: f32 = tris.iter().map(|t| (-signed(&uv, t)).max(0.0)).sum();
    let ok = uv.iter().all(|u| u.is_finite()) && neg_area.min(pos_area) <= (pos_area + neg_area) * 0.01;
    (verts, tris, if ok { uv } else { planar })
}

/// Unwraps the mesh: texture coordinates for every triangle corner, all
/// inside 0..1. Returns the number of charts.
pub fn unwrap(m: &mut Mesh, s: &UnwrapSettings) -> usize {
    let charts = charts(m, s);
    struct Chart {
        faces: Vec<u32>,
        corners: Vec<[Vec2; 3]>,
        size: Vec2,
    }
    let mut packed: Vec<Chart> = Vec::with_capacity(charts.len());
    for faces in charts {
        let (_, tris, mut uv) = flatten(m, &faces);
        // true size: match the 3D area
        let a3: f32 = faces.iter().map(|&f| m.face_normal_raw(f as usize).length() * 0.5).sum();
        let a2: f32 = tris.iter().map(|t| ((uv[t[1]] - uv[t[0]]).perp_dot(uv[t[2]] - uv[t[0]])).abs() * 0.5).sum();
        let k = if a2 > 1e-20 { (a3 / a2).sqrt() } else { 1.0 };
        // mirror a chart that came out inside-out
        let flip = tris.iter().map(|t| (uv[t[1]] - uv[t[0]]).perp_dot(uv[t[2]] - uv[t[0]])).sum::<f32>() < 0.0;
        for u in uv.iter_mut() {
            *u *= k;
            if flip {
                u.x = -u.x;
            }
        }
        // turn to the smallest bounding box
        let best = (0..18)
            .map(|i| {
                let a = i as f32 * 5f32.to_radians();
                let rot = Vec2::from_angle(a);
                let (mn, mx) = uv.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(mn, mx), u| {
                    let q = rot.rotate(*u);
                    (mn.min(q), mx.max(q))
                });
                let s = mx - mn;
                (s.x * s.y, rot)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1;
        for u in uv.iter_mut() {
            *u = best.rotate(*u);
        }
        let (mn, mx) = uv.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), u| (a.min(*u), b.max(*u)));
        let mut size = mx - mn;
        // stand tall charts up so shelves pack tighter
        let tall = size.y > size.x;
        let corners = tris
            .iter()
            .map(|t| {
                t.map(|i| {
                    let q = uv[i] - mn;
                    if tall { Vec2::new(q.y, size.x - q.x) } else { q }
                })
            })
            .collect();
        if tall {
            size = Vec2::new(size.y, size.x);
        }
        packed.push(Chart { faces, corners, size });
    }
    // shelf packing, tallest first, into a square
    packed.sort_by(|a, b| b.size.y.total_cmp(&a.size.y));
    let total: f32 = packed.iter().map(|c| c.size.x * c.size.y).sum();
    let widest = packed.iter().map(|c| c.size.x).fold(0.0, f32::max);
    let mut side = (total.sqrt() * 1.15).max(widest);
    let place = |side: f32, pad: f32| -> (Vec<Vec2>, f32) {
        let (mut x, mut y, mut shelf) = (0.0f32, 0.0f32, 0.0f32);
        let mut at = Vec::with_capacity(packed.len());
        for c in &packed {
            if x + c.size.x > side && x > 0.0 {
                x = 0.0;
                y += shelf + pad;
                shelf = 0.0;
            }
            at.push(Vec2::new(x, y));
            x += c.size.x + pad;
            shelf = shelf.max(c.size.y);
        }
        (at, y + shelf)
    };
    let mut pad = side * s.margin;
    let (mut at, mut height) = place(side, pad);
    for _ in 0..20 {
        if height <= side * 1.02 {
            break;
        }
        side *= 1.05;
        pad = side * s.margin;
        let r = place(side, pad);
        at = r.0;
        height = r.1;
    }
    let scale = 1.0 / side.max(height).max(1e-12);
    m.uvs = vec![[Vec2::ZERO; 3]; m.tris.len()];
    for (c, o) in packed.iter().zip(at) {
        for (k, &f) in c.faces.iter().enumerate() {
            m.uvs[f as usize] = c.corners[k].map(|u| (u + o) * scale);
        }
    }
    packed.len()
}

/// The UV layout drawn as white edges on black (`size` × `size`, 8-bit).
pub fn layout_image(m: &Mesh, size: usize) -> Vec<u8> {
    let mut img = vec![24u8; size * size];
    if m.uvs.len() != m.tris.len() {
        return img;
    }
    let s = size as f32;
    let mut plot = |x: i32, y: i32| {
        if x >= 0 && y >= 0 && (x as usize) < size && (y as usize) < size {
            img[y as usize * size + x as usize] = 230;
        }
    };
    let step = (m.tris.len() / 60_000).max(1);
    for (f, uv) in m.uvs.iter().enumerate().step_by(step) {
        if m.tris[f][0] == NONE {
            continue;
        }
        for i in 0..3 {
            let a = uv[i] * s;
            let b = uv[(i + 1) % 3] * s;
            let n = (a.distance(b).ceil() as usize).max(1);
            for k in 0..=n {
                let q = a.lerp(b, k as f32 / n as f32);
                plot(q.x as i32, (s - 1.0 - q.y) as i32);
            }
        }
    }
    img
}

impl Sculptor {
    /// Unwraps the active level (undoable as a mesh change). Returns the chart count.
    pub fn unwrap_uvs(&mut self, s: &UnwrapSettings) -> usize {
        self.end_stroke();
        let snap = self.snapshot();
        let n = unwrap(&mut self.mesh, s);
        self.set_label("UV unwrap");
        self.push_undo(crate::sculpt::Undo::State(snap));
        n
    }

    pub fn has_uvs(&self) -> bool {
        self.mesh.uvs.len() == self.mesh.tris.len() && !self.mesh.tris.is_empty()
    }

    /// The level whose mesh carries texture coordinates (the active one first).
    pub fn uv_level(&self) -> Option<usize> {
        if self.has_uvs() {
            return Some(self.level);
        }
        (0..self.levels.len()).find(|&k| self.levels[k].mesh.as_ref().is_some_and(|m| m.uvs.len() == m.tris.len() && !m.tris.is_empty()))
    }

    pub(crate) fn export_uvs(&self, out: &mut Vec<u8>) {
        let Some(k) = self.uv_level() else { return };
        let m = if k == self.level { &self.mesh } else { self.levels[k].mesh.as_ref().unwrap() };
        let mut b = Vec::new();
        crate::detail::put_u32(&mut b, k as u32);
        crate::detail::put_u32(&mut b, m.uvs.len() as u32);
        for t in &m.uvs {
            for u in t {
                b.extend_from_slice(&u.x.to_le_bytes());
                b.extend_from_slice(&u.y.to_le_bytes());
            }
        }
        crate::detail::put_chunk(out, b"UVS0", &b);
    }

    pub(crate) fn import_uvs(&mut self, body: &[u8]) {
        let mut r = crate::detail::Reader { b: body, at: 0 };
        let (Some(k), Some(n)) = (r.u32(), r.u32()) else { return };
        let Some(raw) = r.take(n as usize * 24) else { return };
        let uvs: Vec<[Vec2; 3]> = raw
            .chunks_exact(24)
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap());
                [Vec2::new(f(0), f(1)), Vec2::new(f(2), f(3)), Vec2::new(f(4), f(5))]
            })
            .collect();
        let k = k as usize;
        let mesh = if k == self.level { Some(&mut self.mesh) } else { self.levels.get_mut(k).and_then(|l| l.mesh.as_mut()) };
        if let Some(m) = mesh {
            if uvs.len() == m.tris.len() {
                m.uvs = uvs;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_fills_the_square_without_overlap_or_flips() {
        for mut m in [Mesh::icosphere(3), crate::poly::Polys::qcube(3).to_mesh(), crate::poly::Polys::qsphere(6).to_mesh()] {
            let n = unwrap(&mut m, &UnwrapSettings::default());
            assert!(n >= 2 && n < m.tris.len() / 4, "{n} charts");
            assert_eq!(m.uvs.len(), m.tris.len());
            assert!(m.uvs.iter().flatten().all(|u| u.x >= -1e-4 && u.y >= -1e-4 && u.x <= 1.0001 && u.y <= 1.0001));
            // no folded triangles
            let flipped = m.uvs.iter().filter(|t| (t[1] - t[0]).perp_dot(t[2] - t[0]) < -1e-9).count();
            assert_eq!(flipped, 0);
            // little stretch: UV area per 3D area roughly constant
            let ratios: Vec<f32> = (0..m.tris.len())
                .map(|f| {
                    let t = m.uvs[f];
                    ((t[1] - t[0]).perp_dot(t[2] - t[0])).abs() / m.face_normal_raw(f).length()
                })
                .collect();
            let mean = ratios.iter().sum::<f32>() / ratios.len() as f32;
            let bad = ratios.iter().filter(|r| **r < mean * 0.25 || **r > mean * 4.0).count();
            assert!(bad * 50 < ratios.len(), "{bad} badly stretched of {}", ratios.len());
            // coverage: charts use a fair part of the square
            let used: f32 = m.uvs.iter().map(|t| ((t[1] - t[0]).perp_dot(t[2] - t[0])).abs() * 0.5).sum();
            assert!(used > 0.35, "covers {used}");
        }
    }

    #[test]
    fn uvs_follow_subdivision_and_projects() {
        let mut s = Sculptor::new(crate::poly::Polys::qcube(2).to_mesh());
        s.unwrap_uvs(&UnwrapSettings::default());
        s.subdivide().unwrap();
        assert!(s.has_uvs(), "subdivision carries UVs up");
        assert_eq!(s.uv_level(), Some(1));
        let bytes = s.export_extras();
        let (levels, active) = s.clone_levels();
        let mut t = Sculptor::from_levels(levels, active, None);
        t.mesh.uvs.clear();
        t.import_extras(&bytes);
        assert!(t.has_uvs());
    }
}
