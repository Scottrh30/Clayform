//! FiberMesh: hair or fur strands grown from the masked part of the
//! surface, as tapered tubes in a mesh of their own.

use crate::mesh::{Mesh, MeshBuilder, NONE};
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FiberSettings {
    /// How many strands.
    pub count: u32,
    /// Length, as a fraction of the model size.
    pub length: f32,
    /// Root thickness, as a fraction of the length.
    pub thickness: f32,
    /// Droop under gravity (0 = straight out).
    pub gravity: f32,
    /// Random variation of length and direction (0..1).
    pub random: f32,
    /// Bends a strand makes along its length.
    pub segments: u32,
    /// Tip thickness relative to the root.
    pub taper: f32,
    pub seed: u32,
}

impl Default for FiberSettings {
    fn default() -> Self {
        Self { count: 2000, length: 0.12, thickness: 0.04, gravity: 0.5, random: 0.4, segments: 6, taper: 0.1, seed: 1 }
    }
}

/// Grows strands from points weighted by the mask (fully masked = most).
/// Returns None if nothing is masked.
pub fn grow(m: &Mesh, s: &FiberSettings) -> Option<Mesh> {
    // area × mask per face, for picking roots
    let mut cdf = Vec::new();
    let mut faces = Vec::new();
    let mut total = 0.0;
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE {
            continue;
        }
        let w = t.iter().map(|&v| m.mask[v as usize]).sum::<f32>() / 3.0;
        if w <= 0.05 {
            continue;
        }
        total += m.face_normal_raw(f).length() * 0.5 * w;
        cdf.push(total);
        faces.push(f);
    }
    if faces.is_empty() {
        return None;
    }
    let (mn, mx) = m.bounds();
    let size = (mx - mn).max_element();
    let mut rng = crate::brush::Rng::new(s.seed as u64 + 17);
    let sides = 4usize;
    let segs = s.segments.clamp(1, 16) as usize;
    let mut pos: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut partner: Vec<u32> = Vec::new();
    for _ in 0..s.count.min(100_000) {
        let r = rng.f() * total;
        let k = cdf.partition_point(|&c| c < r).min(faces.len() - 1);
        let f = faces[k];
        let t = m.tris[f];
        let (mut a, mut b) = (rng.f(), rng.f());
        if a + b > 1.0 {
            a = 1.0 - a;
            b = 1.0 - b;
        }
        let root = m.positions[t[0] as usize] * (1.0 - a - b) + m.positions[t[1] as usize] * a + m.positions[t[2] as usize] * b;
        let n = m.face_normal_raw(f).normalize_or(Vec3::Y);
        let jitter = Vec3::new(rng.f() - 0.5, rng.f() - 0.5, rng.f() - 0.5) * s.random;
        let mut dir = (n + jitter).normalize_or(n);
        let len = size * s.length * (1.0 - s.random * 0.5 * rng.f());
        let step = len / segs as f32;
        // the strand's spine, bending down a little at every segment
        let mut spine = vec![root];
        let mut p = root;
        for _ in 0..segs {
            dir = (dir + Vec3::NEG_Y * s.gravity * 0.35).normalize_or(dir);
            p += dir * step;
            spine.push(p);
        }
        let r0 = len * s.thickness;
        let base = pos.len() as u32;
        for (i, &c) in spine.iter().enumerate() {
            let along = if i + 1 < spine.len() { spine[i + 1] - c } else { c - spine[i - 1] };
            let ax = along.normalize_or(n);
            let u = ax.any_orthonormal_vector();
            let v = ax.cross(u);
            let rr = r0 * (1.0 - (1.0 - s.taper) * i as f32 / segs as f32);
            for k in 0..sides {
                let ang = k as f32 / sides as f32 * std::f32::consts::TAU;
                pos.push(c + (u * ang.cos() + v * ang.sin()) * rr);
            }
        }
        for i in 0..segs {
            for k in 0..sides {
                let a0 = base + (i * sides + k) as u32;
                let a1 = base + (i * sides + (k + 1) % sides) as u32;
                let b0 = a0 + sides as u32;
                let b1 = a1 + sides as u32;
                let ti = tris.len() as u32;
                tris.push([a0, a1, b1]);
                tris.push([a0, b1, b0]);
                partner.extend([ti + 1, ti]);
            }
        }
        // close both ends
        for (ring, flip) in [(0usize, true), (segs, false)] {
            let r = base + (ring * sides) as u32;
            for (x, y, z) in [(0u32, 1u32, 2u32), (0, 2, 3)] {
                tris.push(if flip { [r + x, r + z, r + y] } else { [r + x, r + y, r + z] });
                partner.push(NONE);
            }
            let l = partner.len() as u32;
            partner[l as usize - 2] = l - 1;
            partner[l as usize - 1] = l - 2;
        }
    }
    let mut out = MeshBuilder::new(pos, tris).with_faces(None, Some(partner)).finish();
    out.export_center = m.export_center;
    out.export_scale = m.export_scale;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strands_grow_from_the_mask_and_droop() {
        let mut m = Mesh::icosphere(4);
        assert!(grow(&m, &FiberSettings::default()).is_none(), "nothing masked");
        for (v, p) in m.positions.iter().enumerate() {
            m.mask[v] = if p.y > 0.7 { 1.0 } else { 0.0 };
        }
        let s = FiberSettings { count: 100, ..Default::default() };
        let f = grow(&m, &s).unwrap();
        assert_eq!(f.positions.len(), 100 * (s.segments as usize + 1) * 4);
        assert!(!f.boundary.iter().any(|b| *b), "every strand is closed");
        // roots on the cap
        let roots_on_cap = f.positions.iter().step_by((s.segments as usize + 1) * 4).filter(|p| p.y > 0.6).count();
        assert_eq!(roots_on_cap, 100);
    }
}
