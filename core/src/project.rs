//! Clayform project files (.clayform): every multiresolution level with its
//! detail and mask, plus a JSON blob of interface settings (camera, brush,
//! view) that the interface writes and reads itself.
//!
//! Layout (little-endian): magic "CLAYFORM", format version (u32), settings
//! length (u32) + UTF-8 JSON, level count (u32), active level (u32), then per
//! level: mesh length (u64) + mesh in the `wire` format (stencils = the Loop
//! stencils that build it from the level below), detail count (u32) +
//! details (3 × f32 each).

use crate::wire;
use glam::Vec3;

const MAGIC: &[u8; 8] = b"CLAYFORM";
const VERSION: u32 = 1;
pub const EXTENSION: &str = "clayform";

/// One level as stored in a project.
pub struct LevelData<M> {
    pub mesh: M,
    pub stencils: Vec<[u32; 4]>,
    pub details: Vec<Vec3>,
}

/// What a project file holds once read (meshes still to be built).
pub struct ProjectParts {
    pub settings_json: String,
    pub levels: Vec<LevelData<wire::Parts>>,
    pub active: usize,
}

pub fn encode(levels: &[LevelData<&crate::mesh::Mesh>], active: usize, settings_json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(settings_json.len() as u32).to_le_bytes());
    out.extend_from_slice(settings_json.as_bytes());
    out.extend_from_slice(&(levels.len() as u32).to_le_bytes());
    out.extend_from_slice(&(active as u32).to_le_bytes());
    for l in levels {
        let m = wire::encode(l.mesh, &l.stencils);
        out.extend_from_slice(&(m.len() as u64).to_le_bytes());
        out.extend_from_slice(&m);
        out.extend_from_slice(&(l.details.len() as u32).to_le_bytes());
        for d in &l.details {
            for f in d.to_array() {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
    }
    out
}

pub fn decode(bytes: &[u8]) -> Result<ProjectParts, String> {
    let bad = || "This isn't a Clayform project, or the file is damaged".to_string();
    if bytes.len() < 24 || &bytes[..8] != MAGIC {
        return Err(bad());
    }
    let mut at = 8;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let s = bytes.get(at..at + n).ok_or_else(bad)?;
        at += n;
        Ok(s)
    };
    let u32_of = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    let version = u32_of(take(4)?);
    if version > VERSION {
        return Err("This project was saved by a newer version of Clayform".into());
    }
    let jl = u32_of(take(4)?) as usize;
    let settings_json = String::from_utf8(take(jl)?.to_vec()).map_err(|_| bad())?;
    let count = u32_of(take(4)?) as usize;
    let active = u32_of(take(4)?) as usize;
    if count == 0 || active >= count {
        return Err(bad());
    }
    let mut levels = Vec::with_capacity(count);
    for _ in 0..count {
        let ml = take(8)?;
        let ml = u64::from_le_bytes(ml.try_into().unwrap()) as usize;
        let mesh = wire::decode_parts(take(ml)?)?;
        let nd = u32_of(take(4)?) as usize;
        let raw = take(nd * 12)?;
        let details = raw
            .chunks_exact(12)
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                Vec3::new(f(0), f(4), f(8))
            })
            .collect();
        let stencils = mesh.stencils.clone();
        levels.push(LevelData { mesh, stencils, details });
    }
    Ok(ProjectParts { settings_json, levels, active })
}
