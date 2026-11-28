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
    /// Topology strokes drawn over the sculpt (turned into faces on demand).
    pub strokes: Vec<Vec<Vec3>>,
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
        self.strokes.clear();
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
            creases: Default::default(),
        }
    }
}

impl Retopo {
    /// Open edges (on only one face).
    pub fn open_edges(&self) -> Vec<(u32, u32)> {
        let mut count: HashMap<(u32, u32), (u32, (u32, u32))> = HashMap::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                let e = count.entry((a.min(b), a.max(b))).or_insert((0, (a, b)));
                e.0 += 1;
            }
        }
        count.into_values().filter(|c| c.0 == 1).map(|c| c.1).collect()
    }

    /// Continues the strip of quads across open edge a-b with a quad the
    /// same size as the one behind it (snapping to nearby vertices).
    /// Returns false if the edge isn't open.
    pub fn extend_edge(&mut self, a: u32, b: u32, project: &dyn Fn(Vec3) -> Vec3, eye: Vec3) -> bool {
        let adjacent = |f: &Vec<u32>| (0..f.len()).any(|i| {
            let (x, y) = (f[i], f[(i + 1) % f.len()]);
            (x == a && y == b) || (x == b && y == a)
        });
        let on: Vec<&Vec<u32>> = self.faces.iter().filter(|f| adjacent(f)).collect();
        if on.len() != 1 {
            return false;
        }
        let face = on[0].clone();
        let n = face.len();
        let ia = face.iter().position(|&v| v == a).unwrap();
        let ib = face.iter().position(|&v| v == b).unwrap();
        // the face's neighbours of a and b away from the edge
        let a_in = if face[(ia + 1) % n] == b { face[(ia + n - 1) % n] } else { face[(ia + 1) % n] };
        let b_in = if face[(ib + 1) % n] == a { face[(ib + n - 1) % n] } else { face[(ib + 1) % n] };
        let (pa, pb) = (self.verts[a as usize], self.verts[b as usize]);
        let tol = pa.distance(pb) * 0.35;
        let place = |s: &mut Retopo, p: Vec3, skip: [u32; 2]| -> u32 {
            let q = project(p);
            let near = (0..s.verts.len() as u32)
                .filter(|v| !skip.contains(v))
                .map(|v| (s.verts[v as usize].distance(q), v))
                .filter(|(d, _)| *d < tol)
                .min_by(|x, y| x.0.total_cmp(&y.0));
            match near {
                Some((_, v)) => v,
                None => s.add_vertex(q),
            }
        };
        let na = place(self, pa + (pa - self.verts[a_in as usize]), [a, b]);
        let nb = place(self, pb + (pb - self.verts[b_in as usize]), [a, b]);
        if na == nb {
            return self.add_face(vec![a, b, na], eye);
        }
        self.add_face(vec![a, b, nb, na], eye)
    }

    /// Topology brush: where the drawn strokes cross become vertices, and the
    /// cells between crossings become faces. `normal_at` gives the surface
    /// normal near a point. Returns how many faces were added.
    pub fn build_from_strokes(&mut self, tol: f32, normal_at: &dyn Fn(Vec3) -> Vec3, eye: Vec3) -> usize {
        let strokes = std::mem::take(&mut self.strokes);
        // crossings: (stroke, distance along it, point)
        let mut hits: Vec<Vec<(f32, Vec3)>> = vec![Vec::new(); strokes.len()];
        let cum: Vec<Vec<f32>> = strokes
            .iter()
            .map(|s| {
                let mut c = vec![0.0];
                for w in s.windows(2) {
                    c.push(c.last().unwrap() + w[0].distance(w[1]));
                }
                c
            })
            .collect();
        for i in 0..strokes.len() {
            for j in i + 1..strokes.len() {
                let (si, sj) = (&strokes[i], &strokes[j]);
                for a in 0..si.len().saturating_sub(1) {
                    for b in 0..sj.len().saturating_sub(1) {
                        let (p, q, s, t) = closest_segments(si[a], si[a + 1], sj[b], sj[b + 1]);
                        if p.distance(q) <= tol {
                            let x = (p + q) * 0.5;
                            let di = cum[i][a] + si[a].distance(si[a + 1]) * s;
                            let dj = cum[j][b] + sj[b].distance(sj[b + 1]) * t;
                            hits[i].push((di, x));
                            hits[j].push((dj, x));
                        }
                    }
                }
            }
        }
        // one node per cluster of nearby crossings
        let mut nodes: Vec<Vec3> = Vec::new();
        let node_of = |nodes: &mut Vec<Vec3>, p: Vec3| -> usize {
            if let Some(k) = nodes.iter().position(|n| n.distance(p) <= tol * 1.5) {
                return k;
            }
            nodes.push(p);
            nodes.len() - 1
        };
        let mut edges: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for h in hits.iter_mut() {
            h.sort_by(|a, b| a.0.total_cmp(&b.0));
            let ids: Vec<usize> = h.iter().map(|&(_, p)| node_of(&mut nodes, p)).collect();
            for w in ids.windows(2) {
                if w[0] != w[1] {
                    edges.insert((w[0].min(w[1]), w[0].max(w[1])));
                }
            }
        }
        if nodes.len() < 3 {
            self.strokes = strokes;
            return 0;
        }
        // neighbours of each node in turning order around its normal
        let mut nbr: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        for &(a, b) in &edges {
            nbr[a].push(b);
            nbr[b].push(a);
        }
        for (v, list) in nbr.iter_mut().enumerate() {
            let n = normal_at(nodes[v]).normalize_or(Vec3::Y);
            let t1 = n.any_orthonormal_vector();
            let t2 = n.cross(t1);
            list.sort_by(|&x, &y| {
                let ang = |u: usize| {
                    let d = nodes[u] - nodes[v];
                    d.dot(t2).atan2(d.dot(t1))
                };
                ang(x).total_cmp(&ang(y))
            });
        }
        // trace the cells: from u->v, turn to the neighbour just before u around v
        let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        let mut cells: Vec<Vec<usize>> = Vec::new();
        for &(a, b) in &edges {
            for (u0, v0) in [(a, b), (b, a)] {
                if used.contains(&(u0, v0)) {
                    continue;
                }
                let mut cell = vec![u0];
                let (mut u, mut v) = (u0, v0);
                let mut ok = true;
                loop {
                    used.insert((u, v));
                    if v == u0 {
                        break;
                    }
                    cell.push(v);
                    if cell.len() > 8 {
                        ok = false;
                        break;
                    }
                    let list = &nbr[v];
                    let k = list.iter().position(|&x| x == u).unwrap();
                    let w = list[(k + list.len() - 1) % list.len()];
                    u = v;
                    v = w;
                }
                if ok && (3..=6).contains(&cell.len()) {
                    cells.push(cell);
                }
            }
        }
        // the largest-looking cells are usually the outside: keep cells whose
        // vertices aren't all shared with a smaller, already-kept cell
        let base = self.verts.len() as u32;
        let tol_v = tol * 1.5;
        let mut map: Vec<u32> = Vec::with_capacity(nodes.len());
        for &p in &nodes {
            let near = (0..base).find(|&v| self.verts[v as usize].distance(p) <= tol_v);
            map.push(near.unwrap_or_else(|| self.add_vertex(p)));
        }
        let mut added = 0;
        for c in cells {
            let f: Vec<u32> = c.iter().map(|&k| map[k]).collect();
            if self.add_face(f, eye) {
                added += 1;
            }
        }
        // drop node vertices that no face used
        let used_v: std::collections::HashSet<u32> = self.faces.iter().flatten().copied().collect();
        let mut v = self.verts.len() as u32;
        while v > base {
            v -= 1;
            if !used_v.contains(&v) {
                self.remove_vertex(v);
            }
        }
        added
    }
}

/// Closest points of segments p0-p1 and q0-q1, with their parameters.
fn closest_segments(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (Vec3, Vec3, f32, f32) {
    let d1 = p1 - p0;
    let d2 = q1 - q0;
    let r = p0 - q0;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-12 && e <= 1e-12 {
        return (p0, q0, 0.0, 0.0);
    }
    if a <= 1e-12 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-12 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut ss = if denom > 1e-12 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut tt = (b * ss + f) / e;
            if tt < 0.0 {
                tt = 0.0;
                ss = (-c / a).clamp(0.0, 1.0);
            } else if tt > 1.0 {
                tt = 1.0;
                ss = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = ss;
            t = tt;
        }
    }
    (p0 + d1 * s, q0 + d2 * t, s, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_extend_and_strokes_make_a_grid() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let flat = |p: Vec3| Vec3::new(p.x, p.y, 0.0);
        let mut r = Retopo::default();
        let v: Vec<u32> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|&(x, y)| r.add_vertex(Vec3::new(x, y, 0.0))).collect();
        assert!(r.add_face(v.clone(), eye));
        assert!(r.extend_edge(v[1], v[2], &flat, eye));
        assert_eq!(r.faces.len(), 2);
        assert!(r.verts.iter().any(|p| (p.x - 2.0).abs() < 1e-4 && p.y.abs() < 1e-4));
        assert!(!r.extend_edge(v[0], v[2], &flat, eye), "not an edge");
        // a 4 x 4 tic-tac-toe of strokes makes a 3 x 3 grid of quads
        let mut t = Retopo::default();
        for k in 0..4 {
            let x = k as f32;
            t.strokes.push((0..=30).map(|i| Vec3::new(x, -0.5 + i as f32 * 0.14, 0.0)).collect());
            t.strokes.push((0..=30).map(|i| Vec3::new(-0.5 + i as f32 * 0.14, x, 0.0)).collect());
        }
        let n = t.build_from_strokes(0.05, &|_| Vec3::Z, eye);
        assert_eq!(n, 9, "{} faces", t.faces.len());
        assert_eq!(t.verts.len(), 16);
        assert!(t.faces.iter().all(|f| f.len() == 4));
        assert!(t.strokes.is_empty());
    }

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
