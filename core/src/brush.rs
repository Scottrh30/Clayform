//! Brush tip and stroke features: falloff curves, alphas (grayscale stamps),
//! chisel shapes, stroke types, lazy mouse and the random source for Spray.

use glam::Vec2;

// ------------------------------------------------------------------ falloff

/// Samples in a falloff lookup table (distance 0 = centre … 1 = edge).
pub const LUT_SIZE: usize = 65;

/// A falloff curve: control points (distance from the centre 0…1, strength
/// 0…1), joined by a smooth monotone spline.
#[derive(Clone, Debug, PartialEq)]
pub struct FalloffCurve {
    pub points: Vec<Vec2>,
}

/// The built-in curve shapes.
pub const CURVE_PRESETS: [(&str, &[(f32, f32)]); 7] = [
    ("Smooth", &[(0.0, 1.0), (0.35, 0.8), (0.7, 0.25), (1.0, 0.0)]),
    ("Linear", &[(0.0, 1.0), (1.0, 0.0)]),
    ("Sharp", &[(0.0, 1.0), (0.12, 0.45), (0.4, 0.1), (1.0, 0.0)]),
    ("Dome", &[(0.0, 1.0), (0.5, 0.88), (0.85, 0.45), (1.0, 0.0)]),
    ("Flat", &[(0.0, 1.0), (0.72, 1.0), (0.9, 0.5), (1.0, 0.0)]),
    ("Hard", &[(0.0, 1.0), (0.92, 1.0), (0.97, 0.3), (1.0, 0.0)]),
    ("Ring", &[(0.0, 0.0), (0.45, 0.25), (0.7, 1.0), (1.0, 0.0)]),
];

impl Default for FalloffCurve {
    fn default() -> Self {
        Self::preset(0)
    }
}

impl FalloffCurve {
    pub fn preset(i: usize) -> Self {
        let pts = CURVE_PRESETS[i.min(CURVE_PRESETS.len() - 1)].1;
        Self { points: pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect() }
    }

    /// Name of the preset this curve matches, if any.
    pub fn preset_name(&self) -> Option<&'static str> {
        CURVE_PRESETS.iter().find(|(_, p)| {
            p.len() == self.points.len() && p.iter().zip(&self.points).all(|(a, b)| (a.0 - b.x).abs() < 1e-4 && (a.1 - b.y).abs() < 1e-4)
        }).map(|(n, _)| *n)
    }

    /// The default curve leaves brushes exactly as they were before curves.
    pub fn is_default(&self) -> bool {
        self.preset_name() == Some("Smooth")
    }

    /// Keeps the points sorted, inside the box, with fixed ends at x = 0 and 1.
    pub fn tidy(&mut self) {
        for p in &mut self.points {
            *p = p.clamp(Vec2::ZERO, Vec2::ONE);
        }
        self.points.sort_by(|a, b| a.x.total_cmp(&b.x));
        if self.points.first().is_none_or(|p| p.x > 0.0) {
            let y = self.points.first().map_or(1.0, |p| p.y);
            self.points.insert(0, Vec2::new(0.0, y));
        }
        if self.points.last().is_none_or(|p| p.x < 1.0) {
            self.points.push(Vec2::new(1.0, 0.0));
        }
        // drop points stacked on the same x (keep the later one)
        let mut out: Vec<Vec2> = Vec::with_capacity(self.points.len());
        for p in self.points.drain(..) {
            if out.last().is_some_and(|q| (p.x - q.x).abs() < 1e-3) && out.len() > 1 {
                *out.last_mut().unwrap() = p;
            } else {
                out.push(p);
            }
        }
        self.points = out;
    }

    /// Strength at distance `x` (monotone cubic Hermite through the points).
    pub fn eval(&self, x: f32) -> f32 {
        let p = &self.points;
        match p.len() {
            0 => return 1.0 - x,
            1 => return p[0].y,
            _ => {}
        }
        let x = x.clamp(0.0, 1.0);
        let k = p.windows(2).position(|w| x <= w[1].x).unwrap_or(p.len() - 2);
        let (a, b) = (p[k], p[k + 1]);
        let h = (b.x - a.x).max(1e-6);
        let slope = |i: usize| -> f32 {
            // Fritsch–Carlson style tangents keep the curve from overshooting
            let d = |j: usize| (p[j + 1].y - p[j].y) / (p[j + 1].x - p[j].x).max(1e-6);
            if i == 0 {
                d(0)
            } else if i == p.len() - 1 {
                d(i - 1)
            } else {
                let (d0, d1) = (d(i - 1), d(i));
                if d0 * d1 <= 0.0 { 0.0 } else { 2.0 / (1.0 / d0 + 1.0 / d1) }
            }
        };
        let (m0, m1) = (slope(k) * h, slope(k + 1) * h);
        let t = (x - a.x) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * a.y + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * b.y + (t3 - t2) * m1;
        y.clamp(0.0, 1.0)
    }

    pub fn lut(&self) -> [f32; LUT_SIZE] {
        let mut out = [0.0; LUT_SIZE];
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.eval(i as f32 / (LUT_SIZE - 1) as f32);
        }
        out[LUT_SIZE - 1] = out[LUT_SIZE - 1].min(self.eval(1.0));
        out
    }

    /// "x,y;x,y;…" for saving in presets.
    pub fn to_text(&self) -> String {
        self.points.iter().map(|p| format!("{:.4},{:.4}", p.x, p.y)).collect::<Vec<_>>().join(";")
    }

    pub fn from_text(s: &str) -> Option<Self> {
        let mut points = Vec::new();
        for part in s.split(';').filter(|p| !p.trim().is_empty()) {
            let (a, b) = part.split_once(',')?;
            points.push(Vec2::new(a.trim().parse().ok()?, b.trim().parse().ok()?));
        }
        if points.len() < 2 {
            return None;
        }
        let mut c = Self { points };
        c.tidy();
        Some(c)
    }
}

/// Looks up a falloff table at distance `x` (0…1).
#[inline]
pub fn lut_at(lut: &[f32; LUT_SIZE], x: f32) -> f32 {
    if x >= 1.0 {
        return 0.0;
    }
    let f = x.max(0.0) * (LUT_SIZE - 1) as f32;
    let i = f as usize;
    let t = f - i as f32;
    lut[i] + (lut[(i + 1).min(LUT_SIZE - 1)] - lut[i]) * t
}

// ------------------------------------------------------------------ alphas

/// A grayscale image shaping the brush tip (0 = no effect, 1 = full).
#[derive(Clone, Debug, PartialEq)]
pub struct Alpha {
    pub name: String,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

/// Names of the built-in alphas.
pub const BUILTIN_ALPHAS: [&str; 10] =
    ["Pores", "Scales", "Weave", "Rings", "Square", "Noise", "Cracks", "Bumps", "Stitches", "Hexagons"];

impl Alpha {
    /// From 8-bit luminance (row-major, top row first). Images are fitted
    /// into a square, keeping their aspect, and normalised to the full range.
    pub fn from_luma(name: &str, w: usize, h: usize, luma: &[u8]) -> Option<Self> {
        if w == 0 || h == 0 || luma.len() < w * h {
            return None;
        }
        let mut data: Vec<f32> = luma[..w * h].iter().map(|&b| b as f32 / 255.0).collect();
        let (mn, mx) = data.iter().fold((1.0f32, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
        if mx - mn > 1e-3 {
            for v in &mut data {
                *v = (*v - mn) / (mx - mn);
            }
        }
        Some(Self { name: name.into(), w, h, data })
    }

    /// Bilinear sample; `u`, `v` in -1…1 across the brush (outside = 0).
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let s = self.w.max(self.h) as f32;
        // centre the image in a square of side s
        let x = (u * 0.5 + 0.5) * s - (s - self.w as f32) * 0.5 - 0.5;
        let y = (v * 0.5 + 0.5) * s - (s - self.h as f32) * 0.5 - 0.5;
        if x < -0.5 || y < -0.5 || x > self.w as f32 - 0.5 || y > self.h as f32 - 0.5 {
            return 0.0;
        }
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let g = |x: usize, y: usize| self.data[y * self.w + x];
        let a = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * tx;
        let b = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * tx;
        a + (b - a) * ty
    }

    /// A built-in, procedurally drawn alpha (see `BUILTIN_ALPHAS`).
    pub fn builtin(name: &str) -> Option<Self> {
        const N: usize = 128;
        let mut rng = Rng::new(0x5eed_u64.wrapping_add(name.len() as u64 * 977));
        let f: Box<dyn Fn(f32, f32) -> f32> = match name {
            "Pores" => {
                let dots: Vec<(Vec2, f32)> = (0..90).map(|_| (Vec2::new(rng.f() * 2.0 - 1.0, rng.f() * 2.0 - 1.0), 0.03 + 0.05 * rng.f())).collect();
                Box::new(move |x, y| {
                    let p = Vec2::new(x, y);
                    // inverted: pores are dents, the rest raised
                    let d = dots.iter().map(|(c, r)| smooth(1.0 - p.distance(*c) / r)).fold(0.0, f32::max);
                    1.0 - d
                })
            }
            "Scales" => Box::new(|x, y| {
                // overlapping rows of round scales
                let (sx, sy) = (x * 3.0, y * 3.5);
                let row = sy.floor();
                let off = if row as i32 % 2 == 0 { 0.0 } else { 0.5 };
                let cx = (sx + off).fract() - 0.5;
                let cy = sy.fract();
                let r = (cx * cx * 1.6 + (cy - 0.1) * (cy - 0.1)).sqrt();
                smooth(1.0 - r * 1.4) * (0.4 + 0.6 * cy)
            }),
            "Weave" => Box::new(|x, y| {
                let (u, v) = (x * 4.0, y * 4.0);
                let (cu, cv) = (u.floor() as i32, v.floor() as i32);
                let horiz = (cu + cv) % 2 == 0;
                let t = if horiz { v.fract() } else { u.fract() };
                let along = if horiz { u.fract() } else { v.fract() };
                let thread = smooth(1.0 - (t - 0.5).abs() * 2.2);
                thread * (0.55 + 0.45 * (along * std::f32::consts::PI).sin())
            }),
            "Rings" => Box::new(|x, y| {
                let r = (x * x + y * y).sqrt();
                if r > 1.0 { 0.0 } else { 0.5 + 0.5 * (r * 18.0).cos() * (1.0 - r) }
            }),
            "Square" => Box::new(|x, y| {
                let m = x.abs().max(y.abs());
                smooth((0.85 - m) * 8.0)
            }),
            "Noise" => {
                let n = ValueNoise::new(&mut rng);
                Box::new(move |x, y| n.fbm(x * 3.0 + 7.0, y * 3.0 + 3.0))
            }
            "Cracks" => {
                let pts: Vec<Vec2> = (0..22).map(|_| Vec2::new(rng.f() * 2.2 - 1.1, rng.f() * 2.2 - 1.1)).collect();
                Box::new(move |x, y| {
                    // distance to the border between the two nearest cells
                    let p = Vec2::new(x, y);
                    let (mut d1, mut d2) = (f32::MAX, f32::MAX);
                    for c in &pts {
                        let d = p.distance(*c);
                        if d < d1 {
                            d2 = d1;
                            d1 = d;
                        } else if d < d2 {
                            d2 = d;
                        }
                    }
                    let edge = d2 - d1;
                    1.0 - smooth(1.0 - edge * 18.0)
                })
            }
            "Bumps" => {
                let dots: Vec<(Vec2, f32)> = (0..26).map(|_| (Vec2::new(rng.f() * 1.8 - 0.9, rng.f() * 1.8 - 0.9), 0.08 + 0.12 * rng.f())).collect();
                Box::new(move |x, y| {
                    let p = Vec2::new(x, y);
                    dots.iter().map(|(c, r)| smooth(1.0 - p.distance(*c) / r)).fold(0.0, f32::max)
                })
            }
            "Stitches" => Box::new(|x, y| {
                // a row of diagonal stitches across the middle
                if y.abs() > 0.35 {
                    return 0.0;
                }
                let u = x * 4.0 + y * 1.5;
                let t = u.fract() - 0.5;
                smooth(1.0 - t.abs() * 6.0) * smooth((0.35 - y.abs()) * 10.0)
            }),
            "Hexagons" => Box::new(|x, y| {
                let s = 4.0;
                let (px, py) = (x * s, y * s);
                // hex grid distance to centre
                let q = Vec2::new(px, py);
                let a = Vec2::new(1.0, 1.732);
                let ha = a * 0.5;
                let m1 = q - a * (q / a).round();
                let m2 = (q - ha) - a * ((q - ha) / a).round();
                let g = if m1.length() < m2.length() { m1 } else { m2 };
                let g = g.abs();
                let d = (g.x * 0.866 + g.y * 0.5).max(g.y);
                smooth((0.43 - d) * 14.0)
            }),
            _ => return None,
        };
        let mut data = vec![0.0; N * N];
        for j in 0..N {
            for i in 0..N {
                let x = (i as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                let y = (j as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                data[j * N + i] = f(x, y).clamp(0.0, 1.0);
            }
        }
        Some(Self { name: name.into(), w: N, h: N, data })
    }

    /// 8-bit preview (row-major), for thumbnails and saving in presets.
    pub fn to_luma(&self) -> Vec<u8> {
        self.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
    }
}

#[inline]
fn smooth(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ------------------------------------------------------------------ chisel shapes

/// 3D shapes the Chisel brush presses into (or out of) the surface, as height
/// fields over the brush disc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChiselShape {
    Rivet,
    Button,
    Ring,
    Star,
    Cross,
    Gem,
    Screw,
    Ridge,
}

impl ChiselShape {
    pub const ALL: [ChiselShape; 8] = [
        ChiselShape::Rivet,
        ChiselShape::Button,
        ChiselShape::Ring,
        ChiselShape::Star,
        ChiselShape::Cross,
        ChiselShape::Gem,
        ChiselShape::Screw,
        ChiselShape::Ridge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ChiselShape::Rivet => "Rivet",
            ChiselShape::Button => "Button",
            ChiselShape::Ring => "Ring",
            ChiselShape::Star => "Star",
            ChiselShape::Cross => "Cross",
            ChiselShape::Gem => "Gem",
            ChiselShape::Screw => "Screw head",
            ChiselShape::Ridge => "Ridge",
        }
    }

    /// Height (0…1, as a fraction of the brush radius) at `u`, `v` (-1…1).
    pub fn height(self, u: f32, v: f32) -> f32 {
        let r = (u * u + v * v).sqrt();
        let a = v.atan2(u);
        match self {
            ChiselShape::Rivet => {
                // domed head
                if r >= 0.9 { 0.0 } else { 0.45 * (1.0 - (r / 0.9).powi(2)).sqrt() }
            }
            ChiselShape::Button => {
                // rim, dished middle and four holes
                if r >= 0.92 {
                    return 0.0;
                }
                let rim = 0.3 * smooth((0.92 - r) * 12.0);
                let dish = if r < 0.7 { -0.08 * (1.0 - (r / 0.7).powi(2)) } else { 0.0 };
                let mut h = rim + dish;
                for k in 0..4 {
                    let ang = std::f32::consts::FRAC_PI_4 + k as f32 * std::f32::consts::FRAC_PI_2;
                    let c = Vec2::new(ang.cos(), ang.sin()) * 0.25;
                    if Vec2::new(u, v).distance(c) < 0.1 {
                        h = 0.02;
                    }
                }
                h.max(0.0)
            }
            ChiselShape::Ring => {
                let t = (r - 0.65).abs() / 0.22;
                if t >= 1.0 { 0.0 } else { 0.3 * (1.0 - t * t).sqrt() }
            }
            ChiselShape::Star => {
                let spikes = 5.0;
                let k = ((a * spikes / 2.0).cos().abs()).powf(3.0);
                let edge = 0.4 + 0.55 * k;
                if r >= edge { 0.0 } else { 0.35 * (1.0 - r / edge) }
            }
            ChiselShape::Cross => {
                let arm = |s: f32, t: f32| if s.abs() < 0.85 && t.abs() < 0.22 { 0.3 * smooth((0.22 - t.abs()) * 12.0) * smooth((0.85 - s.abs()) * 12.0) } else { 0.0 };
                arm(u, v).max(arm(v, u))
            }
            ChiselShape::Gem => {
                // faceted octagon: flat table, sloping facets
                let sector = std::f32::consts::TAU / 8.0;
                let aa = (a.rem_euclid(sector) - sector * 0.5).cos();
                let d = r * aa / (sector * 0.5).cos();
                if d >= 0.9 { 0.0 } else if d < 0.45 { 0.45 } else { 0.45 * (0.9 - d) / 0.45 }
            }
            ChiselShape::Screw => {
                if r >= 0.88 {
                    return 0.0;
                }
                let head = 0.35 * (1.0 - (r / 0.88).powi(4)).sqrt();
                // slot across the head
                if u.abs() < 0.09 && r < 0.75 { head * 0.25 } else { head }
            }
            ChiselShape::Ridge => {
                if u.abs() >= 0.95 {
                    return 0.0;
                }
                let t = v.abs() / 0.3;
                if t >= 1.0 { 0.0 } else { 0.35 * (1.0 - t) * smooth((0.95 - u.abs()) * 6.0) }
            }
        }
    }
}

// ------------------------------------------------------------------ strokes

/// How the pointer path becomes dabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrokeKind {
    /// Evenly spaced dabs along the path (with lazy mouse).
    Freehand,
    /// One dab per pointer event: fast moves leave gaps.
    Dots,
    /// Press to place, drag out the size and turn, release to keep.
    DragRect,
    /// Scattered dabs with varied size, strength and turn.
    Spray,
}

impl StrokeKind {
    pub const ALL: [StrokeKind; 4] = [StrokeKind::Freehand, StrokeKind::Dots, StrokeKind::DragRect, StrokeKind::Spray];
    pub fn name(self) -> &'static str {
        match self {
            StrokeKind::Freehand => "Freehand",
            StrokeKind::Dots => "Dots",
            StrokeKind::DragRect => "Drag Rect",
            StrokeKind::Spray => "Spray",
        }
    }
}

/// Stroke and tip settings that live with the brush.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StrokeSettings {
    pub kind: StrokeKind,
    /// Dab spacing as a fraction of the brush radius (Freehand, Spray).
    pub spacing: f32,
    pub lazy: bool,
    /// Lazy mouse string length, in screen points.
    pub lazy_radius: f32,
    /// Spray: how far dabs scatter (fraction of the radius), and how much
    /// size, strength and turn vary (0…1).
    pub scatter: f32,
    pub jitter: f32,
    /// Pen pressure changes the size / the strength.
    pub pressure_size: bool,
    pub pressure_intensity: bool,
    /// Ignore surfaces facing away from the brush (thin parts).
    pub backface: bool,
    /// Alpha turn in degrees, and whether it follows the stroke direction.
    pub alpha_angle: f32,
    pub alpha_follow: bool,
    /// Chisel shape.
    pub chisel: ChiselShape,
}

impl Default for StrokeSettings {
    fn default() -> Self {
        Self {
            kind: StrokeKind::Freehand,
            spacing: 0.1,
            lazy: false,
            lazy_radius: 40.0,
            scatter: 0.6,
            jitter: 0.5,
            pressure_size: true,
            pressure_intensity: false,
            backface: true,
            alpha_angle: 0.0,
            alpha_follow: false,
            chisel: ChiselShape::Rivet,
        }
    }
}

/// Lazy mouse: the brush trails the pointer on a string, smoothing the line.
#[derive(Clone, Copy, Debug, Default)]
pub struct LazyMouse {
    pub pos: Vec2,
}

impl LazyMouse {
    pub fn new(p: Vec2) -> Self {
        Self { pos: p }
    }

    /// Pulls the brush towards `p`; returns how far it moved.
    pub fn pull(&mut self, p: Vec2, radius: f32) -> f32 {
        let d = p - self.pos;
        let l = d.length();
        if l <= radius {
            return 0.0;
        }
        let step = l - radius;
        self.pos += d / l * step;
        step
    }
}

// ------------------------------------------------------------------ random

/// Small deterministic random source (xorshift) for Spray and alphas.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in 0…1.
    pub fn f(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct ValueNoise {
    v: Vec<f32>,
}

impl ValueNoise {
    fn new(rng: &mut Rng) -> Self {
        Self { v: (0..256).map(|_| rng.f()).collect() }
    }
    fn at(&self, x: i32, y: i32) -> f32 {
        let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) as u32;
        self.v[(h % 256) as usize]
    }
    fn noise(&self, x: f32, y: f32) -> f32 {
        let (xi, yi) = (x.floor() as i32, y.floor() as i32);
        let (tx, ty) = (smooth(x - xi as f32), smooth(y - yi as f32));
        let a = self.at(xi, yi) + (self.at(xi + 1, yi) - self.at(xi, yi)) * tx;
        let b = self.at(xi, yi + 1) + (self.at(xi + 1, yi + 1) - self.at(xi, yi + 1)) * tx;
        a + (b - a) * ty
    }
    fn fbm(&self, x: f32, y: f32) -> f32 {
        let mut s = 0.0;
        let mut amp = 0.5;
        let mut f = 1.0;
        for _ in 0..5 {
            s += amp * self.noise(x * f, y * f);
            amp *= 0.5;
            f *= 2.0;
        }
        s / 0.97
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_pass_through_points_and_round_trip() {
        for i in 0..CURVE_PRESETS.len() {
            let c = FalloffCurve::preset(i);
            for p in &c.points {
                assert!((c.eval(p.x) - p.y).abs() < 1e-3, "{} at {}", CURVE_PRESETS[i].0, p.x);
            }
            let t = FalloffCurve::from_text(&c.to_text()).unwrap();
            assert_eq!(t.preset_name(), Some(CURVE_PRESETS[i].0));
        }
        let lut = FalloffCurve::preset(1).lut();
        assert!((lut_at(&lut, 0.5) - 0.5).abs() < 1e-3);
        assert_eq!(lut_at(&lut, 1.0), 0.0);
        let mut c = FalloffCurve { points: vec![Vec2::new(0.5, 2.0), Vec2::new(0.2, 0.3)] };
        c.tidy();
        assert_eq!(c.points.first().unwrap().x, 0.0);
        assert_eq!(c.points.last().unwrap().x, 1.0);
        assert!(c.points.iter().all(|p| p.y <= 1.0));
    }

    #[test]
    fn alphas_are_drawn_and_sampled() {
        for n in BUILTIN_ALPHAS {
            let a = Alpha::builtin(n).unwrap();
            let (mn, mx) = a.data.iter().fold((1.0f32, 0.0f32), |(a, b), &v| (a.min(v), b.max(v)));
            assert!(mx - mn > 0.3, "{n} is too flat");
        }
        let a = Alpha::from_luma("x", 2, 1, &[0, 255]).unwrap();
        assert!(a.sample(-0.9, 0.0) < 0.1 && a.sample(0.9, 0.0) > 0.9);
        assert_eq!(a.sample(0.0, 0.9), 0.0, "outside a wide image is empty");
    }

    #[test]
    fn lazy_mouse_trails_on_a_string() {
        let mut l = LazyMouse::new(Vec2::ZERO);
        assert_eq!(l.pull(Vec2::new(10.0, 0.0), 20.0), 0.0);
        let moved = l.pull(Vec2::new(50.0, 0.0), 20.0);
        assert!((moved - 30.0).abs() < 1e-4 && (l.pos.x - 30.0).abs() < 1e-4);
    }

    #[test]
    fn chisel_shapes_rise_inside_and_vanish_outside() {
        for s in ChiselShape::ALL {
            assert_eq!(s.height(0.99, 0.99), 0.0, "{}", s.name());
            let peak = (0..21).flat_map(|i| (0..21).map(move |j| (i, j))).map(|(i, j)| s.height(i as f32 / 10.0 - 1.0, j as f32 / 10.0 - 1.0)).fold(0.0, f32::max);
            assert!(peak > 0.2, "{}", s.name());
        }
    }
}
