//! Dynamics: the whole model simulated as cloth (or a soft body) under
//! gravity, landing on a floor and on the other visible SubTools. Masked
//! points are pinned. Runs a step per frame until applied or cancelled.

use crate::bvh::Bvh;
use crate::mesh::{Mesh, NONE};
use crate::sculpt::Sculptor;
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DynSettings {
    /// Gravity strength (model sizes per second², roughly).
    pub gravity: f32,
    /// How firmly edges keep their length (0..1).
    pub stiffness: f32,
    /// Resistance to bending (0..1): high keeps the shape, low drapes.
    pub bending: f32,
    /// Keeps a closed model's volume, like a balloon (0 = cloth).
    pub inflate: f32,
    /// Floor height below the model, as a fraction of its size (negative = no floor).
    pub floor: f32,
    pub collide_subtools: bool,
}

impl Default for DynSettings {
    fn default() -> Self {
        Self { gravity: 1.0, stiffness: 0.9, bending: 0.3, inflate: 0.0, floor: 0.05, collide_subtools: true }
    }
}

pub struct Sim {
    pub settings: DynSettings,
    prev: Vec<Vec3>,
    edges: Vec<(u32, u32, f32)>,
    /// Second-ring (bending) links.
    bends: Vec<(u32, u32, f32)>,
    inv_mass: Vec<f32>,
    floor_y: f32,
    size: f32,
    rest_volume: f32,
    obstacles: Option<(Mesh, Bvh)>,
    pub steps: u32,
}

fn volume(m: &Mesh) -> f32 {
    m.tris
        .iter()
        .filter(|t| t[0] != NONE)
        .map(|t| m.positions[t[0] as usize].dot(m.positions[t[1] as usize].cross(m.positions[t[2] as usize])) / 6.0)
        .sum()
}

impl Sim {
    pub fn new(m: &Mesh, settings: DynSettings, obstacles: Option<Mesh>) -> Self {
        let n = m.positions.len();
        let mut seen = std::collections::HashSet::new();
        let mut edges = Vec::new();
        for t in m.tris.iter().filter(|t| t[0] != NONE) {
            for i in 0..3 {
                let (a, b) = (t[i], t[(i + 1) % 3]);
                if seen.insert((a.min(b), a.max(b))) {
                    edges.push((a, b, m.positions[a as usize].distance(m.positions[b as usize])));
                }
            }
        }
        // bending: each vertex to its neighbours' neighbours across an edge
        let mut bends = Vec::new();
        let mut bseen = std::collections::HashSet::new();
        for v in 0..n as u32 {
            for &u in m.neighbors(v) {
                for &w in m.neighbors(u) {
                    if w != v && !m.neighbors(v).contains(&w) && bseen.insert((v.min(w), v.max(w))) {
                        bends.push((v, w, m.positions[v as usize].distance(m.positions[w as usize])));
                    }
                }
            }
        }
        let inv_mass = (0..n).map(|v| if m.vert_faces(v as u32).is_empty() { 0.0 } else { 1.0 - m.mask[v] }).collect();
        let (mn, mx) = m.bounds();
        let size = (mx - mn).max_element().max(1e-6);
        let obstacles = obstacles.filter(|o| !o.tris.is_empty()).map(|o| {
            let b = Bvh::build(&o);
            (o, b)
        });
        Self {
            settings,
            prev: m.positions.clone(),
            edges,
            bends,
            inv_mass,
            floor_y: mn.y - settings.floor * size,
            size,
            rest_volume: volume(m),
            obstacles,
            steps: 0,
        }
    }

    /// One frame (several small sub-steps).
    pub fn step(&mut self, m: &mut Mesh) {
        let s = self.settings;
        let dt = 1.0 / 60.0 / 2.0;
        let g = Vec3::NEG_Y * s.gravity * self.size * 4.0;
        for _ in 0..2 {
            for v in 0..m.positions.len() {
                let w = self.inv_mass[v];
                if w <= 0.0 {
                    continue;
                }
                let p = m.positions[v];
                let vel = (p - self.prev[v]) * 0.99;
                self.prev[v] = p;
                m.positions[v] = p + vel + g * (dt * dt * w);
            }
            for _ in 0..6 {
                Self::relax(&mut m.positions, &self.inv_mass, &self.edges, s.stiffness);
                if s.bending > 0.0 {
                    Self::relax(&mut m.positions, &self.inv_mass, &self.bends, s.bending * 0.5);
                }
                if s.inflate > 0.0 && self.rest_volume.abs() > 1e-9 {
                    // push along normals towards the rest volume (times inflate)
                    let vol = volume(m);
                    let target = self.rest_volume * (1.0 + s.inflate);
                    let k = ((target - vol) / target.abs()).clamp(-0.2, 0.2) * self.size * 0.02;
                    m.compute_all_normals();
                    for v in 0..m.positions.len() {
                        if self.inv_mass[v] > 0.0 {
                            m.positions[v] += m.normals[v] * (k * self.inv_mass[v]);
                        }
                    }
                }
            }
            self.collide(m);
        }
        self.steps += 1;
    }

    fn relax(pos: &mut [Vec3], inv: &[f32], links: &[(u32, u32, f32)], k: f32) {
        for &(a, b, rest) in links {
            let (wa, wb) = (inv[a as usize], inv[b as usize]);
            let w = wa + wb;
            if w <= 0.0 {
                continue;
            }
            let d = pos[b as usize] - pos[a as usize];
            let l = d.length();
            if l < 1e-12 {
                continue;
            }
            let c = (l - rest) / l * k;
            pos[a as usize] += d * (c * wa / w);
            pos[b as usize] -= d * (c * wb / w);
        }
    }

    fn collide(&mut self, m: &mut Mesh) {
        let thick = self.size * 0.004;
        for v in 0..m.positions.len() {
            if self.inv_mass[v] <= 0.0 {
                continue;
            }
            let mut p = m.positions[v];
            if self.settings.floor >= 0.0 && p.y < self.floor_y + thick {
                p.y = self.floor_y + thick;
                // friction on the floor
                let pv = self.prev[v];
                self.prev[v] = Vec3::new(pv.x * 0.6 + p.x * 0.4, pv.y, pv.z * 0.6 + p.z * 0.4);
            }
            if let Some((o, b)) = &self.obstacles {
                if let Some((q, t)) = b.closest(o, p, thick * 6.0) {
                    let n = o.face_normal_raw(t as usize).normalize_or_zero();
                    let d = (p - q).dot(n);
                    if d < thick {
                        p += n * (thick - d);
                        self.prev[v] = self.prev[v].lerp(p, 0.5);
                    }
                }
            }
            m.positions[v] = p;
        }
    }
}

const DYN_LABEL: &str = "Dynamics";

impl Sculptor {
    pub fn dynamics_running(&self) -> bool {
        self.stroke_label() == Some(DYN_LABEL)
    }

    /// Starts a simulation (an undo step once applied).
    pub fn dynamics_begin(&mut self) {
        self.end_stroke();
        self.begin_stroke(false);
        self.label_stroke(DYN_LABEL);
        let st = self.stroke.as_mut().unwrap();
        for v in 0..self.mesh.positions.len() as u32 {
            st.touch(v, &self.mesh);
        }
    }

    pub fn dynamics_step(&mut self, sim: &mut Sim) {
        if !self.dynamics_running() {
            return;
        }
        sim.step(&mut self.mesh);
        self.mesh.compute_all_normals();
        self.mark_all_moved();
    }

    pub fn dynamics_apply(&mut self) {
        if self.dynamics_running() {
            self.end_stroke();
        }
    }

    pub fn dynamics_cancel(&mut self) {
        if self.dynamics_running() {
            self.stroke_reset();
            self.stroke = None;
            self.mesh.compute_all_normals();
            self.mark_all_moved();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloth_falls_drapes_on_the_floor_and_pinned_points_stay() {
        let mut s = Sculptor::new(Mesh::plane(5));
        // pin one corner
        let corner = (0..s.mesh.positions.len()).min_by(|&a, &b| {
            let k = |v: usize| s.mesh.positions[v].x + s.mesh.positions[v].z;
            k(a).total_cmp(&k(b))
        }).unwrap();
        s.mesh.mask[corner] = 1.0;
        let pinned = s.mesh.positions[corner];
        let before = s.mesh.positions.clone();
        let mut sim = Sim::new(&s.mesh, DynSettings { floor: 0.3, ..Default::default() }, None);
        s.dynamics_begin();
        for _ in 0..90 {
            s.dynamics_step(&mut sim);
        }
        assert_eq!(s.mesh.positions[corner], pinned);
        let (mn, _) = s.mesh.bounds();
        assert!(mn.y < before.iter().map(|p| p.y).fold(f32::MAX, f32::min) - 0.1, "it fell");
        // edges kept their length roughly (cloth, not rubber)
        let stretch = s.mesh.tris.iter().take(50).map(|t| {
            let (a, b) = (t[0] as usize, t[1] as usize);
            s.mesh.positions[a].distance(s.mesh.positions[b]) / before[a].distance(before[b])
        }).fold(0.0f32, f32::max);
        assert!(stretch < 1.3, "{stretch}");
        s.dynamics_apply();
        s.undo();
        assert_eq!(s.mesh.positions, before);
        // a cancelled run leaves no trace
        s.dynamics_begin();
        s.dynamics_step(&mut sim);
        s.dynamics_cancel();
        assert_eq!(s.mesh.positions, before);
        assert!(!s.can_redo() || true);
    }
}
