//! ZSpheres mode: click a sphere to grow a new one from it (drag to size
//! it), move, scale or delete spheres, then make the skin a mesh.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use crate::mesh::Mesh;
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZsTool {
    Draw,
    Move,
    Scale,
    Delete,
}

impl ZsTool {
    pub const ALL: [ZsTool; 4] = [ZsTool::Draw, ZsTool::Move, ZsTool::Scale, ZsTool::Delete];
    pub fn name(self) -> &'static str {
        match self {
            ZsTool::Draw => "Draw",
            ZsTool::Move => "Move",
            ZsTool::Scale => "Scale",
            ZsTool::Delete => "Delete",
        }
    }
}

#[derive(Default)]
pub(super) struct ZsState {
    drag: Option<ZsDrag>,
    /// Skin preview: (tree it was made from, mesh).
    pub(super) preview: Option<(Vec<crate::zspheres::ZNode>, Mesh)>,
    pub(super) preview_dirty: crate::sculpt::GpuDirty,
}

struct ZsDrag {
    /// Sphere(s) being changed: the main one and its mirror.
    nodes: Vec<usize>,
    start: Vec2,
    wpp: f32,
    /// Positions and radii at the start.
    orig: Vec<(Vec3, f32)>,
    /// Draw: the surface normal the new sphere grew along.
    grow: Option<Vec3>,
    plane_n: Vec3,
    anchor: Vec3,
}

fn mirror(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

impl Editor {
    /// The sphere under the cursor (nearest the camera) and where the ray meets it.
    fn zs_pick(&self, p: Vec2, size: Vec2) -> Option<(usize, Vec3)> {
        let (o, d) = self.camera.ray(p, size);
        let mut best: Option<(f32, usize, Vec3)> = None;
        for (i, n) in self.zspheres.nodes.iter().enumerate() {
            let oc = o - n.pos;
            let b = oc.dot(d);
            let c = oc.length_squared() - n.radius * n.radius;
            let disc = b * b - c;
            if disc < 0.0 {
                continue;
            }
            let t = -b - disc.sqrt();
            if t > 0.0 && best.is_none_or(|x| t < x.0) {
                best = Some((t, i, o + d * t));
            }
        }
        best.map(|b| (b.1, b.2))
    }

    pub(super) fn zs_down(&mut self, p: Vec2, size: Vec2, _mods: Mods) -> Option<Drag> {
        let tool = self.zs_tool;
        let mirror_on = self.brush.symmetry.mirror[0];
        if self.zspheres.is_empty() {
            // the first sphere sits where the camera looks
            let (o, d) = self.camera.ray(p, size);
            let f = self.camera.forward();
            let t = (self.camera.target - o).dot(f) / d.dot(f).max(1e-6);
            let mut c = o + d * t;
            if mirror_on && c.x.abs() < self.camera.dist * 0.05 {
                c.x = 0.0;
            }
            self.zspheres.checkpoint();
            let r = self.camera.world_per_point(c, size.y) * 60.0;
            self.zspheres.add(c, r, None);
            self.redraw = true;
            return Some(Drag::None);
        }
        let (hit, point) = self.zs_pick(p, size)?;
        let wpp = self.camera.world_per_point(point, size.y);
        let plane_n = self.camera.forward();
        self.zspheres.checkpoint();
        match tool {
            ZsTool::Delete => {
                if self.zspheres.nodes[hit].parent.is_none() {
                    self.message = Some("The first sphere can't be deleted (Clear removes them all)".into());
                    self.zspheres.undo();
                    return Some(Drag::None);
                }
                let m = if mirror_on { self.zspheres.mirror_of(hit) } else { None };
                // delete the higher index first so the other stays valid
                let mut both: Vec<usize> = std::iter::once(hit).chain(m).collect();
                both.sort_unstable();
                for i in both.into_iter().rev() {
                    if i < self.zspheres.nodes.len() {
                        self.zspheres.remove(i);
                    }
                }
                self.redraw = true;
                Some(Drag::None)
            }
            ZsTool::Draw => {
                let parent = &self.zspheres.nodes[hit];
                let n = (point - parent.pos).normalize_or(Vec3::Y);
                let r = parent.radius * 0.7;
                let pos = parent.pos + n * parent.radius;
                let a = self.zspheres.add(pos, r, Some(hit));
                let mut nodes = vec![a];
                if mirror_on && pos.x.abs() > r * 0.2 {
                    let mp = self.zspheres.mirror_of(hit).unwrap_or(hit);
                    let b = self.zspheres.add(mirror(pos), r, Some(mp));
                    nodes.push(b);
                }
                let orig = nodes.iter().map(|&i| (self.zspheres.nodes[i].pos, self.zspheres.nodes[i].radius)).collect();
                self.zsst.drag = Some(ZsDrag { nodes, start: p, wpp, orig, grow: Some(n), plane_n, anchor: point });
                self.redraw = true;
                Some(Drag::Sculpt)
            }
            ZsTool::Move | ZsTool::Scale => {
                let mut nodes = vec![hit];
                if mirror_on {
                    if let Some(m) = self.zspheres.mirror_of(hit) {
                        nodes.push(m);
                    }
                }
                let orig = nodes.iter().map(|&i| (self.zspheres.nodes[i].pos, self.zspheres.nodes[i].radius)).collect();
                self.zsst.drag = Some(ZsDrag { nodes, start: p, wpp, orig, grow: None, plane_n, anchor: self.zspheres.nodes[hit].pos });
                Some(Drag::Sculpt)
            }
        }
    }

    pub(super) fn zs_move(&mut self, p: Vec2, size: Vec2) {
        let Some(dr) = self.zsst.drag.as_ref() else { return };
        let d = p - dr.start;
        match (self.zs_tool, dr.grow) {
            (ZsTool::Draw, Some(n)) => {
                // dragging away from the click makes the new sphere bigger
                let r = (dr.orig[0].1 + d.length() * dr.wpp * 0.5).max(dr.orig[0].1 * 0.2);
                let parent = self.zspheres.nodes[dr.nodes[0]].parent.unwrap();
                let pp = self.zspheres.nodes[parent].pos;
                let pr = self.zspheres.nodes[parent].radius;
                let pos = pp + n * (pr + r * 0.3);
                self.zspheres.nodes[dr.nodes[0]].radius = r;
                self.zspheres.nodes[dr.nodes[0]].pos = pos;
                if let Some(&m) = dr.nodes.get(1) {
                    self.zspheres.nodes[m].radius = r;
                    self.zspheres.nodes[m].pos = mirror(pos);
                }
            }
            (ZsTool::Scale, _) => {
                let k = (1.0 + d.x / 150.0).max(0.05);
                for (j, &i) in dr.nodes.iter().enumerate() {
                    self.zspheres.nodes[i].radius = dr.orig[j].1 * k;
                }
            }
            _ => {
                // move in the view plane through the sphere
                let (o, dir) = self.camera.ray(p, size);
                let denom = dir.dot(dr.plane_n);
                if denom.abs() < 1e-6 {
                    return;
                }
                let t = (dr.anchor - o).dot(dr.plane_n) / denom;
                let delta = o + dir * t - dr.anchor;
                let first = dr.orig[0].0 + delta;
                self.zspheres.nodes[dr.nodes[0]].pos = first;
                if let Some(&m) = dr.nodes.get(1) {
                    self.zspheres.nodes[m].pos = mirror(first);
                }
            }
        }
        self.redraw = true;
    }

    pub(super) fn zs_cancel(&mut self) {
        self.zsst.drag = None;
    }

    pub(super) fn zs_up(&mut self) {
        self.zsst.drag = None;
        self.redraw = true;
    }

    pub(super) fn zs_overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        let mut o = Overlay::default();
        let hover = p.and_then(|p| self.zs_pick(p, size)).map(|h| h.0);
        let eye = self.camera.eye();
        for (i, n) in self.zspheres.nodes.iter().enumerate() {
            let Some(c) = self.to_screen(n.pos, size) else { continue };
            let r = n.radius / self.camera.world_per_point(n.pos, size.y);
            let style = if Some(i) == hover { OverlayStyle::Active } else { OverlayStyle::Joint };
            let k = 40;
            for s in 0..k {
                let a0 = s as f32 / k as f32 * std::f32::consts::TAU;
                let a1 = (s + 1) as f32 / k as f32 * std::f32::consts::TAU;
                o.lines.push((c + Vec2::from_angle(a0) * r, c + Vec2::from_angle(a1) * r, style));
            }
            if let Some(pi) = n.parent {
                if let Some(pc) = self.to_screen(self.zspheres.nodes[pi].pos, size) {
                    o.lines.push((pc, c, OverlayStyle::Mesh));
                }
            }
            let _ = eye;
        }
        o
    }

    /// The live skin of the spheres (rebuilt when they change).
    pub fn zs_preview(&mut self) -> Option<(&Mesh, &mut crate::sculpt::GpuDirty)> {
        if self.zspheres.is_empty() {
            self.zsst.preview = None;
            return None;
        }
        let fresh = self.zsst.preview.as_ref().is_some_and(|(n, _)| *n == self.zspheres.nodes);
        if !fresh {
            let res = if self.zsst.drag.is_some() { 40 } else { 64 };
            match self.zspheres.skin(res) {
                Some(m) => {
                    self.zsst.preview = Some((self.zspheres.nodes.clone(), m));
                    self.zsst.preview_dirty = crate::sculpt::GpuDirty { topology: true, ..Default::default() };
                }
                None => return None,
            }
        }
        let (_, m) = self.zsst.preview.as_ref().unwrap();
        Some((m, &mut self.zsst.preview_dirty))
    }

    /// Turns the spheres into a mesh: a new SubTool, or the active
    /// SubTool's sculpt (undoable). Returns the triangle count.
    pub fn zs_make_mesh(&mut self, res: usize, as_subtool: bool) -> Result<usize, String> {
        let mut m = self.zspheres.skin(res).ok_or("Add some ZSpheres first: click in the view")?;
        let tris = m.tris.len();
        m.export_center = self.sculptor.mesh.export_center;
        m.export_scale = self.sculptor.mesh.export_scale;
        if as_subtool {
            self.append_subtool(m, "ZSpheres");
        } else {
            self.sculptor.set_label("ZSpheres mesh");
            self.sculptor.set_mesh(m);
        }
        self.mode = super::Mode::Sculpt;
        self.redraw = true;
        Ok(tris)
    }

    /// Undo in ZSpheres mode steps back the spheres.
    pub fn zs_undo(&mut self) -> bool {
        if self.mode == super::Mode::ZSpheres && self.zspheres.can_undo() {
            self.zspheres.undo();
            self.redraw = true;
            true
        } else {
            false
        }
    }
}
