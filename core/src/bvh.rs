//! Bounding volume hierarchy over triangles for fast ray picking.
//! Built once per topology change and refitted cheaply after sculpting.

use crate::mesh::{Mesh, NONE};
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
    /// Parent of each node (NONE for the root), and the leaf holding each
    /// indexed face, for refitting only what moved.
    parent: Vec<u32>,
    leaf_of: Vec<u32>,
    /// Scratch marks for partial refits.
    mark: Vec<u32>,
    stamp: u32,
    /// Faces with index >= `indexed` were added after the build (dynamic
    /// topology) and are tested by brute force until the next rebuild.
    indexed: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    #[allow(dead_code)]
    pub t: f32,
    pub tri: u32,
    pub point: Vec3,
}

const LEAF_SIZE: usize = 4;

/// Builds a `Bvh` a slice at a time (see `MeshBuilder`).
pub struct BvhBuilder {
    nodes: Vec<Node>,
    order: Vec<u32>,
    centroids: Vec<Vec3>,
    stack: Vec<(usize, usize, usize)>,
    phase: u8,
    i: usize,
    indexed: usize,
}

impl BvhBuilder {
    pub fn new(mesh: &Mesh) -> Self {
        let order: Vec<u32> = (0..mesh.tris.len() as u32).filter(|&f| mesh.is_alive(f)).collect();
        let nt = order.len();
        let mut nodes = Vec::with_capacity(nt.max(1) * 2 / LEAF_SIZE + 2);
        nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: nt as u32 });
        BvhBuilder {
            nodes,
            order,
            centroids: Vec::new(),
            stack: vec![(0, 0, nt)],
            phase: 0,
            i: 0,
            indexed: mesh.tris.len(),
        }
    }

    /// Fraction done, 0..1 (approximate while splitting nodes).
    pub fn progress(&self) -> f32 {
        match self.phase {
            0 => 0.2 * self.i as f32 / self.indexed.max(1) as f32,
            1 => 0.2 + 0.6 * (self.nodes.len() as f32 / (2.0 * self.order.len().max(1) as f32 / LEAF_SIZE as f32)).min(1.0),
            2 => 0.8 + 0.2 * (1.0 - self.i as f32 / self.nodes.len().max(1) as f32),
            _ => 1.0,
        }
    }

    /// Works until done (true) or until `keep_going` returns false.
    pub fn step(&mut self, mesh: &Mesh, keep_going: &mut dyn FnMut() -> bool) -> bool {
        loop {
            match self.phase {
                0 => {
                    let end = (self.i + 8192).min(mesh.tris.len());
                    self.centroids.extend(mesh.tris[self.i..end].iter().map(|t| {
                        if t[0] == NONE {
                            Vec3::ZERO
                        } else {
                            (mesh.positions[t[0] as usize] + mesh.positions[t[1] as usize] + mesh.positions[t[2] as usize])
                                / 3.0
                        }
                    }));
                    self.i = end;
                    if end == mesh.tris.len() {
                        self.phase = 1;
                    }
                }
                1 => {
                    for _ in 0..256 {
                        let Some((ni, start, end)) = self.stack.pop() else {
                            self.phase = 2;
                            self.i = self.nodes.len();
                            break;
                        };
                        self.split(ni, start, end);
                    }
                }
                2 => {
                    // refit bottom-up in slices (children have larger indices)
                    let start = self.i.saturating_sub(16384);
                    for i in (start..self.i).rev() {
                        refit_node(&mut self.nodes, &self.order, mesh, i);
                    }
                    self.i = start;
                    if start == 0 {
                        self.phase = 3;
                    }
                }
                _ => return true,
            }
            if !keep_going() {
                return self.phase >= 3;
            }
        }
    }

    fn split(&mut self, ni: usize, start: usize, end: usize) {
        let count = end - start;
        let centroids = &self.centroids;
        let mut cmin = Vec3::splat(f32::MAX);
        let mut cmax = Vec3::splat(f32::MIN);
        for &t in &self.order[start..end] {
            let c = centroids[t as usize];
            cmin = cmin.min(c);
            cmax = cmax.max(c);
        }
        let ext = cmax - cmin;
        if count <= LEAF_SIZE || ext.max_element() <= 0.0 {
            self.nodes[ni] = Node { min: Vec3::ZERO, max: Vec3::ZERO, first: start as u32, count: count as u32 };
            return;
        }
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        let mid = start + count / 2;
        self.order[start..end].select_nth_unstable_by(count / 2, |&a, &b| {
            centroids[a as usize][axis].total_cmp(&centroids[b as usize][axis])
        });
        let left = self.nodes.len();
        self.nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
        self.nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, first: 0, count: 0 });
        self.nodes[ni] = Node { min: Vec3::ZERO, max: Vec3::ZERO, first: left as u32, count: 0 };
        self.stack.push((left, start, mid));
        self.stack.push((left + 1, mid, end));
    }

    pub fn finish(mut self, mesh: &Mesh) -> Bvh {
        self.step(mesh, &mut || true);
        if self.order.is_empty() {
            // nothing to index: an empty hierarchy
            self.nodes.clear();
        }
        let mut parent = vec![NONE; self.nodes.len()];
        let mut leaf_of = vec![NONE; self.indexed];
        for (i, n) in self.nodes.iter().enumerate() {
            if n.count > 0 {
                for &f in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    if (f as usize) < leaf_of.len() {
                        leaf_of[f as usize] = i as u32;
                    }
                }
            } else if (n.first as usize) + 1 < self.nodes.len() && i < n.first as usize {
                parent[n.first as usize] = i as u32;
                parent[n.first as usize + 1] = i as u32;
            }
        }
        let mark = vec![0; self.nodes.len()];
        Bvh { nodes: self.nodes, order: self.order, parent, leaf_of, mark, stamp: 0, indexed: self.indexed }
    }
}

fn refit_node(nodes: &mut [Node], order: &[u32], mesh: &Mesh, i: usize) {
    let n = nodes[i];
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    // (inner nodes always point past themselves; an empty root doesn't)
    if n.count > 0 || n.first as usize <= i {
        for &t in &order[n.first as usize..(n.first + n.count) as usize] {
            if !mesh.is_alive(t) {
                continue;
            }
            for &v in &mesh.tris[t as usize] {
                let p = mesh.positions[v as usize];
                mn = mn.min(p);
                mx = mx.max(p);
            }
        }
    } else {
        let l = nodes[n.first as usize];
        let r = nodes[n.first as usize + 1];
        mn = l.min.min(r.min);
        mx = l.max.max(r.max);
    }
    nodes[i].min = mn;
    nodes[i].max = mx;
}

impl Bvh {
    pub fn build(mesh: &Mesh) -> Self {
        let mut b = BvhBuilder::new(mesh);
        b.step(mesh, &mut || true);
        b.finish(mesh)
    }

    /// Recomputes all bounds bottom-up (children always have larger indices).
    pub fn refit(&mut self, mesh: &Mesh) {
        for i in (0..self.nodes.len()).rev() {
            refit_node(&mut self.nodes, &self.order, mesh, i);
        }
    }

    /// Recomputes the bounds of the leaves holding `faces` and of their
    /// ancestors only. Much cheaper than `refit` when a brush moved a small
    /// part of a big mesh.
    pub fn refit_faces(&mut self, mesh: &Mesh, faces: &[u32]) {
        if faces.len() * 3 > self.indexed || self.nodes.is_empty() {
            self.refit(mesh);
            return;
        }
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.mark.iter_mut().for_each(|m| *m = 0);
            self.stamp = 1;
        }
        let mut todo: Vec<u32> = Vec::new();
        for &f in faces {
            let Some(&leaf) = self.leaf_of.get(f as usize) else { continue };
            let mut n = leaf;
            // walk up until a node already queued
            while n != NONE && self.mark[n as usize] != self.stamp {
                self.mark[n as usize] = self.stamp;
                todo.push(n);
                n = self.parent[n as usize];
            }
        }
        // children have larger indices than their parents
        todo.sort_unstable_by(|a, b| b.cmp(a));
        for n in todo {
            refit_node(&mut self.nodes, &self.order, mesh, n as usize);
        }
    }

    /// Faces added since the build (tested by brute force).
    pub fn unindexed(&self, mesh: &Mesh) -> usize {
        mesh.tris.len().saturating_sub(self.indexed)
    }

    pub fn indexed(&self) -> usize {
        self.indexed
    }

    pub fn raycast(&self, mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
        if self.nodes.is_empty() || mesh.tris.is_empty() {
            return None;
        }
        let mut best: Option<Hit> = None;
        let mut best_t = f32::MAX;
        let test = |t: u32, best: &mut Option<Hit>, best_t: &mut f32| {
            let [a, b, c] = mesh.tris[t as usize];
            if a == NONE || mesh.is_hidden(t) {
                return;
            }
            if let Some(tt) =
                ray_tri(origin, dir, mesh.positions[a as usize], mesh.positions[b as usize], mesh.positions[c as usize])
            {
                if tt < *best_t {
                    *best_t = tt;
                    *best = Some(Hit { t: tt, tri: t, point: origin + dir * tt });
                }
            }
        };
        for t in self.indexed.min(mesh.tris.len())..mesh.tris.len() {
            test(t as u32, &mut best, &mut best_t);
        }
        // avoid 0 * inf = NaN in the slab test for axis-aligned rays
        let safe = |c: f32| if c.abs() < 1e-12 { 1e-12f32.copysign(c) } else { c };
        let inv = Vec3::ONE / Vec3::new(safe(dir.x), safe(dir.y), safe(dir.z));
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
                    test(t, &mut best, &mut best_t);
                }
            } else {
                stack.push(n.first);
                stack.push(n.first + 1);
            }
        }
        best
    }
}

impl Bvh {
    /// Closest point on the surface to `p`, if one lies within `max_dist`.
    pub fn closest_point(&self, mesh: &Mesh, p: Vec3, max_dist: f32) -> Option<Vec3> {
        self.closest(mesh, p, max_dist).map(|c| c.0)
    }

    /// Closest surface point within `max_dist`, with its triangle.
    pub fn closest(&self, mesh: &Mesh, p: Vec3, max_dist: f32) -> Option<(Vec3, u32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best_d2 = max_dist * max_dist;
        let mut best = None;
        let test = |t: u32, best_d2: &mut f32, best: &mut Option<(Vec3, u32)>| {
            let [a, b, c] = mesh.tris[t as usize];
            if a == NONE {
                return;
            }
            let q = closest_on_triangle(p, mesh.positions[a as usize], mesh.positions[b as usize], mesh.positions[c as usize]);
            let d2 = q.distance_squared(p);
            if d2 < *best_d2 {
                *best_d2 = d2;
                *best = Some((q, t));
            }
        };
        for t in self.indexed.min(mesh.tris.len())..mesh.tris.len() {
            test(t as u32, &mut best_d2, &mut best);
        }
        let mut stack: Vec<u32> = vec![0];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            let d = (n.min - p).max(p - n.max).max(Vec3::ZERO);
            if d.length_squared() > best_d2 {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    test(t, &mut best_d2, &mut best);
                }
            } else {
                stack.push(n.first);
                stack.push(n.first + 1);
            }
        }
        best
    }
}

/// Closest point to `p` on triangle abc (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
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
    fn closest_point_on_sphere() {
        let m = Mesh::icosphere(4);
        let b = Bvh::build(&m);
        let q = b.closest_point(&m, Vec3::new(0.0, 2.0, 0.0), 5.0).unwrap();
        assert!((q - Vec3::Y).length() < 0.01, "{q:?}");
        assert!(b.closest_point(&m, Vec3::new(0.0, 3.0, 0.0), 0.5).is_none());
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

    #[test]
    fn partial_refit_matches_full_refit() {
        let mut m = Mesh::icosphere(4);
        let mut a = Bvh::build(&m);
        let mut b = Bvh::build(&m);
        // push a patch of vertices outwards
        let moved: Vec<u32> = (0..m.positions.len() as u32).filter(|&v| m.positions[v as usize].z > 0.8).collect();
        for &v in &moved {
            m.positions[v as usize] *= 1.5;
        }
        let faces: Vec<u32> = moved.iter().flat_map(|&v| m.vert_faces(v).to_vec()).collect();
        a.refit_faces(&m, &faces);
        b.refit(&m);
        for (x, y) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!((x.min, x.max), (y.min, y.max));
        }
        let h = a.raycast(&m, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        assert!(h.point.z > 1.4);
    }
}
