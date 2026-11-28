//! 3D printing: real-world size and a watertightness check.

use crate::io::Placement;
use crate::mesh::{Mesh, NONE};
use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    Mm,
    Cm,
    Inch,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Mm, Unit::Cm, Unit::Inch];
    pub fn name(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::Inch => "in",
        }
    }
    pub fn to_mm(self) -> f32 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::Inch => 25.4,
        }
    }
}

/// What a 3D printer slicer cares about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshCheck {
    /// Edges with only one face (holes).
    pub open_edges: usize,
    /// Edges shared by more than two faces.
    pub nonmanifold_edges: usize,
    /// Separate pieces.
    pub shells: usize,
    /// Enclosed volume in model units³ (negative = inside out).
    pub volume: f32,
}

impl MeshCheck {
    pub fn watertight(&self) -> bool {
        self.open_edges == 0 && self.nonmanifold_edges == 0 && self.volume > 0.0
    }
}

pub fn check(mesh: &Mesh) -> MeshCheck {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    let mut volume = 0.0f64;
    // union-find over vertices for shells
    let mut parent: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for t in mesh.tris.iter().filter(|t| t[0] != NONE) {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra as usize] = rb;
        }
        let [a, b, c] = t.map(|v| mesh.positions[v as usize].as_dvec3());
        volume += a.dot(b.cross(c)) / 6.0;
    }
    let mut roots = std::collections::HashSet::new();
    for t in mesh.tris.iter().filter(|t| t[0] != NONE) {
        roots.insert(find(&mut parent, t[0]));
    }
    MeshCheck {
        open_edges: edges.values().filter(|&&c| c == 1).count(),
        nonmanifold_edges: edges.values().filter(|&&c| c > 2).count(),
        shells: roots.len(),
        volume: volume as f32,
    }
}

/// Placement for printing: Z up, the longest side `size` long (in `unit`,
/// written as millimetres), centred on the plate with its base at Z = 0.
pub fn placement(mesh: &Mesh, size: f32, unit: Unit) -> Placement {
    let (mn, mx) = mesh.bounds();
    let longest = (mx - mn).max_element().max(1e-9);
    let k = size * unit.to_mm() / longest;
    let c = (mn + mx) * 0.5;
    // before the Y-up -> Z-up turn: centre x and z, bottom (min y) at 0
    let offset = Vec3::new(-c.x * k, -mn.y * k, -c.z * k);
    Placement { scale: k, offset, z_up: true }
}

/// The model's size (x, y, z after turning Z up) in `unit` for a given longest side.
pub fn printed_size(mesh: &Mesh, size: f32) -> Vec3 {
    let (mn, mx) = mesh.bounds();
    let e = mx - mn;
    let k = size / e.max_element().max(1e-9);
    Vec3::new(e.x, e.z, e.y) * k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_is_watertight_and_plane_is_not() {
        let c = check(&Mesh::icosphere(3));
        assert!(c.watertight(), "{c:?}");
        assert_eq!(c.shells, 1);
        assert!((c.volume - 4.18879).abs() < 0.1);
        let p = check(&Mesh::plane(4));
        assert!(!p.watertight() && p.open_edges > 0);
    }

    #[test]
    fn print_placement_sits_on_the_plate() {
        let m = Mesh::icosphere(2);
        let pl = placement(&m, 50.0, Unit::Mm);
        let pts: Vec<Vec3> = m.positions.iter().map(|p| pl.pos(*p)).collect();
        let minz = pts.iter().map(|p| p.z).fold(f32::MAX, f32::min);
        let maxz = pts.iter().map(|p| p.z).fold(f32::MIN, f32::max);
        assert!(minz.abs() < 1e-3 && (maxz - 50.0).abs() < 0.2, "{minz} {maxz}");
        let pl_in = placement(&m, 2.0, Unit::Inch);
        let top = m.positions.iter().map(|p| pl_in.pos(*p).z).fold(f32::MIN, f32::max);
        assert!((top - 50.8).abs() < 0.3);
    }
}
