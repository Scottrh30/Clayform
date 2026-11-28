//! SubTools: several objects in one sculpt. The active one's sculptor lives
//! in `Editor::sculptor`; the others wait in `Editor::subtools`.

use super::Editor;
use crate::mesh::{Mesh, MeshBuilder, NONE};
use crate::scene::{bake, concat, BoolOp, SubToolInfo};
use crate::sculpt::Sculptor;
use glam::{Mat4, Vec2};

pub struct SubTool {
    pub info: SubToolInfo,
    /// None for the active SubTool (its sculptor is `Editor::sculptor`).
    pub sculptor: Option<Sculptor>,
}

impl Editor {
    pub fn subtool_count(&self) -> usize {
        self.subtools.len()
    }

    pub fn active_subtool(&self) -> usize {
        self.active
    }

    pub fn subtool_info(&self, i: usize) -> &SubToolInfo {
        &self.subtools[i].info
    }

    pub fn subtool_info_mut(&mut self, i: usize) -> &mut SubToolInfo {
        self.redraw = true;
        &mut self.subtools[i].info
    }

    pub fn sculptor_at(&self, i: usize) -> &Sculptor {
        if i == self.active { &self.sculptor } else { self.subtools[i].sculptor.as_ref().expect("subtool sculptor") }
    }

    pub fn sculptor_at_mut(&mut self, i: usize) -> &mut Sculptor {
        if i == self.active {
            &mut self.sculptor
        } else {
            self.subtools[i].sculptor.as_mut().expect("subtool sculptor")
        }
    }

    fn new_info(&mut self, name: &str) -> SubToolInfo {
        self.next_id += 1;
        SubToolInfo { id: self.next_id, name: name.to_string(), visible: true, op: BoolOp::Add, array: None }
    }

    /// Makes SubTool `i` the one being sculpted.
    pub fn select_subtool(&mut self, i: usize) {
        if i == self.active || i >= self.subtools.len() {
            return;
        }
        self.cancel_drag();
        let next = self.subtools[i].sculptor.take().expect("subtool sculptor");
        let cur = std::mem::replace(&mut self.sculptor, next);
        self.subtools[self.active].sculptor = Some(cur);
        self.active = i;
        self.subtools[i].info.visible = true;
        self.after_switch();
    }

    fn after_switch(&mut self) {
        self.zms = Default::default();
        self.mqs = Default::default();
        self.retopo.pending.clear();
        self.gz_reset();
        self.redraw = true;
    }

    /// Adds a SubTool after the active one and selects it.
    pub fn append_subtool(&mut self, mesh: Mesh, name: &str) {
        self.insert_subtool(Sculptor::new(mesh), name);
    }

    fn insert_subtool(&mut self, sculptor: Sculptor, name: &str) {
        self.cancel_drag();
        let info = self.new_info(name);
        let at = self.active + 1;
        self.subtools.insert(at, SubTool { info, sculptor: Some(sculptor) });
        self.select_subtool(at);
    }

    /// Copies the active SubTool (with all its levels).
    pub fn duplicate_subtool(&mut self) {
        let (levels, level) = self.sculptor.clone_levels();
        let s = Sculptor::from_levels(levels, level, None);
        let mut info = self.subtools[self.active].info.clone();
        let name = format!("{} copy", info.name);
        self.insert_subtool(s, &name);
        info.id = self.subtools[self.active].info.id;
        info.name = name;
        self.subtools[self.active].info = info;
    }

    /// Removes SubTool `i` (not the last one). Returns false if it can't.
    pub fn delete_subtool(&mut self, i: usize) -> bool {
        if self.subtools.len() < 2 || i >= self.subtools.len() {
            return false;
        }
        self.cancel_drag();
        if i == self.active {
            let next = if i + 1 < self.subtools.len() { i + 1 } else { i - 1 };
            self.select_subtool(next);
        }
        self.subtools.remove(i);
        if i < self.active {
            self.active -= 1;
        }
        self.redraw = true;
        true
    }

    pub fn move_subtool(&mut self, i: usize, up: bool) {
        let j = if up { i.wrapping_sub(1) } else { i + 1 };
        if i >= self.subtools.len() || j >= self.subtools.len() {
            return;
        }
        self.subtools.swap(i, j);
        if self.active == i {
            self.active = j;
        } else if self.active == j {
            self.active = i;
        }
        self.redraw = true;
    }

    /// Copy transforms for SubTool `i` (just the identity without an array).
    pub fn array_transforms(&self, i: usize) -> Vec<Mat4> {
        match self.subtools[i].info.array {
            Some(a) => {
                let (mn, mx) = self.sculptor_at(i).mesh.bounds();
                a.transforms(mn, mx)
            }
            None => vec![Mat4::IDENTITY],
        }
    }

    /// SubTool `i`'s current level with its array copies baked in.
    pub fn baked_mesh(&self, i: usize) -> Mesh {
        let m = &self.sculptor_at(i).mesh;
        match self.subtools[i].info.array {
            Some(_) => bake(m, &self.array_transforms(i)),
            None => m.clone(),
        }
    }

    /// Turns the active SubTool's array into real geometry (undoable).
    pub fn convert_array(&mut self) {
        if self.subtools[self.active].info.array.is_none() {
            return;
        }
        let m = self.baked_mesh(self.active);
        self.subtools[self.active].info.array = None;
        self.sculptor.set_label("Convert array");
        self.sculptor.set_mesh(m);
        self.redraw = true;
    }

    fn visible_indices(&self) -> Vec<usize> {
        (0..self.subtools.len()).filter(|&i| self.subtools[i].info.visible).collect()
    }

    /// All visible SubTools as one mesh (arrays baked), e.g. for export.
    pub fn visible_mesh(&self) -> Mesh {
        let meshes: Vec<Mesh> = self.visible_indices().into_iter().map(|i| self.baked_mesh(i)).collect();
        let refs: Vec<&Mesh> = meshes.iter().collect();
        concat(&refs).0
    }

    /// The visible SubTools as boolean parts, top to bottom.
    pub fn boolean_parts(&self) -> Option<(Mesh, Vec<(usize, BoolOp)>)> {
        let vis = self.visible_indices();
        if vis.len() < 2 {
            return None;
        }
        let meshes: Vec<Mesh> = vis.iter().map(|&i| self.baked_mesh(i)).collect();
        let refs: Vec<&Mesh> = meshes.iter().collect();
        let (m, ends) = concat(&refs);
        let parts = vis.iter().zip(ends).map(|(&i, e)| (e, self.subtools[i].info.op)).collect();
        Some((m, parts))
    }

    /// Replaces every visible SubTool with one merged SubTool.
    pub fn merge_visible(&mut self) -> bool {
        let vis = self.visible_indices();
        if vis.len() < 2 {
            return false;
        }
        let merged = self.visible_mesh();
        let name = format!("{} merged", self.subtools[vis[0]].info.name);
        let ids: Vec<u64> = vis.iter().map(|&i| self.subtools[i].info.id).collect();
        self.append_subtool(merged, &name);
        let keep = self.subtools[self.active].info.id;
        for id in ids {
            if let Some(i) = self.subtools.iter().position(|s| s.info.id == id) {
                self.delete_subtool(i);
            }
        }
        if let Some(i) = self.subtools.iter().position(|s| s.info.id == keep) {
            self.select_subtool(i);
        }
        true
    }

    /// Splits the active SubTool into one SubTool per polygroup (current level).
    pub fn split_by_groups(&mut self) -> usize {
        let src = self.sculptor.mesh.clone();
        let mut groups: Vec<u32> =
            (0..src.tris.len()).filter(|&f| src.tris[f][0] != NONE).map(|f| src.groups[f]).collect();
        groups.sort_unstable();
        groups.dedup();
        if groups.len() < 2 {
            return 0;
        }
        let base = self.subtools[self.active].info.name.clone();
        let old = self.active;
        for (k, &g) in groups.iter().enumerate() {
            let mut remap = vec![NONE; src.positions.len()];
            let mut pos = Vec::new();
            let mut mask = Vec::new();
            let mut tris = Vec::new();
            let mut partner_src = Vec::new();
            let mut face_new = vec![NONE; src.tris.len()];
            for (f, t) in src.tris.iter().enumerate() {
                if t[0] == NONE || src.groups[f] != g {
                    continue;
                }
                let mut nt = [0u32; 3];
                for c in 0..3 {
                    let v = t[c] as usize;
                    if remap[v] == NONE {
                        remap[v] = pos.len() as u32;
                        pos.push(src.positions[v]);
                        mask.push(src.mask[v]);
                    }
                    nt[c] = remap[v];
                }
                face_new[f] = tris.len() as u32;
                tris.push(nt);
                partner_src.push(src.partner[f]);
            }
            let partner: Vec<u32> =
                partner_src.iter().map(|&p| if p == NONE { NONE } else { face_new[p as usize] }).collect();
            let n = tris.len();
            let mut m = MeshBuilder::new(pos, tris).with_faces(Some(vec![0; n]), Some(partner)).finish();
            m.mask = mask;
            m.export_center = src.export_center;
            m.export_scale = src.export_scale;
            self.append_subtool(m, &format!("{base} {}", k + 1));
        }
        let first_new = old + 1;
        // remove the original
        let id = self.subtools[old].info.id;
        if let Some(i) = self.subtools.iter().position(|s| s.info.id == id) {
            self.delete_subtool(i);
        }
        self.select_subtool(first_new.saturating_sub(1).min(self.subtools.len() - 1));
        groups.len()
    }

    /// The visible SubTool under the cursor, if any.
    pub fn subtool_at(&mut self, p: Vec2, size: Vec2) -> Option<usize> {
        let (o, d) = self.camera.ray(p, size);
        let mut best: Option<(f32, usize)> = None;
        for i in 0..self.subtools.len() {
            if !self.subtools[i].info.visible {
                continue;
            }
            let s = self.sculptor_at_mut(i);
            s.refresh_bvh();
            if let Some(h) = s.raycast(o, d) {
                if best.is_none_or(|b| h.t < b.0) {
                    best = Some((h.t, i));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Bounds of everything visible.
    pub fn scene_bounds(&self) -> (glam::Vec3, glam::Vec3) {
        let mut mn = glam::Vec3::splat(f32::MAX);
        let mut mx = glam::Vec3::splat(f32::MIN);
        for i in self.visible_indices() {
            let (a, b) = self.sculptor_at(i).mesh.bounds();
            for m in self.array_transforms(i) {
                for c in 0..8 {
                    let q = glam::Vec3::new(
                        if c & 1 == 0 { a.x } else { b.x },
                        if c & 2 == 0 { a.y } else { b.y },
                        if c & 4 == 0 { a.z } else { b.z },
                    );
                    let q = m.transform_point3(q);
                    mn = mn.min(q);
                    mx = mx.max(q);
                }
            }
        }
        if mn.x > mx.x { self.sculptor.mesh.bounds() } else { (mn, mx) }
    }

    /// Replaces the whole scene (opening a project). Not undoable.
    pub fn load_scene(&mut self, parts: Vec<(SubToolInfo, Sculptor)>, active: usize) {
        if parts.is_empty() {
            return;
        }
        self.cancel_drag();
        let active = active.min(parts.len() - 1);
        self.subtools.clear();
        for (i, (mut info, s)) in parts.into_iter().enumerate() {
            self.next_id += 1;
            info.id = self.next_id;
            if i == active {
                self.sculptor = s;
                self.subtools.push(SubTool { info, sculptor: None });
            } else {
                self.subtools.push(SubTool { info, sculptor: Some(s) });
            }
        }
        self.active = active;
        self.after_switch();
    }

    /// Every SubTool's levels (for saving), with its info and active level.
    pub fn scene_levels(&self) -> Vec<(SubToolInfo, Vec<crate::project::LevelData<&Mesh>>, usize)> {
        (0..self.subtools.len())
            .map(|i| {
                let (levels, active) = self.sculptor_at(i).project_levels();
                (self.subtools[i].info.clone(), levels, active)
            })
            .collect()
    }

    /// A revision number that changes whenever any SubTool changes.
    pub fn scene_revision(&self) -> u64 {
        (0..self.subtools.len()).map(|i| self.sculptor_at(i).revision()).sum::<u64>()
            + self.subtools.len() as u64 * 1_000_003
            + self.subtools.iter().map(|s| s.info.id).sum::<u64>()
    }
}
