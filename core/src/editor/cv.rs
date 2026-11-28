//! Curve Tube and Curve Strap: the drawn curve becomes new geometry.

use super::Editor;
use crate::mesh::Mesh;
use crate::sculpt::Tool;
use crate::symmetry::apply_point;
use glam::{Vec2, Vec3};

impl Editor {
    /// Surface points (with normals facing the camera) under the drawn curve.
    fn curve_on_surface(&self, path: &[Vec2], size: Vec2) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::new();
        let mut depth: Option<f32> = None;
        for &q in path {
            let (o, d) = self.camera.ray(q, size);
            if let Some(h) = self.sculptor.raycast(o, d) {
                let mut n = self.sculptor.mesh.face_normal_raw(h.tri as usize).normalize_or_zero();
                if n.dot(d) > 0.0 {
                    n = -n;
                }
                depth = Some((h.point - o).dot(d));
                out.push((h.point, n));
            } else if let Some(t) = depth {
                // off the model: carry on at the last depth, facing the camera
                out.push((o + d * t, -d));
            }
        }
        out
    }

    /// Builds the tube or strap for the drawn curve (and its symmetric
    /// copies) and adds it: into the active mesh when it has a single level
    /// (one undo step), otherwise as a new SubTool.
    pub(super) fn finish_curve_mesh(&mut self) {
        let path = std::mem::take(&mut self.curve);
        let tool = self.stroke_tool;
        if path.len() < 2 {
            return;
        }
        let size = self.last_size;
        self.sculptor.refresh_bvh();
        let pts = self.curve_on_surface(&path, size);
        if pts.len() < 2 {
            self.message = Some("Draw the curve over the model".into());
            return;
        }
        let wpp = self.camera.world_per_point(pts[0].0, size.y);
        let r = self.brush.radius_px * wpp * 0.5;
        let i = self.brush.intensity[tool.index()].clamp(0.0, 1.0);
        let build = |p: &[(Vec3, Vec3)]| -> Option<Mesh> {
            let p = crate::tube::resample(p, r * 0.5);
            match tool {
                // intensity sets how far the tube sits out of the surface
                Tool::CurveTube => crate::tube::tube(&p, r, r * (i * 1.4 - 0.4), self.stroke_negative),
                _ => crate::tube::strap(&p, r * 2.0, r * (0.08 + 0.5 * i)),
            }
        };
        let mut pieces: Vec<Mesh> = Vec::new();
        pieces.extend(build(&pts));
        let c = self.symmetry_center();
        for m in self.brush.symmetry.copies() {
            let mp: Vec<(Vec3, Vec3)> = pts.iter().map(|(p, n)| (apply_point(m, c, *p), (m * *n).normalize_or_zero())).collect();
            pieces.extend(build(&mp));
        }
        if pieces.is_empty() {
            return;
        }
        let name = if tool == Tool::CurveTube { "Curve Tube" } else { "Curve Strap" };
        if self.sculptor.level_count() == 1 {
            let mut refs: Vec<&Mesh> = vec![&self.sculptor.mesh];
            refs.extend(pieces.iter());
            let (joined, _) = crate::scene::concat(&refs);
            self.sculptor.set_label(name);
            self.sculptor.set_mesh(joined);
        } else {
            let refs: Vec<&Mesh> = pieces.iter().collect();
            let (joined, _) = crate::scene::concat(&refs);
            self.append_subtool(joined, if tool == Tool::CurveTube { "Tube" } else { "Strap" });
            self.message = Some(format!("{name} added as a new SubTool (the sculpt has several levels)"));
        }
        self.redraw = true;
    }
}
