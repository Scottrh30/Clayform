//! ZModeler mode: polygon actions picked and dragged in the viewport.

use super::{Drag, Editor, Mods, Overlay, OverlayStyle};
use crate::poly::{Action, Polys, Target};
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ZmSettings {
    pub action: Action,
    pub target: Target,
    /// Divisions per edge for new quad primitives.
    pub divisions: u32,
}

impl Default for ZmSettings {
    fn default() -> Self {
        Self { action: Action::Extrude, target: Target::Polygon, divisions: 2 }
    }
}

#[derive(Default)]
pub(super) struct ZmState {
    /// Polygons of the sculpt at a revision, with each triangle's polygon.
    cache: Option<(u64, usize, Polys, Vec<u32>)>,
    pub(super) drag: Option<ZmDrag>,
    pub(super) bridge: Option<BridgePick>,
}

pub(super) struct ZmDrag {
    base: Polys,
    regions: Vec<Vec<usize>>,
    start: Vec2,
    wpp: f32,
    /// Typical size of the target (bevel height scale).
    extent: f32,
    applied: bool,
    action: Action,
}

pub(super) struct BridgePick {
    lp: Vec<u32>,
    remove: Option<usize>,
    revision: u64,
    outline: Vec<Vec3>,
}

/// A polygon picked under the cursor.
struct Pick {
    face: usize,
    edge: (u32, u32),
    point: Vec3,
}

fn mirror_x(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

impl Editor {
    fn zm_polys(&mut self) -> (&Polys, &[u32]) {
        let rev = self.sculptor.revision();
        let n = self.sculptor.mesh.tris.len();
        let fresh = matches!(&self.zms.cache, Some((r, c, _, _)) if *r == rev && *c == n);
        if !fresh {
            let (p, map) = Polys::from_mesh(&self.sculptor.mesh);
            self.zms.cache = Some((rev, n, p, map));
        }
        let c = self.zms.cache.as_ref().unwrap();
        (&c.2, &c.3)
    }

    fn zm_pick(&mut self, p: Vec2, size: Vec2) -> Option<Pick> {
        let (o, d) = self.camera.ray(p, size);
        let h = self.sculptor.raycast(o, d)?;
        let (polys, map) = self.zm_polys();
        let face = *map.get(h.tri as usize)? as usize;
        if face >= polys.faces.len() {
            return None;
        }
        let edge = polys.nearest_edge(face, h.point);
        Some(Pick { face, edge, point: h.point })
    }

    /// The face mirrored across X from `pick`, if mirroring is on and it differs.
    fn zm_mirror(&mut self, pick: &Pick) -> Option<Pick> {
        if !self.brush.symmetry.mirror[0] {
            return None;
        }
        let q = mirror_x(pick.point);
        let (polys, _) = self.zm_polys();
        let c = mirror_x(polys.face_center(pick.face));
        let size = polys.face_center(pick.face).distance(polys.pos[polys.faces[pick.face][0] as usize]).max(1e-6);
        let face = (0..polys.faces.len())
            .min_by(|&a, &b| polys.face_center(a).distance_squared(c).total_cmp(&polys.face_center(b).distance_squared(c)))?;
        if face == pick.face || polys.face_center(face).distance(c) > size * 0.25 {
            return None;
        }
        let edge = polys.nearest_edge(face, q);
        Some(Pick { face, edge, point: q })
    }

    fn zm_regions(&mut self, pick: &Pick) -> Vec<Vec<usize>> {
        let target = self.zm.target;
        let mirror = self.zm_mirror(pick);
        let (polys, _) = self.zm_polys();
        let a = polys.target_faces(target, pick.face, pick.edge);
        let mut out = vec![a];
        if let Some(m) = mirror {
            let b = polys.target_faces(target, m.face, m.edge);
            // touching regions (sharing a corner) act as one, so no wall forms between them
            let verts_a: std::collections::HashSet<u32> = out[0].iter().flat_map(|&f| polys.faces[f].iter().copied()).collect();
            let touching = b.iter().any(|&f| polys.faces[f].iter().any(|v| verts_a.contains(v)));
            if touching || b.iter().any(|f| out[0].contains(f)) {
                for f in b {
                    if !out[0].contains(&f) {
                        out[0].push(f);
                    }
                }
            } else {
                out.push(b);
            }
        }
        out
    }

    fn zm_commit(&mut self, polys: Polys) {
        if polys.faces.is_empty() {
            self.message = Some("That would delete the whole model".into());
            return;
        }
        let m = polys.to_mesh();
        self.sculptor.set_label(self.zm.action.name());
        self.sculptor.set_mesh(m);
        self.redraw = true;
    }

    pub(super) fn zm_down(&mut self, p: Vec2, size: Vec2, _mods: Mods) -> Option<Drag> {
        let pick = self.zm_pick(p, size)?;
        let action = self.zm.action;
        if action.drags() {
            let regions = self.zm_regions(&pick);
            let (polys, _) = self.zm_polys();
            let base = polys.clone();
            let verts: Vec<Vec3> = regions[0].iter().flat_map(|&f| base.faces[f].iter().map(|&v| base.pos[v as usize])).collect();
            let c = verts.iter().copied().sum::<Vec3>() / verts.len().max(1) as f32;
            let extent = verts.iter().map(|q| q.distance(c)).sum::<f32>() / verts.len().max(1) as f32;
            let wpp = self.camera.world_per_point(pick.point, size.y);
            self.zms.drag = Some(ZmDrag { base, regions, start: p, wpp, extent: extent.max(1e-4), applied: false, action });
            return Some(Drag::Sculpt);
        }
        let mut polys = self.zm_polys().0.clone();
        match action {
            Action::Delete => {
                let mut all: Vec<usize> = self.zm_regions(&pick).concat();
                all.sort_unstable();
                all.dedup();
                polys.delete(&all);
                self.zm_commit(polys);
            }
            Action::InsertEdgeLoop => {
                let mirror = self.zm_mirror(&pick);
                match polys.insert_edge_loop(pick.edge) {
                    Ok(()) => {
                        if let Some(m) = mirror {
                            let _ = polys.insert_edge_loop(m.edge); // same ring: already split
                        }
                        self.zm_commit(polys);
                    }
                    Err(e) => self.message = Some(e),
                }
            }
            Action::Polygroup => {
                let g = polys.groups.iter().copied().max().map_or(0, |g| g + 1);
                for f in self.zm_regions(&pick).concat() {
                    polys.groups[f] = g;
                }
                self.zm_set_groups(&polys);
            }
            Action::GroupIt => {
                let g = polys.groups.iter().copied().max().map_or(0, |g| g + 1);
                let mut faces = polys.flood_by_normal(pick.face, 30f32.to_radians());
                if let Some(m) = self.zm_mirror(&pick) {
                    faces.extend(polys.flood_by_normal(m.face, 30f32.to_radians()));
                }
                for f in faces {
                    polys.groups[f] = g;
                }
                self.zm_set_groups(&polys);
            }
            Action::Bridge => self.zm_bridge(pick, polys),
            Action::Extrude | Action::Inset | Action::Bevel => unreachable!(),
        }
        Some(Drag::None)
    }

    /// Applies new polygroups from a polygon list without changing the mesh.
    fn zm_set_groups(&mut self, polys: &Polys) {
        let (_, map) = self.zm_polys();
        let groups: Vec<u32> = map
            .iter()
            .map(|&f| if (f as usize) < polys.groups.len() { polys.groups[f as usize] } else { 0 })
            .collect();
        self.sculptor.set_label_if_none(self.zm.action.name());
        self.sculptor.set_groups(groups);
        self.redraw = true;
    }

    fn zm_bridge(&mut self, pick: Pick, mut polys: Polys) {
        // a boundary edge picks its hole; anything else picks the polygon
        let hole = polys.hole_loop(pick.edge);
        let (lp, remove) = match hole {
            Some(l) => (l, None),
            None => (polys.faces[pick.face].clone(), Some(pick.face)),
        };
        let rev = self.sculptor.revision();
        match self.zms.bridge.take().filter(|b| b.revision == rev) {
            None => {
                let outline = lp.iter().map(|&v| polys.pos[v as usize]).collect();
                self.zms.bridge = Some(BridgePick { lp, remove, revision: rev, outline });
                self.message = Some("Bridge: now click the polygon or hole to connect to".into());
            }
            Some(first) => {
                if first.remove == remove && remove.is_some() || first.lp == lp {
                    self.zms.bridge = Some(first);
                    return;
                }
                let removes: Vec<usize> = first.remove.into_iter().chain(remove).collect();
                match polys.bridge(&first.lp, &lp, &removes) {
                    Ok(()) => self.zm_commit(polys),
                    Err(e) => self.message = Some(e),
                }
            }
        }
    }

    pub(super) fn zm_move(&mut self, p: Vec2, _size: Vec2) {
        let Some(dr) = self.zms.drag.as_mut() else { return };
        let d = p - dr.start;
        let mut polys = dr.base.clone();
        let inset = (d.length() / 220.0).clamp(0.0, 0.95);
        for region in &dr.regions {
            match dr.action {
                Action::Extrude => polys.extrude(region, -d.y * dr.wpp),
                Action::Inset => polys.inset(region, inset),
                _ => polys.bevel(region, inset, -d.y.signum() * inset * dr.extent * 0.6),
            }
        }
        let m = polys.to_mesh();
        if dr.applied {
            self.sculptor.replace_mesh_preview(m);
        } else {
            dr.applied = true;
            self.sculptor.set_label(dr.action.name());
            self.sculptor.set_mesh(m);
        }
        self.redraw = true;
    }

    pub(super) fn zm_up(&mut self) {
        self.zms.drag = None;
        self.zms.cache = None;
    }

    pub(super) fn zm_overlay(&mut self, p: Option<Vec2>, size: Vec2) -> Overlay {
        let mut o = Overlay::default();
        if let Some(b) = &self.zms.bridge {
            if b.revision == self.sculptor.revision() {
                let pts: Vec<Option<Vec2>> = b.outline.iter().map(|&q| self.to_screen(q, size)).collect();
                for i in 0..pts.len() {
                    if let (Some(a), Some(c)) = (pts[i], pts[(i + 1) % pts.len()]) {
                        o.lines.push((a, c, OverlayStyle::Pending));
                    }
                }
            }
        }
        if self.zms.drag.is_some() {
            return o;
        }
        let Some(p) = p else { return o };
        let Some(pick) = self.zm_pick(p, size) else { return o };
        let action = self.zm.action;
        let regions = if matches!(action, Action::InsertEdgeLoop | Action::Bridge | Action::GroupIt) {
            vec![vec![pick.face]]
        } else {
            self.zm_regions(&pick)
        };
        let (polys, _) = self.zm_polys();
        let mut segs: Vec<(Vec3, Vec3)> = Vec::new();
        for r in &regions {
            for (a, b) in polys.boundary_edges(r) {
                segs.push((polys.pos[a as usize], polys.pos[b as usize]));
            }
        }
        let edge = (polys.pos[pick.edge.0 as usize], polys.pos[pick.edge.1 as usize]);
        segs.truncate(6000);
        for (a, b) in segs {
            if let (Some(a), Some(b)) = (self.to_screen(a, size), self.to_screen(b, size)) {
                o.lines.push((a, b, OverlayStyle::Target));
            }
        }
        if matches!(action, Action::InsertEdgeLoop | Action::Bridge) {
            if let (Some(a), Some(b)) = (self.to_screen(edge.0, size), self.to_screen(edge.1, size)) {
                o.lines.push((a, b, OverlayStyle::Active));
            }
        }
        o
    }

    /// Replaces the sculpt with a quad primitive (undoable).
    pub fn new_primitive(&mut self, kind: crate::poly::Primitive) {
        let d = self.zm.divisions.clamp(1, 32);
        let p = kind.build(d);
        self.cancel_drag();
        self.sculptor.set_label(format!("New {}", kind.name()));
        self.sculptor.set_mesh(p.to_mesh());
        self.frame_mesh();
        self.redraw = true;
    }

    /// Gives every region of faces whose normals turn by less than
    /// `angle_deg` its own polygroup (undoable).
    pub fn auto_groups(&mut self, angle_deg: f32) {
        let mut polys = self.zm_polys().0.clone();
        polys.auto_groups(angle_deg.to_radians());
        self.sculptor.set_label("Group by normals");
        self.zm_set_groups(&polys);
    }

    /// Masked faces (all corners at least half masked) become one new polygroup.
    pub fn group_masked(&mut self) {
        let m = &self.sculptor.mesh;
        let g = m.groups.iter().copied().max().map_or(0, |g| g + 1);
        let mut groups = m.groups.clone();
        let mut any = false;
        for (f, t) in m.tris.iter().enumerate() {
            if t[0] != crate::mesh::NONE && t.iter().all(|&v| m.mask[v as usize] >= 0.5) {
                groups[f] = g;
                any = true;
            }
        }
        if any {
            self.sculptor.set_label("Group masked");
            self.sculptor.set_groups(groups);
            self.redraw = true;
        } else {
            self.message = Some("Nothing is masked".into());
        }
    }

    /// Puts every face back in polygroup 0 (undoable).
    pub fn clear_groups(&mut self) {
        let n = self.sculptor.mesh.tris.len();
        self.sculptor.set_label("Clear polygroups");
        self.sculptor.set_groups(vec![0; n]);
        self.redraw = true;
    }

    /// Masks everything outside the polygroup under the cursor (undoable).
    pub fn mask_by_group_at(&mut self, p: Vec2, size: Vec2) {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return };
        let g = self.sculptor.mesh.groups[h.tri as usize];
        self.sculptor.mask_outside_group(g);
        self.redraw = true;
    }
}
