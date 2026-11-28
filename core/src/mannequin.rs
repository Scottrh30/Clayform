//! A posable mannequin built from capsules, for blocking in figures.

use crate::mesh::Mesh;
use glam::{Quat, Vec3};
use std::f32::consts::PI;

#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub name: &'static str,
    pub parent: Option<usize>,
    /// Rest position relative to the parent joint (in the parent's frame).
    pub offset: Vec3,
    /// Radius of the bone from the parent to this joint.
    pub radius: f32,
    /// Pose rotation of this joint (turns its children).
    pub rot: Quat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mannequin {
    pub joints: Vec<Joint>,
    /// Overall build: 1 = average, lower is slimmer.
    pub bulk: f32,
}

pub const PRESETS: [&str; 6] = ["T-pose", "A-pose", "Walk", "Sit", "Arms up", "Run"];

impl Default for Mannequin {
    fn default() -> Self {
        let mut m = Self::new();
        m.apply_preset("A-pose");
        m
    }
}

fn j(name: &'static str, parent: Option<usize>, offset: [f32; 3], radius: f32) -> Joint {
    Joint { name, parent, offset: Vec3::from(offset), radius, rot: Quat::IDENTITY }
}

impl Mannequin {
    /// A figure about two units tall standing at the origin, in a T-pose.
    pub fn new() -> Self {
        let mut joints = vec![
            j("Pelvis", None, [0.0, -0.02, 0.0], 0.13),
            j("Spine", Some(0), [0.0, 0.24, 0.0], 0.12),
            j("Chest", Some(1), [0.0, 0.24, 0.0], 0.15),
            j("Neck", Some(2), [0.0, 0.17, 0.0], 0.05),
            j("Head", Some(3), [0.0, 0.14, 0.02], 0.055),
        ];
        for (side, s) in [("L", 1.0f32), ("R", -1.0)] {
            let base = joints.len();
            let name = |n: &str| -> &'static str { Box::leak(format!("{side} {n}").into_boxed_str()) };
            joints.push(j(name("shoulder"), Some(2), [0.17 * s, 0.11, 0.0], 0.055));
            joints.push(j(name("elbow"), Some(base), [0.29 * s, 0.0, 0.0], 0.05));
            joints.push(j(name("wrist"), Some(base + 1), [0.26 * s, 0.0, 0.0], 0.04));
            joints.push(j(name("hand"), Some(base + 2), [0.1 * s, 0.0, 0.0], 0.035));
        }
        for (side, s) in [("L", 1.0f32), ("R", -1.0)] {
            let base = joints.len();
            let name = |n: &str| -> &'static str { Box::leak(format!("{side} {n}").into_boxed_str()) };
            joints.push(j(name("hip"), Some(0), [0.1 * s, -0.06, 0.0], 0.08));
            joints.push(j(name("knee"), Some(base), [0.0, -0.44, 0.0], 0.07));
            joints.push(j(name("ankle"), Some(base + 1), [0.0, -0.42, 0.0], 0.05));
            joints.push(j(name("toe"), Some(base + 2), [0.0, -0.06, 0.15], 0.04));
        }
        Self { joints, bulk: 1.0 }
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    /// World position and rotation of every joint.
    pub fn world(&self) -> Vec<(Vec3, Quat)> {
        let mut out: Vec<(Vec3, Quat)> = Vec::with_capacity(self.joints.len());
        for jt in &self.joints {
            let w = match jt.parent {
                None => (jt.offset, jt.rot),
                Some(p) => {
                    let (pp, pr) = out[p];
                    (pp + pr * jt.offset, pr * jt.rot)
                }
            };
            out.push(w);
        }
        out
    }

    /// The pose as numbers (each joint's rotation, then the build).
    pub fn to_pose(&self) -> Vec<f32> {
        let mut v: Vec<f32> = self.joints.iter().flat_map(|j| j.rot.to_array()).collect();
        v.push(self.bulk);
        v
    }

    /// Sets the pose from `to_pose` numbers (ignored if they don't fit).
    pub fn load_pose(&mut self, v: &[f32]) -> bool {
        if v.len() != self.joints.len() * 4 + 1 {
            return false;
        }
        for (i, j) in self.joints.iter_mut().enumerate() {
            j.rot = Quat::from_slice(&v[i * 4..i * 4 + 4]).normalize();
        }
        self.bulk = v[v.len() - 1];
        true
    }

    pub fn reset(&mut self) {
        for jt in &mut self.joints {
            jt.rot = Quat::IDENTITY;
        }
    }

    fn set(&mut self, name: &str, rot: Quat) {
        if let Some(i) = self.find(name) {
            self.joints[i].rot = rot;
        }
    }

    pub fn apply_preset(&mut self, preset: &str) {
        self.reset();
        let x = |deg: f32| Quat::from_rotation_x(deg.to_radians());
        let z = |deg: f32| Quat::from_rotation_z(deg.to_radians());
        let arms_down = |m: &mut Self, deg: f32| {
            m.set("L shoulder", z(-deg));
            m.set("R shoulder", z(deg));
        };
        match preset {
            "A-pose" => arms_down(self, 45.0),
            "Walk" => {
                arms_down(self, 75.0);
                self.set("L hip", x(-25.0));
                self.set("L knee", x(10.0));
                self.set("R hip", x(20.0));
                self.set("R knee", x(25.0));
                self.set("L elbow", Quat::from_rotation_y(-0.4));
                self.set("R elbow", Quat::from_rotation_y(0.4));
            }
            "Sit" => {
                arms_down(self, 70.0);
                self.set("L hip", x(-90.0));
                self.set("R hip", x(-90.0));
                self.set("L knee", x(90.0));
                self.set("R knee", x(90.0));
            }
            "Arms up" => {
                self.set("L shoulder", z(75.0));
                self.set("R shoulder", z(-75.0));
            }
            "Run" => {
                arms_down(self, 65.0);
                self.set("Spine", x(12.0));
                self.set("L hip", x(-50.0));
                self.set("L knee", x(60.0));
                self.set("R hip", x(30.0));
                self.set("R knee", x(70.0));
                self.set("L elbow", Quat::from_rotation_y(-1.3));
                self.set("R elbow", Quat::from_rotation_y(1.3));
            }
            _ => {}
        }
    }

    /// Turns the bone ending at joint `j` (its parent's rotation) so `j`
    /// points at `target`.
    pub fn aim(&mut self, j: usize, target: Vec3) {
        let Some(p) = self.joints[j].parent else { return };
        let w = self.world();
        let (pp, pr) = w[p];
        let cur = (w[j].0 - pp).normalize_or_zero();
        let want = (target - pp).normalize_or_zero();
        if cur == Vec3::ZERO || want == Vec3::ZERO {
            return;
        }
        let r = Quat::from_rotation_arc(cur, want);
        let grand = self.joints[p].parent.map_or(Quat::IDENTITY, |g| w[g].1);
        self.joints[p].rot = (grand.inverse() * r * pr).normalize();
    }

    /// The figure as one mesh of overlapping capsules (voxel remesh merges
    /// them into one surface). Each bone gets its own polygroup.
    pub fn mesh(&self) -> Mesh {
        let w = self.world();
        let mut pos = Vec::new();
        let mut tris = Vec::new();
        let mut groups = Vec::new();
        let mut add = |m: (Vec<Vec3>, Vec<[u32; 3]>), g: u32| {
            let base = pos.len() as u32;
            pos.extend(m.0);
            for t in m.1 {
                tris.push([t[0] + base, t[1] + base, t[2] + base]);
                groups.push(g);
            }
        };
        for (i, jt) in self.joints.iter().enumerate() {
            let r = jt.radius * self.bulk;
            if let Some(p) = jt.parent {
                let (a, b) = (w[p].0, w[i].0);
                let r0 = self.joints[p].radius.min(jt.radius * 1.4) * self.bulk;
                add(capsule(a, b, r0.max(r), r), i as u32);
            }
            if jt.name == "Head" {
                let c = w[i].0 + w[i].1 * Vec3::new(0.0, 0.06, 0.0);
                add(capsule(c - Vec3::Y * 0.02, c + Vec3::Y * 0.05, 0.11 * self.bulk.sqrt(), 0.1 * self.bulk.sqrt()), i as u32);
            }
        }
        // torso blocks: hips and ribcage
        let pel = w[0].0;
        add(capsule(pel + w[0].1 * Vec3::new(-0.08, 0.0, 0.0), pel + w[0].1 * Vec3::new(0.08, 0.0, 0.0), 0.12 * self.bulk, 0.12 * self.bulk), 0);
        let chest = w[2].0;
        add(
            capsule(chest + w[2].1 * Vec3::new(-0.07, 0.02, 0.0), chest + w[2].1 * Vec3::new(0.07, 0.02, 0.0), 0.14 * self.bulk, 0.14 * self.bulk),
            2,
        );
        crate::mesh::MeshBuilder::new(pos, tris).with_faces(Some(groups), None).finish()
    }
}

/// A closed capsule from `a` (radius `ra`) to `b` (radius `rb`).
pub fn capsule(a: Vec3, b: Vec3, ra: f32, rb: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    const SEG: u32 = 16;
    const HALF: u32 = 5; // rings per hemisphere
    let axis = b - a;
    let len = axis.length();
    let dir = if len > 1e-6 { axis / len } else { Vec3::Y };
    let u = dir.any_orthonormal_vector();
    let v = dir.cross(u);
    let mut pos = Vec::new();
    // rings from the bottom pole (at a) to the top pole (at b)
    let mut rings: Vec<(Vec3, f32)> = Vec::new();
    for k in 1..=HALF {
        let t = PI * 0.5 * (k as f32 / HALF as f32); // 0 at pole .. pi/2 at equator
        rings.push((a - dir * (ra * t.cos()), ra * t.sin()));
    }
    for k in 0..HALF {
        let t = PI * 0.5 * (k as f32 / HALF as f32);
        rings.push((b + dir * (rb * t.sin()), rb * t.cos()));
    }
    let bottom = 0u32;
    pos.push(a - dir * ra);
    for &(c, r) in &rings {
        for s in 0..SEG {
            let ang = 2.0 * PI * s as f32 / SEG as f32;
            pos.push(c + (u * ang.cos() + v * ang.sin()) * r);
        }
    }
    let top = pos.len() as u32;
    pos.push(b + dir * rb);
    let mut tris = Vec::new();
    let ring = |k: u32, s: u32| 1 + k * SEG + (s % SEG);
    for s in 0..SEG {
        tris.push([bottom, ring(0, s + 1), ring(0, s)]);
    }
    for k in 0..rings.len() as u32 - 1 {
        for s in 0..SEG {
            let (a0, a1, b0, b1) = (ring(k, s), ring(k, s + 1), ring(k + 1, s), ring(k + 1, s + 1));
            tris.push([a0, a1, b1]);
            tris.push([a0, b1, b0]);
        }
    }
    let last = rings.len() as u32 - 1;
    for s in 0..SEG {
        tris.push([top, ring(last, s), ring(last, s + 1)]);
    }
    (pos, tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_is_closed_and_outward() {
        let (p, t) = capsule(Vec3::ZERO, Vec3::Y, 0.2, 0.1);
        let m = Mesh::new(p, t);
        let mut c: std::collections::HashMap<(u32, u32), i32> = Default::default();
        for t in &m.tris {
            for e in 0..3 {
                *c.entry((t[e], t[(e + 1) % 3])).or_default() += 1;
            }
        }
        assert!(c.iter().all(|(&(a, b), &n)| n == 1 && c.get(&(b, a)) == Some(&1)));
        let center = Vec3::new(0.0, 0.5, 0.0);
        for f in 0..m.tris.len() {
            let q = m.positions[m.tris[f][0] as usize];
            assert!(m.face_normal_raw(f).dot(q - center) > -1e-6);
        }
    }

    #[test]
    fn poses_and_aiming() {
        let mut m = Mannequin::new();
        let h = m.find("L hand").unwrap();
        let t_pose_hand = m.world()[h].0;
        assert!(t_pose_hand.x > 0.7 && t_pose_hand.y > 0.5);
        m.apply_preset("A-pose");
        let a_hand = m.world()[h].0;
        assert!(a_hand.y < t_pose_hand.y - 0.3, "arms come down");
        // aim the left forearm straight forward
        let wrist = m.find("L wrist").unwrap();
        let elbow = m.world()[m.find("L elbow").unwrap()].0;
        m.aim(wrist, elbow + Vec3::Z);
        let d = (m.world()[wrist].0 - elbow).normalize();
        assert!(d.z > 0.99, "{d:?}");
        let mesh = m.mesh();
        assert!(mesh.tris.len() > 2000);
        let (mn, mx) = mesh.bounds();
        assert!(mx.y - mn.y > 1.7 && mx.y - mn.y < 2.3, "{mn:?} {mx:?}");
        for p in PRESETS {
            m.apply_preset(p);
            assert!(m.mesh().positions.iter().all(|q| q.is_finite()));
        }
    }
}
