//! Gizmo deformers: bend, twist, taper, flatten and inflate along an axis.

use glam::{Quat, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deformer {
    Bend,
    Twist,
    Taper,
    Flatten,
    Inflate,
}

impl Deformer {
    pub const ALL: [Deformer; 5] = [Deformer::Bend, Deformer::Twist, Deformer::Taper, Deformer::Flatten, Deformer::Inflate];
    pub fn name(self) -> &'static str {
        match self {
            Deformer::Bend => "Bend",
            Deformer::Twist => "Twist",
            Deformer::Taper => "Taper",
            Deformer::Flatten => "Flatten",
            Deformer::Inflate => "Inflate",
        }
    }
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// Moves `pts` (with normals `nrm`, inside `bounds`) by a deformer along
/// `axis` with strength `amount` (about -1..1: ±180° of bend, ±360° of twist).
pub fn apply(kind: Deformer, axis: usize, amount: f32, pts: &[Vec3], nrm: &[Vec3], bounds: (Vec3, Vec3)) -> Vec<Vec3> {
    let u = AXES[axis.min(2)];
    // the direction a bend curls towards
    let d = if axis == 1 { Vec3::X } else { Vec3::Y };
    let (mn, mx) = bounds;
    let c = (mn + mx) * 0.5;
    let len = (mx - mn).dot(u).max(1e-6);
    let size = (mx - mn).max_element().max(1e-6);
    let start = c - u * (len * 0.5);
    pts.iter()
        .zip(nrm)
        .map(|(&p, &n)| {
            let s = (p - start).dot(u);
            let t = s / len; // 0..1 along the axis
            match kind {
                Deformer::Twist => {
                    let a = amount * std::f32::consts::TAU * (t - 0.5);
                    let rel = p - c;
                    let along = u * rel.dot(u);
                    c + along + Quat::from_axis_angle(u, a) * (rel - along)
                }
                Deformer::Taper => {
                    let k = (1.0 + amount * (t - 0.5) * 2.0).max(0.02);
                    let rel = p - c;
                    let along = u * rel.dot(u);
                    c + along + (rel - along) * k
                }
                Deformer::Flatten => {
                    let k = (1.0 - amount).max(0.0);
                    let rel = p - c;
                    let along = rel.dot(u);
                    p + u * (along * k - along)
                }
                Deformer::Inflate => p + n * (amount * size * 0.25),
                Deformer::Bend => {
                    let k = amount * std::f32::consts::PI / len;
                    if k.abs() < 1e-6 {
                        return p;
                    }
                    let h = (p - start).dot(d);
                    let rest = p - start - u * s - d * h;
                    let r = 1.0 / k;
                    let a = k * s;
                    let x = (r - h) * a.sin();
                    let y = r - (r - h) * a.cos();
                    start + u * x + d * y + rest
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rod() -> (Vec<Vec3>, Vec<Vec3>, (Vec3, Vec3)) {
        let pts: Vec<Vec3> = (0..=10).map(|i| Vec3::new(0.0, i as f32 * 0.2 - 1.0, 0.0)).collect();
        let n = vec![Vec3::X; pts.len()];
        let b = (Vec3::new(-0.1, -1.0, -0.1), Vec3::new(0.1, 1.0, 0.1));
        (pts, n, b)
    }

    #[test]
    fn bend_keeps_length_and_curls() {
        let (p, n, b) = rod();
        let q = apply(Deformer::Bend, 1, 0.5, &p, &n, b);
        let len: f32 = q.windows(2).map(|w| w[0].distance(w[1])).sum();
        assert!((len - 2.0).abs() < 0.02, "{len}");
        assert!(q[10].x > 0.3, "the end swings over: {:?}", q[10]);
        assert_eq!(q[0], p[0]);
    }

    #[test]
    fn twist_taper_flatten() {
        let pts = vec![Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, -1.0, 0.0)];
        let n = vec![Vec3::X; 2];
        let b = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        let t = apply(Deformer::Twist, 1, 0.25, &pts, &n, b);
        // +45° at the top, -45° at the bottom
        assert!((t[0].x - t[0].z.abs()).abs() < 1e-4 && t[0].y == 1.0);
        let tp = apply(Deformer::Taper, 1, 0.5, &pts, &n, b);
        assert!((tp[0].x - 1.5).abs() < 1e-4 && (tp[1].x - 0.5).abs() < 1e-4);
        let f = apply(Deformer::Flatten, 1, 1.0, &pts, &n, b);
        assert!(f[0].y.abs() < 1e-5 && f[1].y.abs() < 1e-5);
    }
}
