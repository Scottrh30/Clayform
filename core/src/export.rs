//! Export writers: PLY (binary), glTF binary (GLB) and FBX (binary 7.4).

use crate::io::Placement;
use crate::mesh::{Mesh, NONE};
use crate::poly::{group_color, Polys};
use glam::Vec3;
use std::io::{Result, Write};

/// Live triangles only.
fn live_tris(mesh: &Mesh) -> impl Iterator<Item = (usize, &[u32; 3])> {
    mesh.tris.iter().enumerate().filter(|(_, t)| t[0] != NONE)
}

/// Each vertex's colour: the polygroup colour of its first face.
fn vertex_colors(mesh: &Mesh) -> Vec<[u8; 3]> {
    let mut out = vec![[200u8, 200, 200]; mesh.positions.len()];
    let mut set = vec![false; mesh.positions.len()];
    for (f, t) in live_tris(mesh) {
        let c = group_color(mesh.groups[f]).map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8);
        for &v in t {
            if !set[v as usize] {
                set[v as usize] = true;
                out[v as usize] = c;
            }
        }
    }
    out
}

// ------------------------------------------------------------------- PLY

/// Binary PLY: positions, normals, polygroup colours; quads stay quads.
pub fn write_ply<W: Write>(mesh: &Mesh, pl: &Placement, w: &mut W) -> Result<()> {
    let (polys, _) = Polys::from_mesh(mesh);
    let colors = vertex_colors(mesh);
    write!(
        w,
        "ply\nformat binary_little_endian 1.0\ncomment Exported from Clayform\nelement vertex {}\n\
         property float x\nproperty float y\nproperty float z\n\
         property float nx\nproperty float ny\nproperty float nz\n\
         property uchar red\nproperty uchar green\nproperty uchar blue\n\
         element face {}\nproperty list uchar int vertex_indices\nend_header\n",
        mesh.positions.len(),
        polys.faces.len()
    )?;
    for (i, p) in mesh.positions.iter().enumerate() {
        let p = pl.pos(*p);
        let n = pl.nrm(mesh.normals[i]);
        for c in [p.x, p.y, p.z, n.x, n.y, n.z] {
            w.write_all(&c.to_le_bytes())?;
        }
        w.write_all(&colors[i])?;
    }
    for f in &polys.faces {
        w.write_all(&[f.len() as u8])?;
        for &v in f {
            w.write_all(&(v as i32).to_le_bytes())?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- GLB

/// glTF 2.0 binary: one mesh with positions, normals, polygroup colours and
/// a clay-coloured material.
pub fn write_glb<W: Write>(mesh: &Mesh, pl: &Placement, w: &mut W) -> Result<()> {
    let n = mesh.positions.len();
    let mut bin: Vec<u8> = Vec::new();
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &mesh.positions {
        let p = pl.pos(*p);
        mn = mn.min(p);
        mx = mx.max(p);
        for c in p.to_array() {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    if n == 0 {
        mn = Vec3::ZERO;
        mx = Vec3::ZERO;
    }
    let nrm_off = bin.len();
    for q in &mesh.normals {
        let q = pl.nrm(q.normalize_or(Vec3::Y));
        for c in q.to_array() {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let col_off = bin.len();
    for c in vertex_colors(mesh) {
        bin.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    let idx_off = bin.len();
    let mut count = 0usize;
    for (_, t) in live_tris(mesh) {
        for &v in t {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        count += 3;
    }
    let json = serde_json_lite(n, count, mn, mx, nrm_off, col_off, idx_off, bin.len());
    let mut json = json.into_bytes();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    w.write_all(b"glTF")?;
    w.write_all(&2u32.to_le_bytes())?;
    w.write_all(&(total as u32).to_le_bytes())?;
    w.write_all(&(json.len() as u32).to_le_bytes())?;
    w.write_all(b"JSON")?;
    w.write_all(&json)?;
    w.write_all(&(bin.len() as u32).to_le_bytes())?;
    w.write_all(b"BIN\0")?;
    w.write_all(&bin)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn serde_json_lite(
    n: usize,
    count: usize,
    mn: Vec3,
    mx: Vec3,
    nrm_off: usize,
    col_off: usize,
    idx_off: usize,
    total: usize,
) -> String {
    let f = |v: f32| if v.is_finite() { format!("{v}") } else { "0".into() };
    format!(
        concat!(
            r#"{{"asset":{{"version":"2.0","generator":"Clayform"}},"scene":0,"scenes":[{{"nodes":[0]}}],"#,
            r#""nodes":[{{"mesh":0,"name":"Sculpt"}}],"#,
            r#""meshes":[{{"name":"Sculpt","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3,"material":0}}]}}],"#,
            r#""materials":[{{"name":"Clay","pbrMetallicRoughness":{{"baseColorFactor":[0.8,0.62,0.52,1.0],"metallicFactor":0.0,"roughnessFactor":0.65}}}}],"#,
            r#""accessors":["#,
            r#"{{"bufferView":0,"componentType":5126,"count":{n},"type":"VEC3","min":[{a},{b},{c}],"max":[{d},{e},{g}]}},"#,
            r#"{{"bufferView":1,"componentType":5126,"count":{n},"type":"VEC3"}},"#,
            r#"{{"bufferView":2,"componentType":5121,"normalized":true,"count":{n},"type":"VEC4"}},"#,
            r#"{{"bufferView":3,"componentType":5125,"count":{count},"type":"SCALAR"}}],"#,
            r#""bufferViews":["#,
            r#"{{"buffer":0,"byteOffset":0,"byteLength":{l0},"target":34962}},"#,
            r#"{{"buffer":0,"byteOffset":{nrm_off},"byteLength":{l1},"target":34962}},"#,
            r#"{{"buffer":0,"byteOffset":{col_off},"byteLength":{l2},"target":34962}},"#,
            r#"{{"buffer":0,"byteOffset":{idx_off},"byteLength":{l3},"target":34963}}],"#,
            r#""buffers":[{{"byteLength":{total4}}}]}}"#
        ),
        n = n,
        count = count,
        a = f(mn.x),
        b = f(mn.y),
        c = f(mn.z),
        d = f(mx.x),
        e = f(mx.y),
        g = f(mx.z),
        l0 = nrm_off,
        nrm_off = nrm_off,
        l1 = col_off - nrm_off,
        col_off = col_off,
        l2 = idx_off - col_off,
        idx_off = idx_off,
        l3 = total - idx_off,
        total4 = total.div_ceil(4) * 4,
    )
}

// ------------------------------------------------------------------- FBX

/// A node of an FBX document.
struct Node {
    name: &'static str,
    props: Vec<Prop>,
    children: Vec<Node>,
}

enum Prop {
    I32(i32),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(Vec<u8>),
    Raw(Vec<u8>),
    ArrF64(Vec<f64>),
    ArrI32(Vec<i32>),
}

fn node(name: &'static str, props: Vec<Prop>, children: Vec<Node>) -> Node {
    Node { name, props, children }
}

fn s(v: &str) -> Prop {
    Prop::Str(v.as_bytes().to_vec())
}

/// FBX object names are "Name\x00\x01Class".
fn obj_name(name: &str, class: &str) -> Prop {
    let mut v = name.as_bytes().to_vec();
    v.extend_from_slice(b"\x00\x01");
    v.extend_from_slice(class.as_bytes());
    Prop::Str(v)
}

fn p70(entries: Vec<Vec<Prop>>) -> Node {
    node("Properties70", vec![], entries.into_iter().map(|e| node("P", e, vec![])).collect())
}

impl Prop {
    fn encode(&self, out: &mut Vec<u8>) {
        fn arr<T: Copy>(out: &mut Vec<u8>, code: u8, v: &[T], put: impl Fn(&mut Vec<u8>, T), size: usize) {
            out.push(code);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // not compressed
            out.extend_from_slice(&((v.len() * size) as u32).to_le_bytes());
            for &x in v {
                put(out, x);
            }
        }
        match self {
            Prop::I32(v) => {
                out.push(b'I');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Prop::I64(v) => {
                out.push(b'L');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Prop::F64(v) => {
                out.push(b'D');
                out.extend_from_slice(&v.to_le_bytes());
            }
            Prop::Bool(v) => {
                out.push(b'C');
                out.push(*v as u8);
            }
            Prop::Str(v) | Prop::Raw(v) => {
                out.push(if matches!(self, Prop::Str(_)) { b'S' } else { b'R' });
                out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                out.extend_from_slice(v);
            }
            Prop::ArrF64(v) => arr(out, b'd', v, |o, x| o.extend_from_slice(&x.to_le_bytes()), 8),
            Prop::ArrI32(v) => arr(out, b'i', v, |o, x| o.extend_from_slice(&x.to_le_bytes()), 4),
        }
    }
}

const NULL_RECORD: [u8; 13] = [0; 13];

impl Node {
    /// Writes this record at the end of `out` (offsets are absolute).
    fn write(&self, out: &mut Vec<u8>, is_last: bool) {
        let start = out.len();
        out.extend_from_slice(&[0; 12]); // end offset, prop count, prop bytes
        out.push(self.name.len() as u8);
        out.extend_from_slice(self.name.as_bytes());
        let props_start = out.len();
        for p in &self.props {
            p.encode(out);
        }
        let props_len = out.len() - props_start;
        if !self.children.is_empty() {
            let n = self.children.len();
            for (i, c) in self.children.iter().enumerate() {
                c.write(out, i + 1 == n);
            }
            out.extend_from_slice(&NULL_RECORD);
        } else if self.props.is_empty() && !is_last {
            out.extend_from_slice(&NULL_RECORD);
        }
        let end = out.len() as u32;
        out[start..start + 4].copy_from_slice(&end.to_le_bytes());
        out[start + 4..start + 8].copy_from_slice(&(self.props.len() as u32).to_le_bytes());
        out[start + 8..start + 12].copy_from_slice(&(props_len as u32).to_le_bytes());
    }
}

const FBX_VERSION: u32 = 7400;
const HEAD_MAGIC: &[u8; 23] = b"Kaydara FBX Binary  \x00\x1a\x00";
// fixed IDs that FBX readers accept (as written by Blender's exporter)
const FILE_ID: [u8; 16] = [0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1];
const TIME_ID: &str = "1970-01-01 10:00:00:000";
const FOOT_ID: [u8; 16] = [0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e];
const FOOT_MAGIC: [u8; 16] = [0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b];

/// Binary FBX 7.4: one model with its mesh (quads kept as quads, per-corner
/// normals, polygroups as smoothing-free polygon groups) and a material.
pub fn write_fbx<W: Write>(mesh: &Mesh, pl: &Placement, w: &mut W) -> Result<()> {
    let (polys, _) = Polys::from_mesh(mesh);
    let verts: Vec<f64> = mesh
        .positions
        .iter()
        .flat_map(|p| pl.pos(*p).to_array().map(|c| c as f64))
        .collect();
    let mut index: Vec<i32> = Vec::new();
    let mut normals: Vec<f64> = Vec::new();
    for f in &polys.faces {
        for (k, &v) in f.iter().enumerate() {
            index.push(if k + 1 == f.len() { -(v as i32) - 1 } else { v as i32 });
            let n = pl.nrm(mesh.normals[v as usize].normalize_or(Vec3::Y));
            normals.extend([n.x as f64, n.y as f64, n.z as f64]);
        }
    }
    let groups: Vec<i32> = polys.groups.iter().map(|&g| g as i32).collect();
    const GEOM: i64 = 1_000_001;
    const MODEL: i64 = 1_000_002;
    const MAT: i64 = 1_000_003;
    let int_p = |name: &str, v: i32| vec![s(name), s("int"), s("Integer"), s(""), Prop::I32(v)];
    let dbl_p = |name: &str, v: f64| vec![s(name), s("double"), s("Number"), s(""), Prop::F64(v)];
    let root = vec![
        node(
            "FBXHeaderExtension",
            vec![],
            vec![
                node("FBXHeaderVersion", vec![Prop::I32(1003)], vec![]),
                node("FBXVersion", vec![Prop::I32(FBX_VERSION as i32)], vec![]),
                node("EncryptionType", vec![Prop::I32(0)], vec![]),
                node(
                    "CreationTimeStamp",
                    vec![],
                    vec![
                        node("Version", vec![Prop::I32(1000)], vec![]),
                        node("Year", vec![Prop::I32(1970)], vec![]),
                        node("Month", vec![Prop::I32(1)], vec![]),
                        node("Day", vec![Prop::I32(1)], vec![]),
                        node("Hour", vec![Prop::I32(10)], vec![]),
                        node("Minute", vec![Prop::I32(0)], vec![]),
                        node("Second", vec![Prop::I32(0)], vec![]),
                        node("Millisecond", vec![Prop::I32(0)], vec![]),
                    ],
                ),
                node("Creator", vec![s("Clayform")], vec![]),
            ],
        ),
        node("FileId", vec![Prop::Raw(FILE_ID.to_vec())], vec![]),
        node("CreationTime", vec![s(TIME_ID)], vec![]),
        node("Creator", vec![s("Clayform")], vec![]),
        node(
            "GlobalSettings",
            vec![],
            vec![
                node("Version", vec![Prop::I32(1000)], vec![]),
                p70(vec![
                    int_p("UpAxis", if pl.z_up { 2 } else { 1 }),
                    int_p("UpAxisSign", 1),
                    int_p("FrontAxis", if pl.z_up { 1 } else { 2 }),
                    int_p("FrontAxisSign", if pl.z_up { -1 } else { 1 }),
                    int_p("CoordAxis", 0),
                    int_p("CoordAxisSign", 1),
                    int_p("OriginalUpAxis", -1),
                    int_p("OriginalUpAxisSign", 1),
                    dbl_p("UnitScaleFactor", 1.0),
                    dbl_p("OriginalUnitScaleFactor", 1.0),
                ]),
            ],
        ),
        node("Documents", vec![], vec![node("Count", vec![Prop::I32(0)], vec![])]),
        node("References", vec![], vec![]),
        node(
            "Definitions",
            vec![],
            vec![
                node("Version", vec![Prop::I32(100)], vec![]),
                node("Count", vec![Prop::I32(3)], vec![]),
                node("ObjectType", vec![s("Model")], vec![node("Count", vec![Prop::I32(1)], vec![])]),
                node("ObjectType", vec![s("Geometry")], vec![node("Count", vec![Prop::I32(1)], vec![])]),
                node("ObjectType", vec![s("Material")], vec![node("Count", vec![Prop::I32(1)], vec![])]),
            ],
        ),
        node(
            "Objects",
            vec![],
            vec![
                node(
                    "Geometry",
                    vec![Prop::I64(GEOM), obj_name("Sculpt", "Geometry"), s("Mesh")],
                    vec![
                        node("Properties70", vec![], vec![]),
                        node("GeometryVersion", vec![Prop::I32(124)], vec![]),
                        node("Vertices", vec![Prop::ArrF64(verts)], vec![]),
                        node("PolygonVertexIndex", vec![Prop::ArrI32(index)], vec![]),
                        node(
                            "LayerElementNormal",
                            vec![Prop::I32(0)],
                            vec![
                                node("Version", vec![Prop::I32(101)], vec![]),
                                node("Name", vec![s("")], vec![]),
                                node("MappingInformationType", vec![s("ByPolygonVertex")], vec![]),
                                node("ReferenceInformationType", vec![s("Direct")], vec![]),
                                node("Normals", vec![Prop::ArrF64(normals)], vec![]),
                            ],
                        ),
                        node(
                            "LayerElementPolygonGroup",
                            vec![Prop::I32(0)],
                            vec![
                                node("Version", vec![Prop::I32(101)], vec![]),
                                node("Name", vec![s("Polygroups")], vec![]),
                                node("MappingInformationType", vec![s("ByPolygon")], vec![]),
                                node("ReferenceInformationType", vec![s("Direct")], vec![]),
                                node("PolygonGroup", vec![Prop::ArrI32(groups)], vec![]),
                            ],
                        ),
                        node(
                            "LayerElementMaterial",
                            vec![Prop::I32(0)],
                            vec![
                                node("Version", vec![Prop::I32(101)], vec![]),
                                node("Name", vec![s("")], vec![]),
                                node("MappingInformationType", vec![s("AllSame")], vec![]),
                                node("ReferenceInformationType", vec![s("IndexToDirect")], vec![]),
                                node("Materials", vec![Prop::ArrI32(vec![0])], vec![]),
                            ],
                        ),
                        node(
                            "Layer",
                            vec![Prop::I32(0)],
                            vec![
                                node("Version", vec![Prop::I32(100)], vec![]),
                                node(
                                    "LayerElement",
                                    vec![],
                                    vec![
                                        node("Type", vec![s("LayerElementNormal")], vec![]),
                                        node("TypedIndex", vec![Prop::I32(0)], vec![]),
                                    ],
                                ),
                                node(
                                    "LayerElement",
                                    vec![],
                                    vec![
                                        node("Type", vec![s("LayerElementMaterial")], vec![]),
                                        node("TypedIndex", vec![Prop::I32(0)], vec![]),
                                    ],
                                ),
                            ],
                        ),
                    ],
                ),
                node(
                    "Model",
                    vec![Prop::I64(MODEL), obj_name("Sculpt", "Model"), s("Mesh")],
                    vec![
                        node("Version", vec![Prop::I32(232)], vec![]),
                        p70(vec![vec![
                            s("DefaultAttributeIndex"),
                            s("int"),
                            s("Integer"),
                            s(""),
                            Prop::I32(0),
                        ]]),
                        node("Shading", vec![Prop::Bool(true)], vec![]),
                        node("Culling", vec![s("CullingOff")], vec![]),
                    ],
                ),
                node(
                    "Material",
                    vec![Prop::I64(MAT), obj_name("Clay", "Material"), s("")],
                    vec![
                        node("Version", vec![Prop::I32(102)], vec![]),
                        node("ShadingModel", vec![s("phong")], vec![]),
                        node("MultiLayer", vec![Prop::I32(0)], vec![]),
                        p70(vec![vec![
                            s("DiffuseColor"),
                            s("Color"),
                            s(""),
                            s("A"),
                            Prop::F64(0.8),
                            Prop::F64(0.62),
                            Prop::F64(0.52),
                        ]]),
                    ],
                ),
            ],
        ),
        node(
            "Connections",
            vec![],
            vec![
                node("C", vec![s("OO"), Prop::I64(MODEL), Prop::I64(0)], vec![]),
                node("C", vec![s("OO"), Prop::I64(GEOM), Prop::I64(MODEL)], vec![]),
                node("C", vec![s("OO"), Prop::I64(MAT), Prop::I64(MODEL)], vec![]),
            ],
        ),
    ];
    let mut out = Vec::new();
    out.extend_from_slice(HEAD_MAGIC);
    out.extend_from_slice(&FBX_VERSION.to_le_bytes());
    let n = root.len();
    for (i, nd) in root.iter().enumerate() {
        nd.write(&mut out, i + 1 == n);
    }
    out.extend_from_slice(&NULL_RECORD);
    // footer
    out.extend_from_slice(&FOOT_ID);
    out.extend_from_slice(&[0; 4]);
    let pad = match (16 - out.len() % 16) % 16 {
        0 => 16,
        p => p,
    };
    out.extend(std::iter::repeat_n(0u8, pad));
    out.extend_from_slice(&FBX_VERSION.to_le_bytes());
    out.extend_from_slice(&[0; 120]);
    out.extend_from_slice(&FOOT_MAGIC);
    w.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{save, save_placed};

    /// A minimal binary FBX reader: (name, property type codes, children).
    fn read_node(b: &[u8], at: usize) -> Option<(String, Vec<u8>, usize, Vec<(String, Vec<u8>)>)> {
        let end = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        if end == 0 {
            return None;
        }
        let nprops = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
        let plen = u32::from_le_bytes(b[at + 8..at + 12].try_into().unwrap()) as usize;
        let nl = b[at + 12] as usize;
        let name = String::from_utf8(b[at + 13..at + 13 + nl].to_vec()).unwrap();
        let mut p = at + 13 + nl;
        let mut codes = Vec::new();
        for _ in 0..nprops {
            let c = b[p];
            codes.push(c);
            p += 1;
            p += match c {
                b'I' | b'F' => 4,
                b'L' | b'D' => 8,
                b'C' => 1,
                b'Y' => 2,
                b'S' | b'R' => 4 + u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize,
                _ => 12 + u32::from_le_bytes(b[p + 8..p + 12].try_into().unwrap()) as usize,
            };
        }
        assert_eq!(p, at + 13 + nl + plen, "{name}: property bytes");
        let mut kids = Vec::new();
        while p < end {
            match read_node(b, p) {
                Some((kn, kc, kend, _)) => {
                    kids.push((kn, kc));
                    p = kend;
                }
                None => p += 13,
            }
        }
        assert_eq!(p, end, "{name}: children end at the record end");
        Some((name, codes, end, kids))
    }

    #[test]
    fn fbx_structure_reads_back() {
        let q = Polys::qcube(2).to_mesh();
        let b = save(&q, "fbx");
        assert_eq!(&b[..23], HEAD_MAGIC);
        let mut p = 27;
        let mut names = Vec::new();
        while let Some((n, _, end, kids)) = read_node(&b, p) {
            if n == "Objects" {
                assert!(kids.iter().any(|k| k.0 == "Geometry"));
            }
            names.push(n);
            p = end;
        }
        for want in ["FBXHeaderExtension", "GlobalSettings", "Objects", "Connections"] {
            assert!(names.iter().any(|n| n == want), "{want} missing");
        }
        // 24 quads: 24 negative (polygon-closing) indices
        let pvi = b.windows(18).position(|w| w == b"PolygonVertexIndex").unwrap();
        let arr = pvi + 18;
        assert_eq!(b[arr], b'i');
        let len = u32::from_le_bytes(b[arr + 1..arr + 5].try_into().unwrap());
        assert_eq!(len, 96);
        assert_eq!(&b[b.len() - 16..], &FOOT_MAGIC);
        assert_eq!((b.len() - 140) % 16, 0, "footer version is 16-byte aligned");
    }

    #[test]
    fn glb_and_ply_headers() {
        let m = Mesh::icosphere(2);
        let glb = save(&m, "glb");
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + jlen]).unwrap();
        assert!(json.contains(r#""count":960"#), "{json}");
        let ply = save(&Polys::qcube(1).to_mesh(), "ply");
        let head = String::from_utf8_lossy(&ply[..400]);
        assert!(head.contains("element vertex 8") && head.contains("element face 6"));
        let body = ply.len() - (head.find("end_header\n").unwrap() + 11);
        assert_eq!(body, 8 * 27 + 6 * (1 + 16));
    }

    #[test]
    fn placement_scales_and_turns() {
        let m = Mesh::icosphere(1);
        let pl = Placement { scale: 10.0, offset: Vec3::new(0.0, 0.0, 5.0), z_up: true };
        let stl = save_placed(&m, "stl", &pl);
        // first vertex of the first triangle
        let rd = |o: usize| f32::from_le_bytes(stl[o..o + 4].try_into().unwrap());
        let v = m.positions[m.tris[0][0] as usize] * 10.0 + Vec3::new(0.0, 0.0, 5.0);
        assert!((rd(96) - v.x).abs() < 1e-4 && (rd(100) + v.z).abs() < 1e-4 && (rd(104) - v.y).abs() < 1e-4);
    }
}

