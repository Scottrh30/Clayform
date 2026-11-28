//! Sculpting engine: brushes, strokes, symmetry helpers and undo/redo.

use crate::brush::{lut_at, Alpha, ChiselShape, LUT_SIZE};
use crate::bvh::{Bvh, Hit};
use crate::dyntopo::remesh_region;
use crate::mesh::{Marker, Mesh};
use glam::{Mat3, Quat, Vec2, Vec3};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Draw,
    Clay,
    ClayBuildup,
    Smooth,
    Flatten,
    TrimDynamic,
    Scrape,
    Inflate,
    Pinch,
    Crease,
    DamStandard,
    Layer,
    Grab,
    Snakehook,
    Nudge,
    Mask,
    ClayTubes,
    Blob,
    Magnify,
    Planar,
    TrimFront,
    Polish,
    HPolish,
    MPolish,
    SmoothValleys,
    SmoothPeaks,
    SmoothStronger,
    Move,
    MoveElastic,
    MoveInfiniteDepth,
    Rake,
    Slash,
    CurveSnakehook,
    Anchors,
    ClothPull,
    ClothTwist,
    ClothInflate,
    GroupPaint,
    Chisel,
    CurveTube,
    CurveStrap,
    Morph,
    Paint,
}

/// What the Anchors brush does while dragging.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnchorMode {
    Move,
    Rotate,
    Scale,
    Inflate,
    Twist,
    Bend,
}

impl AnchorMode {
    pub const ALL: [AnchorMode; 6] =
        [AnchorMode::Move, AnchorMode::Rotate, AnchorMode::Scale, AnchorMode::Inflate, AnchorMode::Twist, AnchorMode::Bend];
    pub fn name(self) -> &'static str {
        match self {
            AnchorMode::Move => "Move",
            AnchorMode::Rotate => "Rotate",
            AnchorMode::Scale => "Scale",
            AnchorMode::Inflate => "Inflate",
            AnchorMode::Twist => "Twist",
            AnchorMode::Bend => "Bend",
        }
    }
}

/// Groups for the brush menu.
pub const TOOL_CATEGORIES: [&str; 8] = ["Sculpt", "Smooth & polish", "Move", "Cloth", "Mask & groups", "Curves", "Stamps", "Paint"];

impl Tool {
    pub const ALL: [Tool; 43] = [
        Tool::Draw,
        Tool::Clay,
        Tool::ClayBuildup,
        Tool::Smooth,
        Tool::Flatten,
        Tool::TrimDynamic,
        Tool::Scrape,
        Tool::Inflate,
        Tool::Pinch,
        Tool::Crease,
        Tool::DamStandard,
        Tool::Layer,
        Tool::Grab,
        Tool::Snakehook,
        Tool::Nudge,
        Tool::Mask,
        Tool::ClayTubes,
        Tool::Blob,
        Tool::Magnify,
        Tool::Planar,
        Tool::TrimFront,
        Tool::Polish,
        Tool::HPolish,
        Tool::MPolish,
        Tool::SmoothValleys,
        Tool::SmoothPeaks,
        Tool::SmoothStronger,
        Tool::Move,
        Tool::MoveElastic,
        Tool::MoveInfiniteDepth,
        Tool::Rake,
        Tool::Slash,
        Tool::CurveSnakehook,
        Tool::Anchors,
        Tool::ClothPull,
        Tool::ClothTwist,
        Tool::ClothInflate,
        Tool::GroupPaint,
        Tool::Chisel,
        Tool::CurveTube,
        Tool::CurveStrap,
        Tool::Morph,
        Tool::Paint,
    ];

    /// Index into `TOOL_CATEGORIES`.
    pub fn category(self) -> usize {
        use Tool::*;
        match self {
            Smooth | Polish | HPolish | MPolish | SmoothValleys | SmoothPeaks | SmoothStronger => 1,
            Grab | Move | MoveElastic | MoveInfiniteDepth | Snakehook | Nudge | Anchors => 2,
            ClothPull | ClothTwist | ClothInflate => 3,
            Mask | GroupPaint => 4,
            CurveSnakehook | CurveTube | CurveStrap => 5,
            Chisel => 6,
            Paint => 7,
            _ => 0,
        }
    }

    /// Brushes that drag a region picked when the stroke starts.
    pub fn is_drag(self) -> bool {
        matches!(
            self,
            Tool::Grab | Tool::Move | Tool::MoveElastic | Tool::MoveInfiniteDepth | Tool::Anchors | Tool::ClothPull | Tool::ClothTwist
        )
    }

    pub fn index(self) -> usize {
        Tool::ALL.iter().position(|t| *t == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Draw => "Draw",
            Tool::Clay => "Clay",
            Tool::ClayBuildup => "Clay Buildup",
            Tool::Smooth => "Smooth",
            Tool::Flatten => "Flatten",
            Tool::TrimDynamic => "Trim Dynamic",
            Tool::Scrape => "Scrape",
            Tool::Inflate => "Inflate",
            Tool::Pinch => "Pinch",
            Tool::Crease => "Crease",
            Tool::DamStandard => "Dam Standard",
            Tool::Layer => "Layer",
            Tool::Grab => "Move Topological",
            Tool::Snakehook => "Snakehook",
            Tool::Nudge => "Nudge",
            Tool::Mask => "Mask",
            Tool::ClayTubes => "Clay Tubes",
            Tool::Blob => "Blob",
            Tool::Magnify => "Magnify",
            Tool::Planar => "Planar",
            Tool::TrimFront => "Trim Front",
            Tool::Polish => "Polish",
            Tool::HPolish => "hPolish",
            Tool::MPolish => "mPolish",
            Tool::SmoothValleys => "Smooth Valleys",
            Tool::SmoothPeaks => "Smooth Peaks",
            Tool::SmoothStronger => "Smooth Stronger",
            Tool::Move => "Move",
            Tool::MoveElastic => "Move Elastic",
            Tool::MoveInfiniteDepth => "Move Infinite Depth",
            Tool::Rake => "Rake",
            Tool::Slash => "Slash",
            Tool::CurveSnakehook => "Curve Snakehook",
            Tool::Anchors => "Anchors",
            Tool::ClothPull => "Cloth Pull",
            Tool::ClothTwist => "Cloth Twist",
            Tool::ClothInflate => "Cloth Inflate",
            Tool::GroupPaint => "Polygroup Paint",
            Tool::Chisel => "Chisel",
            Tool::CurveTube => "Curve Tube",
            Tool::CurveStrap => "Curve Strap",
            Tool::Morph => "Morph",
            Tool::Paint => "Paint",
        }
    }

    /// Keyboard shortcut, if the brush has one.
    pub fn key(self) -> Option<char> {
        let c = self.shortcut();
        (c != ' ').then_some(c)
    }

    pub fn shortcut(self) -> char {
        match self {
            Tool::Draw => 'D',
            Tool::Clay => 'C',
            Tool::ClayBuildup => 'B',
            Tool::Smooth => 'S',
            Tool::Flatten => 'F',
            Tool::TrimDynamic => 'Y',
            Tool::Scrape => 'Q',
            Tool::Inflate => 'I',
            Tool::Pinch => 'P',
            Tool::Crease => 'E',
            Tool::DamStandard => 'V',
            Tool::Layer => 'L',
            Tool::Grab => 'G',
            Tool::Snakehook => 'K',
            Tool::Nudge => 'U',
            Tool::Mask => 'M',
            Tool::Move => 'J',
            Tool::ClayTubes => 'H',
            Tool::Polish => 'O',
            _ => ' ',
        }
    }

    /// One line describing what the brush does (shown as a tooltip).
    pub fn description(self) -> &'static str {
        match self {
            Tool::Draw => "Raises the surface along the brush's average normal",
            Tool::Clay => "Builds up material with a flattened top",
            Tool::ClayBuildup => "Fast, square-edged clay strips for blocking in forms",
            Tool::Smooth => "Relaxes the surface (also Shift with any brush)",
            Tool::Flatten => "Pulls the surface onto a plane",
            Tool::TrimDynamic => "Cuts peaks down to a plane, leaving crisp planar facets",
            Tool::Scrape => "Shaves high spots off; Ctrl fills low spots instead",
            Tool::Inflate => "Pushes each point out along its own normal",
            Tool::Pinch => "Pulls the surface towards the brush centre",
            Tool::Crease => "Cuts a soft groove and pinches its edges",
            Tool::DamStandard => "Cuts a sharp, deep crease line (wrinkles, seams)",
            Tool::Layer => "Raises the surface to a fixed height per stroke",
            Tool::Grab => "Drags the connected surface under the brush (Grab)",
            Tool::Snakehook => "Pulls out horns and tentacles (best with dynamic topology)",
            Tool::Nudge => "Smears the surface sideways along the stroke",
            Tool::Mask => "Paints a mask that protects the surface; Ctrl erases",
            Tool::ClayTubes => "Square-ended clay strokes that follow the brush direction",
            Tool::Blob => "Bulbous, rounded build-up along the surface normals",
            Tool::Magnify => "Spreads the surface away from the brush centre; Ctrl shrinks",
            Tool::Planar => "Flattens to the height where the stroke started, cutting and filling",
            Tool::TrimFront => "Cuts the surface flat, facing the camera",
            Tool::Polish => "Smooths and flattens together for clean, polished forms",
            Tool::HPolish => "Hard polish: crisp planes, cutting peaks and filling dips",
            Tool::MPolish => "Metal polish: flattens surfaces while keeping their edges sharp",
            Tool::SmoothValleys => "Smooths only the low spots, filling creases",
            Tool::SmoothPeaks => "Smooths only the high spots, softening ridges",
            Tool::SmoothStronger => "Several smoothing passes per dab for heavy cleanup",
            Tool::Move => "Drags everything inside the brush sphere, connected or not",
            Tool::MoveElastic => "Drags with a long, rubbery falloff that keeps the shape's volume",
            Tool::MoveInfiniteDepth => "Drags everything under the cursor, through the whole model",
            Tool::Rake => "Combs parallel grooves along the stroke",
            Tool::Slash => "Carves very sharp, narrow cuts",
            Tool::CurveSnakehook => "Draw a curve, release to pull a tapered horn along it",
            Tool::Anchors => "Drag to move, rotate, scale, inflate, twist or bend the region",
            Tool::ClothPull => "Drags the surface like fabric, folding instead of stretching",
            Tool::ClothTwist => "Twists the surface like fabric, gathering it into folds",
            Tool::ClothInflate => "Puffs the surface out like cloth, wrinkling at the edges",
            Tool::GroupPaint => "Paints a new polygroup onto the faces under the brush",
            Tool::Chisel => "Drag out a 3D shape (rivet, button, star…) pressed onto the surface; Ctrl presses it in",
            Tool::CurveTube => "Draw a curve over the model, release to add a round tube along it",
            Tool::CurveStrap => "Draw a curve over the model, release to add a flat strap lying on the surface",
            Tool::Morph => "Brushes the surface back towards the stored morph target",
            Tool::Paint => "Polypaint: paints colour onto the surface (or from a reference image with Spotlight)",
        }
    }

    pub fn default_intensity(self) -> f32 {
        match self {
            Tool::Smooth => 0.6,
            Tool::Grab | Tool::Snakehook => 1.0,
            Tool::Mask => 0.8,
            Tool::Flatten | Tool::TrimDynamic | Tool::Scrape => 0.5,
            Tool::Layer => 0.3,
            Tool::Nudge => 0.6,
            Tool::ClayBuildup => 0.5,
            Tool::DamStandard => 0.45,
            Tool::Move | Tool::MoveElastic | Tool::MoveInfiniteDepth | Tool::Anchors | Tool::ClothPull | Tool::ClothTwist => 1.0,
            Tool::CurveSnakehook | Tool::GroupPaint => 1.0,
            Tool::CurveTube | Tool::CurveStrap => 0.5,
            Tool::Paint => 0.8,
            Tool::Polish | Tool::HPolish | Tool::MPolish | Tool::SmoothValleys | Tool::SmoothPeaks | Tool::SmoothStronger => 0.6,
            Tool::Planar | Tool::TrimFront => 0.5,
            _ => 0.4,
        }
    }

    /// Whether Ctrl / the negative toggle has a meaning for this tool.
    pub fn has_negative(self) -> bool {
        use Tool::*;
        !self.is_drag()
            && !matches!(
                self,
                Smooth | Flatten | Snakehook | Nudge | TrimDynamic | Planar | TrimFront | Polish | HPolish | MPolish
                    | SmoothValleys | SmoothPeaks | SmoothStronger | CurveSnakehook | GroupPaint | CurveTube | CurveStrap | Morph
            )
    }

    /// Smoothing-type brushes (dynamic topology may coarsen with these).
    pub fn is_smooth(self) -> bool {
        self.category() == 1
    }

    /// Brushes that move geometry along the stroke instead of the normal.
    /// Brushes that draw a curve first and act when it's released.
    pub fn is_curve(self) -> bool {
        matches!(self, Tool::CurveSnakehook | Tool::CurveTube | Tool::CurveStrap)
    }

    pub fn uses_motion(self) -> bool {
        matches!(self, Tool::Snakehook | Tool::Nudge | Tool::ClayTubes | Tool::Rake | Tool::CurveSnakehook)
    }
}

pub struct DabParams {
    pub tool: Tool,
    pub center: Vec3,
    pub radius: f32,
    pub intensity: f32,
    pub negative: bool,
    pub seed_tri: u32,
    /// Dynamic topology: target edge length (world units), or None.
    pub detail: Option<f32>,
    /// How far (world units, in the view plane) the brush moved since the
    /// previous dab. Used by Snakehook and Nudge.
    pub motion: Vec3,
    /// Direction the camera looks (Trim Front).
    pub view: Vec3,
    /// Polygroup the Polygroup Paint brush paints.
    pub group: u32,
    /// Colour the Paint brush paints (linear 0..1 sRGB values).
    pub color: Vec3,
}

impl Default for DabParams {
    fn default() -> Self {
        Self {
            tool: Tool::Draw,
            center: Vec3::ZERO,
            radius: 0.1,
            intensity: 0.5,
            negative: false,
            seed_tri: 0,
            detail: None,
            motion: Vec3::ZERO,
            view: Vec3::NEG_Z,
            group: 0,
            color: Vec3::ONE,
        }
    }
}

/// How a dragging brush picks its region when the stroke starts.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gather {
    /// Connected surface inside the brush sphere.
    Topological,
    /// Everything inside the brush sphere.
    Spatial,
    /// Everything within three radii, with a long soft falloff.
    Elastic,
    /// Everything within the brush radius of the line through the centre
    /// along this direction (the view).
    Depth(Vec3),
    /// The whole object, less what's masked (gizmo and deformers).
    Whole,
}

/// What a dragging brush does with its region.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DragKind {
    Move,
    Anchor(AnchorMode),
    ClothPull,
    ClothTwist,
    /// Gizmo: `DragInput::affine` about nothing in particular.
    Affine,
    /// A deformer along an axis (0 = X, 1 = Y, 2 = Z) by `DragInput::amount`.
    Deform(crate::deform::Deformer, usize),
}

/// Pointer state for a dragging brush.
#[derive(Clone, Copy, Debug, Default)]
pub struct DragInput {
    /// World-space movement in the drag plane since the stroke started.
    pub delta: Vec3,
    /// Screen movement since the stroke started, in points (y down).
    pub screen: Vec2,
    /// Direction the camera looks.
    pub view: Vec3,
    /// World units per screen point at the stroke start.
    pub wpp: f32,
    /// Gizmo transform.
    pub affine: glam::Mat4,
    /// Deformer strength (-1..1, roughly).
    pub amount: f32,
}

/// One multiresolution level. The active level's mesh lives in
/// `Sculptor::mesh` (its slot here is `None`); every other level keeps its
/// topology here. Level k > 0 also keeps the Loop stencils that build it
/// from level k-1, and its detail: per-vertex offsets from that smooth
/// subdivision, in each vertex's local frame so they follow the surface.
#[derive(Clone)]
pub(crate) struct Level {
    pub(crate) mesh: Option<Mesh>,
    pub(crate) odd: Vec<[u32; 4]>,
    pub(crate) details: Vec<Vec3>,
}

/// Everything about the sculpt that a topology change can affect.
#[derive(Clone)]
pub(crate) struct State {
    pub(crate) mesh: Mesh,
    pub(crate) levels: Vec<Level>,
    pub(crate) level: usize,
    pub(crate) extra: crate::detail::Extra,
}

fn mesh_bytes(m: &Mesh) -> usize {
    m.positions.len() * 100 + m.tris.len() * 40
}

/// A state to restore; applying one returns its inverse.
pub(crate) enum Undo {
    Verts { idx: Vec<u32>, pos: Vec<Vec3>, mask: Vec<f32> },
    /// Sculpt layers / morph target and the active level's positions.
    Extra(Box<(crate::detail::Extra, Vec<Vec3>)>),
    /// Polypaint of these vertices.
    Colors { idx: Vec<u32>, col: Vec<[u8; 4]> },
    /// The whole polypaint (empty = unpainted).
    AllColors(Vec<[u8; 4]>),
    /// A mannequin's mesh and pose.
    Pose(Box<(Mesh, Vec<f32>)>),
    State(Box<State>),
    /// Go back to this multiresolution level (level changes are exactly
    /// reversible, so nothing needs copying).
    Level(usize),
    /// Remove the top level (undoes a subdivision), then go to `then_level`.
    DropTop { then_level: usize },
    /// Put a removed top level back (redoes a subdivision).
    AddTop { level: Box<Level>, then_level: usize },
    /// Polygroups of these faces.
    Groups { faces: Vec<u32>, groups: Vec<u32> },
    /// Which faces were hidden.
    Hidden(Vec<bool>),
}

impl Undo {
    fn bytes(&self) -> usize {
        match self {
            Undo::Verts { idx, .. } => idx.len() * 20,
            Undo::Level(_) | Undo::DropTop { .. } => 16,
            Undo::Groups { faces, .. } => faces.len() * 8,
            Undo::Hidden(h) => h.len(),
            Undo::AddTop { level, .. } => {
                level.mesh.as_ref().map_or(0, mesh_bytes) + level.odd.len() * 16 + level.details.len() * 12
            }
            Undo::Extra(b) => b.0.bytes() + b.1.len() * 12,
            Undo::Colors { idx, .. } => idx.len() * 8,
            Undo::AllColors(c) => c.len() * 4,
            Undo::Pose(b) => mesh_bytes(&b.0) + b.1.len() * 4,
            Undo::State(s) => {
                mesh_bytes(&s.mesh)
                    + s.levels
                        .iter()
                        .map(|l| l.mesh.as_ref().map_or(0, mesh_bytes) + l.odd.len() * 16 + l.details.len() * 12)
                        .sum::<usize>()
            }
        }
    }
}

/// What the renderer must re-upload: everything (`topology`), or the
/// vertices in dirty blocks of `BLOCK` vertices (`range` is their span).
#[derive(Default)]
pub struct GpuDirty {
    pub topology: bool,
    pub range: Option<(u32, u32)>,
    /// One bit per block of `GpuDirty::BLOCK` vertices.
    pub blocks: Vec<u64>,
}

impl GpuDirty {
    pub const BLOCK: u32 = 256;

    #[inline]
    pub fn add(&mut self, v: u32) {
        self.range = Some(match self.range {
            None => (v, v),
            Some((a, b)) => (a.min(v), b.max(v)),
        });
        let blk = (v / Self::BLOCK) as usize;
        let (w, bit) = (blk / 64, blk % 64);
        if w >= self.blocks.len() {
            self.blocks.resize(w + 1, 0);
        }
        self.blocks[w] |= 1 << bit;
    }

    pub fn all(&mut self, n: usize) {
        if n > 0 {
            self.range = Some((0, n as u32 - 1));
            let nb = (n as u32).div_ceil(Self::BLOCK) as usize;
            self.blocks = vec![u64::MAX; nb.div_ceil(64)];
        }
    }

    /// Runs of dirty vertices `(first, last)` clipped to `n`, clearing them.
    pub fn take_runs(&mut self, n: usize) -> Vec<(u32, u32)> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (w, word) in self.blocks.iter().enumerate() {
            let mut bits = *word;
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let blk = (w * 64 + b) as u32;
                let first = blk * Self::BLOCK;
                if first as usize >= n {
                    continue;
                }
                let last = ((blk + 1) * Self::BLOCK - 1).min(n as u32 - 1);
                match runs.last_mut() {
                    Some(r) if r.1 + 1 == first => r.1 = last,
                    _ => runs.push((first, last)),
                }
            }
        }
        self.blocks.iter_mut().for_each(|w| *w = 0);
        self.range = None;
        runs
    }

    /// Vertices covered by dirty blocks (and clears them); for measurements.
    pub fn take_blocks_vertices(&mut self) -> usize {
        self.take_runs(usize::MAX).iter().map(|r| (r.1 - r.0 + 1) as usize).sum()
    }
}

struct GrabSet {
    verts: Vec<(u32, f32)>,
    orig: Vec<Vec3>,
    /// Linear part of the symmetry copy this set belongs to (identity for
    /// the original); the drag delta is transformed by it.
    xf: Mat3,
    kind: DragKind,
    center: Vec3,
    normal: Vec3,
    /// Vertex normals at the start (Anchors inflate).
    nrm: Vec<Vec3>,
    /// Cloth: per-vertex inverse mass and edges (local indices, rest length).
    inv_mass: Vec<f32>,
    edges: Vec<(u32, u32, f32)>,
    /// Bounds of the gathered vertices (deformers).
    bounds: (Vec3, Vec3),
}

pub(crate) struct Stroke {
    mark: Marker,
    pub(crate) touched: Vec<u32>,
    pub(crate) old_pos: Vec<Vec3>,
    old_mask: Vec<f32>,
    grab: Vec<GrabSet>,
    /// Dynamic-topology strokes undo by restoring this whole snapshot.
    snapshot: Option<Box<State>>,
    modified: bool,
    /// Layer brush: each vertex's position when the stroke first reached it.
    layer_base: std::collections::HashMap<u32, Vec3>,
    /// Planar brush: the plane found by the first dab.
    plane: Option<(Vec3, Vec3)>,
    /// Name of the stroke's undo step.
    label: Option<String>,
    /// Polygroup paint: faces' groups before the stroke.
    group_old: Vec<(u32, u32)>,
    group_seen: std::collections::HashSet<u32>,
    /// Paint strokes record colours instead of positions.
    pub(crate) paint: bool,
    pub(crate) old_col: Vec<[u8; 4]>,
}

impl Stroke {
    #[inline]
    /// Paint strokes: records `v` once; true the first time.
    pub(crate) fn mark_paint(&mut self, v: u32) -> bool {
        self.modified = true;
        if self.mark.mark(v) {
            self.touched.push(v);
            true
        } else {
            false
        }
    }

    pub(crate) fn touch(&mut self, v: u32, mesh: &Mesh) {
        self.modified = true;
        if self.snapshot.is_some() {
            return;
        }
        if self.mark.mark(v) {
            self.touched.push(v);
            self.old_pos.push(mesh.positions[v as usize]);
            self.old_mask.push(mesh.mask[v as usize]);
        }
    }
}

const UNDO_BUDGET_BYTES: usize = 768 * 1024 * 1024;
pub const MAX_TRIANGLES: usize = 6_000_000;

/// How the brush tip weighs the points it reaches, beyond the distance
/// falloff: a falloff curve, an alpha image, and backface masking.
#[derive(Clone, Default)]
pub struct Tip {
    /// Falloff table; None = the standard smooth falloff.
    pub curve: Option<Box<[f32; LUT_SIZE]>>,
    pub alpha: Option<std::sync::Arc<Alpha>>,
    /// The tip's image axes in the surface plane (unit length), set per dab.
    pub frame: (Vec3, Vec3),
    /// Brush normal for backface masking: points facing away are left alone.
    pub backface: Option<Vec3>,
    pub chisel: Option<ChiselShape>,
    /// Paint colours from a reference image (Spotlight).
    pub spot: Option<crate::paint::SpotSampler>,
}

impl Tip {
    #[inline]
    fn fall(&self, x: f32) -> f32 {
        match &self.curve {
            Some(l) => lut_at(l, x),
            None => falloff(x),
        }
    }

    /// Alpha and backface factor for a point at `q` with normal `n`.
    #[inline]
    fn shape(&self, q: Vec3, n: Vec3, center: Vec3, inv_r: f32) -> f32 {
        let mut w = 1.0;
        if let Some(b) = self.backface {
            let d = n.dot(b);
            if d < -0.1 {
                return 0.0;
            }
            if d < 0.2 {
                w *= (d + 0.1) / 0.3;
            }
        }
        if let Some(a) = &self.alpha {
            let t = (q - center) * inv_r;
            w *= a.sample(t.dot(self.frame.0), t.dot(self.frame.1));
        }
        w
    }
}

pub struct Sculptor {
    pub mesh: Mesh,
    /// Brush tip shaping (set by the editor before each stroke / dab).
    pub tip: Tip,
    pub(crate) bvh: Bvh,
    bvh_dirty: bool,
    /// Faces moved since the last refit (empty with `bvh_dirty` = refit all).
    bvh_faces: Vec<u32>,
    pub(crate) undo: Vec<Undo>,
    redo: Vec<Undo>,
    /// What each undo / redo step did, for the history list.
    pub(crate) undo_labels: Vec<String>,
    redo_labels: Vec<String>,
    /// Label for the next undo step (set just before the change).
    next_label: Option<String>,
    vmark: Marker,
    vmark2: Marker,
    fmark: Marker,
    pub(crate) stroke: Option<Stroke>,
    pub(crate) region: Vec<(u32, f32)>,
    stack: Vec<u32>,
    scratch: Vec<Vec3>,
    pub(crate) levels: Vec<Level>,
    pub(crate) level: usize,
    /// Sculpt layers and the morph target.
    pub(crate) extra: crate::detail::Extra,
    /// The highest level as it was before the last topology change, for
    /// "Reproject higher levels" (mesh, how many levels it had above the base).
    pub(crate) lost_top: Option<(std::sync::Arc<Mesh>, usize)>,
    /// Revision after the last merged pose step (slider drags).
    pub(crate) pose_rev: Option<u64>,
    /// Level to return to while a split subdivision is in progress.
    pending_level: Option<usize>,
    /// Counts changes, so the interface can tell when there's unsaved work.
    revision: u64,
    pub dirty: GpuDirty,
}

#[inline]
fn falloff(x: f32) -> f32 {
    if x >= 1.0 {
        0.0
    } else {
        let x3 = x * x * x;
        3.0 * x3 * x - 4.0 * x3 + 1.0
    }
}

pub fn mirror_x(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

impl Sculptor {
    pub fn new(mesh: Mesh) -> Self {
        let bvh = Bvh::build(&mesh);
        Self {
            mesh,
            bvh,
            bvh_dirty: false,
            bvh_faces: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            undo_labels: Vec::new(),
            redo_labels: Vec::new(),
            next_label: None,
            vmark: Marker::new(),
            vmark2: Marker::new(),
            fmark: Marker::new(),
            stroke: None,
            region: Vec::new(),
            stack: Vec::new(),
            tip: Tip::default(),
            scratch: Vec::new(),
            levels: vec![Level { mesh: None, odd: Vec::new(), details: Vec::new() }],
            level: 0,
            extra: Default::default(),
            lost_top: None,
            pose_rev: None,
            pending_level: None,
            revision: 0,
            dirty: GpuDirty { topology: true, ..Default::default() },
        }
    }

    pub fn tri_count(&self) -> usize {
        self.mesh.live_tri_count()
    }

    /// Number of multiresolution levels (1 = no levels).
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Active level, 0 = lowest.
    pub fn level(&self) -> usize {
        self.level
    }

    /// Dynamic topology needs a single level (like ZBrush's Sculptris Pro).
    pub fn dyntopo_allowed(&self) -> bool {
        self.levels.len() == 1 && self.extra.layers.is_empty()
    }

    /// Triangle count of the highest level.
    pub fn top_tri_count(&self) -> usize {
        let top = self.levels.len() - 1;
        if top == self.level {
            self.mesh.tris.len()
        } else {
            self.levels[top].mesh.as_ref().map_or(0, |m| m.tris.len())
        }
    }

    pub(crate) fn snapshot(&self) -> Box<State> {
        Box::new(State { mesh: self.mesh.clone(), levels: self.levels.clone(), level: self.level, extra: self.extra.clone() })
    }

    fn restore(&mut self, s: Box<State>) -> Box<State> {
        let s = *s;
        let cur = Box::new(State {
            mesh: std::mem::replace(&mut self.mesh, s.mesh),
            levels: std::mem::replace(&mut self.levels, s.levels),
            level: std::mem::replace(&mut self.level, s.level),
            extra: std::mem::replace(&mut self.extra, s.extra),
        });
        self.topology_changed();
        cur
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() && self.stroke.is_none()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.stroke.is_none()
    }

    /// Changes every time the sculpt changes (strokes, undo, levels...).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn push_undo(&mut self, u: Undo) {
        self.revision += 1;
        let label = self.next_label.take().unwrap_or_else(|| {
            match &u {
                Undo::Verts { .. } => "Edit",
                Undo::State(_) => "Change mesh",
                Undo::Level(_) => "Change level",
                Undo::DropTop { .. } => "Subdivide",
                Undo::AddTop { .. } => "Remove level",
                Undo::Groups { .. } => "Polygroups",
                Undo::Hidden(_) => "Visibility",
                Undo::Extra(_) => "Layers",
                Undo::Colors { .. } | Undo::AllColors(_) => "Paint",
                Undo::Pose(_) => "Pose",
            }
            .to_string()
        });
        self.undo.push(u);
        self.undo_labels.push(label);
        self.redo.clear();
        self.redo_labels.clear();
        let mut total: usize = self.undo.iter().map(|u| u.bytes()).sum();
        while self.undo.len() > 1 && (total > UNDO_BUDGET_BYTES || self.undo.len() > 100) {
            total -= self.undo.remove(0).bytes();
            self.undo_labels.remove(0);
        }
    }

    /// Names the next undo step (e.g. "Voxel remesh").
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.next_label = Some(label.into());
    }

    /// Names the next undo step unless something already did.
    pub fn set_label_if_none(&mut self, label: impl Into<String>) {
        if self.next_label.is_none() {
            self.next_label = Some(label.into());
        }
    }

    /// Names the stroke in progress (its undo step is made when it ends).
    pub fn label_stroke(&mut self, label: impl Into<String>) {
        if let Some(st) = self.stroke.as_mut() {
            st.label = Some(label.into());
        }
    }

    /// Steps that can be undone (oldest first) and redone (next first).
    pub fn history(&self) -> (Vec<String>, Vec<String>) {
        (self.undo_labels.clone(), self.redo_labels.iter().rev().cloned().collect())
    }

    /// Replaces the mesh (new primitive, import, subdivision), undoable.
    pub fn set_mesh(&mut self, mesh: Mesh) {
        self.set_mesh_built(mesh, None);
    }

    /// Like `set_mesh`, optionally with a picking hierarchy built elsewhere
    /// (e.g. a slice at a time by the interface). The old sculpt moves into
    /// the undo history instead of being copied.
    pub fn set_mesh_built(&mut self, mut mesh: Mesh, bvh: Option<Bvh>) {
        self.end_stroke();
        self.stop_recording();
        self.carry_paint_to(&mut mesh);
        // keep the highest level's shape so its detail can be reprojected
        if self.levels.len() > 1 {
            let top = self.levels.len() - 1;
            self.goto_level(top);
            self.lost_top = Some((std::sync::Arc::new(self.mesh.clone()), top));
        }
        let old = State {
            mesh: std::mem::replace(&mut self.mesh, mesh),
            levels: std::mem::replace(
                &mut self.levels,
                vec![Level { mesh: None, odd: Vec::new(), details: Vec::new() }],
            ),
            level: std::mem::replace(&mut self.level, 0),
            extra: std::mem::take(&mut self.extra),
        };
        self.push_undo(Undo::State(Box::new(old)));
        self.install(bvh);
    }

    fn install(&mut self, bvh: Option<Bvh>) {
        match bvh {
            Some(b) => {
                self.bvh = b;
                self.bvh_dirty = false;
            self.bvh_faces.clear();
        self.bvh_faces.clear();
                self.dirty.topology = true;
                self.dirty.range = None;
            }
            None => self.topology_changed(),
        }
    }

    pub(crate) fn topology_changed(&mut self) {
        self.bvh = Bvh::build(&self.mesh);
        self.bvh_dirty = false;
        self.bvh_faces.clear();
        self.dirty.topology = true;
        self.dirty.range = None;
    }

    /// Adds a multiresolution level above the highest one (moving there).
    pub fn subdivide(&mut self) -> Result<(), String> {
        self.begin_subdivide()?;
        let (fine, odd) = self.mesh.subdivide_with_stencil(true);
        self.finish_subdivide(fine, odd, None)
    }

    /// First half of a subdivision that runs elsewhere (a background
    /// thread): records undo, moves to the top level and returns. The
    /// current `mesh` is then the one to subdivide; finish with
    /// `finish_subdivide` or roll back with `abort_subdivide`.
    pub fn begin_subdivide(&mut self) -> Result<(), String> {
        self.end_stroke();
        self.stop_recording();
        if self.top_tri_count() * 4 > MAX_TRIANGLES {
            return Err(format!("Subdividing would exceed {} M triangles", MAX_TRIANGLES / 1_000_000));
        }
        self.pending_level = Some(self.level);
        let moved = self.level + 1 < self.levels.len();
        while self.level + 1 < self.levels.len() {
            self.step_up();
        }
        if self.mesh.compact() || moved {
            self.topology_changed();
        }
        Ok(())
    }

    /// Installs a subdivided top level computed from the current mesh.
    pub fn finish_subdivide(&mut self, mut fine: Mesh, odd: Vec<[u32; 4]>, bvh: Option<Bvh>) -> Result<(), String> {
        if fine.positions.len() != self.mesh.positions.len() + odd.len() {
            self.abort_subdivide();
            return Err("The subdivided mesh doesn't match the current one".into());
        }
        // a mesh subdivided elsewhere (desktop) arrives without paint, UVs or creases
        crate::paint::carry_paint_up(&self.mesh, &mut fine, &odd);
        crate::topo::carry_up_extras(&self.mesh, &mut fine, &odd);
        let then_level = self.pending_level.take().unwrap_or(self.level);
        let details = vec![Vec3::ZERO; fine.positions.len()];
        let coarse = std::mem::replace(&mut self.mesh, fine);
        self.levels[self.level].mesh = Some(coarse);
        self.levels.push(Level { mesh: None, odd, details });
        self.level += 1;
        self.install(bvh);
        self.push_undo(Undo::DropTop { then_level });
        Ok(())
    }

    /// Undoes `begin_subdivide` (cancelled or failed): back to the level
    /// you were on, with no undo or redo entry.
    pub fn abort_subdivide(&mut self) {
        if let Some(l) = self.pending_level.take() {
            self.goto_level(l);
        }
    }

    /// Changes level without recording undo.
    pub(crate) fn goto_level(&mut self, target: usize) {
        let target = target.min(self.levels.len() - 1);
        if target == self.level {
            return;
        }
        // hidden faces stay hidden on every level: each face of a level
        // has four children (in order) on the next
        let hidden = std::mem::take(&mut self.mesh.hidden);
        let from = self.level;
        while self.level < target {
            self.step_up();
        }
        while self.level > target {
            self.step_down();
        }
        if !hidden.is_empty() {
            let f = 4usize.pow(from.abs_diff(target) as u32);
            let n = self.mesh.tris.len();
            self.mesh.hidden = if target > from && n == hidden.len() * f {
                (0..n).map(|i| hidden[i / f]).collect()
            } else if target < from && n * f == hidden.len() {
                (0..n).map(|i| hidden[i * f..(i + 1) * f].iter().all(|&h| h)).collect()
            } else {
                Vec::new()
            };
            if !self.mesh.hidden.contains(&true) {
                self.mesh.hidden.clear();
            }
        }
        self.topology_changed();
    }

    /// Every level for saving: (mesh, stencils from the level below, detail).
    pub fn project_levels(&self) -> (Vec<crate::project::LevelData<&Mesh>>, usize) {
        let levels = self
            .levels
            .iter()
            .enumerate()
            .map(|(i, l)| crate::project::LevelData {
                mesh: if i == self.level { &self.mesh } else { l.mesh.as_ref().expect("level mesh") },
                stencils: l.odd.clone(),
                details: l.details.clone(),
            })
            .collect();
        (levels, self.level)
    }

    /// A sculptor holding loaded levels, with no undo history.
    pub fn from_levels(levels: Vec<crate::project::LevelData<Mesh>>, active: usize, bvh: Option<Bvh>) -> Self {
        let mut s = Sculptor::new(Mesh::new(Vec::new(), Vec::new()));
        s.load_levels(levels, active, bvh);
        s.clear_history();
        s
    }

    /// Forgets every undo and redo step.
    pub fn clear_history(&mut self) {
        self.end_stroke();
        self.undo.clear();
        self.redo.clear();
        self.undo_labels.clear();
        self.redo_labels.clear();
    }

    /// Every level's mesh copied (for duplicating).
    pub fn clone_levels(&self) -> (Vec<crate::project::LevelData<Mesh>>, usize) {
        let (levels, active) = self.project_levels();
        (
            levels
                .into_iter()
                .map(|l| crate::project::LevelData { mesh: l.mesh.clone(), stencils: l.stencils, details: l.details })
                .collect(),
            active,
        )
    }

    /// Replaces the whole sculpt with loaded levels (the old sculpt moves
    /// into the undo history, so opening a project can be undone).
    pub fn load_levels(&mut self, mut levels: Vec<crate::project::LevelData<Mesh>>, active: usize, bvh: Option<Bvh>) {
        self.end_stroke();
        let active = active.min(levels.len() - 1);
        let mesh = std::mem::replace(&mut levels[active].mesh, Mesh::new(Vec::new(), Vec::new()));
        let new_levels: Vec<Level> = levels
            .into_iter()
            .enumerate()
            .map(|(i, l)| Level { mesh: (i != active).then_some(l.mesh), odd: l.stencils, details: l.details })
            .collect();
        let old = State {
            mesh: std::mem::replace(&mut self.mesh, mesh),
            levels: std::mem::replace(&mut self.levels, new_levels),
            level: std::mem::replace(&mut self.level, active),
            extra: std::mem::take(&mut self.extra),
        };
        self.push_undo(Undo::State(Box::new(old)));
        self.install(bvh);
    }

    /// Moves to another multiresolution level (undoable).
    pub fn set_level(&mut self, target: usize) {
        self.end_stroke();
        self.stop_recording();
        let target = target.min(self.levels.len() - 1);
        if target == self.level {
            return;
        }
        let from = self.level;
        self.goto_level(target);
        if self.next_label.is_none() {
            self.set_label(format!("Level {}", target.min(self.levels.len().saturating_sub(1)) + 1));
        }
        self.push_undo(Undo::Level(from));
    }

    /// Level k-1 -> k: smooth-subdivide the current shape and add back the
    /// stored detail in each vertex's local frame.
    fn step_up(&mut self) {
        let k = self.level + 1;
        let mut fine = self.levels[k].mesh.take().expect("level mesh");
        let s = self.mesh.loop_positions(&self.levels[k].odd);
        fine.positions = s;
        fine.compute_all_normals();
        // frames come from the smooth positions, so measure them all first
        let details = &self.levels[k].details;
        let offsets: Vec<Vec3> = (0..fine.positions.len())
            .map(|v| {
                let d = details[v];
                if d == Vec3::ZERO {
                    return Vec3::ZERO;
                }
                let (t, b, n) = fine.frame(v as u32);
                t * d.x + b * d.y + n * d.z
            })
            .collect();
        for (p, o) in fine.positions.iter_mut().zip(offsets) {
            *p += o;
        }
        let n = self.mesh.positions.len();
        fine.mask[..n].copy_from_slice(&self.mesh.mask);
        for (i, st) in self.levels[k].odd.iter().enumerate() {
            fine.mask[n + i] = (self.mesh.mask[st[0] as usize] + self.mesh.mask[st[1] as usize]) * 0.5;
        }
        fine.compute_all_normals();
        crate::paint::carry_paint_up(&self.mesh, &mut fine, &self.levels[k].odd);
        let coarse = std::mem::replace(&mut self.mesh, fine);
        self.levels[k - 1].mesh = Some(coarse);
        self.levels[k].mesh = None;
        self.level = k;
    }

    /// Level k -> k-1: the lower level takes the shared (even) vertices'
    /// current positions, and level k's detail is re-measured against the
    /// smooth subdivision of that, so going back up reproduces it exactly.
    fn step_down(&mut self) {
        let k = self.level;
        let mut coarse = self.levels[k - 1].mesh.take().expect("level mesh");
        let n = coarse.positions.len();
        coarse.positions.copy_from_slice(&self.mesh.positions[..n]);
        coarse.mask.copy_from_slice(&self.mesh.mask[..n]);
        if self.mesh.colors.len() == self.mesh.positions.len() {
            coarse.colors = self.mesh.colors[..n].to_vec();
        }
        coarse.compute_all_normals();
        let s = coarse.loop_positions(&self.levels[k].odd);
        let actual = std::mem::replace(&mut self.mesh.positions, s);
        self.mesh.compute_all_normals();
        let mut details = Vec::with_capacity(actual.len());
        for (v, p) in actual.iter().enumerate() {
            let off = *p - self.mesh.positions[v];
            let (t, b, nn) = self.mesh.frame(v as u32);
            details.push(Vec3::new(off.dot(t), off.dot(b), off.dot(nn)));
        }
        self.mesh.positions = actual;
        self.mesh.compute_all_normals();
        self.levels[k].details = details;
        let fine = std::mem::replace(&mut self.mesh, coarse);
        self.levels[k].mesh = Some(fine);
        self.levels[k - 1].mesh = None;
        self.level = k - 1;
    }

    /// Bakes the current level as the new lowest level (ZBrush "Del Lower").
    pub fn delete_lower(&mut self) {
        self.end_stroke();
        if self.level == 0 {
            return;
        }
        let snap = self.snapshot();
        self.set_label("Delete lower levels");
        self.push_undo(Undo::State(snap));
        self.stop_recording();
        let cut = self.level;
        self.extra.shift_levels(cut);
        self.levels.drain(0..self.level);
        self.levels[0].odd = Vec::new();
        self.levels[0].details = Vec::new();
        self.level = 0;
        self.topology_changed();
    }

    /// Removes every level above the current one (ZBrush "Del Higher").
    pub fn delete_higher(&mut self) {
        self.end_stroke();
        if self.level + 1 >= self.levels.len() {
            return;
        }
        let snap = self.snapshot();
        self.set_label("Delete higher levels");
        self.push_undo(Undo::State(snap));
        self.levels.truncate(self.level + 1);
        let keep = self.level;
        self.extra.drop_above(keep);
        self.topology_changed();
    }

    /// Refit the picking hierarchy if the surface moved. Call once per frame.
    pub fn refresh_bvh(&mut self) {
        // dynamic topology adds faces the hierarchy doesn't index yet
        let extra = self.bvh.unindexed(&self.mesh);
        if extra > 20_000.max(self.bvh.indexed() / 3) {
            self.bvh = Bvh::build(&self.mesh);
            self.bvh_dirty = false;
            self.bvh_faces.clear();
        } else if self.bvh_dirty {
            if self.bvh_faces.is_empty() {
                self.bvh.refit(&self.mesh);
            } else {
                self.bvh.refit_faces(&self.mesh, &self.bvh_faces);
            }
            self.bvh_dirty = false;
            self.bvh_faces.clear();
        }
    }

    /// The live face closest to `p`.
    pub fn nearest_face(&self, p: Vec3) -> Option<u32> {
        self.bvh.closest(&self.mesh, p, f32::MAX).map(|c| c.1)
    }

    /// Closest point on the surface within `reach`, with its face.
    pub fn closest_face(&self, p: Vec3, reach: f32) -> Option<(Vec3, u32)> {
        self.bvh.closest(&self.mesh, p, reach)
    }

    /// Closest point on the surface within `reach`.
    pub fn closest_point(&self, p: Vec3, reach: f32) -> Option<Vec3> {
        self.bvh.closest_point(&self.mesh, p, reach)
    }

    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.bvh.raycast(&self.mesh, origin, dir)
    }

    /// `dynamic` = the stroke may change topology (undo takes a snapshot).
    pub fn begin_stroke(&mut self, dynamic: bool) {
        self.end_stroke();
        let snapshot = (dynamic && self.dyntopo_allowed()).then(|| self.snapshot());
        let mut mark = Marker::new();
        mark.begin(self.mesh.vertex_count());
        self.stroke = Some(Stroke {
            mark,
            touched: Vec::new(),
            old_pos: Vec::new(),
            old_mask: Vec::new(),
            grab: Vec::new(),
            snapshot,
            modified: false,
            layer_base: std::collections::HashMap::new(),
            plane: None,
            label: None,
            group_old: Vec::new(),
            group_seen: std::collections::HashSet::new(),
            paint: false,
            old_col: Vec::new(),
        });
    }

    /// Records the current mannequin mesh and pose as an undo step (before
    /// the pose changes). With `merge`, repeated calls with the same label
    /// and nothing in between make one step (a slider drag).
    pub fn push_pose_undo(&mut self, label: &str, merge: bool) {
        self.end_stroke();
        if merge && self.undo_labels.last().map(|s| s.as_str()) == Some(label) && self.pose_rev == Some(self.revision) {
            return;
        }
        self.set_label(label);
        self.push_undo(Undo::Pose(Box::new((self.mesh.clone(), self.extra.pose.clone()))));
        self.pose_rev = Some(self.revision);
    }

    /// Name of the running stroke's undo step.
    pub(crate) fn stroke_label(&self) -> Option<&str> {
        self.stroke.as_ref().and_then(|s| s.label.as_deref())
    }

    pub(crate) fn add_revision(&mut self) {
        self.revision += 1;
    }

    /// Every vertex may have moved: refit picking, redraw everything.
    pub(crate) fn mark_all_moved(&mut self) {
        self.bvh_dirty = true;
        self.bvh_faces.clear();
        self.dirty.all(self.mesh.positions.len());
    }

    /// Per-sculpt data added by later features (see `export_extras`).
    pub(crate) fn export_more_extras(&self, out: &mut Vec<u8>) {
        self.export_creases(out);
        self.export_colors(out);
        self.export_uvs(out);
        self.export_hidden(out);
    }
    pub(crate) fn import_more_extras(&mut self, tag: &[u8; 4], body: &[u8]) {
        match tag {
            b"CRSE" => self.import_creases(body),
            b"COLR" => self.import_colors(body),
            b"UVS0" => self.import_uvs(body),
            b"HIDE" => self.import_hidden(body),
            _ => {}
        }
    }

    /// Puts every point the current stroke has moved back where it was
    /// (Drag Rect redraws its stamp this way while you drag).
    pub fn stroke_reset(&mut self) {
        let Some(st) = self.stroke.as_ref() else { return };
        if st.snapshot.is_some() || st.touched.is_empty() {
            return;
        }
        let verts = st.touched.clone();
        for (k, &v) in st.touched.iter().enumerate() {
            self.mesh.positions[v as usize] = st.old_pos[k];
            self.mesh.mask[v as usize] = st.old_mask[k];
        }
        self.after_deform(false, Some(&verts));
    }

    pub fn end_stroke(&mut self) {
        if let Some(st) = self.stroke.take() {
            // the stroke's own name, keeping any label meant for the next change
            let outer = self.next_label.take();
            self.next_label = st.label.clone();
            if let Some(snap) = st.snapshot {
                if st.modified {
                    if self.mesh.compact() {
                        self.topology_changed();
                    }
                    self.push_undo(Undo::State(snap));
                }
            } else if st.paint && !st.touched.is_empty() {
                self.push_undo(Undo::Colors { idx: st.touched, col: st.old_col });
            } else if !st.touched.is_empty() {
                self.push_undo(Undo::Verts { idx: st.touched, pos: st.old_pos, mask: st.old_mask });
            } else if !st.group_old.is_empty() {
                let (faces, groups) = st.group_old.into_iter().unzip();
                self.push_undo(Undo::Groups { faces, groups });
            }
            self.next_label = outer;
        }
    }

    /// Replaces the polygroups (undoable).
    pub fn set_groups(&mut self, groups: Vec<u32>) {
        self.end_stroke();
        if groups.len() != self.mesh.tris.len() {
            return;
        }
        let old = std::mem::replace(&mut self.mesh.groups, groups);
        let faces = (0..old.len() as u32).collect();
        self.push_undo(Undo::Groups { faces, groups: old });
        self.dirty.topology = true;
    }

    /// Replaces the mesh without adding an undo step (live previews; the
    /// change that started the preview is the undo step).
    pub fn replace_mesh_preview(&mut self, mut mesh: Mesh) {
        self.end_stroke();
        self.carry_paint_to(&mut mesh);
        self.mesh = mesh;
        self.revision += 1;
        self.topology_changed();
    }

    /// The mesh one undo step back, if that step replaced the whole mesh.
    pub fn mesh_before_last_change(&self) -> Option<&Mesh> {
        match self.undo.last() {
            Some(Undo::State(s)) => Some(&s.mesh),
            _ => None,
        }
    }

    fn laplacian(mesh: &Mesh, v: u32) -> Vec3 {
        let p0 = mesh.positions[v as usize];
        let mut sum = Vec3::ZERO;
        let mut c = 0.0;
        let boundary = mesh.boundary[v as usize];
        for &u in mesh.neighbors(v) {
            if boundary && !mesh.boundary[u as usize] {
                continue;
            }
            sum += mesh.positions[u as usize];
            c += 1.0;
        }
        if c > 0.0 { sum / c } else { p0 }
    }

    /// Edges among a set of vertices (local indices) with their lengths.
    fn local_edges(&self, verts: &[u32]) -> Vec<(u32, u32, f32)> {
        let local: std::collections::HashMap<u32, u32> =
            verts.iter().enumerate().map(|(i, &v)| (v, i as u32)).collect();
        let mut edges = Vec::new();
        for (i, &v) in verts.iter().enumerate() {
            for &u in self.mesh.neighbors(v) {
                if u > v {
                    if let Some(&j) = local.get(&u) {
                        let l = self.mesh.positions[v as usize].distance(self.mesh.positions[u as usize]);
                        edges.push((i as u32, j, l));
                    }
                }
            }
        }
        edges
    }

    fn relax_cloth(x: &mut [Vec3], inv: &[f32], edges: &[(u32, u32, f32)], iterations: usize, stiffness: f32) {
        for _ in 0..iterations {
            for &(a, b, rest) in edges {
                let (a, b) = (a as usize, b as usize);
                let (wa, wb) = (inv[a], inv[b]);
                let ws = wa + wb;
                if ws <= 0.0 {
                    continue;
                }
                let d = x[b] - x[a];
                let len = d.length();
                if len < 1e-12 {
                    continue;
                }
                let corr = d * ((len - rest) / len / ws * stiffness);
                x[a] += corr * wa;
                x[b] -= corr * wb;
            }
        }
    }

    /// Flood-fills outward from the hit triangle, collecting vertices inside
    /// the brush sphere together with their falloff weight.
    fn collect_region(&mut self, center: Vec3, radius: f32, seed_tri: u32, use_mask: bool) {
        let Sculptor { mesh, vmark, region, stack, tip, .. } = self;
        region.clear();
        stack.clear();
        if seed_tri as usize >= mesh.tris.len() || !mesh.is_alive(seed_tri) || radius <= 0.0 {
            return;
        }
        vmark.begin(mesh.vertex_count());
        let seed = mesh.tris[seed_tri as usize];
        for &v in &seed {
            if vmark.mark(v) {
                stack.push(v);
            }
        }
        let r2 = radius * radius;
        let inv_r = 1.0 / radius;
        let hides = mesh.any_hidden();
        while let Some(v) = stack.pop() {
            if hides && mesh.vert_hidden(v) {
                continue;
            }
            let d2 = mesh.positions[v as usize].distance_squared(center);
            let inside = d2 < r2;
            if inside {
                // a chisel keeps its own profile: no distance falloff
                let mut w = if tip.chisel.is_some() { 1.0 } else { tip.fall(d2.sqrt() * inv_r) };
                if use_mask {
                    w *= 1.0 - mesh.mask[v as usize];
                    // (masking and painting tools ignore the tip's alpha & backface)
                    w *= tip.shape(mesh.positions[v as usize], mesh.normals[v as usize], center, inv_r);
                }
                region.push((v, w));
            }
            if inside || seed.contains(&v) {
                for &u in mesh.neighbors(v) {
                    if vmark.mark(u) {
                        stack.push(u);
                    }
                }
            }
        }
    }

    pub(crate) fn after_deform(&mut self, verts_from_region: bool, extra: Option<&[u32]>) {
        let mut list: Vec<u32> = Vec::new();
        let verts: &[u32] = if verts_from_region {
            list.extend(self.region.iter().map(|r| r.0));
            &list
        } else {
            extra.unwrap_or(&[])
        };
        for &v in verts {
            self.dirty.add(v);
        }
        let Sculptor { mesh, fmark, vmark2, .. } = self;
        mesh.update_normals(verts, fmark, vmark2);
        // normals of the one-ring changed too
        for &f in verts.iter().flat_map(|&v| mesh.vert_faces(v)) {
            for &u in &mesh.tris[f as usize] {
                self.dirty.add(u);
            }
            self.bvh_faces.push(f);
        }
        self.bvh_dirty = true;
    }

    pub fn dab(&mut self, p: &DabParams) {
        if p.tool.is_drag() || matches!(p.tool, Tool::CurveTube | Tool::CurveStrap) {
            return;
        }
        let mut seed = p.seed_tri;
        if let Some(detail) = p.detail {
            let dynamic_stroke = self.stroke.as_ref().is_some_and(|s| s.snapshot.is_some());
            if p.tool != Tool::Mask && p.tool != Tool::GroupPaint && dynamic_stroke {
                seed = self.remesh(seed, p.center, p.radius, detail);
            }
        }
        self.collect_region(p.center, p.radius, seed, p.tool != Tool::Mask && p.tool != Tool::GroupPaint);
        if self.region.is_empty() {
            return;
        }
        if p.tool == Tool::GroupPaint {
            self.paint_group(p.group);
            return;
        }
        if p.tool == Tool::Paint {
            self.paint_dab(p);
            return;
        }
        if let Some(st) = self.stroke.as_mut() {
            for &(v, _) in &self.region {
                st.touch(v, &self.mesh);
            }
        }

        let s = if p.negative && p.tool.has_negative() { -1.0 } else { 1.0 };
        let r = p.radius;
        let i = p.intensity.clamp(0.0, 1.0);

        // weighted area normal & centre
        let mut an = Vec3::ZERO;
        let mut ac = Vec3::ZERO;
        let mut wsum = 0.0;
        for &(v, w) in &self.region {
            let w = w + 1e-4;
            an += self.mesh.normals[v as usize] * w;
            ac += self.mesh.positions[v as usize] * w;
            wsum += w;
        }
        let an = an.try_normalize().unwrap_or(Vec3::Y);
        let ac = ac / wsum;

        let mesh = &mut self.mesh;
        let region = &self.region;
        match p.tool {
            Tool::Draw => {
                let d = an * (s * 0.08 * r * i);
                for &(v, w) in region {
                    mesh.positions[v as usize] += d * w;
                }
            }
            Tool::Inflate => {
                let k = s * 0.06 * r * i;
                for &(v, w) in region {
                    let n = mesh.normals[v as usize];
                    mesh.positions[v as usize] += n * (k * w);
                }
            }
            Tool::Smooth => {
                self.scratch.clear();
                for &(v, w) in region {
                    let p0 = mesh.positions[v as usize];
                    let mut sum = Vec3::ZERO;
                    let mut c = 0.0;
                    let boundary = mesh.boundary[v as usize];
                    for &u in mesh.neighbors(v) {
                        if boundary && !mesh.boundary[u as usize] {
                            continue;
                        }
                        sum += mesh.positions[u as usize];
                        c += 1.0;
                    }
                    let target = if c > 0.0 { sum / c } else { p0 };
                    self.scratch.push(p0.lerp(target, (w * i).clamp(0.0, 1.0)));
                }
                for (k, &(v, _)) in region.iter().enumerate() {
                    mesh.positions[v as usize] = self.scratch[k];
                }
            }
            Tool::Flatten => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    mesh.positions[v as usize] = pv - an * (d * w * i * 0.5);
                }
            }
            Tool::Clay => {
                let plane = ac + an * (s * r * 0.1);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - plane).dot(an);
                    if d * s < 0.0 {
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.45);
                    }
                }
            }
            Tool::Pinch => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (s * w * i * 0.3);
                }
            }
            Tool::Crease => {
                let push = an * (-s * 0.05 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (w * i * 0.25) + push * w;
                }
            }
            Tool::DamStandard => {
                // deep, narrow groove: cubic falloff for the cut, strong pinch
                let push = an * (-s * 0.07 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (w * i * 0.4) + push * (w * w * w);
                }
            }
            Tool::ClayBuildup => {
                let plane = ac + an * (s * r * 0.15);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - plane).dot(an);
                    if d * s < 0.0 {
                        // square-edged strips: a harder falloff than Clay
                        let w = w.sqrt();
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.6);
                    }
                }
            }
            Tool::TrimDynamic => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    if d > 0.0 {
                        let w = (w * 2.0).min(1.0);
                        mesh.positions[v as usize] = pv - an * (d * w * i);
                    }
                }
            }
            Tool::Scrape => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    if d * s > 0.0 {
                        mesh.positions[v as usize] = pv - an * (d * w * i * 0.5);
                    }
                }
            }
            Tool::Layer => {
                // raise towards a fixed height above where the stroke found
                // each point, so overlapping dabs don't pile up
                let height = s * r * 0.12 * i;
                let bases = self.stroke.as_mut().map(|st| &mut st.layer_base);
                let mut scratch = std::collections::HashMap::new();
                let bases = bases.unwrap_or(&mut scratch);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let base = *bases.entry(v).or_insert(pv);
                    let current = (pv - base).dot(an);
                    let target = height * w.min(1.0);
                    let step = target - current;
                    if step * s > 0.0 {
                        mesh.positions[v as usize] = pv + an * (step * 0.5);
                    }
                }
            }
            Tool::Snakehook => {
                let m = p.motion * i;
                for &(v, w) in region {
                    mesh.positions[v as usize] += m * w;
                }
            }
            Tool::Nudge => {
                let mut m = p.motion;
                m -= an * m.dot(an);
                let m = m * (i * 0.7);
                for &(v, w) in region {
                    mesh.positions[v as usize] += m * w;
                }
            }
            Tool::Mask => {
                for &(v, w) in region {
                    let m = &mut mesh.mask[v as usize];
                    *m = (*m + s * i * w * 0.5).clamp(0.0, 1.0);
                }
                for &(v, _) in region {
                    self.dirty.add(v);
                }
                return;
            }
            Tool::ClayTubes | Tool::Rake => {
                let mut dir = p.motion - an * p.motion.dot(an);
                if dir.length_squared() < 1e-12 {
                    dir = an.any_orthonormal_vector();
                }
                let dir = dir.normalize();
                let side = an.cross(dir);
                if p.tool == Tool::ClayTubes {
                    let plane = ac + an * (s * r * 0.12);
                    for &(v, w) in region {
                        if w <= 0.0 {
                            continue;
                        }
                        let pv = mesh.positions[v as usize];
                        let t = pv - p.center;
                        let (a, b) = ((t.dot(dir) / r).abs() * 1.3, (t.dot(side) / r).abs());
                        let sq = (1.0 - a.max(b).powi(4)).clamp(0.0, 1.0) * (1.0 - mesh.mask[v as usize]);
                        let d = (pv - plane).dot(an);
                        if d * s < 0.0 {
                            mesh.positions[v as usize] = pv - an * (d * sq * i * 0.5);
                        }
                    }
                } else {
                    for &(v, w) in region {
                        let pv = mesh.positions[v as usize];
                        let c = (pv - p.center).dot(side) / r;
                        let groove = 0.5 - 0.5 * (c * std::f32::consts::PI * 6.0).cos();
                        mesh.positions[v as usize] = pv - an * (s * 0.03 * r * i * w * groove);
                    }
                }
            }
            Tool::Blob => {
                let k = s * 0.05 * r * i;
                for &(v, w) in region {
                    let n = mesh.normals[v as usize];
                    mesh.positions[v as usize] += n * (k * w.sqrt());
                }
            }
            Tool::Magnify => {
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = pv - p.center;
                    t -= an * t.dot(an);
                    mesh.positions[v as usize] = pv + t * (s * w * i * 0.2) + an * (s * 0.01 * r * w);
                }
            }
            Tool::Planar | Tool::TrimFront => {
                let (pc, pn) = if p.tool == Tool::Planar {
                    match self.stroke.as_mut() {
                        Some(st) => *st.plane.get_or_insert((ac, an)),
                        None => (ac, an),
                    }
                } else {
                    (ac, -p.view.normalize_or_zero())
                };
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - pc).dot(pn);
                    if p.tool == Tool::Planar || d > 0.0 {
                        let w = (w * 2.0).min(1.0);
                        mesh.positions[v as usize] = pv - pn * (d * w * i * 0.6);
                    }
                }
            }
            Tool::Polish | Tool::HPolish | Tool::MPolish => {
                self.scratch.clear();
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let d = (pv - ac).dot(an);
                    let target = Self::laplacian(mesh, v);
                    let q = match p.tool {
                        Tool::Polish => pv.lerp(target, (w * i * 0.4).min(1.0)) - an * (d * w * i * 0.25),
                        Tool::HPolish => {
                            let w = (w * 2.0).min(1.0);
                            pv - an * (d * w * i * 0.5)
                        }
                        _ => {
                            // relax along the surface only, flatten the middle
                            let mut t = target - pv;
                            let n = mesh.normals[v as usize];
                            t -= n * t.dot(n);
                            pv + t * (w * i * 0.5) - an * (d * w * w * w * i * 0.4)
                        }
                    };
                    self.scratch.push(q);
                }
                for (k, &(v, _)) in region.iter().enumerate() {
                    mesh.positions[v as usize] = self.scratch[k];
                }
            }
            Tool::SmoothValleys | Tool::SmoothPeaks | Tool::SmoothStronger => {
                let passes = if p.tool == Tool::SmoothStronger { 3 } else { 1 };
                for _ in 0..passes {
                    self.scratch.clear();
                    for &(v, w) in region {
                        let pv = mesh.positions[v as usize];
                        let mut delta = Self::laplacian(mesh, v) - pv;
                        let n = mesh.normals[v as usize];
                        let dn = delta.dot(n);
                        let keep = match p.tool {
                            Tool::SmoothValleys => dn > 0.0,
                            Tool::SmoothPeaks => dn < 0.0,
                            _ => true,
                        };
                        if !keep {
                            delta -= n * dn; // still even out the spacing
                        }
                        let k = if p.tool == Tool::SmoothStronger { (w * i * 1.5).min(1.0) } else { (w * i).min(1.0) };
                        self.scratch.push(pv + delta * k);
                    }
                    for (k, &(v, _)) in region.iter().enumerate() {
                        mesh.positions[v as usize] = self.scratch[k];
                    }
                }
            }
            Tool::Slash => {
                let push = an * (-s * 0.09 * r * i);
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let mut t = p.center - pv;
                    t -= an * t.dot(an);
                    let w3 = w * w * w;
                    mesh.positions[v as usize] = pv + t * (w * i * 0.5) + push * (w3 * w3);
                }
            }
            Tool::CurveSnakehook => {
                let m = p.motion * i;
                for &(v, w) in region {
                    mesh.positions[v as usize] += m * w;
                }
            }
            Tool::ClothInflate => {
                let verts: Vec<u32> = region.iter().map(|r| r.0).collect();
                let edges = self.local_edges(&verts);
                let mesh = &mut self.mesh;
                let region = &self.region;
                let k = s * 0.06 * r * i;
                let mut x: Vec<Vec3> = region
                    .iter()
                    .map(|&(v, w)| mesh.positions[v as usize] + mesh.normals[v as usize] * (k * w))
                    .collect();
                // fabric doesn't stretch: pull edges back towards their length
                let inv: Vec<f32> = region.iter().map(|&(_, w)| w.min(1.0)).collect();
                let edges: Vec<(u32, u32, f32)> = edges.into_iter().map(|(a, b, l)| (a, b, l * 1.02)).collect();
                Self::relax_cloth(&mut x, &inv, &edges, 4, 0.6);
                for (k, &(v, _)) in region.iter().enumerate() {
                    mesh.positions[v as usize] = x[k];
                }
            }
            Tool::Morph => {
                if let Some(target) = self.extra.morph_at(self.level, mesh.positions.len()) {
                    for &(v, w) in region {
                        let pv = mesh.positions[v as usize];
                        mesh.positions[v as usize] = pv.lerp(target[v as usize], (w * i).min(1.0));
                    }
                }
            }
            Tool::Chisel => {
                // raise (or press in) the shape's height field over the disc
                let shape = self.tip.chisel.unwrap_or(ChiselShape::Rivet);
                let (fu, fv) = self.tip.frame;
                let depth = s * r * (0.25 + 1.75 * i);
                let inv_r = 1.0 / r;
                for &(v, w) in region {
                    let pv = mesh.positions[v as usize];
                    let t = (pv - p.center) * inv_r;
                    let h = shape.height(t.dot(fu), t.dot(fv));
                    if h > 0.0 {
                        mesh.positions[v as usize] = pv + an * (h * depth * w);
                    }
                }
            }
            Tool::Grab
            | Tool::Move
            | Tool::MoveElastic
            | Tool::MoveInfiniteDepth
            | Tool::Anchors
            | Tool::ClothPull
            | Tool::ClothTwist
            | Tool::CurveTube
            | Tool::CurveStrap
            | Tool::Paint
            | Tool::GroupPaint => unreachable!(),
        }
        self.after_deform(true, None);
    }

    /// Polygroup paint: faces whose corners are all well inside the brush
    /// (and their quad partners) join `group`.
    fn paint_group(&mut self, group: u32) {
        let inside: std::collections::HashSet<u32> =
            self.region.iter().filter(|r| r.1 > 0.15).map(|r| r.0).collect();
        let Some(st) = self.stroke.as_mut() else { return };
        let mesh = &mut self.mesh;
        let mut changed = false;
        for &v in &inside {
            for &f in mesh.vert_faces(v).to_vec().iter() {
                let t = mesh.tris[f as usize];
                if t[0] == crate::mesh::NONE || !t.iter().all(|u| inside.contains(u)) {
                    continue;
                }
                let p = mesh.partner[f as usize];
                for face in [f, p] {
                    if face == crate::mesh::NONE || mesh.groups[face as usize] == group {
                        continue;
                    }
                    if st.group_seen.insert(face) {
                        st.group_old.push((face, mesh.groups[face as usize]));
                    }
                    mesh.groups[face as usize] = group;
                    changed = true;
                }
            }
        }
        if changed {
            st.modified = true;
            self.dirty.topology = true;
        }
    }

    /// Starts a dragging brush region (once per symmetry side at stroke start).
    pub fn drag_begin(&mut self, center: Vec3, radius: f32, seed_tri: u32, xf: Mat3, gather: Gather, kind: DragKind) {
        let cloth = matches!(kind, DragKind::ClothPull | DragKind::ClothTwist);
        // cloth simulates a wider patch than the brush so folds can form
        let reach = if cloth { radius * 2.5 } else { radius };
        let mut verts: Vec<(u32, f32)> = match gather {
            Gather::Topological => {
                self.collect_region(center, reach, seed_tri, true);
                self.region.clone()
            }
            Gather::Whole => (0..self.mesh.positions.len() as u32)
                .filter(|&v| !self.mesh.vert_faces(v).is_empty() && self.mesh.mask[v as usize] < 1.0)
                .map(|v| (v, 1.0 - self.mesh.mask[v as usize]))
                .collect(),
            Gather::Spatial | Gather::Elastic | Gather::Depth(_) => {
                let far = if gather == Gather::Elastic { radius * 3.0 } else { radius };
                let (mn, mx) = (center - Vec3::splat(far), center + Vec3::splat(far));
                (0..self.mesh.positions.len() as u32)
                    .filter(|&v| !self.mesh.vert_faces(v).is_empty())
                    .filter_map(|v| {
                        let q = self.mesh.positions[v as usize];
                        let d = match gather {
                            Gather::Depth(dir) => {
                                let dir = dir.normalize_or_zero();
                                let t = q - center;
                                (t - dir * t.dot(dir)).length()
                            }
                            _ => {
                                if q.cmplt(mn).any() || q.cmpgt(mx).any() {
                                    return None;
                                }
                                q.distance(center)
                            }
                        };
                        let x = d / radius;
                        let w = match gather {
                            Gather::Elastic => {
                                if d >= far {
                                    return None;
                                }
                                (1.0 / (1.0 + x * x)).powf(1.5) * falloff(d / far)
                            }
                            _ => {
                                if x >= 1.0 {
                                    return None;
                                }
                                self.tip.fall(x)
                            }
                        };
                        Some((v, w * (1.0 - self.mesh.mask[v as usize])))
                    })
                    .collect()
            }
        };
        if self.mesh.any_hidden() {
            verts.retain(|&(v, _)| !self.mesh.vert_hidden(v));
        }
        if cloth {
            // the brush moves its own radius; the rest follows as cloth
            for (k, &(v, _)) in verts.clone().iter().enumerate() {
                let d = self.mesh.positions[v as usize].distance(center);
                verts[k].1 = self.tip.fall(d / radius) * (1.0 - self.mesh.mask[v as usize]);
            }
        }
        let ids: Vec<u32> = verts.iter().map(|r| r.0).collect();
        let orig: Vec<Vec3> = ids.iter().map(|&v| self.mesh.positions[v as usize]).collect();
        let nrm: Vec<Vec3> = ids.iter().map(|&v| self.mesh.normals[v as usize]).collect();
        let normal = verts
            .iter()
            .zip(&nrm)
            .map(|(r, n)| *n * (r.1 + 1e-4))
            .sum::<Vec3>()
            .try_normalize()
            .unwrap_or(Vec3::Z);
        let (inv_mass, edges) = if cloth {
            let inv = ids
                .iter()
                .zip(&verts)
                .map(|(&v, r)| {
                    let d = self.mesh.positions[v as usize].distance(center);
                    if d > reach * 0.85 || self.mesh.boundary[v as usize] && d > radius { 0.0 } else { (1.0 - r.1).powi(2) }
                })
                .collect();
            (inv, self.local_edges(&ids))
        } else {
            (Vec::new(), Vec::new())
        };
        let bounds = orig.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        if let Some(st) = self.stroke.as_mut() {
            st.grab.push(GrabSet { verts, orig, xf, kind, center, normal, nrm, inv_mass, edges, bounds });
        }
    }

    /// Updates every dragging-brush region for the current pointer.
    pub fn drag_update(&mut self, input: DragInput) {
        let Some(mut st) = self.stroke.take() else { return };
        let mut moved: Vec<u32> = Vec::new();
        for set in &st.grab {
            let det = set.xf.determinant().signum();
            let d = set.xf * input.delta;
            let view = (set.xf * input.view).normalize_or_zero();
            let angle = input.screen.x * 0.01 * det;
            let mut x: Vec<Vec3> = Vec::with_capacity(set.verts.len());
            if let DragKind::Deform(kind, axis) = set.kind {
                let moved = crate::deform::apply(kind, axis, input.amount, &set.orig, &set.nrm, set.bounds);
                for (k, &(_, w)) in set.verts.iter().enumerate() {
                    x.push(set.orig[k].lerp(moved[k], w));
                }
            }
            for (k, &(_, w)) in set.verts.iter().enumerate() {
                if matches!(set.kind, DragKind::Deform(..)) {
                    break;
                }
                let o = set.orig[k];
                let rel = o - set.center;
                let rot = |axis: Vec3, a: f32| -> Vec3 {
                    if axis.length_squared() < 1e-12 {
                        return o;
                    }
                    set.center + Quat::from_axis_angle(axis.normalize(), a * w) * rel
                };
                let q = match set.kind {
                    DragKind::Move | DragKind::ClothPull | DragKind::Anchor(AnchorMode::Move) => o + d * w,
                    DragKind::Anchor(AnchorMode::Rotate) => rot(view, angle),
                    DragKind::Anchor(AnchorMode::Twist) | DragKind::ClothTwist => rot(set.normal, angle),
                    DragKind::Anchor(AnchorMode::Bend) => rot(set.normal.cross(view), angle),
                    DragKind::Anchor(AnchorMode::Scale) => {
                        set.center + rel * (1.0 - input.screen.y * 0.005 * w).max(0.05)
                    }
                    DragKind::Anchor(AnchorMode::Inflate) => o + set.nrm[k] * (-input.screen.y * input.wpp * w),
                    DragKind::Affine => o.lerp(input.affine.transform_point3(o), w),
                    DragKind::Deform(..) => o,
                };
                x.push(q);
            }
            if !set.edges.is_empty() {
                Self::relax_cloth(&mut x, &set.inv_mass, &set.edges, 12, 1.0);
            }
            for (k, &(v, _)) in set.verts.iter().enumerate() {
                st.modified = true;
                if st.snapshot.is_none() && st.mark.mark(v) {
                    st.touched.push(v);
                    st.old_pos.push(self.mesh.positions[v as usize]);
                    st.old_mask.push(self.mesh.mask[v as usize]);
                }
                self.mesh.positions[v as usize] = x[k];
                moved.push(v);
            }
        }
        self.stroke = Some(st);
        self.after_deform(false, Some(&moved));
    }

    /// Dynamic topology for one dab. Returns a live face to start the brush
    /// region from (the hit face may have been collapsed away).
    fn remesh(&mut self, seed: u32, center: Vec3, radius: f32, detail: f32) -> u32 {
        // dynamic topology rebuilds faces, so texture coordinates can't survive
        self.mesh.uvs.clear();
        let mut changed = Vec::new();
        let stats = {
            let Sculptor { mesh, fmark, vmark, .. } = self;
            remesh_region(mesh, seed, center, radius, detail, fmark, vmark, &mut changed)
        };
        if stats.splits + stats.collapses == 0 {
            return seed;
        }
        if let Some(st) = self.stroke.as_mut() {
            st.modified = true;
            // Layer heights: new vertices start between their edge's ends
            if !st.layer_base.is_empty() {
                let pos = &self.mesh.positions;
                let base = |b: &std::collections::HashMap<u32, Vec3>, v: u32| {
                    b.get(&v).copied().unwrap_or(pos[v as usize])
                };
                for &(nv, a, b) in &stats.split_log {
                    let mid = (base(&st.layer_base, a) + base(&st.layer_base, b)) * 0.5;
                    st.layer_base.insert(nv, mid);
                }
                for &(a, b) in &stats.collapse_log {
                    let mid = (base(&st.layer_base, a) + base(&st.layer_base, b)) * 0.5;
                    st.layer_base.insert(a, mid);
                    st.layer_base.remove(&b);
                }
            }
        }
        {
            let Sculptor { mesh, fmark, vmark2, .. } = self;
            mesh.update_normals(&changed, fmark, vmark2);
        }
        for &v in &changed {
            let faces = self.mesh.vert_faces(v);
            self.bvh_faces.extend_from_slice(faces);
        }
        self.dirty.topology = true;
        self.bvh_dirty = true;
        if self.mesh.is_alive(seed) {
            return seed;
        }
        changed
            .iter()
            .filter_map(|&v| self.mesh.vert_faces(v).first().map(|&f| (self.mesh.positions[v as usize].distance_squared(center), f)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(seed, |(_, f)| f)
    }

    /// Starts a grab region (call once per symmetry side at stroke start).
    pub fn grab_begin(&mut self, center: Vec3, radius: f32, seed_tri: u32, xf: Mat3) {
        self.drag_begin(center, radius, seed_tri, xf, Gather::Topological, DragKind::Move);
    }

    /// Moves grabbed vertices by `delta` (world units) from where they started.
    pub fn grab_update(&mut self, delta: Vec3) {
        self.drag_update(DragInput { delta, ..Default::default() });
    }

    fn apply(&mut self, e: Undo) -> Undo {
        self.revision += 1;
        match e {
            Undo::Verts { idx, pos, mask } => {
                let mut cur_pos = Vec::with_capacity(idx.len());
                let mut cur_mask = Vec::with_capacity(idx.len());
                for (k, &v) in idx.iter().enumerate() {
                    cur_pos.push(self.mesh.positions[v as usize]);
                    cur_mask.push(self.mesh.mask[v as usize]);
                    self.mesh.positions[v as usize] = pos[k];
                    self.mesh.mask[v as usize] = mask[k];
                }
                self.after_deform(false, Some(&idx));
                Undo::Verts { idx, pos: cur_pos, mask: cur_mask }
            }
            Undo::State(s) => Undo::State(self.restore(s)),
            Undo::Colors { idx, col } => {
                let mut cur = Vec::with_capacity(idx.len());
                if self.mesh.colors.len() == self.mesh.positions.len() {
                    for (k, &v) in idx.iter().enumerate() {
                        cur.push(self.mesh.colors[v as usize]);
                        self.mesh.colors[v as usize] = col[k];
                        self.dirty.add(v);
                    }
                }
                Undo::Colors { idx, col: cur }
            }
            Undo::Pose(b) => {
                let (mesh, pose) = *b;
                let cur_mesh = std::mem::replace(&mut self.mesh, mesh);
                let cur_pose = std::mem::replace(&mut self.extra.pose, pose);
                self.topology_changed();
                Undo::Pose(Box::new((cur_mesh, cur_pose)))
            }
            Undo::AllColors(c) => {
                let cur = std::mem::replace(&mut self.mesh.colors, c);
                self.dirty.all(self.mesh.positions.len());
                Undo::AllColors(cur)
            }
            Undo::Extra(b) => {
                let (extra, pos) = *b;
                let cur_extra = std::mem::replace(&mut self.extra, extra);
                let cur_pos = if pos.len() == self.mesh.positions.len() {
                    let c = std::mem::replace(&mut self.mesh.positions, pos);
                    self.mesh.compute_all_normals();
                    self.dirty.all(self.mesh.positions.len());
                    self.bvh_dirty = true;
                    self.bvh_faces.clear();
                    c
                } else {
                    Vec::new()
                };
                Undo::Extra(Box::new((cur_extra, cur_pos)))
            }
            Undo::Groups { faces, groups } => {
                let mut cur = Vec::with_capacity(faces.len());
                for (k, &f) in faces.iter().enumerate() {
                    if let Some(g) = self.mesh.groups.get_mut(f as usize) {
                        cur.push(*g);
                        *g = groups[k];
                    } else {
                        cur.push(0);
                    }
                }
                self.dirty.topology = true;
                Undo::Groups { faces, groups: cur }
            }
            Undo::Hidden(h) => {
                let cur = std::mem::replace(&mut self.mesh.hidden, h);
                self.dirty.topology = true;
                Undo::Hidden(cur)
            }
            Undo::Level(l) => {
                let cur = self.level;
                self.goto_level(l);
                Undo::Level(cur)
            }
            Undo::DropTop { then_level } => {
                let cur = self.level;
                let top = self.levels.len() - 1;
                self.goto_level(top);
                let coarse = self.levels[top - 1].mesh.take().expect("level mesh");
                let fine = std::mem::replace(&mut self.mesh, coarse);
                let mut level = self.levels.pop().expect("top level");
                level.mesh = Some(fine);
                self.level = top - 1;
                self.topology_changed();
                self.goto_level(then_level);
                Undo::AddTop { level: Box::new(level), then_level: cur }
            }
            Undo::AddTop { level, then_level } => {
                let cur = self.level;
                let top = self.levels.len() - 1;
                self.goto_level(top);
                let mut level = *level;
                let fine = level.mesh.take().expect("level mesh");
                let coarse = std::mem::replace(&mut self.mesh, fine);
                self.levels[top].mesh = Some(coarse);
                self.levels.push(level);
                self.level = top + 1;
                self.topology_changed();
                self.goto_level(then_level);
                Undo::DropTop { then_level: cur }
            }
        }
    }

    pub fn undo(&mut self) {
        if !self.can_undo() {
            return;
        }
        let e = self.undo.pop().unwrap();
        let inv = self.apply(e);
        self.redo.push(inv);
        if let Some(l) = self.undo_labels.pop() {
            self.redo_labels.push(l);
        }
    }

    pub fn redo(&mut self) {
        if !self.can_redo() {
            return;
        }
        let e = self.redo.pop().unwrap();
        let inv = self.apply(e);
        self.undo.push(inv);
        if let Some(l) = self.redo_labels.pop() {
            self.undo_labels.push(l);
        }
    }

    fn mask_op(&mut self, f: impl Fn(f32) -> f32) {
        self.end_stroke();
        let n = self.mesh.vertex_count();
        let idx: Vec<u32> = (0..n as u32).collect();
        let old = self.mesh.mask.clone();
        for m in &mut self.mesh.mask {
            *m = f(*m);
        }
        self.push_undo(Undo::Verts { idx, pos: self.mesh.positions.clone(), mask: old });
        self.dirty.all(n);
    }

    /// Snaps every vertex of the active level onto `target`'s surface (part
    /// of the step that created this level, so no undo step of its own).
    pub fn project_onto(&mut self, target: &Mesh, bvh: &Bvh, reach: f32) {
        self.end_stroke();
        for p in &mut self.mesh.positions {
            if let Some(q) = bvh.closest_point(target, *p, reach) {
                *p = q;
            }
        }
        self.mesh.compute_all_normals();
        self.topology_changed();
    }

    /// Masks every vertex not touching a face of polygroup `g` (undoable).
    pub fn mask_outside_group(&mut self, g: u32) {
        self.end_stroke();
        let n = self.mesh.vertex_count();
        let mut inside = vec![false; n];
        for (f, t) in self.mesh.tris.iter().enumerate() {
            if t[0] != crate::mesh::NONE && self.mesh.groups[f] == g {
                for &v in t {
                    inside[v as usize] = true;
                }
            }
        }
        let old = self.mesh.mask.clone();
        for (v, m) in self.mesh.mask.iter_mut().enumerate() {
            *m = if inside[v] { 0.0 } else { 1.0 };
        }
        let idx: Vec<u32> = (0..n as u32).collect();
        self.push_undo(Undo::Verts { idx, pos: self.mesh.positions.clone(), mask: old });
        self.dirty.all(n);
    }

    pub fn clear_mask(&mut self) {
        self.set_label("Clear mask");
        self.mask_op(|_| 0.0);
    }
    pub fn invert_mask(&mut self) {
        self.set_label("Invert mask");
        self.mask_op(|m| 1.0 - m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front_hit(s: &Sculptor) -> Hit {
        s.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap()
    }

    fn dab(s: &mut Sculptor, tool: Tool, negative: bool) {
        let h = front_hit(s);
        s.dab(&DabParams { tool, center: h.point, radius: 0.4, intensity: 1.0, negative, seed_tri: h.tri, detail: None, motion: Vec3::new(0.05, 0.0, 0.0), ..Default::default() });
        s.refresh_bvh();
    }

    #[test]
    fn draw_raises_and_undo_restores() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        for _ in 0..5 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let after = front_hit(&s).point.z;
        assert!(after > before + 0.05, "{before} -> {after}");
        s.undo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - before).abs() < 1e-5);
        s.redo();
        s.refresh_bvh();
        assert!((front_hit(&s).point.z - after).abs() < 1e-5);
    }

    #[test]
    fn negative_draw_digs() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        dab(&mut s, Tool::Draw, true);
        s.end_stroke();
        assert!(front_hit(&s).point.z < before);
    }

    #[test]
    fn mask_blocks_draw() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        s.begin_stroke(false);
        for _ in 0..10 {
            dab(&mut s, Tool::Mask, false);
        }
        s.end_stroke();
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        dab(&mut s, Tool::Draw, false);
        s.end_stroke();
        assert!((front_hit(&s).point.z - before).abs() < 0.01);
    }

    #[test]
    fn smooth_and_others_stay_finite() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        for t in Tool::ALL {
            s.begin_stroke(false);
            dab(&mut s, t, false);
            dab(&mut s, t, true);
            s.end_stroke();
        }
        assert!(s.mesh.positions.iter().all(|p| p.is_finite()));
        assert!(s.mesh.normals.iter().all(|n| n.is_finite()));
    }

    fn top_z(s: &Sculptor) -> f32 {
        s.mesh.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max)
    }

    #[test]
    fn trim_and_scrape_never_raise() {
        for tool in [Tool::TrimDynamic, Tool::Scrape] {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let before = top_z(&s);
            s.begin_stroke(false);
            for _ in 0..5 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            assert!(top_z(&s) < before - 0.005, "{tool:?} should cut the dome down");
        }
    }

    #[test]
    fn layer_height_is_capped_per_stroke() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let before = front_hit(&s).point.z;
        s.begin_stroke(false);
        for _ in 0..40 {
            dab(&mut s, Tool::Layer, false);
        }
        s.end_stroke();
        let rise = front_hit(&s).point.z - before;
        assert!(rise > 0.02 && rise <= 0.4 * 0.12 + 1e-3, "{rise}");
    }

    #[test]
    fn layer_with_dynamic_topology_stays_level() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let before = front_hit(&s).point.z;
        s.begin_stroke(true);
        for _ in 0..30 {
            let h = front_hit(&s);
            s.dab(&DabParams {
                tool: Tool::Layer,
                center: h.point,
                radius: 0.4,
                intensity: 1.0,
                negative: false,
                seed_tri: h.tri,
                detail: Some(0.03),
                motion: Vec3::ZERO,
                ..Default::default()
            });
            s.refresh_bvh();
        }
        s.end_stroke();
        // no point may rise above the layer height (no spikes from new vertices)
        let max_rise = s.mesh.positions.iter().map(|p| p.length() - 1.0).fold(f32::MIN, f32::max);
        assert!(max_rise <= 0.4 * 0.12 + 0.01, "{max_rise}");
        assert!(front_hit(&s).point.z > before + 0.02);
    }

    #[test]
    fn dam_standard_digs_deeper_than_crease() {
        let depth = |tool| {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let before = front_hit(&s).point.z;
            s.begin_stroke(false);
            for _ in 0..3 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            before - front_hit(&s).point.z
        };
        assert!(depth(Tool::DamStandard) > depth(Tool::Crease));
    }

    #[test]
    fn snakehook_and_nudge_follow_motion() {
        for tool in [Tool::Snakehook, Tool::Nudge] {
            let mut s = Sculptor::new(Mesh::icosphere(4));
            let cx: f32 = s.mesh.positions.iter().map(|p| p.x).sum();
            s.begin_stroke(false);
            for _ in 0..4 {
                dab(&mut s, tool, false);
            }
            s.end_stroke();
            let after: f32 = s.mesh.positions.iter().map(|p| p.x).sum();
            assert!(after > cx + 0.05, "{tool:?} should move points along +x");
        }
    }

    #[test]
    fn grab_moves_with_mirror() {
        let mut s = Sculptor::new(Mesh::icosphere(4));
        let h = s.raycast(Vec3::new(0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        let hm = s.raycast(Vec3::new(-0.7, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        s.begin_stroke(false);
        s.grab_begin(h.point, 0.3, h.tri, Mat3::IDENTITY);
        s.grab_begin(hm.point, 0.3, hm.tri, Mat3::from_diagonal(Vec3::new(-1.0, 1.0, 1.0)));
        s.grab_update(Vec3::new(0.5, 0.0, 0.0));
        s.end_stroke();
        let maxx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        let minx = s.mesh.positions.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!(maxx > 1.2 && minx < -1.2, "{minx} {maxx}");
    }

    fn closed_manifold(m: &Mesh) -> bool {
        let mut count = std::collections::HashMap::new();
        for t in m.tris.iter().filter(|t| t[0] != crate::mesh::NONE) {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        count.values().all(|&c| c == 2)
    }

    #[test]
    fn dynamic_stroke_adds_detail_and_undoes() {
        let mut s = Sculptor::new(Mesh::icosphere(3));
        let n0 = s.tri_count();
        let before = front_hit(&s).point.z;
        s.begin_stroke(true);
        for _ in 0..6 {
            let h = front_hit(&s);
            s.dab(&DabParams {
                tool: Tool::Draw,
                center: h.point,
                radius: 0.4,
                intensity: 1.0,
                negative: false,
                seed_tri: h.tri,
                detail: Some(0.03),
                motion: Vec3::ZERO,
                ..Default::default()
            });
            s.refresh_bvh();
        }
        s.end_stroke();
        assert!(s.tri_count() > n0 * 2, "{} -> {}", n0, s.tri_count());
        assert!(closed_manifold(&s.mesh));
        assert!(s.mesh.tris.iter().all(|t| t[0] != crate::mesh::NONE), "compacted after the stroke");
        assert!(front_hit(&s).point.z > before + 0.05);
        s.undo();
        assert_eq!(s.tri_count(), n0);
        s.redo();
        assert!(s.tri_count() > n0 * 2);
    }

    #[test]
    fn dynamic_topology_needs_one_level() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        assert!(!s.dyntopo_allowed());
        let n = s.tri_count();
        s.begin_stroke(true);
        let h = front_hit(&s);
        s.dab(&DabParams { tool: Tool::Draw, center: h.point, radius: 0.4, intensity: 1.0, negative: false, seed_tri: h.tri, detail: Some(0.01), motion: Vec3::ZERO, ..Default::default() });
        s.end_stroke();
        assert_eq!(s.tri_count(), n);
        s.delete_lower();
        assert!(s.dyntopo_allowed());
        assert_eq!(s.tri_count(), n);
    }

    #[test]
    fn multires_round_trip_keeps_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        // sculpt fine detail at the top level
        s.begin_stroke(false);
        for _ in 0..4 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let top = s.mesh.positions.clone();
        s.set_level(0);
        assert_eq!(s.tri_count(), 320);
        s.set_level(2);
        let err = top.iter().zip(&s.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "detail lost: {err}");
    }

    #[test]
    fn multires_low_level_edit_moves_high_level() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..4 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let fine_detail = s.mesh.positions.clone();
        s.set_level(0);
        // move the whole base mesh right
        for p in &mut s.mesh.positions {
            p.x += 0.5;
        }
        s.mesh.compute_all_normals();
        s.set_level(1);
        let avg: f32 = s.mesh.positions.iter().zip(&fine_detail).map(|(a, b)| a.x - b.x).sum::<f32>()
            / fine_detail.len() as f32;
        assert!((avg - 0.5).abs() < 0.02, "{avg}");
        // and the sculpted bump survives the move (relative to the shifted shape)
        let bump = s.mesh.positions.iter().map(|p| p.z).fold(f32::MIN, f32::max);
        assert!(bump > 1.05, "{bump}");
        s.undo();
        assert_eq!(s.level(), 0);
    }

    #[test]
    fn split_subdivide_matches_and_aborts_cleanly() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.set_level(0);
        let undo_depth = s.undo.len();
        s.begin_subdivide().unwrap();
        assert_eq!(s.level(), 1, "moves to the top level first");
        s.abort_subdivide();
        assert_eq!((s.level(), s.level_count(), s.undo.len()), (0, 2, undo_depth));
        assert!(!s.can_redo());
        s.begin_subdivide().unwrap();
        let (fine, odd) = s.mesh.subdivide_with_stencil(true);
        let bytes = crate::wire::encode(&fine, &odd);
        let (fine, odd) = crate::wire::decode(&bytes).unwrap();
        s.finish_subdivide(fine, odd, None).unwrap();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        s.set_level(0);
        s.set_level(2);
        s.undo();
        s.undo();
        s.undo();
        assert_eq!(s.level_count(), 2);
        assert_eq!(s.level(), 0, "back where the subdivision started");
        s.redo();
        assert_eq!((s.level(), s.level_count()), (2, 3));
    }

    #[test]
    fn level_and_subdivide_undo_round_trip_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..3 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        let sculpted = s.mesh.positions.clone();
        s.subdivide().unwrap();
        s.undo(); // drop level 3
        assert_eq!((s.level(), s.level_count()), (1, 2));
        let err = sculpted.iter().zip(&s.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
        s.redo();
        assert_eq!((s.level(), s.level_count()), (2, 3));
        s.set_level(0);
        s.undo();
        assert_eq!(s.level(), 2);
        s.redo();
        assert_eq!(s.level(), 0);
    }

    #[test]
    fn project_round_trip_keeps_levels_and_detail() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        s.subdivide().unwrap();
        s.subdivide().unwrap();
        s.begin_stroke(false);
        for _ in 0..3 {
            dab(&mut s, Tool::Draw, false);
        }
        s.end_stroke();
        s.set_level(1);
        let (levels, active) = s.project_levels();
        let bytes = crate::project::encode(&levels, active, r#"{"hello":1}"#);
        let mut p = crate::project::decode(&bytes).unwrap();
        assert_eq!(p.settings_json, r#"{"hello":1}"#);
        let st = p.subtools.remove(0);
        let loaded: Vec<crate::project::LevelData<Mesh>> = st
            .levels
            .into_iter()
            .map(|l| {
                let (b, st) = l.mesh.into_builder();
                crate::project::LevelData { mesh: b.finish(), stencils: st, details: l.details }
            })
            .collect();
        let mut t = Sculptor::new(Mesh::icosphere(1));
        let rev = t.revision();
        t.load_levels(loaded, st.active, None);
        assert!(t.revision() > rev);
        assert_eq!((t.level(), t.level_count()), (1, 3));
        s.set_level(2);
        t.set_level(2);
        let err = s.mesh.positions.iter().zip(&t.mesh.positions).map(|(a, b)| (*a - *b).length()).fold(0.0f32, f32::max);
        assert!(err < 1e-5, "{err}");
        t.undo(); // back to level 1
        t.undo(); // opening the project is undoable too
        assert_eq!(t.level_count(), 1);
        assert!(crate::project::decode(&bytes[..30]).is_err());
    }

    #[test]
    fn subdivide_undo() {
        let mut s = Sculptor::new(Mesh::icosphere(2));
        let n = s.tri_count();
        s.subdivide().unwrap();
        assert_eq!(s.tri_count(), n * 4);
        s.undo();
        assert_eq!(s.tri_count(), n);
    }
}
