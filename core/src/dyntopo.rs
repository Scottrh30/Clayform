//! Dynamic topology (like SculptGL's "dynamic" mode or Sculptris Pro):
//! under the brush, edges longer than the detail size are split and edges
//! much shorter are collapsed, so resolution follows where you sculpt.

use crate::mesh::{Marker, Mesh, NONE};
use glam::Vec3;

/// Edges shorter than `detail * COLLAPSE_RATIO` are collapsed.
pub const COLLAPSE_RATIO: f32 = 0.4;
/// Safety limit on splits per dab.
const MAX_SPLITS_PER_DAB: usize = 40_000;

/// Splits edge a-b at its midpoint (both adjacent faces). Returns the new
/// vertex. `touched` receives every vertex whose ring changed.
pub fn split_edge(m: &mut Mesh, a: u32, b: u32, touched: &mut Vec<u32>) -> u32 {
    let mid = (m.positions[a as usize] + m.positions[b as usize]) * 0.5;
    let nrm = (m.normals[a as usize] + m.normals[b as usize]).normalize_or_zero();
    let mask = (m.mask[a as usize] + m.mask[b as usize]) * 0.5;
    let faces: Vec<u32> = m.vert_faces(a).iter().copied().filter(|&f| m.tris[f as usize].contains(&b)).collect();
    let nv = m.add_vertex(mid, nrm, mask);
    for f in faces {
        let t = m.tris[f as usize];
        // orient so (x, y) is the edge in this face's winding order
        let i = (0..3).find(|&i| (t[i] == a && t[(i + 1) % 3] == b) || (t[i] == b && t[(i + 1) % 3] == a)).unwrap();
        let (x, y, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
        m.set_face(f, [x, nv, c]);
        m.add_face([nv, y, c]);
        touched.push(c);
    }
    touched.extend([a, b, nv]);
    nv
}

fn normal_of(m: &Mesh, t: [u32; 3], moved: u32, to: Vec3) -> Vec3 {
    let p = |v: u32| if v == moved { to } else { m.positions[v as usize] };
    (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]))
}

/// Whether collapsing a-b (into their midpoint) keeps the surface manifold
/// and doesn't flip any surrounding triangle.
pub fn can_collapse(m: &Mesh, a: u32, b: u32) -> bool {
    if m.boundary[a as usize] || m.boundary[b as usize] {
        return false;
    }
    let (na, nb) = (m.neighbors(a), m.neighbors(b));
    if !na.contains(&b) {
        return false;
    }
    // link condition: exactly the two opposite corners are shared
    let common: Vec<u32> = na.iter().copied().filter(|x| nb.contains(x)).collect();
    if common.len() != 2 || na.len() + nb.len() < 8 {
        return false;
    }
    // the two opposite corners each lose an edge; keep them at valence >= 3
    if common.iter().any(|&c| m.neighbors(c).len() <= 3 || m.boundary[c as usize]) {
        return false;
    }
    let mid = (m.positions[a as usize] + m.positions[b as usize]) * 0.5;
    for (v, other) in [(a, b), (b, a)] {
        for &f in m.vert_faces(v) {
            let t = m.tris[f as usize];
            if t.contains(&other) {
                continue;
            }
            let before = m.face_normal_raw(f as usize);
            let after = normal_of(m, t, v, mid);
            if before.dot(after) <= 0.2 * before.length() * after.length() {
                return false;
            }
        }
    }
    true
}

/// Collapses edge a-b: `a` moves to the midpoint and takes over `b`'s
/// faces, the two faces on the edge are deleted, `b` becomes an orphan.
pub fn collapse_edge(m: &mut Mesh, a: u32, b: u32, touched: &mut Vec<u32>) {
    let mid = (m.positions[a as usize] + m.positions[b as usize]) * 0.5;
    m.positions[a as usize] = mid;
    m.mask[a as usize] = (m.mask[a as usize] + m.mask[b as usize]) * 0.5;
    let shared: Vec<u32> = m.vert_faces(a).iter().copied().filter(|&f| m.tris[f as usize].contains(&b)).collect();
    for f in shared {
        touched.extend(m.tris[f as usize]);
        m.kill_face(f);
    }
    let b_faces: Vec<u32> = m.vert_faces(b).to_vec();
    for f in b_faces {
        let mut t = m.tris[f as usize];
        for v in &mut t {
            if *v == b {
                *v = a;
            }
        }
        touched.extend(t);
        m.set_face(f, t);
    }
    touched.extend(m.neighbors(b).iter().copied());
    touched.push(a);
    touched.push(b);
}

/// Statistics from one remesh pass.
#[derive(Default, Debug, Clone, Copy)]
pub struct RemeshStats {
    pub splits: usize,
    pub collapses: usize,
}

/// Re-tessellates the part of the surface inside the brush sphere so edges
/// end up roughly `detail` long. `fmark`/`vmark` are scratch markers.
/// Returns stats; `changed` receives all vertices whose ring/position changed.
pub fn remesh_region(
    m: &mut Mesh,
    seed_tri: u32,
    center: Vec3,
    radius: f32,
    detail: f32,
    fmark: &mut Marker,
    vmark: &mut Marker,
    changed: &mut Vec<u32>,
) -> RemeshStats {
    let mut stats = RemeshStats::default();
    if !m.is_alive(seed_tri) || detail <= 0.0 {
        return stats;
    }
    let r2 = radius * radius;
    let split2 = detail * detail;
    let collapse2 = (detail * COLLAPSE_RATIO).powi(2);
    let inside = |m: &Mesh, v: u32| m.positions[v as usize].distance_squared(center) < r2;

    // faces touching the brush sphere, found by flood fill from the hit face
    let mut faces: Vec<u32> = Vec::new();
    fmark.begin(m.tris.len());
    vmark.begin(m.vertex_count());
    let mut stack = vec![seed_tri];
    fmark.mark(seed_tri);
    while let Some(f) = stack.pop() {
        faces.push(f);
        for &v in &m.tris[f as usize] {
            if !vmark.mark(v) || (!inside(m, v) && !m.tris[seed_tri as usize].contains(&v)) {
                continue;
            }
            for &g in m.vert_faces(v) {
                if fmark.mark(g) {
                    stack.push(g);
                }
            }
        }
    }

    let mut touched: Vec<u32> = Vec::new();

    // --- split long edges, longest first, a few passes
    for _pass in 0..6 {
        let mut cand: Vec<(f32, u32, u32)> = Vec::new();
        for &f in &faces {
            let t = m.tris[f as usize];
            if t[0] == NONE {
                continue;
            }
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                if a > b && m.edge_face_count(a, b) == 2 {
                    continue; // each interior edge once
                }
                let pa = m.positions[a as usize];
                let pb = m.positions[b as usize];
                let l2 = pa.distance_squared(pb);
                if l2 > split2 && ((pa + pb) * 0.5).distance_squared(center) < r2 {
                    cand.push((l2, a, b));
                }
            }
        }
        if cand.is_empty() {
            break;
        }
        cand.sort_by(|x, y| y.0.total_cmp(&x.0));
        let before = m.tris.len();
        for (_, a, b) in cand {
            if stats.splits >= MAX_SPLITS_PER_DAB {
                break;
            }
            // still an edge, still long?
            if !m.vert_faces(a).iter().any(|&f| m.tris[f as usize].contains(&b)) {
                continue;
            }
            if m.positions[a as usize].distance_squared(m.positions[b as usize]) <= split2 {
                continue;
            }
            split_edge(m, a, b, &mut touched);
            stats.splits += 1;
        }
        faces.extend(before as u32..m.tris.len() as u32);
        // rings of touched vertices must be current for the next pass
        for &v in &touched {
            m.rebuild_ring(v);
        }
        changed.append(&mut touched);
    }

    // --- collapse short edges
    let mut cand: Vec<(f32, u32, u32)> = Vec::new();
    for &f in &faces {
        let t = m.tris[f as usize];
        if t[0] == NONE {
            continue;
        }
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            if a > b {
                continue;
            }
            let pa = m.positions[a as usize];
            let pb = m.positions[b as usize];
            let l2 = pa.distance_squared(pb);
            if l2 < collapse2 && ((pa + pb) * 0.5).distance_squared(center) < r2 {
                cand.push((l2, a, b));
            }
        }
    }
    cand.sort_by(|x, y| x.0.total_cmp(&y.0));
    for (_, a, b) in cand {
        if m.vert_faces(a).is_empty() || m.vert_faces(b).is_empty() {
            continue; // already collapsed away
        }
        if m.positions[a as usize].distance_squared(m.positions[b as usize]) >= collapse2 {
            continue;
        }
        if !can_collapse(m, a, b) {
            continue;
        }
        collapse_edge(m, a, b, &mut touched);
        stats.collapses += 1;
        touched.sort_unstable();
        touched.dedup();
        for &v in &touched {
            m.rebuild_ring(v);
        }
        changed.append(&mut touched);
    }
    changed.sort_unstable();
    changed.dedup();
    changed.retain(|&v| !m.vert_faces(v).is_empty());
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bvh::Bvh;

    fn closed_manifold(m: &Mesh) -> bool {
        use std::collections::HashMap;
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in m.tris.iter().filter(|t| t[0] != NONE) {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        count.values().all(|&c| c == 2)
    }

    fn front(m: &Mesh) -> u32 {
        Bvh::build(m).raycast(m, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap().tri
    }

    #[test]
    fn split_refines_and_stays_manifold() {
        let mut m = Mesh::icosphere(3);
        let n0 = m.live_tri_count();
        let seed = front(&m);
        let (mut fm, mut vm) = (Marker::new(), Marker::new());
        let mut changed = Vec::new();
        let s = remesh_region(&mut m, seed, Vec3::Z, 0.5, 0.03, &mut fm, &mut vm, &mut changed);
        assert!(s.splits > 50, "{s:?}");
        assert!(m.live_tri_count() > n0 + 100);
        assert!(closed_manifold(&m));
        m.compact();
        assert!(closed_manifold(&m));
        assert!(m.boundary.iter().all(|b| !b));
    }

    #[test]
    fn collapse_coarsens_and_stays_manifold() {
        let mut m = Mesh::icosphere(5);
        let n0 = m.live_tri_count();
        let seed = front(&m);
        let (mut fm, mut vm) = (Marker::new(), Marker::new());
        let mut changed = Vec::new();
        let s = remesh_region(&mut m, seed, Vec3::Z, 0.5, 0.3, &mut fm, &mut vm, &mut changed);
        assert!(s.collapses > 20, "{s:?}");
        assert!(m.live_tri_count() < n0);
        assert!(closed_manifold(&m));
        m.compact();
        assert!(closed_manifold(&m));
        assert!(m.normals.iter().all(|n| n.is_finite()));
    }
}
