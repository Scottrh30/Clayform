//! Retopology mode: click on the sculpt to place vertices and build faces.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use glam::Vec2;

const PICK_PX: f32 = 10.0;

#[derive(Default)]
pub(super) struct RtState {
    pub(super) drag: Option<RtDrag>,
    /// A topology stroke is being drawn.
    pub(super) stroking: bool,
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

    /// The open retopology edge nearest the cursor on screen, if close.
    fn rt_open_edge_at(&self, p: Vec2, size: Vec2) -> Option<(u32, u32)> {
        let mut best: Option<(f32, (u32, u32))> = None;
        for (a, b) in self.retopo.open_edges() {
            let (qa, qb) = (self.retopo.verts[a as usize], self.retopo.verts[b as usize]);
            let (Some(sa), Some(sb)) = (self.to_screen(qa, size), self.to_screen(qb, size)) else { continue };
            if !self.visible((qa + qb) * 0.5) {
                continue;
            }
            let ab = sb - sa;
            let t = ((p - sa).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = (sa + ab * t).distance(p);
            if d <= PICK_PX * 1.5 && best.is_none_or(|x| d < x.0) {
                best = Some((d, (a, b)));
            }
        }
        best.map(|b| b.1)
    }

    pub(super) fn rt_down(&mut self, p: Vec2, size: Vec2, mods: Mods) -> Option<Drag> {
        if mods.shift {
            // Shift+click an open edge: continue the strip of quads
            if let Some((a, b)) = self.rt_open_edge_at(p, size) {
                self.retopo.checkpoint();
                let (mn, mx) = self.sculptor.mesh.bounds();
                let reach = (mx - mn).length();
                let s = &self.sculptor;
                let eye = self.camera.eye();
                if !self.retopo.extend_edge(a, b, &|q| s.closest_point(q, reach).unwrap_or(q), eye) {
                    self.retopo.undo();
                }
                self.redraw = true;
                return Some(Drag::None);
            }
        }
        if self.retopo_draw && !mods.ctrl && self.rt_vertex_at(p, size).is_none() {
            let (o, d) = self.camera.ray(p, size);
            let h = self.sculptor.raycast(o, d)?;
            self.retopo.strokes.push(vec![h.point]);
            self.rts.stroking = true;
            self.redraw = true;
            return Some(Drag::Sculpt);
        }
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
        if self.rts.stroking {
            let (o, d) = self.camera.ray(p, size);
            if let Some(h) = self.sculptor.raycast(o, d) {
                let wpp = self.camera.world_per_point(h.point, size.y);
                if let Some(st) = self.retopo.strokes.last_mut() {
                    if st.last().is_none_or(|q| q.distance(h.point) > wpp * 4.0) {
                        st.push(h.point);
                        self.redraw = true;
                    }
                }
            }
            return;
        }
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
        if self.rts.stroking {
            self.rts.stroking = false;
            if self.retopo.strokes.last().is_some_and(|s| s.len() < 2) {
                self.retopo.strokes.pop();
            }
            self.redraw = true;
            return;
        }
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
        for st in &self.retopo.strokes {
            for w in st.windows(2) {
                if let (Some(a), Some(b)) = (self.to_screen(w[0], size), self.to_screen(w[1], size)) {
                    o.lines.push((a, b, OverlayStyle::Curve));
                }
            }
        }
        if let Some(pp) = p {
            if self.rts.drag.is_none() && !self.rts.stroking {
                if let Some((a, b)) = self.rt_open_edge_at(pp, size) {
                    let (qa, qb) = (self.retopo.verts[a as usize], self.retopo.verts[b as usize]);
                    if let (Some(sa), Some(sb)) = (self.to_screen(qa, size), self.to_screen(qb, size)) {
                        o.lines.push((sa, sb, OverlayStyle::Target));
                    }
                }
            }
        }
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

    /// Topology brush: turns the drawn strokes into faces where they cross.
    pub fn retopo_build_strokes(&mut self) -> usize {
        if self.retopo.strokes.len() < 2 {
            self.message = Some("Draw at least two crossing strokes first".into());
            return 0;
        }
        self.retopo.checkpoint();
        let (mn, mx) = self.sculptor.mesh.bounds();
        let size = (mx - mn).max_element();
        let s = &self.sculptor;
        let eye = self.camera.eye();
        let normal_at = |q: glam::Vec3| {
            s.closest_face(q, size).map_or(glam::Vec3::Y, |(_, f)| s.mesh.face_normals[f as usize])
        };
        // crossings within ~1% of the model count as touching
        let n = self.retopo.build_from_strokes(size * 0.012, &normal_at, eye);
        self.message = Some(if n == 0 { "No closed cells: make the strokes cross each other".into() } else { format!("Made {n} faces from the strokes") });
        self.redraw = true;
        n
    }

    pub fn retopo_clear_strokes(&mut self) {
        self.retopo.strokes.clear();
        self.redraw = true;
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
