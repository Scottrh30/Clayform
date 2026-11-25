//! Triangle mesh with editable connectivity (per-vertex face and neighbour
//! rings), normals, mask, subdivision stencils and primitive generation.
//!
//! Dynamic topology may leave dead faces (`DEAD`) and orphan vertices behind
//! while a stroke is in progress; `compact()` removes them afterwards.

use glam::Vec3;
use std::collections::HashMap;

/// Marks a deleted face in `Mesh::tris` until the next `compact()`.
pub const DEAD: [u32; 3] = [u32::MAX; 3];
/// "No vertex" in a subdivision stencil (boundary edge).
pub const NONE: u32 = u32::MAX;

#[derive(Clone)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub mask: Vec<f32>,
    pub tris: Vec<[u32; 3]>,
    pub face_normals: Vec<Vec3>,
    vf: Vec<Vec<u32>>,
    nbr: Vec<Vec<u32>>,
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
    /// Starts a new generation covering indices `0..n` (grows as needed).
    pub fn begin(&mut self, n: usize) {
        if self.marks.len() < n {
            self.marks.resize(n, 0);
        }
        self.cur = self.cur.wrapping_add(1);
        if self.cur == 0 {
            self.marks.iter_mut().for_each(|m| *m = 0);
            self.cur = 1;
        }
    }
    /// Returns true when `i` was not yet marked in this generation.
    #[inline]
    pub fn mark(&mut self, i: u32) -> bool {
        let i = i as usize;
        if i >= self.marks.len() {
            self.marks.resize(i + 1, 0);
        }
        let m = &mut self.marks[i];
        if *m == self.cur {
            false
        } else {
            *m = self.cur;
            true
        }
    }
    #[inline]
    pub fn is_marked(&self, i: u32) -> bool {
        self.marks.get(i as usize).is_some_and(|m| *m == self.cur)
    }
}

impl Default for Marker {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a mesh's connectivity and normals a slice at a time, so an
/// interface can spread the work of a big mesh over several frames.
pub struct MeshBuilder {
    mesh: Mesh,
    phase: u8,
    i: usize,
    tmp: Vec<u32>,
}

impl MeshBuilder {
    pub fn new(positions: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Self {
        let n = positions.len();
        let nt = tris.len();
        MeshBuilder {
            mesh: Mesh {
                normals: vec![Vec3::Y; n],
                mask: vec![0.0; n],
                face_normals: vec![Vec3::Y; nt],
                positions,
                tris,
                vf: vec![Vec::new(); n],
                nbr: vec![Vec::new(); n],
                boundary: vec![false; n],
                export_center: Vec3::ZERO,
                export_scale: 1.0,
            },
            phase: 0,
            i: 0,
            tmp: Vec::with_capacity(32),
        }
    }

    /// Sets per-vertex mask and the export transform (before `finish`).
    pub fn with_extras(mut self, mask: Option<Vec<f32>>, center: Vec3, scale: f32) -> Self {
        if let Some(m) = mask {
            if m.len() == self.mesh.positions.len() {
                self.mesh.mask = m;
            }
        }
        self.mesh.export_center = center;
        self.mesh.export_scale = scale;
        self
    }

    /// Fraction done, 0..1.
    pub fn progress(&self) -> f32 {
        let (nv, nt) = (self.mesh.positions.len().max(1) as f32, self.mesh.tris.len().max(1) as f32);
        let i = self.i as f32;
        match self.phase {
            0 => 0.25 * i / nt,
            1 => 0.25 + 0.45 * i / nv,
            2 => 0.7 + 0.15 * i / nt,
            3 => 0.85 + 0.15 * i / nv,
            _ => 1.0,
        }
    }

    /// Does work until finished (returns true) or until `keep_going`
    /// (checked every few thousand items) returns false.
    pub fn step(&mut self, keep_going: &mut dyn FnMut() -> bool) -> bool {
        const CHUNK: usize = 4096;
        let m = &mut self.mesh;
        loop {
            match self.phase {
                0 => {
                    let end = (self.i + CHUNK).min(m.tris.len());
                    for fi in self.i..end {
                        let t = m.tris[fi];
                        if t[0] != NONE {
                            for v in t {
                                m.vf[v as usize].push(fi as u32);
                            }
                        }
                    }
                    self.i = end;
                    if end == m.tris.len() {
                        self.phase = 1;
                        self.i = 0;
                    }
                }
                1 => {
                    let end = (self.i + CHUNK).min(m.positions.len());
                    for v in self.i..end {
                        m.rebuild_ring_with(v as u32, &mut self.tmp);
                    }
                    self.i = end;
                    if end == m.positions.len() {
                        self.phase = 2;
                        self.i = 0;
                    }
                }
                2 => {
                    let end = (self.i + CHUNK).min(m.tris.len());
                    for f in self.i..end {
                        m.face_normals[f] = m.face_normal_raw(f);
                    }
                    self.i = end;
                    if end == m.tris.len() {
                        self.phase = 3;
                        self.i = 0;
                    }
                }
                3 => {
                    let end = (self.i + CHUNK).min(m.positions.len());
                    for v in self.i..end {
                        m.normals[v] = m.vertex_normal_from_faces(v as u32);
                    }
                    self.i = end;
                    if end == m.positions.len() {
                        self.phase = 4;
                    }
                }
                _ => return true,
            }
            if !keep_going() {
                return self.phase >= 4;
            }
        }
    }

    pub fn finish(mut self) -> Mesh {
        self.step(&mut || true);
        self.mesh
    }
}

impl Mesh {
    pub fn new(positions: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Self {
        MeshBuilder::new(positions, tris).finish()
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
                let key = [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
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

    /// Number of live triangles.
    pub fn live_tri_count(&self) -> usize {
        self.tris.iter().filter(|t| t[0] != NONE).count()
    }

    #[inline]
    pub fn is_alive(&self, f: u32) -> bool {
        self.tris[f as usize][0] != NONE
    }

    #[inline]
    pub fn neighbors(&self, v: u32) -> &[u32] {
        &self.nbr[v as usize]
    }

    #[inline]
    pub fn vert_faces(&self, v: u32) -> &[u32] {
        &self.vf[v as usize]
    }

    /// Recomputes the neighbour list and boundary flag of `v` from its faces.
    pub fn rebuild_ring(&mut self, v: u32) {
        let mut tmp = Vec::with_capacity(16);
        self.rebuild_ring_with(v, &mut tmp);
    }

    fn rebuild_ring_with(&mut self, v: u32, tmp: &mut Vec<u32>) {
        tmp.clear();
        for &f in &self.vf[v as usize] {
            for &u in &self.tris[f as usize] {
                if u != v {
                    tmp.push(u);
                }
            }
        }
        // every interior edge appears exactly twice; boundary edges once
        tmp.sort_unstable();
        let ring = &mut self.nbr[v as usize];
        ring.clear();
        let mut boundary = false;
        let mut i = 0;
        while i < tmp.len() {
            let mut j = i + 1;
            while j < tmp.len() && tmp[j] == tmp[i] {
                j += 1;
            }
            if j - i == 1 {
                boundary = true;
            }
            ring.push(tmp[i]);
            i = j;
        }
        self.boundary[v as usize] = boundary;
    }

    pub fn edge_face_count(&self, a: u32, b: u32) -> usize {
        self.vert_faces(a).iter().filter(|&&f| self.tris[f as usize].contains(&b)).count()
    }

    // ------------------------------------------------- editing primitives

    /// Appends a vertex with no faces; returns its index.
    pub fn add_vertex(&mut self, p: Vec3, n: Vec3, mask: f32) -> u32 {
        self.positions.push(p);
        self.normals.push(n);
        self.mask.push(mask);
        self.vf.push(Vec::new());
        self.nbr.push(Vec::new());
        self.boundary.push(false);
        (self.positions.len() - 1) as u32
    }

    /// Appends a face and registers it with its vertices; returns its index.
    pub fn add_face(&mut self, t: [u32; 3]) -> u32 {
        let f = self.tris.len() as u32;
        self.tris.push(t);
        self.face_normals.push(Vec3::ZERO);
        for &v in &t {
            self.vf[v as usize].push(f);
        }
        f
    }

    /// Replaces the corners of a live face, keeping vertex→face lists right.
    pub fn set_face(&mut self, f: u32, t: [u32; 3]) {
        let old = self.tris[f as usize];
        for &v in &old {
            if !t.contains(&v) {
                self.vf[v as usize].retain(|&x| x != f);
            }
        }
        for &v in &t {
            if !old.contains(&v) {
                self.vf[v as usize].push(f);
            }
        }
        self.tris[f as usize] = t;
    }

    /// Deletes a face (it stays as `DEAD` until `compact()`).
    pub fn kill_face(&mut self, f: u32) {
        let old = self.tris[f as usize];
        if old[0] == NONE {
            return;
        }
        for &v in &old {
            self.vf[v as usize].retain(|&x| x != f);
        }
        self.tris[f as usize] = DEAD;
    }

    /// Removes dead faces and vertices without faces, renumbering the rest.
    /// Returns false when there was nothing to remove.
    pub fn compact(&mut self) -> bool {
        let dead_faces = self.tris.iter().any(|t| t[0] == NONE);
        let orphans = self.vf.iter().any(|f| f.is_empty());
        if !dead_faces && !orphans {
            return false;
        }
        let mut remap = vec![NONE; self.positions.len()];
        let mut positions = Vec::with_capacity(self.positions.len());
        let mut mask = Vec::with_capacity(self.positions.len());
        for t in self.tris.iter().filter(|t| t[0] != NONE) {
            for &v in t {
                if remap[v as usize] == NONE {
                    remap[v as usize] = positions.len() as u32;
                    positions.push(self.positions[v as usize]);
                    mask.push(self.mask[v as usize]);
                }
            }
        }
        let tris: Vec<[u32; 3]> = self
            .tris
            .iter()
            .filter(|t| t[0] != NONE)
            .map(|t| [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]])
            .collect();
        let (c, s) = (self.export_center, self.export_scale);
        *self = Mesh::new(positions, tris);
        self.mask = mask;
        self.export_center = c;
        self.export_scale = s;
        true
    }

    #[inline]
    pub fn face_normal_raw(&self, f: usize) -> Vec3 {
        let [a, b, c] = self.tris[f];
        if a == NONE {
            return Vec3::ZERO;
        }
        let (pa, pb, pc) = (self.positions[a as usize], self.positions[b as usize], self.positions[c as usize]);
        (pb - pa).cross(pc - pa)
    }

    pub fn compute_all_normals(&mut self) {
        self.face_normals.resize(self.tris.len(), Vec3::Y);
        self.normals.resize(self.positions.len(), Vec3::Y);
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
        for (v, p) in self.positions.iter().enumerate() {
            if self.vf[v].is_empty() {
                continue;
            }
            mn = mn.min(*p);
            mx = mx.max(*p);
        }
        if mn.x > mx.x {
            return (Vec3::splat(-1.0), Vec3::splat(1.0));
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

    /// Local frame (tangent, bitangent, normal) at `v`, used to store
    /// multiresolution detail so it follows the surface.
    pub fn frame(&self, v: u32) -> (Vec3, Vec3, Vec3) {
        let n = self.normals[v as usize];
        let p = self.positions[v as usize];
        let mut t = Vec3::ZERO;
        if let Some(&u) = self.neighbors(v).first() {
            t = self.positions[u as usize] - p;
            t -= n * t.dot(n);
        }
        let t = t.try_normalize().unwrap_or_else(|| n.any_orthonormal_vector());
        (t, n.cross(t), n)
    }

    // ---------------------------------------------------------- subdivision

    /// Positions of the next finer level from this mesh's positions:
    /// Loop rules for original (even) vertices, then one entry per `odd`
    /// stencil `[a, b, c, d]` (edge a-b, opposite corners c and d; d = NONE
    /// on a boundary).
    pub fn loop_positions(&self, odd: &[[u32; 4]]) -> Vec<Vec3> {
        let n = self.positions.len();
        let mut out = Vec::with_capacity(n + odd.len());
        for v in 0..n as u32 {
            let p = self.positions[v as usize];
            let np = if self.boundary[v as usize] {
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
                if nb.len() < 3 {
                    p
                } else {
                    let beta = if nb.len() == 3 { 3.0 / 16.0 } else { 3.0 / (8.0 * k) };
                    let sum: Vec3 = nb.iter().map(|&u| self.positions[u as usize]).sum();
                    p * (1.0 - k * beta) + sum * beta
                }
            };
            out.push(np);
        }
        for &[a, b, c, d] in odd {
            let pa = self.positions[a as usize];
            let pb = self.positions[b as usize];
            out.push(if d != NONE {
                (pa + pb) * 0.375 + (self.positions[c as usize] + self.positions[d as usize]) * 0.125
            } else {
                (pa + pb) * 0.5
            });
        }
        out
    }

    /// One level of subdivision. `smooth = true` is Loop subdivision,
    /// `false` splits every triangle into four without moving anything.
    pub fn subdivide(&self, smooth: bool) -> Mesh {
        self.subdivide_with_stencil(smooth).0
    }

    /// Like `subdivide`, also returning the odd-vertex stencils so later
    /// position updates (multiresolution) don't need to rebuild topology.
    pub fn subdivide_with_stencil(&self, smooth: bool) -> (Mesh, Vec<[u32; 4]>) {
        self.subdivide_with_progress(smooth, &mut |_| true).expect("not cancelled")
    }

    /// `subdivide_with_stencil` reporting progress (0..1); the callback
    /// returns false to cancel, which gives `Err(())`.
    pub fn subdivide_with_progress(
        &self,
        smooth: bool,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<(Mesh, Vec<[u32; 4]>), ()> {
        let n = self.positions.len() as u32;
        let total = self.tris.len().max(1);
        let mut edges: HashMap<(u32, u32), u32> = HashMap::with_capacity(self.tris.len() * 3 / 2);
        let mut odd: Vec<[u32; 4]> = Vec::with_capacity(self.tris.len() * 3 / 2);
        let mut new_tris = Vec::with_capacity(self.tris.len() * 4);
        for (ti, t) in self.tris.iter().enumerate().filter(|(_, t)| t[0] != NONE) {
            if ti % 16384 == 0 && !progress(0.6 * ti as f32 / total as f32) {
                return Err(());
            }
            let mut mids = [0u32; 3];
            for e in 0..3 {
                let a = t[e];
                let b = t[(e + 1) % 3];
                let c = t[(e + 2) % 3];
                let key = (a.min(b), a.max(b));
                let idx = *edges.entry(key).or_insert_with(|| {
                    odd.push([key.0, key.1, c, NONE]);
                    n + odd.len() as u32 - 1
                });
                let s = &mut odd[(idx - n) as usize];
                if s[2] != c {
                    s[3] = c;
                }
                mids[e] = idx;
            }
            let [a, b, c] = *t;
            let [ab, bc, ca] = mids;
            new_tris.push([a, ab, ca]);
            new_tris.push([ab, b, bc]);
            new_tris.push([ca, bc, c]);
            new_tris.push([ab, bc, ca]);
        }
        let positions = if smooth {
            self.loop_positions(&odd)
        } else {
            let mut p = self.positions.clone();
            p.extend(odd.iter().map(|s| (self.positions[s[0] as usize] + self.positions[s[1] as usize]) * 0.5));
            p
        };
        let mut mask = self.mask.clone();
        mask.extend(odd.iter().map(|s| (self.mask[s[0] as usize] + self.mask[s[1] as usize]) * 0.5));
        if !progress(0.7) {
            return Err(());
        }
        let mut out = Mesh::new(positions, new_tris);
        out.mask = mask;
        out.export_center = self.export_center;
        out.export_scale = self.export_scale;
        progress(1.0);
        Ok((out, odd))
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
    fn stencil_positions_match_subdivision() {
        let c = Mesh::cube(1);
        let (fine, odd) = c.subdivide_with_stencil(true);
        let p = c.loop_positions(&odd);
        assert_eq!(p.len(), fine.positions.len());
        assert!(p.iter().zip(&fine.positions).all(|(a, b)| (*a - *b).length() < 1e-6));
    }

    #[test]
    fn kill_and_compact() {
        let mut m = Mesh::icosphere(1);
        let n = m.tris.len();
        m.kill_face(0);
        assert_eq!(m.live_tri_count(), n - 1);
        assert!(m.compact());
        assert_eq!(m.tris.len(), n - 1);
        assert!(m.boundary.iter().any(|b| *b));
        assert!(!m.compact());
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
