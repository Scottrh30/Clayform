//! The editor: sculptor + camera + brush settings, driven by pointer input.
//! Interface-independent so the stroke logic can be unit tested.

use crate::camera::Camera;
use crate::mesh::Mesh;
use crate::remesh::{voxel_remesh, VoxelSettings};
use crate::sculpt::{AnchorMode, DabParams, DragInput, DragKind, Gather, Sculptor, Tool};
use crate::symmetry::{apply_point, Symmetry};
use glam::Mat3;
use glam::{Vec2, Vec3, Vec4};

mod mq;
mod rt;
mod zm;

pub use mq::MannequinDrag;
pub use zm::ZmSettings;

/// What the left mouse button does in the viewport.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Sculpt,
    /// Polygon modelling (ZModeler).
    ZModeler,
    /// Drawing new topology over the sculpt.
    Retopo,
    /// Posing a mannequin.
    Mannequin,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Sculpt, Mode::ZModeler, Mode::Retopo, Mode::Mannequin];
    pub fn name(self) -> &'static str {
        match self {
            Mode::Sculpt => "Sculpt",
            Mode::ZModeler => "ZModeler",
            Mode::Retopo => "Retopology",
            Mode::Mannequin => "Mannequin",
        }
    }
}

/// Lines and points drawn over the viewport (screen space, points).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub lines: Vec<(Vec2, Vec2, OverlayStyle)>,
    pub points: Vec<(Vec2, OverlayStyle)>,
    /// Filled polygons (retopology faces).
    pub polys: Vec<Vec<Vec2>>,
    /// Whether the brush circle makes sense in this mode.
    pub brush: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayStyle {
    /// What a click would act on.
    Target,
    /// The first pick of a two-step action (bridge).
    Pending,
    /// Retopology edges / vertices.
    Mesh,
    /// Retopology face being built.
    Building,
    /// Mannequin joints.
    Joint,
    /// The joint under the cursor or being dragged.
    Active,
    /// A curve being drawn.
    Curve,
}

/// How dynamic topology picks its triangle size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetailMode {
    /// Relative to the brush on screen (zoom in or shrink the brush for finer detail).
    Brush,
    /// A fixed size in the model, whatever the brush size or zoom.
    Constant,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Brush {
    pub tool: Tool,
    pub radius_px: f32,
    pub intensity: [f32; Tool::ALL.len()],
    pub negative: bool,
    pub symmetry: Symmetry,
    /// Dynamic topology on/off (only applies with a single level).
    pub dyntopo: bool,
    /// Dynamic-topology detail, 0 (coarse) to 1 (fine).
    pub detail: f32,
    pub detail_mode: DetailMode,
    /// With dynamic topology, the Smooth brush also lowers triangle density.
    pub smooth_reduces: bool,
    /// What the Anchors brush does.
    pub anchor_mode: AnchorMode,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            tool: Tool::Draw,
            radius_px: 60.0,
            intensity: Tool::ALL.map(|t| t.default_intensity()),
            negative: false,
            symmetry: Symmetry::default(),
            dyntopo: false,
            detail: 0.7,
            detail_mode: DetailMode::Brush,
            smooth_reduces: true,
            anchor_mode: AnchorMode::Move,
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
    /// Size of the model when the stroke started (constant-detail mode).
    stroke_extent: f32,
    /// Centre for local symmetry, measured when a stroke starts.
    sym_center: Vec3,
    /// Set whenever the picture changes; the interface clears it after drawing.
    pub redraw: bool,
    pub mode: Mode,
    pub zm: ZmSettings,
    zms: zm::ZmState,
    pub retopo: crate::retopo::Retopo,
    rts: rt::RtState,
    pub mannequin: crate::mannequin::Mannequin,
    mqs: mq::MqState,
    /// Curve Snakehook: the screen path being drawn.
    curve: Vec<Vec2>,
    /// Dragging brushes: pointer and scale at the stroke start.
    drag_start: Vec2,
    drag_wpp: f32,
    /// Polygroup paint: the group this stroke paints.
    paint_group: u32,
    last_size: Vec2,
    /// A one-line note for the status bar (picks, errors), taken by the interface.
    pub message: Option<String>,
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
            stroke_extent: 2.0,
            sym_center: Vec3::ZERO,
            redraw: true,
            mode: Mode::Sculpt,
            zm: ZmSettings::default(),
            zms: Default::default(),
            retopo: Default::default(),
            rts: Default::default(),
            mannequin: Default::default(),
            mqs: Default::default(),
            curve: Vec::new(),
            drag_start: Vec2::ZERO,
            drag_wpp: 0.01,
            paint_group: 0,
            last_size: Vec2::new(800.0, 600.0),
            message: None,
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

    /// Whether strokes currently change topology.
    pub fn dyntopo_active(&self) -> bool {
        self.brush.dyntopo && self.sculptor.dyntopo_allowed()
    }

    /// Constant detail: edges per longest side of the model, from 20 at
    /// detail 0 to 500 at detail 1.
    pub fn constant_resolution(detail: f32) -> f32 {
        20.0 * 25f32.powf(detail.clamp(0.0, 1.0))
    }

    /// Dynamic-topology edge length for a dab with world radius `r`.
    fn dab_detail(&self, r: f32) -> Option<f32> {
        if !self.dyntopo_active() {
            return None;
        }
        let base = match self.brush.detail_mode {
            DetailMode::Brush => r * Self::detail_ratio(self.brush.detail),
            DetailMode::Constant => self.stroke_extent / Self::constant_resolution(self.brush.detail),
        };
        // smoothing with dynamic topology evens out and coarsens, like Sculptris Pro
        Some(if self.stroke_tool.is_smooth() && self.brush.smooth_reduces { base * 4.0 } else { base })
    }

    /// Target edge length as a fraction of the brush radius: 35 % at
    /// detail 0 down to 3 % at detail 1 (exponential, so the slider feels even).
    pub fn detail_ratio(detail: f32) -> f32 {
        0.35 * (0.03f32 / 0.35).powf(detail.clamp(0.0, 1.0))
    }

    pub fn set_level(&mut self, level: usize) {
        self.cancel_drag();
        self.sculptor.set_level(level);
        self.redraw = true;
    }

    pub fn subdivide(&mut self) -> Result<(), String> {
        self.cancel_drag();
        let r = self.sculptor.subdivide();
        self.redraw = true;
        r
    }

    pub fn delete_lower(&mut self) {
        self.cancel_drag();
        self.sculptor.delete_lower();
        self.redraw = true;
    }

    pub fn delete_higher(&mut self) {
        self.cancel_drag();
        self.sculptor.delete_higher();
        self.redraw = true;
    }

    /// DynaMesh-style voxel remesh of the current shape (undoable). The
    /// result is a single level. Returns the new triangle count.
    pub fn voxel_remesh(&mut self, settings: &VoxelSettings) -> Result<usize, String> {
        self.cancel_drag();
        let m = voxel_remesh(&self.sculptor.mesh, settings)?;
        let n = m.tris.len();
        self.sculptor.set_label("Voxel remesh");
        self.sculptor.set_mesh(m);
        self.redraw = true;
        Ok(n)
    }

    /// Replaces the sculpt with a remeshed version (undoable, keeps the view).
    pub fn apply_remeshed(&mut self, m: Mesh, bvh: Option<crate::bvh::Bvh>) {
        self.cancel_drag();
        self.sculptor.set_label_if_none("Voxel remesh");
        self.sculptor.set_mesh_built(m, bvh);
        self.redraw = true;
    }

    pub fn begin_subdivide(&mut self) -> Result<(), String> {
        self.cancel_drag();
        let r = self.sculptor.begin_subdivide();
        self.redraw = true;
        r
    }

    pub fn finish_subdivide(
        &mut self,
        fine: Mesh,
        odd: Vec<[u32; 4]>,
        bvh: Option<crate::bvh::Bvh>,
    ) -> Result<(), String> {
        let r = self.sculptor.finish_subdivide(fine, odd, bvh);
        self.redraw = true;
        r
    }

    pub fn abort_subdivide(&mut self) {
        self.sculptor.abort_subdivide();
        self.redraw = true;
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

    /// Undoes or redoes until `done` steps of the history are applied
    /// (0 = the oldest state still remembered).
    pub fn goto_history(&mut self, done: usize) {
        self.cancel_drag();
        loop {
            let (u, _) = self.sculptor.history();
            if u.len() > done && self.sculptor.can_undo() {
                self.sculptor.undo();
            } else if u.len() < done && self.sculptor.can_redo() {
                self.sculptor.redo();
            } else {
                break;
            }
        }
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

    pub fn cancel_drag(&mut self) {
        if self.drag == Drag::Sculpt {
            self.sculptor.end_stroke();
        }
        self.grab = None;
        self.curve.clear();
        self.zms.drag = None;
        self.rts.drag = None;
        self.mqs.drag = None;
        self.drag = Drag::None;
    }

    /// Switches what the left button does.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.cancel_drag();
        self.zms.bridge = None;
        self.retopo.pending.clear();
        if mode == Mode::Mannequin {
            self.enter_mannequin();
        }
        self.mode = mode;
        self.redraw = true;
    }

    /// Projects a world point to the viewport (None behind the camera).
    pub fn to_screen(&self, q: Vec3, size: Vec2) -> Option<Vec2> {
        let vp = self.camera.proj(size.x / size.y) * self.camera.view();
        let clip = vp * Vec4::from((q, 1.0));
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y))
    }

    /// Whether a point on (or near) the surface can be seen from the camera.
    pub fn visible(&self, q: Vec3) -> bool {
        let eye = self.camera.eye();
        let d = q - eye;
        let len = d.length();
        if len < 1e-6 {
            return true;
        }
        match self.sculptor.raycast(eye, d / len) {
            Some(h) => h.t >= len - len * 0.01 - 1e-4,
            None => true,
        }
    }

    /// What to draw over the viewport for the cursor at `p`.
    pub fn overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        match self.mode {
            Mode::Sculpt => {
                let mut o = Overlay { brush: true, ..Default::default() };
                for w in self.curve.windows(2) {
                    o.lines.push((w[0], w[1], OverlayStyle::Curve));
                }
                o
            }
            Mode::ZModeler => self.zm_overlay(p, size),
            Mode::Retopo => self.rt_overlay(p, size),
            Mode::Mannequin => self.mq_overlay(p, size),
        }
    }

    // ------------------------------------------------------------------ input

    /// `p` and `size` are in CSS pixels / points, origin top-left of the viewport.
    pub fn pointer_down(&mut self, button: Button, p: Vec2, size: Vec2, mods: Mods) -> Drag {
        self.cancel_drag();
        self.last_size = size;
        self.sculptor.refresh_bvh();
        self.last_pos = p;
        self.drag = match button {
            Button::Left if mods.alt => Drag::Orbit,
            Button::Left if self.mode != Mode::Sculpt => {
                let handled = match self.mode {
                    Mode::ZModeler => self.zm_down(p, size, mods),
                    Mode::Retopo => self.rt_down(p, size, mods),
                    _ => self.mq_down(p, size, mods),
                };
                match handled {
                    Some(d) => d,
                    None if mods.shift => Drag::Pan,
                    None => Drag::Orbit,
                }
            }
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
        self.last_size = size;
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
            Drag::Sculpt => match self.mode {
                Mode::Sculpt => {
                    self.sculptor.refresh_bvh();
                    self.continue_sculpt(p, size);
                }
                Mode::ZModeler => self.zm_move(p, size),
                Mode::Retopo => self.rt_move(p, size),
                Mode::Mannequin => self.mq_move(p, size),
            },
            Drag::None => {}
        }
    }

    pub fn pointer_up(&mut self) {
        match self.mode {
            Mode::Sculpt if self.drag == Drag::Sculpt && self.stroke_tool == Tool::CurveSnakehook => {
                self.finish_curve();
            }
            Mode::ZModeler if self.drag == Drag::Sculpt => self.zm_up(),
            Mode::Retopo if self.drag == Drag::Sculpt => self.rt_up(),
            Mode::Mannequin if self.drag == Drag::Sculpt => self.mq_up(),
            _ => {}
        }
        self.cancel_drag();
    }

    pub fn wheel(&mut self, delta_y: f32) {
        if self.drag != Drag::Sculpt {
            self.camera.zoom(delta_y);
            self.redraw = true;
        }
    }

    /// Centre the symmetry copies are made about.
    pub fn symmetry_center(&self) -> Vec3 {
        if self.brush.symmetry.local { self.sym_center } else { Vec3::ZERO }
    }

    /// Re-measures the model centre used by local symmetry.
    pub fn update_symmetry_center(&mut self) {
        let (mn, mx) = self.sculptor.mesh.bounds();
        self.sym_center = (mn + mx) * 0.5;
    }

    fn dab_at(&mut self, p: Vec2, size: Vec2, screen_motion: Vec2) {
        self.last_size = size;
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return };
        let wpp = self.camera.world_per_point(h.point, size.y);
        let r = self.brush.radius_px * wpp;
        // screen movement since the last dab, as a world-space vector in the view plane
        let f = self.camera.forward();
        let right = f.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(f);
        let motion = (right * screen_motion.x - up * screen_motion.y) * wpp;
        let mut params = DabParams {
            tool: self.stroke_tool,
            center: h.point,
            radius: r,
            intensity: self.brush.intensity[self.stroke_tool.index()],
            negative: self.stroke_negative,
            seed_tri: h.tri,
            detail: self.dab_detail(r),
            motion,
            view: f,
            group: self.paint_group,
        };
        self.sculptor.dab(&params);
        let c = self.symmetry_center();
        for m in self.brush.symmetry.copies() {
            if let Some(hm) = self.sculptor.raycast(apply_point(m, c, o), m * d) {
                params.center = hm.point;
                params.seed_tri = hm.tri;
                params.motion = m * motion;
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
        if self.dyntopo_active() && self.brush.detail_mode == DetailMode::Constant {
            let (mn, mx) = self.sculptor.mesh.bounds();
            self.stroke_extent = (mx - mn).max_element().max(1e-6);
        }
        if self.brush.symmetry.local {
            self.update_symmetry_center();
        }
        if self.stroke_tool == Tool::GroupPaint {
            self.paint_group = self.sculptor.mesh.groups.iter().copied().max().map_or(0, |g| g + 1);
        }
        self.sculptor.begin_stroke(self.dyntopo_active() && !self.stroke_tool.is_drag());
        self.sculptor.label_stroke(self.stroke_tool.name());
        if self.stroke_tool.is_drag() {
            let wpp = self.camera.world_per_point(h.point, size.y);
            let r = self.brush.radius_px * wpp;
            let view = self.camera.forward();
            let gather = match self.stroke_tool {
                Tool::Grab | Tool::ClothPull | Tool::ClothTwist | Tool::Anchors => Gather::Topological,
                Tool::MoveElastic => Gather::Elastic,
                Tool::MoveInfiniteDepth => Gather::Depth(view),
                _ => Gather::Spatial,
            };
            let kind = match self.stroke_tool {
                Tool::Anchors => DragKind::Anchor(self.brush.anchor_mode),
                Tool::ClothPull => DragKind::ClothPull,
                Tool::ClothTwist => DragKind::ClothTwist,
                _ => DragKind::Move,
            };
            self.sculptor.drag_begin(h.point, r, h.tri, Mat3::IDENTITY, gather, kind);
            let c = self.symmetry_center();
            for m in self.brush.symmetry.copies() {
                let (om, dm) = (apply_point(m, c, o), m * d);
                if let Some(hm) = self.sculptor.raycast(om, dm) {
                    let gm = match gather {
                        Gather::Depth(v) => Gather::Depth(m * v),
                        g => g,
                    };
                    self.sculptor.drag_begin(hm.point, r, hm.tri, m, gm, kind);
                }
            }
            self.grab = Some((h.point, view));
            self.drag_start = p;
            self.drag_wpp = wpp;
            self.drag = Drag::Sculpt;
        } else if self.stroke_tool == Tool::CurveSnakehook {
            // draw the path first; the horn is pulled on release
            self.curve = vec![p];
            self.drag = Drag::Sculpt;
        } else {
            // the drag state must be Sculpt while the first dab is applied
            self.drag = Drag::Sculpt;
            self.dab_at(p, size, Vec2::ZERO);
            self.last_dab = p;
        }
        true
    }

    fn continue_sculpt(&mut self, p: Vec2, size: Vec2) {
        if self.stroke_tool.is_drag() {
            let Some((anchor, normal)) = self.grab else { return };
            let (o, d) = self.camera.ray(p, size);
            let denom = d.dot(normal);
            if denom.abs() > 1e-6 {
                let t = (anchor - o).dot(normal) / denom;
                self.sculptor.drag_update(DragInput {
                    delta: o + d * t - anchor,
                    screen: p - self.drag_start,
                    view: normal,
                    wpp: self.drag_wpp,
                });
                self.redraw = true;
            }
            return;
        }
        if self.stroke_tool == Tool::CurveSnakehook {
            if self.curve.last().is_none_or(|q| q.distance(p) > 3.0) {
                self.curve.push(p);
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
            self.dab_at(from, size, dir * spacing);
        }
        self.last_dab = from;
    }

    /// Curve Snakehook: pulls the surface at the curve's start along the
    /// drawn path, the brush shrinking towards the tip.
    fn finish_curve(&mut self) {
        let path = std::mem::take(&mut self.curve);
        if path.len() < 2 {
            return;
        }
        let size = self.last_size;
        let full = self.brush.radius_px;
        // resample evenly so the taper follows length, not pointer speed
        let total: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
        if total < 2.0 {
            return;
        }
        let step = (full * 0.08).max(1.5);
        let n = (total / step).ceil() as usize;
        let at = |s: f32| -> Vec2 {
            let mut left = s;
            for w in path.windows(2) {
                let l = w[0].distance(w[1]);
                if left <= l {
                    return w[0].lerp(w[1], if l > 0.0 { left / l } else { 0.0 });
                }
                left -= l;
            }
            *path.last().unwrap()
        };
        self.sculptor.refresh_bvh();
        // the horn starts where the curve starts on the surface; each step
        // drags the brush (on the tip) forward along the path, in the view plane
        let (o, d) = self.camera.ray(path[0], size);
        let Some(h0) = self.sculptor.raycast(o, d) else { return };
        let wpp = self.camera.world_per_point(h0.point, size.y);
        let f = self.camera.forward();
        let right = f.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(f);
        let mut tip = h0.point;
        let mut tri = h0.tri;
        let mut mirror_tips: Vec<(Mat3, Vec3, u32)> = Vec::new();
        let c = self.symmetry_center();
        for m in self.brush.symmetry.copies() {
            if let Some(hm) = self.sculptor.raycast(apply_point(m, c, o), m * d) {
                mirror_tips.push((m, hm.point, hm.tri));
            }
        }
        self.stroke_tool = Tool::CurveSnakehook;
        self.sculptor.begin_stroke(self.dyntopo_active());
        self.sculptor.label_stroke("Curve Snakehook");
        let mut prev = path[0];
        for k in 1..=n {
            let s = total * k as f32 / n as f32;
            let q = at(s);
            let sm = q - prev;
            prev = q;
            let motion = (right * sm.x - up * sm.y) * wpp;
            let taper = 1.0 - 0.75 * (s / total);
            let r = full * taper * wpp;
            let mut params = DabParams {
                tool: Tool::CurveSnakehook,
                center: tip,
                radius: r,
                intensity: self.brush.intensity[Tool::CurveSnakehook.index()],
                negative: false,
                seed_tri: tri,
                detail: self.dab_detail(r),
                motion,
                view: f,
                group: 0,
            };
            self.sculptor.dab(&params);
            tip += motion * params.intensity;
            for (m, mt, mtri) in mirror_tips.iter_mut() {
                params.center = *mt;
                params.seed_tri = *mtri;
                params.motion = *m * motion;
                self.sculptor.dab(&params);
                *mt += params.motion * params.intensity;
            }
            // keep the seed face live and near the tip
            self.sculptor.refresh_bvh();
            if let Some(nt) = self.sculptor.nearest_face(tip) {
                tri = nt;
            }
            for (_, mt, mtri) in mirror_tips.iter_mut() {
                if let Some(nt) = self.sculptor.nearest_face(*mt) {
                    *mtri = nt;
                }
            }
        }
        self.sculptor.end_stroke();
        self.redraw = true;
    }

    /// Where each symmetric copy of the brush lands on screen (cursor over
    /// the mesh, copies facing the camera only).
    pub fn mirror_screen_points(&self, p: Vec2, size: Vec2) -> Vec<Vec2> {
        let copies = self.brush.symmetry.copies();
        if copies.is_empty() {
            return Vec::new();
        }
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return Vec::new() };
        let n = self.sculptor.mesh.face_normal_raw(h.tri as usize);
        let c = self.symmetry_center();
        let vp = self.camera.proj(size.x / size.y) * self.camera.view();
        let eye = self.camera.eye();
        copies
            .into_iter()
            .filter_map(|m| {
                let q = apply_point(m, c, h.point);
                if (m * n).dot(eye - q) <= 0.0 {
                    return None; // on the far side of the model
                }
                let clip = vp * Vec4::from((q, 1.0));
                if clip.w <= 0.0 {
                    return None;
                }
                let ndc = clip.truncate() / clip.w;
                Some(Vec2::new((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y))
            })
            .collect()
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
    fn dynamic_topology_stroke_refines_mesh() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(3));
        e.view_from(ViewDir::Front);
        e.brush.dyntopo = true;
        e.brush.detail = 1.0;
        let n0 = e.sculptor.tri_count();
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, Mods::default());
        for i in 1..=10 {
            e.pointer_move(SIZE * 0.5 + Vec2::X * (i as f32 * 6.0), SIZE);
        }
        e.pointer_up();
        assert!(e.sculptor.tri_count() > n0 + 200, "{n0} -> {}", e.sculptor.tri_count());
    }

    #[test]
    fn voxel_remesh_is_undoable_and_keeps_view() {
        let mut e = Editor::new();
        e.sculptor.subdivide().unwrap();
        let (yaw, dist) = (e.camera.yaw, e.camera.dist);
        let n0 = e.sculptor.tri_count();
        let n = e.voxel_remesh(&VoxelSettings { resolution: 48, smooth: 1, project: true, signed: false }).unwrap();
        assert_eq!(e.sculptor.tri_count(), n);
        assert_eq!(e.sculptor.level_count(), 1);
        assert_eq!((e.camera.yaw, e.camera.dist), (yaw, dist));
        e.undo();
        assert_eq!(e.sculptor.tri_count(), n0);
        assert_eq!(e.sculptor.level_count(), 2);
    }

    fn stroke(e: &mut Editor, mods: Mods) {
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, mods);
        for i in 1..=10 {
            e.pointer_move(SIZE * 0.5 + Vec2::X * (i as f32 * 6.0), SIZE);
        }
        e.pointer_up();
    }

    fn mean_edge(m: &Mesh) -> f32 {
        let (mut s, mut n) = (0.0, 0);
        for t in &m.tris {
            for k in 0..3 {
                s += m.positions[t[k] as usize].distance(m.positions[t[(k + 1) % 3] as usize]);
                n += 1;
            }
        }
        s / n as f32
    }

    #[test]
    fn constant_detail_ignores_brush_size() {
        let tri_counts: Vec<usize> = [30.0, 90.0]
            .iter()
            .map(|&radius| {
                let mut e = Editor::new();
                e.set_mesh(Mesh::icosphere(2));
                e.view_from(ViewDir::Front);
                e.brush.dyntopo = true;
                e.brush.detail_mode = DetailMode::Constant;
                e.brush.detail = 0.5;
                e.brush.radius_px = radius;
                e.brush.intensity = [0.0; Tool::ALL.len()];
                stroke(&mut e, Mods::default());
                // edges near the stroke should approach extent / resolution
                let target = 2.0 / Editor::constant_resolution(0.5);
                let m = &e.sculptor.mesh;
                let near: Vec<f32> = m
                    .tris
                    .iter()
                    .filter(|t| m.positions[t[0] as usize].z > 0.97)
                    .map(|t| m.positions[t[0] as usize].distance(m.positions[t[1] as usize]))
                    .collect();
                let avg = near.iter().sum::<f32>() / near.len() as f32;
                assert!(avg < target * 1.6, "radius {radius}: {avg} vs {target}");
                e.sculptor.tri_count()
            })
            .collect();
        assert!(tri_counts[1] > tri_counts[0], "bigger brush covers more area at the same size");
    }

    #[test]
    fn smooth_with_dyntopo_reduces_triangles() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(5));
        e.view_from(ViewDir::Front);
        e.brush.dyntopo = true;
        e.brush.detail = 0.6;
        let n0 = e.sculptor.tri_count();
        let edge0 = mean_edge(&e.sculptor.mesh);
        e.brush.tool = Tool::Smooth;
        stroke(&mut e, Mods::default());
        assert!(e.sculptor.tri_count() < n0, "{n0} -> {}", e.sculptor.tri_count());
        assert!(mean_edge(&e.sculptor.mesh) > edge0);
        // and with the option off it doesn't coarsen
        let mut e2 = Editor::new();
        e2.set_mesh(Mesh::icosphere(5));
        e2.view_from(ViewDir::Front);
        e2.brush.dyntopo = true;
        e2.brush.detail = 0.6;
        e2.brush.smooth_reduces = false;
        e2.brush.tool = Tool::Smooth;
        stroke(&mut e2, Mods::default());
        assert!(e2.sculptor.tri_count() > e.sculptor.tri_count());
    }

    fn raised(e: &Editor, dir: Vec3) -> bool {
        let (o, d) = (dir * 5.0, -dir);
        e.sculptor.raycast(o, d).is_some_and(|h| h.point.length() > 1.01)
    }

    fn dab_front(e: &mut Editor) {
        e.brush.intensity = [1.0; Tool::ALL.len()];
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, Mods::default());
        for i in 0..6 {
            e.pointer_move(SIZE * 0.5 + Vec2::new(0.0, if i % 2 == 0 { 12.0 } else { 0.0 }), SIZE);
        }
        e.pointer_up();
        e.sculptor.refresh_bvh();
    }

    #[test]
    fn radial_symmetry_makes_copies_around_the_axis() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.brush.symmetry = Symmetry { mirror: [false; 3], radial: 4, radial_axis: 1, local: false };
        dab_front(&mut e);
        for dir in [Vec3::Z, Vec3::X, Vec3::NEG_Z, Vec3::NEG_X] {
            assert!(raised(&e, dir), "no copy facing {dir:?}");
        }
        assert!(!raised(&e, Vec3::Y));
    }

    #[test]
    fn mirror_y_and_local_centre() {
        let mut e = Editor::new();
        let mut m = Mesh::icosphere(4);
        for p in &mut m.positions {
            p.y += 0.5; // model centre is (0, 0.5, 0)
        }
        m.compute_all_normals();
        e.set_mesh(m);
        e.view_from(ViewDir::Front);
        e.brush.symmetry = Symmetry { mirror: [false, true, false], radial: 1, radial_axis: 1, local: true };
        // stroke on the upper front; the copy should land on the lower front
        e.brush.intensity = [1.0; Tool::ALL.len()];
        let d = Vec3::NEG_Z;
        let before_low = e.sculptor.raycast(Vec3::new(0.0, 0.0, 5.0), d).unwrap().point.z;
        let target = Vec3::new(0.0, 1.0, 0.87);
        let clip = e.camera.proj(SIZE.x / SIZE.y) * e.camera.view() * Vec4::from((target, 1.0));
        let ndc = clip.truncate() / clip.w;
        let sp = Vec2::new((ndc.x * 0.5 + 0.5) * SIZE.x, (0.5 - ndc.y * 0.5) * SIZE.y);
        e.pointer_down(Button::Left, sp, SIZE, Mods::default());
        e.pointer_move(sp + Vec2::new(10.0, 0.0), SIZE);
        e.pointer_up();
        e.sculptor.refresh_bvh();
        let after_low = e.sculptor.raycast(Vec3::new(0.0, 0.0, 5.0), d).unwrap().point.z;
        assert!(after_low > before_low + 0.01, "{before_low} -> {after_low}");
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
        let m = e.mirror_screen_points(p, SIZE)[0];
        assert!((m.x - (SIZE.x * 0.5 - 100.0)).abs() < 3.0, "{m:?}");
    }

    fn front_z(e: &Editor) -> f32 {
        e.sculptor.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap().point.z
    }

    fn drag(e: &mut Editor, from: Vec2, to: Vec2) {
        e.pointer_down(Button::Left, from, SIZE, Mods::default());
        for i in 1..=10 {
            e.pointer_move(from.lerp(to, i as f32 / 10.0), SIZE);
        }
        e.pointer_up();
        e.sculptor.refresh_bvh();
    }

    #[test]
    fn history_names_steps_and_jumps() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(3));
        e.view_from(ViewDir::Front);
        e.brush.tool = Tool::Clay;
        stroke(&mut e, Mods::default());
        e.brush.tool = Tool::Inflate;
        stroke(&mut e, Mods::default());
        e.sculptor.clear_mask();
        let (done, undone) = e.sculptor.history();
        assert_eq!(done[done.len() - 3..], ["Clay", "Inflate", "Clear mask"]);
        assert!(undone.is_empty());
        let after_clay = done.len() - 2;
        e.goto_history(after_clay);
        let (done, undone) = e.sculptor.history();
        assert_eq!(done.last().unwrap(), "Clay");
        assert_eq!(undone, ["Inflate", "Clear mask"]);
        e.goto_history(after_clay + 2);
        assert_eq!(e.sculptor.history().0.last().unwrap(), "Clear mask");
    }

    #[test]
    fn dragging_brushes_move_and_undo() {
        for tool in [Tool::Move, Tool::MoveElastic, Tool::MoveInfiniteDepth, Tool::ClothPull, Tool::Anchors] {
            let mut e = Editor::new();
            e.set_mesh(Mesh::icosphere(4));
            e.view_from(ViewDir::Front);
            e.brush.tool = tool;
            e.brush.symmetry.mirror = [false; 3];
            e.brush.anchor_mode = AnchorMode::Move;
            let before = e.sculptor.mesh.positions.clone();
            drag(&mut e, SIZE * 0.5, SIZE * 0.5 + Vec2::new(80.0, 0.0));
            let moved = e.sculptor.mesh.positions.iter().zip(&before).filter(|(a, b)| a.distance(**b) > 1e-3).count();
            assert!(moved > 10, "{tool:?} moved {moved}");
            assert!(e.sculptor.mesh.positions.iter().all(|p| p.is_finite()));
            if tool == Tool::MoveInfiniteDepth {
                // the back of the sphere moves too
                let back = e.sculptor.mesh.positions.iter().zip(&before).filter(|(a, b)| b.z < -0.9 && a.x > b.x + 0.05).count();
                assert!(back > 3, "{back}");
            }
            e.undo();
            assert_eq!(e.sculptor.mesh.positions, before);
        }
    }

    #[test]
    fn anchors_scale_and_twist() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.brush.tool = Tool::Anchors;
        e.brush.anchor_mode = AnchorMode::Inflate;
        let z0 = front_z(&e);
        drag(&mut e, SIZE * 0.5, SIZE * 0.5 - Vec2::new(0.0, 60.0));
        assert!(front_z(&e) > z0 + 0.05);
        e.brush.anchor_mode = AnchorMode::Twist;
        drag(&mut e, SIZE * 0.5, SIZE * 0.5 + Vec2::new(60.0, 0.0));
        assert!(e.sculptor.mesh.positions.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn curve_snakehook_pulls_a_horn() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.brush.tool = Tool::CurveSnakehook;
        e.brush.symmetry.mirror = [false; 3];
        let n0 = e.sculptor.tri_count();
        let before = e.sculptor.mesh.positions.clone();
        // drawing alone changes nothing
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, Mods::default());
        for i in 1..=20 {
            e.pointer_move(SIZE * 0.5 + Vec2::new(i as f32 * 8.0, -(i as f32) * 2.0), SIZE);
        }
        assert_eq!(e.sculptor.mesh.positions, before);
        assert!(e.overlay(None, SIZE).lines.len() > 10);
        e.pointer_up();
        let pulled = e.sculptor.mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0f32, f32::max);
        assert!(pulled > 0.5, "horn pulled {pulled}");
        // the tip follows the curve to the right and up
        let tip = e.sculptor.mesh.positions.iter().zip(&before).max_by(|x, y| x.0.distance(*x.1).total_cmp(&y.0.distance(*y.1))).unwrap().0;
        assert!(tip.x > 0.4 && tip.y > 0.05, "{tip:?}");
        assert_eq!(e.sculptor.tri_count(), n0);
        e.undo();
        assert_eq!(e.sculptor.mesh.positions, before);
    }

    #[test]
    fn polygroup_paint_and_undo() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.brush.tool = Tool::GroupPaint;
        stroke(&mut e, Mods::default());
        let painted = e.sculptor.mesh.groups.iter().filter(|&&g| g == 1).count();
        assert!(painted > 20, "{painted}");
        e.undo();
        assert!(e.sculptor.mesh.groups.iter().all(|&g| g == 0));
    }

    #[test]
    fn zmodeler_extrude_by_dragging_is_one_undo_step() {
        let mut e = Editor::new();
        e.new_primitive(crate::poly::Primitive::QCube);
        e.view_from(ViewDir::Front);
        e.set_mode(Mode::ZModeler);
        e.brush.symmetry.mirror = [false; 3];
        e.zm.action = crate::poly::Action::Extrude;
        let n0 = e.sculptor.tri_count();
        let z0 = front_z(&e);
        assert!(!e.overlay(Some(SIZE * 0.5), SIZE).lines.is_empty(), "hover shows the target");
        drag(&mut e, SIZE * 0.5, SIZE * 0.5 - Vec2::new(0.0, 80.0));
        assert!(front_z(&e) > z0 + 0.1);
        assert_eq!(e.sculptor.tri_count(), n0 + 8, "four side quads");
        e.undo();
        assert_eq!(e.sculptor.tri_count(), n0);
        // click actions: delete a polygon, then polygroup one
        e.zm.action = crate::poly::Action::Delete;
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, Mods::default());
        e.pointer_up();
        assert_eq!(e.sculptor.tri_count(), n0 - 2);
        e.undo();
        e.zm.action = crate::poly::Action::Polygroup;
        e.pointer_down(Button::Left, SIZE * 0.5, SIZE, Mods::default());
        e.pointer_up();
        let g = e.sculptor.mesh.groups.iter().copied().max().unwrap();
        assert_eq!(e.sculptor.mesh.groups.iter().filter(|&&x| x == g).count(), 2);
    }

    #[test]
    fn zmodeler_mirror_extrudes_both_sides() {
        let mut e = Editor::new();
        e.zm.divisions = 2;
        e.new_primitive(crate::poly::Primitive::QCube);
        e.view_from(ViewDir::Front);
        e.set_mode(Mode::ZModeler);
        e.brush.symmetry.mirror[0] = true;
        let n0 = e.sculptor.tri_count();
        // a front quad off the centre line
        let target = Vec3::new(0.4, 0.4, 0.8);
        let sp = e.to_screen(target, SIZE).unwrap();
        drag(&mut e, sp, sp - Vec2::new(0.0, 60.0));
        // the two quads touch at the centre line, so they extrude as one block
        assert_eq!(e.sculptor.tri_count(), n0 + 12);
        e.undo();
        // apart, each gets its own walls
        e.zm.divisions = 4;
        e.new_primitive(crate::poly::Primitive::QCube);
        let n0 = e.sculptor.tri_count();
        let sp = e.to_screen(Vec3::new(0.5, 0.1, 0.8), SIZE).unwrap();
        drag(&mut e, sp, sp - Vec2::new(0.0, 60.0));
        assert_eq!(e.sculptor.tri_count(), n0 + 16);
    }

    #[test]
    fn retopo_clicks_build_a_face_and_undo() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.set_mode(Mode::Retopo);
        for p in [Vec2::new(-60.0, -60.0), Vec2::new(60.0, -60.0), Vec2::new(60.0, 60.0), Vec2::new(-60.0, 60.0)] {
            e.pointer_down(Button::Left, SIZE * 0.5 + p, SIZE, Mods::default());
            e.pointer_up();
        }
        assert_eq!(e.retopo.faces.len(), 1);
        assert_eq!(e.overlay(None, SIZE).polys.len(), 1);
        // dragging a vertex moves it along the surface
        let v0 = e.retopo.verts[0];
        drag(&mut e, SIZE * 0.5 + Vec2::new(-60.0, -60.0), SIZE * 0.5 + Vec2::new(-90.0, -60.0));
        assert!(e.retopo.verts[0].x < v0.x - 0.05);
        assert!(e.retopo_undo());
        assert_eq!(e.retopo.verts[0], v0);
        assert!(e.retopo_undo());
        assert!(e.retopo.faces.is_empty());
    }

    #[test]
    fn retopo_quad_patch_projects_onto_the_sphere() {
        let mut e = Editor::new();
        e.set_mesh(Mesh::icosphere(4));
        e.view_from(ViewDir::Front);
        e.set_mode(Mode::Retopo);
        for p in [Vec2::new(-60.0, -60.0), Vec2::new(60.0, -60.0), Vec2::new(60.0, 60.0), Vec2::new(-60.0, 60.0)] {
            e.pointer_down(Button::Left, SIZE * 0.5 + p, SIZE, Mods::default());
            e.pointer_up();
        }
        e.retopo_apply(2).unwrap();
        assert_eq!(e.mode, Mode::Sculpt);
        assert_eq!(e.sculptor.level_count(), 3);
        for p in &e.sculptor.mesh.positions {
            assert!((p.length() - 1.0).abs() < 0.01, "projected onto the sphere: {}", p.length());
        }
    }

    #[test]
    fn mannequin_mode_poses_by_dragging_joints() {
        let mut e = Editor::new();
        e.set_mode(Mode::Mannequin);
        assert!(e.mannequin_shown());
        e.view_from(ViewDir::Front);
        let o = e.overlay(None, SIZE);
        assert!(o.points.len() > 15);
        let hand = e.mannequin.find("L hand").unwrap();
        let hp = e.mannequin.world()[hand].0;
        let sp = e.to_screen(hp, SIZE).unwrap();
        drag(&mut e, sp, sp - Vec2::new(0.0, 150.0));
        let moved = e.mannequin.world()[hand].0;
        assert!(moved.y > hp.y + 0.1, "{hp:?} -> {moved:?}");
        assert!(e.mannequin_shown());
        e.mannequin_preset("Sit");
        assert!(e.mannequin_shown());
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// `cargo test --release -p clayform-core bench_stroke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_stroke() {
        for level in [7u32, 8] {
            let mut e = Editor::new();
            e.set_mesh(Mesh::icosphere(level));
            e.view_from(ViewDir::Front);
            e.brush.symmetry.mirror = [false; 3];
            let size = Vec2::new(1600.0, 1000.0);
            let n = e.sculptor.tri_count();
            let t0 = Instant::now();
            e.pointer_down(Button::Left, size * 0.5 - Vec2::X * 200.0, size, Mods::default());
            let mut refit = 0.0;
            let mut gpu_verts = 0usize;
            for i in 0..60 {
                e.pointer_move(size * 0.5 + Vec2::new(i as f32 * 6.0 - 200.0, (i as f32 * 0.3).sin() * 40.0), size);
                // what the frame loop does each frame
                let t = Instant::now();
                e.sculptor.refresh_bvh();
                refit += t.elapsed().as_secs_f64();
                gpu_verts += e.sculptor.dirty.take_blocks_vertices();
            }
            e.pointer_up();
            let total = t0.elapsed().as_secs_f64();
            println!(
                "{n} tris: stroke {:.1} ms/move (refit {:.1} ms/move), GPU re-upload {:.0} verts/move",
                total * 1000.0 / 60.0,
                refit * 1000.0 / 60.0,
                gpu_verts as f64 / 60.0
            );
        }
    }
}
