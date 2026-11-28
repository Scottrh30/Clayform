//! Mannequin mode: drag the joints to pose the figure.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use glam::{Vec2, Vec3};

const PICK_PX: f32 = 12.0;

#[derive(Default)]
pub(super) struct MqState {
    pub(super) drag: Option<MannequinDrag>,
    /// Sculpt revision showing the current pose (None: not on screen).
    shown: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct MannequinDrag {
    pub joint: usize,
    plane_point: Vec3,
    normal: Vec3,
}

impl Editor {
    /// Whether the sculpt on screen is the mannequin being posed.
    pub fn mannequin_shown(&self) -> bool {
        self.mqs.shown == Some(self.sculptor.revision())
    }

    pub(super) fn enter_mannequin(&mut self) {
        if !self.mannequin_shown() {
            self.sculptor.set_label("New mannequin");
            self.sculptor.set_mesh(self.mannequin.mesh());
            self.sculptor.extra.pose = self.mannequin.to_pose();
            self.frame_mesh();
            self.mqs.shown = Some(self.sculptor.revision());
        }
    }

    /// Rebuilds the figure after a pose change (the undo step is made
    /// before the change, by whatever changes the pose).
    pub fn mannequin_changed(&mut self) {
        if self.mannequin_shown() {
            self.sculptor.replace_mesh_preview(self.mannequin.mesh());
        } else {
            self.sculptor.set_label("New mannequin");
            self.sculptor.set_mesh(self.mannequin.mesh());
            self.frame_mesh();
        }
        self.sculptor.extra.pose = self.mannequin.to_pose();
        self.mqs.shown = Some(self.sculptor.revision());
        if self.sculptor.pose_rev.is_some() {
            self.sculptor.pose_rev = Some(self.sculptor.revision());
        }
        self.redraw = true;
    }

    pub fn mannequin_preset(&mut self, preset: &str) {
        if self.mannequin_shown() {
            self.sculptor.push_pose_undo(preset, false);
            self.sculptor.pose_rev = None;
            self.mqs.shown = Some(self.sculptor.revision());
        }
        self.mannequin.apply_preset(preset);
        self.mannequin_changed();
    }

    /// Changes the mannequin's build (one undo step per slider drag).
    pub fn mannequin_bulk(&mut self, bulk: f32) {
        self.mannequin.bulk = bulk;
        if self.mannequin_shown() {
            self.sculptor.push_pose_undo("Mannequin build", true);
            self.mqs.shown = Some(self.sculptor.revision());
            self.mannequin_changed();
        }
    }

    /// After undo / redo: if the sculpt is a mannequin, take its pose back.
    pub(super) fn sync_pose(&mut self) {
        let pose = self.sculptor.extra.pose.clone();
        if !pose.is_empty() && self.mannequin.load_pose(&pose) {
            self.mqs.shown = Some(self.sculptor.revision());
        }
    }

    fn mq_joint_at(&self, p: Vec2, size: Vec2) -> Option<usize> {
        let w = self.mannequin.world();
        let mut best: Option<(f32, usize)> = None;
        for (i, (q, _)) in w.iter().enumerate() {
            if self.mannequin.joints[i].parent.is_none() {
                continue;
            }
            let Some(s) = self.to_screen(*q, size) else { continue };
            let d = s.distance(p);
            if d <= PICK_PX && best.is_none_or(|b| d < b.0) {
                best = Some((d, i));
            }
        }
        best.map(|b| b.1)
    }

    pub(super) fn mq_down(&mut self, p: Vec2, size: Vec2, _mods: Mods) -> Option<Drag> {
        if !self.mannequin_shown() {
            return None;
        }
        let joint = self.mq_joint_at(p, size)?;
        let w = self.mannequin.world();
        self.sculptor.push_pose_undo("Pose", false);
        self.sculptor.pose_rev = None;
        self.mqs.shown = Some(self.sculptor.revision());
        self.mqs.drag = Some(MannequinDrag { joint, plane_point: w[joint].0, normal: self.camera.forward() });
        Some(Drag::Sculpt)
    }

    pub(super) fn mq_move(&mut self, p: Vec2, size: Vec2) {
        let Some(dr) = self.mqs.drag else { return };
        let (o, d) = self.camera.ray(p, size);
        let denom = d.dot(dr.normal);
        if denom.abs() < 1e-6 {
            return;
        }
        let t = (dr.plane_point - o).dot(dr.normal) / denom;
        self.mannequin.aim(dr.joint, o + d * t);
        self.mannequin_changed();
    }

    pub(super) fn mq_up(&mut self) {
        self.mqs.drag = None;
    }

    pub(super) fn mq_overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        let mut o = Overlay::default();
        if !self.mannequin_shown() {
            return o;
        }
        let w = self.mannequin.world();
        let hover = self.mqs.drag.map(|d| d.joint).or_else(|| p.and_then(|p| self.mq_joint_at(p, size)));
        let screen: Vec<Option<Vec2>> = w.iter().map(|(q, _)| self.to_screen(*q, size)).collect();
        for (i, jt) in self.mannequin.joints.iter().enumerate() {
            if let (Some(par), Some(b)) = (jt.parent, screen[i]) {
                if let Some(a) = screen[par] {
                    o.lines.push((a, b, OverlayStyle::Joint));
                }
            }
        }
        for (i, s) in screen.iter().enumerate() {
            if let Some(s) = s {
                if self.mannequin.joints[i].parent.is_some() {
                    o.points.push((*s, if hover == Some(i) { OverlayStyle::Active } else { OverlayStyle::Joint }));
                }
            }
        }
        o
    }
}
