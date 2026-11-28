//! OBJ and STL import / export.

use crate::mesh::Mesh;
use glam::Vec3;
use std::io::Write;

/// Parses an OBJ or STL file (chosen by the file name's extension) and
/// normalises it for sculpting.
pub fn load(file_name: &str, bytes: &[u8]) -> Result<Mesh, String> {
    let ext = extension(file_name);
    let mut mesh = match ext.as_str() {
        "obj" => parse_obj(&String::from_utf8_lossy(bytes))?,
        "stl" => parse_stl(bytes)?,
        _ => return Err(format!("Unsupported format '.{ext}' (use .obj or .stl)")),
    };
    if mesh.tris.is_empty() {
        return Err("The file contains no triangles".into());
    }
    mesh.normalize_for_sculpting();
    Ok(mesh)
}

/// Serialises the mesh as OBJ (`"obj"`) or binary STL (`"stl"`).
pub fn save(mesh: &Mesh, ext: &str) -> Vec<u8> {
    let mut w = Vec::new();
    let r = match ext.to_ascii_lowercase().as_str() {
        "stl" => write_stl(mesh, &mut w),
        _ => write_obj(mesh, &mut w),
    };
    r.expect("writing to memory cannot fail");
    w
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

#[inline]
fn export_pos(mesh: &Mesh, p: Vec3) -> Vec3 {
    p / mesh.export_scale + mesh.export_center
}

pub fn parse_obj(src: &str) -> Result<Mesh, String> {
    // quads become partner pairs; "g" / "o" names become polygroups
    let mut p = crate::poly::Polys {
        pos: Vec::new(),
        mask: Vec::new(),
        faces: Vec::new(),
        groups: Vec::new(),
        export_center: Vec3::ZERO,
        export_scale: 1.0,
    };
    let mut group_ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut group = 0u32;
    for (ln, line) in src.lines().enumerate() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let mut c = [0f32; 3];
                for k in 0..3 {
                    c[k] = it
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| format!("Bad vertex on line {}", ln + 1))?;
                }
                p.pos.push(Vec3::from(c));
            }
            Some("g") | Some("o") => {
                let name = it.collect::<Vec<_>>().join(" ");
                let next = group_ids.len() as u32;
                group = *group_ids.entry(name).or_insert(next);
            }
            Some("f") => {
                let mut poly: Vec<u32> = Vec::with_capacity(4);
                for tok in it {
                    let first = tok.split('/').next().unwrap_or("");
                    let i: i64 = first.parse().map_err(|_| format!("Bad face on line {}", ln + 1))?;
                    let idx = if i < 0 { p.pos.len() as i64 + i } else { i - 1 };
                    if idx < 0 || idx as usize >= p.pos.len() {
                        return Err(format!("Face index out of range on line {}", ln + 1));
                    }
                    poly.push(idx as u32);
                }
                poly.dedup();
                if poly.len() >= 3 {
                    p.faces.push(poly);
                    p.groups.push(group);
                }
            }
            _ => {}
        }
    }
    p.mask = vec![0.0; p.pos.len()];
    Ok(p.to_mesh())
}

pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, String> {
    let is_binary = bytes.len() >= 84 && {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        84 + n * 50 == bytes.len()
    };
    let mut soup = Vec::new();
    if is_binary {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        let rd = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        for i in 0..n {
            let base = 84 + i * 50 + 12;
            for k in 0..3 {
                let o = base + k * 12;
                soup.push(Vec3::new(rd(o), rd(o + 4), rd(o + 8)));
            }
        }
    } else {
        let text = String::from_utf8_lossy(bytes);
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if it.next() == Some("vertex") {
                let c: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() == 3 {
                    soup.push(Vec3::new(c[0], c[1], c[2]));
                }
            }
        }
        soup.truncate(soup.len() / 3 * 3);
    }
    Ok(Mesh::from_soup_welded(&soup))
}

fn write_obj<W: Write>(mesh: &Mesh, w: &mut W) -> std::io::Result<()> {
    // quads are written as quads, and each polygroup as its own "g" group
    let (polys, _) = crate::poly::Polys::from_mesh(mesh);
    writeln!(w, "# Exported from Clayform")?;
    writeln!(w, "o sculpt")?;
    for p in &mesh.positions {
        let p = export_pos(mesh, *p);
        writeln!(w, "v {:.6} {:.6} {:.6}", p.x, p.y, p.z)?;
    }
    for n in &mesh.normals {
        writeln!(w, "vn {:.5} {:.5} {:.5}", n.x, n.y, n.z)?;
    }
    let mut order: Vec<usize> = (0..polys.faces.len()).collect();
    order.sort_by_key(|&f| polys.groups[f]);
    let several = polys.groups.iter().any(|&g| g != polys.groups[0]);
    let mut current = u32::MAX;
    for f in order {
        if several && polys.groups[f] != current {
            current = polys.groups[f];
            writeln!(w, "g group{current}")?;
        }
        write!(w, "f")?;
        for &v in &polys.faces[f] {
            write!(w, " {}//{}", v + 1, v + 1)?;
        }
        writeln!(w)?;
    }
    Ok(())
}

fn write_stl<W: Write>(mesh: &Mesh, w: &mut W) -> std::io::Result<()> {
    let mut header = [0u8; 80];
    let tag = b"Clayform binary STL";
    header[..tag.len()].copy_from_slice(tag);
    w.write_all(&header)?;
    w.write_all(&(mesh.tris.len() as u32).to_le_bytes())?;
    for (f, t) in mesh.tris.iter().enumerate() {
        let n = mesh.face_normals[f].normalize_or_zero();
        for c in n.to_array() {
            w.write_all(&c.to_le_bytes())?;
        }
        for &v in t {
            let p = export_pos(mesh, mesh.positions[v as usize]);
            for c in p.to_array() {
                w.write_all(&c.to_le_bytes())?;
            }
        }
        w.write_all(&[0, 0])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_roundtrip_with_quads_and_scale() {
        let src = "v 0 0 0\nv 4 0 0\nv 4 4 0\nv 0 4 0\nf 1/1/1 2/2/2 3/3/3 4/4/4\n";
        let mut m = parse_obj(src).unwrap();
        assert_eq!(m.tris.len(), 2);
        m.normalize_for_sculpting();
        let mut buf = Vec::new();
        write_obj(&m, &mut buf).unwrap();
        let back = parse_obj(&String::from_utf8(buf).unwrap()).unwrap();
        assert!((back.positions[2] - Vec3::new(4.0, 4.0, 0.0)).length() < 1e-4);
    }

    #[test]
    fn load_and_save_by_extension() {
        let bytes = save(&Mesh::icosphere(1), "stl");
        let m = load("ball.STL", &bytes).unwrap();
        assert_eq!(m.tris.len(), 80);
        assert!(load("ball.fbx", &bytes).is_err());
        assert!(String::from_utf8(save(&m, "obj")).unwrap().starts_with("# Exported from Clayform"));
    }

    #[test]
    fn obj_keeps_quads_and_groups() {
        let q = crate::poly::Polys::qcube(2).to_mesh();
        let mut buf = Vec::new();
        write_obj(&q, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 24);
        assert_eq!(text.lines().filter(|l| l.starts_with("g ")).count(), 6);
        let back = parse_obj(&text).unwrap();
        assert_eq!(back.tris.len(), 48);
        assert!(back.partner.iter().all(|&p| p != crate::mesh::NONE));
        let mut g = back.groups.clone();
        g.sort_unstable();
        g.dedup();
        assert_eq!(g.len(), 6);
    }

    #[test]
    fn stl_roundtrip() {
        let m = Mesh::icosphere(2);
        let mut buf = Vec::new();
        write_stl(&m, &mut buf).unwrap();
        let back = parse_stl(&buf).unwrap();
        assert_eq!(back.tris.len(), m.tris.len());
        assert_eq!(back.positions.len(), m.positions.len());
    }
}
