//! Meshes from distance fields on a grid (surface nets), and 2D signed
//! distance from masks. Used by ZSpheres and Shadowbox.

use crate::mesh::{Mesh, MeshBuilder};
use glam::Vec3;

/// A regular grid of samples: `dims` points along each axis from `origin`,
/// `h` apart.
pub struct Grid {
    pub origin: Vec3,
    pub h: f32,
    pub dims: [usize; 3],
}

impl Grid {
    /// A grid covering `mn`..`mx` (plus a margin) at about `res` samples
    /// along the longest side.
    pub fn around(mn: Vec3, mx: Vec3, res: usize) -> Self {
        let ext = (mx - mn).max_element().max(1e-6);
        let h = ext / res.max(8) as f32;
        let pad = 2.0 * h;
        let origin = mn - Vec3::splat(pad);
        let size = mx - mn + Vec3::splat(2.0 * pad);
        let dims = [0, 1, 2].map(|k| (size[k] / h).ceil() as usize + 1);
        Self { origin, h, dims }
    }

    pub fn point(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.h
    }

    /// Samples `f` (negative inside) at every grid point.
    pub fn sample(&self, f: &dyn Fn(Vec3) -> f32) -> Vec<f32> {
        let [nx, ny, nz] = self.dims;
        let mut out = Vec::with_capacity(nx * ny * nz);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    out.push(f(self.point(i, j, k)));
                }
            }
        }
        out
    }
}

/// Surface nets over sampled values (negative = inside): quads (as partner
/// triangle pairs), lightly smoothed.
pub fn mesh_field(g: &Grid, v: &[f32], smooth: usize) -> Mesh {
    let [nx, ny, nz] = g.dims;
    let at = |i: usize, j: usize, k: usize| v[i + nx * (j + ny * k)];
    let cell = |i: usize, j: usize, k: usize| i + nx * (j + ny * k);
    let mut vert: Vec<u32> = vec![u32::MAX; nx * ny * nz];
    let mut pos: Vec<Vec3> = Vec::new();
    const CORNERS: [[usize; 3]; 8] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
    const EDGES: [[usize; 2]; 12] = [[0, 1], [2, 3], [4, 5], [6, 7], [0, 2], [1, 3], [4, 6], [5, 7], [0, 4], [1, 5], [2, 6], [3, 7]];
    for k in 0..nz - 1 {
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let c: [f32; 8] = CORNERS.map(|o| at(i + o[0], j + o[1], k + o[2]));
                let inside = c.iter().filter(|x| **x < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                let mut n = 0.0;
                for [a, b] in EDGES {
                    if (c[a] < 0.0) != (c[b] < 0.0) {
                        let t = c[a] / (c[a] - c[b]);
                        let pa = Vec3::new(CORNERS[a][0] as f32, CORNERS[a][1] as f32, CORNERS[a][2] as f32);
                        let pb = Vec3::new(CORNERS[b][0] as f32, CORNERS[b][1] as f32, CORNERS[b][2] as f32);
                        sum += pa + (pb - pa) * t;
                        n += 1.0;
                    }
                }
                vert[cell(i, j, k)] = pos.len() as u32;
                pos.push(g.point(i, j, k) + sum / n * g.h);
            }
        }
    }
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for k in 1..nz - 1 {
        for j in 1..ny - 1 {
            for i in 1..nx - 1 {
                let d0 = at(i, j, k);
                for axis in 0..3 {
                    let mut e = [i, j, k];
                    e[axis] += 1;
                    if e[axis] >= g.dims[axis] {
                        continue;
                    }
                    let d1 = at(e[0], e[1], e[2]);
                    if (d0 < 0.0) == (d1 < 0.0) {
                        continue;
                    }
                    let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
                    let mut q = [0u32; 4];
                    let mut ok = true;
                    for (n, (du, dw)) in [(1usize, 1usize), (0, 1), (0, 0), (1, 0)].into_iter().enumerate() {
                        let mut c = [i, j, k];
                        c[u] -= du;
                        c[w] -= dw;
                        let id = vert[cell(c[0], c[1], c[2])];
                        if id == u32::MAX {
                            ok = false;
                            break;
                        }
                        q[n] = id;
                    }
                    if !ok {
                        continue;
                    }
                    if d0 >= 0.0 {
                        q.reverse();
                    }
                    tris.push([q[0], q[1], q[2]]);
                    tris.push([q[0], q[2], q[3]]);
                }
            }
        }
    }
    let partner: Vec<u32> = (0..tris.len() as u32).map(|t| t ^ 1).collect();
    let mut m = MeshBuilder::new(pos, tris).with_faces(None, Some(partner)).finish();
    for _ in 0..smooth {
        let prev = m.positions.clone();
        for vi in 0..prev.len() {
            let nb = m.neighbors(vi as u32);
            if nb.is_empty() {
                continue;
            }
            let avg = nb.iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nb.len() as f32;
            m.positions[vi] = prev[vi].lerp(avg, 0.5);
        }
    }
    m.compute_all_normals();
    m
}

/// Signed distance (in pixels, negative inside) of a w × h mask
/// (true = inside), by a two-pass chamfer transform.
pub fn signed_distance_2d(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    let dist = |inside: bool| -> Vec<f32> {
        let inf = 1e9f32;
        let mut d: Vec<f32> = mask.iter().map(|&m| if m == inside { 0.0 } else { inf }).collect();
        let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let mut v = d[i];
                if x > 0 {
                    v = v.min(d[i - 1] + a);
                }
                if y > 0 {
                    v = v.min(d[i - w] + a);
                    if x > 0 {
                        v = v.min(d[i - w - 1] + b);
                    }
                    if x + 1 < w {
                        v = v.min(d[i - w + 1] + b);
                    }
                }
                d[i] = v;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let i = y * w + x;
                let mut v = d[i];
                if x + 1 < w {
                    v = v.min(d[i + 1] + a);
                }
                if y + 1 < h {
                    v = v.min(d[i + w] + a);
                    if x + 1 < w {
                        v = v.min(d[i + w + 1] + b);
                    }
                    if x > 0 {
                        v = v.min(d[i + w - 1] + b);
                    }
                }
                d[i] = v;
            }
        }
        d
    };
    // small masks: exact distances to the boundary pixels (smooth iso-lines);
    // big ones: the chamfer estimate
    if w * h <= 160 * 160 {
        let edge: Vec<(f32, f32)> = (0..w * h)
            .filter(|&i| {
                let (x, y) = (i % w, i / w);
                let m = mask[i];
                (x > 0 && mask[i - 1] != m) || (x + 1 < w && mask[i + 1] != m) || (y > 0 && mask[i - w] != m) || (y + 1 < h && mask[i + w] != m)
            })
            .filter(|&i| mask[i])
            .map(|i| ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5))
            .collect();
        if edge.is_empty() {
            let v = if mask.first() == Some(&true) { -1e3 } else { 1e3 };
            return vec![v; w * h];
        }
        return (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let d = edge.iter().map(|(ex, ey)| (ex - x) * (ex - x) + (ey - y) * (ey - y)).fold(f32::MAX, f32::min).sqrt();
                // the surface lies half a pixel outside the inside boundary pixels
                if mask[i] { -(d + 0.5) } else { d - 0.5 }
            })
            .collect();
    }
    // distance to the nearest inside pixel (outside) / outside pixel (inside)
    let to_in = dist(true);
    let to_out = dist(false);
    mask.iter().enumerate().map(|(i, &m)| if m { 0.5 - to_out[i] } else { to_in[i] - 0.5 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sphere_field_meshes_closed_and_round() {
        let g = Grid::around(Vec3::splat(-1.0), Vec3::splat(1.0), 40);
        let v = g.sample(&|p| p.length() - 1.0);
        let m = mesh_field(&g, &v, 0);
        assert!(!m.boundary.iter().any(|b| *b));
        assert!(m.positions.iter().all(|p| (p.length() - 1.0).abs() < 0.06));
        assert!(m.partner.iter().all(|p| *p != crate::mesh::NONE));
        // outward facing
        let out = (0..m.tris.len()).filter(|&f| m.face_normal_raw(f).dot(m.positions[m.tris[f][0] as usize]) > 0.0).count();
        assert_eq!(out, m.tris.len());
        let mask: Vec<bool> = (0..100).map(|i| (i % 10) >= 3 && (i % 10) < 7 && i / 10 >= 3 && i / 10 < 7).collect();
        let sd = signed_distance_2d(&mask, 10, 10);
        assert!(sd[55] < 0.0 && sd[0] > 3.0);
    }
}
