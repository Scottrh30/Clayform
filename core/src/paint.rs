//! Polypaint: colour per vertex, painted with the Paint brush (optionally
//! from a reference image, Spotlight-style), filled, picked, carried through
//! levels and topology changes, and saved with the project.

use crate::bvh::Bvh;
use crate::mesh::{mix_color, Mesh, NONE, NO_PAINT};
use crate::sculpt::{DabParams, Sculptor, Undo};
use glam::{Mat4, Vec2, Vec3};
use std::sync::Arc;

/// A reference image as placed over the viewport, to paint colours from.
#[derive(Clone, Debug, PartialEq)]
pub struct Spotlight {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    /// Where it sits in the viewport: x, y, width, height (points).
    pub rect: [f32; 4],
}

impl Spotlight {
    /// Colour (0..1) and coverage at image coordinates `u`, `v` (0..1).
    pub fn sample(&self, u: f32, v: f32) -> Option<(Vec3, f32)> {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) || self.w == 0 || self.h == 0 {
            return None;
        }
        let x = ((u * self.w as f32) as usize).min(self.w - 1);
        let y = ((v * self.h as f32) as usize).min(self.h - 1);
        let i = (y * self.w + x) * 4;
        let c = &self.rgba[i..i + 4];
        Some((Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.0, c[3] as f32 / 255.0))
    }
}

/// A spotlight image and the camera it's seen through, for one stroke.
#[derive(Clone)]
pub struct SpotSampler {
    pub image: Arc<Spotlight>,
    pub view_proj: Mat4,
    pub size: Vec2,
}

impl SpotSampler {
    pub fn at(&self, p: Vec3) -> Option<(Vec3, f32)> {
        let c = self.view_proj * p.extend(1.0);
        if c.w <= 0.0 {
            return None;
        }
        let ndc = c.truncate() / c.w;
        let sx = (ndc.x * 0.5 + 0.5) * self.size.x;
        let sy = (0.5 - ndc.y * 0.5) * self.size.y;
        let r = self.image.rect;
        self.image.sample((sx - r[0]) / r[2].max(1e-3), (sy - r[1]) / r[3].max(1e-3))
    }
}

fn to_rgb(c: [u8; 4]) -> Vec3 {
    Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.0
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The colour (0..1) a vertex shows: paint over `base` by its coverage.
pub fn shown_color(c: [u8; 4], base: Vec3) -> Vec3 {
    base.lerp(to_rgb(c), c[3] as f32 / 255.0)
}

/// When a level is rebuilt from the one below: the even vertices take the
/// lower level's paint where it changed, and new points between changed
/// ones are re-averaged. The higher level's own finer paint is kept.
pub(crate) fn carry_paint_up(coarse: &Mesh, fine: &mut Mesh, odd: &[[u32; 4]]) {
    let n = coarse.positions.len();
    if coarse.colors.len() != n {
        return;
    }
    if fine.colors.len() != fine.positions.len() {
        let mut c = coarse.colors.clone();
        c.extend(odd.iter().map(|s| mix_color(coarse.colors[s[0] as usize], coarse.colors[s[1] as usize])));
        fine.colors = c;
        return;
    }
    let changed: Vec<bool> = (0..n).map(|i| fine.colors[i] != coarse.colors[i]).collect();
    if !changed.iter().any(|c| *c) {
        return;
    }
    fine.colors[..n].copy_from_slice(&coarse.colors);
    for (i, s) in odd.iter().enumerate() {
        if changed[s[0] as usize] || changed[s[1] as usize] {
            fine.colors[n + i] = mix_color(coarse.colors[s[0] as usize], coarse.colors[s[1] as usize]);
        }
    }
}

/// Barycentric colour of `src` at `p` on face `t`.
fn color_on_face(src: &Mesh, t: u32, p: Vec3) -> [u8; 4] {
    let [a, b, c] = src.tris[t as usize];
    let (pa, pb, pc) = (src.positions[a as usize], src.positions[b as usize], src.positions[c as usize]);
    let v0 = pb - pa;
    let v1 = pc - pa;
    let v2 = p - pa;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let den = (d00 * d11 - d01 * d01).max(1e-20);
    let v = ((d11 * d20 - d01 * d21) / den).clamp(0.0, 1.0);
    let w = ((d00 * d21 - d01 * d20) / den).clamp(0.0, 1.0);
    let u = (1.0 - v - w).max(0.0);
    let ca = src.colors[a as usize];
    let cb = src.colors[b as usize];
    let cc = src.colors[c as usize];
    [0, 1, 2, 3].map(|k| (ca[k] as f32 * u + cb[k] as f32 * v + cc[k] as f32 * w).round().clamp(0.0, 255.0) as u8)
}

/// Copies `src`'s paint onto `dst` (nearest surface point).
pub fn transfer_colors(dst: &mut Mesh, src: &Mesh, bvh: &Bvh) {
    if src.colors.len() != src.positions.len() {
        return;
    }
    let (mn, mx) = src.bounds();
    let reach = (mx - mn).length() * 0.1;
    dst.colors = dst
        .positions
        .iter()
        .map(|&p| match bvh.closest(src, p, reach) {
            Some((q, t)) => color_on_face(src, t, q),
            None => NO_PAINT,
        })
        .collect();
}

impl Sculptor {
    pub fn is_painted(&self) -> bool {
        self.mesh.colors.len() == self.mesh.positions.len()
    }

    /// Gives a replacement mesh this one's paint (if it has none of its own).
    pub(crate) fn carry_paint_to(&mut self, mesh: &mut Mesh) {
        if !self.is_painted() || mesh.colors.len() == mesh.positions.len() || self.mesh.tris.is_empty() {
            return;
        }
        self.refresh_bvh();
        transfer_colors(mesh, &self.mesh, &self.bvh);
    }

    fn ensure_colors(&mut self) {
        if !self.is_painted() {
            self.mesh.colors = vec![NO_PAINT; self.mesh.positions.len()];
        }
    }

    /// The Paint brush: blends the colour (or the spotlight image) into the
    /// region; Negative erases paint.
    pub(crate) fn paint_dab(&mut self, p: &DabParams) {
        self.ensure_colors();
        let k0 = p.intensity.clamp(0.0, 1.0);
        let spot = self.tip.spot.clone();
        let Some(st) = self.stroke.as_mut() else { return };
        st.paint = true;
        let mesh = &mut self.mesh;
        for &(v, w) in &self.region {
            let vi = v as usize;
            let (color, cover) = match &spot {
                Some(s) => match s.at(mesh.positions[vi]) {
                    Some(c) => c,
                    None => continue,
                },
                None => (p.color, 1.0),
            };
            let k = (w * k0 * cover).clamp(0.0, 1.0);
            if k <= 0.0 {
                continue;
            }
            if st.mark_paint(v) {
                st.old_col.push(mesh.colors[vi]);
            }
            let old = mesh.colors[vi];
            let a = old[3] as f32 / 255.0;
            mesh.colors[vi] = if p.negative {
                [old[0], old[1], old[2], to_u8(a * (1.0 - k))]
            } else {
                // an unpainted point takes the colour straight away (its
                // coverage still grows smoothly)
                let from = if old[3] == 0 { color } else { to_rgb(old) };
                let c = from.lerp(color, k);
                [to_u8(c.x), to_u8(c.y), to_u8(c.z), to_u8(a + (1.0 - a) * k)]
            };
            self.dirty.add(v);
        }
    }

    /// Paints every unmasked point with `color` (one undo step).
    pub fn fill_color(&mut self, color: Vec3) {
        self.end_stroke();
        let old = self.mesh.colors.clone();
        self.ensure_colors();
        for v in 0..self.mesh.positions.len() {
            let k = 1.0 - self.mesh.mask[v];
            if k <= 0.0 {
                continue;
            }
            let o = self.mesh.colors[v];
            let from = if o[3] == 0 { color } else { to_rgb(o) };
            let c = from.lerp(color, k);
            let a = o[3] as f32 / 255.0;
            self.mesh.colors[v] = [to_u8(c.x), to_u8(c.y), to_u8(c.z), to_u8(a + (1.0 - a) * k)];
        }
        self.set_label("Fill colour");
        self.push_undo(Undo::AllColors(old));
        self.dirty.all(self.mesh.positions.len());
    }

    /// Removes all paint from the active level (one undo step).
    pub fn clear_paint(&mut self) {
        self.end_stroke();
        if self.mesh.colors.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.mesh.colors);
        self.set_label("Clear paint");
        self.push_undo(Undo::AllColors(old));
        self.dirty.all(self.mesh.positions.len());
    }

    /// The paint under a surface point (None where there is none).
    pub fn paint_at(&self, tri: u32, p: Vec3) -> Option<Vec3> {
        if !self.is_painted() || self.mesh.tris.get(tri as usize).is_none_or(|t| t[0] == NONE) {
            return None;
        }
        let c = color_on_face(&self.mesh, tri, p);
        (c[3] > 8).then(|| to_rgb(c))
    }

    pub(crate) fn export_colors(&self, out: &mut Vec<u8>) {
        let per_level: Vec<&[[u8; 4]]> = (0..self.levels.len())
            .map(|k| if k == self.level { self.mesh.colors.as_slice() } else { self.levels[k].mesh.as_ref().map_or(&[][..], |m| m.colors.as_slice()) })
            .collect();
        if per_level.iter().all(|c| c.is_empty()) {
            return;
        }
        let mut b = Vec::new();
        crate::detail::put_u32(&mut b, per_level.len() as u32);
        for c in per_level {
            crate::detail::put_u32(&mut b, c.len() as u32);
            for px in c {
                b.extend_from_slice(px);
            }
        }
        crate::detail::put_chunk(out, b"COLR", &b);
    }

    pub(crate) fn import_colors(&mut self, body: &[u8]) {
        let mut r = crate::detail::Reader { b: body, at: 0 };
        let Some(n) = r.u32() else { return };
        for k in 0..n as usize {
            let Some(c) = r.u32() else { return };
            let Some(raw) = r.take(c as usize * 4) else { return };
            let colors: Vec<[u8; 4]> = raw.chunks_exact(4).map(|x| [x[0], x[1], x[2], x[3]]).collect();
            let mesh = if k == self.level { Some(&mut self.mesh) } else { self.levels.get_mut(k).and_then(|l| l.mesh.as_mut()) };
            if let Some(m) = mesh {
                if colors.len() == m.positions.len() {
                    m.colors = colors;
                }
            }
        }
        self.dirty.all(self.mesh.positions.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::Tool;

    fn paint(s: &mut Sculptor, color: Vec3, negative: bool) {
        s.refresh_bvh();
        let h = s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        s.begin_stroke(false);
        s.dab(&DabParams { tool: Tool::Paint, center: h.point, radius: 0.4, intensity: 1.0, negative, seed_tri: h.tri, color, ..Default::default() });
        s.end_stroke();
    }

    #[test]
    fn paint_undo_fill_and_levels() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = s.mesh.positions.clone();
        paint(&mut s, Vec3::new(1.0, 0.0, 0.0), false);
        assert!(s.is_painted());
        assert_eq!(s.mesh.positions, before, "painting doesn't move the surface");
        let h = s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        let c = s.paint_at(h.tri, h.point).unwrap();
        assert!(c.x > 0.9 && c.y < 0.1);
        s.undo();
        assert!(s.mesh.colors.iter().all(|c| c[3] == 0));
        s.redo();
        // erase
        paint(&mut s, Vec3::ONE, true);
        assert!(s.paint_at(h.tri, h.point).is_none());
        s.undo();
        // carried up through subdivision and back down
        s.subdivide().unwrap();
        assert!(s.is_painted());
        let red = s.mesh.colors.iter().filter(|c| c[3] > 200 && c[0] > 200 && c[1] < 50).count();
        assert!(red > 50);
        s.set_level(0);
        s.fill_color(Vec3::new(0.0, 0.0, 1.0));
        s.set_level(1);
        assert!(s.mesh.colors.iter().all(|c| c[2] > 200 && c[3] == 255), "lower-level fill reaches the top level");
        // remeshing keeps the paint
        let remeshed = crate::remesh::voxel_remesh(&s.mesh, &crate::remesh::VoxelSettings { resolution: 48, ..Default::default() }).unwrap();
        s.set_mesh(remeshed);
        assert!(s.is_painted() && s.mesh.colors.iter().all(|c| c[2] > 150));
        // and the project
        let bytes = s.export_extras();
        let mut t = Sculptor::new(s.mesh.clone());
        t.mesh.colors.clear();
        t.import_extras(&bytes);
        assert_eq!(t.mesh.colors, s.mesh.colors);
        s.clear_paint();
        assert!(!s.is_painted());
        s.undo();
        assert!(s.is_painted());
    }

    #[test]
    fn spotlight_paints_from_the_image() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        // a 2x1 image: left half green, right half blue, covering the view
        let img = Spotlight { w: 2, h: 1, rgba: vec![0, 255, 0, 255, 0, 0, 255, 255], rect: [0.0, 0.0, 800.0, 600.0] };
        let cam = crate::camera::Camera::default();
        let size = Vec2::new(800.0, 600.0);
        s.tip.spot = Some(SpotSampler { image: Arc::new(img), view_proj: cam.proj(size.x / size.y) * cam.view(), size });
        s.refresh_bvh();
        let (o, d) = cam.ray(Vec2::new(300.0, 300.0), size);
        let h = s.raycast(o, d).unwrap();
        s.begin_stroke(false);
        s.dab(&DabParams { tool: Tool::Paint, center: h.point, radius: 0.15, intensity: 1.0, seed_tri: h.tri, ..Default::default() });
        s.end_stroke();
        let c = s.paint_at(h.tri, h.point).unwrap();
        assert!(c.y > 0.9 && c.z < 0.1, "{c:?}");
    }
}
