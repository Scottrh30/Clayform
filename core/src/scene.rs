//! Several objects (SubTools) in one sculpt: their names, visibility,
//! boolean role and array (repeat) settings.

use crate::mesh::{Mesh, MeshBuilder, NONE};
use glam::{Mat4, Quat, Vec3};

/// How a SubTool combines with the ones above it in a boolean.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoolOp {
    Add,
    Subtract,
    Intersect,
}

impl BoolOp {
    pub const ALL: [BoolOp; 3] = [BoolOp::Add, BoolOp::Subtract, BoolOp::Intersect];
    pub fn name(self) -> &'static str {
        match self {
            BoolOp::Add => "Add",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
        }
    }
    pub fn code(self) -> char {
        match self {
            BoolOp::Add => 'a',
            BoolOp::Subtract => 's',
            BoolOp::Intersect => 'i',
        }
    }
    pub fn from_code(c: char) -> BoolOp {
        match c {
            's' => BoolOp::Subtract,
            'i' => BoolOp::Intersect,
            _ => BoolOp::Add,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArrayKind {
    /// Copies in a row, each moved (and turned, scaled) a step further.
    Line,
    /// Copies around an axis through the object's centre offset by a radius.
    Circle,
    /// A 3D grid of copies.
    Grid,
}

impl ArrayKind {
    pub const ALL: [ArrayKind; 3] = [ArrayKind::Line, ArrayKind::Circle, ArrayKind::Grid];
    pub fn name(self) -> &'static str {
        match self {
            ArrayKind::Line => "Line",
            ArrayKind::Circle => "Circle",
            ArrayKind::Grid => "Grid",
        }
    }
}

/// ArrayMesh-style repeats of a SubTool, shown live until converted.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ArraySettings {
    pub kind: ArrayKind,
    /// Copies (line and circle), including the original.
    pub count: u32,
    /// Line: offset between copies, in object sizes (×, y, z).
    pub offset: Vec3,
    /// Line: turn per copy (degrees about x, y, z) and scale per copy.
    pub turn: Vec3,
    pub scale: f32,
    /// Circle: axis (0 = X, 1 = Y, 2 = Z), radius in object sizes, and how
    /// far round the copies go (degrees).
    pub axis: usize,
    pub radius: f32,
    pub sweep: f32,
    /// Grid: copies along x, y, z and the gap between them (object sizes).
    pub grid: [u32; 3],
    pub gap: f32,
}

impl Default for ArraySettings {
    fn default() -> Self {
        Self {
            kind: ArrayKind::Line,
            count: 4,
            offset: Vec3::new(1.2, 0.0, 0.0),
            turn: Vec3::ZERO,
            scale: 1.0,
            axis: 1,
            radius: 1.0,
            sweep: 360.0,
            grid: [3, 1, 3],
            gap: 0.2,
        }
    }
}

/// Most copies an array may have.
pub const MAX_COPIES: usize = 64;

impl ArraySettings {
    /// One transform per copy (the first is the original), for a mesh with
    /// bounds `(mn, mx)`.
    pub fn transforms(&self, mn: Vec3, mx: Vec3) -> Vec<Mat4> {
        let size = (mx - mn).max_element().max(1e-6);
        let c = (mn + mx) * 0.5;
        let mut out = Vec::new();
        match self.kind {
            ArrayKind::Line => {
                let n = self.count.clamp(1, MAX_COPIES as u32);
                let mut m = Mat4::IDENTITY;
                let step = Mat4::from_translation(c + self.offset * size)
                    * Mat4::from_quat(Quat::from_euler(
                        glam::EulerRot::XYZ,
                        self.turn.x.to_radians(),
                        self.turn.y.to_radians(),
                        self.turn.z.to_radians(),
                    ))
                    * Mat4::from_scale(Vec3::splat(self.scale.max(0.05)))
                    * Mat4::from_translation(-c);
                for _ in 0..n {
                    out.push(m);
                    m = step * m;
                }
            }
            ArrayKind::Circle => {
                let n = self.count.clamp(1, MAX_COPIES as u32);
                let axis = [Vec3::X, Vec3::Y, Vec3::Z][self.axis.min(2)];
                // the circle's centre sits `radius` sizes away, beside the object
                let side = if self.axis == 0 { Vec3::Y } else { Vec3::X };
                let pivot = c - side * self.radius * size;
                let full = (self.sweep - 360.0).abs() < 1e-3;
                let div = if full { n as f32 } else { (n.max(2) - 1) as f32 };
                for i in 0..n {
                    let a = self.sweep.to_radians() * i as f32 / div;
                    out.push(
                        Mat4::from_translation(pivot) * Mat4::from_axis_angle(axis, a) * Mat4::from_translation(-pivot),
                    );
                }
            }
            ArrayKind::Grid => {
                let e = mx - mn;
                let step = e + Vec3::splat(self.gap * size);
                let g = self.grid.map(|v| v.clamp(1, 16));
                'outer: for k in 0..g[2] {
                    for j in 0..g[1] {
                        for i in 0..g[0] {
                            if out.len() >= MAX_COPIES {
                                break 'outer;
                            }
                            out.push(Mat4::from_translation(Vec3::new(i as f32, j as f32, k as f32) * step));
                        }
                    }
                }
            }
        }
        out.truncate(MAX_COPIES);
        out
    }
}

/// What the scene knows about each SubTool besides its sculpt.
#[derive(Clone, Debug, PartialEq)]
pub struct SubToolInfo {
    /// Stable id (for the renderer's buffers).
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub op: BoolOp,
    pub array: Option<ArraySettings>,
}

/// The mesh with its array copies baked in (live triangles only).
pub fn bake(mesh: &Mesh, xfs: &[Mat4]) -> Mesh {
    let live: Vec<[u32; 3]> = mesh.tris.iter().copied().filter(|t| t[0] != NONE).collect();
    let groups: Vec<u32> = (0..mesh.tris.len()).filter(|&f| mesh.tris[f][0] != NONE).map(|f| mesh.groups[f]).collect();
    let n = mesh.positions.len() as u32;
    let mut pos = Vec::with_capacity(mesh.positions.len() * xfs.len());
    let mut mask = Vec::new();
    let mut tris = Vec::with_capacity(live.len() * xfs.len());
    let mut gs = Vec::new();
    for (k, m) in xfs.iter().enumerate() {
        let flip = m.determinant() < 0.0;
        pos.extend(mesh.positions.iter().map(|p| m.transform_point3(*p)));
        mask.extend_from_slice(&mesh.mask);
        let base = n * k as u32;
        for t in &live {
            let t = [t[0] + base, t[1] + base, t[2] + base];
            tris.push(if flip { [t[0], t[2], t[1]] } else { t });
        }
        gs.extend_from_slice(&groups);
    }
    let mut out = MeshBuilder::new(pos, tris).with_faces(Some(gs), None).finish();
    out.mask = mask;
    out.export_center = mesh.export_center;
    out.export_scale = mesh.export_scale;
    out
}

/// Several meshes as one (for merging and booleans); also returns where each
/// part's live triangles end.
pub fn concat(meshes: &[&Mesh]) -> (Mesh, Vec<usize>) {
    let mut pos = Vec::new();
    let mut mask = Vec::new();
    let mut tris = Vec::new();
    let mut groups = Vec::new();
    let mut ends = Vec::new();
    let mut gbase = 0;
    for m in meshes {
        let base = pos.len() as u32;
        pos.extend_from_slice(&m.positions);
        mask.extend_from_slice(&m.mask);
        let mut gmax = 0;
        for (f, t) in m.tris.iter().enumerate() {
            if t[0] != NONE {
                tris.push([t[0] + base, t[1] + base, t[2] + base]);
                groups.push(m.groups[f] + gbase);
                gmax = gmax.max(m.groups[f] + 1);
            }
        }
        gbase += gmax;
        ends.push(tris.len());
    }
    let mut out = MeshBuilder::new(pos, tris).with_faces(Some(groups), None).finish();
    out.mask = mask;
    if let Some(m) = meshes.first() {
        out.export_center = m.export_center;
        out.export_scale = m.export_scale;
    }
    (out, ends)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_transforms() {
        let a = ArraySettings { count: 3, offset: Vec3::new(1.0, 0.0, 0.0), ..Default::default() };
        let t = a.transforms(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(t.len(), 3);
        assert!((t[2].transform_point3(Vec3::ZERO) - Vec3::new(4.0, 0.0, 0.0)).length() < 1e-4);
        let c = ArraySettings { kind: ArrayKind::Circle, count: 4, radius: 1.0, ..Default::default() };
        let t = c.transforms(Vec3::splat(-1.0), Vec3::splat(1.0));
        // radius 1 object size (2 units) round Y: the opposite copy is 4 units away
        assert!((t[2].transform_point3(Vec3::ZERO) - Vec3::new(-4.0, 0.0, 0.0)).length() < 1e-4);
        let g = ArraySettings { kind: ArrayKind::Grid, grid: [2, 2, 2], gap: 0.0, ..Default::default() };
        assert_eq!(g.transforms(Vec3::ZERO, Vec3::ONE).len(), 8);
        let m = Mesh::icosphere(1);
        let baked = bake(&m, &a.transforms(Vec3::splat(-1.0), Vec3::splat(1.0)));
        assert_eq!(baked.tris.len(), m.tris.len() * 3);
        let (both, ends) = concat(&[&m, &baked]);
        assert_eq!(ends, vec![80, 320]);
        assert_eq!(both.tris.len(), 320);
    }
}
