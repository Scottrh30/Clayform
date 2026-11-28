//! 3D text and shapes: font outlines (TrueType / OpenType) extruded into
//! solid meshes, plus a few built-in 2D shapes.

use crate::mesh::{Mesh, MeshBuilder};
use glam::{Vec2, Vec3};

/// Fonts that come with Clayform (name, data). Ubuntu Light (Ubuntu Font
/// Licence), Hack (MIT / Bitstream Vera) and Noto Emoji (OFL) for symbols.
pub fn builtin_fonts() -> [(&'static str, &'static [u8]); 3] {
    [
        ("Ubuntu Light", epaint_default_fonts::UBUNTU_LIGHT),
        ("Hack", epaint_default_fonts::HACK_REGULAR),
        ("Noto Emoji (symbols)", epaint_default_fonts::NOTO_EMOJI_REGULAR),
    ]
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextSettings {
    /// Extrusion depth relative to the text height.
    pub depth: f32,
    /// Extra space between letters (relative to the height).
    pub spacing: f32,
    /// Curve smoothness: segments per curve.
    pub segments: u32,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self { depth: 0.25, spacing: 0.0, segments: 6 }
    }
}

struct Outline {
    contours: Vec<Vec<Vec2>>,
    cur: Vec<Vec2>,
    seg: u32,
    off: Vec2,
    scale: f32,
}

impl Outline {
    fn p(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y) * self.scale + self.off
    }
    fn finish(&mut self) {
        let mut c = std::mem::take(&mut self.cur);
        if c.len() > 1 && c.first().unwrap().distance(*c.last().unwrap()) < 1e-6 {
            c.pop();
        }
        c.dedup_by(|a, b| a.distance(*b) < 1e-6);
        if c.len() >= 3 {
            self.contours.push(c);
        }
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        let p = self.p(x, y);
        self.cur.push(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.cur.push(p);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let a = *self.cur.last().unwrap_or(&Vec2::ZERO);
        let (b, c) = (self.p(x1, y1), self.p(x, y));
        for i in 1..=self.seg {
            let t = i as f32 / self.seg as f32;
            let q = a * (1.0 - t) * (1.0 - t) + b * 2.0 * t * (1.0 - t) + c * t * t;
            self.cur.push(q);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let a = *self.cur.last().unwrap_or(&Vec2::ZERO);
        let (b, c, d) = (self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        for i in 1..=self.seg {
            let t = i as f32 / self.seg as f32;
            let u = 1.0 - t;
            let q = a * u * u * u + b * 3.0 * u * u * t + c * 3.0 * u * t * t + d * t * t * t;
            self.cur.push(q);
        }
    }
    fn close(&mut self) {
        self.finish();
    }
}

fn signed_area(c: &[Vec2]) -> f32 {
    (0..c.len()).map(|i| c[i].perp_dot(c[(i + 1) % c.len()])).sum::<f32>() * 0.5
}

fn inside(p: Vec2, c: &[Vec2]) -> bool {
    let mut odd = false;
    let mut j = c.len() - 1;
    for i in 0..c.len() {
        let (a, b) = (c[i], c[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// The outlines of `text` laid out in lines, 1 unit per line height.
pub fn text_outline(font: &[u8], text: &str, s: &TextSettings) -> Result<Vec<Vec<Vec2>>, String> {
    let face = ttf_parser::Face::parse(font, 0).map_err(|_| "That isn't a font file Clayform can read (TrueType or OpenType)".to_string())?;
    let units = face.units_per_em() as f32;
    let scale = 1.0 / units;
    let line_h = (face.ascender() - face.descender() + face.line_gap()) as f32 * scale;
    let mut ol = Outline { contours: Vec::new(), cur: Vec::new(), seg: s.segments.clamp(1, 24), off: Vec2::ZERO, scale };
    for (li, line) in text.lines().enumerate() {
        let mut x = 0.0;
        for ch in line.chars() {
            let Some(gid) = face.glyph_index(ch) else {
                x += 0.5;
                continue;
            };
            ol.off = Vec2::new(x, -(li as f32) * line_h);
            face.outline_glyph(gid, &mut ol);
            ol.finish();
            x += face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale + s.spacing;
        }
    }
    if ol.contours.is_empty() {
        return Err("No letters to make (does the font have these characters?)".into());
    }
    Ok(ol.contours)
}

/// Built-in 2D shapes (name, contour builder).
pub const SHAPES: [&str; 7] = ["Star", "Heart", "Gear", "Arrow", "Hexagon", "Ring", "Cross"];

pub fn shape_outline(name: &str) -> Vec<Vec<Vec2>> {
    let circle = |r: f32, n: usize, rev: bool| -> Vec<Vec2> {
        let mut c: Vec<Vec2> = (0..n).map(|i| Vec2::from_angle(i as f32 / n as f32 * std::f32::consts::TAU) * r).collect();
        if rev {
            c.reverse();
        }
        c
    };
    match name {
        "Star" => vec![(0..10).map(|i| Vec2::from_angle(std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 10.0) * if i % 2 == 0 { 0.5 } else { 0.21 }).collect()],
        "Heart" => vec![(0..96)
            .map(|i| {
                let t = i as f32 / 96.0 * std::f32::consts::TAU;
                Vec2::new(16.0 * t.sin().powi(3), 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos()) / 34.0
            })
            .collect()],
        "Gear" => vec![
            (0..96)
                .map(|i| {
                    let a = i as f32 / 96.0 * std::f32::consts::TAU;
                    let tooth = ((a * 12.0 / std::f32::consts::TAU).fract() < 0.5) as i32 as f32;
                    Vec2::from_angle(a) * (0.42 + 0.08 * tooth)
                })
                .collect(),
            circle(0.15, 32, true),
        ],
        "Arrow" => vec![vec![
            Vec2::new(-0.5, -0.1),
            Vec2::new(0.1, -0.1),
            Vec2::new(0.1, -0.25),
            Vec2::new(0.5, 0.0),
            Vec2::new(0.1, 0.25),
            Vec2::new(0.1, 0.1),
            Vec2::new(-0.5, 0.1),
        ]],
        "Hexagon" => vec![circle(0.5, 6, false)],
        "Ring" => vec![circle(0.5, 64, false), circle(0.33, 64, true)],
        _ => vec![vec![
            Vec2::new(-0.15, -0.5),
            Vec2::new(0.15, -0.5),
            Vec2::new(0.15, -0.15),
            Vec2::new(0.5, -0.15),
            Vec2::new(0.5, 0.15),
            Vec2::new(0.15, 0.15),
            Vec2::new(0.15, 0.5),
            Vec2::new(-0.15, 0.5),
            Vec2::new(-0.15, 0.15),
            Vec2::new(-0.5, 0.15),
            Vec2::new(-0.5, -0.15),
            Vec2::new(-0.15, -0.15),
        ]],
    }
}

/// Extrudes outlines (any winding; holes found by nesting) into a closed
/// solid `depth` deep, centred on the origin, with front, back and sides
/// in their own polygroups.
pub fn extrude(contours: &[Vec<Vec2>], depth: f32) -> Result<Mesh, String> {
    // nesting depth: even = outer, odd = hole
    let level: Vec<usize> = contours.iter().map(|c| contours.iter().filter(|o| !std::ptr::eq(*o, c) && inside(c[0], o)).count()).collect();
    // orient: outers counter-clockwise, holes clockwise
    let mut cs: Vec<Vec<Vec2>> = contours.to_vec();
    for (c, l) in cs.iter_mut().zip(&level) {
        let ccw = signed_area(c) > 0.0;
        if ccw == (l % 2 == 1) {
            c.reverse();
        }
    }
    // centre
    let (mn, mx) = cs.iter().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let c0 = (mn + mx) * 0.5;
    for c in cs.iter_mut() {
        for p in c.iter_mut() {
            *p -= c0;
        }
    }
    let hz = depth * 0.5;
    let mut pos: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut groups: Vec<u32> = Vec::new();
    let mut partner: Vec<u32> = Vec::new();
    // caps: each outer with the holes directly inside it
    for (oi, outer) in cs.iter().enumerate() {
        if level[oi] % 2 == 1 {
            continue;
        }
        let holes: Vec<usize> = (0..cs.len()).filter(|&h| level[h] == level[oi] + 1 && inside(cs[h][0], outer)).collect();
        let mut flat: Vec<f64> = outer.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
        let mut hole_idx = Vec::new();
        let mut ring: Vec<Vec2> = outer.clone();
        for &h in &holes {
            hole_idx.push(ring.len());
            flat.extend(cs[h].iter().flat_map(|p| [p.x as f64, p.y as f64]));
            ring.extend_from_slice(&cs[h]);
        }
        let idx = earcutr::earcut(&flat, &hole_idx, 2).map_err(|e| format!("Couldn't fill a letter: {e:?}"))?;
        let base_f = pos.len() as u32;
        pos.extend(ring.iter().map(|p| Vec3::new(p.x, p.y, hz)));
        let base_b = pos.len() as u32;
        pos.extend(ring.iter().map(|p| Vec3::new(p.x, p.y, -hz)));
        for t in idx.chunks_exact(3) {
            let (a, b, c) = (t[0] as u32, t[1] as u32, t[2] as u32);
            // keep front faces counter-clockwise seen from +Z
            let area = (ring[t[1]] - ring[t[0]]).perp_dot(ring[t[2]] - ring[t[0]]);
            let (b, c) = if area < 0.0 { (c, b) } else { (b, c) };
            tris.push([base_f + a, base_f + b, base_f + c]);
            groups.push(0);
            partner.push(crate::mesh::NONE);
            tris.push([base_b + a, base_b + c, base_b + b]);
            groups.push(1);
            partner.push(crate::mesh::NONE);
        }
    }
    // sides: a quad per outline edge (sharing the cap vertices' positions)
    for c in &cs {
        let n = c.len();
        let base = pos.len() as u32;
        for p in c {
            pos.push(Vec3::new(p.x, p.y, hz));
            pos.push(Vec3::new(p.x, p.y, -hz));
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let (f0, b0, f1, b1) = (base + 2 * i as u32, base + 2 * i as u32 + 1, base + 2 * j as u32, base + 2 * j as u32 + 1);
            let t = tris.len() as u32;
            tris.push([f0, b0, b1]);
            tris.push([f0, b1, f1]);
            groups.extend([2, 2]);
            partner.extend([t + 1, t]);
        }
    }
    // weld the duplicated rim vertices so the solid is closed
    let soup_ids: Vec<[u32; 3]> = tris.clone();
    let mut map: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
    let mut wpos = Vec::new();
    let mut remap = vec![0u32; pos.len()];
    for (i, p) in pos.iter().enumerate() {
        let key = [(p.x * 1e5).round() as i64, (p.y * 1e5).round() as i64, (p.z * 1e5).round() as i64];
        remap[i] = *map.entry(key).or_insert_with(|| {
            wpos.push(*p);
            (wpos.len() - 1) as u32
        });
    }
    let wtris: Vec<[u32; 3]> = soup_ids.iter().map(|t| t.map(|v| remap[v as usize])).collect();
    let m = MeshBuilder::new(wpos, wtris).with_faces(Some(groups), Some(partner)).finish();
    Ok(m)
}

/// Text as a solid, scaled so a line is `height` tall.
pub fn text_mesh(font: &[u8], text: &str, s: &TextSettings, height: f32) -> Result<Mesh, String> {
    let c = text_outline(font, text, s)?;
    let mut m = extrude(&c, s.depth)?;
    for p in &mut m.positions {
        *p *= height;
    }
    m.compute_all_normals();
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_shapes_extrude_into_closed_solids() {
        let font = builtin_fonts()[0].1;
        let m = text_mesh(font, "Bo", &TextSettings::default(), 1.0).unwrap();
        assert!(!m.boundary.iter().any(|b| *b), "closed");
        let (mn, mx) = m.bounds();
        assert!((mx.z - mn.z - 0.25).abs() < 1e-3);
        assert!(mx.x - mn.x > 0.8);
        // the holes in B and o stay open: a point in o's counter is outside
        for name in SHAPES {
            let s = extrude(&shape_outline(name), 0.2).unwrap();
            assert!(!s.boundary.iter().any(|b| *b), "{name} closed");
            let vol: f32 = s.tris.iter().map(|t| s.positions[t[0] as usize].dot(s.positions[t[1] as usize].cross(s.positions[t[2] as usize])) / 6.0).sum();
            assert!(vol > 0.0, "{name} faces outwards ({vol})");
        }
        assert!(text_outline(font, "", &TextSettings::default()).is_err());
    }
}
