//! Baking: normal, displacement and colour maps from the highest level
//! onto a lower level's texture coordinates, done a slice of texels at a
//! time so the interface can show progress.

use crate::bvh::Bvh;
use crate::mesh::{Mesh, NONE};
use crate::sculpt::Sculptor;
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BakeSettings {
    /// Texture size in pixels (square).
    pub size: usize,
    /// How far from the low surface detail is searched, as a fraction of
    /// the model size.
    pub reach: f32,
}

impl Default for BakeSettings {
    fn default() -> Self {
        Self { size: 1024, reach: 0.03 }
    }
}

/// The baked images (row 0 is the top of the image, V = 1).
pub struct Maps {
    pub size: usize,
    /// Tangent-space normals, RGB.
    pub normal: Vec<u8>,
    /// Displacement, 16-bit: 32768 = on the low surface, ±32767 = ±`disp_range`.
    pub displacement: Vec<u16>,
    pub disp_range: f32,
    /// Polypaint, RGBA (None if the sculpt isn't painted).
    pub color: Option<Vec<u8>>,
}

struct Texel {
    tri: u32,
    bary: Vec3,
}

/// A bake in progress.
pub struct Baker {
    low: Mesh,
    high: Mesh,
    bvh: Bvh,
    size: usize,
    reach: f32,
    texels: Vec<(u32, Texel)>,
    tangents: Vec<(Vec3, Vec3)>,
    pub done: usize,
    normal: Vec<u8>,
    disp: Vec<f32>,
    color: Option<Vec<u8>>,
    filled: Vec<bool>,
}

impl Baker {
    /// `low` must have texture coordinates; `high` is the detailed surface.
    pub fn new(low: Mesh, high: Mesh, s: &BakeSettings) -> Result<Self, String> {
        if low.uvs.len() != low.tris.len() || low.tris.is_empty() {
            return Err("Unwrap UVs first (Texture folder › Unwrap)".into());
        }
        let size = s.size.clamp(64, 8192);
        let (mn, mx) = high.bounds();
        let reach = (mx - mn).max_element() * s.reach.max(1e-4);
        // which triangle (and where in it) each texel centre lies in
        let mut owner: Vec<Option<Texel>> = (0..size * size).map(|_| None).collect();
        let fs = size as f32;
        for (f, t) in low.tris.iter().enumerate() {
            if t[0] == NONE {
                continue;
            }
            let uv = low.uvs[f].map(|u| Vec2::new(u.x * fs, (1.0 - u.y) * fs));
            let (lo, hi) = (uv[0].min(uv[1]).min(uv[2]), uv[0].max(uv[1]).max(uv[2]));
            let area = (uv[1] - uv[0]).perp_dot(uv[2] - uv[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            for y in (lo.y.floor().max(0.0) as usize)..=(hi.y.ceil().min(fs - 1.0) as usize) {
                for x in (lo.x.floor().max(0.0) as usize)..=(hi.x.ceil().min(fs - 1.0) as usize) {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = (uv[1] - p).perp_dot(uv[2] - p) / area;
                    let w1 = (uv[2] - p).perp_dot(uv[0] - p) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                        owner[y * size + x] = Some(Texel { tri: f as u32, bary: Vec3::new(w0, w1, w2) });
                    }
                }
            }
        }
        let texels: Vec<(u32, Texel)> = owner.into_iter().enumerate().filter_map(|(i, t)| t.map(|t| (i as u32, t))).collect();
        // a tangent frame per triangle from its UV directions
        let tangents = low
            .tris
            .iter()
            .enumerate()
            .map(|(f, t)| {
                if t[0] == NONE {
                    return (Vec3::X, Vec3::Y);
                }
                let (p0, p1, p2) = (low.positions[t[0] as usize], low.positions[t[1] as usize], low.positions[t[2] as usize]);
                let [u0, u1, u2] = low.uvs[f];
                let (e1, e2) = (p1 - p0, p2 - p0);
                let (d1, d2) = (u1 - u0, u2 - u0);
                let r = d1.x * d2.y - d2.x * d1.y;
                if r.abs() < 1e-20 {
                    let n = e1.cross(e2).normalize_or(Vec3::Z);
                    let t = n.any_orthonormal_vector();
                    return (t, n.cross(t));
                }
                let tan = (e1 * d2.y - e2 * d1.y) / r;
                let bit = (e2 * d1.x - e1 * d2.x) / r;
                (tan, bit)
            })
            .collect();
        let bvh = Bvh::build(&high);
        let painted = high.colors.len() == high.positions.len();
        Ok(Self {
            normal: vec![0; size * size * 3],
            disp: vec![f32::NAN; size * size],
            color: painted.then(|| vec![0; size * size * 4]),
            filled: vec![false; size * size],
            low,
            high,
            bvh,
            size,
            reach,
            texels,
            tangents,
            done: 0,
        })
    }

    pub fn total(&self) -> usize {
        self.texels.len()
    }

    pub fn progress(&self) -> f32 {
        self.done as f32 / self.texels.len().max(1) as f32
    }

    /// Bakes up to `count` more texels; true when every texel is done.
    pub fn step(&mut self, count: usize) -> bool {
        let end = (self.done + count).min(self.texels.len());
        for k in self.done..end {
            let (i, ref tx) = self.texels[k];
            let t = self.low.tris[tx.tri as usize];
            let b = tx.bary;
            let p = self.low.positions[t[0] as usize] * b.x + self.low.positions[t[1] as usize] * b.y + self.low.positions[t[2] as usize] * b.z;
            let n = (self.low.normals[t[0] as usize] * b.x + self.low.normals[t[1] as usize] * b.y + self.low.normals[t[2] as usize] * b.z)
                .normalize_or(Vec3::Z);
            let (tan, bit) = self.tangents[tx.tri as usize];
            let tn = (tan - n * tan.dot(n)).normalize_or(n.any_orthonormal_vector());
            let bn = n.cross(tn) * if n.cross(tn).dot(bit) < 0.0 { -1.0 } else { 1.0 };
            // cast inwards from just outside the reach
            let o = p + n * self.reach;
            let hit = self.bvh.raycast(&self.high, o, -n).filter(|h| h.t <= self.reach * 2.0);
            let i = i as usize;
            match hit {
                Some(h) => {
                    let ht = self.high.tris[h.tri as usize];
                    let w = bary(&self.high, ht, h.point);
                    let hn = (self.high.normals[ht[0] as usize] * w.x + self.high.normals[ht[1] as usize] * w.y + self.high.normals[ht[2] as usize] * w.z)
                        .normalize_or(n);
                    let ts = Vec3::new(hn.dot(tn), hn.dot(bn), hn.dot(n)).normalize_or(Vec3::Z);
                    let c = ts * 0.5 + Vec3::splat(0.5);
                    self.normal[i * 3..i * 3 + 3].copy_from_slice(&[to8(c.x), to8(c.y), to8(c.z)]);
                    self.disp[i] = self.reach - h.t;
                    if let Some(col) = self.color.as_mut() {
                        let cs = [ht[0], ht[1], ht[2]].map(|v| self.high.colors[v as usize]);
                        let px: [u8; 4] = [0, 1, 2, 3].map(|ch| {
                            (cs[0][ch] as f32 * w.x + cs[1][ch] as f32 * w.y + cs[2][ch] as f32 * w.z).round().clamp(0.0, 255.0) as u8
                        });
                        col[i * 4..i * 4 + 4].copy_from_slice(&px);
                    }
                }
                None => {
                    self.normal[i * 3..i * 3 + 3].copy_from_slice(&[128, 128, 255]);
                    self.disp[i] = 0.0;
                }
            }
            self.filled[i] = true;
        }
        self.done = end;
        end >= self.texels.len()
    }

    /// Pads charts outwards (so filtering doesn't bleed the background in)
    /// and returns the maps.
    pub fn finish(mut self) -> Maps {
        let n = self.size;
        let mut filled = self.filled.clone();
        for _ in 0..8 {
            let prev = filled.clone();
            for y in 0..n {
                for x in 0..n {
                    let i = y * n + x;
                    if prev[i] {
                        continue;
                    }
                    let mut src = None;
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < n && (yy as usize) < n && prev[yy as usize * n + xx as usize] {
                            src = Some(yy as usize * n + xx as usize);
                            break;
                        }
                    }
                    if let Some(j) = src {
                        let (a, b) = (self.normal[j * 3..j * 3 + 3].to_vec(), self.disp[j]);
                        self.normal[i * 3..i * 3 + 3].copy_from_slice(&a);
                        self.disp[i] = b;
                        if let Some(c) = self.color.as_mut() {
                            let v = c[j * 4..j * 4 + 4].to_vec();
                            c[i * 4..i * 4 + 4].copy_from_slice(&v);
                        }
                        filled[i] = true;
                    }
                }
            }
        }
        for i in 0..n * n {
            if !filled[i] {
                self.normal[i * 3..i * 3 + 3].copy_from_slice(&[128, 128, 255]);
                self.disp[i] = 0.0;
            }
        }
        // (a floor so a flat bake stays mid-grey instead of magnifying float noise)
        let range = self.disp.iter().fold(0.0f32, |m, d| m.max(d.abs())).max(self.reach * 0.05).max(1e-9);
        let displacement = self.disp.iter().map(|d| (32768.0 + d / range * 32767.0).round().clamp(0.0, 65535.0) as u16).collect();
        Maps { size: n, normal: self.normal, displacement, disp_range: range, color: self.color }
    }
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn bary(m: &Mesh, t: [u32; 3], p: Vec3) -> Vec3 {
    let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let den = (d00 * d11 - d01 * d01).max(1e-20);
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    Vec3::new(1.0 - v - w, v, w)
}

impl Sculptor {
    /// The level carrying UVs (as it is) and the highest level, for baking.
    pub fn bake_meshes(&mut self) -> Result<(Mesh, Mesh), String> {
        self.end_stroke();
        let low_level = self.uv_level().ok_or("Unwrap UVs first (Texture folder › Unwrap)")?;
        let back = self.level;
        let top = self.levels.len() - 1;
        self.goto_level(top);
        let high = self.mesh.clone();
        self.goto_level(low_level);
        let mut low = self.mesh.clone();
        low.compute_all_normals();
        self.goto_level(back);
        Ok((low, high))
    }
}

/// The low level as OBJ with texture coordinates (export units).
pub fn obj_with_uvs(m: &Mesh) -> String {
    use std::fmt::Write as _;
    let mut s = String::from("# Clayform\nmtllib clayform_bake.mtl\nusemtl clayform\n");
    let k = 1.0 / m.export_scale;
    for p in &m.positions {
        let q = *p * k + m.export_center;
        let _ = writeln!(s, "v {} {} {}", q.x, q.y, q.z);
    }
    let mut vt: Vec<Vec2> = Vec::new();
    let mut index: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    let mut faces = String::new();
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE {
            continue;
        }
        let ids: Vec<usize> = (0..3)
            .map(|c| {
                let u = m.uvs[f][c];
                *index.entry((u.x.to_bits(), u.y.to_bits())).or_insert_with(|| {
                    vt.push(u);
                    vt.len()
                })
            })
            .collect();
        let _ = writeln!(faces, "f {}/{} {}/{} {}/{}", t[0] + 1, ids[0], t[1] + 1, ids[1], t[2] + 1, ids[2]);
    }
    for u in vt {
        let _ = writeln!(s, "vt {} {}", u.x, u.y);
    }
    s.push_str(&faces);
    s
}

/// A material file pointing at the baked maps.
pub fn mtl(has_color: bool) -> String {
    let mut s = String::from("newmtl clayform\nKd 0.8 0.8 0.8\nbump -bm 1 clayform_normal.png\nnorm clayform_normal.png\ndisp clayform_displacement.png\n");
    if has_color {
        s.push_str("map_Kd clayform_color.png\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{DabParams, Tool};

    #[test]
    fn bakes_a_bump_into_the_maps() {
        let mut s = Sculptor::new(crate::poly::Polys::qsphere(6).to_mesh());
        s.unwrap_uvs(&crate::uv::UnwrapSettings::default());
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        s.refresh_bvh();
        let h = s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        s.begin_stroke(false);
        for _ in 0..6 {
            s.dab(&DabParams { tool: Tool::Draw, center: h.point, radius: 0.3, intensity: 1.0, seed_tri: h.tri, ..Default::default() });
        }
        s.dab(&DabParams { tool: Tool::Paint, center: h.point, radius: 0.3, intensity: 1.0, seed_tri: h.tri, color: Vec3::new(1.0, 0.0, 0.0), ..Default::default() });
        s.end_stroke();
        let lvl = s.level();
        s.set_level(0);
        let (low, high) = s.bake_meshes().unwrap();
        assert_eq!(s.level(), 0, "returns to the level it was on");
        let _ = lvl;
        let mut b = Baker::new(low, high, &BakeSettings { size: 128, reach: 0.08 }).unwrap();
        assert!(b.total() > 128 * 128 / 4);
        while !b.step(2000) {}
        let maps = b.finish();
        let up = maps.displacement.iter().filter(|d| **d > 32768 + 6000).count();
        assert!(up > 10, "the bump shows in the displacement map ({up} texels)");
        let red = maps.color.as_ref().unwrap().chunks(4).filter(|c| c[0] > 200 && c[1] < 60 && c[3] > 120).count();
        assert!(red > 10);
        // the bump tilts normals away from straight up in tangent space
        let tilted = maps.normal.chunks(3).filter(|c| c[2] < 248).count();
        assert!(tilted > 10);
        let obj = obj_with_uvs(&s.bake_meshes().unwrap().0);
        assert!(obj.contains("\nvt ") && obj.contains("/"));
    }
}
