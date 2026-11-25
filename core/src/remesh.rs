//! DynaMesh-style voxel remeshing: rebuild the sculpt as an even, closed
//! surface from a voxel grid.
//!
//! 1. Distances to the surface are measured in a narrow band of voxels.
//! 2. Inside/outside comes from a winding count along each voxel column, so
//!    overlapping closed parts merge into one shape (a union). Open meshes
//!    become a thin closed shell instead.
//! 3. Surface nets extract the zero level: one vertex per surface voxel
//!    cell, one quad (two triangles) per grid edge the surface crosses.
//! 4. Optional smoothing passes, then optional projection back onto the
//!    original surface to keep its shape crisp.

use crate::bvh::{closest_on_triangle, Bvh};
use crate::mesh::{Mesh, NONE};
use glam::Vec3;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelSettings {
    /// Voxels across the longest side of the mesh.
    pub resolution: u32,
    /// Relaxation passes after extraction (DynaMesh "Blur").
    pub smooth: u32,
    /// Snap the new vertices back onto the original surface.
    pub project: bool,
}

impl Default for VoxelSettings {
    fn default() -> Self {
        Self { resolution: 128, smooth: 1, project: true }
    }
}

pub const MIN_RESOLUTION: u32 = 16;
pub const MAX_RESOLUTION: u32 = 512;

#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    fn finish(&self) -> u64 {
        self.0 ^ (self.0 >> 29)
    }
}

type FastMap<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;

struct Grid {
    origin: Vec3,
    h: f32,
    dims: [i64; 3],
}

impl Grid {
    #[inline]
    fn key(&self, i: i64, j: i64, k: i64) -> u64 {
        (i + self.dims[0] * (j + self.dims[1] * k)) as u64
    }
    #[inline]
    fn pos(&self, i: i64, j: i64, k: i64) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.h
    }
    #[inline]
    fn inside_grid(&self, i: i64, j: i64, k: i64) -> bool {
        i >= 0 && j >= 0 && k >= 0 && i < self.dims[0] && j < self.dims[1] && k < self.dims[2]
    }
}

/// Splits triangles until no edge is longer than `max_len` (only for
/// measuring distances; the input mesh isn't changed).
fn for_each_small_triangle(a: Vec3, b: Vec3, c: Vec3, max_len: f32, f: &mut impl FnMut(Vec3, Vec3, Vec3)) {
    let l = a.distance_squared(b).max(b.distance_squared(c)).max(c.distance_squared(a));
    if l <= max_len * max_len {
        f(a, b, c);
        return;
    }
    let (ab, bc, ca) = ((a + b) * 0.5, (b + c) * 0.5, (c + a) * 0.5);
    for_each_small_triangle(a, ab, ca, max_len, f);
    for_each_small_triangle(ab, b, bc, max_len, f);
    for_each_small_triangle(ca, bc, c, max_len, f);
    for_each_small_triangle(ab, bc, ca, max_len, f);
}

/// Error text used when a progress callback asks to stop.
pub const CANCELLED: &str = "Cancelled";

pub fn voxel_remesh(src: &Mesh, s: &VoxelSettings) -> Result<Mesh, String> {
    voxel_remesh_with(src, s, &mut |_| true)
}

/// Like `voxel_remesh`, reporting progress (0..1) to `progress`, which
/// returns false to cancel.
pub fn voxel_remesh_with(src: &Mesh, s: &VoxelSettings, progress: &mut dyn FnMut(f32) -> bool) -> Result<Mesh, String> {
    let mut report = |p: f32| if progress(p) { Ok(()) } else { Err(CANCELLED.to_string()) };
    let tris: Vec<[Vec3; 3]> = src
        .tris
        .iter()
        .filter(|t| t[0] != NONE)
        .map(|t| [src.positions[t[0] as usize], src.positions[t[1] as usize], src.positions[t[2] as usize]])
        .collect();
    if tris.is_empty() {
        return Err("Nothing to remesh".into());
    }
    let res = s.resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION) as f32;
    let (mn, mx) = src.bounds();
    let extent = mx - mn;
    let h = extent.max_element().max(1e-6) / res;
    const PAD: i64 = 3;
    let dims = [
        (extent.x / h).ceil() as i64 + 2 * PAD + 1,
        (extent.y / h).ceil() as i64 + 2 * PAD + 1,
        (extent.z / h).ceil() as i64 + 2 * PAD + 1,
    ];
    let grid = Grid { origin: mn - Vec3::splat(PAD as f32 * h), h, dims };
    let closed = !src.boundary.iter().enumerate().any(|(v, b)| *b && !src.vert_faces(v as u32).is_empty());

    // ---- 1. unsigned distance in a narrow band around the surface
    // corners of cells next to the surface are within sqrt(3)*h of it
    let band = 1.8 * h;
    let mut dist: FastMap<f32> = FastMap::default();
    for (ti, t) in tris.iter().enumerate() {
        if ti % 2048 == 0 {
            report(0.45 * ti as f32 / tris.len() as f32)?;
        }
        for_each_small_triangle(t[0], t[1], t[2], 2.0 * h, &mut |a, b, c| {
            let n = (b - a).cross(c - a).normalize_or_zero();
            let lo = ((a.min(b).min(c) - Vec3::splat(band) - grid.origin) / h).ceil();
            let hi = ((a.max(b).max(c) + Vec3::splat(band) - grid.origin) / h).floor();
            for k in (lo.z as i64).max(0)..=(hi.z as i64).min(dims[2] - 1) {
                for j in (lo.y as i64).max(0)..=(hi.y as i64).min(dims[1] - 1) {
                    for i in (lo.x as i64).max(0)..=(hi.x as i64).min(dims[0] - 1) {
                        let p = grid.pos(i, j, k);
                        if n != Vec3::ZERO && n.dot(p - a).abs() >= band {
                            continue;
                        }
                        let d = closest_on_triangle(p, a, b, c).distance(p);
                        if d < band {
                            let e = dist.entry(grid.key(i, j, k)).or_insert(f32::MAX);
                            if d < *e {
                                *e = d;
                            }
                        }
                    }
                }
            }
        });
    }

    // ---- 2. inside/outside by winding number along +x columns
    let mut inside: FastMap<()> = FastMap::default();
    if closed {
        let (ny, nz) = (dims[1], dims[2]);
        let mut columns: Vec<Vec<(f32, i8)>> = vec![Vec::new(); (ny * nz) as usize];
        // a tiny irrational offset keeps column centres off triangle edges
        let jit = h * 1.234_567e-4;
        for (ti, t) in tris.iter().enumerate() {
            if ti % 4096 == 0 {
                report(0.45 + 0.1 * ti as f32 / tris.len() as f32)?;
            }
            let [a, b, c] = *t;
            let n = (b - a).cross(c - a);
            if n.x.abs() < 1e-20 {
                continue;
            }
            let ymin = a.y.min(b.y).min(c.y);
            let ymax = a.y.max(b.y).max(c.y);
            let zmin = a.z.min(b.z).min(c.z);
            let zmax = a.z.max(b.z).max(c.z);
            let j0 = (((ymin - grid.origin.y - jit) / h).ceil() as i64).max(0);
            let j1 = (((ymax - grid.origin.y - jit) / h).floor() as i64).min(ny - 1);
            let k0 = (((zmin - grid.origin.z - jit) / h).ceil() as i64).max(0);
            let k1 = (((zmax - grid.origin.z - jit) / h).floor() as i64).min(nz - 1);
            let area = (b.y - a.y) * (c.z - a.z) - (b.z - a.z) * (c.y - a.y);
            if area == 0.0 {
                continue;
            }
            let sign = if n.x > 0.0 { 1i8 } else { -1i8 };
            for k in k0..=k1 {
                let z = grid.origin.z + k as f32 * h + jit;
                for j in j0..=j1 {
                    let y = grid.origin.y + j as f32 * h + jit;
                    let e = |p: Vec3, q: Vec3| (q.y - p.y) * (z - p.z) - (q.z - p.z) * (y - p.y);
                    let (w0, w1, w2) = (e(b, c), e(c, a), e(a, b));
                    let inside_tri = if area > 0.0 {
                        w0 > 0.0 && w1 > 0.0 && w2 > 0.0
                    } else {
                        w0 < 0.0 && w1 < 0.0 && w2 < 0.0
                    };
                    if !inside_tri {
                        continue;
                    }
                    let x = (n.dot(a) - n.y * y - n.z * z) / n.x;
                    columns[(j + ny * k) as usize].push((x, sign));
                }
            }
        }
        for k in 0..nz {
            report(0.55 + 0.1 * k as f32 / nz as f32)?;
            for j in 0..ny {
                let col = &mut columns[(j + ny * k) as usize];
                if col.is_empty() {
                    continue;
                }
                col.sort_by(|p, q| p.0.total_cmp(&q.0));
                let mut w: i32 = 0;
                let mut ci = 0;
                for i in 0..dims[0] {
                    let x = grid.origin.x + i as f32 * h;
                    while ci < col.len() && col[ci].0 < x {
                        w += col[ci].1 as i32;
                        ci += 1;
                    }
                    if w != 0 {
                        inside.insert(grid.key(i, j, k), ());
                    }
                    if ci == col.len() {
                        break;
                    }
                }
            }
        }
    }

    // signed distance (negative inside)
    let shell = 0.75 * h;
    let sdf = |i: i64, j: i64, k: i64| -> f32 {
        if !grid.inside_grid(i, j, k) {
            return band;
        }
        let key = grid.key(i, j, k);
        let d = dist.get(&key).copied();
        if closed {
            let inn = inside.contains_key(&key);
            let d = d.unwrap_or(band);
            if inn { -d } else { d }
        } else {
            d.map_or(band, |d| d - shell)
        }
    };

    // ---- 3. surface nets
    let mut positions: Vec<Vec3> = Vec::new();
    let mut cell_vertex: FastMap<u32> = FastMap::default();
    const CORNERS: [[i64; 3]; 8] =
        [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1], [1, 1, 1]];
    const EDGES: [[usize; 2]; 12] =
        [[0, 1], [2, 3], [4, 5], [6, 7], [0, 2], [1, 3], [4, 6], [5, 7], [0, 4], [1, 5], [2, 6], [3, 7]];
    let mut vertex_for = |ci: i64, cj: i64, ck: i64, positions: &mut Vec<Vec3>| -> u32 {
        *cell_vertex.entry(grid.key(ci, cj, ck)).or_insert_with(|| {
            let mut v = [0f32; 8];
            for (n, c) in CORNERS.iter().enumerate() {
                v[n] = sdf(ci + c[0], cj + c[1], ck + c[2]);
            }
            let mut sum = Vec3::ZERO;
            let mut cnt = 0.0;
            for [a, b] in EDGES {
                if (v[a] < 0.0) != (v[b] < 0.0) {
                    let t = v[a] / (v[a] - v[b]);
                    let pa = Vec3::new(CORNERS[a][0] as f32, CORNERS[a][1] as f32, CORNERS[a][2] as f32);
                    let pb = Vec3::new(CORNERS[b][0] as f32, CORNERS[b][1] as f32, CORNERS[b][2] as f32);
                    sum += pa + (pb - pa) * t;
                    cnt += 1.0;
                }
            }
            let local = if cnt > 0.0 { sum / cnt } else { Vec3::splat(0.5) };
            positions.push(grid.pos(ci, cj, ck) + local * h);
            (positions.len() - 1) as u32
        })
    };

    let mut out_tris: Vec<[u32; 3]> = Vec::new();
    let mut keys: Vec<u64> = dist.keys().copied().collect();
    keys.sort_unstable();
    let nkeys = keys.len().max(1);
    for (ki, key) in keys.into_iter().enumerate() {
        if ki % 8192 == 0 {
            report(0.65 + 0.2 * ki as f32 / nkeys as f32)?;
        }
        let key = key as i64;
        let i = key % dims[0];
        let j = (key / dims[0]) % dims[1];
        let k = key / (dims[0] * dims[1]);
        let d0 = sdf(i, j, k);
        for axis in 0..3 {
            let mut e = [0i64; 3];
            e[axis] = 1;
            let d1 = sdf(i + e[0], j + e[1], k + e[2]);
            if (d0 < 0.0) == (d1 < 0.0) {
                continue;
            }
            // the four cells around this grid edge, counter-clockwise about +axis
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut quad = [0u32; 4];
            for (n, (du, dv)) in [(-1i64, -1i64), (0, -1), (0, 0), (-1, 0)].into_iter().enumerate() {
                let mut c = [i, j, k];
                c[u] += du;
                c[v] += dv;
                quad[n] = vertex_for(c[0], c[1], c[2], &mut positions);
            }
            if d1 < 0.0 {
                // surface faces -axis here
                quad.reverse();
            }
            let [a, b, c, d] = quad;
            let (pa, pb, pc, pd) =
                (positions[a as usize], positions[b as usize], positions[c as usize], positions[d as usize]);
            if pa.distance_squared(pc) <= pb.distance_squared(pd) {
                out_tris.push([a, b, c]);
                out_tris.push([a, c, d]);
            } else {
                out_tris.push([a, b, d]);
                out_tris.push([b, c, d]);
            }
        }
    }
    if out_tris.is_empty() {
        return Err("The remesh produced no surface; try a higher resolution".into());
    }

    report(0.85)?;
    let mut mesh = Mesh::new(positions, out_tris);
    report(0.9)?;

    // ---- 4. relax, then project onto the original surface
    for _ in 0..s.smooth {
        let prev = mesh.positions.clone();
        for v in 0..prev.len() {
            let nb = mesh.neighbors(v as u32);
            if nb.is_empty() {
                continue;
            }
            let avg: Vec3 = nb.iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nb.len() as f32;
            mesh.positions[v] = prev[v].lerp(avg, 0.5);
        }
    }
    if s.project {
        let bvh = Bvh::build(src);
        let reach = 2.0 * h;
        let nv = mesh.positions.len().max(1);
        for (vi, p) in mesh.positions.iter_mut().enumerate() {
            if vi % 8192 == 0 {
                report(0.9 + 0.1 * vi as f32 / nv as f32)?;
            }
            if let Some(q) = bvh.closest_point(src, *p, reach) {
                *p = q;
            }
        }
        // one gentle pass evens out spacing after the snap
        let prev = mesh.positions.clone();
        for v in 0..prev.len() {
            let nb = mesh.neighbors(v as u32);
            if nb.is_empty() {
                continue;
            }
            let avg: Vec3 = nb.iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nb.len() as f32;
            let n = mesh.normals[v];
            let mut t = avg - prev[v];
            t -= n * t.dot(n); // tangential only, so the shape doesn't shrink
            mesh.positions[v] = prev[v] + t * 0.5;
        }
    }
    mesh.compute_all_normals();
    mesh.export_center = src.export_center;
    mesh.export_scale = src.export_scale;
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_counts(m: &Mesh) -> (usize, usize) {
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &m.tris {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let bad = count.values().filter(|&&c| c != 2).count();
        (count.len(), bad)
    }

    fn merged(a: &Mesh, b: &Mesh, offset: Vec3) -> Mesh {
        let n = a.positions.len() as u32;
        let mut p = a.positions.clone();
        p.extend(b.positions.iter().map(|q| *q + offset));
        let mut t = a.tris.clone();
        t.extend(b.tris.iter().map(|t| [t[0] + n, t[1] + n, t[2] + n]));
        Mesh::new(p, t)
    }

    #[test]
    fn sphere_remesh_is_closed_round_and_outward() {
        let src = Mesh::icosphere(4);
        let m = voxel_remesh(&src, &VoxelSettings { resolution: 48, smooth: 1, project: true }).unwrap();
        let (edges, bad) = edge_counts(&m);
        assert!(edges > 1000);
        assert!(bad * 1000 < edges, "{bad} non-manifold edges of {edges}");
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!((p.length() - 1.0).abs() < 0.02, "{}", p.length());
            assert!(p.dot(*n) > 0.0, "normals must point outward");
        }
    }

    #[test]
    fn resolution_controls_density() {
        let src = Mesh::icosphere(4);
        let lo = voxel_remesh(&src, &VoxelSettings { resolution: 32, smooth: 0, project: false }).unwrap();
        let hi = voxel_remesh(&src, &VoxelSettings { resolution: 64, smooth: 0, project: false }).unwrap();
        let ratio = hi.tris.len() as f32 / lo.tris.len() as f32;
        assert!((3.0..5.0).contains(&ratio), "{ratio}");
    }

    #[test]
    fn overlapping_parts_merge_into_one_surface() {
        let s = Mesh::icosphere(4);
        let two = merged(&s, &s, Vec3::new(1.0, 0.0, 0.0));
        let m = voxel_remesh(&two, &VoxelSettings { resolution: 64, smooth: 1, project: true }).unwrap();
        let (a, b) = (Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0));
        for p in &m.positions {
            // every vertex lies on the outside of the union, none buried inside
            let da = p.distance(a);
            let db = p.distance(b);
            assert!(da.max(db) > 0.98 && da.min(db) < 1.03, "{da} {db}");
        }
        let (_, bad) = edge_counts(&m);
        assert!(bad < 20, "{bad}");
    }

    #[test]
    fn progress_rises_and_cancel_stops() {
        let src = Mesh::icosphere(3);
        let mut seen = Vec::new();
        voxel_remesh_with(&src, &VoxelSettings { resolution: 48, ..Default::default() }, &mut |p| {
            seen.push(p);
            true
        })
        .unwrap();
        assert!(seen.len() > 5 && seen.windows(2).all(|w| w[1] >= w[0]) && *seen.last().unwrap() >= 0.9);
        let mut calls = 0;
        let r = voxel_remesh_with(&src, &VoxelSettings::default(), &mut |_| {
            calls += 1;
            calls < 3
        });
        assert_eq!(r.err().as_deref(), Some(CANCELLED));
    }

    #[test]
    fn open_plane_becomes_closed_shell() {
        let src = Mesh::plane(3);
        let m = voxel_remesh(&src, &VoxelSettings { resolution: 32, smooth: 0, project: false }).unwrap();
        assert!(m.boundary.iter().all(|b| !b), "shell should have no open edges");
        let (mn, mx) = m.bounds();
        assert!(mx.y - mn.y < 0.2 && mx.x - mn.x > 1.8);
    }
}
