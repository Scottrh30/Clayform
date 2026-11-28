//! Retopology mode: click on the sculpt to place vertices and build faces.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use glam::Vec2;

const PICK_PX: f32 = 10.0;

#[derive(Default)]
pub(super) struct RtState {
    pub(super) drag: Option<RtDrag>,
}

pub(super) struct RtDrag {
    v: u32,
    start: Vec2,
    moved: bool,
    /// The vertex was just placed (it's already part of the face being built).
    created: bool,
}

impl Editor {
    /// The retopology vertex nearest the cursor on screen, if visible and close.
    fn rt_vertex_at(&self, p: Vec2, size: Vec2) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for (i, &q) in self.retopo.verts.iter().enumerate() {
            let Some(s) = self.to_screen(q, size) else { continue };
            let d = s.distance(p);
            if d <= PICK_PX && best.is_none_or(|b| d < b.0) && self.visible(q) {
                best = Some((d, i as u32));
            }
        }
        best.map(|b| b.1)
    }

    pub(super) fn rt_down(&mut self, p: Vec2, size: Vec2, mods: Mods) -> Option<Drag> {
        if let Some(v) = self.rt_vertex_at(p, size) {
            if mods.ctrl {
                self.retopo.checkpoint();
                self.retopo.remove_vertex(v);
                self.redraw = true;
                return Some(Drag::None);
            }
            self.rts.drag = Some(RtDrag { v, start: p, moved: false, created: false });
            return Some(Drag::Sculpt);
        }
        let (o, d) = self.camera.ray(p, size);
        let h = self.sculptor.raycast(o, d)?;
        if mods.ctrl {
            return Some(Drag::None);
        }
        self.retopo.checkpoint();
        let v = self.retopo.add_vertex(h.point);
        self.retopo.pick(v, self.camera.eye());
        self.rts.drag = Some(RtDrag { v, start: p, moved: false, created: true });
        self.redraw = true;
        Some(Drag::Sculpt)
    }

    pub(super) fn rt_move(&mut self, p: Vec2, size: Vec2) {
        let Some(dr) = self.rts.drag.as_mut() else { return };
        if !dr.moved && dr.start.distance(p) < 4.0 {
            return;
        }
        if !dr.moved && !dr.created {
            self.retopo.checkpoint();
        }
        dr.moved = true;
        let v = dr.v as usize;
        let (o, d) = self.camera.ray(p, size);
        if let Some(h) = self.sculptor.raycast(o, d) {
            if v < self.retopo.verts.len() {
                self.retopo.verts[v] = h.point;
            }
        }
        self.redraw = true;
    }

    pub(super) fn rt_up(&mut self) {
        if let Some(dr) = self.rts.drag.take() {
            if !dr.moved && !dr.created {
                self.retopo.checkpoint();
                self.retopo.pick(dr.v, self.camera.eye());
            }
        }
        self.redraw = true;
    }

    pub(super) fn rt_overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        let mut o = Overlay::default();
        let screen: Vec<Option<Vec2>> = self
            .retopo
            .verts
            .iter()
            .map(|&q| if self.visible(q) { self.to_screen(q, size) } else { None })
            .collect();
        for f in &self.retopo.faces {
            let pts: Option<Vec<Vec2>> = f.iter().map(|&v| screen[v as usize]).collect();
            if let Some(pts) = pts {
                o.polys.push(pts);
            }
        }
        for (a, b) in self.retopo.edges() {
            if let (Some(a), Some(b)) = (screen[a as usize], screen[b as usize]) {
                o.lines.push((a, b, OverlayStyle::Mesh));
            }
        }
        let pend: Vec<Vec2> = self.retopo.pending.iter().filter_map(|&v| screen[v as usize]).collect();
        for w in pend.windows(2) {
            o.lines.push((w[0], w[1], OverlayStyle::Building));
        }
        let hover = p.and_then(|p| self.rt_vertex_at(p, size));
        if let (Some(&last), Some(p)) = (pend.last(), p) {
            if self.rts.drag.is_none() {
                o.lines.push((last, hover.and_then(|h| screen[h as usize]).unwrap_or(p), OverlayStyle::Building));
            }
        }
        for (i, s) in screen.iter().enumerate() {
            if let Some(s) = s {
                let style = if Some(i as u32) == hover {
                    OverlayStyle::Active
                } else if self.retopo.pending.contains(&(i as u32)) {
                    OverlayStyle::Building
                } else {
                    OverlayStyle::Mesh
                };
                o.points.push((*s, style));
            }
        }
        o
    }

    /// Relaxes the retopology, keeping it on the sculpt.
    pub fn retopo_relax(&mut self) {
        if self.retopo.is_empty() {
            return;
        }
        self.retopo.checkpoint();
        let (mn, mx) = self.sculptor.mesh.bounds();
        let reach = (mx - mn).length();
        let s = &self.sculptor;
        self.retopo.relax(10, &|q| s.closest_point(q, reach).unwrap_or(q));
        self.redraw = true;
    }

    pub fn retopo_mirror(&mut self) {
        if self.retopo.is_empty() {
            return;
        }
        self.retopo.checkpoint();
        let (mn, mx) = self.sculptor.mesh.bounds();
        self.retopo.mirror_x((mx - mn).max_element() * 0.004);
        self.redraw = true;
    }

    /// Replaces the sculpt with the retopology. With `levels` > 0 it is
    /// subdivided that many times, each level projected onto the old
    /// sculpt so its detail carries over (undoable).
    pub fn retopo_apply(&mut self, levels: u32) -> Result<(), String> {
        let polys = self.retopo.to_polys();
        if polys.faces.is_empty() {
            return Err("Draw some faces first: click four points on the model to make a quad".into());
        }
        self.cancel_drag();
        let original = self.sculptor.mesh.clone();
        let bvh = crate::bvh::Bvh::build(&original);
        let mut m = polys.to_mesh();
        m.export_center = original.export_center;
        m.export_scale = original.export_scale;
        self.sculptor.set_label("Apply retopology");
        self.sculptor.set_mesh(m);
        let reach = {
            let (mn, mx) = original.bounds();
            (mx - mn).length()
        };
        for _ in 0..levels.min(5) {
            self.sculptor.subdivide()?;
            self.sculptor.project_onto(&original, &bvh, reach);
        }
        self.retopo.clear();
        self.mode = super::Mode::Sculpt;
        self.redraw = true;
        Ok(())
    }

    /// Undo in retopology mode steps back the retopology instead of the sculpt.
    pub fn retopo_undo(&mut self) -> bool {
        if self.mode == super::Mode::Retopo && self.retopo.can_undo() {
            self.retopo.undo();
            self.redraw = true;
            true
        } else {
            false
        }
    }

    /// Closes the face being built from the vertices picked so far.
    pub fn retopo_close_face(&mut self) {
        if self.retopo.pending.len() >= 3 {
            self.retopo.checkpoint();
            let f = std::mem::take(&mut self.retopo.pending);
            self.retopo.add_face(f, self.camera.eye());
        } else {
            self.retopo.pending.clear();
        }
        self.redraw = true;
    }
}
