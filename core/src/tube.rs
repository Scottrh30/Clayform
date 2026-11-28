//! Geometry swept along a 3D path: round tubes and flat straps (Curve Tube
//! and Curve Strap brushes). Both are closed, so they voxel-remesh and
//! boolean cleanly.

use crate::mesh::Mesh;
use glam::Vec3;

/// Signed volume (positive when the faces point outwards).
pub fn signed_volume(m: &Mesh) -> f32 {
    m.tris
        .iter()
        .filter(|t| t[0] != crate::mesh::NONE)
        .map(|t| m.positions[t[0] as usize].dot(m.positions[t[1] as usize].cross(m.positions[t[2] as usize])) / 6.0)
        .sum()
}

/// Resamples `pts` (with matching surface normals) to even `step` spacing,
/// then smooths the path a little.
pub fn resample(pts: &[(Vec3, Vec3)], step: f32) -> Vec<(Vec3, Vec3)> {
    if pts.len() < 2 || step <= 0.0 {
        return pts.to_vec();
    }
    let total: f32 = pts.windows(2).map(|w| w[0].0.distance(w[1].0)).sum();
    if total < step * 0.5 {
        return vec![pts[0], *pts.last().unwrap()];
    }
    let n = (total / step).ceil().max(1.0) as usize;
    let mut out = Vec::with_capacity(n + 1);
    let mut seg = 0;
    let mut seg_start = 0.0;
    for k in 0..=n {
        let s = total * k as f32 / n as f32;
        while seg + 1 < pts.len() - 1 && seg_start + pts[seg].0.distance(pts[seg + 1].0) < s {
            seg_start += pts[seg].0.distance(pts[seg + 1].0);
            seg += 1;
        }
        let l = pts[seg].0.distance(pts[seg + 1].0).max(1e-9);
        let t = ((s - seg_start) / l).clamp(0.0, 1.0);
        let p = pts[seg].0.lerp(pts[seg + 1].0, t);
        let nn = pts[seg].1.lerp(pts[seg + 1].1, t).normalize_or_zero();
        out.push((p, nn));
    }
    // two gentle smoothing passes (ends stay put)
    for _ in 0..2 {
        let prev = out.clone();
        for i in 1..out.len().saturating_sub(1) {
            out[i].0 = prev[i].0 * 0.5 + (prev[i - 1].0 + prev[i + 1].0) * 0.25;
            out[i].1 = (prev[i].1 * 2.0 + prev[i - 1].1 + prev[i + 1].1).normalize_or_zero();
        }
    }
    out
}

/// Frames along the path: tangent, and a side vector across it that stays
/// as close as possible to the surface (from the normals).
fn frames(path: &[(Vec3, Vec3)]) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = path.len();
    (0..n)
        .map(|i| {
            let a = path[i.saturating_sub(1)].0;
            let b = path[(i + 1).min(n - 1)].0;
            let t = (b - a).normalize_or(Vec3::X);
            let mut up = path[i].1;
            up = (up - t * up.dot(t)).normalize_or(t.any_orthonormal_vector());
            let side = t.cross(up).normalize_or_zero();
            (t, up, side)
        })
        .collect()
}

/// Sweeps a closed cross-section (2D points in the side/up plane) along the
/// path, `scale(i)` sizing it per ring, and caps both ends.
fn sweep(path: &[(Vec3, Vec3)], section: &[(f32, f32)], lift: f32, scale: &dyn Fn(f32) -> f32) -> Mesh {
    let fr = frames(path);
    let ns = section.len();
    let n = path.len();
    let mut pos = Vec::with_capacity(n * ns + 2);
    for (i, ((p, _), (_, up, side))) in path.iter().zip(&fr).enumerate() {
        let s = scale(i as f32 / (n - 1).max(1) as f32);
        let c = *p + *up * lift;
        for &(x, y) in section {
            pos.push(c + (*side * x + *up * y) * s);
        }
    }
    let mut tris = Vec::new();
    for i in 0..n - 1 {
        for j in 0..ns {
            let a = (i * ns + j) as u32;
            let b = (i * ns + (j + 1) % ns) as u32;
            let c = a + ns as u32;
            let d = b + ns as u32;
            tris.push([a, c, b]);
            tris.push([b, c, d]);
        }
    }
    // caps: fans around a centre point
    let c0 = pos.len() as u32;
    pos.push(pos[..ns].iter().copied().sum::<Vec3>() / ns as f32);
    let c1 = pos.len() as u32;
    pos.push(pos[(n - 1) * ns..n * ns].iter().copied().sum::<Vec3>() / ns as f32);
    for j in 0..ns {
        let a = j as u32;
        let b = ((j + 1) % ns) as u32;
        tris.push([c0, a, b]);
        let base = ((n - 1) * ns) as u32;
        tris.push([c1, base + b, base + a]);
    }
    let mut m = Mesh::new(pos, tris);
    // the winding above may face inwards depending on the frame handedness
    if signed_volume(&m) < 0.0 {
        for t in &mut m.tris {
            t.swap(1, 2);
        }
        m = Mesh::new(m.positions.clone(), m.tris.clone());
    }
    m
}

/// A round tube of `radius`, its centre `lift` above the path.
pub fn tube(path: &[(Vec3, Vec3)], radius: f32, lift: f32, taper: bool) -> Option<Mesh> {
    if path.len() < 2 {
        return None;
    }
    let sides = 16;
    let section: Vec<(f32, f32)> = (0..sides)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / sides as f32;
            (a.cos() * radius, a.sin() * radius)
        })
        .collect();
    let scale = move |t: f32| if taper { 1.0 - 0.8 * t } else { 1.0 };
    Some(sweep(path, &section, lift, &scale))
}

/// A flat strap `width` wide and `thickness` thick, lying on the surface.
pub fn strap(path: &[(Vec3, Vec3)], width: f32, thickness: f32) -> Option<Mesh> {
    if path.len() < 2 {
        return None;
    }
    let (hw, ht) = (width * 0.5, thickness * 0.5);
    // rounded-edge rectangle
    let section = [(-hw, -ht), (hw, -ht), (hw * 1.04, 0.0), (hw, ht), (-hw, ht), (-hw * 1.04, 0.0)];
    Some(sweep(path, &section, ht * 0.6, &|_| 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc() -> Vec<(Vec3, Vec3)> {
        (0..30)
            .map(|i| {
                let a = i as f32 / 29.0 * 2.0;
                (Vec3::new(a.cos(), a.sin(), 0.0), Vec3::new(a.cos(), a.sin(), 0.0))
            })
            .collect()
    }

    #[test]
    fn tubes_and_straps_are_closed_and_outward() {
        let p = resample(&arc(), 0.1);
        assert!(p.len() > 10);
        for m in [tube(&p, 0.1, 0.05, false).unwrap(), tube(&p, 0.1, 0.0, true).unwrap(), strap(&p, 0.3, 0.05).unwrap()] {
            assert!(!m.boundary.iter().any(|b| *b), "open edges");
            assert!(signed_volume(&m) > 0.0);
        }
        let t = tube(&p, 0.1, 0.0, false).unwrap();
        let expect = std::f32::consts::PI * 0.01 * 2.0; // area * length of the arc
        assert!((signed_volume(&t) - expect).abs() < expect * 0.15, "{}", signed_volume(&t));
    }
}
