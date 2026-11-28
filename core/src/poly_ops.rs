//! More ZModeler operations: QMesh, slide, spin, collapse, split, weld,
//! creases and filling holes.

use crate::poly::Polys;
use glam::Vec3;
use std::collections::{HashMap, HashSet};

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// Splits a vertex cycle at repeated vertices into simple cycles.
fn split_loops(f: Vec<u32>) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for v in f.into_iter() {
        if let Some(i) = stack.iter().position(|&u| u == v) {
            let lp: Vec<u32> = stack.drain(i..).collect();
            stack.push(v);
            if lp.len() >= 3 {
                out.push(lp);
            }
        } else {
            stack.push(v);
        }
    }
    if stack.len() >= 3 {
        out.push(stack);
    }
    out
}

impl Polys {
    /// Face holding directed edge a -> b, and the index of `a` in it.
    fn face_with(&self, a: u32, b: u32) -> Option<(usize, usize)> {
        self.faces.iter().enumerate().find_map(|(fi, f)| {
            (0..f.len()).find(|&i| f[i] == a && f[(i + 1) % f.len()] == b).map(|i| (fi, i))
        })
    }

    /// Replaces vertex `gone` with `keep` everywhere, dropping faces that
    /// become degenerate.
    pub fn merge_verts(&mut self, keep: u32, gone: u32) {
        let mut faces = Vec::with_capacity(self.faces.len());
        let mut groups = Vec::with_capacity(self.faces.len());
        for (f, g) in self.faces.drain(..).zip(self.groups.drain(..)) {
            let mut f: Vec<u32> = f.into_iter().map(|v| if v == gone { keep } else { v }).collect();
            f.dedup();
            while f.len() > 1 && f.first() == f.last() {
                f.pop();
            }
            // a face pinched at a repeated point splits into separate loops
            for lp in split_loops(f) {
                faces.push(lp);
                groups.push(g);
            }
        }
        self.faces = faces;
        self.groups = groups;
        self.drop_small_faces();
        let cr: Vec<(u32, u32)> = self.creases.iter().copied().collect();
        self.creases = cr
            .into_iter()
            .map(|(a, b)| key(if a == gone { keep } else { a }, if b == gone { keep } else { b }))
            .filter(|(a, b)| a != b)
            .collect();
    }

    fn drop_small_faces(&mut self) {
        let keep: Vec<bool> = self.faces.iter().map(|f| {
            let u: HashSet<u32> = f.iter().copied().collect();
            u.len() >= 3
        }).collect();
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        let mut i = 0;
        self.groups.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }

    /// Removes pairs of faces that use the same vertices (back to back).
    fn drop_doubled_faces(&mut self) {
        let mut seen: HashMap<Vec<u32>, usize> = HashMap::new();
        let mut kill = vec![false; self.faces.len()];
        for (i, f) in self.faces.iter().enumerate() {
            let mut k = f.clone();
            k.sort_unstable();
            if let Some(&j) = seen.get(&k) {
                kill[i] = true;
                kill[j] = true;
            } else {
                seen.insert(k, i);
            }
        }
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            !kill[i - 1]
        });
        let mut i = 0;
        self.groups.retain(|_| {
            i += 1;
            !kill[i - 1]
        });
    }

    /// Extrude, then weld the moved vertices onto any point they land on, so
    /// the new part merges into what it touches (faces left back to back go).
    pub fn qmesh(&mut self, region: &[usize], dist: f32) {
        let before: HashSet<u32> = (0..self.pos.len() as u32).collect();
        let region_verts: HashSet<u32> = region.iter().flat_map(|&f| self.faces[f].iter().copied()).collect();
        let shortest = region
            .iter()
            .flat_map(|&f| {
                let f = &self.faces[f];
                (0..f.len()).map(move |i| (f[i], f[(i + 1) % f.len()]))
            })
            .map(|(a, b)| self.pos[a as usize].distance(self.pos[b as usize]))
            .fold(f32::MAX, f32::min);
        self.extrude(region, dist);
        let tol = shortest * 0.3;
        let moved: Vec<u32> = region.iter().flat_map(|&f| self.faces[f].iter().copied()).collect::<HashSet<u32>>().into_iter().collect();
        let used: HashSet<u32> = self.faces.iter().flatten().copied().collect();
        let mut merged = false;
        for v in moved {
            let p = self.pos[v as usize];
            let target = used
                .iter()
                .copied()
                .filter(|u| *u != v && before.contains(u) && !region_verts.contains(u))
                .map(|u| (self.pos[u as usize].distance(p), u))
                .filter(|(d, _)| *d <= tol)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, u)) = target {
                self.merge_verts(u, v);
                merged = true;
            }
        }
        if merged {
            self.drop_doubled_faces();
        }
    }

    /// Slides edge a-b of face `f` along the neighbouring edges: towards the
    /// rest of `f` for t > 0, into the face across the edge for t < 0.
    /// `base` holds the positions to slide from.
    pub fn slide_edge(&mut self, f: usize, e: (u32, u32), t: f32, base: &[Vec3]) {
        let (a, b) = e;
        let face = &self.faces[f];
        let n = face.len();
        let Some(i) = (0..n).find(|&i| face[i] == a && face[(i + 1) % n] == b) else { return };
        let (ta, tb) = if t >= 0.0 {
            (face[(i + n - 1) % n], face[(i + 2) % n])
        } else {
            match self.face_with(b, a) {
                Some((g, j)) => {
                    let gf = &self.faces[g];
                    let m = gf.len();
                    (gf[(j + 2) % m], gf[(j + m - 1) % m])
                }
                None => return,
            }
        };
        let t = t.abs().min(0.95);
        self.pos[a as usize] = base[a as usize].lerp(base[ta as usize], t);
        self.pos[b as usize] = base[b as usize].lerp(base[tb as usize], t);
    }

    /// Turns the edge between two polygons one step around them.
    pub fn spin_edge(&mut self, e: (u32, u32)) -> Result<(), String> {
        let (a, b) = e;
        let (Some((f1, i1)), Some((f2, i2))) = (self.face_with(a, b), self.face_with(b, a)) else {
            return Err("Spin needs an edge between two polygons".into());
        };
        let (p1, p2) = (self.faces[f1].clone(), self.faces[f2].clone());
        let (n1, n2) = (p1.len(), p2.len());
        // the merged outline: f1 from b round to a, then f2 from a round to b
        let mut lp: Vec<u32> = (0..n1).map(|k| p1[(i1 + 1 + k) % n1]).collect(); // b .. a
        lp.extend((1..n2 - 1).map(|k| p2[(i2 + 1 + k) % n2])); // after a .. before b
        let n = lp.len();
        let ia = n1 - 1; // position of a
        let ib = 0; // position of b
        let (x, y) = ((ia + 1) % n, (ib + 1) % n);
        let (lo, hi) = (x.min(y), x.max(y));
        if hi - lo < 2 || n - (hi - lo) < 2 {
            return Err("That edge can't turn".into());
        }
        let fa: Vec<u32> = lp[lo..=hi].to_vec();
        let mut fb: Vec<u32> = lp[hi..].to_vec();
        fb.extend_from_slice(&lp[..=lo]);
        let g = self.groups[f1];
        let (g1, g2) = (self.groups[f1], self.groups[f2]);
        self.faces[f1] = fa;
        self.faces[f2] = fb;
        self.groups[f1] = g1;
        self.groups[f2] = if g2 == g1 { g } else { g2 };
        self.creases.remove(&key(a, b));
        Ok(())
    }

    /// Merges the edge's two points into one at its middle.
    pub fn collapse_edge(&mut self, e: (u32, u32)) -> Result<(), String> {
        let (a, b) = e;
        if a == b {
            return Err("Pick an edge".into());
        }
        let mid = (self.pos[a as usize] + self.pos[b as usize]) * 0.5;
        self.pos[a as usize] = mid;
        self.merge_verts(a, b);
        Ok(())
    }

    /// Merges point `b` into point `a` (at their middle).
    pub fn weld_points(&mut self, a: u32, b: u32) {
        if a != b {
            let mid = (self.pos[a as usize] + self.pos[b as usize]) * 0.5;
            self.pos[a as usize] = mid;
            self.merge_verts(a, b);
        }
    }

    /// Puts new vertex `m` between a and b in every face using that edge.
    fn insert_on_edge(&mut self, a: u32, b: u32, m: u32, skip: usize) {
        for (fi, f) in self.faces.iter_mut().enumerate() {
            if fi == skip {
                continue;
            }
            let n = f.len();
            if let Some(i) = (0..n).find(|&i| (f[i] == a && f[(i + 1) % n] == b) || (f[i] == b && f[(i + 1) % n] == a)) {
                f.insert(i + 1, m);
            }
        }
    }

    /// Cuts face `f` in two, from the middle of edge `e` across to the
    /// opposite edge's middle (or opposite corner).
    pub fn split_face(&mut self, f: usize, e: (u32, u32)) -> Result<(), String> {
        let face = self.faces[f].clone();
        let n = face.len();
        if n < 4 {
            return Err("Split needs a polygon with four or more sides".into());
        }
        let Some(i) = (0..n).find(|&i| face[i] == e.0 && face[(i + 1) % n] == e.1) else {
            return Err("Pick an edge of the polygon".into());
        };
        let rot: Vec<u32> = (0..n).map(|k| face[(i + k) % n]).collect(); // a, b, ...
        let (a, b) = (rot[0], rot[1]);
        let m1 = self.pos.len() as u32;
        self.pos.push((self.pos[a as usize] + self.pos[b as usize]) * 0.5);
        self.mask.push(0.0);
        let g = self.groups[f];
        let (fa, fb, cut) = if n % 2 == 0 {
            let k = n / 2 + 1; // opposite edge rot[k-1] .. rot[k]
            let (c, d) = (rot[k - 1], rot[k % n]);
            let m2 = self.pos.len() as u32;
            self.pos.push((self.pos[c as usize] + self.pos[d as usize]) * 0.5);
            self.mask.push(0.0);
            let mut fa = vec![m1];
            fa.extend_from_slice(&rot[1..k]);
            fa.push(m2);
            let mut fb = vec![m2];
            fb.extend_from_slice(&rot[k..]);
            fb.push(a);
            fb.push(m1);
            (fa, fb, Some((c, d, m2)))
        } else {
            let k = n / 2 + 1; // opposite corner
            let mut fa = vec![m1];
            fa.extend_from_slice(&rot[1..=k]);
            let mut fb = vec![rot[k]];
            fb.extend_from_slice(&rot[k + 1..]);
            fb.push(a);
            fb.push(m1);
            (fa, fb, None)
        };
        self.insert_on_edge(a, b, m1, f);
        if let Some((c, d, m2)) = cut {
            self.insert_on_edge(c, d, m2, f);
        }
        self.faces[f] = fa;
        self.faces.push(fb);
        self.groups.push(g);
        // a creased edge stays creased in both halves
        if self.creases.remove(&key(a, b)) {
            self.creases.insert(key(a, m1));
            self.creases.insert(key(m1, b));
        }
        Ok(())
    }

    /// Fills the hole that open edge `e` lies on with one polygon.
    pub fn fill_hole(&mut self, e: (u32, u32)) -> Result<(), String> {
        let lp = self.hole_loop(e).ok_or("Click an open edge (the border of a hole)")?;
        let g = self.groups.iter().copied().max().map_or(0, |g| g + 1);
        self.faces.push(lp);
        self.groups.push(g);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(p: &Polys) -> bool {
        let m = p.to_mesh();
        !m.boundary.iter().any(|b| *b)
    }

    #[test]
    fn split_spin_collapse_weld_keep_a_closed_cube() {
        let mut p = Polys::qcube(1);
        let e = (p.faces[0][0], p.faces[0][1]);
        p.split_face(0, e).unwrap();
        assert_eq!(p.faces.len(), 7);
        assert!(closed(&p), "split");
        let e2 = (p.faces[0][1], p.faces[0][2]);
        p.spin_edge(e2).unwrap();
        assert!(closed(&p), "spin");
        let f = p.faces[3].clone();
        p.collapse_edge((f[0], f[1])).unwrap();
        assert!(closed(&p), "collapse");
        let n = p.faces.len();
        let f = p.faces[2].clone();
        p.weld_points(f[0], f[2]);
        assert!(p.faces.len() < n);
    }

    #[test]
    fn qmesh_merges_into_what_it_meets() {
        // two cubes' worth: extrude the top of a cube up into a box sitting on it
        let mut p = Polys::qplane(1);
        // a second, separate quad one unit above the first, facing down
        let base = p.pos.len() as u32;
        let up: Vec<Vec3> = p.pos.iter().map(|q| *q + Vec3::Y).collect();
        p.pos.extend(up);
        p.mask.extend(vec![0.0; 4]);
        let mut top = p.faces[0].iter().map(|v| v + base).collect::<Vec<u32>>();
        top.reverse();
        p.faces.push(top);
        p.groups.push(1);
        let n0 = p.faces.len();
        let normal_up = p.face_normal(0).y > 0.0;
        p.qmesh(&[0], if normal_up { 1.0 } else { -1.0 });
        // the extruded cap welded onto the upper quad and both disappeared
        assert_eq!(p.faces.len(), n0 - 2 + 4, "four sides remain, caps gone");
    }

    #[test]
    fn slide_and_fill_hole() {
        let mut p = Polys::qcube(2);
        let base = p.pos.clone();
        let f = 0;
        let e = (p.faces[f][0], p.faces[f][1]);
        p.slide_edge(f, e, 0.5, &base);
        assert!(p.pos[e.0 as usize].distance(base[e.0 as usize]) > 1e-3);
        let mut q = Polys::qcube(2);
        let open_face = q.faces[5].clone();
        q.delete(&[5]);
        assert!(!closed(&q));
        q.fill_hole((open_face[1], open_face[0])).unwrap();
        assert!(closed(&q));
    }
}
