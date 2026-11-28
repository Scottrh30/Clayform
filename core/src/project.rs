//! Clayform project files (.clayform): every multiresolution level with its
//! detail and mask, plus a JSON blob of interface settings (camera, brush,
//! view) that the interface writes and reads itself.
//!
//! Layout (little-endian): magic "CLAYFORM", format version (u32), settings
//! length (u32) + UTF-8 JSON, SubTool count (u32), active SubTool (u32), then
//! per SubTool: info length (u32) + UTF-8 JSON, level count (u32), active
//! level (u32), then per level: mesh length (u64) + mesh in the `wire` format
//! (stencils = the Loop stencils that build it from the level below), detail
//! count (u32) + details (3 × f32 each). Version 1 files have no SubTool
//! count, active SubTool or info: one object.

use crate::wire;
use glam::Vec3;

const MAGIC: &[u8; 8] = b"CLAYFORM";
/// 1: one object. 2: several SubTools, each with an info JSON blob.
const VERSION: u32 = 2;
pub const EXTENSION: &str = "clayform";

/// One level as stored in a project.
pub struct LevelData<M> {
    pub mesh: M,
    pub stencils: Vec<[u32; 4]>,
    pub details: Vec<Vec3>,
}

/// One SubTool: its info (JSON written by the interface), levels and active level.
pub struct SubToolData<M> {
    pub info_json: String,
    pub levels: Vec<LevelData<M>>,
    pub active: usize,
}

/// What a project file holds once read (meshes still to be built).
pub struct ProjectParts {
    pub settings_json: String,
    pub subtools: Vec<SubToolData<wire::Parts>>,
    pub active_subtool: usize,
}

/// A project with a single object.
pub fn encode(levels: &[LevelData<&crate::mesh::Mesh>], active: usize, settings_json: &str) -> Vec<u8> {
    let st = SubToolData {
        info_json: String::new(),
        levels: levels.iter().map(|l| LevelData { mesh: l.mesh, stencils: l.stencils.clone(), details: l.details.clone() }).collect(),
        active,
    };
    encode_scene(&[st], 0, settings_json)
}

fn put_levels(out: &mut Vec<u8>, levels: &[LevelData<&crate::mesh::Mesh>], active: usize) {
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
}

pub fn encode_scene(subtools: &[SubToolData<&crate::mesh::Mesh>], active_subtool: usize, settings_json: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(settings_json.len() as u32).to_le_bytes());
    out.extend_from_slice(settings_json.as_bytes());
    out.extend_from_slice(&(subtools.len() as u32).to_le_bytes());
    out.extend_from_slice(&(active_subtool as u32).to_le_bytes());
    for st in subtools {
        out.extend_from_slice(&(st.info_json.len() as u32).to_le_bytes());
        out.extend_from_slice(st.info_json.as_bytes());
        put_levels(&mut out, &st.levels, st.active);
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
    let (count, active_subtool) = if version >= 2 {
        (u32_of(take(4)?) as usize, u32_of(take(4)?) as usize)
    } else {
        (1, 0)
    };
    if count == 0 || active_subtool >= count {
        return Err(bad());
    }
    let mut subtools = Vec::with_capacity(count);
    for _ in 0..count {
        let info_json = if version >= 2 {
            let il = u32_of(take(4)?) as usize;
            String::from_utf8(take(il)?.to_vec()).map_err(|_| bad())?
        } else {
            String::new()
        };
        let n = u32_of(take(4)?) as usize;
        let active = u32_of(take(4)?) as usize;
        if n == 0 || active >= n {
            return Err(bad());
        }
        let mut levels = Vec::with_capacity(n);
        for _ in 0..n {
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
        subtools.push(SubToolData { info_json, levels, active });
    }
    Ok(ProjectParts { settings_json, subtools, active_subtool })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    #[test]
    fn scene_round_trip_keeps_every_subtool() {
        let a = Mesh::icosphere(2);
        let b = crate::poly::Polys::qcube(1).to_mesh();
        let sts = vec![
            SubToolData { info_json: "{\"name\":\"A\"}".into(), levels: vec![LevelData { mesh: &a, stencils: vec![], details: vec![] }], active: 0 },
            SubToolData { info_json: "{\"name\":\"B\"}".into(), levels: vec![LevelData { mesh: &b, stencils: vec![], details: vec![] }], active: 0 },
        ];
        let bytes = encode_scene(&sts, 1, "{}");
        let p = decode(&bytes).unwrap();
        assert_eq!(p.subtools.len(), 2);
        assert_eq!(p.active_subtool, 1);
        assert_eq!(p.subtools[1].info_json, "{\"name\":\"B\"}");
        assert_eq!(p.subtools[0].levels[0].mesh.tris.len(), a.tris.len());
        assert_eq!(p.subtools[1].levels[0].mesh.tris.len(), b.tris.len());
        // a single-object project still opens
        let one = encode(&[LevelData { mesh: &a, stencils: vec![], details: vec![] }], 0, "{}");
        assert_eq!(decode(&one).unwrap().subtools.len(), 1);
    }
}
