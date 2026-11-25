//! Sculpting engine: brushes, strokes, symmetry helpers and undo/redo.

use crate::bvh::{Bvh, Hit};
use crate::dyntopo::remesh_region;
use crate::mesh::{Marker, Mesh};
use glam::{Mat3, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Draw,
    Clay,
    ClayBuildup,
    Smooth,
    Flatten,
    TrimDynamic,
    Scrape,
    Inflate,
    Pinch,
    Crease,
    DamStandard,
    Layer,
    Grab,
    Snakehook,
    Nudge,
    Mask,
}

impl Tool {
    pub const ALL: [Tool; 16] = [
        Tool::Draw,
        Tool::Clay,
        Tool::ClayBuildup,
        Tool::Smooth,
        Tool::Flatten,
        Tool::TrimDynamic,
        Tool::Scrape,
        Tool::Inflate,
        Tool::Pinch,
        Tool::Crease,
        Tool::DamStandard,
        Tool::Layer,
        Tool::Grab,
        Tool::Snakehook,
        Tool::Nudge,
        Tool::Mask,
    ];

    pub fn index(self) -> usize {
        Tool::ALL.iter().position(|t| *t == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Draw => "Draw",
            Tool::Clay => "Clay",
            Tool::ClayBuildup => "Clay Buildup",
            Tool::Smooth => "Smooth",
            Tool::Flatten => "Flatten",
            Tool::TrimDynamic => "Trim Dynamic",
            Tool::Scrape => "Scrape",
            Tool::Inflate => "Inflate",
            Tool::Pinch => "Pinch",
            Tool::Crease => "Crease",
            Tool::DamStandard => "Dam Standard",
            Tool::Layer => "Layer",
            Tool::Grab => "Grab",
            Tool::Snakehook => "Snakehook",
            Tool::Nudge => "Nudge",
            Tool::Mask => "Mask",
        }
    }

    pub fn shortcut(self) -> char {
        match self {
            Tool::Draw => 'D',
            Tool::Clay => 'C',
            Tool::ClayBuildup => 'B',
            Tool::Smooth => 'S',
            Tool::Flatten => 'F',
            Tool::TrimDynamic => 'Y',
            Tool::Scrape => 'Q',
            Tool::Inflate => 'I',
            Tool::Pinch => 'P',
            Tool::Crease => 'E',
            Tool::DamStandard => 'V',
            Tool::Layer => 'L',
            Tool::Grab => 'G',
            Tool::Snakehook => 'K',
            Tool::Nudge => 'U',
            Tool::Mask => 'M',
        }
    }

    /// One line describing what the brush does (shown as a tooltip).
    pub fn description(self) -> &'static str {
        match self {
            Tool::Draw => "Raises the surface along the brush's average normal",
            Tool::Clay => "Builds up material with a flattened top",
            Tool::ClayBuildup => "Fast, square-edged clay strips for blocking in forms",
            Tool::Smooth => "Relaxes the surface (also Shift with any brush)",
            Tool::Flatten => "Pulls the surface onto a plane",
            Tool::TrimDynamic => "Cuts peaks down to a plane, leaving crisp planar facets",
            Tool::Scrape => "Shaves high spots off; Ctrl fills low spots instead",
            Tool::Inflate => "Pushes each point out along its own normal",
            Tool::Pinch => "Pulls the surface towards the brush centre",
            Tool::Crease => "Cuts a soft groove and pinches its edges",
            Tool::DamStandard => "Cuts a sharp, deep crease line (wrinkles, seams)",
            Tool::Layer => "Raises the surface to a fixed height per stroke",
            Tool::Grab => "Drags a region of the surface",
            Tool::Snakehook => "Pulls out horns and tentacles (best with dynamic topology)",
            Tool::Nudge => "Smears the surface sideways along the stroke",
            Tool::Mask => "Paints a mask that protects the surface; Ctrl erases",
        }
    }

    pub fn default_intensity(self) -> f32 {
        match self {
            Tool::Smooth => 0.6,
            Tool::Grab | Tool::Snakehook => 1.0,
            Tool::Mask => 0.8,
            Tool::Flatten | Tool::TrimDynamic | Tool::Scrape => 0.5,
            Tool::Layer => 0.3,
            Tool::Nudge => 0.6,
            Tool::ClayBuildup => 0.5,
            Tool::DamStandard => 0.45,
            _ => 0.4,
        }
    }

    /// Whether Ctrl / the negative toggle has a meaning for this tool.
    pub fn has_negative(self) -> bool {
        !matches!(self, Tool::Smooth | Tool::Flatten | Tool::Grab | Tool::Snakehook | Tool::Nudge | Tool::TrimDynamic)
    }

    /// Brushes that move geometry along the stroke instead of the normal.
    pub fn uses_motion(self) -> bool {
        matches!(self, Tool::Snakehook | Tool::Nudge)
    }
}

pub struct DabParams {
    pub tool: Tool,
    pub center: Vec3,
    pub radius: f32,
    pub intensity: f32,
    pub negative: bool,
    pub seed_tri: u32,
    /// Dynamic topology: target edge length (world units), or None.
    pub detail: Option<f32>,
    /// How far (world units, in the view plane) the brush moved since the
    /// previous dab. Used by Snakehook and Nudge.
    pub motion: Vec3,
}

/// One multiresolution level. The active level's mesh lives in
/// `Sculptor::mesh` (its slot here is `None`); every other level keeps its
/// topology here. Level k > 0 also keeps the Loop stencils that build it
/// from level k-1, and its detail: per-vertex offsets from that smooth
/// subdivision, in each vertex's local frame so they follow the surface.
#[derive(Clone)]
struct Level {
    mesh: Option<Mesh>,
    odd: Vec<[u32; 4]>,
    details: Vec<Vec3>,
}

/// Everything about the sculpt that a topology change can affect.
#[derive(Clone)]
struct State {
    mesh: Mesh,
    levels: Vec<Level>,
    level: usize,
}

fn mesh_bytes(m: &Mesh) -> usize {
    m.positions.len() * 100 + m.tris.len() * 40
}

/// A state to restore; applying one returns its inverse.
enum Undo {
    Verts { idx: Vec<u32>, pos: Vec<Vec3>, mask: Vec<f32> },
    State(Box<State>),
    /// Go back to this multiresolution level (level changes are exactly
    /// reversible, so nothing needs copying).
    Level(usize),
    /// Remove the top level (undoes a subdivision), then go to `then_level`.
    DropTop { then_level: usize },
    /// Put a removed top level back (redoes a subdivision).
    AddTop { level: Box<Level>, then_level: usize },
}

impl Undo {
    fn bytes(&self) -> usize {
        match self {
            Undo::Verts { idx, .. } => idx.len() * 20,
            Undo::Level(_) | Undo::DropTop { .. } => 16,
            Undo::AddTop { level, .. } => {
                level.mesh.as_ref().map_or(0, mesh_bytes) + level.odd.len() * 16 + level.details.len() * 12
            }
            Undo::State(s) => {
                mesh_bytes(&s.mesh)
                    + s.levels
                        .iter()
                        .map(|l| l.mesh.as_ref().map_or(0, mesh_bytes) + l.odd.len() * 16 + l.details.len() * 12)
                        .sum::<usize>()
            }
        }
    }
}

#[derive(Default)]
pub struct GpuDirty {
    pub topology: bool,
    pub range: Option<(u32, u32)>,
}

impl GpuDirty {
    fn add(&mut self, v: u32) {
        self.range = Some(match self.range {
            None => (v, v),
            Some((a, b)) => (a.min(v), b.max(v)),
        });
    }
    fn all(&mut self, n: usize) {
        if n > 0 {
            self.range = Some((0, n as u32 - 1));
        }
    }
}

struct GrabSet {
    verts: Vec<(u32, f32)>,
    orig: Vec<Vec3>,
    /// Linear part of the symmetry copy this set belongs to (identity for
    /// the original); the drag delta is transformed by it.
    xf: Mat3,
}

struct Stroke {
    mark: Marker,
    touched: Vec<u32>,
    old_pos: Vec<Vec3>,
    old_mask: Vec<f32>,
    grab: Vec<GrabSet>,
    /// Dynamic-topology strokes undo by restoring this whole snapshot.
    snapshot: Option<Box<State>>,
    modified: bool,
    /// Layer brush: each vertex's position when the stroke first reached it.
    layer_base: std::collections::HashMap<u32, Vec3>,
}

impl Stroke {
    #[inline]
    fn touch(&mut self, v: u32, mesh: &Mesh) {
        self.modified = true;
        if self.snapshot.is_some() {
            return;
        }
        if self.mark.mark(v) {
            self.touched.push(v);
            self.old_pos.push(mesh.positions[v as usize]);
            self.old_mask.push(mesh.mask[v as usize]);
        }
    }
}

const UNDO_BUDGET_BYTES: usize = 768 * 1024 * 1024;
pub const MAX_TRIANGLES: usize = 6_000_000;

pub struct Sculptor {
    pub mesh: Mesh,
    bvh: Bvh,
    bvh_dirty: bool,
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    vmark: Marker,
    vmark2: Marker,
    fmark: Marker,
    stroke: Option<Stroke>,
    region: Vec<(u32, f32)>,
    stack: Vec<u32>,
    scratch: Vec<Vec3>,
    levels: Vec<Level>,
    level: usize,
    /// Level to return to while a split subdivision is in progress.
    pending_level: Option<usize>,
    /// Counts changes, so the interface can tell when there's unsaved work.
    revision: u64,
    pub dirty: GpuDirty,
}

#[inline]
fn falloff(x: f32) -> f32 {
    if x >= 1.0 {
        0.0
    } else {
        let x3 = x * x * x;
        3.0 * x3 * x - 4.0 * x3 + 1.0
    }
}

pub fn mirror_x(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

impl Sculptor {
    pub fn new(mesh: Mesh) -> Self {
        let bvh = Bvh::build(&mesh);
        Self {
            mesh,
            bvh,
            bvh_dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            vmark: Marker::new(),
            vmark2: Marker::new(),
            fmark: Marker::new(),
            stroke: None,
            region: Vec::new(),
            stack: Vec::new(),
            scratch: Vec::new(),
            levels: vec![Level { mesh: None, odd: Vec::new(), details: Vec::new() }],
            level: 0,
            pending_level: None,
            revision: 0,
            dirty: GpuDirty { topology: true, range: None },
        }
    }

    pub fn tri_count(&self) -> usize {
        self.mesh.live_tri_count()
    }

    /// Number of multiresolution levels (1 = no levels).
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Active level, 0 = lowest.
    pub fn level(&self) -> usize {
        self.level
    }

    /// Dynamic topology needs a single level (like ZBrush's Sculptris Pro).
    pub fn dyntopo_allowed(&self) -> bool {
        self.levels.len() == 1
    }

    /// Triangle count of the highest level.
    pub fn top_tri_count(&self) -> usize {
        let top = self.levels.len() - 1;
        if top == self.level {
            self.mesh.tris.len()
        } else {
            self.levels[top].mesh.as_ref().map_or(0, |m| m.tris.len())
        }
    }

    fn snapshot(&self) -> Box<State> {
        Box::new(State { mesh: self.mesh.clone(), levels: self.levels.clone(), level: self.level })
    }

    fn restore(&mut self, s: Box<State>) -> Box<State> {
        let s = *s;
        let cur = Box::new(State {
            mesh: std::mem::replace(&mut self.mesh, s.mesh),
            levels: std::mem::replace(&mut self.levels, s.levels),
            level: std::mem::replace(&mut self.level, s.level),
        });
        self.topology_changed();
        cur
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() && self.stroke.is_none()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.stroke.is_none()
    }

    /// Changes every time the sculpt changes (strokes, undo, levels...).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn push_undo(&mut self, u: Undo) {
        self.revision += 1;
        self.undo.push(u);
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(|u| u.bytes()).sum();
        while self.undo.len() > 1 && (total > UNDO_BUDGET_BYTES || self.undo.len() > 100) {
            total -= self.undo.remove(0).bytes();
        }
    }

    /// Replaces the mesh (new primitive, import, subdivision), undoable.
    pub fn set_mesh(&mut self, mesh: Mesh) {
        self.set_mesh_built(mesh, None);
    }

    /// Like `set_mesh`, optionally with a picking hierarchy built elsewhere
    /// (e.g. a slice at a time by the interface). The old sculpt moves into
    /// the undo history instead of being copied.
    pub fn set_mesh_built(&mut self, mesh: Mesh, bvh: Option<Bvh>) {
        self.end_stroke();
        let old = State {
            mesh: std::mem::replace(&mut self.mesh, mesh),
            levels: std::mem::replace(
                &mut self.levels,
                vec![Level { mesh: None, odd: Vec::new(), details: Vec::new() }],
            ),
            level: std::mem::replace(&mut self.level, 0),
        };
        self.push_undo(Undo::State(Box::new(old)));
        self.install(bvh);
    }

    fn install(&mut self, bvh: Option<Bvh>) {
        match bvh {
            Some(b) => {
                self.bvh = b;
                self.bvh_dirty = false;
                self.dirty.topology = true;
                self.dirty.range = None;
            }
            None => self.topology_changed(),
        }
    }

    fn topology_changed(&mut self) {
        self.bvh = Bvh::build(&self.mesh);
        self.bvh_dirty = false;
        self.dirty.topology = true;
        self.dirty.range = None;
    }

    /// Adds a multiresolution level above the highest one (moving there).
    pub fn subdivide(&mut self) -> Result<(), String> {
        self.begin_subdivide()?;
        let (fine, odd) = self.mesh.subdivide_with_stencil(true);
        self.finish_subdivide(fine, odd, None)
    }

    /// First half of a subdivision that runs elsewhere (a background
    /// thread): records undo, moves to the top level and returns. The
    /// current `mesh` is then the one to subdivide; finish with
    /// `finish_subdivide` or roll back with `abort_subdivide`.
    pub fn begin_subdivide(&mut self) -> Result<(), String> {
        self.end_stroke();
        if self.top_tri_count() * 4 > MAX_TRIANGLES {
            return Err(format!("Subdividing would exceed {} M triangles", MAX_TRIANGLES / 1_000_000));
        }
        self.pending_level = Some(self.level);
        let moved = self.level + 1 < self.levels.len();
        while self.level + 1 < self.levels.len() {
            self.step_up();
        }
        if self.mesh.compact() || moved {
            self.topology_changed();
        }
        Ok(())
    }

    /// Installs a subdivided top level computed from the current mesh.
    pub fn finish_subdivide(&mut self, fine: Mesh, odd: Vec<[u32; 4]>, bvh: Option<Bvh>) -> Result<(), String> {
        if fine.positions.len() != self.mesh.positions.len() + odd.len() {
            self.abort_subdivide();
            return Err("The subdivided mesh doesn't match the current one".into());
        }
        let then_level = self.pending_level.take().unwrap_or(self.level);
        let details = vec![Vec3::ZERO; fine.positions.len()];
        let coarse = std::mem::replace(&mut self.mesh, fine);
        self.levels[self.level].mesh = Some(coarse);
        self.levels.push(Level { mesh: None, odd, details });
        self.level += 1;
        self.install(bvh);
        self.push_undo(Undo::DropTop { then_level });
        Ok(())
    }

    /// Undoes `begin_subdivide` (cancelled or failed): back to the level
    /// you were on, with no undo or redo entry.
    pub fn abort_subdivide(&mut self) {
        if let Some(l) = self.pending_level.take() {
            self.goto_level(l);
        }
    }

    /// Changes level without recording undo.
    fn goto_level(&mut self, target: usize) {
        let target = target.min(self.levels.len() - 1);
        if target == self.level {
            return;
        }
        while self.level < target {
            self.step_up();
        }
        while self.level > target {
            self.step_down();
        }
        self.topology_changed();
    }

    /// Every level for saving: (mesh, stencils from the level below, detail).
    pub fn project_levels(&self) -> (Vec<crate::project::LevelData<&Mesh>>, usize) {
        let levels = self
            .levels
            .iter()
            .enumerate()
            .map(|(i, l)| crate::project::LevelData {
                mesh: if i == self.level { &self.mesh } else { l.mesh.as_ref().expect("level mesh") },
                stencils: l.odd.clone(),
                details: l.details.clone(),
            })
            .collect();
        (levels, self.level)
    }

    /// Replaces the whole sculpt with loaded levels (the old sculpt moves
    /// into the undo history, so opening a project can be undone).
    pub fn load_levels(&mut self, mut levels: Vec<crate::project::LevelData<Mesh>>, active: usize, bvh: Option<Bvh>) {
        self.end_stroke();
        let active = active.min(levels.len() - 1);
        let mesh = std::mem::replace(&mut levels[active].mesh, Mesh::new(Vec::new(), Vec::new()));
        let new_levels: Vec<Level> = levels
            .into_iter()
            .enumerate()
            .map(|(i, l)| Level { mesh: (i != active).then_some(l.mesh), odd: l.stencils, details: l.details })
            .collect();
        let old = State {
            mesh: std::mem::replace(&mut self.mesh, mesh),
            levels: std::mem::replace(&mut self.levels, new_levels),
            level: std::mem::replace(&mut self.level, active),
        };
        self.push_undo(Undo::State(Box::new(old)));
        self.install(bvh);
    }

    /// Moves to another multiresolution level (undoable).
    pub fn set_level(&mut self, target: usize) {
        self.end_stroke();
        let target = target.min(self.levels.len() - 1);
        if target == self.level {
            return;
        }
        let from = self.level;
        self.goto_level(target);
        self.push_undo(Undo::Level(from));
    }

    /// Level k-1 -> k: smooth-subdivide the current shape and add back the
    /// stored detail in each vertex's local frame.
    fn step_up(&mut self) {
        let k = self.level + 1;
        let mut fine = self.levels[k].mesh.take().expect("level mesh");
        let s = self.mesh.loop_positions(&self.levels[k].odd);
        fine.positions = s;
        fine.compute_all_normals();
        // frames come from the smooth positions, so measure them all first
        let details = &self.levels[k].details;
        let offsets: Vec<Vec3> = (0..fine.positions.len())
            .map(|v| {
                let d = details[v];
                if d == Vec3::ZERO {
                    return Vec3::ZERO;
                }
                let (t, b, n) = fine.frame(v as u32);
                t * d.x + b * d.y + n * d.z
            })
            .collect();
        for (p, o) in fine.positions.iter_mut().zip(offsets) {
            *p += o;
        }
        let n = self.mesh.positions.len();
        fine.mask[..n].copy_from_slice(&self.mesh.mask);
        for (i, st) in self.levels[k].odd.iter().enumerate() {
            fine.mask[n + i] = (self.mesh.mask[st[0] as usize] + self.mesh.mask[st[1] as usize]) * 0.5;
        }
        fine.compute_all_normals();
        let coarse = std::mem::replace(&mut self.mesh, fine);
        self.levels[k - 1].mesh = Some(coarse);
        self.levels[k].mesh = None;
        self.level = k;
    }

    /// Level k -> k-1: the lower level takes the shared (even) vertices'
    /// current positions, and level k's detail is re-measured against the
    /// smooth subdivision of that, so going back up reproduces it exactly.
    fn step_down(&mut self) {
        let k = self.level;
        let mut coarse = self.levels[k - 1].mesh.take().expect("level mesh");
        let n = coarse.positions.len();
        coarse.positions.copy_from_slice(&self.mesh.positions[..n]);
        coarse.mask.copy_from_slice(&self.mesh.mask[..n]);
        coarse.compute_all_normals();
        let s = coarse.loop_positions(&self.levels[k].odd);
        let actual = std::mem::replace(&mut self.mesh.positions, s);
        self.mesh.compute_all_normals();
        let mut details = Vec::with_capacity(actual.len());
        for (v, p) in actual.iter().enumerate() {
            let off = *p - self.mesh.positions[v];
            let (t, b, nn) = self.mesh.frame(v as u32);
            details.push(Vec3::new(off.dot(t), off.dot(b), off.dot(nn)));
        }
        self.mesh.positions = actual;
        self.mesh.compute_all_normals();
        self.levels[k].details = details;
        let fine = std::mem::replace(&mut self.mesh, coarse);
        self.levels[k].mesh = Some(fine);
        self.levels[k - 1].mesh = None;
        self.level = k - 1;
    }

    /// Bakes the current level as the new lowest level (ZBrush "Del Lower").
    pub fn delete_lower(&mut self) {
        self.end_stroke();
        if self.level == 0 {
            return;
        }
        let snap = self.snapshot();
        self.push_undo(Undo::State(snap));
        self.levels.drain(0..self.level);
        self.levels[0].odd = Vec::new();
        self.levels[0].details = Vec::new();
        self.level = 0;
        self.topology_changed();
    }

    /// Removes every level above the current one (ZBrush "Del Higher").
    pub fn delete_higher(&mut self) {
        self.end_stroke();
        if self.level + 1 >= self.levels.len() {
            return;
        }
        let snap = self.snapshot();
        self.push_undo(Undo::State(snap));
        self.levels.truncate(self.level + 1);
        self.topology_changed();
    }

    /// Refit the picking hierarchy if the surface moved. Call once per frame.
    pub fn refresh_bvh(&mut self) {
        // dynamic topology adds faces the hierarchy doesn't index yet
        let extra = self.bvh.unindexed(&self.mesh);
        if extra > 20_000.max(self.bvh.indexed() / 3) {
            self.bvh = Bvh::build(&self.mesh);
            self.bvh_dirty = false;
        } else if self.bvh_dirty {
            self.bvh.refit(&self.mesh);
            self.bvh_dirty = false;
        }
    }

    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.bvh.raycast(&self.mesh, origin, dir)
    }

    /// `dynamic` = the stroke may change topology (undo takes a snapshot).
    pub fn begin_stroke(&mut self, dynamic: bool) {
        self.end_stroke();
        let snapshot = (dynamic && self.dyntopo_allowed()).then(|| self.snapshot());
        let mut mark = Marker::new();
        mark.begin(self.mesh.vertex_count());
        self.stroke = Some(Stroke {
            mark,
            touched: Vec::new(),
            old_pos: Vec::new(),
            old_mask: Vec::new(),
            grab: Vec::new(),
            snapshot,
            modified: false,
            layer_base: std::collections::HashMap::new(),
        });
    }

    pub fn end_stroke(&mut self) {
        if let Some(st) = self.stroke.take() {
            if let Some(snap) = st.snapshot {
                if st.modified {
                    if self.mesh.compact() {
                        self.topology_changed();
                    }
                    self.push_undo(Undo::State(snap));
                }
            } else if !st.touched.is_empty() {
                self.push_undo(Undo::Verts { idx: st.touched, pos: st.old_pos, mask: st.old_mask });
            }
        }
    }

    /// Flood-fills outward from the hit triangle, collecting vertices inside
    /// the brush sphere together with their falloff weight.
    fn collect_region(&mut self, center: Vec3, radius: f32, seed_tri: u32, use_mask: bool) {
        let Sculptor { mesh, vmark, region, stack, .. } = self;
        region.clear();
        stack.clear();
        if seed_tri as usize >= mesh.tris.len() || !mesh.is_alive(seed_tri) || radius <= 0.0 {
            return;
        }
        vmark.begin(mesh.vertex_count());
        let seed = mesh.tris[seed_tri as usize];
        for &v in &seed {
            if vmark.mark(v) {
                stack.push(v);
            }
        }
        let r2 = radius * radius;
        let inv_r = 1.0 / radius;
        while let Some(v) = stack.pop() {
            let d2 = mesh.positions[v as usize].distance_squared(center);
            let inside = d2 < r2;
            if inside {
                let mut w = falloff(d2.sqrt() * inv_r);
                if use_mask {
                    w *= 1.0 - mesh.mask[v as usize];
                }
                region.push((v, w));
            }
            if inside || seed.contains(&v) {
                for &u in mesh.neighbors(v) {
                    if vmark.mark(u) {
                        stack.push(u);
                    }
                }
            }
        }
    }

    fn after_deform(&mut self, verts_from_region: bool, extra: Option<&[u32]>) {
        let mut list: Vec<u32> = Vec::new();
        let verts: &[u32] = if verts_from_region {
            list.extend(self.region.iter().map(|r| r.0));
            &list
        } else {
            extra.unwrap_or(&[])
        };
        for &v in verts {
            self.dirty.add(v);
        }
        let Sculptor { mesh, fmark, vmark2, .. } = self;
        mesh.update_normals(verts, fmark, vmark2);
        // normals of the one-ring changed too
        for &f in verts.iter().flat_map(|&v| mesh.vert_faces(v)) {
            for &u in &mesh.tris[f as usize] {
                self.dirty.add(u);
            }
        }
        self.bvh_dirty = true;
    }

    pub fn dab(&mut self, p: &DabParams) {
        if p.tool == Tool::Grab {
            return;
        }
        let mut seed = p.seed_tri;
        if let Some(detail) = p.detail {
            let dynamic_stroke = self.stroke.as_ref().is_some_and(|s| s.snapshot.is_some());
            if p.tool != Tool::Mask && dynamic_stroke {
                seed = self.remesh(seed, p.center, p.radius, detail);
            }
        }
        self.collect_region(p.center, p.radius, seed, p.tool != Tool::Mask);
        if self.region.is_empty() {
            return;
        }
        if let Some(st) = self.stroke.as_mut() {
            for &(v, _) in &self.region {
                st.touch(v, &self.mesh);
            }
        }

        let s = if p.negative && p.tool.has_negative() { -1.0 } else { 1.0 };
        let r = p.radius;
        let i = p.intensity.clamp(0.0, 1.0);

        // weighted area normal & centre
        let mut an = Vec3::ZERO;
        let mut ac = Vec3::ZERO;
        let mut wsum = 0.0;
        for &(v, w) in &self.region {
            let w = w + 1e-4;
            an += self.mesh.normals[v as usize] * w;
            ac += self.mesh.positions[v as usize] * w;
            wsum += w;
        }
        let an = an.try_normalize().unwrap_or(Vec3::Y);
        let ac = ac / wsum;

        let mesh = &mut self.mesh;
        let region = &self.region;
        match p.tool {
            Tool::Draw => {
                let d = an * (s * 0.08 * r * i);
                for &(v, w) in region {
                    mesh.positions[v as usize] += d * w;
                }
            }
            Tool::Inflate => {
                let k = s * 0.06 * r * i;
                for &(v, w) in region {
                    let n = mesh.normals[v as usize];
                    mesh.positions[v as usize] += n * (k * w);
                }
            }
            Tool::Smooth => {
                self.scratch.clear();
                for &(v, w) in region {
                    let p0 = mesh.positions[v as usize];
                    let mut sum = Vec3::ZERO;
                    let mut c = 0.0;
                    let boundary = mesh.boundary[v as usize];
                    for &u in mesh.neighbors(v) {
                        if boundary && !mesh.boundary[u as usize] {
                            continue;
                        }
                        sum += mesh.positions[u as usize];
                        c += 1.0;
                    }
                    let target = if c > 0.0 { sum / c } else { p0 };
                    self.scratch.push(p0.lerp(target, (w * i).clamp(0.0, 1.0)));
                }
                for (k, &(v, _)) in region.iter().enumerate() {
                    mesh.positions[v as usize] = self.scratch[k];
                }
            }
            Tool::Flatten => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    mesh.positions[v as usize] = pv - an * (d * w * i * 0.5);
                }
            }
            Tool::Clay => {
                let plane = ac + an * (s * r * 0.1);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - plane).dot(an);
                    if d * s < 0.0 {
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.45);
                    }
                }
            }
            Tool::Pinch => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (s * w * i * 0.3);
                }
            }
            Tool::Crease => {
                let push = an * (-s * 0.05 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (w * i * 0.25) + push * w;
                }
            }
            Tool::DamStandard => {
                // deep, narrow groove: cubic falloff for the cut, strong pinch
                let push = an * (-s * 0.07 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (w * i * 0.4) + push * (w * w * w);
                }
            }
            Tool::ClayBuildup => {
                let plane = ac + an * (s * r * 0.15);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - plane).dot(an);
                    if d * s < 0.0 {
                        // square-edged strips: a harder falloff than Clay
                        let w = w.sqrt();
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.6);
                    }
                }
            }
            Tool::TrimDynamic => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    if d > 0.0 {
                        let w = (w * 2.0).min(1.0);
                        mesh.positions[v as usize] = pv - an * (d * w * i);
                    }
                }
            }
            Tool::Scrape => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    if d * s > 0.0 {
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.5);
                    }
                }
            }
            Tool::Layer => {
                // raise towards a fixed height above where the stroke found
                // each point, so overlapping dabs don't pile up
                let height = s * r * 0.12 * i;
                let bases = self.stroke.as_mut().map(|st| &mut st.layer_base);
                let mut scratch = std::collections::HashMap::new();
                let bases = bases.unwrap_or(&mut scratch);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let base = *bases.entry(v).or_insert(pv);
                    let current = (pv - base).dot(an);
                    let target = height * w.min(1.0);
                    let step = target - current;
                    if step * s > 0.0 {
                        mesh.positions[v as usize] = pv + an * (step * 0.5);
                    }
                }
            }
            Tool::Snakehook => {
                let m = p.motion * i;
                for &(v, w) in region {
                    mesh.positions[v as usize] += m * w;
                }
            }
            Tool::Nudge => {
                let mut m = p.motion;
                m -= an * m.dot(an);
                let m = m * (i * 0.7);
                for &(v, w) in region {
                    mesh.positions[v as usize] += m * w;
                }
            }
            Tool::Mask => {
                for &(v, w) in region {
                    let m = &mut mesh.mask[v as usize];
                    *m = (*m + s * i * w * 0.5).clamp(0.0, 1.0);
                }
                for &(v, _) in region {
                    self.dirty.add(v);
                }
                return;
            }
            Tool::Grab => unreachable!(),
        }
        self.after_deform(true, None);
    }

    /// Dynamic topology for one dab. Returns a live face to start the brush
    /// region from (the hit face may have been collapsed away).
    fn remesh(&mut self, seed: u32, center: Vec3, radius: f32, detail: f32) -> u32 {
        let mut changed = Vec::new();
        let stats = {
            let Sculptor { mesh, fmark, vmark, .. } = self;
            remesh_region(mesh, seed, center, radius, detail, fmark, vmark, &mut changed)
        };
        if stats.splits + stats.collapses == 0 {
            return seed;
        }
        if let Some(st) = self.stroke.as_mut() {
            st.modified = true;
            // Layer heights: new vertices start between their edge's ends
            if !st.layer_base.is_empty() {
                let pos = &self.mesh.positions;
                let base = |b: &std::collections::HashMap<u32, Vec3>, v: u32| {
                    b.get(&v).copied().unwrap_or(pos[v as usize])
                };
                for &(nv, a, b) in &stats.split_log {
                    let mid = (base(&st.layer_base, a) + base(&st.layer_base, b)) * 0.5;
                    st.layer_base.insert(nv, mid);
                }
                for &(a, b) in &stats.collapse_log {
                    let mid = (base(&st.layer_base, a) + base(&st.layer_base, b)) * 0.5;
                    st.layer_base.insert(a, mid);
                    st.layer_base.remove(&b);
                }
            }
        }
        {
            let Sculptor { mesh, fmark, vmark2, .. } = self;
            mesh.update_normals(&changed, fmark, vmark2);
        }
        self.dirty.topology = true;
        self.bvh_dirty = true;
        if self.mesh.is_alive(seed) {
            return seed;
        }
        changed
            .iter()
            .filter_map(|&v| self.mesh.vert_faces(v).first().map(|&f| (self.mesh.positions[v as usize].distance_squared(center), f)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(seed, |(_, f)| f)
    }

    /// Starts a grab region (call once per symmetry side at stroke start).
    pub fn grab_begin(&mut self, center: Vec3, radius: f32, seed_tri: u32, xf: Mat3) {
        self.collect_region(center, radius, seed_tri, true);
        let verts = self.region.clone();
        let orig = verts.iter().map(|&(v, _)| self.mesh.positions[v as usize]).collect();
        if let Some(st) = self.stroke.as_mut() {
            st.grab.push(GrabSet { verts, orig, xf });
        }
    }

    /// Moves grabbed vertices by `delta` (world units) from where they started.
    pub fn grab_update(&mut self, delta: Vec3) {
        let Some(mut st) = self.stroke.take() else { return };
        let mut moved: Vec<u32> = Vec::new();
        for set in &st.grab {
            let d = set.xf * delta;
            for (k, &(v, w)) in set.verts.iter().enumerate() {
                st.modified = true;
                if st.snapshot.is_none() && st.mark.mark(v) {
                    st.touched.push(v);
                    st.old_pos.push(self.mesh.positions[v as usize]);
                    st.old_mask.push(self.mesh.mask[v as usize]);
                }
                self.mesh.positions[v as usize] = set.orig[k] + d * w;
                moved.push(v);
            }
        }
        self.stroke = Some(st);
        self.after_deform(false, Some(&moved));
    }

    fn apply(&mut self, e: Undo) -> Undo {
        self.revision += 1;
        match e {
            Undo::Verts { idx, pos, mask } => {
                let mut cur_pos = Vec::with_capacity(idx.len());
                let mut cur_mask = Vec::with_capacity(idx.len());
                for (k, &v) in idx.iter().enumerate() {
                    cur_pos.push(self.mesh.positions[v as usize]);
                    cur_mask.push(self.mesh.mask[v as usize]);
                    self.mesh.positions[v as usize] = pos[k];
                    self.mesh.mask[v as usize] = mask[k];
                }
                self.after_deform(false, Some(&idx));
                Undo::Verts { idx, pos: cur_pos, mask: cur_mask }
            }
            Undo::State(s) => Undo::State(self.restore(s)),
            Undo::Level(l) => {
                let cur = self.level;
                self.goto_level(l);
                Undo::Level(cur)
            }
            Undo::DropTop { then_level } => {
                let cur = self.level;
                let top = self.levels.len() - 1;
                self.goto_level(top);
                let coarse = self.levels[top - 1].mesh.take().expect("level mesh");
                let fine = std::mem::replace(&mut self.mesh, coarse);
                let mut level = self.levels.pop().expect("top level");
                level.mesh = Some(fine);
                self.level = top - 1;
                self.topology_changed();
                self.goto_level(then_level);
                Undo::AddTop { level: Box::new(level), then_level: cur }
            }
            Undo::AddTop { level, then_level } => {
                let cur = self.level;
                let top = self.levels.len() - 1;
                self.goto_level(top);
                let mut level = *level;
                let fine = level.mesh.take().expect("level mesh");
                let coarse = std::mem::replace(&mut self.mesh, fine);
                self.levels[top].mesh = Some(coarse);
                self.levels.push(level);
                self.level = top + 1;
                self.topology_changed();
                self.goto_level(then_level);
                Undo::DropTop { then_level: cur }
            }
        }
    }

    pub fn undo(&mut self) {
        if !self.can_undo() {
            return;
        }
        let e = self.undo.pop().unwrap();
        let inv = self.apply(e);
        self.redo.push(inv);
    }

    pub fn redo(&mut self) {
        if !self.can_redo() {
            return;
        }
        let e = self.redo.pop().unwrap();
        let inv = self.apply(e);
        self.undo.push(inv);
    }

    fn mask_op(&mut self, f: impl Fn(f32) -> f32) {
        self.end_stroke();
        let n = self.mesh.vertex_count();
        let idx: Vec<u32> = (0..n as u32).collect();
        let old = self.mesh.mask.clone();
        for m in &mut self.mesh.mask {
            *m = f(*m);
        }
        self.push_undo(Undo::Verts { idx, pos: self.mesh.positions.clone(), mask: old });
        self.dirty.all(n);
    }

    pub fn clear_mask(&mut self) {
        self.mask_op(|_| 0.0);
    }
    pub fn invert_mask(&mut self) {
        self.mask_op(|m| 1.0 - m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_hit(s: &Sculptor) -> Hit {
        s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap()
    }

    fn dab(s: &mut Sculptor, tool: Tool, negative: bool) {
        let h = front_hit(s);
        s.dab(&DabParams { tool, center: h.point, radius: 0.4, intensity: 1.0, negative, seed_tri: h.tri, detail: None, motion: Vec3::new(0.05, 0.0, 0.0) });
        s.refresh_bvh();
    }

    #[test]
    fn draw_raises_and_undo_restores() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        for _ in 0..5 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let after = front_hit(&s).point.z;
        assert!(after > before + 0.05, "{before} -> {after}");
        s.undo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - before).abs() < 1e-5);
        s.redo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - after).abs() < 1e-5);
    }

    #[test]
    fn negative_draw_digs() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        dab(&mut s, Tool::Draw, true);
        s.end_stroke();
        assert!(front_hit(&s).point.z < before);
    }

    #[test]
    fn mask_blocks_draw() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        s.begin_stroke(false);
        for _ in 0..10 {
            dab(&mut s, Tool::Mask, false);
        }
        s.end_stroke();
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        dab(&mut s, Tool::Draw, false);
        s.end_stroke();
        assert!((front_hit(&s).point.z - before).abs() < 0.01);
    }

    #[test]
    fn smooth_and_others_stay_finite() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        for t in Tool::ALL {
            s.begin_stroke(false);
            dab(&mut s, t, false);
            dab(&mut s, t, true);
            s.end_stroke();
        }
        assert!(s.mesh.positions.iter().all(|p| p.is_finite()));
        assert!(s.mesh.normals.iter().all(|n| n.is_finite()));
    }

    fn top_z(s: &Sculptor) -> f32 {
        s.mesh.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max)
    }

    #[test]
    fn trim_and_scrape_never_raise() {
        for tool in [Tool::TrimDynamic, Tool::Scrape] {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let before = top_z(&s);
            s.begin_stroke(false);
            for _ in 0..5 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            assert!(top_z(&s) < before - 0.005, "{tool:?} should cut the dome down");
        }
    }

    #[test]
    fn layer_height_is_capped_per_stroke() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        for _ in 0..40 {
            dab(&mut s, Tool::Layer, false);
        }
        s.end_stroke();
        let rise = front_hit(&s).point.z - before;
        assert!(rise > 0.02 && rise <= 0.4 * 0.12 + 1e-3, "{rise}");
    }

    #[test]
    fn layer_with_dynamic_topology_stays_level() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let before = front_hit(&s).point.z;
        s.begin_stroke(true);
        for _ in 0..30 {
            let h = front_hit(&s);
            s.dab(&DabParams {
                tool: Tool::Layer,
                center: h.point,
                radius: 0.4,
                intensity: 1.0,
                negative: false,
                seed_tri: h.tri,
                detail: Some(0.03),
                motion: Vec3::ZERO,
            });
            s.refresh_bvh();
        }
        s.end_stroke();
        // no point may rise above the layer height (no spikes from new vertices)
        let max_rise = s.mesh.positions.iter().map(|p| p.length() - 1.0).fold(f32::MIN, f32::max);
        assert!(max_rise <= 0.4 * 0.12 + 0.01, "{max_rise}");
        assert!(front_hit(&s).point.z > before + 0.02);
    }

    #[test]
    fn dam_standard_digs_deeper_than_crease() {
        let depth = |tool| {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let before = front_hit(&s).point.z;
            s.begin_stroke(false);
            for _ in 0..3 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            before - front_hit(&s).point.z
        };
        assert!(depth(Tool::DamStandard) > depth(Tool::Crease));
    }

    #[test]
    fn snakehook_and_nudge_follow_motion() {
        for tool in [Tool::Snakehook, Tool::Nudge] {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let cx: f32 = s.mesh.positions.iter().map(|p| p.x).sum();
            s.begin_stroke(false);
            for _ in 0..4 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            let after: f32 = s.mesh.positions.iter().map(|p| p.x).sum();
            assert!(after > cx + 0.05, "{tool:?} should move points along +x");
        }
    }

    #[test]
    fn grab_moves_with_mirror() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let h = s.raycast(Vec3::new(0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        let hm = s.raycast(Vec3::new(-0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        s.begin_stroke(false);
        s.grab_begin(h.point, 0.3, h.tri, Mat3::IDENTITY);
        s.grab_begin(hm.point, 0.3, hm.tri, Mat3::from_diagonal(Vec3::new(-1.0, 1.0, 1.0)));
        s.grab_update(Vec3::new(0.5, 0.0, 0.0));
        s.end_stroke();
        let maxx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let minx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!(maxx > 1.2 && minx < -1.2, "{minx} {maxx}");
    }

    fn closed_manifold(m: &Mesh) -> bool {
        let mut count = std::collections::HashMap::new();
        for t in m.tris.iter().filter(|t| t[0] != crate::mesh::NONE) {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        count.values().all(|&c| c == 2)
    }

    #[test]
    fn dynamic_stroke_adds_detail_and_undoes() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let n0 = s.tri_count();
        let before = front_hit(&s).point.z;
        s.begin_stroke(true);
        for _ in 0..6 {
            let h = front_hit(&s);
            s.dab(&DabParams {
                tool: Tool::Draw,
                center: h.point,
                radius: 0.4,
                intensity: 1.0,
                negative: false,
                seed_tri: h.tri,
                detail: Some(0.03),
                motion: Vec3::ZERO,
            });
            s.refresh_bvh();
        }
        s.end_stroke();
        assert!(s.tri_count() > n0 * 2, "{} -> {}", n0, s.tri_count());
        assert!(closed_manifold(&s.mesh));
        assert!(s.mesh.tris.iter().all(|t| t[0] != crate::mesh::NONE), "compacted after the stroke");
        assert!(front_hit(&s).point.z > before + 0.05);
        s.undo();
        assert_eq!(s.tri_count(), n0);
        s.redo();
        assert!(s.tri_count() > n0 * 2);
    }

    #[test]
    fn dynamic_topology_needs_one_level() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        assert!(!s.dyntopo_allowed());
        let n = s.tri_count();
        s.begin_stroke(true);
        let h = front_hit(&s);
        s.dab(&DabParams { tool: Tool::Draw, center: h.point, radius: 0.4, intensity: 1.0, negative: false, seed_tri: h.tri, detail: Some(0.01), motion: Vec3::ZERO });
        s.end_stroke();
        assert_eq!(s.tri_count(), n);
        s.delete_lower();
        assert!(s.dyntopo_allowed());
        assert_eq!(s.tri_count(), n);
    }

    #[test]
    fn multires_round_trip_keeps_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        // sculpt fine detail at the top level
        s.begin_stroke(false);
        for _ in 0..4 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let top = s.mesh.positions.clone();
        s.set_level(0);
        assert_eq!(s.tri_count(), 320);
        s.set_level(2);
        let err = top.iter().zip(&s.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "detail lost: {err}");
    }

    #[test]
    fn multires_low_level_edit_moves_high_level() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..4 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let fine_detail = s.mesh.positions.clone();
        s.set_level(0);
        // move the whole base mesh right
        for p in &mut s.mesh.positions {
            p.x += 0.5;
        }
        s.mesh.compute_all_normals();
        s.set_level(1);
        let avg: f32 = s.mesh.positions.iter().zip(&fine_detail).map(|(a, b)| a.x - b.x).sum::<f32>()
            / fine_detail.len() as f32;
        assert!((avg - 0.5).abs() < 0.02, "{avg}");
        // and the sculpted bump survives the move (relative to the shifted shape)
        let bump = s.mesh.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
        assert!(bump > 1.05, "{bump}");
        s.undo();
        assert_eq!(s.level(), 0);
    }

    #[test]
    fn split_subdivide_matches_and_aborts_cleanly() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.set_level(0);
        let undo_depth = s.undo.len();
        s.begin_subdivide().unwrap();
        assert_eq!(s.level(), 1, "moves to the top level first");
        s.abort_subdivide();
        assert_eq!((s.level(), s.level_count(), s.undo.len()), (0, 2, undo_depth));
        assert!(!s.can_redo());
        s.begin_subdivide().unwrap();
        let (fine, odd) = s.mesh.subdivide_with_stencil(true);
        let bytes = crate::wire::encode(&fine, &odd);
        let (fine, odd) = crate::wire::decode(&bytes).unwrap();
        s.finish_subdivide(fine, odd, None).unwrap();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        s.set_level(0);
        s.set_level(2);
        s.undo();
        s.undo();
        s.undo();
        assert_eq!(s.level_count(), 2);
        assert_eq!(s.level(), 0, "back where the subdivision started");
        s.redo();
        assert_eq!((s.level(), s.level_count()), (2, 3));
    }

    #[test]
    fn level_and_subdivide_undo_round_trip_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..3 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let sculpted = s.mesh.positions.clone();
        s.subdivide().unwrap();
        s.undo(); // drop level 3
        assert_eq!((s.level(), s.level_count()), (1, 2));
        let err = sculpted.iter().zip(&s.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
        s.redo();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        s.set_level(0);
        s.undo();
        assert_eq!(s.level(), 2);
        s.redo();
        assert_eq!(s.level(), 0);
    }

    #[test]
    fn project_round_trip_keeps_levels_and_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..3 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        s.set_level(1);
        let (levels, active) = s.project_levels();
        let bytes = crate::project::encode(&levels, active, r#"{"hello":1}"#);
        let p = crate::project::decode(&bytes).unwrap();
        assert_eq!(p.settings_json, r#"{"hello":1}"#);
        let loaded: Vec<crate::project::LevelData<Mesh>> = p
            .levels
            .into_iter()
            .map(|l| {
                let (b, st) = l.mesh.into_builder();
                crate::project::LevelData { mesh: b.finish(), stencils: st, details: l.details }
            })
            .collect();
        let mut t = Sculptor::new(Mesh::icosphere(1));
        let rev = t.revision();
        t.load_levels(loaded, p.active, None);
        assert!(t.revision() > rev);
        assert_eq!((t.level(), t.level_count()), (1, 3));
        s.set_level(2);
        t.set_level(2);
        let err = s.mesh.positions.iter().zip(&t.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-5, "{err}");
        t.undo(); // back to level 1
        t.undo(); // opening the project is undoable too
        assert_eq!(t.level_count(), 1);
        assert!(crate::project::decode(&bytes[..30]).is_err());
    }

    #[test]
    fn subdivide_undo() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        let n = s.tri_count();
        s.subdivide().unwrap();
        assert_eq!(s.tri_count(), n * 4);
        s.undo();
        assert_eq!(s.tri_count(), n);
    }
}
