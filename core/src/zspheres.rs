//! ZSpheres: a tree of spheres sketched in the viewport, skinned into a
//! smooth base mesh (round cones between linked spheres, blended together).

use crate::mesh::Mesh;
use crate::volume::{mesh_field, Grid};
use glam::Vec3;

#[derive(Clone, Debug, PartialEq)]
pub struct ZNode {
    pub pos: Vec3,
    pub radius: f32,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZTree {
    pub nodes: Vec<ZNode>,
    history: Vec<Vec<ZNode>>,
}

/// Distance to a round cone from sphere (a, ra) to sphere (b, rb).
fn round_cone(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    let ba = b - a;
    let l2 = ba.length_squared();
    if l2 < 1e-12 {
        return (p - a).length() - ra.max(rb);
    }
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = pa.dot(ba);
    let z = y - l2;
    let x2 = (pa * l2 - ba * y).length_squared();
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = rr.signum() * rr * rr * x2;
    if z.signum() * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - rb;
    }
    if y.signum() * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - ra;
    }
    ((x2 * a2 * il2).sqrt() + y * rr) * il2 - ra
}

fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

impl ZTree {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn checkpoint(&mut self) {
        self.history.push(self.nodes.clone());
        if self.history.len() > 200 {
            self.history.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(n) = self.history.pop() {
            self.nodes = n;
        }
    }

    pub fn clear(&mut self) {
        if !self.nodes.is_empty() {
            self.checkpoint();
            self.nodes.clear();
        }
    }

    pub fn add(&mut self, pos: Vec3, radius: f32, parent: Option<usize>) -> usize {
        self.nodes.push(ZNode { pos, radius, parent });
        self.nodes.len() - 1
    }

    /// Removes a sphere and everything hanging from it.
    pub fn remove(&mut self, i: usize) {
        let mut gone = vec![false; self.nodes.len()];
        gone[i] = true;
        // parents always come before their children
        for k in 0..self.nodes.len() {
            if let Some(p) = self.nodes[k].parent {
                if gone[p] {
                    gone[k] = true;
                }
            }
        }
        let mut map = vec![usize::MAX; self.nodes.len()];
        let mut out = Vec::new();
        for (k, n) in self.nodes.iter().enumerate() {
            if !gone[k] {
                map[k] = out.len();
                out.push(n.clone());
            }
        }
        for n in &mut out {
            n.parent = n.parent.map(|p| map[p]);
        }
        self.nodes = out;
    }

    /// Spheres hanging from `i` (itself included).
    pub fn subtree(&self, i: usize) -> Vec<usize> {
        let mut inside = vec![false; self.nodes.len()];
        inside[i] = true;
        for k in i + 1..self.nodes.len() {
            if let Some(p) = self.nodes[k].parent {
                if inside[p] {
                    inside[k] = true;
                }
            }
        }
        (0..self.nodes.len()).filter(|&k| inside[k]).collect()
    }

    /// The sphere mirrored across X from `i`, if there is one.
    pub fn mirror_of(&self, i: usize) -> Option<usize> {
        let n = &self.nodes[i];
        let target = Vec3::new(-n.pos.x, n.pos.y, n.pos.z);
        (0..self.nodes.len())
            .filter(|&k| k != i)
            .find(|&k| self.nodes[k].pos.distance(target) < n.radius * 0.05 + 1e-4 && (self.nodes[k].radius - n.radius).abs() < n.radius * 0.05 + 1e-4)
    }

    /// Signed distance to the skin.
    pub fn distance(&self, p: Vec3) -> f32 {
        let mut d = f32::MAX;
        for n in &self.nodes {
            let di = match n.parent {
                Some(pi) => {
                    let q = &self.nodes[pi];
                    round_cone(p, q.pos, n.pos, q.radius, n.radius)
                }
                None => (p - n.pos).length() - n.radius,
            };
            let k = n.radius * 0.25;
            d = if d == f32::MAX { di } else { smooth_min(d, di, k) };
        }
        d
    }

    /// The skinned mesh at about `res` samples across.
    pub fn skin(&self, res: usize) -> Option<Mesh> {
        if self.nodes.is_empty() {
            return None;
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for n in &self.nodes {
            mn = mn.min(n.pos - Vec3::splat(n.radius * 1.3));
            mx = mx.max(n.pos + Vec3::splat(n.radius * 1.3));
        }
        let g = Grid::around(mn, mx, res);
        // only links near a sample matter: prune by bounds per sample would be
        // faster, but the trees are small
        let v = g.sample(&|p| self.distance(p));
        let m = mesh_field(&g, &v, 1);
        (!m.tris.is_empty()).then_some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_skins_into_one_closed_mesh() {
        let mut t = ZTree::default();
        let a = t.add(Vec3::ZERO, 0.4, None);
        let b = t.add(Vec3::new(1.0, 0.0, 0.0), 0.25, Some(a));
        t.add(Vec3::new(1.6, 0.5, 0.0), 0.15, Some(b));
        let m = t.skin(64).unwrap();
        assert!(!m.boundary.iter().any(|x| *x));
        let (mn, mx) = m.bounds();
        assert!(mn.x < -0.35 && mx.x > 1.7 && mx.y > 0.6, "{mn} {mx}");
        // the link is thinner in the middle than the root sphere
        assert!(t.distance(Vec3::new(0.5, 0.33, 0.0)) > 0.0);
        assert!(t.distance(Vec3::new(0.5, 0.0, 0.0)) < -0.2);
        t.checkpoint();
        t.remove(b);
        assert_eq!(t.nodes.len(), 1);
        t.undo();
        assert_eq!(t.nodes.len(), 3);
        assert_eq!(t.subtree(1), vec![1, 2]);
    }
}
