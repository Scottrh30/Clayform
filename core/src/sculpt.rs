//! Sculpting engine: brushes, strokes, symmetry helpers and undo/redo.

use crate::bvh::{Bvh, Hit};
use crate::mesh::{Marker, Mesh};
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Draw,
    Clay,
    Smooth,
    Flatten,
    Inflate,
    Pinch,
    Crease,
    Grab,
    Mask,
}

impl Tool {
    pub const ALL: [Tool; 9] = [
        Tool::Draw,
        Tool::Clay,
        Tool::Smooth,
        Tool::Flatten,
        Tool::Inflate,
        Tool::Pinch,
        Tool::Crease,
        Tool::Grab,
        Tool::Mask,
    ];

    pub fn index(self) -> usize {
        Tool::ALL.iter().position(|t| *t == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Draw => "Draw",
            Tool::Clay => "Clay",
            Tool::Smooth => "Smooth",
            Tool::Flatten => "Flatten",
            Tool::Inflate => "Inflate",
            Tool::Pinch => "Pinch",
            Tool::Crease => "Crease",
            Tool::Grab => "Grab",
            Tool::Mask => "Mask",
        }
    }

    pub fn shortcut(self) -> char {
        match self {
            Tool::Draw => 'D',
            Tool::Clay => 'C',
            Tool::Smooth => 'S',
            Tool::Flatten => 'F',
            Tool::Inflate => 'I',
            Tool::Pinch => 'P',
            Tool::Crease => 'E',
            Tool::Grab => 'G',
            Tool::Mask => 'M',
        }
    }

    pub fn default_intensity(self) -> f32 {
        match self {
            Tool::Smooth => 0.6,
            Tool::Grab => 1.0,
            Tool::Mask => 0.8,
            Tool::Flatten => 0.5,
            _ => 0.4,
        }
    }

    /// Whether Ctrl / the negative toggle has a meaning for this tool.
    pub fn has_negative(self) -> bool {
        !matches!(self, Tool::Smooth | Tool::Flatten | Tool::Grab)
    }
}

pub struct DabParams {
    pub tool: Tool,
    pub center: Vec3,
    pub radius: f32,
    pub intensity: f32,
    pub negative: bool,
    pub seed_tri: u32,
}

/// A state to restore; applying one returns its inverse.
enum Undo {
    Verts { idx: Vec<u32>, pos: Vec<Vec3>, mask: Vec<f32> },
    Mesh(Box<Mesh>),
}

impl Undo {
    fn bytes(&self) -> usize {
        match self {
            Undo::Verts { idx, .. } => idx.len() * 20,
            Undo::Mesh(m) => m.positions.len() * 40 + m.tris.len() * 36,
        }
    }
}

#[derive(Default)]
pub struct GpuDirty {
    pub topology: bool,
    pub range: Option<(u32, u32)>,
}

impl GpuDirty {
    fn add(&mut self, v: u32) {
        self.range = Some(match self.range {
            None => (v, v),
            Some((a, b)) => (a.min(v), b.max(v)),
        });
    }
    fn all(&mut self, n: usize) {
        if n > 0 {
            self.range = Some((0, n as u32 - 1));
        }
    }
}

struct GrabSet {
    verts: Vec<(u32, f32)>,
    orig: Vec<Vec3>,
    mirror: bool,
}

struct Stroke {
    mark: Marker,
    touched: Vec<u32>,
    old_pos: Vec<Vec3>,
    old_mask: Vec<f32>,
    grab: Vec<GrabSet>,
}

impl Stroke {
    #[inline]
    fn touch(&mut self, v: u32, mesh: &Mesh) {
        if self.mark.mark(v) {
            self.touched.push(v);
            self.old_pos.push(mesh.positions[v as usize]);
            self.old_mask.push(mesh.mask[v as usize]);
        }
    }
}

const UNDO_BUDGET_BYTES: usize = 768 * 1024 * 1024;
pub const MAX_TRIANGLES: usize = 6_000_000;

pub struct Sculptor {
    pub mesh: Mesh,
    bvh: Bvh,
    bvh_dirty: bool,
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    vmark: Marker,
    vmark2: Marker,
    fmark: Marker,
    stroke: Option<Stroke>,
    region: Vec<(u32, f32)>,
    stack: Vec<u32>,
    scratch: Vec<Vec3>,
    pub dirty: GpuDirty,
}

#[inline]
fn falloff(x: f32) -> f32 {
    if x >= 1.0 {
        0.0
    } else {
        let x3 = x * x * x;
        3.0 * x3 * x - 4.0 * x3 + 1.0
    }
}

pub fn mirror_x(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

impl Sculptor {
    pub fn new(mesh: Mesh) -> Self {
        let bvh = Bvh::build(&mesh);
        Self {
            mesh,
            bvh,
            bvh_dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            vmark: Marker::new(),
            vmark2: Marker::new(),
            fmark: Marker::new(),
            stroke: None,
            region: Vec::new(),
            stack: Vec::new(),
            scratch: Vec::new(),
            dirty: GpuDirty { topology: true, range: None },
        }
    }

    pub fn tri_count(&self) -> usize {
        self.mesh.tris.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() && self.stroke.is_none()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.stroke.is_none()
    }

    fn push_undo(&mut self, u: Undo) {
        self.undo.push(u);
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(|u| u.bytes()).sum();
        while self.undo.len() > 1 && (total > UNDO_BUDGET_BYTES || self.undo.len() > 100) {
            total -= self.undo.remove(0).bytes();
        }
    }

    /// Replaces the mesh (new primitive, import, subdivision), undoable.
    pub fn set_mesh(&mut self, mesh: Mesh) {
        self.end_stroke();
        let old = std::mem::replace(&mut self.mesh, mesh);
        self.push_undo(Undo::Mesh(Box::new(old)));
        self.topology_changed();
    }

    fn topology_changed(&mut self) {
        self.bvh = Bvh::build(&self.mesh);
        self.bvh_dirty = false;
        self.dirty.topology = true;
        self.dirty.range = None;
    }

    pub fn subdivide(&mut self) -> Result<(), String> {
        if self.mesh.tris.len() * 4 > MAX_TRIANGLES {
            return Err(format!(
                "Subdividing would exceed {} M triangles",
                MAX_TRIANGLES / 1_000_000
            ));
        }
        let m = self.mesh.subdivide(true);
        self.set_mesh(m);
        Ok(())
    }

    /// Refit the picking hierarchy if the surface moved. Call once per frame.
    pub fn refresh_bvh(&mut self) {
        if self.bvh_dirty {
            self.bvh.refit(&self.mesh);
            self.bvh_dirty = false;
        }
    }

    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.bvh.raycast(&self.mesh, origin, dir)
    }

    pub fn begin_stroke(&mut self) {
        self.end_stroke();
        let mut mark = Marker::new();
        mark.begin(self.mesh.vertex_count());
        self.stroke = Some(Stroke {
            mark,
            touched: Vec::new(),
            old_pos: Vec::new(),
            old_mask: Vec::new(),
            grab: Vec::new(),
        });
    }

    pub fn end_stroke(&mut self) {
        if let Some(st) = self.stroke.take() {
            if !st.touched.is_empty() {
                self.push_undo(Undo::Verts { idx: st.touched, pos: st.old_pos, mask: st.old_mask });
            }
        }
    }

    /// Flood-fills outward from the hit triangle, collecting vertices inside
    /// the brush sphere together with their falloff weight.
    fn collect_region(&mut self, center: Vec3, radius: f32, seed_tri: u32, use_mask: bool) {
        let Sculptor { mesh, vmark, region, stack, .. } = self;
        region.clear();
        stack.clear();
        if seed_tri as usize >= mesh.tris.len() || radius <= 0.0 {
            return;
        }
        vmark.begin(mesh.vertex_count());
        let seed = mesh.tris[seed_tri as usize];
        for &v in &seed {
            if vmark.mark(v) {
                stack.push(v);
            }
        }
        let r2 = radius * radius;
        let inv_r = 1.0 / radius;
        while let Some(v) = stack.pop() {
            let d2 = mesh.positions[v as usize].distance_squared(center);
            let inside = d2 < r2;
            if inside {
                let mut w = falloff(d2.sqrt() * inv_r);
                if use_mask {
                    w *= 1.0 - mesh.mask[v as usize];
                }
                region.push((v, w));
            }
            if inside || seed.contains(&v) {
                for &u in mesh.neighbors(v) {
                    if vmark.mark(u) {
                        stack.push(u);
                    }
                }
            }
        }
    }

    fn after_deform(&mut self, verts_from_region: bool, extra: Option<&[u32]>) {
        let mut list: Vec<u32> = Vec::new();
        let verts: &[u32] = if verts_from_region {
            list.extend(self.region.iter().map(|r| r.0));
            &list
        } else {
            extra.unwrap_or(&[])
        };
        for &v in verts {
            self.dirty.add(v);
        }
        let Sculptor { mesh, fmark, vmark2, .. } = self;
        mesh.update_normals(verts, fmark, vmark2);
        // normals of the one-ring changed too
        for &f in verts.iter().flat_map(|&v| mesh.vert_faces(v)) {
            for &u in &mesh.tris[f as usize] {
                self.dirty.add(u);
            }
        }
        self.bvh_dirty = true;
    }

    pub fn dab(&mut self, p: &DabParams) {
        if p.tool == Tool::Grab {
            return;
        }
        self.collect_region(p.center, p.radius, p.seed_tri, p.tool != Tool::Mask);
        if self.region.is_empty() {
            return;
        }
        if let Some(st) = self.stroke.as_mut() {
            for &(v, _) in &self.region {
                st.touch(v, &self.mesh);
            }
        }

        let s = if p.negative && p.tool.has_negative() { -1.0 } else { 1.0 };
        let r = p.radius;
        let i = p.intensity.clamp(0.0, 1.0);

        // weighted area normal & centre
        let mut an = Vec3::ZERO;
        let mut ac = Vec3::ZERO;
        let mut wsum = 0.0;
        for &(v, w) in &self.region {
            let w = w + 1e-4;
            an += self.mesh.normals[v as usize] * w;
            ac += self.mesh.positions[v as usize] * w;
            wsum += w;
        }
        let an = an.try_normalize().unwrap_or(Vec3::Y);
        let ac = ac / wsum;

        let mesh = &mut self.mesh;
        let region = &self.region;
        match p.tool {
            Tool::Draw => {
                let d = an * (s * 0.08 * r * i);
                for &(v, w) in region {
                    mesh.positions[v as usize] += d * w;
                }
            }
            Tool::Inflate => {
                let k = s * 0.06 * r * i;
                for &(v, w) in region {
                    let n = mesh.normals[v as usize];
                    mesh.positions[v as usize] += n * (k * w);
                }
            }
            Tool::Smooth => {
                self.scratch.clear();
                for &(v, w) in region {
                    let p0 = mesh.positions[v as usize];
                    let mut sum = Vec3::ZERO;
                    let mut c = 0.0;
                    let boundary = mesh.boundary[v as usize];
                    for &u in mesh.neighbors(v) {
                        if boundary && !mesh.boundary[u as usize] {
                            continue;
                        }
                        sum += mesh.positions[u as usize];
                        c += 1.0;
                    }
                    let target = if c > 0.0 { sum / c } else { p0 };
                    self.scratch.push(p0.lerp(target, (w * i).clamp(0.0, 1.0)));
                }
                for (k, &(v, _)) in region.iter().enumerate() {
                    mesh.positions[v as usize] = self.scratch[k];
                }
            }
            Tool::Flatten => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    mesh.positions[v as usize] = pv - an * (d * w * i * 0.5);
                }
            }
            Tool::Clay => {
                let plane = ac + an * (s * r * 0.1);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - plane).dot(an);
                    if d * s < 0.0 {
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.45);
                    }
                }
            }
            Tool::Pinch => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (s * w * i * 0.3);
                }
            }
            Tool::Crease => {
                let push = an * (-s * 0.05 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (w * i * 0.25) + push * w;
                }
            }
            Tool::Mask => {
                for &(v, w) in region {
                    let m = &mut mesh.mask[v as usize];
                    *m = (*m + s * i * w * 0.5).clamp(0.0, 1.0);
                }
                for &(v, _) in region {
                    self.dirty.add(v);
                }
                return;
            }
            Tool::Grab => unreachable!(),
        }
        self.after_deform(true, None);
    }

    /// Starts a grab region (call once per symmetry side at stroke start).
    pub fn grab_begin(&mut self, center: Vec3, radius: f32, seed_tri: u32, mirror: bool) {
        self.collect_region(center, radius, seed_tri, true);
        let verts = self.region.clone();
        let orig = verts.iter().map(|&(v, _)| self.mesh.positions[v as usize]).collect();
        if let Some(st) = self.stroke.as_mut() {
            st.grab.push(GrabSet { verts, orig, mirror });
        }
    }

    /// Moves grabbed vertices by `delta` (world units) from where they started.
    pub fn grab_update(&mut self, delta: Vec3) {
        let Some(mut st) = self.stroke.take() else { return };
        let mut moved: Vec<u32> = Vec::new();
        for set in &st.grab {
            let d = if set.mirror { mirror_x(delta) } else { delta };
            for (k, &(v, w)) in set.verts.iter().enumerate() {
                if st.mark.mark(v) {
                    st.touched.push(v);
                    st.old_pos.push(self.mesh.positions[v as usize]);
                    st.old_mask.push(self.mesh.mask[v as usize]);
                }
                self.mesh.positions[v as usize] = set.orig[k] + d * w;
                moved.push(v);
            }
        }
        self.stroke = Some(st);
        self.after_deform(false, Some(&moved));
    }

    fn apply(&mut self, e: Undo) -> Undo {
        match e {
            Undo::Verts { idx, pos, mask } => {
                let mut cur_pos = Vec::with_capacity(idx.len());
                let mut cur_mask = Vec::with_capacity(idx.len());
                for (k, &v) in idx.iter().enumerate() {
                    cur_pos.push(self.mesh.positions[v as usize]);
                    cur_mask.push(self.mesh.mask[v as usize]);
                    self.mesh.positions[v as usize] = pos[k];
                    self.mesh.mask[v as usize] = mask[k];
                }
                self.after_deform(false, Some(&idx));
                Undo::Verts { idx, pos: cur_pos, mask: cur_mask }
            }
            Undo::Mesh(m) => {
                let old = std::mem::replace(&mut self.mesh, *m);
                self.topology_changed();
                Undo::Mesh(Box::new(old))
            }
        }
    }

    pub fn undo(&mut self) {
        if !self.can_undo() {
            return;
        }
        let e = self.undo.pop().unwrap();
        let inv = self.apply(e);
        self.redo.push(inv);
    }

    pub fn redo(&mut self) {
        if !self.can_redo() {
            return;
        }
        let e = self.redo.pop().unwrap();
        let inv = self.apply(e);
        self.undo.push(inv);
    }

    fn mask_op(&mut self, f: impl Fn(f32) -> f32) {
        self.end_stroke();
        let n = self.mesh.vertex_count();
        let idx: Vec<u32> = (0..n as u32).collect();
        let old = self.mesh.mask.clone();
        for m in &mut self.mesh.mask {
            *m = f(*m);
        }
        self.push_undo(Undo::Verts { idx, pos: self.mesh.positions.clone(), mask: old });
        self.dirty.all(n);
    }

    pub fn clear_mask(&mut self) {
        self.mask_op(|_| 0.0);
    }
    pub fn invert_mask(&mut self) {
        self.mask_op(|m| 1.0 - m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_hit(s: &Sculptor) -> Hit {
        s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap()
    }

    fn dab(s: &mut Sculptor, tool: Tool, negative: bool) {
        let h = front_hit(s);
        s.dab(&DabParams { tool, center: h.point, radius: 0.4, intensity: 1.0, negative, seed_tri: h.tri });
        s.refresh_bvh();
    }

    #[test]
    fn draw_raises_and_undo_restores() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke();
        for _ in 0..5 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let after = front_hit(&s).point.z;
        assert!(after > before + 0.05, "{before} -> {after}");
        s.undo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - before).abs() < 1e-5);
        s.redo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - after).abs() < 1e-5);
    }

    #[test]
    fn negative_draw_digs() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke();
        dab(&mut s, Tool::Draw, true);
        s.end_stroke();
        assert!(front_hit(&s).point.z < before);
    }

    #[test]
    fn mask_blocks_draw() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        s.begin_stroke();
        for _ in 0..10 {
            dab(&mut s, Tool::Mask, false);
        }
        s.end_stroke();
        let before = front_hit(&s).point.z;
        s.begin_stroke();
        dab(&mut s, Tool::Draw, false);
        s.end_stroke();
        assert!((front_hit(&s).point.z - before).abs() < 0.01);
    }

    #[test]
    fn smooth_and_others_stay_finite() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        for t in Tool::ALL {
            s.begin_stroke();
            dab(&mut s, t, false);
            dab(&mut s, t, true);
            s.end_stroke();
        }
        assert!(s.mesh.positions.iter().all(|p| p.is_finite()));
        assert!(s.mesh.normals.iter().all(|n| n.is_finite()));
    }

    #[test]
    fn grab_moves_with_mirror() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let h = s.raycast(Vec3::new(0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        let hm = s.raycast(Vec3::new(-0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        s.begin_stroke();
        s.grab_begin(h.point, 0.3, h.tri, false);
        s.grab_begin(hm.point, 0.3, hm.tri, true);
        s.grab_update(Vec3::new(0.5, 0.0, 0.0));
        s.end_stroke();
        let maxx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let minx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!(maxx > 1.2 && minx < -1.2, "{minx} {maxx}");
    }

    #[test]
    fn subdivide_undo() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        let n = s.tri_count();
        s.subdivide().unwrap();
        assert_eq!(s.tri_count(), n * 4);
        s.undo();
        assert_eq!(s.tri_count(), n);
    }
}
