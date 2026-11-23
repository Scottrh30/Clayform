//! The editor: sculptor + camera + brush settings, driven by pointer input.
//! Interface-independent so the stroke logic can be unit tested.

use crate::camera::Camera;
use crate::mesh::Mesh;
use crate::sculpt::{mirror_x, DabParams, Sculptor, Tool};
use glam::{Vec2, Vec3, Vec4};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Brush {
    pub tool: Tool,
    pub radius_px: f32,
    pub intensity: [f32; Tool::ALL.len()],
    pub negative: bool,
    pub symmetry: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            tool: Tool::Draw,
            radius_px: 60.0,
            intensity: Tool::ALL.map(|t| t.default_intensity()),
            negative: false,
            symmetry: true,
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drag {
    None,
    Sculpt,
    Orbit,
    Pan,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewDir {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

pub struct Editor {
    pub sculptor: Sculptor,
    pub camera: Camera,
    pub brush: Brush,
    drag: Drag,
    stroke_tool: Tool,
    stroke_negative: bool,
    last_dab: Vec2,
    last_pos: Vec2,
    grab: Option<(Vec3, Vec3)>,
    /// Set whenever the picture changes; the interface clears it after drawing.
    pub redraw: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        let mut e = Self {
            sculptor: Sculptor::new(Mesh::icosphere(6)),
            camera: Camera::default(),
            brush: Brush::default(),
            drag: Drag::None,
            stroke_tool: Tool::Draw,
            stroke_negative: false,
            last_dab: Vec2::ZERO,
            last_pos: Vec2::ZERO,
            grab: None,
            redraw: true,
        };
        e.frame_mesh();
        e
    }

    pub fn drag(&self) -> Drag {
        self.drag
    }

    /// The tool a press would use right now (Shift = Smooth).
    pub fn effective_tool(&self, mods: Mods) -> Tool {
        if self.drag == Drag::Sculpt {
            self.stroke_tool
        } else if mods.shift && self.brush.tool != Tool::Mask {
            Tool::Smooth
        } else {
            self.brush.tool
        }
    }

    pub fn effective_negative(&self, mods: Mods) -> bool {
        let t = self.effective_tool(mods);
        let neg = if self.drag == Drag::Sculpt { self.stroke_negative } else { self.brush.negative ^ mods.ctrl };
        neg && t.has_negative()
    }

    pub fn set_mesh(&mut self, m: Mesh) {
        self.cancel_drag();
        self.sculptor.set_mesh(m);
        self.frame_mesh();
        self.redraw = true;
    }

    pub fn frame_mesh(&mut self) {
        let (mn, mx) = self.sculptor.mesh.bounds();
        self.camera.frame(mn, mx);
        self.redraw = true;
    }

    pub fn reset_camera(&mut self) {
        let fov = self.camera.fov_y;
        self.camera = Camera { fov_y: fov, ..Camera::default() };
        self.frame_mesh();
    }

    pub fn view_from(&mut self, dir: ViewDir) {
        use std::f32::consts::{FRAC_PI_2, PI};
        let (yaw, pitch) = match dir {
            ViewDir::Front => (0.0, 0.0),
            ViewDir::Back => (PI, 0.0),
            ViewDir::Left => (-FRAC_PI_2, 0.0),
            ViewDir::Right => (FRAC_PI_2, 0.0),
            ViewDir::Top => (0.0, 1.55),
            ViewDir::Bottom => (0.0, -1.55),
        };
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.redraw = true;
    }

    pub fn undo(&mut self) {
        if self.drag == Drag::None {
            self.sculptor.undo();
            self.redraw = true;
        }
    }

    pub fn redo(&mut self) {
        if self.drag == Drag::None {
            self.sculptor.redo();
            self.redraw = true;
        }
    }

    fn cancel_drag(&mut self) {
        if self.drag == Drag::Sculpt {
            self.sculptor.end_stroke();
        }
        self.grab = None;
        self.drag = Drag::None;
    }

    // ------------------------------------------------------------------ input

    /// `p` and `size` are in CSS pixels / points, origin top-left of the viewport.
    pub fn pointer_down(&mut self, button: Button, p: Vec2, size: Vec2, mods: Mods) -> Drag {
        self.cancel_drag();
        self.sculptor.refresh_bvh();
        self.last_pos = p;
        self.drag = match button {
            Button::Left if mods.alt => Drag::Orbit,
            Button::Left => {
                if self.begin_sculpt(p, size, mods) {
                    Drag::Sculpt
                } else if mods.shift {
                    Drag::Pan
                } else {
                    Drag::Orbit
                }
            }
            Button::Right if mods.shift || mods.ctrl => Drag::Pan,
            Button::Right => Drag::Orbit,
            Button::Middle => Drag::Pan,
        };
        self.drag
    }

    pub fn pointer_move(&mut self, p: Vec2, size: Vec2) {
        let d = p - self.last_pos;
        self.last_pos = p;
        match self.drag {
            Drag::Orbit => {
                self.camera.orbit(d);
                self.redraw = true;
            }
            Drag::Pan => {
                self.camera.pan(d, size.y);
                self.redraw = true;
            }
            Drag::Sculpt => {
                self.sculptor.refresh_bvh();
                self.continue_sculpt(p, size);
            }
            Drag::None => {}
        }
    }

    pub fn pointer_up(&mut self) {
        self.cancel_drag();
    }

    pub fn wheel(&mut self, delta_y: f32) {
        if self.drag != Drag::Sculpt {
            self.camera.zoom(delta_y);
            self.redraw = true;
        }
    }

    fn dab_at(&mut self, p: Vec2, size: Vec2) {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return };
        let r = self.brush.radius_px * self.camera.world_per_point(h.point, size.y);
        let mut params = DabParams {
            tool: self.stroke_tool,
            center: h.point,
            radius: r,
            intensity: self.brush.intensity[self.stroke_tool.index()],
            negative: self.stroke_negative,
            seed_tri: h.tri,
        };
        self.sculptor.dab(&params);
        if self.brush.symmetry {
            if let Some(hm) = self.sculptor.raycast(mirror_x(o), mirror_x(d)) {
                params.center = hm.point;
                params.seed_tri = hm.tri;
                self.sculptor.dab(&params);
            }
        }
        self.redraw = true;
    }

    fn begin_sculpt(&mut self, p: Vec2, size: Vec2, mods: Mods) -> bool {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return false };
        self.stroke_tool = if mods.shift && self.brush.tool != Tool::Mask { Tool::Smooth } else { self.brush.tool };
        self.stroke_negative = self.brush.negative ^ mods.ctrl;
        self.sculptor.begin_stroke();
        if self.stroke_tool == Tool::Grab {
            let r = self.brush.radius_px * self.camera.world_per_point(h.point, size.y);
            self.sculptor.grab_begin(h.point, r, h.tri, false);
            if self.brush.symmetry {
                if let Some(hm) = self.sculptor.raycast(mirror_x(o), mirror_x(d)) {
                    self.sculptor.grab_begin(hm.point, r, hm.tri, true);
                }
            }
            self.grab = Some((h.point, self.camera.forward()));
        } else {
            // the drag state must be Sculpt while the first dab is applied
            self.drag = Drag::Sculpt;
            self.dab_at(p, size);
            self.last_dab = p;
        }
        true
    }

    fn continue_sculpt(&mut self, p: Vec2, size: Vec2) {
        if self.stroke_tool == Tool::Grab {
            let Some((anchor, normal)) = self.grab else { return };
            let (o, d) = self.camera.ray(p, size);
            let denom = d.dot(normal);
            if denom.abs() > 1e-6 {
                let t = (anchor - o).dot(normal) / denom;
                self.sculptor.grab_update(o + d * t - anchor);
                self.redraw = true;
            }
            return;
        }
        let spacing = (self.brush.radius_px * 0.1).max(1.0);
        let mut from = self.last_dab;
        let mut dist = (p - from).length();
        while dist >= spacing {
            let dir = (p - from) / dist;
            from += dir * spacing;
            dist -= spacing;
            self.dab_at(from, size);
        }
        self.last_dab = from;
    }

    /// Where the mirrored brush lands on screen, if symmetry is on and the
    /// cursor is over the mesh.
    pub fn mirror_screen_point(&self, p: Vec2, size: Vec2) -> Option<Vec2> {
        if !self.brush.symmetry {
            return None;
        }
        let (o, d) = self.camera.ray(p, size);
        let h = self.sculptor.raycast(o, d)?;
        let clip = self.camera.proj(size.x / size.y) * self.camera.view() * Vec4::from((mirror_x(h.point), 1.0));
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Vec2 = Vec2::new(800.0, 600.0);

    fn hit_depth(e: &Editor) -> f32 {
        let (o, d) = e.camera.ray(SIZE * 0.5, SIZE);
        e.sculptor.raycast(o, d).unwrap().t
    }

    #[test]
    fn stroke_raises_surface_and_undoes() {
        let mut e = Editor::new();
        e.view_from(ViewDir::Front);
        let before = hit_depth(&e);
        assert_eq!(e.pointer_down(Button::Left, SIZE * 0.5 - Vec2::X * 40.0, SIZE, Mods::default()), Drag::Sculpt);
        for i in 0..=16 {
            e.pointer_move(SIZE * 0.5 + Vec2::X * (i as f32 * 5.0 - 40.0), SIZE);
        }
        e.pointer_up();
        e.sculptor.refresh_bvh();
        assert!(hit_depth(&e) < before - 0.01, "surface should come towards the camera");
        e.undo();
        e.sculptor.refresh_bvh();
        assert!((hit_depth(&e) - before).abs() < 1e-4);
    }

    #[test]
    fn press_off_mesh_orbits() {
        let mut e = Editor::new();
        let yaw = e.camera.yaw;
        assert_eq!(e.pointer_down(Button::Left, Vec2::new(5.0, 5.0), SIZE, Mods::default()), Drag::Orbit);
        e.pointer_move(Vec2::new(55.0, 5.0), SIZE);
        e.pointer_up();
        assert!((e.camera.yaw - yaw).abs() > 0.1);
        assert!(!e.sculptor.can_redo() && !e.sculptor.can_undo());
    }

    #[test]
    fn shift_smooths_and_ctrl_inverts() {
        let e = Editor::new();
        assert_eq!(e.effective_tool(Mods { shift: true, ..Default::default() }), Tool::Smooth);
        assert!(e.effective_negative(Mods { ctrl: true, ..Default::default() }));
    }

    #[test]
    fn mirror_point_is_on_other_side() {
        let mut e = Editor::new();
        e.view_from(ViewDir::Front);
        let p = Vec2::new(SIZE.x * 0.5 + 100.0, SIZE.y * 0.5);
        let m = e.mirror_screen_point(p, SIZE).unwrap();
        assert!((m.x - (SIZE.x * 0.5 - 100.0)).abs() < 3.0, "{m:?}");
    }
}
