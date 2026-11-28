//! Transform mode: a gizmo to move, rotate and scale the active SubTool (or
//! its unmasked part), and deformers dragged along an axis.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use crate::deform::Deformer;
use crate::sculpt::{DragInput, DragKind, Gather};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};

const ARROW_PX: f32 = 95.0;
const SCALE_PX: f32 = 118.0;
const RING_PX: f32 = 70.0;
const OUTER_PX: f32 = 140.0;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GizmoSettings {
    /// None: the move/rotate/scale gizmo. Some: drag left/right to deform.
    pub deformer: Option<Deformer>,
    /// Deformer axis (0 = X, 1 = Y, 2 = Z).
    pub axis: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Move(usize),
    MoveFree,
    Rotate(usize),
    Scale(usize),
    ScaleAll,
    Deform,
}

#[derive(Default)]
pub(super) struct GzState {
    /// Pivot (centre of the unmasked part) at a sculpt revision.
    pivot: Option<(u64, Vec3)>,
    drag: Option<GzDrag>,
}

struct GzDrag {
    handle: Handle,
    start: Vec2,
    pivot: Vec3,
    pivot_s: Vec2,
    /// Screen direction of the dragged axis (unit) and world length per point.
    dir_s: Vec2,
    /// Screen points per world unit along the dragged axis, times wpp.
    axis_s: f32,
    wpp: f32,
    normal: Vec3,
    /// Ctrl was held when the drag began: snap to round steps.
    snap: Option<f32>,
}

/// Rounds `v` to a multiple of `step`.
fn snap_to(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

impl Editor {
    pub(super) fn gz_reset(&mut self) {
        self.gzs = GzState::default();
    }

    /// Centre of what the gizmo moves: the unmasked part's bounding box.
    fn gz_pivot(&mut self) -> Vec3 {
        let rev = self.sculptor.revision();
        if let Some((r, p)) = self.gzs.pivot {
            if r == rev {
                return p;
            }
        }
        let m = &self.sculptor.mesh;
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (v, p) in m.positions.iter().enumerate() {
            if m.mask[v] < 0.5 && !m.vert_faces(v as u32).is_empty() {
                mn = mn.min(*p);
                mx = mx.max(*p);
            }
        }
        let p = if mn.x <= mx.x { (mn + mx) * 0.5 } else { Vec3::ZERO };
        self.gzs.pivot = Some((rev, p));
        p
    }

    /// Screen geometry: pivot, axis directions (unit) and whether each axis
    /// can be seen (not pointing straight at the camera).
    fn gz_frame(&mut self, size: Vec2) -> Option<(Vec2, [Vec2; 3], f32, Vec3)> {
        let pivot = self.gz_pivot();
        let ps = self.to_screen(pivot, size)?;
        let wpp = self.camera.world_per_point(pivot, size.y);
        let mut dirs = [Vec2::ZERO; 3];
        for (k, a) in AXES.iter().enumerate() {
            let q = self.to_screen(pivot + *a * wpp * 40.0, size)?;
            dirs[k] = (q - ps) / 40.0;
        }
        Some((ps, dirs, wpp, pivot))
    }

    fn ring_points(&self, pivot: Vec3, axis: usize, r: f32, size: Vec2) -> Vec<Vec2> {
        let a = AXES[(axis + 1) % 3];
        let b = AXES[(axis + 2) % 3];
        (0..=48)
            .filter_map(|i| {
                let t = std::f32::consts::TAU * i as f32 / 48.0;
                self.to_screen(pivot + (a * t.cos() + b * t.sin()) * r, size)
            })
            .collect()
    }

    fn gz_handle_at(&mut self, p: Vec2, size: Vec2) -> Option<Handle> {
        if self.gizmo.deformer.is_some() {
            return Some(Handle::Deform);
        }
        let (ps, dirs, wpp, pivot) = self.gz_frame(size)?;
        if p.distance(ps) < 12.0 {
            return Some(Handle::MoveFree);
        }
        let seg = |a: Vec2, b: Vec2| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (a + ab * t).distance(p)
        };
        for k in 0..3 {
            if dirs[k].length() > 0.15 && p.distance(ps + dirs[k] * SCALE_PX) < 9.0 {
                return Some(Handle::Scale(k));
            }
        }
        let mut best: Option<(f32, Handle)> = None;
        for k in 0..3 {
            if dirs[k].length() > 0.15 {
                let d = seg(ps + dirs[k] * 14.0, ps + dirs[k] * ARROW_PX);
                if d < 8.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, Handle::Move(k)));
                }
            }
        }
        if let Some(b) = best {
            return Some(b.1);
        }
        for k in 0..3 {
            let ring = self.ring_points(pivot, k, RING_PX * wpp, size);
            let d = ring.windows(2).map(|w| seg(w[0], w[1])).fold(f32::MAX, f32::min);
            if d < 6.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, Handle::Rotate(k)));
            }
        }
        if let Some(b) = best {
            return Some(b.1);
        }
        if (p.distance(ps) - OUTER_PX).abs() < 7.0 {
            return Some(Handle::ScaleAll);
        }
        None
    }

    pub(super) fn gz_down(&mut self, p: Vec2, size: Vec2, mods: Mods) -> Option<Drag> {
        let handle = self.gz_handle_at(p, size)?;
        let (ps, dirs, wpp, pivot) = self.gz_frame(size)?;
        let (dir_s, axis_s) = match handle {
            Handle::Move(k) | Handle::Scale(k) => (dirs[k].normalize_or_zero(), dirs[k].length().max(0.05)),
            _ => (Vec2::X, 1.0),
        };
        let kind = match (handle, self.gizmo.deformer) {
            (Handle::Deform, Some(d)) => DragKind::Deform(d, self.gizmo.axis),
            _ => DragKind::Affine,
        };
        let label = match handle {
            Handle::Move(_) | Handle::MoveFree => "Move".to_string(),
            Handle::Rotate(_) => "Rotate".to_string(),
            Handle::Scale(_) | Handle::ScaleAll => "Scale".to_string(),
            Handle::Deform => self.gizmo.deformer.map_or("Deform", |d| d.name()).to_string(),
        };
        self.sculptor.begin_stroke(false);
        self.sculptor.label_stroke(label);
        self.sculptor.drag_begin(pivot, 1.0, 0, Mat3::IDENTITY, Gather::Whole, kind);
        // move snapping uses a round step about a tenth of the object's size
        let snap = mods.ctrl.then(|| {
            let (mn, mx) = self.sculptor.mesh.bounds();
            let size = (mx - mn).max_element().max(1e-4);
            10f32.powf((size * 0.1).log10().round())
        });
        self.gzs.drag = Some(GzDrag { handle, start: p, pivot, pivot_s: ps, dir_s, axis_s, wpp, normal: self.camera.forward(), snap });
        Some(Drag::Sculpt)
    }

    pub(super) fn gz_move(&mut self, p: Vec2, size: Vec2) {
        let Some(dr) = self.gzs.drag.as_ref() else { return };
        let d = p - dr.start;
        let around = |m: Mat4| Mat4::from_translation(dr.pivot) * m * Mat4::from_translation(-dr.pivot);
        let mut input = DragInput { wpp: dr.wpp, view: dr.normal, ..Default::default() };
        match dr.handle {
            Handle::Move(k) => {
                let mut along = d.dot(dr.dir_s) * dr.wpp / dr.axis_s;
                if let Some(st) = dr.snap {
                    along = snap_to(along, st);
                }
                input.affine = Mat4::from_translation(AXES[k] * along);
            }
            Handle::MoveFree => {
                let (o, dir) = self.camera.ray(p, size);
                let denom = dir.dot(dr.normal);
                if denom.abs() > 1e-6 {
                    let t = (dr.pivot - o).dot(dr.normal) / denom;
                    let mut v = o + dir * t - dr.pivot;
                    if let Some(st) = dr.snap {
                        v = Vec3::new(snap_to(v.x, st), snap_to(v.y, st), snap_to(v.z, st));
                    }
                    input.affine = Mat4::from_translation(v);
                }
            }
            Handle::Rotate(k) => {
                let a0 = (dr.start - dr.pivot_s).to_angle();
                let a1 = (p - dr.pivot_s).to_angle();
                let toward = AXES[k].dot(self.camera.eye() - dr.pivot) > 0.0;
                let mut ang = -(a1 - a0) * if toward { 1.0 } else { -1.0 };
                if dr.snap.is_some() {
                    ang = snap_to(ang, 15f32.to_radians());
                }
                input.affine = around(Mat4::from_quat(Quat::from_axis_angle(AXES[k], ang)));
            }
            Handle::Scale(k) => {
                let mut f = (1.0 + d.dot(dr.dir_s) / SCALE_PX).max(0.02);
                if dr.snap.is_some() {
                    f = snap_to(f, 0.1).max(0.1);
                }
                let mut s = Vec3::ONE;
                s[k] = f;
                input.affine = around(Mat4::from_scale(s));
            }
            Handle::ScaleAll => {
                let r0 = dr.start.distance(dr.pivot_s).max(1.0);
                let mut f = (p.distance(dr.pivot_s) / r0).max(0.02);
                if dr.snap.is_some() {
                    f = snap_to(f, 0.1).max(0.1);
                }
                input.affine = around(Mat4::from_scale(Vec3::splat(f)));
            }
            Handle::Deform => {
                input.amount = d.x / 300.0;
            }
        }
        self.sculptor.drag_update(input);
        self.redraw = true;
    }

    pub(super) fn gz_up(&mut self) {
        self.gzs.drag = None;
        self.gzs.pivot = None;
    }

    pub(super) fn gz_overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        let mut o = Overlay::default();
        if self.gizmo.deformer.is_some() {
            // show the deformer axis through the pivot
            if let Some((ps, dirs, _, _)) = self.gz_frame(size) {
                let d = dirs[self.gizmo.axis.min(2)].normalize_or_zero();
                o.lines.push((ps - d * 160.0, ps + d * 160.0, OverlayStyle::Axis(self.gizmo.axis.min(2) as u8)));
                o.points.push((ps, OverlayStyle::Active));
            }
            return o;
        }
        if self.sculptor.mesh.tris.is_empty() {
            return o;
        }
        let hover = match &self.gzs.drag {
            Some(dr) => Some(dr.handle),
            None => p.and_then(|p| self.gz_handle_at(p, size)),
        };
        let Some((ps, dirs, wpp, pivot)) = self.gz_frame(size) else { return o };
        let st = |h: Handle, k: u8| if hover == Some(h) { OverlayStyle::Active } else { OverlayStyle::Axis(k) };
        for k in 0..3 {
            let ring = self.ring_points(pivot, k, RING_PX * wpp, size);
            let s = st(Handle::Rotate(k), k as u8);
            for w in ring.windows(2) {
                o.lines.push((w[0], w[1], s));
            }
        }
        for k in 0..3 {
            if dirs[k].length() > 0.15 {
                let d = dirs[k].normalize();
                let s = st(Handle::Move(k), k as u8);
                let tip = ps + d * ARROW_PX;
                o.lines.push((ps + d * 14.0, tip, s));
                let side = Vec2::new(-d.y, d.x) * 5.0;
                o.lines.push((tip, tip - d * 10.0 + side, s));
                o.lines.push((tip, tip - d * 10.0 - side, s));
                o.points.push((ps + d * SCALE_PX, st(Handle::Scale(k), k as u8)));
            }
        }
        let outer: Vec<Vec2> = (0..=64)
            .map(|i| ps + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 64.0) * OUTER_PX)
            .collect();
        let s = if hover == Some(Handle::ScaleAll) { OverlayStyle::Active } else { OverlayStyle::Pending };
        for w in outer.windows(2) {
            o.lines.push((w[0], w[1], s));
        }
        o.points.push((ps, if hover == Some(Handle::MoveFree) { OverlayStyle::Active } else { OverlayStyle::Joint }));
        o
    }
}
