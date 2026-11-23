//! Triangle mesh with compact (CSR) adjacency, normals, mask, and
//! subdivision / primitive generation.

use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub mask: Vec<f32>,
    pub tris: Vec<[u32; 3]>,
    pub face_normals: Vec<Vec3>,
    nbr_off: Vec<u32>,
    nbr: Vec<u32>,
    vf_off: Vec<u32>,
    vf: Vec<u32>,
    pub boundary: Vec<bool>,
    /// Imported meshes are normalised into a unit-ish volume for sculpting.
    /// Exported positions are `p / export_scale + export_center`.
    pub export_center: Vec3,
    pub export_scale: f32,
}

/// Generation-stamp marker: O(1) "visited" tests without clearing arrays.
pub struct Marker {
    marks: Vec<u32>,
    cur: u32,
}

impl Marker {
    pub fn new() -> Self {
        Self { marks: Vec::new(), cur: 1 }
    }
    pub fn begin(&mut self, n: usize) {
        if self.marks.len() != n {
            self.marks = vec![0; n];
            self.cur = 1;
        } else {
            self.cur = self.cur.wrapping_add(1);
            if self.cur == 0 {
                self.marks.iter_mut().for_each(|m| *m = 0);
                self.cur = 1;
            }
        }
    }
    /// Returns true when `i` was not yet marked in this generation.
    #[inline]
    pub fn mark(&mut self, i: u32) -> bool {
        let m = &mut self.marks[i as usize];
        if *m == self.cur {
            false
        } else {
            *m = self.cur;
            true
        }
    }
}

impl Default for Marker {
    fn default() -> Self {
        Self::new()
    }
}

impl Mesh {
    pub fn new(positions: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Self {
        let n = positions.len();
        let mut m = Mesh {
            normals: vec![Vec3::Y; n],
            mask: vec![0.0; n],
            face_normals: vec![Vec3::Y; tris.len()],
            positions,
            tris,
            nbr_off: Vec::new(),
            nbr: Vec::new(),
            vf_off: Vec::new(),
            vf: Vec::new(),
            boundary: Vec::new(),
            export_center: Vec3::ZERO,
            export_scale: 1.0,
        };
        m.build_topology();
        m.compute_all_normals();
        m
    }

    /// Builds a mesh from a triangle soup, welding coincident vertices.
    pub fn from_soup_welded(soup: &[Vec3]) -> Self {
        let mut map: HashMap<[i64; 3], u32> = HashMap::new();
        let mut positions = Vec::new();
        let mut tris = Vec::with_capacity(soup.len() / 3);
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in soup {
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        let eps = ((mx - mn).max_element().max(1e-6)) * 1e-6;
        for t in soup.chunks_exact(3) {
            let mut idx = [0u32; 3];
            for k in 0..3 {
                let p = t[k];
                let key = [
                    (p.x / eps).round() as i64,
                    (p.y / eps).round() as i64,
                    (p.z / eps).round() as i64,
                ];
                idx[k] = *map.entry(key).or_insert_with(|| {
                    positions.push(p);
                    (positions.len() - 1) as u32
                });
            }
            if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
                tris.push(idx);
            }
        }
        Mesh::new(positions, tris)
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    #[inline]
    pub fn neighbors(&self, v: u32) -> &[u32] {
        let v = v as usize;
        &self.nbr[self.nbr_off[v] as usize..self.nbr_off[v + 1] as usize]
    }

    #[inline]
    pub fn vert_faces(&self, v: u32) -> &[u32] {
        let v = v as usize;
        &self.vf[self.vf_off[v] as usize..self.vf_off[v + 1] as usize]
    }

    fn build_topology(&mut self) {
        let n = self.positions.len();
        // vertex -> faces
        let mut counts = vec![0u32; n + 1];
        for t in &self.tris {
            for &v in t {
                counts[v as usize + 1] += 1;
            }
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut fill = counts.clone();
        let mut vf = vec![0u32; self.tris.len() * 3];
        for (fi, t) in self.tris.iter().enumerate() {
            for &v in t {
                vf[fill[v as usize] as usize] = fi as u32;
                fill[v as usize] += 1;
            }
        }
        self.vf_off = counts;
        self.vf = vf;

        // vertex -> one-ring neighbours, boundary flags
        let mut nbr_off = Vec::with_capacity(n + 1);
        let mut nbr = Vec::with_capacity(self.tris.len() * 3);
        let mut boundary = vec![false; n];
        let mut tmp: Vec<u32> = Vec::with_capacity(32);
        nbr_off.push(0u32);
        for v in 0..n as u32 {
            tmp.clear();
            for &f in self.vert_faces(v) {
                for &u in &self.tris[f as usize] {
                    if u != v {
                        tmp.push(u);
                    }
                }
            }
            // every interior edge appears exactly twice; boundary edges once
            tmp.sort_unstable();
            let mut i = 0;
            while i < tmp.len() {
                let mut j = i + 1;
                while j < tmp.len() && tmp[j] == tmp[i] {
                    j += 1;
                }
                if j - i == 1 {
                    boundary[v as usize] = true;
                }
                nbr.push(tmp[i]);
                i = j;
            }
            nbr_off.push(nbr.len() as u32);
        }
        self.nbr_off = nbr_off;
        self.nbr = nbr;
        self.boundary = boundary;
    }

    fn edge_face_count(&self, a: u32, b: u32) -> usize {
        self.vert_faces(a)
            .iter()
            .filter(|&&f| self.tris[f as usize].contains(&b))
            .count()
    }

    #[inline]
    fn face_normal_raw(&self, f: usize) -> Vec3 {
        let [a, b, c] = self.tris[f];
        let (pa, pb, pc) = (
            self.positions[a as usize],
            self.positions[b as usize],
            self.positions[c as usize],
        );
        (pb - pa).cross(pc - pa)
    }

    pub fn compute_all_normals(&mut self) {
        if self.face_normals.len() != self.tris.len() {
            self.face_normals = vec![Vec3::Y; self.tris.len()];
        }
        if self.normals.len() != self.positions.len() {
            self.normals = vec![Vec3::Y; self.positions.len()];
        }
        for f in 0..self.tris.len() {
            self.face_normals[f] = self.face_normal_raw(f);
        }
        for v in 0..self.positions.len() as u32 {
            self.normals[v as usize] = self.vertex_normal_from_faces(v);
        }
    }

    #[inline]
    fn vertex_normal_from_faces(&self, v: u32) -> Vec3 {
        let mut s = Vec3::ZERO;
        for &f in self.vert_faces(v) {
            s += self.face_normals[f as usize];
        }
        s.try_normalize().unwrap_or(Vec3::Y)
    }

    /// Recomputes face normals around `verts` and the vertex normals of every
    /// vertex sharing those faces.
    pub fn update_normals(&mut self, verts: &[u32], fmark: &mut Marker, vmark: &mut Marker) {
        fmark.begin(self.tris.len());
        vmark.begin(self.positions.len());
        let mut faces = Vec::with_capacity(verts.len() * 2);
        for &v in verts {
            for &f in self.vert_faces(v) {
                if fmark.mark(f) {
                    faces.push(f);
                }
            }
        }
        let mut ring = Vec::with_capacity(verts.len() * 2);
        for &f in &faces {
            self.face_normals[f as usize] = self.face_normal_raw(f as usize);
            for &v in &self.tris[f as usize] {
                if vmark.mark(v) {
                    ring.push(v);
                }
            }
        }
        for &v in &ring {
            self.normals[v as usize] = self.vertex_normal_from_faces(v);
        }
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        for p in &self.positions {
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        (mn, mx)
    }

    /// Centres the mesh and scales it so its largest half-extent is 1,
    /// remembering the transform for export.
    pub fn normalize_for_sculpting(&mut self) {
        let (mn, mx) = self.bounds();
        let c = (mn + mx) * 0.5;
        let half = ((mx - mn) * 0.5).max_element().max(1e-9);
        let s = 1.0 / half;
        for p in &mut self.positions {
            *p = (*p - c) * s;
        }
        self.export_center = c;
        self.export_scale = s;
        self.compute_all_normals();
    }

    /// One level of subdivision. `smooth = true` is Loop subdivision,
    /// `false` splits every triangle into four without moving anything.
    pub fn subdivide(&self, smooth: bool) -> Mesh {
        let n = self.positions.len();
        let mut new_pos: Vec<Vec3> = Vec::with_capacity(n + self.tris.len() * 3 / 2 + 8);
        let mut new_mask: Vec<f32> = Vec::with_capacity(new_pos.capacity());

        // even (original) vertices
        for v in 0..n as u32 {
            let p = self.positions[v as usize];
            let np = if !smooth {
                p
            } else if self.boundary[v as usize] {
                let mut bsum = Vec3::ZERO;
                let mut bc = 0;
                for &u in self.neighbors(v) {
                    if self.edge_face_count(v, u) == 1 {
                        bsum += self.positions[u as usize];
                        bc += 1;
                    }
                }
                if bc == 2 { p * 0.75 + bsum * 0.125 } else { p }
            } else {
                let nb = self.neighbors(v);
                let k = nb.len() as f32;
                if k < 3.0 {
                    p
                } else {
                    let beta = if nb.len() == 3 { 3.0 / 16.0 } else { 3.0 / (8.0 * k) };
                    let sum: Vec3 = nb.iter().map(|&u| self.positions[u as usize]).sum();
                    p * (1.0 - k * beta) + sum * beta
                }
            };
            new_pos.push(np);
            new_mask.push(self.mask[v as usize]);
        }

        // odd (edge) vertices
        struct EdgeAcc {
            idx: u32,
            opp_sum: Vec3,
            opp_count: u32,
        }
        let mut edges: HashMap<(u32, u32), EdgeAcc> = HashMap::with_capacity(self.tris.len() * 3 / 2);
        let mut edge_list: Vec<(u32, u32)> = Vec::with_capacity(self.tris.len() * 3 / 2);
        let mut face_mid = Vec::with_capacity(self.tris.len());
        for t in &self.tris {
            let mut mids = [0u32; 3];
            for e in 0..3 {
                let a = t[e];
                let b = t[(e + 1) % 3];
                let c = t[(e + 2) % 3];
                let key = (a.min(b), a.max(b));
                let next = (n + edge_list.len()) as u32;
                let acc = edges.entry(key).or_insert_with(|| {
                    edge_list.push(key);
                    EdgeAcc { idx: next, opp_sum: Vec3::ZERO, opp_count: 0 }
                });
                acc.opp_sum += self.positions[c as usize];
                acc.opp_count += 1;
                mids[e] = acc.idx;
            }
            face_mid.push(mids);
        }
        for key in &edge_list {
            let acc = &edges[key];
            let pa = self.positions[key.0 as usize];
            let pb = self.positions[key.1 as usize];
            let p = if smooth && acc.opp_count == 2 {
                (pa + pb) * 0.375 + acc.opp_sum * 0.125
            } else {
                (pa + pb) * 0.5
            };
            new_pos.push(p);
            new_mask.push((self.mask[key.0 as usize] + self.mask[key.1 as usize]) * 0.5);
        }

        let mut new_tris = Vec::with_capacity(self.tris.len() * 4);
        for (t, m) in self.tris.iter().zip(face_mid.iter()) {
            let [a, b, c] = *t;
            let [ab, bc, ca] = *m;
            new_tris.push([a, ab, ca]);
            new_tris.push([ab, b, bc]);
            new_tris.push([ca, bc, c]);
            new_tris.push([ab, bc, ca]);
        }
        let mut out = Mesh::new(new_pos, new_tris);
        out.mask = new_mask;
        out.export_center = self.export_center;
        out.export_scale = self.export_scale;
        out
    }

    // ---------------------------------------------------------------- primitives

    pub fn icosphere(levels: u32) -> Mesh {
        let t = (1.0 + 5f32.sqrt()) / 2.0;
        let p: Vec<Vec3> = [
            (-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0),
            (0.0, -1.0, t), (0.0, 1.0, t), (0.0, -1.0, -t), (0.0, 1.0, -t),
            (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0),
        ]
        .iter()
        .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
        .collect();
        let tris = vec![
            [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
            [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
            [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
            [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
        ];
        let mut m = Mesh::new(p, tris);
        for _ in 0..levels {
            m = m.subdivide(false);
            for p in &mut m.positions {
                *p = p.normalize();
            }
        }
        m.compute_all_normals();
        m
    }

    pub fn cube(levels: u32) -> Mesh {
        let s = 0.8;
        let p: Vec<Vec3> = [
            (-s, -s, -s), (s, -s, -s), (s, s, -s), (-s, s, -s),
            (-s, -s, s), (s, -s, s), (s, s, s), (-s, s, s),
        ]
        .iter()
        .map(|&(x, y, z)| Vec3::new(x, y, z))
        .collect();
        let tris = vec![
            [0, 2, 1], [0, 3, 2], // back
            [4, 5, 6], [4, 6, 7], // front
            [0, 1, 5], [0, 5, 4], // bottom
            [3, 7, 6], [3, 6, 2], // top
            [0, 4, 7], [0, 7, 3], // left
            [1, 2, 6], [1, 6, 5], // right
        ];
        let mut m = Mesh::new(p, tris);
        for _ in 0..levels {
            m = m.subdivide(false);
        }
        m
    }

    pub fn plane(levels: u32) -> Mesh {
        let s = 1.0;
        let p = vec![
            Vec3::new(-s, 0.0, -s),
            Vec3::new(s, 0.0, -s),
            Vec3::new(s, 0.0, s),
            Vec3::new(-s, 0.0, s),
        ];
        let tris = vec![[0, 2, 1], [0, 3, 2]];
        let mut m = Mesh::new(p, tris);
        for _ in 0..levels {
            m = m.subdivide(false);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icosphere_counts_and_closed() {
        let m = Mesh::icosphere(3);
        assert_eq!(m.tris.len(), 20 * 4usize.pow(3));
        assert_eq!(m.positions.len(), 10 * 4usize.pow(3) + 2);
        assert!(m.boundary.iter().all(|b| !b));
        // normals point outward
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!(p.dot(*n) > 0.9);
        }
    }

    #[test]
    fn plane_has_boundary() {
        let m = Mesh::plane(2);
        assert!(m.boundary.iter().any(|b| *b));
        assert_eq!(m.tris.len(), 2 * 16);
    }

    #[test]
    fn loop_subdivision_shrinks_cube_toward_sphere() {
        let c = Mesh::cube(0);
        let s = c.subdivide(true).subdivide(true);
        assert_eq!(s.tris.len(), 12 * 16);
        let (mn, mx) = s.bounds();
        assert!(mx.x < 0.8 && mn.x > -0.8);
        assert!(s.boundary.iter().all(|b| !b));
    }

    #[test]
    fn weld_soup() {
        let a = Vec3::ZERO;
        let b = Vec3::X;
        let c = Vec3::Y;
        let d = Vec3::new(1.0, 1.0, 0.0);
        let m = Mesh::from_soup_welded(&[a, b, c, b, d, c]);
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.tris.len(), 2);
    }
}
