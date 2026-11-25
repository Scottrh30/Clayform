//! Compact binary encoding of meshes, used to hand work to the desktop
//! backend (which runs it on a native thread) and get the result back.
//!
//! Layout (little-endian): magic "CFM1", vertex count, triangle count,
//! stencil count (u32 each), export centre (3 × f32), export scale (f32),
//! positions (3 × f32 per vertex), masks (f32 per vertex), triangles
//! (3 × u32), stencils (4 × u32). Dead faces are skipped.

use crate::mesh::{Mesh, NONE};
use glam::Vec3;

const MAGIC: &[u8; 4] = b"CFM1";

pub fn encode(mesh: &Mesh, stencils: &[[u32; 4]]) -> Vec<u8> {
    let tris: Vec<[u32; 3]> = mesh.tris.iter().copied().filter(|t| t[0] != NONE).collect();
    let nv = mesh.positions.len();
    let mut out = Vec::with_capacity(32 + nv * 16 + tris.len() * 12 + stencils.len() * 16);
    out.extend_from_slice(MAGIC);
    for n in [nv as u32, tris.len() as u32, stencils.len() as u32] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    for f in mesh.export_center.to_array().into_iter().chain([mesh.export_scale]) {
        out.extend_from_slice(&f.to_le_bytes());
    }
    for p in &mesh.positions {
        for f in p.to_array() {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    for m in &mesh.mask {
        out.extend_from_slice(&m.to_le_bytes());
    }
    for t in &tris {
        for i in t {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    for s in stencils {
        for i in s {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.b.get(self.at..self.at + 4).ok_or("truncated mesh data")?;
        self.at += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
}

/// A decoded mesh before its connectivity is built (see `MeshBuilder`).
pub struct Parts {
    pub positions: Vec<Vec3>,
    pub mask: Vec<f32>,
    pub tris: Vec<[u32; 3]>,
    pub stencils: Vec<[u32; 4]>,
    pub center: Vec3,
    pub scale: f32,
}

impl Parts {
    /// Starts building the mesh (call `step` / `finish` on the result).
    pub fn into_builder(self) -> (crate::mesh::MeshBuilder, Vec<[u32; 4]>) {
        let b = crate::mesh::MeshBuilder::new(self.positions, self.tris).with_extras(Some(self.mask), self.center, self.scale);
        (b, self.stencils)
    }
}

/// Decodes a mesh (topology is rebuilt) and its stencils.
pub fn decode(bytes: &[u8]) -> Result<(Mesh, Vec<[u32; 4]>), String> {
    let (b, st) = decode_parts(bytes)?.into_builder();
    Ok((b.finish(), st))
}

/// Decodes the raw arrays without building connectivity.
pub fn decode_parts(bytes: &[u8]) -> Result<Parts, String> {
    if bytes.len() < 32 || &bytes[..4] != MAGIC {
        return Err("not Clayform mesh data".into());
    }
    let mut r = Reader { b: bytes, at: 4 };
    let (nv, nt, ns) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
    let expected = 32 + nv * 16 + nt * 12 + ns * 16;
    if bytes.len() != expected {
        return Err(format!("mesh data has the wrong size ({} bytes, expected {expected})", bytes.len()));
    }
    let center = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
    let scale = r.f32()?;
    let mut positions = Vec::with_capacity(nv);
    for _ in 0..nv {
        positions.push(Vec3::new(r.f32()?, r.f32()?, r.f32()?));
    }
    let mut mask = Vec::with_capacity(nv);
    for _ in 0..nv {
        mask.push(r.f32()?);
    }
    let mut tris = Vec::with_capacity(nt);
    for _ in 0..nt {
        let t = [r.u32()?, r.u32()?, r.u32()?];
        if t.iter().any(|&i| i as usize >= nv) {
            return Err("mesh data has a bad vertex index".into());
        }
        tris.push(t);
    }
    let mut stencils = Vec::with_capacity(ns);
    for _ in 0..ns {
        stencils.push([r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
    }
    Ok(Parts { positions, mask, tris, stencils, center, scale })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut m = Mesh::cube(1);
        m.mask[3] = 0.5;
        m.export_scale = 2.5;
        let (fine, odd) = m.subdivide_with_stencil(true);
        let (back, odd2) = decode(&encode(&fine, &odd)).unwrap();
        assert_eq!(back.positions, fine.positions);
        assert_eq!(back.tris, fine.tris);
        assert_eq!(back.mask, fine.mask);
        assert_eq!(odd2, odd);
        assert_eq!(back.export_scale, 2.5);
        assert!(decode(&encode(&fine, &odd)[..40]).is_err());
    }
}
