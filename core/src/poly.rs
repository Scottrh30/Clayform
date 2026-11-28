//! Polygon modelling (ZModeler-style). Works on a polygon list — triangles,
//! quads and the occasional n-gon — converted from the sculpt mesh (where a
//! quad is a pair of partner triangles) and back.

use crate::mesh::{Mesh, MeshBuilder, NONE};
use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Polys {
    pub pos: Vec<Vec3>,
    pub mask: Vec<f32>,
    /// Vertex cycles, counter-clockwise seen from outside.
    pub faces: Vec<Vec<u32>>,
    pub groups: Vec<u32>,
    pub export_center: Vec3,
    pub export_scale: f32,
    /// Creased edges (smaller vertex first).
    pub creases: std::collections::HashSet<(u32, u32)>,
}

/// Which polygons an action applies to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Polygon,
    Polygroup,
    /// The strip of quads crossing the picked edge.
    Polyloop,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Polygon, Target::Polygroup, Target::Polyloop];
    pub fn name(self) -> &'static str {
        match self {
            Target::Polygon => "Polygon",
            Target::Polygroup => "Polygroup",
            Target::Polyloop => "Polyloop",
        }
    }
}

/// ZModeler actions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Extrude,
    Inset,
    Bevel,
    InsertEdgeLoop,
    Delete,
    Bridge,
    /// Give the target a new polygroup.
    Polygroup,
    /// Flood a new polygroup over faces facing roughly the same way (PolyGroupIt).
    GroupIt,
    /// Extrude that merges into geometry it meets.
    QMesh,
    SlideEdge,
    Spin,
    Collapse,
    Split,
    WeldPoints,
    Crease,
    /// Fill a hole with a polygon.
    InsertPolygon,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::Extrude,
        Action::QMesh,
        Action::Inset,
        Action::Bevel,
        Action::InsertEdgeLoop,
        Action::SlideEdge,
        Action::Split,
        Action::Spin,
        Action::Collapse,
        Action::WeldPoints,
        Action::Delete,
        Action::Bridge,
        Action::InsertPolygon,
        Action::Crease,
        Action::Polygroup,
        Action::GroupIt,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Action::Extrude => "Extrude",
            Action::Inset => "Inset",
            Action::Bevel => "Bevel",
            Action::InsertEdgeLoop => "Insert edge loop",
            Action::Delete => "Delete",
            Action::Bridge => "Bridge",
            Action::Polygroup => "Polygroup",
            Action::GroupIt => "PolyGroupIt",
            Action::QMesh => "QMesh",
            Action::SlideEdge => "Slide edge",
            Action::Spin => "Spin edge",
            Action::Collapse => "Collapse edge",
            Action::Split => "Split polygon",
            Action::WeldPoints => "Weld points",
            Action::Crease => "Crease",
            Action::InsertPolygon => "Insert polygon",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Action::Extrude => "Drag to pull the target out (or push it in)",
            Action::Inset => "Drag to inset the target, adding a ring of polygons",
            Action::Bevel => "Drag to inset and raise the target, giving a chamfered edge",
            Action::InsertEdgeLoop => "Click an edge to add a loop across the quads it crosses",
            Action::Delete => "Click to delete the target polygons",
            Action::Bridge => "Click one polygon (or hole edge), then another, to connect them",
            Action::Polygroup => "Click to put the target in a new polygroup",
            Action::GroupIt => "Click to group the faces that face roughly the same way as the one clicked",
            Action::QMesh => "Drag to extrude; where it meets other polygons they merge (no faces left inside)",
            Action::SlideEdge => "Drag sideways to slide the edge along its neighbouring edges",
            Action::Spin => "Click an edge between two polygons to turn it one step around them",
            Action::Collapse => "Click an edge to merge its two points into one",
            Action::Split => "Click an edge to cut its polygon in two, across to the opposite side",
            Action::WeldPoints => "Click near a point, then near another: they merge into one",
            Action::Crease => "Click to crease (or uncrease) the edge, or the target's border, for subdivision",
            Action::InsertPolygon => "Click an open edge to fill its hole with a polygon",
        }
    }
    /// Whether dragging sets an amount.
    pub fn drags(self) -> bool {
        matches!(self, Action::Extrude | Action::Inset | Action::Bevel | Action::QMesh | Action::SlideEdge)
    }
}

fn newell_normal(pos: &[Vec3], f: &[u32]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..f.len() {
        let a = pos[f[i] as usize];
        let b = pos[f[(i + 1) % f.len()] as usize];
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n
}

impl Polys {
    /// Converts a mesh: partner triangle pairs become quads. Also returns,
    /// for every mesh triangle, the polygon it belongs to.
    pub fn from_mesh(m: &Mesh) -> (Polys, Vec<u32>) {
        let mut faces = Vec::new();
        let mut groups = Vec::new();
        let mut tri_face = vec![NONE; m.tris.len()];
        for f in 0..m.tris.len() {
            let t = m.tris[f];
            if t[0] == NONE || tri_face[f] != NONE {
                continue;
            }
            let p = m.partner[f];
            let quad = (p != NONE && m.tris[p as usize][0] != NONE).then(|| {
                let q = m.tris[p as usize];
                let shared: Vec<u32> = t.iter().copied().filter(|v| q.contains(v)).collect();
                if shared.len() != 2 {
                    return None;
                }
                let uf = t.iter().position(|v| !q.contains(v))?;
                let up = *q.iter().find(|v| !t.contains(v))?;
                // t rotated to start at its own corner: (uf, s1, s2)
                let s1 = t[(uf + 1) % 3];
                let s2 = t[(uf + 2) % 3];
                Some([t[uf], s1, up, s2])
            });
            let id = faces.len() as u32;
            if let Some(Some(q)) = quad {
                faces.push(q.to_vec());
                tri_face[f] = id;
                tri_face[p as usize] = id;
            } else {
                faces.push(t.to_vec());
                tri_face[f] = id;
            }
            groups.push(m.groups[f]);
        }
        (
            Polys {
                pos: m.positions.clone(),
                mask: m.mask.clone(),
                faces,
                groups,
                export_center: m.export_center,
                export_scale: m.export_scale,
                creases: m.creases.iter().map(|e| (e[0], e[1])).collect(),
            },
            tri_face,
        )
    }

    /// Triangulates back into a sculpt mesh (quads become partner pairs,
    /// split along their shorter diagonal) and drops unused vertices.
    pub fn to_mesh(&self) -> Mesh {
        // keep vertex order, dropping only unused vertices
        let mut used = vec![NONE; self.pos.len()];
        for f in &self.faces {
            for &v in f {
                used[v as usize] = 0;
            }
        }
        let mut pos = Vec::new();
        let mut mask = Vec::new();
        for v in 0..self.pos.len() {
            if used[v] != NONE {
                used[v] = pos.len() as u32;
                pos.push(self.pos[v]);
                mask.push(self.mask.get(v).copied().unwrap_or(0.0));
            }
        }
        let mut tris = Vec::new();
        let mut groups = Vec::new();
        let mut partner = Vec::new();
        for (fi, f) in self.faces.iter().enumerate() {
            let f: Vec<u32> = f.iter().map(|&v| used[v as usize]).collect();
            let g = self.groups[fi];
            match f.len() {
                3 => {
                    tris.push([f[0], f[1], f[2]]);
                    groups.push(g);
                    partner.push(NONE);
                }
                4 => {
                    let p = |i: usize| pos[f[i] as usize];
                    let (t0, t1) = if p(0).distance_squared(p(2)) <= p(1).distance_squared(p(3)) {
                        ([f[0], f[1], f[2]], [f[0], f[2], f[3]])
                    } else {
                        ([f[0], f[1], f[3]], [f[1], f[2], f[3]])
                    };
                    let i = tris.len() as u32;
                    tris.extend([t0, t1]);
                    groups.extend([g, g]);
                    partner.extend([i + 1, i]);
                }
                n if n > 4 => {
                    // n-gon: fan around its centroid
                    let c: Vec3 = f.iter().map(|&v| pos[v as usize]).sum::<Vec3>() / n as f32;
                    let ci = pos.len() as u32;
                    pos.push(c);
                    mask.push(0.0);
                    for k in 0..n {
                        tris.push([f[k], f[(k + 1) % n], ci]);
                        groups.push(g);
                        partner.push(NONE);
                    }
                }
                _ => {}
            }
        }
        let mut m = MeshBuilder::new(pos, tris).with_faces(Some(groups), Some(partner)).finish();
        // creases on edges that still exist
        let live: std::collections::HashSet<(u32, u32)> = self
            .faces
            .iter()
            .flat_map(|f| (0..f.len()).map(move |i| (f[i].min(f[(i + 1) % f.len()]), f[i].max(f[(i + 1) % f.len()]))))
            .collect();
        m.creases = self
            .creases
            .iter()
            .filter(|e| live.contains(e))
            .filter_map(|&(a, b)| {
                let (x, y) = (used[a as usize], used[b as usize]);
                (x != NONE && y != NONE).then(|| [x.min(y), x.max(y)])
            })
            .collect();
        m.mask = mask;
        m.export_center = self.export_center;
        m.export_scale = self.export_scale;
        m
    }

    fn next_group(&self) -> u32 {
        self.groups.iter().copied().max().map_or(0, |g| g + 1)
    }

    fn add_vertex(&mut self, p: Vec3) -> u32 {
        self.pos.push(p);
        self.mask.push(0.0);
        (self.pos.len() - 1) as u32
    }

    /// Directed edge -> (face, index of the edge's first vertex in the face).
    fn edge_map(&self) -> HashMap<(u32, u32), (usize, usize)> {
        let mut m = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 0..f.len() {
                m.insert((f[i], f[(i + 1) % f.len()]), (fi, i));
            }
        }
        m
    }

    pub fn face_normal(&self, f: usize) -> Vec3 {
        newell_normal(&self.pos, &self.faces[f]).normalize_or_zero()
    }

    pub fn face_center(&self, f: usize) -> Vec3 {
        let face = &self.faces[f];
        face.iter().map(|&v| self.pos[v as usize]).sum::<Vec3>() / face.len() as f32
    }

    /// The edge of face `f` closest to point `p`.
    pub fn nearest_edge(&self, f: usize, p: Vec3) -> (u32, u32) {
        let face = &self.faces[f];
        let mut best = (f32::MAX, (face[0], face[1]));
        for i in 0..face.len() {
            let (a, b) = (face[i], face[(i + 1) % face.len()]);
            let (pa, pb) = (self.pos[a as usize], self.pos[b as usize]);
            let ab = pb - pa;
            let t = ((p - pa).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let d = (pa + ab * t).distance_squared(p);
            if d < best.0 {
                best = (d, (a, b));
            }
        }
        best.1
    }

    /// The faces an action targets, picked at face `f` / edge `e`.
    pub fn target_faces(&self, target: Target, f: usize, e: (u32, u32)) -> Vec<usize> {
        match target {
            Target::Polygon => vec![f],
            Target::Polygroup => {
                let g = self.groups[f];
                (0..self.faces.len()).filter(|&i| self.groups[i] == g).collect()
            }
            Target::Polyloop => {
                let ring = self.quad_ring(e);
                if ring.is_empty() { vec![f] } else { ring.into_iter().map(|(q, _)| q).collect() }
            }
        }
    }

    /// Quads crossed by the edge ring through edge `e`: (face, index i such
    /// that the face's edges i and i+2 are the ring edges).
    fn quad_ring(&self, e: (u32, u32)) -> Vec<(usize, usize)> {
        let em = self.edge_map();
        let mut ring: Vec<(usize, usize)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut walk = |start: (u32, u32), ring: &mut Vec<(usize, usize)>, front: bool| {
            let mut edge = start;
            loop {
                let Some(&(fi, i)) = em.get(&edge) else { break };
                let f = &self.faces[fi];
                if f.len() != 4 || !seen.insert(fi) {
                    break;
                }
                if front {
                    ring.push((fi, i));
                } else {
                    ring.insert(0, (fi, i));
                }
                // opposite edge of the quad, entered from the far side
                let (c, d) = (f[(i + 2) % 4], f[(i + 3) % 4]);
                edge = (d, c);
            }
        };
        walk(e, &mut ring, true);
        walk((e.1, e.0), &mut ring, false);
        ring
    }

    fn region_normal(&self, region: &[usize]) -> Vec3 {
        let n: Vec3 = region.iter().map(|&f| newell_normal(&self.pos, &self.faces[f])).sum();
        n.normalize_or_zero()
    }

    /// Directed boundary edges (a, b) of a set of faces, in the faces' winding.
    pub fn boundary_edges(&self, region: &[usize]) -> Vec<(u32, u32)> {
        let inside: std::collections::HashSet<(u32, u32)> = region
            .iter()
            .flat_map(|&f| {
                let face = &self.faces[f];
                (0..face.len()).map(move |i| (face[i], face[(i + 1) % face.len()]))
            })
            .collect();
        inside.iter().copied().filter(|&(a, b)| !inside.contains(&(b, a))).collect()
    }

    /// Moves the region along its normal by `dist`, joining it to the rest
    /// with a band of side quads (new polygroup).
    pub fn extrude(&mut self, region: &[usize], dist: f32) {
        self.lift(region, 0.0, dist, true);
    }

    /// Shrinks the region towards its centre by `amount` (0..1), adding a
    /// ring of quads around it.
    pub fn inset(&mut self, region: &[usize], amount: f32) {
        self.lift(region, amount, 0.0, false);
    }

    /// Inset and raise together: a chamfered edge around the region.
    pub fn bevel(&mut self, region: &[usize], amount: f32, height: f32) {
        self.lift(region, amount, height, false);
    }

    fn lift(&mut self, region: &[usize], inset: f32, height: f32, move_all: bool) {
        if region.is_empty() {
            return;
        }
        let n = self.region_normal(region);
        let boundary = self.boundary_edges(region);
        let region_verts: Vec<u32> = {
            let mut v: Vec<u32> = region.iter().flat_map(|&f| self.faces[f].iter().copied()).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let center: Vec3 = region_verts.iter().map(|&v| self.pos[v as usize]).sum::<Vec3>() / region_verts.len() as f32;
        let on_boundary: std::collections::HashSet<u32> = boundary.iter().flat_map(|&(a, b)| [a, b]).collect();
        let mut map: HashMap<u32, u32> = HashMap::new();
        for &v in &region_verts {
            let p = self.pos[v as usize];
            if on_boundary.contains(&v) {
                let q = p + (center - p) * inset + n * height;
                let nv = self.add_vertex(q);
                map.insert(v, nv);
            } else if move_all || height != 0.0 {
                self.pos[v as usize] = p + n * height;
            }
        }
        for &f in region {
            for v in self.faces[f].iter_mut() {
                if let Some(&nv) = map.get(v) {
                    *v = nv;
                }
            }
        }
        let g = self.next_group();
        for (a, b) in boundary {
            self.faces.push(vec![a, b, map[&b], map[&a]]);
            self.groups.push(g);
        }
    }

    pub fn delete(&mut self, region: &[usize]) {
        let mut keep = vec![true; self.faces.len()];
        for &f in region {
            keep[f] = false;
        }
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

    /// Inserts an edge loop across the quad ring through edge `e`. Faces at
    /// the ends of an open ring get the new vertex in their outline.
    pub fn insert_edge_loop(&mut self, e: (u32, u32)) -> Result<(), String> {
        let ring = self.quad_ring(e);
        if ring.is_empty() {
            return Err("Pick an edge of a quad".into());
        }
        let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |s: &mut Polys, a: u32, b: u32| -> u32 {
            let k = (a.min(b), a.max(b));
            if let Some(&m) = mids.get(&k) {
                return m;
            }
            let p = (s.pos[a as usize] + s.pos[b as usize]) * 0.5;
            let m = s.add_vertex(p);
            mids.insert(k, m);
            m
        };
        let ring_faces: std::collections::HashSet<usize> = ring.iter().map(|r| r.0).collect();
        let mut new_faces = Vec::new();
        for &(fi, i) in &ring {
            let f = self.faces[fi].clone();
            let p = |k: usize| f[(i + k) % 4];
            let (p0, p1, p2, p3) = (p(0), p(1), p(2), p(3));
            let m01 = mid(self, p0, p1);
            let m23 = mid(self, p2, p3);
            self.faces[fi] = vec![p0, m01, m23, p3];
            new_faces.push((vec![m01, p1, p2, m23], self.groups[fi]));
        }
        // faces outside the ring that share a split edge get the new vertex
        for fi in 0..self.faces.len() {
            if ring_faces.contains(&fi) {
                continue;
            }
            let f = self.faces[fi].clone();
            let mut out = Vec::with_capacity(f.len() + 1);
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                out.push(a);
                if let Some(&m) = mids.get(&(a.min(b), a.max(b))) {
                    out.push(m);
                }
            }
            self.faces[fi] = out;
        }
        for (f, g) in new_faces {
            self.faces.push(f);
            self.groups.push(g);
        }
        Ok(())
    }

    /// The vertex loop around the hole that boundary edge `e` lies on, in
    /// the order a face filling the hole would use.
    pub fn hole_loop(&self, e: (u32, u32)) -> Option<Vec<u32>> {
        let em = self.edge_map();
        // boundary edges: (a, b) present but (b, a) missing; the hole runs b -> a
        let (a, b) = if em.contains_key(&e) && !em.contains_key(&(e.1, e.0)) {
            e
        } else if em.contains_key(&(e.1, e.0)) && !em.contains_key(&e) {
            (e.1, e.0)
        } else {
            return None;
        };
        let mut next: HashMap<u32, u32> = HashMap::new();
        for &(x, y) in em.keys() {
            if !em.contains_key(&(y, x)) {
                next.insert(y, x); // hole direction: reverse of the face edge
            }
        }
        let mut lp = vec![b];
        let mut cur = b;
        while let Some(&nx) = next.get(&cur) {
            if nx == b {
                return Some(lp);
            }
            if lp.len() > em.len() {
                return None;
            }
            lp.push(nx);
            cur = nx;
        }
        let _ = a;
        None
    }

    /// Connects two loops with a tube of quads. Each loop is in the winding
    /// of a face that would cover it (a polygon, or a hole's filling).
    /// `remove` lists faces to delete first (bridged polygons).
    pub fn bridge(&mut self, la: &[u32], lb: &[u32], remove: &[usize]) -> Result<(), String> {
        let n = la.len();
        if n != lb.len() || n < 3 {
            return Err(format!("Bridge needs the same number of edges on both sides ({} and {})", n, lb.len()));
        }
        // pair a_i with b_(o - i), choosing the offset with the shortest links
        let o = (0..n)
            .min_by(|&x, &y| {
                let cost = |o: usize| -> f32 {
                    (0..n).map(|i| self.pos[la[i] as usize].distance(self.pos[lb[(o + n - i) % n] as usize])).sum()
                };
                cost(x).total_cmp(&cost(y))
            })
            .unwrap();
        let g = self.next_group();
        let mut faces = Vec::with_capacity(n);
        for i in 0..n {
            let a0 = la[i];
            let a1 = la[(i + 1) % n];
            let x = lb[(o + 2 * n - i - 1) % n];
            let y = lb[(o + n - i) % n];
            faces.push(vec![a0, a1, x, y]);
        }
        self.delete(remove);
        for f in faces {
            self.faces.push(f);
            self.groups.push(g);
        }
        Ok(())
    }

    /// Faces reachable from `f` across edges whose normals stay within
    /// `max_angle` of face `f`'s normal.
    pub fn flood_by_normal(&self, f: usize, max_angle: f32) -> Vec<usize> {
        let em = self.edge_map();
        let n0 = self.face_normal(f);
        let cos = max_angle.cos();
        let mut seen = vec![false; self.faces.len()];
        let mut out = vec![f];
        seen[f] = true;
        let mut stack = vec![f];
        while let Some(g) = stack.pop() {
            let face = &self.faces[g];
            for i in 0..face.len() {
                let (a, b) = (face[i], face[(i + 1) % face.len()]);
                if let Some(&(h, _)) = em.get(&(b, a)) {
                    if !seen[h] && self.face_normal(h).dot(n0) >= cos {
                        seen[h] = true;
                        out.push(h);
                        stack.push(h);
                    }
                }
            }
        }
        out
    }

    /// Splits the whole surface into polygroups of similarly facing faces.
    pub fn auto_groups(&mut self, max_angle: f32) {
        let mut done = vec![false; self.faces.len()];
        let mut g = 0;
        for f in 0..self.faces.len() {
            if done[f] {
                continue;
            }
            for h in self.flood_by_normal(f, max_angle) {
                if !done[h] {
                    done[h] = true;
                    self.groups[h] = g;
                }
            }
            g += 1;
        }
    }

    /// Merges vertices closer than `eps` (used to build primitives).
    fn weld(&mut self, eps: f32) {
        let mut map: HashMap<[i64; 3], u32> = HashMap::new();
        let mut remap = vec![0u32; self.pos.len()];
        let mut pos = Vec::new();
        for (i, p) in self.pos.iter().enumerate() {
            let k = [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
            remap[i] = *map.entry(k).or_insert_with(|| {
                pos.push(*p);
                (pos.len() - 1) as u32
            });
        }
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = remap[*v as usize];
            }
            f.dedup();
        }
        self.mask = vec![0.0; pos.len()];
        self.pos = pos;
    }

    fn empty() -> Polys {
        Polys {
            pos: Vec::new(),
            mask: Vec::new(),
            faces: Vec::new(),
            groups: Vec::new(),
            export_center: Vec3::ZERO,
            export_scale: 1.0,
            creases: Default::default(),
        }
    }

    fn grid(&mut self, origin: Vec3, u: Vec3, v: Vec3, div: u32, group: u32) {
        let base = self.pos.len() as u32;
        for j in 0..=div {
            for i in 0..=div {
                self.pos.push(origin + u * (i as f32 / div as f32) + v * (j as f32 / div as f32));
            }
        }
        let w = div + 1;
        for j in 0..div {
            for i in 0..div {
                let a = base + j * w + i;
                self.faces.push(vec![a, a + 1, a + 1 + w, a + w]);
                self.groups.push(group);
            }
        }
    }

    /// Quad cube, `div` quads per edge, one polygroup per side.
    pub fn qcube(div: u32) -> Polys {
        let div = div.max(1);
        let mut p = Polys::empty();
        let s = 0.8;
        let sides = [
            (Vec3::new(-s, -s, s), Vec3::X, Vec3::Y),   // front +z
            (Vec3::new(s, -s, -s), -Vec3::X, Vec3::Y),  // back -z
            (Vec3::new(s, -s, s), -Vec3::Z, Vec3::Y),   // right +x
            (Vec3::new(-s, -s, -s), Vec3::Z, Vec3::Y),  // left -x
            (Vec3::new(-s, s, s), Vec3::X, -Vec3::Z),   // top +y
            (Vec3::new(-s, -s, -s), Vec3::X, Vec3::Z),  // bottom -y
        ];
        for (g, (o, u, v)) in sides.into_iter().enumerate() {
            p.grid(o, u * 2.0 * s, v * 2.0 * s, div, g as u32);
        }
        p.weld(1e-5);
        p
    }

    /// Quad plane in XZ, facing +Y.
    pub fn qplane(div: u32) -> Polys {
        let mut p = Polys::empty();
        p.grid(Vec3::new(-1.0, 0.0, 1.0), Vec3::X * 2.0, -Vec3::Z * 2.0, div.max(1), 0);
        p.mask = vec![0.0; p.pos.len()];
        p
    }

    /// Quad sphere: a subdivided cube pushed onto a sphere (one polygroup).
    pub fn qsphere(div: u32) -> Polys {
        let mut p = Polys::qcube(div.max(1));
        for q in &mut p.pos {
            *q = q.normalize();
        }
        p.groups.iter_mut().for_each(|g| *g = 0);
        p
    }

    /// Cylinder along Y: `sides` around, `rings` quads high, n-gon caps.
    pub fn qcylinder(sides: u32, rings: u32) -> Polys {
        let (sides, rings) = (sides.max(3), rings.max(1));
        let mut p = Polys::empty();
        let (r, h) = (0.7, 1.6);
        for j in 0..=rings {
            let y = -h / 2.0 + h * j as f32 / rings as f32;
            for i in 0..sides {
                let a = std::f32::consts::TAU * i as f32 / sides as f32;
                p.pos.push(Vec3::new(r * a.cos(), y, -r * a.sin()));
            }
        }
        for j in 0..rings {
            for i in 0..sides {
                let a = j * sides + i;
                let b = j * sides + (i + 1) % sides;
                p.faces.push(vec![a, b, b + sides, a + sides]);
                p.groups.push(0);
            }
        }
        let top: Vec<u32> = (0..sides).map(|i| rings * sides + i).collect();
        let bottom: Vec<u32> = (0..sides).rev().collect();
        p.faces.push(top);
        p.groups.push(1);
        p.faces.push(bottom);
        p.groups.push(2);
        p.mask = vec![0.0; p.pos.len()];
        p
    }
}

/// A distinct, stable colour for polygroup `g` (sRGB, 0..1; golden-ratio hues).
pub fn group_color(g: u32) -> [f32; 3] {
    let h = (g as f32 * 0.618_034 + 0.12).fract() * 6.0;
    let (s, v) = (0.55 + 0.25 * ((g / 7) % 2) as f32, 0.95 - 0.2 * ((g / 3) % 2) as f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, gg, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, gg + m, b + m]
}

/// Quad primitives for polygon modelling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Primitive {
    QCube,
    QSphere,
    QCylinder,
    QPlane,
}

impl Primitive {
    pub const ALL: [Primitive; 4] = [Primitive::QCube, Primitive::QSphere, Primitive::QCylinder, Primitive::QPlane];
    pub fn name(self) -> &'static str {
        match self {
            Primitive::QCube => "QCube",
            Primitive::QSphere => "QSphere",
            Primitive::QCylinder => "QCylinder",
            Primitive::QPlane => "QPlane",
        }
    }
    pub fn build(self, div: u32) -> Polys {
        match self {
            Primitive::QCube => Polys::qcube(div),
            Primitive::QSphere => Polys::qsphere(div.max(2) * 2),
            Primitive::QCylinder => Polys::qcylinder(8 * div.max(1), div.max(1)),
            Primitive::QPlane => Polys::qplane(div),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(m: &Mesh) -> bool {
        let mut c: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &m.tris {
            for e in 0..3 {
                *c.entry((t[e], t[(e + 1) % 3])).or_default() += 1;
            }
        }
        c.iter().all(|(&(a, b), &n)| n == 1 && c.get(&(b, a)) == Some(&1))
    }

    fn outward(m: &Mesh) -> bool {
        // every face normal points away from the centre of mass
        let c: Vec3 = m.positions.iter().copied().sum::<Vec3>() / m.positions.len() as f32;
        (0..m.tris.len()).filter(|&f| {
            let t = m.tris[f];
            let fc = (m.positions[t[0] as usize] + m.positions[t[1] as usize] + m.positions[t[2] as usize]) / 3.0;
            m.face_normal_raw(f).dot(fc - c) < 0.0
        }).count() == 0
    }

    #[test]
    fn groups_by_normal() {
        let mut c = Polys::qcube(2);
        c.groups.iter_mut().for_each(|g| *g = 0);
        c.auto_groups(0.5);
        let mut g = c.groups.clone();
        g.sort_unstable();
        g.dedup();
        assert_eq!(g.len(), 6, "a cube has six sides");
        assert_eq!(c.flood_by_normal(0, 0.5).len(), 4);
    }

    #[test]
    fn primitives_are_closed_quads() {
        let c = Polys::qcube(2);
        assert_eq!(c.faces.len(), 24);
        assert!(c.faces.iter().all(|f| f.len() == 4));
        assert_eq!(c.pos.len(), 26);
        let m = c.to_mesh();
        assert!(closed(&m) && outward(&m));
        let (back, _) = Polys::from_mesh(&m);
        assert_eq!(back.faces.len(), 24, "quads survive the round trip");
        assert_eq!(back.groups.iter().copied().max(), Some(5));
        assert!(closed(&Polys::qcylinder(8, 2).to_mesh()));
        assert!(outward(&Polys::qcylinder(8, 2).to_mesh()));
        assert!(closed(&Polys::qsphere(3).to_mesh()));
    }

    #[test]
    fn extrude_inset_bevel_stay_closed() {
        for which in 0..3 {
            let mut c = Polys::qcube(1);
            let top = (0..c.faces.len()).find(|&f| c.face_normal(f).y > 0.9).unwrap();
            let y0 = c.face_center(top).y;
            match which {
                0 => c.extrude(&[top], 0.5),
                1 => c.inset(&[top], 0.3),
                _ => c.bevel(&[top], 0.3, 0.2),
            }
            assert_eq!(c.faces.len(), 6 + 4);
            let m = c.to_mesh();
            assert!(closed(&m), "action {which}");
            let y1 = c.face_center(top).y;
            match which {
                0 => assert!((y1 - y0 - 0.5).abs() < 1e-5),
                1 => assert!((y1 - y0).abs() < 1e-5),
                _ => assert!((y1 - y0 - 0.2).abs() < 1e-5),
            }
        }
        // extruding a whole polygroup keeps the cube closed too
        let mut c = Polys::qcube(3);
        let f = (0..c.faces.len()).find(|&f| c.face_normal(f).x > 0.9).unwrap();
        let region = c.target_faces(Target::Polygroup, f, (0, 0));
        assert_eq!(region.len(), 9);
        c.extrude(&region, -0.2);
        assert!(closed(&c.to_mesh()));
    }

    #[test]
    fn edge_loop_splits_the_ring() {
        let mut c = Polys::qcube(1);
        let front = (0..c.faces.len()).find(|&f| c.face_normal(f).z > 0.9).unwrap();
        // a vertical edge of the front face: the ring runs around the sides
        let f = c.faces[front].clone();
        let e = (0..4)
            .map(|i| (f[i], f[(i + 1) % 4]))
            .find(|&(a, b)| (c.pos[a as usize].y - c.pos[b as usize].y).abs() > 1.0)
            .unwrap();
        c.insert_edge_loop(e).unwrap();
        assert_eq!(c.faces.len(), 6 + 4);
        let m = c.to_mesh();
        assert!(closed(&m) && outward(&m));
    }

    #[test]
    fn bridge_two_faces_makes_a_tunnel() {
        let mut c = Polys::qcube(1);
        let front = (0..c.faces.len()).find(|&f| c.face_normal(f).z > 0.9).unwrap();
        let back = (0..c.faces.len()).find(|&f| c.face_normal(f).z < -0.9).unwrap();
        c.inset(&[front, back], 0.4);
        let la = c.faces[front].clone();
        let lb = c.faces[back].clone();
        c.bridge(&la, &lb, &[front, back]).unwrap();
        let m = c.to_mesh();
        assert!(closed(&m), "tunnel through the cube is watertight");
        // genus 1: V - E + F = 0
        let e = m.tris.len() * 3 / 2;
        assert_eq!(m.positions.len() as i64 - e as i64 + m.tris.len() as i64, 0);
    }

    #[test]
    fn delete_and_bridge_holes() {
        // a cylinder's caps can be deleted and found again as holes
        let mut c = Polys::qcylinder(6, 1);
        let caps: Vec<usize> = (0..c.faces.len()).filter(|&f| c.faces[f].len() == 6).collect();
        let e0 = (c.faces[caps[0]][0], c.faces[caps[0]][1]);
        c.delete(&caps);
        assert_eq!(c.hole_loop(e0).map(|l| l.len()), Some(6));
        // two facing planes bridged along their open edges make a closed box
        let mut a = Polys::qplane(1);
        let mut b = Polys::qplane(1);
        for p in &mut b.pos {
            p.y += 1.0;
        }
        b.faces[0].reverse(); // faces down
        let n = a.pos.len() as u32;
        a.pos.extend(b.pos.iter().copied());
        a.mask.extend(b.mask.iter().copied());
        a.faces.push(b.faces[0].iter().map(|v| v + n).collect());
        a.groups.push(1);
        let ea = (a.faces[0][0], a.faces[0][1]);
        let eb = (a.faces[1][0], a.faces[1][1]);
        let la = a.hole_loop(ea).unwrap();
        let lb = a.hole_loop(eb).unwrap();
        a.bridge(&la, &lb, &[]).unwrap();
        assert_eq!(a.faces.len(), 6);
        assert!(closed(&a.to_mesh()));
    }
}
