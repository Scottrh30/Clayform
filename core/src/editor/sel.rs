//! Selection gestures in Sculpt mode (like ZBrush):
//! - Ctrl + drag off the model: mask inside a lasso or rectangle
//!   (Ctrl + Alt: unmask), mirrored by symmetry. Ctrl + click off the model inverts the mask,
//!   Ctrl + Alt + click clears it.
//! - Ctrl + Shift + drag: the Select tool shows only what's inside (with Alt:
//!   hides it); Clip, Trim and Slice draw a line instead and act on the
//!   shaded side (Alt flips the side). Ctrl + Shift + click on the model
//!   isolates that polygroup (Alt: hides it); off the model shows everything.

use super::{Drag, Editor, Mode, Mods, Overlay, OverlayStyle};
use crate::select::{Plane, Shape};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// Outline used by the mask and Select gestures.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SelectShape {
    #[default]
    Lasso,
    Rect,
}

/// What Ctrl + Shift + drag does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CutTool {
    /// Show only what's inside the outline (Alt: hide it).
    #[default]
    Select,
    /// Flatten the shaded side onto the line.
    Clip,
    /// Cut away the shaded side and cap the hole.
    Trim,
    /// Split the faces along the line into new polygroups.
    Slice,
}

impl CutTool {
    pub const ALL: [CutTool; 4] = [CutTool::Select, CutTool::Clip, CutTool::Trim, CutTool::Slice];
    pub fn name(self) -> &'static str {
        match self {
            CutTool::Select => "Select (hide)",
            CutTool::Clip => "Clip",
            CutTool::Trim => "Trim",
            CutTool::Slice => "Slice",
        }
    }
    pub fn is_line(self) -> bool {
        self != CutTool::Select
    }
}

/// A one-shot click on the model, armed from a button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickAction {
    /// Mask everything outside the clicked polygroup.
    MaskGroup,
    /// Hide the clicked polygroup.
    HideGroup,
    /// Show only the clicked polygroup.
    IsolateGroup,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Mask { value: f32 },
    Hide { keep: bool },
    Cut { tool: CutTool, flip: bool },
}

pub(super) struct SelGesture {
    kind: Kind,
    pts: Vec<Vec2>,
    on_model: bool,
}

impl Editor {
    /// Starts a selection gesture if the modifiers ask for one.
    pub(super) fn sel_down(&mut self, p: Vec2, size: Vec2, mods: Mods) -> Option<Drag> {
        if self.mode != Mode::Sculpt {
            return None;
        }
        if let Some(a) = self.pending_pick.take() {
            self.do_pick(a, p, size);
            return Some(Drag::None);
        }
        if !mods.ctrl {
            return None;
        }
        let (o, d) = self.camera.ray(p, size);
        let on_model = self.sculptor.raycast(o, d).is_some();
        let kind = if mods.shift {
            match self.cut_tool {
                CutTool::Select => Kind::Hide { keep: !mods.alt },
                t => Kind::Cut { tool: t, flip: mods.alt },
            }
        } else if !on_model {
            Kind::Mask { value: if mods.alt { 0.0 } else { 1.0 } }
        } else {
            return None;
        };
        self.sel = Some(SelGesture { kind, pts: vec![p], on_model });
        self.redraw = true;
        Some(Drag::Sculpt)
    }

    pub(super) fn sel_active(&self) -> bool {
        self.sel.is_some()
    }

    pub(super) fn sel_move(&mut self, p: Vec2) {
        let Some(g) = self.sel.as_mut() else { return };
        let line = matches!(g.kind, Kind::Cut { .. });
        if line || self.select_shape == SelectShape::Rect {
            g.pts.truncate(1);
            g.pts.push(p);
        } else if g.pts.last().is_none_or(|q| q.distance(p) >= 3.0) {
            g.pts.push(p);
        }
        self.redraw = true;
    }

    fn shape_of(&self, g: &SelGesture) -> Option<Shape> {
        let a = g.pts[0];
        let b = *g.pts.last().unwrap();
        if self.select_shape == SelectShape::Rect {
            ((b - a).abs().min_element() >= 2.0).then_some(Shape::Rect(a, b))
        } else {
            (g.pts.len() >= 3).then(|| Shape::Lasso(g.pts.clone()))
        }
    }

    fn is_click(g: &SelGesture) -> bool {
        let a = g.pts[0];
        g.pts.iter().all(|q| q.distance(a) < 4.0)
    }

    pub(super) fn sel_up(&mut self) {
        let Some(g) = self.sel.take() else { return };
        let size = self.last_size;
        self.redraw = true;
        let res: Result<Option<String>, String> = match g.kind {
            Kind::Mask { value } if Self::is_click(&g) => {
                if value > 0.5 {
                    self.sculptor.invert_mask();
                } else {
                    self.sculptor.clear_mask();
                }
                Ok(None)
            }
            Kind::Hide { keep } if Self::is_click(&g) => {
                if g.on_model {
                    self.do_pick(if keep { PickAction::IsolateGroup } else { PickAction::HideGroup }, g.pts[0], size);
                    Ok(None)
                } else {
                    self.sculptor.show_all();
                    Ok(None)
                }
            }
            Kind::Mask { value } => match self.shape_of(&g) {
                Some(shape) => {
                    let sel = self.verts_inside(&shape, size);
                    let label = if value > 0.5 { "Mask lasso" } else { "Unmask lasso" };
                    self.sculptor.mask_selected(label, &sel, value);
                    Ok(None)
                }
                None => Ok(None),
            },
            Kind::Hide { keep } => match self.shape_of(&g) {
                Some(shape) => {
                    let sel = self.faces_inside(&shape, size);
                    self.sculptor.hide_faces(&sel, keep).map(|_| None)
                }
                None => Ok(None),
            },
            Kind::Cut { tool, flip } => {
                let (a, b) = (g.pts[0], *g.pts.last().unwrap());
                if a.distance(b) < 6.0 {
                    Ok(None)
                } else {
                    let planes = self.cut_planes(a, b, flip, size);
                    match tool {
                        CutTool::Clip => {
                            let n = self.sculptor.clip(&planes);
                            Ok((n == 0).then(|| "Nothing is past the line".to_string()))
                        }
                        CutTool::Trim => self.sculptor.trim(&planes).map(|_| None),
                        CutTool::Slice => self.sculptor.slice(&planes).map(|_| None),
                        CutTool::Select => Ok(None),
                    }
                }
            }
        };
        match res {
            Ok(Some(m)) | Err(m) => self.message = Some(m),
            Ok(None) => {}
        }
    }

    fn do_pick(&mut self, a: PickAction, p: Vec2, size: Vec2) {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else {
            self.message = Some("Click on the model to pick a polygroup".into());
            return;
        };
        let g = self.sculptor.mesh.groups[h.tri as usize];
        let r = match a {
            PickAction::MaskGroup => {
                self.sculptor.set_label("Mask by polygroup");
                self.sculptor.mask_outside_group(g);
                Ok(true)
            }
            PickAction::HideGroup => self.sculptor.hide_group(g, false),
            PickAction::IsolateGroup => self.sculptor.hide_group(g, true),
        };
        if let Err(e) = r {
            self.message = Some(e);
        }
        self.redraw = true;
    }

    fn view_proj(&self, size: Vec2) -> Mat4 {
        self.camera.proj(size.x / size.y) * self.camera.view()
    }

    /// Every symmetric copy (including the point itself) on screen.
    fn sym_points(&self, q: Vec3) -> impl Iterator<Item = Vec3> + '_ {
        let c = self.symmetry_center();
        std::iter::once(q).chain(self.sel_sym.iter().map(move |m| *m * (q - c) + c))
    }

    fn inside(&self, vp: &Mat4, shape: &Shape, size: Vec2, q: Vec3) -> bool {
        self.sym_points(q).any(|q| {
            let clip = *vp * Vec4::from((q, 1.0));
            if clip.w <= 1e-6 {
                return false;
            }
            let ndc = clip.truncate() / clip.w;
            shape.contains(Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y))
        })
    }

    /// Vertices whose screen position (or a symmetric copy's) is inside.
    pub fn verts_inside(&mut self, shape: &Shape, size: Vec2) -> Vec<bool> {
        self.sel_sym = self.brush.symmetry.copies();
        let vp = self.view_proj(size);
        let m = &self.sculptor.mesh;
        (0..m.vertex_count()).map(|v| self.inside(&vp, shape, size, m.positions[v])).collect()
    }

    /// Faces whose centre is inside (hiding ignores symmetry, as in ZBrush).
    pub fn faces_inside(&mut self, shape: &Shape, size: Vec2) -> Vec<bool> {
        self.sel_sym.clear();
        let vp = self.view_proj(size);
        let m = &self.sculptor.mesh;
        m.tris
            .iter()
            .map(|t| {
                t[0] != crate::mesh::NONE && {
                    let c = (m.positions[t[0] as usize] + m.positions[t[1] as usize] + m.positions[t[2] as usize]) / 3.0;
                    self.inside(&vp, shape, size, c)
                }
            })
            .collect()
    }

    /// The plane through the screen line a -> b along the view, its cut
    /// side to the right of the drag (`flip` swaps), plus symmetric copies.
    pub fn cut_planes(&self, a: Vec2, b: Vec2, flip: bool, size: Vec2) -> Vec<Plane> {
        let at = |p: Vec2, t: f32| {
            let (o, d) = self.camera.ray(p, size);
            o + d * t
        };
        let dist = (self.camera.eye() - self.sculptor.mesh.bounds().0.lerp(self.sculptor.mesh.bounds().1, 0.5)).length().max(0.1);
        let (pa, pb, pc) = (at(a, dist), at(b, dist), at(a, dist * 2.0));
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        let mut pl = Plane::through(pa, n);
        // the screen point to the right of the drag must be on the cut side
        let dir = (b - a).normalize_or_zero();
        let right = (a + b) * 0.5 + Vec2::new(-dir.y, dir.x) * 20.0;
        if pl.eval(at(right, dist)) < 0.0 {
            pl = pl.flipped();
        }
        if flip {
            pl = pl.flipped();
        }
        let mut out = vec![pl];
        let c = self.symmetry_center();
        for m in self.brush.symmetry.copies() {
            let q = pl.transformed(m, c);
            if !out.iter().any(|o| o.n.dot(q.n) > 0.9999 && (o.d - q.d).abs() < 1e-4) {
                out.push(q);
            }
        }
        out
    }

    /// The gesture being drawn.
    pub(super) fn sel_overlay(&self, o: &mut Overlay) {
        let Some(g) = &self.sel else { return };
        let (style, line) = match g.kind {
            Kind::Mask { value } => (OverlayStyle::Select(if value > 0.5 { 0 } else { 1 }), false),
            Kind::Hide { keep } => (OverlayStyle::Select(if keep { 2 } else { 3 }), false),
            Kind::Cut { .. } => (OverlayStyle::Select(4), true),
        };
        if line {
            let (a, b) = (g.pts[0], *g.pts.last().unwrap());
            if a.distance(b) < 1.0 {
                return;
            }
            // extend across the view, with ticks on the side that's cut
            let dir = (b - a).normalize();
            let far = self.last_size.length();
            o.lines.push((a - dir * far, b + dir * far, style));
            let flip = matches!(g.kind, Kind::Cut { flip: true, .. });
            let right = Vec2::new(-dir.y, dir.x) * if flip { -1.0 } else { 1.0 };
            let len = a.distance(b);
            let ticks = (len / 14.0).max(2.0) as usize;
            for i in 0..=ticks {
                let q = a + dir * len * i as f32 / ticks as f32;
                o.lines.push((q, q + right * 10.0 - dir * 5.0, style));
            }
        } else if let Some(shape) = self.shape_of(g) {
            let pts = shape.outline();
            for i in 0..pts.len() {
                o.lines.push((pts[i], pts[(i + 1) % pts.len()], style));
            }
        } else {
            for w in g.pts.windows(2) {
                o.lines.push((w[0], w[1], style));
            }
        }
    }

    /// Extract: the masked area becomes a new SubTool, a shell `thickness`
    /// thick (a fraction of the model's size).
    pub fn extract(&mut self, thickness: f32, smooth: u32) -> Result<(), String> {
        let (mn, mx) = self.sculptor.mesh.bounds();
        let t = thickness * (mx - mn).max_element().max(1e-6);
        let m = crate::select::extract_shell(&self.sculptor.mesh, t, smooth)?;
        let name = format!("{} extract", self.subtools[self.active].info.name);
        self.append_subtool(m, &name);
        self.redraw = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Button;

    fn drag(e: &mut Editor, a: Vec2, b: Vec2, mods: Mods) {
        let size = Vec2::new(800.0, 600.0);
        e.pointer_down(Button::Left, a, size, mods);
        for i in 1..=8 {
            e.pointer_move(a.lerp(b, i as f32 / 8.0), size);
        }
        e.pointer_up();
    }

    #[test]
    fn gestures_mask_hide_and_trim() {
        let size = Vec2::new(800.0, 600.0);
        let mut e = Editor::new();
        e.brush.symmetry.mirror = [false; 3];
        e.select_shape = SelectShape::Rect;
        let ctrl = Mods { ctrl: true, ..Default::default() };
        drag(&mut e, Vec2::new(1.0, 1.0), Vec2::new(400.0, 599.0), ctrl);
        let m = &e.sculptor.mesh;
        let mut masked = 0;
        for v in 0..m.vertex_count() {
            let s = e.to_screen(m.positions[v], size).unwrap();
            if (s.x - 400.0).abs() > 2.0 {
                assert_eq!(m.mask[v] == 1.0, s.x < 400.0, "vertex at {s}");
            }
            masked += (m.mask[v] == 1.0) as usize;
        }
        assert!(masked > m.vertex_count() / 3 && masked < m.vertex_count() * 2 / 3);
        // Ctrl + Alt + click off the model clears it
        let ctrl_alt = Mods { ctrl: true, alt: true, ..Default::default() };
        drag(&mut e, Vec2::new(2.0, 2.0), Vec2::new(2.0, 2.0), ctrl_alt);
        assert!(e.sculptor.mesh.mask.iter().all(|&m| m == 0.0));
        // Ctrl + Shift: show only the left half
        let cs = Mods { ctrl: true, shift: true, ..Default::default() };
        drag(&mut e, Vec2::new(1.0, 1.0), Vec2::new(400.0, 599.0), cs);
        assert!(e.sculptor.mesh.any_hidden());
        let hidden = e.sculptor.mesh.hidden.iter().filter(|&&h| h).count();
        assert!(hidden > e.sculptor.mesh.tris.len() / 3);
        // Ctrl + Shift + click off the model shows everything again
        drag(&mut e, Vec2::new(2.0, 2.0), Vec2::new(2.0, 2.0), cs);
        assert!(!e.sculptor.mesh.any_hidden());
        // Trim: a downward line cuts away its right-hand side (screen left)
        e.cut_tool = CutTool::Trim;
        drag(&mut e, Vec2::new(400.0, 5.0), Vec2::new(400.0, 595.0), cs);
        let m = &e.sculptor.mesh;
        assert!(m.vertex_count() > 100);
        for p in &m.positions {
            assert!(e.to_screen(*p, size).unwrap().x > 398.0);
        }
        e.undo();
        assert!(e.sculptor.mesh.positions.iter().any(|p| e.to_screen(*p, size).unwrap().x < 300.0));
    }

    #[test]
    fn symmetric_trim_cuts_both_sides() {
        let size = Vec2::new(800.0, 600.0);
        let mut e = Editor::new();
        e.brush.symmetry.mirror = [true, false, false];
        e.view_from(crate::editor::ViewDir::Front);
        e.cut_tool = CutTool::Trim;
        let cs = Mods { ctrl: true, shift: true, ..Default::default() };
        // cut the screen-left third: the mirror copy cuts the right third
        drag(&mut e, Vec2::new(300.0, 5.0), Vec2::new(300.0, 595.0), cs);
        let xs: Vec<f32> = e.sculptor.mesh.positions.iter().map(|p| e.to_screen(*p, size).unwrap().x).collect();
        let (mn, mx) = xs.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(mn > 298.0 && mx < 502.0, "{mn}..{mx}");
    }

    #[test]
    fn picking_a_polygroup_isolates_it() {
        let size = Vec2::new(800.0, 600.0);
        let mut e = Editor::new();
        e.brush.symmetry.mirror = [false; 3];
        let n = e.sculptor.mesh.tris.len();
        let groups: Vec<u32> = (0..n).map(|f| (e.sculptor.mesh.face_normal_raw(f).x > 0.0) as u32).collect();
        e.sculptor.set_groups(groups);
        e.pending_pick = Some(PickAction::IsolateGroup);
        e.pointer_down(Button::Left, Vec2::new(400.0, 300.0), size, Mods::default());
        e.pointer_up();
        let m = &e.sculptor.mesh;
        let shown: std::collections::HashSet<u32> = (0..n).filter(|&f| !m.hidden[f]).map(|f| m.groups[f]).collect();
        assert_eq!(shown.len(), 1);
        assert!(e.pending_pick.is_none());
    }
}
