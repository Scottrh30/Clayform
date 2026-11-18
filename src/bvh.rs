//! Bounding volume hierarchy over triangles for fast ray picking.
//! Built once per topology change and refitted cheaply after sculpting.

use crate::mesh::Mesh;
use glam::Vec3;

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// leaf: first index into `order`; inner: index of left child (right = +1)
    first: u32,
    /// number of triangles for leaves, 0 for inner nodes
    count: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    #[allow(dead_code)]
    pub t: f32,
    pub tri: u32,
    pub point: Vec3,
}

const LEAF_SIZE: usize = 4;

impl Bvh {
    pub fn build(mesh: &Mesh) -> Self {
        let nt = mesh.tris.len();
        let mut order: Vec<u32> = (0..nt as u32).collect();
        let centroids: Vec<Vec3> = mesh
            .tris
            .iter()
            .map(|t| {
                (mesh.positions[t[0] as usize] + mesh.positions[t[1] as usize] + mesh.positions[t[2] as usize])
                    / 3.0
            })
            .collect();
        let mut nodes = Vec::with_capacity(nt.max(1) * 2 / LEAF_SIZE + 2);
        nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: nt as u32 });
        let mut stack = vec![(0usize, 0usize, nt)];
        while let Some((ni, start, end)) = stack.pop() {
            let count = end - start;
            // centroid bounds
            let mut cmin = Vec3::splat(f32::MAX);
            let mut cmax = Vec3::splat(f32::MIN);
            for &t in &order[start..end] {
                let c = centroids[t as usize];
                cmin = cmin.min(c);
                cmax = cmax.max(c);
            }
            let ext = cmax - cmin;
            if count <= LEAF_SIZE || ext.max_element() <= 0.0 {
                nodes[ni] = Node { min: Vec3::ZERO, max: Vec3::ZERO, first: start as u32, count: count as u32 };
                continue;
            }
            let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
            let mid = start + count / 2;
            order[start..end].select_nth_unstable_by(count / 2, |&a, &b| {
                centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis])
            });
            let left = nodes.len();
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
            nodes[ni] = Node { min: Vec3::ZERO, max: Vec3::ZERO, first: left as u32, count: 0 };
            stack.push((left, start, mid));
            stack.push((left + 1, mid, end));
        }
        let mut bvh = Bvh { nodes, order };
        bvh.refit(mesh);
        bvh
    }

    /// Recomputes all bounds bottom-up (children always have larger indices).
    pub fn refit(&mut self, mesh: &Mesh) {
        for i in (0..self.nodes.len()).rev() {
            let n = self.nodes[i];
            let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            if n.count > 0 {
                for &t in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    for &v in &mesh.tris[t as usize] {
                        let p = mesh.positions[v as usize];
                        mn = mn.min(p);
                        mx = mx.max(p);
                    }
                }
            } else {
                let l = self.nodes[n.first as usize];
                let r = self.nodes[n.first as usize + 1];
                mn = l.min.min(r.min);
                mx = l.max.max(r.max);
            }
            self.nodes[i].min = mn;
            self.nodes[i].max = mx;
        }
    }

    pub fn raycast(&self, mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
        if self.nodes.is_empty() || mesh.tris.is_empty() {
            return None;
        }
        // avoid 0 * inf = NaN in the slab test for axis-aligned rays
        let safe = |c: f32| if c.abs() < 1e-12 { 1e-12f32.copysign(c) } else { c };
        let inv = Vec3::ONE / Vec3::new(safe(dir.x), safe(dir.y), safe(dir.z));
        let mut best: Option<Hit> = None;
        let mut best_t = f32::MAX;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            let Some(tn) = ray_aabb(origin, inv, n.min, n.max) else { continue };
            if tn > best_t {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    let [a, b, c] = mesh.tris[t as usize];
                    if let Some(tt) = ray_tri(
                        origin,
                        dir,
                        mesh.positions[a as usize],
                        mesh.positions[b as usize],
                        mesh.positions[c as usize],
                    ) {
                        if tt < best_t {
                            best_t = tt;
                            best = Some(Hit { t: tt, tri: t, point: origin + dir * tt });
                        }
                    }
                }
            } else {
                stack.push(n.first);
                stack.push(n.first + 1);
            }
        }
        best
    }
}

#[inline]
fn ray_aabb(o: Vec3, inv: Vec3, mn: Vec3, mx: Vec3) -> Option<f32> {
    let t1 = (mn - o) * inv;
    let t2 = (mx - o) * inv;
    let tmin = t1.min(t2).max_element().max(0.0);
    let tmax = t1.max(t2).min_element();
    if tmax >= tmin { Some(tmin) } else { None }
}

/// Möller–Trumbore, double sided.
#[inline]
fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    if t > 1e-6 { Some(t) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_sphere_front() {
        let m = Mesh::icosphere(4);
        let b = Bvh::build(&m);
        let h = b.raycast(&m, Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        assert!((h.point.z - 1.0).abs() < 0.01, "{:?}", h.point);
        assert!(b.raycast(&m, Vec3::new(3.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).is_none());
    }

    #[test]
    fn refit_after_move() {
        let mut m = Mesh::icosphere(3);
        let mut b = Bvh::build(&m);
        for p in &mut m.positions {
            *p *= 2.0;
        }
        b.refit(&m);
        let h = b.raycast(&m, Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        assert!((h.point.z - 2.0).abs() < 0.05);
    }
}
