//! Surface detail tools: sculpt layers, the morph target, Surface Noise,
//! projecting detail from other objects (Project All) and reprojecting
//! higher levels after a topology change. Also the sculptor's "extras" blob
//! stored in project files.

use crate::bvh::Bvh;
use crate::mesh::Mesh;
use crate::sculpt::{Sculptor, Undo};
use glam::Vec3;

/// A sculpt layer: an offset for every vertex of one level, shown at a
/// strength (0 = off, 1 = as sculpted, more exaggerates, less than 0 inverts).
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub strength: f32,
    pub level: usize,
    pub delta: Vec<Vec3>,
}

/// What the interface shows for each layer.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerInfo {
    pub name: String,
    pub visible: bool,
    pub strength: f32,
    pub level: usize,
    pub recording: bool,
    /// On the active level (layers on other levels can't be changed there).
    pub usable: bool,
}

/// Layers and the morph target, kept with the sculpt (and in its undo states).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    pub layers: Vec<Layer>,
    /// The layer being recorded into, and the positions when recording began.
    pub recording: Option<(usize, Vec<Vec3>)>,
    /// Stored shape: (level, positions).
    pub morph: Option<(usize, Vec<Vec3>)>,
    /// When the sculpt is a posed mannequin: its pose (so undo can bring
    /// back the pose with the mesh).
    pub pose: Vec<f32>,
}

impl Extra {
    pub(crate) fn bytes(&self) -> usize {
        self.layers.iter().map(|l| l.delta.len() * 12 + 64).sum::<usize>()
            + self.recording.as_ref().map_or(0, |r| r.1.len() * 12)
            + self.morph.as_ref().map_or(0, |m| m.1.len() * 12)
            + self.pose.len() * 4
    }

    /// The levels below `cut` were deleted: renumber, dropping what was on them.
    pub(crate) fn shift_levels(&mut self, cut: usize) {
        self.recording = None;
        self.layers.retain(|l| l.level >= cut);
        for l in &mut self.layers {
            l.level -= cut;
        }
        self.morph = self.morph.take().filter(|m| m.0 >= cut).map(|(l, p)| (l - cut, p));
    }

    /// The levels above `keep` were deleted.
    pub(crate) fn drop_above(&mut self, keep: usize) {
        self.recording = None;
        self.layers.retain(|l| l.level <= keep);
        self.morph = self.morph.take().filter(|m| m.0 <= keep);
    }

    pub(crate) fn morph_at(&self, level: usize, n: usize) -> Option<&[Vec3]> {
        self.morph.as_ref().filter(|m| m.0 == level && m.1.len() == n).map(|m| m.1.as_slice())
    }
}

// ------------------------------------------------------------------ noise

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoiseKind {
    /// Soft, rolling bumps (fractal value noise).
    Smooth,
    /// Sharp ridges, like veins or rock strata.
    Ridged,
    /// Rounded cells, like pebbles or scales.
    Cells,
    /// Fine crackle lines between cells.
    Crackle,
}

impl NoiseKind {
    pub const ALL: [NoiseKind; 4] = [NoiseKind::Smooth, NoiseKind::Ridged, NoiseKind::Cells, NoiseKind::Crackle];
    pub fn name(self) -> &'static str {
        match self {
            NoiseKind::Smooth => "Smooth",
            NoiseKind::Ridged => "Ridged",
            NoiseKind::Cells => "Cells",
            NoiseKind::Crackle => "Crackle",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NoiseSettings {
    pub kind: NoiseKind,
    /// Feature size as a fraction of the model size.
    pub scale: f32,
    /// Height as a fraction of the model size (negative carves in).
    pub strength: f32,
    pub octaves: u32,
    pub seed: u32,
    /// Masked areas are left alone.
    pub use_mask: bool,
}

impl Default for NoiseSettings {
    fn default() -> Self {
        Self { kind: NoiseKind::Smooth, scale: 0.08, strength: 0.005, octaves: 4, seed: 1, use_mask: true }
    }
}

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

fn rnd(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn value3(p: Vec3, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let v = |dx: i32, dy: i32, dz: i32| rnd(hash3(x + dx, y + dy, z + dz, seed));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(v(0, 0, 0), v(1, 0, 0), u.x);
    let x10 = lerp(v(0, 1, 0), v(1, 1, 0), u.x);
    let x01 = lerp(v(0, 0, 1), v(1, 0, 1), u.x);
    let x11 = lerp(v(0, 1, 1), v(1, 1, 1), u.x);
    lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z) * 2.0 - 1.0
}

/// Distances to the nearest and second-nearest feature point (Worley noise).
fn worley(p: Vec3, seed: u32) -> (f32, f32) {
    let i = p.floor();
    let (mut d1, mut d2) = (f32::MAX, f32::MAX);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = i + Vec3::new(dx as f32, dy as f32, dz as f32);
                let h = hash3(c.x as i32, c.y as i32, c.z as i32, seed);
                let q = c + Vec3::new(rnd(h), rnd(h.rotate_left(11)), rnd(h.rotate_left(22)));
                let d = q.distance(p);
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                } else if d < d2 {
                    d2 = d;
                }
            }
        }
    }
    (d1, d2)
}

/// Noise value around 0 (roughly -1…1) at `p` in feature-size units.
pub fn noise_at(s: &NoiseSettings, p: Vec3) -> f32 {
    let oct = s.octaves.clamp(1, 8);
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..oct {
        let q = p * freq + Vec3::splat(o as f32 * 17.13);
        let seed = s.seed.wrapping_add(o * 101);
        let n = match s.kind {
            NoiseKind::Smooth => value3(q, seed),
            NoiseKind::Ridged => 1.0 - 2.0 * value3(q, seed).abs(),
            NoiseKind::Cells => {
                let (d1, _) = worley(q, seed);
                1.0 - 2.0 * (d1 * 1.2).min(1.0)
            }
            NoiseKind::Crackle => {
                let (d1, d2) = worley(q, seed);
                // lines where the two nearest cells meet
                let e = ((d2 - d1) * 6.0).min(1.0);
                e * 2.0 - 1.0
            }
        };
        sum += n * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

// ------------------------------------------------------------------ sculptor

const NOISE_LABEL: &str = "Surface noise";
const STRENGTH_LABEL: &str = "Layer strength";

impl Sculptor {
    /// Records the layers, morph and positions as one undo step, before a change.
    fn push_extra(&mut self, label: &str) {
        self.end_stroke();
        self.set_label(label);
        self.push_undo(Undo::Extra(Box::new((self.extra.clone(), self.mesh.positions.clone()))));
    }

    /// Ends recording into a layer: its offset takes everything sculpted
    /// since recording began (as part of the current undo step).
    fn fold_recording(&mut self) {
        if let Some((i, base)) = self.extra.recording.take() {
            let pos = &self.mesh.positions;
            if let Some(l) = self.extra.layers.get_mut(i) {
                if l.delta.len() == pos.len() && base.len() == pos.len() && l.strength.abs() > 1e-4 {
                    let inv = 1.0 / l.strength;
                    for ((d, p), b) in l.delta.iter_mut().zip(pos).zip(&base) {
                        *d += (*p - *b) * inv;
                    }
                }
            }
        }
    }

    /// Stops recording into a layer (an undo step of its own).
    pub fn stop_recording(&mut self) {
        if self.extra.recording.is_some() {
            self.push_extra("Stop recording layer");
            self.fold_recording();
        }
    }

    fn layer_usable(&self, l: &Layer) -> bool {
        l.level == self.level && l.delta.len() == self.mesh.positions.len()
    }

    pub fn layers(&self) -> Vec<LayerInfo> {
        self.extra
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| LayerInfo {
                name: l.name.clone(),
                visible: l.visible,
                strength: l.strength,
                level: l.level,
                recording: self.extra.recording.as_ref().is_some_and(|r| r.0 == i),
                usable: self.layer_usable(l),
            })
            .collect()
    }

    /// Adds every visible layer's offset (scaled) to or from the positions.
    fn shift_by(&mut self, delta: &[Vec3], k: f32) {
        if delta.len() != self.mesh.positions.len() || k == 0.0 {
            return;
        }
        for (p, d) in self.mesh.positions.iter_mut().zip(delta) {
            *p += *d * k;
        }
        self.after_bulk_move();
    }

    fn after_bulk_move(&mut self) {
        self.mesh.compute_all_normals();
        self.dirty.all(self.mesh.positions.len());
        self.topology_changed_soft();
    }

    /// A new empty layer on the active level, recording.
    pub fn add_layer(&mut self, name: &str) -> usize {
        self.push_extra("New layer");
        self.fold_recording();
        let n = self.mesh.positions.len();
        self.extra.layers.push(Layer { name: name.into(), visible: true, strength: 1.0, level: self.level, delta: vec![Vec3::ZERO; n] });
        let i = self.extra.layers.len() - 1;
        self.extra.recording = Some((i, self.mesh.positions.clone()));
        i
    }

    /// Starts recording strokes into layer `i` (shown at strength 1 if it was off).
    pub fn record_layer(&mut self, i: usize) -> bool {
        let Some(l) = self.extra.layers.get(i) else { return false };
        if !self.layer_usable(l) || self.extra.recording.as_ref().is_some_and(|r| r.0 == i) {
            return false;
        }
        self.push_extra("Record layer");
        self.fold_recording();
        let l = &self.extra.layers[i];
        let (delta, vis, s) = (l.delta.clone(), l.visible, l.strength);
        // record at a usable strength
        let shown = if vis { s } else { 0.0 };
        let target = if s.abs() < 0.05 { 1.0 } else { s };
        self.shift_by(&delta, target - shown);
        let l = &mut self.extra.layers[i];
        l.visible = true;
        l.strength = target;
        self.extra.recording = Some((i, self.mesh.positions.clone()));
        true
    }

    pub fn set_layer_visible(&mut self, i: usize, visible: bool) {
        let Some(l) = self.extra.layers.get(i) else { return };
        if !self.layer_usable(l) || l.visible == visible {
            return;
        }
        self.push_extra(if visible { "Show layer" } else { "Hide layer" });
        self.fold_recording();
        let l = &self.extra.layers[i];
        let (delta, s) = (l.delta.clone(), l.strength);
        self.shift_by(&delta, if visible { s } else { -s });
        self.extra.layers[i].visible = visible;
    }

    /// Changes a layer's strength; repeated calls (a slider drag) make one undo step.
    pub fn set_layer_strength(&mut self, i: usize, strength: f32) {
        let Some(l) = self.extra.layers.get(i) else { return };
        if !self.layer_usable(l) {
            return;
        }
        let label = format!("{STRENGTH_LABEL}: {}", l.name);
        let merge = self.undo_labels.last() == Some(&label) && self.extra.recording.is_none() && self.stroke.is_none();
        if !merge {
            self.push_extra(&label);
            self.fold_recording();
        }
        let l = &self.extra.layers[i];
        let (delta, old, vis) = (l.delta.clone(), l.strength, l.visible);
        if vis {
            self.shift_by(&delta, strength - old);
        }
        self.extra.layers[i].strength = strength;
        self.bump_revision();
    }

    pub fn rename_layer(&mut self, i: usize, name: &str) {
        if let Some(l) = self.extra.layers.get_mut(i) {
            l.name = name.into();
            self.bump_revision();
        }
    }

    /// Removes a layer and its effect.
    pub fn delete_layer(&mut self, i: usize) {
        if i >= self.extra.layers.len() {
            return;
        }
        self.push_extra("Delete layer");
        self.fold_recording();
        let l = &self.extra.layers[i];
        if l.visible && self.layer_usable(l) {
            let (delta, s) = (l.delta.clone(), l.strength);
            self.shift_by(&delta, -s);
        }
        self.extra.layers.remove(i);
    }

    /// Keeps a layer's effect in the sculpt and removes the layer.
    pub fn bake_layer(&mut self, i: usize) {
        if i >= self.extra.layers.len() {
            return;
        }
        self.push_extra("Bake layer");
        self.fold_recording();
        self.extra.layers.remove(i);
    }

    // ---------------------------------------------------------- morph target

    /// Whether a morph target is stored for the active level.
    pub fn has_morph(&self) -> bool {
        self.extra.morph_at(self.level, self.mesh.positions.len()).is_some()
    }

    pub fn morph_level(&self) -> Option<usize> {
        self.extra.morph.as_ref().map(|m| m.0)
    }

    pub fn store_morph(&mut self) {
        self.push_extra("Store morph target");
        self.extra.morph = Some((self.level, self.mesh.positions.clone()));
    }

    pub fn delete_morph(&mut self) {
        if self.extra.morph.is_some() {
            self.push_extra("Delete morph target");
            self.extra.morph = None;
        }
    }

    /// Swaps the sculpt with the morph target.
    pub fn switch_morph(&mut self) {
        if !self.has_morph() {
            return;
        }
        self.push_extra("Switch morph target");
        let (_, m) = self.extra.morph.as_mut().unwrap();
        std::mem::swap(m, &mut self.mesh.positions);
        self.after_bulk_move();
    }

    // ---------------------------------------------------------- surface noise

    /// Whether Surface Noise is being previewed.
    pub fn noise_previewing(&self) -> bool {
        self.stroke_label() == Some(NOISE_LABEL)
    }

    /// Shows the noise on the model (replacing any earlier preview).
    pub fn noise_preview(&mut self, s: &NoiseSettings) {
        if self.noise_previewing() {
            self.stroke_reset();
        } else {
            self.begin_stroke(false);
            self.label_stroke(NOISE_LABEL);
        }
        let (mn, mx) = self.mesh.bounds();
        let size = (mx - mn).max_element().max(1e-6);
        let inv = 1.0 / (s.scale.max(1e-4) * size);
        let amp = s.strength * size;
        let n = self.mesh.positions.len();
        let mesh = &mut self.mesh;
        let st = self.stroke.as_mut().unwrap();
        for v in 0..n {
            if mesh.vert_faces(v as u32).is_empty() {
                continue;
            }
            let w = if s.use_mask { 1.0 - mesh.mask[v] } else { 1.0 };
            if w <= 0.0 {
                continue;
            }
            st.touch(v as u32, mesh);
            let p = mesh.positions[v];
            let h = noise_at(s, p * inv);
            mesh.positions[v] = p + mesh.normals[v] * (h * amp * w);
        }
        self.after_bulk_move();
    }

    /// Keeps the previewed noise (one undo step).
    pub fn noise_apply(&mut self) {
        if self.noise_previewing() {
            self.end_stroke();
        }
    }

    pub fn noise_cancel(&mut self) {
        if self.noise_previewing() {
            self.stroke_reset();
            self.stroke = None;
            self.after_bulk_move();
        }
    }

    // ---------------------------------------------------------- project all

    /// Pulls every unmasked vertex onto `source`'s surface along its normal
    /// (within `reach`), by `strength` (one undo step). Returns how many moved.
    pub fn project_from(&mut self, source: &Mesh, bvh: &Bvh, reach: f32, strength: f32) -> usize {
        self.end_stroke();
        self.begin_stroke(false);
        self.label_stroke("Project All");
        let n = self.mesh.positions.len();
        let mut moved = 0;
        let mesh = &mut self.mesh;
        let st = self.stroke.as_mut().unwrap();
        for v in 0..n {
            if mesh.vert_faces(v as u32).is_empty() {
                continue;
            }
            let w = (1.0 - mesh.mask[v]) * strength.clamp(0.0, 1.0);
            if w <= 0.0 {
                continue;
            }
            let p = mesh.positions[v];
            let nn = mesh.normals[v];
            if let Some(q) = project_point(source, bvh, p, nn, reach) {
                st.touch(v as u32, mesh);
                mesh.positions[v] = p.lerp(q, w);
                moved += 1;
            }
        }
        self.after_bulk_move();
        self.end_stroke();
        moved
    }

    // ---------------------------------------------------------- reproject

    /// Levels that can be rebuilt over the new topology (after a remesh,
    /// ZModeler edit, retopology… on a sculpt that had levels).
    pub fn reproject_levels(&self) -> Option<usize> {
        self.lost_top.as_ref().filter(|_| self.levels.len() == 1).map(|l| l.1)
    }

    pub fn forget_lost_levels(&mut self) {
        self.lost_top = None;
    }

    /// Subdivides the new base as many times as the old sculpt had levels
    /// and projects the old highest level's detail onto it (one undo step).
    pub fn reproject_higher(&mut self) -> Result<(), String> {
        self.reproject_higher_with(false)
    }

    /// `local`: only pull points that sit right on the old surface and face
    /// the same way (after a local edit like ZModeler's, so new parts keep
    /// their shape while untouched areas get their detail back).
    pub fn reproject_higher_with(&mut self, local: bool) -> Result<(), String> {
        let Some((top, count)) = self.lost_top.clone() else { return Err("Nothing to reproject".into()) };
        if self.levels.len() != 1 {
            return Err("The sculpt already has levels".into());
        }
        let snap = self.snapshot();
        let n0 = self.undo.len();
        for _ in 0..count {
            if let Err(e) = self.subdivide() {
                // undo the partial work
                while self.undo.len() > n0 {
                    self.undo();
                }
                self.undo.truncate(n0);
                self.undo_labels.truncate(n0);
                return Err(e);
            }
        }
        let bvh = Bvh::build(&top);
        let (mn, mx) = top.bounds();
        let reach = if local {
            // a few of the old top level's edges
            let (sum, cnt) = top.tris.iter().filter(|t| t[0] != crate::mesh::NONE).take(20_000).fold((0.0f32, 0usize), |(s, c), t| {
                (s + top.positions[t[0] as usize].distance(top.positions[t[1] as usize]), c + 1)
            });
            sum / cnt.max(1) as f32 * 4.0
        } else {
            (mx - mn).max_element() * 0.08
        };
        for v in 0..self.mesh.positions.len() {
            let p = self.mesh.positions[v];
            let n = self.mesh.normals[v];
            let q = if local {
                let nn = n.normalize_or_zero();
                [nn, -nn]
                    .into_iter()
                    .filter_map(|d| bvh.raycast(&top, p, d))
                    .filter(|h| h.t <= reach && top.face_normal_raw(h.tri as usize).normalize_or_zero().dot(nn) > 0.6)
                    .min_by(|a, b| a.t.total_cmp(&b.t))
                    .map(|h| h.point)
            } else {
                project_point(&top, &bvh, p, n, reach)
            };
            if let Some(q) = q {
                self.mesh.positions[v] = q;
            }
        }
        self.mesh.compute_all_normals();
        self.undo.truncate(n0);
        self.undo_labels.truncate(n0);
        self.set_label("Reproject higher levels");
        self.push_undo(Undo::State(snap));
        self.topology_changed();
        self.lost_top = None;
        Ok(())
    }

    // ---------------------------------------------------------- extras

    /// Layers, morph target (and later sections' per-sculpt data) for a project.
    pub fn export_extras(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.extra.layers.is_empty() {
            let mut b = Vec::new();
            put_u32(&mut b, self.extra.layers.len() as u32);
            for (i, l) in self.extra.layers.iter().enumerate() {
                // a layer being recorded includes what was sculpted so far
                let mut delta = l.delta.clone();
                if let Some((ri, base)) = &self.extra.recording {
                    if *ri == i && base.len() == delta.len() && delta.len() == self.mesh.positions.len() && l.strength.abs() > 1e-4 {
                        for ((d, p), b0) in delta.iter_mut().zip(&self.mesh.positions).zip(base) {
                            *d += (*p - *b0) / l.strength;
                        }
                    }
                }
                put_str(&mut b, &l.name);
                b.push(l.visible as u8);
                b.extend_from_slice(&l.strength.to_le_bytes());
                put_u32(&mut b, l.level as u32);
                put_vecs(&mut b, &delta);
            }
            put_chunk(&mut out, b"LAYR", &b);
        }
        if let Some((level, pos)) = &self.extra.morph {
            let mut b = Vec::new();
            put_u32(&mut b, *level as u32);
            put_vecs(&mut b, pos);
            put_chunk(&mut out, b"MRPH", &b);
        }
        self.export_more_extras(&mut out);
        out
    }

    /// Reads what `export_extras` wrote (unknown chunks are skipped).
    pub fn import_extras(&mut self, bytes: &[u8]) {
        let mut at = 0;
        while at + 12 <= bytes.len() {
            let tag: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
            let len = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            let Some(body) = bytes.get(at + 12..at + 12 + len) else { break };
            at += 12 + len;
            let mut r = Reader { b: body, at: 0 };
            match &tag {
                b"LAYR" => {
                    let mut layers = Vec::new();
                    let ok = (|| {
                        let n = r.u32()?;
                        for _ in 0..n {
                            let name = r.str()?;
                            let visible = r.u8()? != 0;
                            let strength = r.f32()?;
                            let level = r.u32()? as usize;
                            let delta = r.vecs()?;
                            layers.push(Layer { name, visible, strength, level, delta });
                        }
                        Some(())
                    })();
                    if ok.is_some() {
                        self.extra.layers = layers;
                    }
                }
                b"MRPH" => {
                    if let (Some(level), Some(pos)) = (r.u32(), r.vecs()) {
                        self.extra.morph = Some((level as usize, pos));
                    }
                }
                _ => self.import_more_extras(&tag, body),
            }
        }
    }

    fn bump_revision(&mut self) {
        self.add_revision();
    }

    /// Refit picking after every vertex moved.
    fn topology_changed_soft(&mut self) {
        self.mark_all_moved();
    }
}

/// Where a ray along ±`n` from `p` meets `mesh` within `reach` (the nearer
/// side), else the closest surface point within `reach`.
pub(crate) fn project_point(mesh: &Mesh, bvh: &Bvh, p: Vec3, n: Vec3, reach: f32) -> Option<Vec3> {
    let n = n.normalize_or_zero();
    let mut best: Option<(f32, Vec3)> = None;
    if n != Vec3::ZERO {
        for dir in [n, -n] {
            if let Some(h) = bvh.raycast(mesh, p, dir) {
                if h.t <= reach && best.is_none_or(|b| h.t < b.0) {
                    best = Some((h.t, h.point));
                }
            }
        }
    }
    best.map(|b| b.1).or_else(|| bvh.closest_point(mesh, p, reach))
}

pub(crate) fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
pub(crate) fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s.as_bytes());
}
pub(crate) fn put_vecs(b: &mut Vec<u8>, v: &[Vec3]) {
    put_u32(b, v.len() as u32);
    for p in v {
        for f in p.to_array() {
            b.extend_from_slice(&f.to_le_bytes());
        }
    }
}
pub(crate) fn put_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(body);
}

pub(crate) struct Reader<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl Reader<'_> {
    pub fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.take(4).map(|s| f32::from_le_bytes(s.try_into().unwrap()))
    }
    pub fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    pub fn vecs(&mut self) -> Option<Vec<Vec3>> {
        let n = self.u32()? as usize;
        let raw = self.take(n.checked_mul(12)?)?;
        Some(
            raw.chunks_exact(12)
                .map(|c| {
                    let f = |i: usize| f32::from_le_bytes(c[i..i + 4].try_into().unwrap());
                    Vec3::new(f(0), f(4), f(8))
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{DabParams, Tool};

    fn stroke(s: &mut Sculptor, tool: Tool) {
        s.refresh_bvh();
        let h = s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap();
        s.begin_stroke(false);
        for _ in 0..4 {
            s.dab(&DabParams { tool, center: h.point, radius: 0.5, intensity: 1.0, seed_tri: h.tri, ..Default::default() });
        }
        s.end_stroke();
        s.refresh_bvh();
    }

    fn front(s: &mut Sculptor) -> f32 {
        s.refresh_bvh();
        s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z).unwrap().point.z
    }

    #[test]
    fn layers_record_toggle_fade_and_undo() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let base = front(&mut s);
        s.add_layer("Bump");
        stroke(&mut s, Tool::Draw);
        let raised = front(&mut s);
        assert!(raised > base + 0.05);
        s.set_layer_visible(0, false);
        assert!((front(&mut s) - base).abs() < 1e-4, "hidden layer leaves the base");
        s.set_layer_visible(0, true);
        assert!((front(&mut s) - raised).abs() < 1e-4);
        s.set_layer_strength(0, 0.5);
        s.set_layer_strength(0, 0.5);
        let half = front(&mut s);
        assert!((half - (base + raised) * 0.5).abs() < 0.01, "{base} {half} {raised}");
        // sculpting without a layer, then hiding the layer keeps that work
        stroke(&mut s, Tool::Inflate);
        let both = front(&mut s);
        s.set_layer_visible(0, false);
        assert!(front(&mut s) < both - 0.02);
        // undo walks back through hide, the inflate, both strength changes (one step)
        s.undo();
        assert!((front(&mut s) - both).abs() < 1e-4);
        s.undo();
        assert!((front(&mut s) - half).abs() < 1e-4);
        s.undo();
        assert!((front(&mut s) - raised).abs() < 1e-4, "slider changes merged into one step");
        assert_eq!(s.layers()[0].strength, 1.0);
        // the layer survives a project save
        let mut t = Sculptor::new(Mesh::icosphere(4));
        t.import_extras(&s.export_extras());
        assert_eq!(t.layers().len(), 1);
        assert!(t.layers()[0].usable);
        s.delete_layer(0);
        assert!((front(&mut s) - base).abs() < 1e-4);
        assert!(s.layers().is_empty());
    }

    #[test]
    fn layers_block_dynamic_topology_and_follow_levels() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        assert!(s.dyntopo_allowed());
        s.add_layer("A");
        assert!(!s.dyntopo_allowed());
        s.subdivide().unwrap();
        assert!(!s.layers()[0].usable, "layer belongs to level 1");
        assert!(!s.layers()[0].recording, "changing level stops recording");
        s.set_level(0);
        assert!(s.layers()[0].usable);
    }

    #[test]
    fn morph_brush_returns_to_the_stored_shape() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let base = front(&mut s);
        s.store_morph();
        stroke(&mut s, Tool::Draw);
        let raised = front(&mut s);
        for _ in 0..4 {
            stroke(&mut s, Tool::Morph);
        }
        assert!((front(&mut s) - base).abs() < (raised - base) * 0.1);
        stroke(&mut s, Tool::Draw);
        let r2 = front(&mut s);
        s.switch_morph();
        assert!((front(&mut s) - base).abs() < 1e-4);
        s.switch_morph();
        assert!((front(&mut s) - r2).abs() < 1e-4);
    }

    #[test]
    fn noise_previews_cancels_and_applies() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = s.mesh.positions.clone();
        let ns = NoiseSettings { strength: 0.05, ..Default::default() };
        s.noise_preview(&ns);
        assert!(s.noise_previewing());
        let p1 = s.mesh.positions.clone();
        assert!(p1.iter().zip(&before).any(|(a, b)| a.distance(*b) > 0.01));
        s.noise_preview(&NoiseSettings { kind: NoiseKind::Cells, ..ns });
        assert_ne!(s.mesh.positions, p1, "preview follows the settings");
        s.noise_cancel();
        assert_eq!(s.mesh.positions, before);
        assert!(!s.noise_previewing());
        s.noise_preview(&ns);
        s.noise_apply();
        assert_eq!(s.mesh.positions, p1, "same settings give the same noise");
        s.undo();
        assert_eq!(s.mesh.positions, before);
        for k in NoiseKind::ALL {
            let v: Vec<f32> = (0..200).map(|i| noise_at(&NoiseSettings { kind: k, ..ns }, Vec3::new(i as f32 * 0.137, 0.3, i as f32 * 0.071))).collect();
            let (mn, mx) = v.iter().fold((1.0f32, -1.0f32), |(a, b), &x| (a.min(x), b.max(x)));
            assert!(mx - mn > 0.3 && mn >= -1.01 && mx <= 1.01, "{k:?} {mn} {mx}");
        }
    }

    #[test]
    fn project_all_snaps_onto_another_surface() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let mut big = Mesh::icosphere(4);
        for p in &mut big.positions {
            *p *= 1.1;
        }
        let bvh = Bvh::build(&big);
        let n = s.project_from(&big, &bvh, 0.3, 1.0);
        assert_eq!(n, s.mesh.positions.len());
        assert!(s.mesh.positions.iter().all(|p| (p.length() - 1.1).abs() < 0.01));
        s.undo();
        assert!(s.mesh.positions.iter().all(|p| (p.length() - 1.0).abs() < 1e-4));
    }

    #[test]
    fn reproject_rebuilds_levels_with_the_old_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        stroke(&mut s, Tool::Draw);
        let bump = front(&mut s);
        assert!(bump > 1.05);
        // new base topology (a different sphere): levels are gone
        s.set_mesh(Mesh::icosphere(3));
        assert_eq!(s.level_count(), 1);
        assert_eq!(s.reproject_levels(), Some(2));
        s.reproject_higher().unwrap();
        assert_eq!(s.level_count(), 3);
        assert!((front(&mut s) - bump).abs() < 0.02, "{} vs {bump}", front(&mut s));
        s.undo();
        assert_eq!(s.level_count(), 1);
    }
}
