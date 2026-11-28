//! Retopology: new polygons drawn over the sculpt, kept apart from it until
//! applied.

use crate::poly::Polys;
use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Retopo {
    pub verts: Vec<Vec3>,
    pub faces: Vec<Vec<u32>>,
    /// Vertices picked for the face being built.
    pub pending: Vec<u32>,
    history: Vec<(Vec<Vec3>, Vec<Vec<u32>>)>,
}

fn newell(pos: &[Vec3], f: &[u32]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..f.len() {
        let a = pos[f[i] as usize];
        let b = pos[f[(i + 1) % f.len()] as usize];
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

impl Retopo {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    /// Remembers the current state so `undo` can return to it.
    pub fn checkpoint(&mut self) {
        self.history.push((self.verts.clone(), self.faces.clone()));
        if self.history.len() > 200 {
            self.history.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some((v, f)) = self.history.pop() {
            self.verts = v;
            self.faces = f;
            self.pending.clear();
        }
    }

    pub fn clear(&mut self) {
        if !self.is_empty() {
            self.checkpoint();
        }
        self.verts.clear();
        self.faces.clear();
        self.pending.clear();
    }

    pub fn add_vertex(&mut self, p: Vec3) -> u32 {
        self.verts.push(p);
        (self.verts.len() - 1) as u32
    }

    /// Deletes a vertex and the faces using it.
    pub fn remove_vertex(&mut self, v: u32) {
        self.faces.retain(|f| !f.contains(&v));
        self.verts.remove(v as usize);
        for f in &mut self.faces {
            for u in f.iter_mut() {
                if *u > v {
                    *u -= 1;
                }
            }
        }
        self.pending.retain(|&u| u != v);
        for u in &mut self.pending {
            if *u > v {
                *u -= 1;
            }
        }
    }

    /// Adds a face, wound so it faces `eye`. Returns false for a duplicate
    /// or degenerate face.
    pub fn add_face(&mut self, mut face: Vec<u32>, eye: Vec3) -> bool {
        face.dedup();
        if face.len() < 3 || face.first() == face.last() {
            return false;
        }
        let mut key = face.clone();
        key.sort_unstable();
        if self.faces.iter().any(|f| {
            let mut k = f.clone();
            k.sort_unstable();
            k == key
        }) {
            return false;
        }
        let c: Vec3 = face.iter().map(|&v| self.verts[v as usize]).sum::<Vec3>() / face.len() as f32;
        if newell(&self.verts, &face).dot(eye - c) < 0.0 {
            face.reverse();
        }
        self.faces.push(face);
        true
    }

    /// Picks a vertex for the face being built: four make a quad, or
    /// clicking the first one again closes a triangle (or any polygon).
    /// Returns true when a face was made.
    pub fn pick(&mut self, v: u32, eye: Vec3) -> bool {
        if self.pending.first() == Some(&v) && self.pending.len() >= 3 {
            let f = std::mem::take(&mut self.pending);
            return self.add_face(f, eye);
        }
        if self.pending.contains(&v) {
            return false;
        }
        self.pending.push(v);
        if self.pending.len() == 4 {
            let f = std::mem::take(&mut self.pending);
            return self.add_face(f, eye);
        }
        false
    }

    /// Unique edges of all faces.
    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                if seen.insert((a.min(b), a.max(b))) {
                    out.push((a, b));
                }
            }
        }
        out
    }

    /// Relaxes interior vertices (open edges stay put), keeping them on the
    /// surface with `project`.
    pub fn relax(&mut self, iterations: usize, project: &dyn Fn(Vec3) -> Vec3) {
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let n = self.verts.len();
        let mut fixed = vec![true; n];
        let mut nbr: Vec<Vec<u32>> = vec![Vec::new(); n];
        for f in &self.faces {
            for &v in f {
                fixed[v as usize] = false;
            }
        }
        for (&(a, b), &c) in &count {
            nbr[a as usize].push(b);
            nbr[b as usize].push(a);
            if c == 1 {
                fixed[a as usize] = true;
                fixed[b as usize] = true;
            }
        }
        for _ in 0..iterations {
            let prev = self.verts.clone();
            for v in 0..n {
                if fixed[v] || nbr[v].is_empty() {
                    continue;
                }
                let avg = nbr[v].iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nbr[v].len() as f32;
                self.verts[v] = project(prev[v].lerp(avg, 0.5));
            }
        }
    }

    /// Mirrors the faces across X (vertices within `eps` of the plane are
    /// shared by both halves).
    pub fn mirror_x(&mut self, eps: f32) {
        let n = self.verts.len();
        let mut map = vec![0u32; n];
        for v in 0..n {
            let p = self.verts[v];
            if p.x.abs() <= eps {
                self.verts[v].x = 0.0;
                map[v] = v as u32;
            } else {
                map[v] = self.add_vertex(Vec3::new(-p.x, p.y, p.z));
            }
        }
        let faces = self.faces.clone();
        for f in faces {
            if f.iter().all(|&v| map[v as usize] == v) {
                continue; // lies on the plane
            }
            let mut m: Vec<u32> = f.iter().map(|&v| map[v as usize]).collect();
            m.reverse();
            self.faces.push(m);
        }
    }

    /// The faces as a polygon list (unused vertices dropped by `to_mesh`).
    pub fn to_polys(&self) -> Polys {
        Polys {
            pos: self.verts.clone(),
            mask: vec![0.0; self.verts.len()],
            faces: self.faces.clone(),
            groups: vec![0; self.faces.len()],
            export_center: Vec3::ZERO,
            export_scale: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_picks_make_a_quad_facing_the_eye() {
        let mut r = Retopo::default();
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let v: Vec<u32> = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)]
            .iter()
            .map(|&(x, y)| r.add_vertex(Vec3::new(x, y, 0.0)))
            .collect();
        for (i, &u) in v.iter().enumerate() {
            assert_eq!(r.pick(u, eye), i == 3);
        }
        assert_eq!(r.faces.len(), 1);
        assert!(newell(&r.verts, &r.faces[0]).z > 0.0, "wound towards the camera");
        // a triangle: three picks then the first again
        let e = r.add_vertex(Vec3::new(2.0, 0.5, 0.0));
        r.pick(v[3], eye);
        r.pick(v[2], eye);
        r.pick(e, eye);
        assert!(r.pick(v[3], eye));
        assert_eq!(r.edges().len(), 6);
        r.mirror_x(1e-4);
        assert_eq!(r.faces.len(), 4);
        let m = r.to_polys().to_mesh();
        assert_eq!(m.tris.len(), 2 * 2 + 2);
        r.remove_vertex(e);
        assert_eq!(r.faces.len(), 3);
    }

    #[test]
    fn relax_keeps_open_edges() {
        let mut r = Retopo::default();
        // 3x3 grid of quads with a displaced middle row
        for j in 0..4 {
            for i in 0..4 {
                let jitter = if (1..3).contains(&i) && (1..3).contains(&j) { 0.3 } else { 0.0 };
                r.add_vertex(Vec3::new(i as f32 + jitter, j as f32, 0.0));
            }
        }
        for j in 0..3u32 {
            for i in 0..3u32 {
                let a = j * 4 + i;
                r.faces.push(vec![a, a + 1, a + 5, a + 4]);
            }
        }
        let corner = r.verts[0];
        r.relax(20, &|p| Vec3::new(p.x, p.y, 0.0));
        assert_eq!(r.verts[0], corner);
        assert!((r.verts[5].x - 1.0).abs() < 0.05, "{}", r.verts[5].x);
    }
}
