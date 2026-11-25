//! Symmetry: mirror across X, Y and/or Z, plus radial copies around an
//! axis, about the world origin or the model's own centre.

use glam::{Mat3, Vec3};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Symmetry {
    /// Mirror across the planes x = 0, y = 0, z = 0 (through the centre).
    pub mirror: [bool; 3],
    /// Radial copies around `radial_axis` (1 = off).
    pub radial: u32,
    /// 0 = X, 1 = Y, 2 = Z.
    pub radial_axis: usize,
    /// Use the model's centre instead of the world origin.
    pub local: bool,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self { mirror: [true, false, false], radial: 1, radial_axis: 1, local: false }
    }
}

pub const MAX_RADIAL: u32 = 32;
pub const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

impl Symmetry {
    pub fn is_active(&self) -> bool {
        self.mirror.iter().any(|m| *m) || self.radial > 1
    }

    /// The linear part of every symmetric copy except the original.
    pub fn copies(&self) -> Vec<Mat3> {
        let mut set = vec![Mat3::IDENTITY];
        for axis in 0..3 {
            if self.mirror[axis] {
                let mut d = Vec3::ONE;
                d[axis] = -1.0;
                let s = Mat3::from_diagonal(d);
                set = set.iter().flat_map(|m| [*m, s * *m]).collect();
            }
        }
        let n = self.radial.clamp(1, MAX_RADIAL);
        if n > 1 {
            let mut axis = Vec3::ZERO;
            axis[self.radial_axis.min(2)] = 1.0;
            let base = set.clone();
            for k in 1..n {
                let r = Mat3::from_axis_angle(axis, std::f32::consts::TAU * k as f32 / n as f32);
                set.extend(base.iter().map(|m| r * *m));
            }
        }
        set.remove(0);
        set
    }

    /// Short description for the status bar, e.g. "Sym X+Z · Radial 8 (Y)".
    pub fn label(&self) -> String {
        let axes: Vec<&str> = (0..3).filter(|&a| self.mirror[a]).map(|a| AXIS_NAMES[a]).collect();
        let mut parts = Vec::new();
        if !axes.is_empty() {
            parts.push(format!("Sym {}", axes.join("+")));
        }
        if self.radial > 1 {
            parts.push(format!("Radial {} ({})", self.radial, AXIS_NAMES[self.radial_axis.min(2)]));
        }
        if parts.is_empty() {
            return String::new();
        }
        let mut s = parts.join(" · ");
        if self.local {
            s.push_str(" · local");
        }
        s
    }
}

/// Applies a symmetry copy about `center` to a point.
#[inline]
pub fn apply_point(m: Mat3, center: Vec3, p: Vec3) -> Vec3 {
    center + m * (p - center)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_counts() {
        let mut s = Symmetry { mirror: [false; 3], radial: 1, radial_axis: 1, local: false };
        assert!(s.copies().is_empty() && !s.is_active());
        s.mirror = [true, false, false];
        assert_eq!(s.copies().len(), 1);
        s.mirror = [true, true, true];
        assert_eq!(s.copies().len(), 7);
        s.mirror = [false; 3];
        s.radial = 8;
        assert_eq!(s.copies().len(), 7);
        s.mirror = [true, false, false];
        assert_eq!(s.copies().len(), 15);
        assert_eq!(s.label(), "Sym X · Radial 8 (Y)");
    }
}
