//! Shadowbox: a solid carved from three silhouettes (front, side and top),
//! like ZBrush's Shadowbox: a point is inside when all three masks are.

use crate::mesh::Mesh;
use crate::volume::{mesh_field, signed_distance_2d, Grid};
use glam::Vec3;

/// Mask resolution (pixels per side).
pub const SIZE: usize = 96;

/// Three square masks over -1..1, row 0 at the top:
/// front = X right / Y up, side = Z right (towards the camera) / Y up,
/// top = X right / Z down.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadowbox {
    pub front: Vec<bool>,
    pub side: Vec<bool>,
    pub top: Vec<bool>,
}

impl Default for Shadowbox {
    fn default() -> Self {
        // a rounded block to start from
        let disc: Vec<bool> = (0..SIZE * SIZE)
            .map(|i| {
                let (x, y) = ((i % SIZE) as f32 / SIZE as f32 * 2.0 - 1.0, (i / SIZE) as f32 / SIZE as f32 * 2.0 - 1.0);
                x * x + y * y < 0.5
            })
            .collect();
        Self { front: disc.clone(), side: disc.clone(), top: disc }
    }
}

fn px(u: f32) -> f32 {
    (u * 0.5 + 0.5) * SIZE as f32 - 0.5
}

/// Bilinear sample of a signed-distance image at -1..1 coordinates (row 0 top).
fn sample(sd: &[f32], u: f32, v: f32) -> f32 {
    let x = px(u).clamp(0.0, (SIZE - 1) as f32);
    let y = px(-v).clamp(0.0, (SIZE - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(SIZE - 1), (y0 + 1).min(SIZE - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let g = |x: usize, y: usize| sd[y * SIZE + x];
    let a = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * tx;
    let b = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * tx;
    a + (b - a) * ty
}

impl Shadowbox {
    pub fn empty() -> Self {
        let e = vec![false; SIZE * SIZE];
        Self { front: e.clone(), side: e.clone(), top: e }
    }

    /// The model's silhouettes from the front, side and top (inside -1..1
    /// after fitting the model's bounds).
    pub fn from_mesh(m: &Mesh) -> Self {
        let mut s = Self::empty();
        let (mn, mx) = m.bounds();
        let c = (mn + mx) * 0.5;
        let k = 0.9 / ((mx - mn).max_element() * 0.5).max(1e-6);
        let fit = |p: Vec3| (p - c) * k;
        let fill = |img: &mut Vec<bool>, pts: [(f32, f32); 3]| {
            // rasterise the triangle into the mask
            let to = |(u, v): (f32, f32)| ((u * 0.5 + 0.5) * SIZE as f32, (0.5 - v * 0.5) * SIZE as f32);
            let p: Vec<(f32, f32)> = pts.iter().map(|q| to(*q)).collect();
            let (x0, x1) = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min), p.iter().map(|q| q.0).fold(f32::MIN, f32::max));
            let (y0, y1) = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min), p.iter().map(|q| q.1).fold(f32::MIN, f32::max));
            let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
            if area.abs() < 1e-9 {
                return;
            }
            for y in (y0.floor().max(0.0) as usize)..=(y1.ceil().min(SIZE as f32 - 1.0) as usize) {
                for x in (x0.floor().max(0.0) as usize)..=(x1.ceil().min(SIZE as f32 - 1.0) as usize) {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let e = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (sy - a.1) - (b.1 - a.1) * (sx - a.0);
                    let (w0, w1, w2) = (e(p[1], p[2]), e(p[2], p[0]), e(p[0], p[1]));
                    if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                        img[y * SIZE + x] = true;
                    }
                }
            }
        };
        for t in m.tris.iter().filter(|t| t[0] != crate::mesh::NONE) {
            let q = t.map(|v| fit(m.positions[v as usize]));
            fill(&mut s.front, q.map(|p| (p.x, p.y)));
            fill(&mut s.side, q.map(|p| (p.z, p.y)));
            fill(&mut s.top, q.map(|p| (p.x, -p.z)));
        }
        s
    }

    /// The carved solid, `res` samples across, in a box of half-size `half`.
    pub fn build(&self, res: usize, half: f32) -> Option<Mesh> {
        let px_size = 2.0 / SIZE as f32; // one mask pixel in -1..1 units
        let f = signed_distance_2d(&self.front, SIZE, SIZE);
        let s = signed_distance_2d(&self.side, SIZE, SIZE);
        let t = signed_distance_2d(&self.top, SIZE, SIZE);
        let g = Grid::around(Vec3::splat(-1.0), Vec3::splat(1.0), res);
        let v = g.sample(&|p| {
            let a = sample(&f, p.x, p.y);
            let b = sample(&s, p.z, p.y);
            let c = sample(&t, p.x, -p.z);
            // intersection of the three extrusions; also closed at the box walls
            let walls = (p.abs().max_element() - 1.0) / px_size;
            a.max(b).max(c).max(walls)
        });
        let mut m = mesh_field(&g, &v, 2);
        if m.tris.is_empty() {
            return None;
        }
        for p in &mut m.positions {
            *p *= half;
        }
        m.compute_all_normals();
        Some(m)
    }

    /// Paints (or erases) a disc of `radius` pixels at mask pixel (x, y).
    pub fn paint(img: &mut [bool], x: f32, y: f32, radius: f32, on: bool) {
        let r2 = radius * radius;
        for j in 0..SIZE {
            for i in 0..SIZE {
                let (dx, dy) = (i as f32 + 0.5 - x, j as f32 + 0.5 - y);
                if dx * dx + dy * dy <= r2 {
                    img[j * SIZE + i] = on;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_discs_carve_a_rounded_solid_and_models_give_silhouettes() {
        let sb = Shadowbox::default();
        let m = sb.build(48, 1.0).unwrap();
        assert!(!m.boundary.iter().any(|b| *b));
        let (mn, mx) = m.bounds();
        assert!(mx.x > 0.6 && mx.x < 0.8 && mn.y < -0.6, "{mn} {mx}");
        // an L from the front, square from the side and top: an L-shaped prism
        let mut l = Shadowbox::empty();
        for j in 0..SIZE {
            for i in 0..SIZE {
                let (x, y) = (i as f32 / SIZE as f32, j as f32 / SIZE as f32);
                l.front[j * SIZE + i] = (0.1..0.9).contains(&x) && (0.1..0.9).contains(&y) && (x < 0.4 || y > 0.6);
                l.side[j * SIZE + i] = (0.1..0.9).contains(&x) && (0.1..0.9).contains(&y);
                l.top[j * SIZE + i] = (0.1..0.9).contains(&x) && (0.1..0.9).contains(&y);
            }
        }
        let lm = l.build(48, 1.0).unwrap();
        let back = Shadowbox::from_mesh(&lm);
        let at = |img: &[bool], x: f32, y: f32| img[(y * SIZE as f32) as usize * SIZE + (x * SIZE as f32) as usize];
        assert!(!at(&back.front, 0.75, 0.25), "the L's notch stays empty");
        assert!(at(&back.front, 0.25, 0.25) && at(&back.front, 0.75, 0.75) && at(&back.side, 0.5, 0.5));
    }
}
