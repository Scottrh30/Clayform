//! Selection, masking and hiding: mask tools (grow, shrink, blur, sharpen,
//! by cavity or peaks), hiding faces, the plane cuts behind Clip, Trim and
//! Slice curves, and Extract (a masked area becomes a thick shell).

use crate::mesh::{Mesh, MeshBuilder, NONE};
use crate::sculpt::{Sculptor, Undo};
use glam::{Vec2, Vec3};
use std::collections::HashMap;

/// A cutting plane: points with `n·p > d` are on the cut side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub n: Vec3,
    pub d: f32,
}

impl Plane {
    pub fn through(point: Vec3, n: Vec3) -> Self {
        let n = n.normalize_or_zero();
        Plane { n, d: n.dot(point) }
    }
    #[inline]
    pub fn eval(&self, p: Vec3) -> f32 {
        self.n.dot(p) - self.d
    }
    pub fn flipped(self) -> Self {
        Plane { n: -self.n, d: -self.d }
    }
    /// The plane mirrored by `m` about `c` (symmetry copies).
    pub fn transformed(self, m: glam::Mat3, c: Vec3) -> Self {
        let q = m * (self.n * self.d - c) + c;
        Plane::through(q, m * self.n)
    }
}

/// A screen-space selection outline.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Vec2, Vec2),
    Lasso(Vec<Vec2>),
}

impl Shape {
    pub fn contains(&self, p: Vec2) -> bool {
        match self {
            Shape::Rect(a, b) => {
                let (mn, mx) = (a.min(*b), a.max(*b));
                p.x >= mn.x && p.x <= mx.x && p.y >= mn.y && p.y <= mx.y
            }
            Shape::Lasso(pts) => point_in_polygon(pts, p),
        }
    }
    /// The outline as a closed loop of points.
    pub fn outline(&self) -> Vec<Vec2> {
        match self {
            Shape::Rect(a, b) => vec![*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)],
            Shape::Lasso(p) => p.clone(),
        }
    }
}

pub fn point_in_polygon(pts: &[Vec2], p: Vec2) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ------------------------------------------------------------- plane cut

/// Splits every face the plane crosses. Returns the new mesh and, per face,
/// whether it lies on the cut side. Mask, paint, UVs, polygroups, creases
/// and hidden faces carry over; new points sit exactly on the plane.
pub fn split_by_plane(m: &Mesh, pl: Plane) -> (Mesh, Vec<bool>) {
    let size = {
        let (a, b) = m.bounds();
        (b - a).max_element().max(1e-6)
    };
    let eps = size * 1e-5;
    let s: Vec<f32> = m.positions.iter().map(|&p| pl.eval(p)).collect();
    let sign = |v: u32| {
        let x = s[v as usize];
        if x > eps {
            1i8
        } else if x < -eps {
            -1
        } else {
            0
        }
    };
    let mut pos = m.positions.clone();
    let mut mask = m.mask.clone();
    let painted = m.colors.len() == m.positions.len();
    let mut colors = if painted { m.colors.clone() } else { Vec::new() };
    let has_uv = m.uvs.len() == m.tris.len();
    let has_hidden = m.hidden.len() == m.tris.len();
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    let mut tris = Vec::with_capacity(m.tris.len() + 64);
    let mut groups = Vec::with_capacity(m.tris.len() + 64);
    let mut side = Vec::with_capacity(m.tris.len() + 64);
    let mut uvs = Vec::new();
    let mut hidden = Vec::new();
    let mut old_to_new = vec![NONE; m.tris.len()];
    let mut was_split = vec![false; m.tris.len()];
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE {
            continue;
        }
        let sg = t.map(sign);
        let push = |tris: &mut Vec<[u32; 3]>, groups: &mut Vec<u32>, side: &mut Vec<bool>, uvs: &mut Vec<[Vec2; 3]>, hidden: &mut Vec<bool>, tri: [u32; 3], uv: [Vec2; 3], pos_side: bool| {
            tris.push(tri);
            groups.push(m.groups[f]);
            side.push(pos_side);
            if has_uv {
                uvs.push(uv);
            }
            if has_hidden {
                hidden.push(m.hidden[f]);
            }
        };
        let uv = if has_uv { m.uvs[f] } else { [Vec2::ZERO; 3] };
        if !sg.contains(&1) || !sg.contains(&-1) {
            old_to_new[f] = tris.len() as u32;
            push(&mut tris, &mut groups, &mut side, &mut uvs, &mut hidden, *t, uv, sg.contains(&1));
            continue;
        }
        was_split[f] = true;
        // walk the corners, adding a point where an edge crosses
        let mut poly_pos: Vec<(u32, Vec2)> = Vec::new();
        let mut poly_neg: Vec<(u32, Vec2)> = Vec::new();
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let (sa, sb) = (sg[k], sg[(k + 1) % 3]);
            if sa >= 0 {
                poly_pos.push((a, uv[k]));
            }
            if sa <= 0 {
                poly_neg.push((a, uv[k]));
            }
            if sa * sb < 0 {
                let (fa, fb) = (s[a as usize], s[b as usize]);
                let w = fa / (fa - fb);
                let key = (a.min(b), a.max(b));
                let nv = *cut.entry(key).or_insert_with(|| {
                    // interpolate from the key's order so both faces agree
                    let (x, y) = key;
                    let wx = s[x as usize] / (s[x as usize] - s[y as usize]);
                    pos.push(m.positions[x as usize].lerp(m.positions[y as usize], wx));
                    mask.push(m.mask[x as usize] + (m.mask[y as usize] - m.mask[x as usize]) * wx);
                    if painted {
                        colors.push(crate::mesh::mix_color(m.colors[x as usize], m.colors[y as usize]));
                    }
                    (pos.len() - 1) as u32
                });
                let u = uv[k].lerp(uv[(k + 1) % 3], w);
                poly_pos.push((nv, u));
                poly_neg.push((nv, u));
            }
        }
        for (poly, ps) in [(&poly_pos, true), (&poly_neg, false)] {
            for i in 1..poly.len().saturating_sub(1) {
                let tri = [poly[0].0, poly[i].0, poly[i + 1].0];
                if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
                    continue;
                }
                push(&mut tris, &mut groups, &mut side, &mut uvs, &mut hidden, tri, [poly[0].1, poly[i].1, poly[i + 1].1], ps);
            }
        }
    }
    // quads survive where neither half was split
    let partner: Vec<u32> = {
        let mut p = vec![NONE; tris.len()];
        for (f, &nf) in old_to_new.iter().enumerate() {
            if nf == NONE {
                continue;
            }
            let q = m.partner[f];
            if q != NONE && !was_split[q as usize] && old_to_new[q as usize] != NONE {
                p[nf as usize] = old_to_new[q as usize];
            }
        }
        p
    };
    let mut creases = Vec::new();
    for &[a, b] in &m.creases {
        match cut.get(&(a, b)) {
            Some(&c) => creases.extend([[a.min(c), a.max(c)], [b.min(c), b.max(c)]]),
            None => creases.push([a, b]),
        }
    }
    let mut out = MeshBuilder::new(pos, tris).with_faces(Some(groups), Some(partner)).finish();
    out.mask = mask;
    out.colors = colors;
    if has_uv {
        out.uvs = uvs;
    }
    if has_hidden && hidden.contains(&true) {
        out.hidden = hidden;
    }
    out.creases = creases;
    out.export_center = m.export_center;
    out.export_scale = m.export_scale;
    (out, side)
}

/// Drops everything on the cut side and caps the opening flat (a new
/// polygroup). Errors when nothing (or everything) would be cut away.
pub fn trim_by_plane(m: &Mesh, pl: Plane) -> Result<Mesh, String> {
    let (mut out, side) = split_by_plane(m, pl);
    let cut = side.iter().filter(|&&s| s).count();
    if cut == 0 {
        return Err("The line doesn't cross the model".into());
    }
    if cut == side.len() {
        return Err("That would cut away the whole model".into());
    }
    let size = {
        let (a, b) = out.bounds();
        (b - a).max_element().max(1e-6)
    };
    // boundary edges already lying in the plane were open before: leave them
    let eps = size * 1e-4;
    let on_plane = |p: Vec3| pl.eval(p).abs() < eps;
    let mut was_open: std::collections::HashSet<(u32, u32)> = Default::default();
    for (f, t) in out.tris.iter().enumerate() {
        if side[f] || t[0] == NONE {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if out.edge_face_count(a, b) == 1 {
                was_open.insert((a.min(b), a.max(b)));
            }
        }
    }
    for f in 0..side.len() {
        if side[f] {
            out.kill_face(f as u32);
        }
    }
    // the new opening: open edges on the plane that weren't open before,
    // directed as they run in their (kept) face
    let mut next: HashMap<u32, u32> = HashMap::new();
    for (f, t) in out.tris.iter().enumerate() {
        if t[0] == NONE || side[f] {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if out.edge_face_count(a, b) == 1
                && !was_open.contains(&(a.min(b), a.max(b)))
                && on_plane(out.positions[a as usize])
                && on_plane(out.positions[b as usize])
            {
                // the cap runs the other way round
                next.insert(b, a);
            }
        }
    }
    let loops = chain_loops(&next);
    let g = out.groups.iter().copied().max().map_or(0, |g| g + 1);
    let caps = cap_loops(&out.positions, &loops, pl.n);
    let has_uv = out.uvs.len() == out.tris.len();
    for t in caps {
        out.add_face_in_group(t, g);
        if has_uv {
            out.uvs.push([Vec2::ZERO; 3]);
        }
    }
    out.compact();
    out.compute_all_normals();
    Ok(out)
}

/// Joins directed edges (from -> to) into closed loops.
fn chain_loops(next: &HashMap<u32, u32>) -> Vec<Vec<u32>> {
    let mut used: std::collections::HashSet<u32> = Default::default();
    let mut loops = Vec::new();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        if used.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        used.insert(s);
        let mut v = s;
        let mut closed = false;
        while let Some(&n) = next.get(&v) {
            if n == s {
                closed = true;
                break;
            }
            if !used.insert(n) {
                break;
            }
            lp.push(n);
            v = n;
        }
        if closed && lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

/// Triangulates planar loops (outlines and the holes inside them) facing `n`.
fn cap_loops(pos: &[Vec3], loops: &[Vec<u32>], n: Vec3) -> Vec<[u32; 3]> {
    if loops.is_empty() {
        return Vec::new();
    }
    let u = n.any_orthonormal_vector();
    let w = n.cross(u);
    let p2 = |v: u32| {
        let p = pos[v as usize];
        Vec2::new(p.dot(u), p.dot(w))
    };
    let polys: Vec<Vec<Vec2>> = loops.iter().map(|l| l.iter().map(|&v| p2(v)).collect()).collect();
    // nesting depth: even = outline, odd = hole of the loop around it
    let inside_of = |i: usize, j: usize| i != j && point_in_polygon(&polys[j], polys[i][0]);
    let depth: Vec<usize> = (0..loops.len()).map(|i| (0..loops.len()).filter(|&j| inside_of(i, j)).count()).collect();
    let mut out = Vec::new();
    for o in (0..loops.len()).filter(|&i| depth[i] % 2 == 0) {
        let holes: Vec<usize> = (0..loops.len()).filter(|&h| depth[h] == depth[o] + 1 && inside_of(h, o)).collect();
        let mut flat: Vec<f64> = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        let mut hole_idx = Vec::new();
        for (k, &li) in std::iter::once(&o).chain(holes.iter()).enumerate() {
            if k > 0 {
                hole_idx.push(ids.len());
            }
            for (q, &v) in polys[li].iter().zip(&loops[li]) {
                flat.extend([q.x as f64, q.y as f64]);
                ids.push(v);
            }
        }
        let Ok(idx) = earcutr::earcut(&flat, &hole_idx, 2) else { continue };
        for t in idx.chunks_exact(3) {
            let tri = [ids[t[0]], ids[t[1]], ids[t[2]]];
            let (a, b, c) = (pos[tri[0] as usize], pos[tri[1] as usize], pos[tri[2] as usize]);
            if (b - a).cross(c - a).dot(n) >= 0.0 {
                out.push(tri);
            } else {
                out.push([tri[0], tri[2], tri[1]]);
            }
        }
    }
    out
}

// --------------------------------------------------------------- extract

/// The masked area (faces whose corners are all at least half masked, and
/// shown) as a closed shell `thickness` thick, grown outward from the
/// surface. `smooth` relaxes its outline.
pub fn extract_shell(m: &Mesh, thickness: f32, smooth: u32) -> Result<Mesh, String> {
    let mut remap = vec![NONE; m.positions.len()];
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    for (f, t) in m.tris.iter().enumerate() {
        if t[0] == NONE || m.is_hidden(f as u32) || !t.iter().all(|&v| m.mask[v as usize] >= 0.5) {
            continue;
        }
        tris.push(t.map(|v| {
            if remap[v as usize] == NONE {
                remap[v as usize] = pos.len() as u32;
                pos.push(m.positions[v as usize]);
            }
            remap[v as usize]
        }));
    }
    if tris.is_empty() {
        return Err("Mask the area to extract first".into());
    }
    let mut skin = Mesh::new(pos, tris);
    // relax the outline along itself so it isn't a staircase of triangles
    let n = skin.vertex_count();
    let ring: Vec<Vec<u32>> = (0..n as u32)
        .map(|v| {
            if !skin.boundary[v as usize] {
                return Vec::new();
            }
            skin.neighbors(v).iter().copied().filter(|&u| skin.boundary[u as usize] && skin.edge_face_count(u, v) == 1).collect()
        })
        .collect();
    for _ in 0..smooth * 12 {
        let prev = skin.positions.clone();
        for v in 0..n {
            if ring[v].len() == 2 {
                let avg = (prev[ring[v][0] as usize] + prev[ring[v][1] as usize]) * 0.5;
                skin.positions[v] = prev[v].lerp(avg, 0.5);
            }
        }
    }
    skin.compute_all_normals();
    let t = thickness.abs().max(1e-5);
    let mut p = skin.positions.clone();
    let k = p.len() as u32;
    p.extend(skin.positions.iter().zip(&skin.normals).map(|(&q, &nn)| q + nn * t));
    let mut faces = Vec::new();
    let mut groups = Vec::new();
    for tr in &skin.tris {
        // outer surface (offset) keeps the winding, inner one is reversed
        faces.push([tr[0] + k, tr[1] + k, tr[2] + k]);
        groups.push(0);
        faces.push([tr[0], tr[2], tr[1]]);
        groups.push(1);
        for e in 0..3 {
            let (a, b) = (tr[e], tr[(e + 1) % 3]);
            if skin.edge_face_count(a, b) == 1 {
                faces.push([b + k, a + k, a]);
                faces.push([b + k, a, b]);
                groups.extend([2, 2]);
            }
        }
    }
    let mut out = MeshBuilder::new(p, faces).with_faces(Some(groups), None).finish();
    out.export_center = m.export_center;
    out.export_scale = m.export_scale;
    Ok(out)
}

// ------------------------------------------------------- sculptor tools

impl Sculptor {
    /// Replaces the mask (one undo step). False when nothing changed.
    pub(crate) fn set_mask(&mut self, label: &str, mask: Vec<f32>) -> bool {
        self.end_stroke();
        if mask == self.mesh.mask {
            return false;
        }
        let n = self.mesh.vertex_count();
        let old = std::mem::replace(&mut self.mesh.mask, mask);
        self.set_label(label);
        self.push_undo(Undo::Verts { idx: (0..n as u32).collect(), pos: self.mesh.positions.clone(), mask: old });
        self.dirty.all(n);
        true
    }

    /// Sets the mask of the selected (shown) vertices to `value`.
    pub fn mask_selected(&mut self, label: &str, sel: &[bool], value: f32) -> bool {
        let mut m = self.mesh.mask.clone();
        for (v, x) in m.iter_mut().enumerate() {
            if sel.get(v).copied().unwrap_or(false) && !self.mesh.vert_hidden(v as u32) {
                *x = value;
            }
        }
        self.set_mask(label, m)
    }

    /// One step of a neighbourhood filter over the shown vertices.
    fn mask_filter(&self, f: impl Fn(f32, &mut dyn Iterator<Item = f32>) -> f32) -> Vec<f32> {
        let m = &self.mesh;
        (0..m.vertex_count() as u32)
            .map(|v| {
                let cur = m.mask[v as usize];
                if m.vert_hidden(v) || m.neighbors(v).is_empty() {
                    return cur;
                }
                let mut it = m.neighbors(v).iter().map(|&u| m.mask[u as usize]);
                f(cur, &mut it)
            })
            .collect()
    }

    pub fn grow_mask(&mut self) -> bool {
        let m = self.mask_filter(|c, it| it.fold(c, f32::max));
        self.set_mask("Grow mask", m)
    }
    pub fn shrink_mask(&mut self) -> bool {
        let m = self.mask_filter(|c, it| it.fold(c, f32::min));
        self.set_mask("Shrink mask", m)
    }
    pub fn blur_mask(&mut self) -> bool {
        let m = self.mask_filter(|c, it| {
            let (s, n) = it.fold((0.0, 0), |(s, n), x| (s + x, n + 1));
            c * 0.4 + 0.6 * s / n.max(1) as f32
        });
        self.set_mask("Blur mask", m)
    }
    /// Pushes the mask's soft edge back towards fully on or off.
    pub fn sharpen_mask(&mut self) -> bool {
        let m = self.mesh.iter_sharpen();
        self.set_mask("Sharpen mask", m)
    }

    /// Masks the creases and hollows (`peaks` = the ridges and bumps
    /// instead), strongest where the surface bends most.
    pub fn mask_by_curvature(&mut self, peaks: bool, strength: f32) -> bool {
        let m = &self.mesh;
        let n = m.vertex_count();
        let mut k = vec![0.0f32; n];
        for v in 0..n {
            let nb = m.neighbors(v as u32);
            if nb.is_empty() {
                continue;
            }
            let p = m.positions[v];
            let avg = nb.iter().map(|&u| m.positions[u as usize]).sum::<Vec3>() / nb.len() as f32;
            let len = nb.iter().map(|&u| m.positions[u as usize].distance(p)).sum::<f32>() / nb.len() as f32;
            // positive in a hollow (the neighbours sit above the point)
            k[v] = m.normals[v].dot(avg - p) / len.max(1e-9);
        }
        // a light blur so single noisy points don't speckle the mask
        let k: Vec<f32> = (0..n)
            .map(|v| {
                let nb = m.neighbors(v as u32);
                if nb.is_empty() {
                    k[v]
                } else {
                    0.5 * k[v] + 0.5 * nb.iter().map(|&u| k[u as usize]).sum::<f32>() / nb.len() as f32
                }
            })
            .collect();
        let signed: Vec<f32> = k.iter().map(|&x| if peaks { -x } else { x }).collect();
        let mut pos: Vec<f32> = signed.iter().copied().filter(|&x| x > 0.0).collect();
        if pos.is_empty() {
            return false;
        }
        pos.sort_by(f32::total_cmp);
        let top = pos[(pos.len() * 9 / 10).min(pos.len() - 1)].max(1e-6);
        let gain = strength.clamp(0.05, 1.0);
        let mut mask = self.mesh.mask.clone();
        for v in 0..n {
            if !self.mesh.vert_hidden(v as u32) {
                mask[v] = (signed[v] / (top * (1.05 - gain))).clamp(0.0, 1.0);
            }
        }
        self.set_mask(if peaks { "Mask peaks" } else { "Mask cavity" }, mask)
    }

    /// Replaces which faces are hidden (one undo step). False when nothing
    /// changed or nothing would be left showing.
    pub fn set_hidden(&mut self, label: &str, mut hidden: Vec<bool>) -> Result<bool, String> {
        self.end_stroke();
        let m = &self.mesh;
        if hidden.len() != m.tris.len() || !hidden.contains(&true) {
            hidden.clear();
        } else if m.tris.iter().zip(&hidden).all(|(t, &h)| h || t[0] == NONE) {
            return Err("That would hide everything".into());
        }
        if hidden == self.mesh.hidden {
            return Ok(false);
        }
        let old = std::mem::replace(&mut self.mesh.hidden, hidden);
        self.set_label(label);
        self.push_undo(Undo::Hidden(old));
        self.dirty.topology = true;
        Ok(true)
    }

    fn hidden_vec(&self) -> Vec<bool> {
        if self.mesh.hidden.len() == self.mesh.tris.len() { self.mesh.hidden.clone() } else { vec![false; self.mesh.tris.len()] }
    }

    pub fn show_all(&mut self) -> bool {
        self.set_hidden("Show all", Vec::new()).unwrap_or(false)
    }

    pub fn invert_visibility(&mut self) -> Result<bool, String> {
        if !self.mesh.any_hidden() {
            return Err("Nothing is hidden".into());
        }
        let h = self.mesh.hidden.iter().map(|&h| !h).collect();
        self.set_hidden("Invert visibility", h)
    }

    /// Hides the selected faces, or (`keep`) everything but them.
    pub fn hide_faces(&mut self, sel: &[bool], keep: bool) -> Result<bool, String> {
        let mut h = self.hidden_vec();
        for (f, x) in h.iter_mut().enumerate() {
            let s = sel.get(f).copied().unwrap_or(false);
            if s != keep {
                *x = true;
            }
        }
        self.set_hidden(if keep { "Show only" } else { "Hide" }, h)
    }

    pub fn hide_group(&mut self, g: u32, keep: bool) -> Result<bool, String> {
        let sel: Vec<bool> = self.mesh.groups.iter().map(|&x| x == g).collect();
        self.hide_faces(&sel, keep)
    }

    /// Hides faces whose corners are all at least half masked (`masked`),
    /// or the unmasked ones.
    pub fn hide_by_mask(&mut self, masked: bool) -> Result<bool, String> {
        let m = &self.mesh;
        let sel: Vec<bool> = m.tris.iter().map(|t| t[0] != NONE && t.iter().all(|&v| (m.mask[v as usize] >= 0.5) == masked)).collect();
        self.hide_faces(&sel, false)
    }

    /// Removes the hidden faces for good (one undo step).
    pub fn delete_hidden(&mut self) -> Result<usize, String> {
        if !self.mesh.any_hidden() {
            return Err("Nothing is hidden".into());
        }
        if self.level_count() > 1 {
            return Err("Delete the other subdivision levels first (it changes the topology)".into());
        }
        let mut m = self.mesh.clone();
        let mut n = 0;
        for f in 0..m.tris.len() {
            if m.hidden[f] && m.tris[f][0] != NONE {
                m.kill_face(f as u32);
                n += 1;
            }
        }
        m.hidden.clear();
        m.compact();
        m.compute_all_normals();
        self.set_label("Delete hidden");
        self.set_mesh_built(m, None);
        Ok(n)
    }

    /// Clip: flattens what lies past each plane onto it (masked points
    /// stay, partly masked ones move part way).
    pub fn clip(&mut self, planes: &[Plane]) -> usize {
        self.end_stroke();
        let old = self.mesh.positions.clone();
        let mut moved = 0;
        for v in 0..self.mesh.vertex_count() {
            if self.mesh.vert_faces(v as u32).is_empty() || self.mesh.vert_hidden(v as u32) {
                continue;
            }
            let w = 1.0 - self.mesh.mask[v];
            let mut p = self.mesh.positions[v];
            for pl in planes {
                let e = pl.eval(p);
                if e > 0.0 {
                    p -= pl.n * e * w;
                }
            }
            if p != self.mesh.positions[v] {
                self.mesh.positions[v] = p;
                moved += 1;
            }
        }
        if moved == 0 {
            return 0;
        }
        let n = old.len();
        self.set_label("Clip");
        self.push_undo(Undo::Verts { idx: (0..n as u32).collect(), pos: old, mask: self.mesh.mask.clone() });
        self.mesh.compute_all_normals();
        self.mark_all_moved();
        moved
    }

    /// Trim: cuts away what lies past the planes and caps the cut flat.
    pub fn trim(&mut self, planes: &[Plane]) -> Result<(), String> {
        self.no_levels("trim")?;
        let mut m = self.mesh.clone();
        let mut any = false;
        for &pl in planes {
            match trim_by_plane(&m, pl) {
                Ok(x) => {
                    m = x;
                    any = true;
                }
                Err(e) if planes.len() == 1 => return Err(e),
                Err(_) => {}
            }
        }
        if !any {
            return Err("The line doesn't cross the model".into());
        }
        self.set_label("Trim");
        self.set_mesh_built(m, None);
        Ok(())
    }

    /// Slice: splits the faces along the planes; the cut side gets new
    /// polygroups.
    pub fn slice(&mut self, planes: &[Plane]) -> Result<(), String> {
        self.no_levels("slice")?;
        let mut m = self.mesh.clone();
        let mut any = false;
        for &pl in planes {
            let (mut x, side) = split_by_plane(&m, pl);
            if !side.contains(&true) || !side.contains(&false) {
                continue;
            }
            let base = x.groups.iter().copied().max().map_or(0, |g| g + 1);
            let mut map: HashMap<u32, u32> = HashMap::new();
            for (f, &s) in side.iter().enumerate() {
                if s {
                    let len = map.len() as u32;
                    let g = *map.entry(x.groups[f]).or_insert(base + len);
                    x.groups[f] = g;
                }
            }
            m = x;
            any = true;
        }
        if !any {
            return Err("The line doesn't cross the model".into());
        }
        self.set_label("Slice");
        self.set_mesh_built(m, None);
        Ok(())
    }

    fn no_levels(&self, what: &str) -> Result<(), String> {
        if self.level_count() > 1 {
            Err(format!("Delete the other subdivision levels to {what} (it changes the topology)"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn export_hidden(&self, out: &mut Vec<u8>) {
        if !self.mesh.any_hidden() {
            return;
        }
        let mut b = Vec::new();
        crate::detail::put_u32(&mut b, self.level as u32);
        crate::detail::put_u32(&mut b, self.mesh.hidden.len() as u32);
        let mut bits = vec![0u8; self.mesh.hidden.len().div_ceil(8)];
        for (i, &h) in self.mesh.hidden.iter().enumerate() {
            if h {
                bits[i / 8] |= 1 << (i % 8);
            }
        }
        b.extend_from_slice(&bits);
        crate::detail::put_chunk(out, b"HIDE", &b);
    }

    pub(crate) fn import_hidden(&mut self, body: &[u8]) {
        let mut r = crate::detail::Reader { b: body, at: 0 };
        let (Some(k), Some(n)) = (r.u32(), r.u32()) else { return };
        let Some(bits) = r.take((n as usize).div_ceil(8)) else { return };
        if k as usize != self.level || n as usize != self.mesh.tris.len() {
            return;
        }
        self.mesh.hidden = (0..n as usize).map(|i| bits[i / 8] & (1 << (i % 8)) != 0).collect();
        self.dirty.topology = true;
    }
}

trait Sharpen {
    fn iter_sharpen(&self) -> Vec<f32>;
}
impl Sharpen for Mesh {
    fn iter_sharpen(&self) -> Vec<f32> {
        (0..self.vertex_count())
            .map(|v| {
                let x = self.mask[v];
                if self.vert_hidden(v as u32) { x } else { (0.5 + (x - 0.5) * 2.0).clamp(0.0, 1.0) }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(m: &Mesh) -> f32 {
        m.tris
            .iter()
            .filter(|t| t[0] != NONE)
            .map(|t| {
                let [a, b, c] = t.map(|v| m.positions[v as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn closed(m: &Mesh) -> bool {
        m.tris.iter().filter(|t| t[0] != NONE).all(|t| (0..3).all(|k| m.edge_face_count(t[k], t[(k + 1) % 3]) == 2))
    }

    #[test]
    fn trim_cuts_flat_and_stays_closed() {
        let m = Mesh::icosphere(4);
        let full = volume(&m);
        let out = trim_by_plane(&m, Plane::through(Vec3::new(0.3, 0.0, 0.0), Vec3::X)).unwrap();
        assert!(closed(&out), "the cap closes the cut");
        assert!(out.positions.iter().all(|p| p.x < 0.3 + 1e-4));
        let v = volume(&out);
        // spherical cap beyond x = 0.3 holds pi h^2 (3 - h) / 3 of a unit ball
        let h = 0.7f32;
        let expect = full - std::f32::consts::PI * h * h * (3.0 - h) / 3.0;
        assert!((v - expect).abs() < 0.03, "volume {v} vs {expect}");
        assert!(out.positions.iter().filter(|p| (p.x - 0.3).abs() < 1e-4).count() > 20);
        // the flat cap faces +X
        let g = out.groups.iter().copied().max().unwrap();
        assert!(out.tris.iter().enumerate().filter(|(f, _)| out.groups[*f] == g).all(|(f, _)| out.face_normal_raw(f).normalize().x > 0.99));
    }

    #[test]
    fn trim_through_a_hollow_shell_caps_a_ring() {
        let a = Mesh::icosphere(3);
        let mut pos = a.positions.clone();
        let k = pos.len() as u32;
        pos.extend(a.positions.iter().map(|p| *p * 0.5));
        let mut tris = a.tris.clone();
        tris.extend(a.tris.iter().map(|t| [t[0] + k, t[2] + k, t[1] + k]));
        let shell = Mesh::new(pos, tris);
        let out = trim_by_plane(&shell, Plane::through(Vec3::ZERO, Vec3::Y)).unwrap();
        assert!(closed(&out));
        let v = volume(&out);
        let expect = volume(&shell) * 0.5;
        assert!((v - expect).abs() < expect * 0.03, "{v} vs {expect}");
    }

    #[test]
    fn slice_splits_into_new_groups_without_moving_anything() {
        let m = Mesh::icosphere(3);
        let (out, side) = split_by_plane(&m, Plane::through(Vec3::new(0.0, 0.2, 0.0), Vec3::Y));
        assert!(closed(&out));
        assert!(side.contains(&true) && side.contains(&false));
        assert!((volume(&out) - volume(&m)).abs() < 1e-4);
        for (f, t) in out.tris.iter().enumerate() {
            let c = t.iter().map(|&v| out.positions[v as usize].y).sum::<f32>() / 3.0;
            assert_eq!(side[f], c > 0.2, "face {f} on the wrong side");
        }
        let mut s = Sculptor::new(m);
        s.slice(&[Plane::through(Vec3::new(0.0, 0.2, 0.0), Vec3::Y)]).unwrap();
        let groups: std::collections::HashSet<u32> = s.mesh.groups.iter().copied().collect();
        assert_eq!(groups.len(), 2);
        s.undo();
        assert_eq!(s.mesh.groups.iter().copied().collect::<std::collections::HashSet<u32>>().len(), 1);
    }

    #[test]
    fn clip_flattens_past_the_plane_except_masked() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = s.mesh.positions.clone();
        let keep = (0..before.len()).find(|&v| before[v].x > 0.8).unwrap();
        s.mesh.mask[keep] = 1.0;
        assert!(s.clip(&[Plane::through(Vec3::new(0.5, 0.0, 0.0), Vec3::X)]) > 10);
        for (v, p) in s.mesh.positions.iter().enumerate() {
            if v != keep {
                assert!(p.x <= 0.5 + 1e-5);
            }
        }
        assert_eq!(s.mesh.positions[keep], before[keep], "masked points stay");
        s.undo();
        assert_eq!(s.mesh.positions, before);
    }

    #[test]
    fn mask_grow_shrink_blur_sharpen() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let v0 = 0usize;
        s.mesh.mask[v0] = 1.0;
        assert!(s.grow_mask());
        let ring = s.mesh.neighbors(0).len();
        assert_eq!(s.mesh.mask.iter().filter(|&&m| m == 1.0).count(), ring + 1);
        assert!(s.shrink_mask());
        assert_eq!(s.mesh.mask.iter().filter(|&&m| m == 1.0).count(), 1);
        s.grow_mask();
        s.grow_mask();
        assert!(s.blur_mask());
        let soft = s.mesh.mask.iter().filter(|&&m| m > 0.02 && m < 0.98).count();
        assert!(soft > 0, "blurring softens the edge");
        s.sharpen_mask();
        s.sharpen_mask();
        s.sharpen_mask();
        let soft2 = s.mesh.mask.iter().filter(|&&m| m > 0.02 && m < 0.98).count();
        assert!(soft2 < soft, "sharpening hardens it ({soft2} vs {soft})");
        // each was one undo step
        for _ in 0..7 {
            s.undo();
        }
        assert!(s.mesh.mask[1..].iter().all(|&m| m == 0.0) && s.mesh.mask[0] == 1.0);
    }

    #[test]
    fn cavity_mask_finds_the_groove() {
        // a sphere with a groove pressed round its equator
        let mut m = Mesh::icosphere(5);
        for p in &mut m.positions {
            if p.y.abs() < 0.08 {
                *p *= 0.93;
            }
        }
        m.compute_all_normals();
        let mut s = Sculptor::new(m);
        assert!(s.mask_by_curvature(false, 0.6));
        let m = &s.mesh;
        let avg = |f: &dyn Fn(f32) -> bool| {
            let v: Vec<f32> = (0..m.vertex_count()).filter(|&v| f(m.positions[v].y)).map(|v| m.mask[v]).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        let groove = avg(&|y| y.abs() < 0.12 && y.abs() > 0.06);
        let pole = avg(&|y| y.abs() > 0.5);
        assert!(groove > 0.2 && pole < 0.05, "groove {groove}, elsewhere {pole}");
        assert!(s.mask_by_curvature(true, 0.6));
        let rim = {
            let v: Vec<f32> = (0..s.mesh.vertex_count()).filter(|&v| (s.mesh.positions[v].y.abs() - 0.08).abs() < 0.03).map(|v| s.mesh.mask[v]).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        assert!(rim > 0.2, "the groove's rims are peaks ({rim})");
    }

    #[test]
    fn hidden_faces_are_skipped_and_carried() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        // hide the front half (+Z)
        let sel: Vec<bool> = (0..s.mesh.tris.len()).map(|f| s.mesh.face_normal_raw(f).z > 0.0).collect();
        assert!(s.hide_faces(&sel, false).unwrap());
        s.refresh_bvh();
        let h = s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        assert!(h.point.z < 0.0, "the ray goes through the hidden front to the back");
        assert!(s.hide_faces(&vec![true; s.mesh.tris.len()], true).is_ok());
        assert!(s.set_hidden("x", vec![true; s.mesh.tris.len()]).is_err(), "can't hide everything");
        // a brush on the back leaves hidden points alone
        let before = s.mesh.positions.clone();
        s.begin_stroke(false);
        s.dab(&crate::sculpt::DabParams { tool: crate::sculpt::Tool::Inflate, center: Vec3::new(0.0, 0.0, 0.0), radius: 3.0, intensity: 1.0, seed_tri: h.tri, ..Default::default() });
        s.end_stroke();
        for v in 0..before.len() {
            if s.mesh.vert_hidden(v as u32) {
                assert_eq!(s.mesh.positions[v], before[v]);
            }
        }
        assert!(s.mesh.positions != before);
        // subdivision and level changes keep the hidden part hidden
        let hidden_before = s.mesh.hidden.iter().filter(|&&h| h).count();
        s.subdivide().unwrap();
        assert_eq!(s.mesh.hidden.iter().filter(|&&h| h).count(), hidden_before * 4);
        s.set_level(0);
        assert_eq!(s.mesh.hidden.iter().filter(|&&h| h).count(), hidden_before);
        s.set_level(1);
        assert_eq!(s.mesh.hidden.iter().filter(|&&h| h).count(), hidden_before * 4);
        // undo back to before the hide shows everything
        while s.can_undo() {
            s.undo();
        }
        assert!(!s.mesh.any_hidden());
    }

    #[test]
    fn delete_hidden_and_invert() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let n = s.mesh.tris.len();
        let sel: Vec<bool> = (0..n).map(|f| s.mesh.face_normal_raw(f).normalize().y > 0.3).collect();
        let k = sel.iter().filter(|&&x| x).count();
        s.hide_faces(&sel, false).unwrap();
        s.invert_visibility().unwrap();
        assert_eq!(s.mesh.hidden.iter().filter(|&&h| h).count(), n - k);
        assert_eq!(s.delete_hidden().unwrap(), n - k);
        assert_eq!(s.mesh.live_tri_count(), k);
        assert!(!s.mesh.any_hidden());
        s.undo();
        assert_eq!(s.mesh.live_tri_count(), n);
        assert!(s.mesh.any_hidden());
    }

    #[test]
    fn hidden_faces_survive_saving() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let sel: Vec<bool> = (0..s.mesh.tris.len()).map(|f| f % 3 == 0).collect();
        s.hide_faces(&sel, false).unwrap();
        let mut blob = Vec::new();
        s.export_hidden(&mut blob);
        let want = s.mesh.hidden.clone();
        let mut t = Sculptor::new(Mesh::icosphere(3));
        // the chunk is tag + 8-byte length + body
        t.import_hidden(&blob[12..]);
        assert_eq!(t.mesh.hidden, want);
    }

    #[test]
    fn extract_makes_a_closed_shell() {
        let mut m = Mesh::icosphere(4);
        for v in 0..m.vertex_count() {
            if m.positions[v].y > 0.5 {
                m.mask[v] = 1.0;
            }
        }
        let shell = extract_shell(&m, 0.05, 2).unwrap();
        assert!(closed(&shell));
        let v = volume(&shell);
        // a cap of the unit sphere above y = 0.5 has area 2 pi (1 - 0.5) = pi
        let expect = std::f32::consts::PI * 0.05;
        assert!(v > 0.0 && (v - expect).abs() < expect * 0.25, "volume {v} vs {expect}");
        assert!(extract_shell(&Mesh::icosphere(2), 0.05, 0).is_err());
    }
}
