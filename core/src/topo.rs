//! Topology tools: automatic quad remeshing, decimation, closing holes,
//! mirror-and-weld and edge creasing.

use crate::bvh::Bvh;
use crate::mesh::{Mesh, MeshBuilder, NONE};
use crate::poly::Polys;
use crate::remesh::{voxel_remesh_with, VoxelSettings, MAX_RESOLUTION, MIN_RESOLUTION};
use crate::sculpt::{Sculptor, Undo};
use glam::{DVec3, Vec3};
use std::collections::{BinaryHeap, HashMap, HashSet};

fn tri_area(m: &Mesh, t: &[u32; 3]) -> f32 {
    let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
    (b - a).cross(c - a).length() * 0.5
}

fn surface_area(m: &Mesh) -> f32 {
    m.tris.iter().filter(|t| t[0] != NONE).map(|t| tri_area(m, t)).sum()
}

/// Polygon-edge neighbours (quad diagonals left out).
fn poly_neighbors(p: &Polys) -> Vec<Vec<u32>> {
    let mut nbr: Vec<Vec<u32>> = vec![Vec::new(); p.pos.len()];
    for f in &p.faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            if !nbr[a as usize].contains(&b) {
                nbr[a as usize].push(b);
            }
            if !nbr[b as usize].contains(&a) {
                nbr[b as usize].push(a);
            }
        }
    }
    nbr
}

// ------------------------------------------------------------------ quad remesh

/// Automatic retopology: even quads at about `target` polygons, relaxed and
/// projected onto `src`. Optionally keeps `src`'s polygroups. The edge flow is
/// even but follows the voxel grid, not the model's features.
pub fn quad_remesh(
    src: &Mesh,
    target: usize,
    keep_groups: bool,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Mesh, String> {
    let area = surface_area(src);
    if area <= 0.0 {
        return Err("The model has no surface".into());
    }
    let h = (area / target.max(50) as f32).sqrt();
    let (mn, mx) = src.bounds();
    let res = ((mx - mn).max_element() / h).round().clamp(MIN_RESOLUTION as f32, MAX_RESOLUTION as f32) as u32;
    let mut settings = VoxelSettings { resolution: res, smooth: 0, project: false, signed: false };
    let mut report = |p: f32| progress(p);
    let mut quads = voxel_remesh_with(src, &settings, &mut |p| report(p * 0.3))?;
    // the count scales with resolution squared: correct once if it's well off
    let got = (quads.tris.len() / 2).max(1) as f32;
    let want = target.max(50) as f32;
    if got > want * 1.2 || got < want * 0.8 {
        let r2 = (res as f32 * (want / got).sqrt()).round().clamp(MIN_RESOLUTION as f32, MAX_RESOLUTION as f32) as u32;
        if r2 != res {
            settings.resolution = r2;
            quads = voxel_remesh_with(src, &settings, &mut |p| report(0.3 + p * 0.3))?;
        }
    }
    let (mut polys, _) = Polys::from_mesh(&quads);
    let nbr = poly_neighbors(&polys);
    let bvh = Bvh::build(src);
    let reach = h * 4.0;
    // relax along the surface, snapping back each round
    let iters = 12;
    for it in 0..iters {
        if !progress(0.6 + 0.3 * it as f32 / iters as f32) {
            return Err(crate::remesh::CANCELLED.into());
        }
        let prev = polys.pos.clone();
        for v in 0..prev.len() {
            if nbr[v].len() < 2 {
                continue;
            }
            let avg = nbr[v].iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nbr[v].len() as f32;
            let q = prev[v].lerp(avg, 0.6);
            polys.pos[v] = bvh.closest_point(src, q, reach).unwrap_or(q);
        }
    }
    if keep_groups {
        for (fi, f) in polys.faces.iter().enumerate() {
            let c = f.iter().map(|&v| polys.pos[v as usize]).sum::<Vec3>() / f.len() as f32;
            if let Some((_, t)) = bvh.closest(src, c, reach) {
                polys.groups[fi] = src.groups[t as usize];
            }
        }
    } else {
        polys.groups.iter_mut().for_each(|g| *g = 0);
    }
    polys.export_center = src.export_center;
    polys.export_scale = src.export_scale;
    let mut m = polys.to_mesh();
    m.compute_all_normals();
    progress(1.0);
    Ok(m)
}

// ------------------------------------------------------------------ decimation

fn ring_of(vf: &[Vec<u32>], tris: &[[u32; 3]], v: u32) -> HashSet<u32> {
    vf[v as usize].iter().flat_map(|&f| tris[f as usize]).filter(|&u| u != v).collect()
}

type Quadric = [f64; 10];

fn plane_quadric(n: DVec3, p: DVec3, w: f64) -> Quadric {
    let d = -n.dot(p);
    let (a, b, c) = (n.x, n.y, n.z);
    [a * a * w, a * b * w, a * c * w, a * d * w, b * b * w, b * c * w, b * d * w, c * c * w, c * d * w, d * d * w]
}

fn q_add(a: &mut Quadric, b: &Quadric) {
    for i in 0..10 {
        a[i] += b[i];
    }
}

fn q_err(q: &Quadric, p: DVec3) -> f64 {
    let (x, y, z) = (p.x, p.y, p.z);
    q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z
        + 2.0 * q[6] * y
        + q[7] * z * z
        + 2.0 * q[8] * z
        + q[9]
}

/// Position minimising the quadric, if well conditioned.
fn q_optimal(q: &Quadric) -> Option<DVec3> {
    let m = glam::DMat3::from_cols(
        DVec3::new(q[0], q[1], q[2]),
        DVec3::new(q[1], q[4], q[5]),
        DVec3::new(q[2], q[5], q[7]),
    );
    let det = m.determinant();
    if det.abs() < 1e-12 {
        return None;
    }
    Some(m.inverse() * DVec3::new(-q[3], -q[6], -q[8]))
}

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    a: u32,
    b: u32,
    stamp: (u32, u32),
    pos: DVec3,
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // a min-heap on cost
        o.cost.total_cmp(&self.cost)
    }
}

/// Quadric-error decimation down to about `target` triangles, keeping the
/// shape and detail where it matters. Open edges (and polygroup borders with
/// `keep_groups`) are held in place.
pub fn decimate(m: &Mesh, target: usize, keep_groups: bool, progress: &mut dyn FnMut(f32) -> bool) -> Result<Mesh, String> {
    let mut pos: Vec<DVec3> = m.positions.iter().map(|p| p.as_dvec3()).collect();
    let mut tris: Vec<[u32; 3]> = m.tris.clone();
    let mut alive: Vec<bool> = tris.iter().map(|t| t[0] != NONE).collect();
    let mut live = alive.iter().filter(|a| **a).count();
    let start = live;
    if target >= live {
        return Ok(m.clone());
    }
    let nv = pos.len();
    let mut vf: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (f, t) in tris.iter().enumerate() {
        if alive[f] {
            for &v in t {
                vf[v as usize].push(f as u32);
            }
        }
    }
    let mut quad: Vec<Quadric> = vec![[0.0; 10]; nv];
    let face_n = |pos: &[DVec3], t: &[u32; 3]| -> DVec3 {
        (pos[t[1] as usize] - pos[t[0] as usize]).cross(pos[t[2] as usize] - pos[t[0] as usize])
    };
    for (f, t) in tris.iter().enumerate() {
        if !alive[f] {
            continue;
        }
        let n = face_n(&pos, t);
        let a2 = n.length();
        if a2 < 1e-20 {
            continue;
        }
        let q = plane_quadric(n / a2, pos[t[0] as usize], a2 * 0.5);
        for &v in t {
            q_add(&mut quad[v as usize], &q);
        }
    }
    // edges with their faces: borders (one face) and group borders get walls
    let mut edge_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (f, t) in tris.iter().enumerate() {
        if alive[f] {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                edge_faces.entry((a.min(b), a.max(b))).or_default().push(f as u32);
            }
        }
    }
    let mut locked = vec![false; nv];
    for (&(a, b), fs) in &edge_faces {
        let border = fs.len() == 1 || (keep_groups && fs.len() == 2 && m.groups[fs[0] as usize] != m.groups[fs[1] as usize]);
        if !border {
            continue;
        }
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let e = pb - pa;
        let n = face_n(&pos, &tris[fs[0] as usize]).normalize_or_zero();
        let wall = e.cross(n).normalize_or_zero();
        let q = plane_quadric(wall, pa, e.length_squared() * 1000.0);
        q_add(&mut quad[a as usize], &q);
        q_add(&mut quad[b as usize], &q);
        if fs.len() == 1 {
            locked[a as usize] = true;
            locked[b as usize] = true;
        }
    }
    let mut stamp = vec![0u32; nv];
    let mut mask: Vec<f32> = m.mask.clone();
    let cand = |a: u32, b: u32, pos: &[DVec3], quad: &[Quadric], stamp: &[u32], locked: &[bool]| -> Cand {
        let mut q = quad[a as usize];
        q_add(&mut q, &quad[b as usize]);
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let p = match (locked[a as usize], locked[b as usize]) {
            (true, false) => pa,
            (false, true) => pb,
            _ => {
                let mut best = (q_err(&q, (pa + pb) * 0.5), (pa + pb) * 0.5);
                for c in [pa, pb].into_iter().chain(q_optimal(&q).filter(|o| o.distance(pa) < pa.distance(pb) * 2.0)) {
                    let e = q_err(&q, c);
                    if e < best.0 {
                        best = (e, c);
                    }
                }
                best.1
            }
        };
        Cand { cost: q_err(&q, p).max(0.0), a, b, stamp: (stamp[a as usize], stamp[b as usize]), pos: p }
    };
    let mut heap: BinaryHeap<Cand> = edge_faces.keys().map(|&(a, b)| cand(a, b, &pos, &quad, &stamp, &locked)).collect();
    let mut steps = 0usize;
    while live > target {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.a, c.b);
        if stamp[a as usize] != c.stamp.0 || stamp[b as usize] != c.stamp.1 || vf[a as usize].is_empty() || vf[b as usize].is_empty() {
            continue;
        }
        steps += 1;
        if steps % 4096 == 0 && !progress((start - live) as f32 / (start - target).max(1) as f32) {
            return Err(crate::remesh::CANCELLED.into());
        }
        if locked[a as usize] && locked[b as usize] {
            continue;
        }
        // faces on the edge, and the link condition (no pinching)
        let shared: Vec<u32> = vf[a as usize].iter().copied().filter(|f| tris[*f as usize].contains(&b)).collect();
        if shared.is_empty() {
            continue;
        }
        let (ra, rb) = (ring_of(&vf, &tris, a), ring_of(&vf, &tris, b));
        let common = ra.intersection(&rb).count();
        if common != shared.len() {
            continue;
        }
        // no face may flip or collapse
        let p = c.pos;
        let mut ok = true;
        for &v in &[a, b] {
            for &f in &vf[v as usize] {
                let t = tris[f as usize];
                if t.contains(&a) && t.contains(&b) {
                    continue;
                }
                let before = face_n(&pos, &t);
                let mut moved = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
                for k in 0..3 {
                    if t[k] == v {
                        moved[k] = p;
                    }
                }
                let after = (moved[1] - moved[0]).cross(moved[2] - moved[0]);
                if after.length_squared() < 1e-24 || before.normalize_or_zero().dot(after.normalize_or_zero()) < 0.3 {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
        }
        if !ok {
            continue;
        }
        // collapse b into a
        pos[a as usize] = p;
        let qb = quad[b as usize];
        q_add(&mut quad[a as usize], &qb);
        mask[a as usize] = mask[a as usize].max(mask[b as usize]);
        locked[a as usize] |= locked[b as usize];
        for &f in &shared {
            alive[f as usize] = false;
            live -= 1;
            for &v in &tris[f as usize] {
                vf[v as usize].retain(|&x| x != f);
            }
        }
        let bf = std::mem::take(&mut vf[b as usize]);
        for f in bf {
            for v in tris[f as usize].iter_mut() {
                if *v == b {
                    *v = a;
                }
            }
            vf[a as usize].push(f);
        }
        stamp[a as usize] += 1;
        stamp[b as usize] = u32::MAX;
        for u in ring_of(&vf, &tris, a) {
            heap.push(cand(a.min(u), a.max(u), &pos, &quad, &stamp, &locked));
        }
    }
    // compact
    let mut remap = vec![NONE; nv];
    let mut out_pos = Vec::new();
    let mut out_mask = Vec::new();
    let mut out_tris = Vec::new();
    let mut groups = Vec::new();
    for (f, t) in tris.iter().enumerate() {
        if !alive[f] {
            continue;
        }
        let mut nt = [0u32; 3];
        for k in 0..3 {
            let v = t[k] as usize;
            if remap[v] == NONE {
                remap[v] = out_pos.len() as u32;
                out_pos.push(pos[v].as_vec3());
                out_mask.push(mask[v]);
            }
            nt[k] = remap[v];
        }
        out_tris.push(nt);
        groups.push(m.groups[f]);
    }
    let mut out = MeshBuilder::new(out_pos, out_tris).with_faces(Some(groups), None).finish();
    out.mask = out_mask;
    out.export_center = m.export_center;
    out.export_scale = m.export_scale;
    progress(1.0);
    Ok(out)
}

// ------------------------------------------------------------------ holes

/// Directed loops around every hole, each in the order a filling face would use.
fn hole_loops(p: &Polys) -> Vec<Vec<u32>> {
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    for f in &p.faces {
        for i in 0..f.len() {
            edges.insert((f[i], f[(i + 1) % f.len()]));
        }
    }
    let mut next: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &edges {
        if !edges.contains(&(b, a)) {
            next.insert(b, a);
        }
    }
    let mut seen: HashSet<u32> = HashSet::new();
    let mut loops = Vec::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        if seen.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        seen.insert(s);
        let mut cur = s;
        let mut closed = false;
        while let Some(&n) = next.get(&cur) {
            if n == s {
                closed = true;
                break;
            }
            if !seen.insert(n) {
                break;
            }
            lp.push(n);
            cur = n;
        }
        if closed && lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

/// Fills every hole with a smooth patch of quads (a new polygroup each).
/// Returns the mesh and how many holes were closed.
pub fn close_holes(m: &Mesh) -> (Mesh, usize) {
    let (mut p, _) = Polys::from_mesh(m);
    let loops = hole_loops(&p);
    if loops.is_empty() {
        return (m.clone(), 0);
    }
    let mut g = p.groups.iter().copied().max().map_or(0, |g| g + 1);
    let mut interior: Vec<u32> = Vec::new();
    for lp in &loops {
        let n = lp.len();
        if n <= 4 {
            p.faces.push(lp.clone());
            p.groups.push(g);
            g += 1;
            continue;
        }
        let c: Vec3 = lp.iter().map(|&v| p.pos[v as usize]).sum::<Vec3>() / n as f32;
        let rings = (n / 8).clamp(1, 12);
        let mut prev: Vec<u32> = lp.clone();
        for r in 1..=rings {
            let t = r as f32 / (rings + 1) as f32;
            let ring: Vec<u32> = lp
                .iter()
                .map(|&v| {
                    let q = p.pos[v as usize].lerp(c, t);
                    p.pos.push(q);
                    p.mask.push(0.0);
                    let id = (p.pos.len() - 1) as u32;
                    interior.push(id);
                    id
                })
                .collect();
            for i in 0..n {
                let j = (i + 1) % n;
                p.faces.push(vec![prev[i], prev[j], ring[j], ring[i]]);
                p.groups.push(g);
            }
            prev = ring;
        }
        p.pos.push(c);
        p.mask.push(0.0);
        let ci = (p.pos.len() - 1) as u32;
        interior.push(ci);
        for i in 0..n {
            p.faces.push(vec![prev[i], prev[(i + 1) % n], ci]);
            p.groups.push(g);
        }
        g += 1;
    }
    // relax the new vertices into a smooth membrane
    let nbr = poly_neighbors(&p);
    for _ in 0..60 {
        let prev = p.pos.clone();
        for &v in &interior {
            let nb = &nbr[v as usize];
            if !nb.is_empty() {
                p.pos[v as usize] = nb.iter().map(|&u| prev[u as usize]).sum::<Vec3>() / nb.len() as f32;
            }
        }
    }
    let count = loops.len();
    (p.to_mesh(), count)
}

// ------------------------------------------------------------------ mirror

/// Replaces the -X half with a mirror of the +X half, welded along the middle.
pub fn mirror_weld(m: &Mesh) -> Result<Mesh, String> {
    let (mut p, _) = Polys::from_mesh(m);
    let keep: Vec<bool> = (0..p.faces.len()).map(|f| p.face_center(f).x >= 0.0).collect();
    if !keep.iter().any(|k| *k) {
        return Err("Nothing on the +X side to mirror".into());
    }
    let mut i = 0;
    p.faces.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    let mut i = 0;
    p.groups.retain(|_| {
        i += 1;
        keep[i - 1]
    });
    let used: HashSet<u32> = p.faces.iter().flatten().copied().collect();
    // edge length scale for the weld tolerance
    let (mn, mx) = m.bounds();
    let eps = (mx - mn).max_element() * 1e-4;
    // vertices on (or pulled onto) the middle plane are shared by both halves
    let boundary: HashSet<u32> = hole_loops(&p).into_iter().flatten().collect();
    let mut avg_edge = 0.0f32;
    let mut ne = 0.0f32;
    for f in &p.faces {
        for k in 0..f.len() {
            avg_edge += p.pos[f[k] as usize].distance(p.pos[f[(k + 1) % f.len()] as usize]);
            ne += 1.0;
        }
    }
    avg_edge /= ne.max(1.0);
    for &v in &used {
        let x = p.pos[v as usize].x;
        if x < 0.0 || (boundary.contains(&v) && x < avg_edge * 0.75) {
            p.pos[v as usize].x = 0.0;
        }
    }
    let n = p.pos.len();
    let mut map = vec![NONE; n];
    for &v in &used {
        let q = p.pos[v as usize];
        map[v as usize] = if q.x.abs() <= eps {
            v
        } else {
            p.pos.push(Vec3::new(-q.x, q.y, q.z));
            p.mask.push(p.mask[v as usize]);
            (p.pos.len() - 1) as u32
        };
    }
    let faces = p.faces.clone();
    let groups = p.groups.clone();
    for (f, g) in faces.into_iter().zip(groups) {
        if f.iter().all(|&v| map[v as usize] == v) {
            continue;
        }
        let mut mf: Vec<u32> = f.iter().map(|&v| map[v as usize]).collect();
        mf.reverse();
        mf.dedup();
        if mf.len() >= 3 {
            p.faces.push(mf);
            p.groups.push(g);
        }
    }
    let creases: Vec<(u32, u32)> = p.creases.iter().copied().collect();
    for (a, b) in creases {
        if map.get(a as usize).is_some_and(|x| *x != NONE) && map.get(b as usize).is_some_and(|x| *x != NONE) {
            let (x, y) = (map[a as usize], map[b as usize]);
            p.creases.insert((x.min(y), x.max(y)));
        }
    }
    Ok(p.to_mesh())
}

// ------------------------------------------------------------------ creases

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CreaseMode {
    /// Edges between polygroups.
    GroupBorders,
    /// Edges whose faces meet at more than this angle (degrees).
    Angle(f32),
    /// Open edges.
    Borders,
    Clear,
}

/// Creased edges of `m` for `mode` (Clear gives none).
pub fn crease_edges(m: &Mesh, mode: CreaseMode) -> Vec<[u32; 2]> {
    let mut edge_faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE {
            continue;
        }
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(f as u32);
        }
    }
    let mut out: Vec<[u32; 2]> = edge_faces
        .into_iter()
        .filter(|(_, fs)| match mode {
            CreaseMode::GroupBorders => fs.len() == 2 && m.groups[fs[0] as usize] != m.groups[fs[1] as usize],
            CreaseMode::Angle(deg) => {
                fs.len() == 2 && {
                    // quad diagonals are never creased
                    let (f0, f1) = (fs[0], fs[1]);
                    m.partner[f0 as usize] != f1
                        && m.face_normals[f0 as usize].dot(m.face_normals[f1 as usize]) < deg.to_radians().cos()
                }
            }
            CreaseMode::Borders => fs.len() == 1,
            CreaseMode::Clear => false,
        })
        .map(|(k, _)| [k.0, k.1])
        .collect();
    out.sort_unstable();
    out
}

impl Sculptor {
    pub fn crease_count(&self) -> usize {
        self.mesh.creases.len()
    }

    /// Creases (or clears, or adds to) edges of the active level, carrying
    /// them up through the higher levels (one undo step).
    pub fn set_creases(&mut self, mode: CreaseMode, add: bool) {
        self.end_stroke();
        let snap = self.snapshot();
        let mut edges = crease_edges(&self.mesh, mode);
        if add && mode != CreaseMode::Clear {
            edges.extend(self.mesh.creases.iter().copied());
            edges.sort_unstable();
            edges.dedup();
        }
        self.mesh.creases = edges;
        self.carry_creases_up();
        self.set_label(if mode == CreaseMode::Clear { "Uncrease" } else { "Crease" });
        self.push_undo(Undo::State(snap));
        self.add_revision();
    }

    /// Creases the edges (or uncreases them, if all already are), one undo step.
    pub fn toggle_creases(&mut self, edges: &[(u32, u32)]) -> bool {
        self.end_stroke();
        let mut cur: HashSet<[u32; 2]> = self.mesh.creases.iter().copied().collect();
        let keys: Vec<[u32; 2]> = edges.iter().map(|&(a, b)| [a.min(b), a.max(b)]).collect();
        let all_on = keys.iter().all(|k| cur.contains(k));
        let snap = self.snapshot();
        for k in &keys {
            if all_on {
                cur.remove(k);
            } else {
                cur.insert(*k);
            }
        }
        let mut list: Vec<[u32; 2]> = cur.into_iter().collect();
        list.sort_unstable();
        self.mesh.creases = list;
        self.carry_creases_up();
        self.set_label(if all_on { "Uncrease" } else { "Crease" });
        self.push_undo(Undo::State(snap));
        !all_on
    }

    /// Higher levels' creases follow the active level's (each edge splits in two).
    fn carry_creases_up(&mut self) {
        let mut cur = self.mesh.creases.clone();
        let mut n_coarse = self.mesh.positions.len() as u32;
        for k in self.level + 1..self.levels.len() {
            let map: HashMap<(u32, u32), u32> =
                self.levels[k].odd.iter().enumerate().map(|(i, s)| ((s[0].min(s[1]), s[0].max(s[1])), n_coarse + i as u32)).collect();
            let mut next = Vec::with_capacity(cur.len() * 2);
            for &[a, b] in &cur {
                if let Some(&m) = map.get(&(a, b)) {
                    next.push([a.min(m), a.max(m)]);
                    next.push([b.min(m), b.max(m)]);
                }
            }
            next.sort_unstable();
            if let Some(mesh) = self.levels[k].mesh.as_mut() {
                n_coarse = mesh.positions.len() as u32;
                mesh.creases = next.clone();
            }
            cur = next;
        }
    }

    pub(crate) fn export_creases(&self, out: &mut Vec<u8>) {
        let per_level: Vec<&[[u32; 2]]> = (0..self.levels.len())
            .map(|k| if k == self.level { self.mesh.creases.as_slice() } else { self.levels[k].mesh.as_ref().map_or(&[][..], |m| m.creases.as_slice()) })
            .collect();
        if per_level.iter().all(|c| c.is_empty()) {
            return;
        }
        let mut b = Vec::new();
        crate::detail::put_u32(&mut b, per_level.len() as u32);
        for c in per_level {
            crate::detail::put_u32(&mut b, c.len() as u32);
            for e in c {
                crate::detail::put_u32(&mut b, e[0]);
                crate::detail::put_u32(&mut b, e[1]);
            }
        }
        crate::detail::put_chunk(out, b"CRSE", &b);
    }

    pub(crate) fn import_creases(&mut self, body: &[u8]) {
        let mut r = crate::detail::Reader { b: body, at: 0 };
        let Some(n) = r.u32() else { return };
        for k in 0..n as usize {
            let Some(c) = r.u32() else { return };
            let mut edges = Vec::with_capacity(c as usize);
            for _ in 0..c {
                let (Some(a), Some(b)) = (r.u32(), r.u32()) else { return };
                edges.push([a, b]);
            }
            let mesh = if k == self.level { Some(&mut self.mesh) } else { self.levels.get_mut(k).and_then(|l| l.mesh.as_mut()) };
            if let Some(m) = mesh {
                let nv = m.positions.len() as u32;
                m.creases = edges.into_iter().filter(|e| e[0] < nv && e[1] < nv).collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_remesh_gives_quads_near_the_target_on_the_surface() {
        let src = Mesh::icosphere(5);
        let m = quad_remesh(&src, 800, true, &mut |_| true).unwrap();
        let quads = m.partner.iter().filter(|p| **p != NONE).count() / 2;
        let tris = m.tris.len();
        assert!(quads * 2 * 10 >= tris * 9, "mostly quads: {quads} quads of {tris} triangles");
        assert!((400..1600).contains(&quads), "{quads} quads for a target of 800");
        assert!(m.positions.iter().all(|p| (p.length() - 1.0).abs() < 0.02));
        assert!(!m.boundary.iter().any(|b| *b));
    }

    #[test]
    fn decimation_keeps_the_shape_with_fewer_triangles() {
        let src = Mesh::icosphere(5);
        let m = decimate(&src, src.tris.len() / 10, false, &mut |_| true).unwrap();
        assert!(m.tris.len() <= src.tris.len() / 10 + 2 && m.tris.len() > src.tris.len() / 20);
        assert!(m.positions.iter().all(|p| (p.length() - 1.0).abs() < 0.02));
        assert!(!m.boundary.iter().any(|b| *b), "stays closed");
        // open meshes keep their border
        let plane = Mesh::plane(5);
        let d = decimate(&plane, plane.tris.len() / 8, false, &mut |_| true).unwrap();
        let (a, b) = (plane.bounds(), d.bounds());
        assert!((a.0 - b.0).length() < 1e-4 && (a.1 - b.1).length() < 1e-4);
    }

    #[test]
    fn holes_close_and_mirror_welds() {
        // cut a cap off a sphere
        let mut p = Polys::from_mesh(&Mesh::icosphere(4)).0;
        let keep: Vec<bool> = (0..p.faces.len()).map(|f| p.face_center(f).y < 0.6).collect();
        let mut i = 0;
        p.faces.retain(|_| { i += 1; keep[i - 1] });
        p.groups.truncate(p.faces.len());
        let open = p.to_mesh();
        assert!(open.boundary.iter().any(|b| *b));
        let (closed, n) = close_holes(&open);
        assert_eq!(n, 1);
        assert!(!closed.boundary.iter().any(|b| *b));
        let top = closed.positions.iter().map(|q| q.y).fold(f32::MIN, f32::max);
        assert!(top > 0.55 && top < 0.7, "a gentle cap, top at {top}");
        // mirror: squash the -X side, mirror the +X side back
        let mut s = Mesh::icosphere(4);
        for q in &mut s.positions {
            if q.x < 0.0 {
                q.x *= 0.3;
            }
        }
        let mm = mirror_weld(&s).unwrap();
        assert!(!mm.boundary.iter().any(|b| *b), "welded shut");
        let (mn, mx) = mm.bounds();
        assert!((mn.x + mx.x).abs() < 1e-3 && mx.x > 0.95);
    }

    #[test]
    fn creases_keep_subdivision_sharp_and_carry_up() {
        let mut s = Sculptor::new(Polys::qcube(1).to_mesh());
        let (_, m0) = s.mesh.bounds();
        s.set_creases(CreaseMode::Angle(30.0), false);
        assert_eq!(s.crease_count(), 12, "the cube's edges");
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        let (mn, mx) = s.mesh.bounds();
        assert!((mx.x - m0.x).abs() < 1e-4 && (mn.x + m0.x).abs() < 1e-4, "corners stay put: {mx:?}");
        assert_eq!(s.crease_count(), 48);
        // creasing a lower level updates the levels above
        s.set_level(0);
        s.set_creases(CreaseMode::Clear, false);
        s.set_level(2);
        assert_eq!(s.crease_count(), 0);
        let (_, mx2) = s.mesh.bounds();
        assert!(mx2.x < m0.x * 0.95, "uncreased cube rounds off: {mx2:?}");
        // saved and loaded with the project extras
        s.undo(); // level
        s.undo(); // uncrease
        assert_eq!(s.level(), 0);
        assert_eq!(s.crease_count(), 12);
        let bytes = s.export_extras();
        let (levels, active) = s.clone_levels();
        let mut t = Sculptor::from_levels(levels, active, None);
        for l in 0..3 {
            t.set_level(l);
            t.mesh.creases.clear();
        }
        t.set_level(0);
        t.import_extras(&bytes);
        assert_eq!(t.crease_count(), 12);
        t.set_level(2);
        assert_eq!(t.crease_count(), 48);
    }
}
