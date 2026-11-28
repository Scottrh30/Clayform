//! Clayform's interface (Leptos, compiled to WebAssembly). The sculpting
//! engine runs right here in the page so strokes never wait on the host;
//! the Tauri shell only provides native file dialogs.

mod brush_ui;
mod detail_ui;
mod topo_ui;
mod paint_ui;
mod special_ui;
mod select_ui;
mod host;
mod render;
mod conveniences;
mod keys;
mod output_ui;
mod scene_ui;
mod tools_ui;

use clayform_core::editor::{Brush, Button, DetailMode, Drag, Editor, Mode, Mods, Overlay, ViewDir, ZmSettings};
use clayform_core::sculpt::{AnchorMode, TOOL_CATEGORIES};
use clayform_core::snapshot::SnapshotSettings;
use tools_ui::RefImage;
use clayform_core::glam::Vec2;
use clayform_core::io;
use clayform_core::bvh::{Bvh, BvhBuilder};
use clayform_core::mesh::Mesh;
use clayform_core::project::{self, LevelData};
use clayform_core::symmetry::{Symmetry, AXIS_NAMES, MAX_RADIAL};
use clayform_core::remesh::{VoxelSettings, CANCELLED, MAX_RESOLUTION, MIN_RESOLUTION};
use clayform_core::wire;
use clayform_core::sculpt::{Tool, MAX_TRIANGLES};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use render::{Renderer, ViewSettings, BACKGROUNDS, CUSTOM_MATCAP, MATERIALS};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

thread_local! {
    static EDITOR: RefCell<Editor> = RefCell::new(Editor::new());
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

fn with_ed<R>(f: impl FnOnce(&mut Editor) -> R) -> R {
    EDITOR.with(|e| f(&mut e.borrow_mut()))
}

const PANEL_SIDE_KEY: &str = "clayform.panelSide";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// Interface state. Every field is a cheap, copyable signal handle.
#[derive(Clone, Copy)]
struct Ui {
    tool: RwSignal<Tool>,
    radius: RwSignal<f32>,
    intensity: RwSignal<[f32; Tool::ALL.len()]>,
    negative: RwSignal<bool>,
    symmetry: RwSignal<Symmetry>,
    dyntopo: RwSignal<bool>,
    detail: RwSignal<f32>,
    level: RwSignal<usize>,
    level_count: RwSignal<usize>,
    top_tris: RwSignal<usize>,
    voxel: RwSignal<VoxelSettings>,
    busy: RwSignal<bool>,
    /// Background job shown in the progress card: (label, progress 0..1).
    job: RwSignal<Option<(&'static str, f32)>>,
    detail_mode: RwSignal<DetailMode>,
    smooth_reduces: RwSignal<bool>,
    view: RwSignal<ViewSettings>,
    fov: RwSignal<f32>,
    panel_right: RwSignal<bool>,
    sphere_detail: RwSignal<u32>,
    stats: RwSignal<(usize, usize)>,
    can_undo: RwSignal<bool>,
    can_redo: RwSignal<bool>,
    status: RwSignal<String>,
    menu: RwSignal<Option<&'static str>>,
    cursor: RwSignal<Option<(f32, f32)>>,
    mirror: RwSignal<Vec<(f32, f32)>>,
    /// Open project: where it's saved (desktop), its file name, and the
    /// engine revision at the last save (to show unsaved changes).
    project_path: RwSignal<Option<String>>,
    project_name: RwSignal<String>,
    saved_rev: RwSignal<u64>,
    rev: RwSignal<u64>,
    mods: RwSignal<(bool, bool)>,
    dragging: RwSignal<Drag>,
    gpu: RwSignal<Option<String>>,
    mode: RwSignal<Mode>,
    anchor_mode: RwSignal<AnchorMode>,
    zm: RwSignal<ZmSettings>,
    /// Lines and points drawn over the viewport for the current mode.
    overlay: RwSignal<Overlay>,
    vp_size: RwSignal<(f32, f32)>,
    snap: RwSignal<SnapshotSettings>,
    /// Snapshot image preview (data URL).
    snap_thumb: RwSignal<Option<String>>,
    /// Size of an added/subtracted snapshot relative to the model.
    snap_scale: RwSignal<f32>,
    refs: RwSignal<Vec<RefImage>>,
    retopo_levels: RwSignal<u32>,
    /// Retopology vertices and faces.
    retopo_count: RwSignal<(usize, usize)>,
    group_angle: RwSignal<f32>,
    bulk: RwSignal<f32>,
    render: RwSignal<output_ui::RenderSettings>,
    keymap: RwSignal<keys::KeyMap>,
    /// The shortcut editor is waiting for a key for this command.
    key_capture: RwSignal<Option<String>>,
    show_keys: RwSignal<bool>,
    recent: RwSignal<Vec<String>>,
    /// Minutes between autosaves (0 = off).
    autosave_minutes: RwSignal<u32>,
    /// A recovery copy from an earlier session (when it was written, ms).
    recovery: RwSignal<Option<f64>>,
    autosaved_at: RwSignal<Option<f64>>,
    subtools: RwSignal<Vec<clayform_core::scene::SubToolInfo>>,
    active_subtool: RwSignal<usize>,
    live_bool: RwSignal<bool>,
    /// Live Boolean preview resolution.
    bool_res: RwSignal<u32>,
    gizmo: RwSignal<clayform_core::editor::GizmoSettings>,
    stroke: RwSignal<clayform_core::brush::StrokeSettings>,
    falloff: RwSignal<clayform_core::brush::FalloffCurve>,
    alpha: RwSignal<Option<std::sync::Arc<clayform_core::brush::Alpha>>>,
    user_alphas: RwSignal<Vec<std::sync::Arc<clayform_core::brush::Alpha>>>,
    /// (a pen was seen, its latest pressure)
    pressure: RwSignal<(bool, f32)>,
    sdetail: RwSignal<detail_ui::DetailState>,
    noise: RwSignal<clayform_core::detail::NoiseSettings>,
    paint_color: RwSignal<clayform_core::glam::Vec3>,
    /// The next click on the model picks its colour.
    picking: RwSignal<bool>,
    spot_on: RwSignal<bool>,
    spot_ref: RwSignal<Option<u32>>,
}

impl Ui {
    fn new() -> Self {
        let b = Brush::default();
        Self {
            tool: RwSignal::new(b.tool),
            radius: RwSignal::new(b.radius_px),
            intensity: RwSignal::new(b.intensity),
            negative: RwSignal::new(b.negative),
            symmetry: RwSignal::new(b.symmetry),
            dyntopo: RwSignal::new(b.dyntopo),
            detail: RwSignal::new(b.detail),
            level: RwSignal::new(0),
            level_count: RwSignal::new(1),
            top_tris: RwSignal::new(0),
            voxel: RwSignal::new(VoxelSettings::default()),
            busy: RwSignal::new(false),
            job: RwSignal::new(None),
            detail_mode: RwSignal::new(b.detail_mode),
            smooth_reduces: RwSignal::new(b.smooth_reduces),
            view: RwSignal::new(ViewSettings {
                material: 0,
                show_mask: true,
                flat: false,
                wireframe: false,
                background: 0,
                ao: true,
                ao_strength: 0.8,
                cavity: true,
                cavity_strength: 0.6,
                polygroups: false,
                paint: true,
                lighting: render::Lighting::default(),
            }),
            fov: RwSignal::new(40.0),
            panel_right: RwSignal::new(
                storage().and_then(|s| s.get_item(PANEL_SIDE_KEY).ok().flatten()).is_some_and(|v| v == "right"),
            ),
            sphere_detail: RwSignal::new(6),
            stats: RwSignal::new((0, 0)),
            can_undo: RwSignal::new(false),
            can_redo: RwSignal::new(false),
            status: RwSignal::new("Ready — drag on the sphere to sculpt".into()),
            menu: RwSignal::new(None),
            cursor: RwSignal::new(None),
            mirror: RwSignal::new(Vec::new()),
            project_path: RwSignal::new(None),
            project_name: RwSignal::new("Untitled".into()),
            saved_rev: RwSignal::new(0),
            rev: RwSignal::new(0),
            mods: RwSignal::new((false, false)),
            dragging: RwSignal::new(Drag::None),
            gpu: RwSignal::new(None),
            mode: RwSignal::new(Mode::Sculpt),
            anchor_mode: RwSignal::new(b.anchor_mode),
            stroke: RwSignal::new(b.stroke),
            falloff: RwSignal::new(Default::default()),
            alpha: RwSignal::new(None),
            user_alphas: RwSignal::new(Vec::new()),
            pressure: RwSignal::new((false, 1.0)),
            sdetail: RwSignal::new(Default::default()),
            noise: RwSignal::new(detail_ui::default_noise()),
            paint_color: RwSignal::new(clayform_core::glam::Vec3::new(0.85, 0.2, 0.15)),
            picking: RwSignal::new(false),
            spot_on: RwSignal::new(false),
            spot_ref: RwSignal::new(None),
            zm: RwSignal::new(ZmSettings::default()),
            overlay: RwSignal::new(Overlay::default()),
            vp_size: RwSignal::new((800.0, 600.0)),
            snap: RwSignal::new(SnapshotSettings::default()),
            snap_thumb: RwSignal::new(None),
            snap_scale: RwSignal::new(0.6),
            refs: RwSignal::new(Vec::new()),
            retopo_levels: RwSignal::new(2),
            retopo_count: RwSignal::new((0, 0)),
            group_angle: RwSignal::new(30.0),
            bulk: RwSignal::new(1.0),
            render: RwSignal::new(Default::default()),
            keymap: RwSignal::new(keys::load()),
            key_capture: RwSignal::new(None),
            show_keys: RwSignal::new(false),
            recent: RwSignal::new(conveniences::load_recent()),
            autosave_minutes: RwSignal::new(conveniences::autosave_minutes()),
            recovery: RwSignal::new(None),
            autosaved_at: RwSignal::new(None),
            subtools: RwSignal::new(Vec::new()),
            active_subtool: RwSignal::new(0),
            live_bool: RwSignal::new(false),
            bool_res: RwSignal::new(96),
            gizmo: RwSignal::new(Default::default()),
        }
    }

    /// Pulls mesh statistics and undo state from the engine.
    fn refresh(self) {
        let (v, t, u, r, lv, lc, top, rev, rt, msg, sts, act) = with_ed(|e| {
            let rt = (e.retopo.verts.len(), e.retopo.faces.len());
            let sts: Vec<_> = (0..e.subtool_count()).map(|i| e.subtool_info(i).clone()).collect();
            let act = e.active_subtool();
            let scene_rev = e.scene_revision();
            let retopo_undo = (e.mode == Mode::Retopo && e.retopo.can_undo()) || (e.mode == Mode::ZSpheres && e.zspheres.can_undo());
            let s = &e.sculptor;
            (
                s.mesh.vertex_count(),
                s.tri_count(),
                s.can_undo() || retopo_undo,
                s.can_redo(),
                s.level(),
                s.level_count(),
                s.top_tri_count(),
                scene_rev,
                rt,
                e.message.take(),
                sts,
                act,
            )
        });
        if sts != self.subtools.get_untracked() {
            self.subtools.set(sts);
        }
        if act != self.active_subtool.get_untracked() {
            self.active_subtool.set(act);
        }
        self.retopo_count.set(rt);
        if let Some(m) = msg {
            self.status.set(m);
        }
        self.update_overlay();
        // reference images are saved in the project too: changing them
        // counts as an unsaved change
        let rev = rev.wrapping_add(tools_ui::refs_fingerprint(self));
        if rev != self.rev.get_untracked() {
            self.rev.set(rev);
        }
        self.stats.set((v, t));
        self.can_undo.set(u);
        self.can_redo.set(r);
        self.level.set(lv);
        self.level_count.set(lc);
        self.top_tris.set(top);
        detail_ui::refresh(self);
    }

    /// Recomputes what's drawn over the viewport (hover targets, retopology, joints).
    fn update_overlay(self) {
        let cursor = self.cursor.get_untracked().map(|c| Vec2::new(c.0, c.1));
        let (w, h) = self.vp_size.get_untracked();
        let o = with_ed(|e| e.overlay(cursor, Vec2::new(w.max(1.0), h.max(1.0))));
        if o != self.overlay.get_untracked() {
            self.overlay.set(o);
        }
    }
}

fn fmt_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

// ----------------------------------------------------------------- actions

// Some variants are only used inside view! closures, which rustc's
// dead-code lint can't see through.
#[allow(dead_code)]
#[derive(Clone, Copy)]
enum Action {
    NewSphere,
    NewCube,
    NewPlane,
    Import,
    OpenProject,
    SaveProject,
    SaveProjectAs,
    ExportObj,
    ExportStl,
    Undo,
    Redo,
    Subdivide,
    LevelUp,
    LevelDown,
    DeleteLower,
    DeleteHigher,
    ToggleDyntopo,
    VoxelRemesh,
    LoadMatcap,
    ClearMask,
    InvertMask,
    FrameMesh,
    ResetCamera,
    View(ViewDir),
    TogglePanelSide,
    NewPrimitive(clayform_core::poly::Primitive),
    NewMannequin,
    Export(&'static str),
    RenderImage,
    RenderTurntable,
}

fn act(ui: Ui, a: Action) {
    ui.menu.set(None);
    let changes_mesh = !matches!(
        a,
        Action::FrameMesh
            | Action::ResetCamera
            | Action::View(_)
            | Action::TogglePanelSide
            | Action::ExportObj
            | Action::ExportStl
            | Action::Export(_)
            | Action::SaveProject
            | Action::SaveProjectAs
    );
    if changes_mesh && ui.busy.get_untracked() {
        ui.status.set("Please wait for the current job to finish (or cancel it)".into());
        return;
    }
    match a {
        Action::NewSphere => new_mesh(ui, Mesh::icosphere(ui.sphere_detail.get_untracked()), "sphere"),
        Action::NewCube => new_mesh(ui, Mesh::cube(6), "cube"),
        Action::NewPlane => new_mesh(ui, Mesh::plane(7), "plane"),
        Action::Import => spawn_local(async move {
            match host::open_mesh().await {
                Ok(Some(f)) => match io::load(&f.name, &f.bytes) {
                    Ok(m) => new_mesh(ui, m, &f.name),
                    Err(e) => ui.status.set(format!("Import failed: {e}")),
                },
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Import failed: {e}")),
            }
        }),
        Action::OpenProject => spawn_local(open_project(ui)),
        Action::LoadMatcap => spawn_local(async move {
            match host::open_image().await {
                Ok(Some(f)) => match use_matcap(&f.bytes) {
                    Ok(()) => {
                        store_matcap(&f.bytes);
                        ui.view.update(|v| v.material = CUSTOM_MATCAP);
                        ui.status.set(format!("Matcap: {}", f.name));
                    }
                    Err(e) => ui.status.set(format!("Couldn't use that image: {e}")),
                },
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't open the image: {e}")),
            }
        }),
        Action::SaveProject => spawn_local(save_project(ui, false)),
        Action::SaveProjectAs => spawn_local(save_project(ui, true)),
        Action::ExportObj => spawn_local(output_ui::export_mesh(ui, "obj")),
        Action::ExportStl => spawn_local(output_ui::export_mesh(ui, "stl")),
        Action::Export(ext) => spawn_local(output_ui::export_mesh(ui, ext)),
        Action::RenderImage => spawn_local(output_ui::render_still(ui, ui.render.get_untracked())),
        Action::RenderTurntable => spawn_local(output_ui::render_turntable(ui, ui.render.get_untracked())),
        Action::Undo => with_ed(|e| {
            if !e.retopo_undo() && !e.zs_undo() {
                e.undo()
            }
        }),
        Action::NewPrimitive(p) => {
            with_ed(|e| e.new_primitive(p));
            ui.status.set(format!("{} — quads with one polygroup per side; try ZModeler mode", p.name()));
        }
        Action::NewMannequin => {
            with_ed(|e| {
                e.mannequin = clayform_core::mannequin::Mannequin::default();
                e.mannequin.bulk = ui.bulk.get_untracked();
                e.mannequin_changed();
            });
            ui.mode.set(Mode::Mannequin);
            ui.status.set("Mannequin — drag the joints to pose it, then Convert to sculpt".into());
        }
        Action::Redo => with_ed(|e| e.redo()),
        Action::Subdivide => {
            if host::in_desktop_app() {
                // subdivide on a background thread in the desktop backend
                let bytes = match with_ed(|e| e.begin_subdivide().map(|_| wire::encode(&e.sculptor.mesh, &[]))) {
                    Ok(b) => b,
                    Err(e) => {
                        ui.status.set(e);
                        return;
                    }
                };
                start_job(ui, "Subdividing");
                spawn_local(async move {
                    let result = host::invoke_bytes("subdivide", &bytes, &[]).await.and_then(|b| wire::decode_parts(&b));
                    let result = match result {
                        Ok(parts) => Ok(build_in_slices(ui, parts).await),
                        Err(e) => Err(e),
                    };
                    match result {
                        Ok((fine, odd, bvh)) => match with_ed(|e| e.finish_subdivide(fine, odd, Some(bvh))) {
                            Ok(()) => {
                                let (lv, n) = with_ed(|e| (e.sculptor.level_count(), e.sculptor.tri_count()));
                                ui.status.set(format!("Added level {lv} — {} triangles", fmt_count(n)));
                            }
                            Err(e) => ui.status.set(e),
                        },
                        Err(e) => {
                            with_ed(|ed| ed.abort_subdivide());
                            ui.status.set(job_error("Subdivide", &e));
                        }
                    }
                    end_job(ui);
                });
            } else {
                match with_ed(|e| e.subdivide()) {
                    Ok(()) => {
                        let (lv, n) = with_ed(|e| (e.sculptor.level_count(), e.sculptor.tri_count()));
                        ui.status.set(format!("Added level {lv} — {} triangles", fmt_count(n)));
                    }
                    Err(e) => ui.status.set(e),
                }
            }
        }
        Action::LevelUp => with_ed(|e| e.set_level(e.sculptor.level() + 1)),
        Action::LevelDown => with_ed(|e| e.set_level(e.sculptor.level().saturating_sub(1))),
        Action::DeleteLower => {
            with_ed(|e| e.delete_lower());
            ui.status.set("Deleted the lower levels; this level is now the base".into());
        }
        Action::DeleteHigher => {
            with_ed(|e| e.delete_higher());
            ui.status.set("Deleted the higher levels".into());
        }
        Action::VoxelRemesh => {
            if ui.busy.get_untracked() {
                return;
            }
            run_remesh(ui, None, ui.voxel.get_untracked(), "Remeshed");
            return;
        }
        Action::ToggleDyntopo => {
            if ui.level_count.get_untracked() > 1 {
                ui.status.set("Dynamic topology needs a single level: use Delete lower / Delete higher first".into());
            } else {
                ui.dyntopo.update(|d| *d = !*d);
            }
        }
        Action::ClearMask => with_ed(|e| {
            e.sculptor.clear_mask();
            e.redraw = true;
        }),
        Action::InvertMask => with_ed(|e| {
            e.sculptor.invert_mask();
            e.redraw = true;
        }),
        Action::FrameMesh => with_ed(|e| e.frame_all()),
        Action::ResetCamera => with_ed(|e| e.reset_camera()),
        Action::View(d) => with_ed(|e| e.view_from(d)),
        Action::TogglePanelSide => ui.panel_right.update(|r| *r = !*r),
    }
    ui.refresh();
}

/// Voxel-remeshes `mesh` (or the sculpt) in the background and makes the
/// result the sculpt. `done` starts the status message.
fn run_remesh(ui: Ui, mesh: Option<Mesh>, settings: VoxelSettings, done: &'static str) {
    if host::in_desktop_app() {
        let bytes = with_ed(|e| {
            e.cancel_drag();
            wire::encode(mesh.as_ref().unwrap_or(&e.sculptor.mesh), &[])
        });
        start_job(ui, "Remeshing");
        spawn_local(async move {
            let t0 = now_ms();
            let headers = [
                ("resolution", settings.resolution.to_string()),
                ("smooth", settings.smooth.to_string()),
                ("project", if settings.project { "1" } else { "0" }.to_string()),
                ("signed", if settings.signed { "1" } else { "0" }.to_string()),
            ];
            let result = host::invoke_bytes("remesh", &bytes, &headers).await.and_then(|b| wire::decode_parts(&b));
            let result = match result {
                Ok(parts) => Ok(build_in_slices(ui, parts).await),
                Err(e) => Err(e),
            };
            match result {
                Ok((m, _, bvh)) => {
                    let n = m.tris.len();
                    with_ed(|e| e.apply_remeshed(m, Some(bvh)));
                    ui.status.set(format!(
                        "{done} — {} triangles in {:.1} s",
                        fmt_count(n),
                        (now_ms() - t0) / 1000.0
                    ));
                }
                Err(e) => ui.status.set(job_error("Remesh", &e)),
            }
            end_job(ui);
        });
        return;
    }
    ui.busy.set(true);
    ui.status.set(format!("Remeshing at resolution {}…", settings.resolution));
    spawn_local(async move {
        // let the status line paint before the (blocking) remesh
        sleep_ms(40).await;
        let t0 = now_ms();
        let result = match &mesh {
            Some(m) => clayform_core::remesh::voxel_remesh(m, &settings).map(|r| {
                let n = r.tris.len();
                with_ed(|e| e.apply_remeshed(r, None));
                n
            }),
            None => with_ed(|e| e.voxel_remesh(&settings)),
        };
        match result {
            Ok(n) => ui.status.set(format!(
                "{done} — {} triangles in {:.1} s",
                fmt_count(n),
                (now_ms() - t0) / 1000.0
            )),
            Err(e) => ui.status.set(format!("Remesh failed: {e}")),
        }
        ui.busy.set(false);
        ui.refresh();
    });
}

/// Builds a mesh's connectivity and picking hierarchy a slice at a time,
/// yielding between slices so the window keeps drawing and responding.
async fn build_in_slices(ui: Ui, parts: wire::Parts) -> (Mesh, Vec<[u32; 4]>, Bvh) {
    let (mut b, stencils) = parts.into_builder();
    ui.job.set(Some(("Finishing", 0.0)));
    loop {
        let t0 = now_ms();
        let done = b.step(&mut || now_ms() - t0 < 12.0);
        ui.job.set(Some(("Finishing", b.progress() * 0.7)));
        if done {
            break;
        }
        sleep_ms(0).await;
    }
    let mesh = b.finish();
    let mut bb = BvhBuilder::new(&mesh);
    loop {
        let t0 = now_ms();
        let done = bb.step(&mesh, &mut || now_ms() - t0 < 12.0);
        ui.job.set(Some(("Finishing", 0.7 + bb.progress() * 0.3)));
        if done {
            break;
        }
        sleep_ms(0).await;
    }
    let bvh = bb.finish(&mesh);
    (mesh, stencils, bvh)
}

/// Interface settings stored in a project file.
fn settings_json(ui: Ui) -> String {
    let (cam, brush) = with_ed(|e| (e.camera.clone_state(), e.brush));
    let v = ui.view.get_untracked();
    let vx = ui.voxel.get_untracked();
    serde_json::json!({
        "camera": { "target": cam.0, "yaw": cam.1, "pitch": cam.2, "dist": cam.3, "fov": ui.fov.get_untracked() },
        "brush": {
            "tool": brush.tool.index(),
            "radius": brush.radius_px,
            "intensity": brush.intensity.to_vec(),
            "negative": brush.negative,
            "mirror": brush.symmetry.mirror,
            "radial": brush.symmetry.radial,
            "radial_axis": brush.symmetry.radial_axis,
            "local": brush.symmetry.local,
            "dyntopo": brush.dyntopo,
            "detail": brush.detail,
            "constant_detail": brush.detail_mode == DetailMode::Constant,
            "smooth_reduces": brush.smooth_reduces,
            "anchor_mode": AnchorMode::ALL.iter().position(|m| *m == brush.anchor_mode),
            "tip": brush_ui::tip_json(ui),
        },
        "zmodeler": {
            "action": clayform_core::poly::Action::ALL.iter().position(|a| *a == ui.zm.get_untracked().action),
            "target": clayform_core::poly::Target::ALL.iter().position(|t| *t == ui.zm.get_untracked().target),
            "divisions": ui.zm.get_untracked().divisions,
        },
        "references": tools_ui::refs_json(ui),
        "lighting": output_ui::lighting_json(ui),
        "render": output_ui::render_json(ui),
        "view": {
            "material": v.material, "show_mask": v.show_mask, "flat": v.flat, "wireframe": v.wireframe,
            "background": v.background, "ao": v.ao, "ao_strength": v.ao_strength,
            "cavity": v.cavity, "cavity_strength": v.cavity_strength, "polygroups": v.polygroups,
        },
        "voxel": { "resolution": vx.resolution, "smooth": vx.smooth, "project": vx.project },
        "zspheres": special_ui::zs_json(),
    })
    .to_string()
}

fn apply_settings(ui: Ui, json: &str) {
    let Ok(j) = serde_json::from_str::<serde_json::Value>(json) else { return };
    let f = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let b = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_bool());
    if let Some(c) = j.get("camera") {
        let t = c.get("target").and_then(|t| t.as_array()).map(|a| {
            let g = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            clayform_core::glam::Vec3::new(g(0), g(1), g(2))
        });
        with_ed(|e| {
            if let Some(t) = t {
                e.camera.target = t;
            }
            if let Some(y) = f(c, "yaw") {
                e.camera.yaw = y;
            }
            if let Some(p) = f(c, "pitch") {
                e.camera.pitch = p;
            }
            if let Some(d) = f(c, "dist") {
                e.camera.dist = d;
            }
            e.redraw = true;
        });
        if let Some(fov) = f(c, "fov") {
            ui.fov.set(fov);
        }
    }
    if let Some(br) = j.get("brush") {
        if let Some(i) = br.get("tool").and_then(|x| x.as_u64()) {
            if let Some(t) = Tool::ALL.get(i as usize) {
                ui.tool.set(*t);
            }
        }
        if let Some(r) = f(br, "radius") {
            ui.radius.set(r);
        }
        if let Some(a) = br.get("intensity").and_then(|x| x.as_array()) {
            ui.intensity.update(|arr| {
                for (k, v) in a.iter().enumerate().take(arr.len()) {
                    if let Some(v) = v.as_f64() {
                        arr[k] = v as f32;
                    }
                }
            });
        }
        if let Some(n) = b(br, "negative") {
            ui.negative.set(n);
        }
        ui.symmetry.update(|s| {
            if let Some(m) = br.get("mirror").and_then(|x| x.as_array()) {
                for (k, v) in m.iter().enumerate().take(3) {
                    s.mirror[k] = v.as_bool().unwrap_or(false);
                }
            }
            if let Some(r) = br.get("radial").and_then(|x| x.as_u64()) {
                s.radial = (r as u32).clamp(1, MAX_RADIAL);
            }
            if let Some(a) = br.get("radial_axis").and_then(|x| x.as_u64()) {
                s.radial_axis = (a as usize).min(2);
            }
            if let Some(l) = b(br, "local") {
                s.local = l;
            }
        });
        if let Some(d) = b(br, "dyntopo") {
            ui.dyntopo.set(d);
        }
        if let Some(d) = f(br, "detail") {
            ui.detail.set(d);
        }
        if let Some(c) = b(br, "constant_detail") {
            ui.detail_mode.set(if c { DetailMode::Constant } else { DetailMode::Brush });
        }
        if let Some(r) = b(br, "smooth_reduces") {
            ui.smooth_reduces.set(r);
        }
        if let Some(m) = br.get("anchor_mode").and_then(|x| x.as_u64()).and_then(|i| AnchorMode::ALL.get(i as usize)) {
            ui.anchor_mode.set(*m);
        }
        if let Some(t) = br.get("tip") {
            brush_ui::apply_tip_json(ui, t);
        }
    }
    if let Some(z) = j.get("zmodeler") {
        ui.zm.update(|s| {
            if let Some(a) = z.get("action").and_then(|x| x.as_u64()).and_then(|i| clayform_core::poly::Action::ALL.get(i as usize)) {
                s.action = *a;
            }
            if let Some(t) = z.get("target").and_then(|x| x.as_u64()).and_then(|i| clayform_core::poly::Target::ALL.get(i as usize)) {
                s.target = *t;
            }
            if let Some(d) = z.get("divisions").and_then(|x| x.as_u64()) {
                s.divisions = (d as u32).clamp(1, 32);
            }
        });
    }
    if let Some(z) = j.get("zspheres") {
        special_ui::apply_zs_json(z);
    }
    if let Some(r) = j.get("references") {
        tools_ui::apply_refs_json(ui, r);
    }
    if let Some(l) = j.get("lighting") {
        output_ui::apply_lighting_json(ui, l);
    }
    if let Some(r) = j.get("render") {
        output_ui::apply_render_json(ui, r);
    }
    if let Some(v) = j.get("view") {
        ui.view.update(|s| {
            if let Some(m) = v.get("material").and_then(|x| x.as_u64()) {
                s.material = m as u32;
            }
            s.show_mask = b(v, "show_mask").unwrap_or(s.show_mask);
            s.flat = b(v, "flat").unwrap_or(s.flat);
            s.wireframe = b(v, "wireframe").unwrap_or(s.wireframe);
            if let Some(bg) = v.get("background").and_then(|x| x.as_u64()) {
                s.background = bg as u32;
            }
            s.ao = b(v, "ao").unwrap_or(s.ao);
            s.ao_strength = f(v, "ao_strength").unwrap_or(s.ao_strength);
            s.cavity = b(v, "cavity").unwrap_or(s.cavity);
            s.cavity_strength = f(v, "cavity_strength").unwrap_or(s.cavity_strength);
            s.polygroups = b(v, "polygroups").unwrap_or(s.polygroups);
        });
    }
    if let Some(v) = j.get("voxel") {
        ui.voxel.update(|s| {
            if let Some(r) = v.get("resolution").and_then(|x| x.as_u64()) {
                s.resolution = r as u32;
            }
            if let Some(m) = v.get("smooth").and_then(|x| x.as_u64()) {
                s.smooth = m as u32;
            }
            s.project = b(v, "project").unwrap_or(s.project);
        });
    }
}

async fn save_project(ui: Ui, ask: bool) {
    let path = if ask { None } else { ui.project_path.get_untracked() };
    with_ed(|e| e.cancel_drag());
    // (settings_json reads the editor itself, so build it first)
    let settings = settings_json(ui);
    let bytes = scene_ui::encode_project(&settings);
    let name = {
        let n = ui.project_name.get_untracked();
        if n.ends_with(".clayform") { n } else { format!("{n}.clayform") }
    };
    let rev = ui.rev.get_untracked();
    let mb = bytes.len() as f64 / 1e6;
    match host::save_project(bytes, path.as_deref(), &name).await {
        Ok(Some(p)) => {
            let file = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
            if host::in_desktop_app() {
                ui.project_path.set(Some(p.clone()));
            }
            ui.project_name.set(file.trim_end_matches(".clayform").to_string());
            ui.saved_rev.set(rev);
            conveniences::saved(ui, host::in_desktop_app().then_some(p.as_str()));
            ui.status.set(format!("Saved {p} ({mb:.1} MB)"));
        }
        Ok(None) => {}
        Err(e) => ui.status.set(format!("Save failed: {e}")),
    }
}

async fn open_project(ui: Ui) {
    let opened = match host::open_project().await {
        Ok(Some(o)) => o,
        Ok(None) => return,
        Err(e) => {
            ui.status.set(format!("Open failed: {e}"));
            return;
        }
    };
    conveniences::open_project_bytes(ui, opened.name, opened.path, opened.bytes).await;
}

const MATCAP_KEY: &str = "clayform.matcap";

/// Decodes a PNG/JPEG and installs it as the custom matcap.
fn use_matcap(bytes: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width().max(img.height()) > 1024 {
        img.resize(1024, 1024, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    RENDERER.with(|r| match r.borrow_mut().as_mut() {
        Some(r) => {
            r.set_matcap(w, h, rgba.as_raw());
            Ok(())
        }
        None => Err("the 3D view isn't ready".to_string()),
    })?;
    with_ed(|e| e.redraw = true);
    Ok(())
}

/// Remembers the matcap between sessions (if it's small enough).
fn store_matcap(bytes: &[u8]) {
    use base64::Engine as _;
    if bytes.len() < 3_000_000 {
        if let Some(s) = storage() {
            let _ = s.set_item(MATCAP_KEY, &base64::engine::general_purpose::STANDARD.encode(bytes));
        }
    }
}

fn restore_matcap() {
    use base64::Engine as _;
    let Some(data) = storage().and_then(|s| s.get_item(MATCAP_KEY).ok().flatten()) else { return };
    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
        let _ = use_matcap(&bytes);
    }
}

fn start_job(ui: Ui, label: &'static str) {
    ui.busy.set(true);
    ui.job.set(Some((label, 0.0)));
    ui.status.set(format!("{label}…"));
}

fn end_job(ui: Ui) {
    ui.busy.set(false);
    ui.job.set(None);
    ui.refresh();
}

fn job_error(what: &str, e: &str) -> String {
    if e.contains(CANCELLED) { format!("{what} cancelled") } else { format!("{what} failed: {e}") }
}

async fn sleep_ms(ms: i32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

fn now_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}

fn new_mesh(ui: Ui, m: Mesh, name: &str) {
    let tris = m.tris.len();
    with_ed(|e| {
        e.sculptor.set_label(format!("New {name}"));
        e.set_mesh(m)
    });
    ui.status.set(format!("{name} — {} triangles", fmt_count(tris)));
    ui.refresh();
}

// ------------------------------------------------------------------- frame loop

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

fn start_frame_loop(canvas: web_sys::HtmlCanvasElement, ui: Ui) {
    let cell: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let next = cell.clone();
    *cell.borrow_mut() = Some(Closure::new(move || {
        draw_frame(&canvas, ui);
        if let Some(f) = next.borrow().as_ref() {
            request_frame(f);
        }
    }));
    let first = cell.borrow();
    if let Some(f) = first.as_ref() {
        request_frame(f);
    }
}

fn draw_frame(canvas: &web_sys::HtmlCanvasElement, ui: Ui) {
    // a running Dynamics simulation advances every frame
    with_ed(|e| e.dyn_tick());
    if let Some(e) = render::GPU_ERROR.lock().ok().and_then(|mut g| g.take()) {
        log(&format!("GPU error: {e}"));
        ui.gpu.set(Some(e));
    }
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let (cw, ch) = (canvas.client_width() as f64, canvas.client_height() as f64);
    if cw < 1.0 || ch < 1.0 {
        return;
    }
    let (w, h) = ((cw * dpr).round() as u32, (ch * dpr).round() as u32);
    let mut resized = false;
    if canvas.width() != w || canvas.height() != h {
        canvas.set_width(w);
        canvas.set_height(h);
        resized = true;
    }
    let view = ui.view.get_untracked();
    with_ed(|e| {
        if !(e.redraw || resized) {
            return;
        }
        e.redraw = false;
        RENDERER.with(|r| {
            if let Some(r) = r.borrow_mut().as_mut() {
                sync_scene(r, e, &view);
                r.render([w, h], e.camera.view(), e.camera.proj((cw / ch) as f32), &view, e.camera.dist * 0.06);
            }
        });
    });
}

/// Sends every SubTool (and the live boolean preview) to the renderer.
pub(crate) fn sync_scene(r: &mut Renderer, e: &mut Editor, view: &ViewSettings) {
    r.begin_sync();
    let dragging = e.drag() != Drag::None;
    let preview = scene_ui::live_preview_active(dragging);
    for i in 0..e.subtool_count() {
        let info = e.subtool_info(i).clone();
        let xfs = if info.array.is_some() { e.array_transforms(i) } else { Vec::new() };
        let active = i == e.active_subtool();
        let ov = render::ObjectView {
            id: info.id,
            // in ZSpheres mode the spheres' skin stands in for the active SubTool
            visible: info.visible && !preview && !(active && e.mode == Mode::ZSpheres && !e.zspheres.is_empty()),
            dim: !active && e.subtool_count() > 1,
            instances: &xfs,
            paint: view.paint,
        };
        let s = e.sculptor_at_mut(i);
        r.sync_object(&ov, &s.mesh, &mut s.dirty, view.wireframe, view.polygroups);
    }
    scene_ui::sync_preview(r, view, dragging);
    if e.mode == Mode::ZSpheres {
        if let Some((m, dirty)) = e.zs_preview() {
            let ov = render::ObjectView { id: u64::MAX - 1, visible: true, dim: false, instances: &[], paint: false };
            r.sync_object(&ov, m, dirty, false, false);
        }
    }
    r.end_sync();
}

// ----------------------------------------------------------------- viewport

/// Pressure of a pen (1 for a mouse or touch, which report no real pressure).
fn pen_pressure(ui: Ui, ev: &ev::PointerEvent) -> f32 {
    if ev.pointer_type() == "pen" {
        let p = ev.pressure().clamp(0.0, 1.0);
        if ui.pressure.get_untracked().0 == false || (ui.pressure.get_untracked().1 - p).abs() > 0.05 {
            ui.pressure.set((true, p));
        }
        p
    } else {
        1.0
    }
}

fn local_pos(ev: &web_sys::MouseEvent) -> Vec2 {
    Vec2::new(ev.offset_x() as f32, ev.offset_y() as f32)
}

fn canvas_size(ev: &web_sys::Event) -> Vec2 {
    ev.current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el| Vec2::new(el.client_width() as f32, el.client_height() as f32))
        .unwrap_or(Vec2::ONE)
}

#[component]
fn Viewport(ui: Ui) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let started = StoredValue::new(false);
    Effect::new(move || {
        let Some(canvas) = canvas_ref.get() else { return };
        if started.get_value() {
            return;
        }
        started.set_value(true);
        let canvas: web_sys::HtmlCanvasElement = canvas;
        spawn_local(async move {
            match Renderer::new(canvas.clone()).await {
                Ok(r) => {
                    log(&format!("Clayform renderer: {}", r.adapter_name));
                    if let Some(e) = &r.ao_error {
                        ui.status.set(format!("Ambient occlusion isn't available on this GPU: {e}"));
                    }
                    RENDERER.with(|slot| *slot.borrow_mut() = Some(r));
                    restore_matcap();
                    with_ed(|e| e.redraw = true);
                    start_frame_loop(canvas, ui);
                }
                Err(e) => ui.gpu.set(Some(e)),
            }
        });
    });

    let on_down = move |ev: ev::PointerEvent| {
        ev.prevent_default();
        conveniences::touch();
        ui.menu.set(None);
        if ev.button() == 0 && paint_ui::try_pick(ui, local_pos(&ev), canvas_size(&ev)) {
            return;
        }
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        let button = match ev.button() {
            1 => Button::Middle,
            2 => Button::Right,
            _ => Button::Left,
        };
        // while a background job runs, the left button only orbits
        let busy = ui.busy.get_untracked();
        let mods = Mods { shift: ev.shift_key(), ctrl: ev.ctrl_key() || ev.meta_key(), alt: ev.alt_key() || busy };
        let size = canvas_size(&ev);
        let pr = pen_pressure(ui, &ev);
        let d = with_ed(|e| {
            e.pressure = pr;
            e.pointer_down(button, local_pos(&ev), size, mods)
        });
        ui.dragging.set(d);
        if d != Drag::Orbit && d != Drag::Pan {
            ui.refresh();
        }
    };
    let on_move = move |ev: ev::PointerEvent| {
        conveniences::touch();
        let p = local_pos(&ev);
        let size = canvas_size(&ev);
        ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key()));
        let pr = pen_pressure(ui, &ev);
        let mirror = with_ed(|e| {
            e.pressure = pr;
            if e.drag() != Drag::None {
                e.pointer_move(p, size);
            }
            e.mirror_screen_points(p, size)
        });
        ui.cursor.set(Some((p.x, p.y)));
        ui.vp_size.set((size.x, size.y));
        ui.mirror.set(mirror.into_iter().map(|m| (m.x, m.y)).collect());
        ui.update_overlay();
    };
    let on_up = move |_ev: ev::PointerEvent| {
        with_ed(|e| e.pointer_up());
        ui.dragging.set(Drag::None);
        ui.refresh();
    };
    let on_wheel = move |ev: ev::WheelEvent| {
        ev.prevent_default();
        let scale = if ev.delta_mode() == 1 { 30.0 } else { 1.0 };
        with_ed(|e| e.wheel(-(ev.delta_y() as f32) * scale));
        ui.update_overlay();
    };

    let brush_visible = move || {
        matches!(ui.dragging.get(), Drag::None | Drag::Sculpt) && ui.cursor.get().is_some() && ui.overlay.get().brush
    };
    let brush_color = move || {
        let (shift, ctrl) = ui.mods.get();
        let _ = (ui.tool.get(), ui.negative.get(), ui.dragging.get());
        let neg = with_ed(|e| e.effective_negative(Mods { shift, ctrl, alt: false }));
        if neg { "#6eaaff" } else { "#ff965a" }
    };

    view! {
        <div class="viewport">
            <canvas
                node_ref=canvas_ref
                on:pointerdown=on_down
                on:pointermove=on_move
                on:pointerup=on_up
                on:pointercancel=on_up
                on:pointerleave=move |_| if ui.dragging.get_untracked() == Drag::None {
                    ui.cursor.set(None);
                    ui.mirror.set(Vec::new());
                    ui.update_overlay();
                }
                on:wheel=on_wheel
                on:contextmenu=|ev: ev::MouseEvent| ev.prevent_default()
            ></canvas>
            <svg class="overlay" style:display=move || if brush_visible() { "block" } else { "none" }>
                <circle
                    cx=move || ui.cursor.get().map(|c| c.0).unwrap_or(-999.0)
                    cy=move || ui.cursor.get().map(|c| c.1).unwrap_or(-999.0)
                    r=move || ui.radius.get()
                    fill="none" stroke=brush_color stroke-width="1.5"
                ></circle>
                <circle
                    cx=move || ui.cursor.get().map(|c| c.0).unwrap_or(-999.0)
                    cy=move || ui.cursor.get().map(|c| c.1).unwrap_or(-999.0)
                    r="1.5" fill=brush_color
                ></circle>
                {move || {
                    let r = ui.radius.get();
                    ui.mirror
                        .get()
                        .into_iter()
                        .map(|(x, y)| {
                            view! {
                                <circle cx=x cy=y r=r fill="none" stroke=brush_color stroke-width="1" stroke-opacity="0.45"></circle>
                            }
                        })
                        .collect_view()
                }}
            </svg>
            <tools_ui::ModeOverlay ui />
            <conveniences::RecoveryBanner ui />
            <tools_ui::References ui />
            <Show when=move || ui.job.get().is_some()>
                <div class="job-card">
                    <div class="job-row">
                        <span class="job-label">{move || ui.job.get().map(|j| j.0).unwrap_or("")}"…"</span>
                        <span class="job-pct">{move || format!("{:.0}%", ui.job.get().map(|j| j.1).unwrap_or(0.0) * 100.0)}</span>
                        <button class="job-cancel" on:click=move |_| {
                            ui.status.set("Cancelling…".into());
                            output_ui::cancel_local_job();
                            spawn_local(host::cancel_job());
                        }>"Cancel"</button>
                    </div>
                    <div class="job-bar">
                        <div class="job-fill" style:width=move || format!("{:.1}%", ui.job.get().map(|j| j.1).unwrap_or(0.0) * 100.0)></div>
                    </div>
                    <div class="job-hint">"You can keep rotating the view while this runs."</div>
                </div>
            </Show>
            <Show when=move || ui.gpu.get().is_some()>
                <div class="gpu-error">
                    <b>"The 3D view could not start"</b>
                    <p>{move || ui.gpu.get().unwrap_or_default()}</p>
                </div>
            </Show>
        </div>
    }
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

// ------------------------------------------------------------------ widgets

/// A top-bar dropdown menu (SculptGL-style).
#[component]
fn Menu(ui: Ui, id: &'static str, label: &'static str, children: Children) -> impl IntoView {
    let open = move || ui.menu.get() == Some(id);
    view! {
        <div class="menu" class:open=open>
            <button
                class="menu-title"
                on:click=move |_| ui.menu.update(|m| *m = if *m == Some(id) { None } else { Some(id) })
                on:pointerenter=move |_| if ui.menu.get_untracked().is_some() { ui.menu.set(Some(id)) }
            >{label}</button>
            <div class="dropdown">{children()}</div>
        </div>
    }
}

#[component]
fn Item(
    ui: Ui,
    action: Action,
    label: &'static str,
    #[prop(optional)] key: &'static str,
    #[prop(optional, into)] enabled: Option<Signal<bool>>,
) -> impl IntoView {
    let disabled = move || enabled.map(|e| !e.get()).unwrap_or(false);
    view! {
        <button class="item" disabled=disabled on:click=move |_| act(ui, action)>
            <span>{label}</span>
            <span class="key">{key}</span>
        </button>
    }
}

/// A collapsible folder in the side panel (SculptGL-style).
#[component]
fn Folder(title: &'static str, #[prop(optional)] closed: bool, children: Children) -> impl IntoView {
    let open = RwSignal::new(!closed);
    view! {
        <section class="folder" class:collapsed=move || !open.get()>
            <button class="folder-title" on:click=move |_| open.update(|o| *o = !*o)>
                <span class="chev">{move || if open.get() { "▾" } else { "▸" }}</span>
                {title}
            </button>
            <div class="folder-body">{children()}</div>
        </section>
    }
}

#[component]
fn Slider(
    label: &'static str,
    min: f32,
    max: f32,
    step: f32,
    #[prop(into)] value: Signal<f32>,
    on_change: impl Fn(f32) + 'static,
    #[prop(optional)] format: Option<fn(f32) -> String>,
) -> impl IntoView {
    let fmt = format.unwrap_or(|v| format!("{v:.2}"));
    view! {
        <label class="row slider">
            <span class="label">{label}</span>
            <input
                type="range"
                min=min.to_string()
                max=max.to_string()
                step=step.to_string()
                prop:value=move || value.get().to_string()
                on:input=move |ev| {
                    if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                        on_change(v)
                    }
                }
            />
            <span class="value">{move || fmt(value.get())}</span>
        </label>
    }
}

#[component]
fn Check(
    label: &'static str,
    #[prop(into)] value: Signal<bool>,
    on_change: impl Fn(bool) + 'static,
    #[prop(optional, into)] disabled: Option<Signal<bool>>,
) -> impl IntoView {
    let off = move || disabled.map(|d| d.get()).unwrap_or(false);
    view! {
        <label class="row check" class:disabled=off>
            <input
                type="checkbox"
                disabled=off
                prop:checked=move || value.get()
                on:change=move |ev| on_change(event_target_checked(&ev))
            />
            <span>{label}</span>
        </label>
    }
}

// ----------------------------------------------------------------------- app

#[component]
fn MenuBar(ui: Ui) -> impl IntoView {
    let can_undo = Signal::derive(move || ui.can_undo.get());
    let can_redo = Signal::derive(move || ui.can_redo.get());
    view! {
        <header class="menubar">
            <span class="brand">"Clayform"</span>
            <span class="doc-name" title=move || ui.project_path.get().unwrap_or_else(|| "Not saved yet".into())>
                {move || ui.project_name.get()}
                <span class="dirty" title="Unsaved changes">{move || if ui.rev.get() != ui.saved_rev.get() { " •" } else { "" }}</span>
            </span>
            <Menu ui id="file" label="File">
                <Item ui action=Action::OpenProject label="Open project…" key="Ctrl+O" />
                <Show when=move || !ui.recent.get().is_empty()>
                    <div class="menu-sub">"Open recent"</div>
                    {move || ui
                        .recent
                        .get()
                        .into_iter()
                        .map(|p| {
                            let name = conveniences::file_name(&p);
                            let path = p.clone();
                            view! {
                                <button class="item recent" title=p.clone() on:click=move |_| {
                                    ui.menu.set(None);
                                    spawn_local(conveniences::open_recent(ui, path.clone()));
                                }>
                                    <span>{name}</span>
                                </button>
                            }
                        })
                        .collect_view()}
                    <button class="item recent-clear" on:click=move |_| {
                        conveniences::clear_recent(ui);
                        ui.menu.set(None);
                    }><span>"Clear recent"</span></button>
                </Show>
                <Item ui action=Action::SaveProject label="Save project" key="Ctrl+S" />
                <Item ui action=Action::SaveProjectAs label="Save project as…" key="Ctrl+Shift+S" />
                <div class="sep"></div>
                <Item ui action=Action::Import label="Import OBJ / STL…" key="Ctrl+I" />
                <Item ui action=Action::ExportObj label="Export OBJ…" key="Ctrl+E" />
                <Item ui action=Action::ExportStl label="Export STL…" />
                <Item ui action=Action::Export("ply") label="Export PLY…" />
                <Item ui action=Action::Export("glb") label="Export glTF (GLB)…" />
                <Item ui action=Action::Export("fbx") label="Export FBX…" />
            </Menu>
            <Menu ui id="edit" label="Edit">
                <Item ui action=Action::Undo label="Undo" key="Ctrl+Z" enabled=can_undo />
                <Item ui action=Action::Redo label="Redo" key="Ctrl+Shift+Z" enabled=can_redo />
                <div class="sep"></div>
                <Item ui action=Action::Subdivide label="Subdivide (add level)" key="Ctrl+D" />
                <Item ui action=Action::VoxelRemesh label="Voxel remesh" key="R" />
            </Menu>
            <Menu ui id="scene" label="Scene">
                <Item ui action=Action::NewSphere label="New sphere" />
                <Item ui action=Action::NewCube label="New cube" />
                <Item ui action=Action::NewPlane label="New plane" />
                <div class="sep"></div>
                <Item ui action=Action::NewPrimitive(clayform_core::poly::Primitive::QCube) label="New QCube (quads)" />
                <Item ui action=Action::NewPrimitive(clayform_core::poly::Primitive::QSphere) label="New QSphere (quads)" />
                <Item ui action=Action::NewPrimitive(clayform_core::poly::Primitive::QCylinder) label="New QCylinder (quads)" />
                <Item ui action=Action::NewPrimitive(clayform_core::poly::Primitive::QPlane) label="New QPlane (quads)" />
                <Item ui action=Action::NewMannequin label="New mannequin" />
                <div class="sep"></div>
                <div class="menu-widget">
                    <Slider
                        label="Sphere detail"
                        min=3.0 max=8.0 step=1.0
                        value=Signal::derive(move || ui.sphere_detail.get() as f32)
                        on_change=move |v| ui.sphere_detail.set(v as u32)
                        format=|v| format!("{v:.0}")
                    />
                </div>
            </Menu>
            <Menu ui id="camera" label="Camera">
                <Item ui action=Action::FrameMesh label="Frame mesh" key="Home" />
                <Item ui action=Action::ResetCamera label="Reset camera" />
                <div class="sep"></div>
                <Item ui action=Action::View(ViewDir::Front) label="Front" key="1" />
                <Item ui action=Action::View(ViewDir::Back) label="Back" key="Ctrl+1" />
                <Item ui action=Action::View(ViewDir::Right) label="Right" key="3" />
                <Item ui action=Action::View(ViewDir::Left) label="Left" key="Ctrl+3" />
                <Item ui action=Action::View(ViewDir::Top) label="Top" key="7" />
                <Item ui action=Action::View(ViewDir::Bottom) label="Bottom" key="Ctrl+7" />
                <div class="sep"></div>
                <div class="menu-widget">
                    <Slider
                        label="Field of view"
                        min=15.0 max=90.0 step=1.0
                        value=ui.fov
                        on_change=move |v| ui.fov.set(v)
                        format=|v| format!("{v:.0}°")
                    />
                </div>
            </Menu>
            <Menu ui id="render" label="Render">
                <Item ui action=Action::RenderImage label="Render image…" key="F12" />
                <Item ui action=Action::RenderTurntable label="Render turntable…" />
                <div class="sep"></div>
                <div class="menu-note">"Size, depth of field, turntable length and format are in the Render folder."</div>
            </Menu>
            <Menu ui id="background" label="Background">
                {BACKGROUNDS
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _, _))| {
                        let i = i as u32;
                        view! {
                            <button
                                class="item radio"
                                class:checked=move || ui.view.get().background == i
                                on:click=move |_| {
                                    ui.view.update(|v| v.background = i);
                                    ui.menu.set(None);
                                }
                            >
                                <span>{*name}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </Menu>
            <Menu ui id="help" label="Help">
                <div class="help">
                    <table>
                        <tr><td>"Left drag on mesh"</td><td>"Sculpt (or the mode's action)"</td></tr>
                        <tr><td>"Shift + drag / Ctrl + drag"</td><td>"Smooth / invert the brush"</td></tr>
                        <tr><td>"Left drag off mesh, Alt + left, right drag"</td><td>"Rotate"</td></tr>
                        <tr><td>"Middle drag, Shift + right drag"</td><td>"Pan"</td></tr>
                        <tr><td>"Wheel"</td><td>"Zoom"</td></tr>
                        {move || {
                            let map = ui.keymap.get();
                            [
                                ("undo", "Undo"), ("redo", "Redo"), ("open", "Open project"), ("save", "Save project"),
                                ("render", "Render image"), ("subdivide", "Subdivide"), ("remesh", "Voxel remesh"),
                                ("dyntopo", "Dynamic topology"), ("radius_down", "Smaller brush"), ("radius_up", "Bigger brush"),
                                ("next_mode", "Next mode"), ("front", "Front view"), ("frame", "Frame mesh"),
                            ]
                            .into_iter()
                            .map(|(id, label)| view! { <tr><td>{keys::keys_label(&map, id)}</td><td>{label}</td></tr> })
                            .collect_view()
                        }}
                    </table>
                    <div class="row buttons">
                        <button on:click=move |_| {
                            ui.menu.set(None);
                            ui.show_keys.set(true);
                        }>"All keyboard shortcuts…"</button>
                    </div>
                    <p class="about">"Clayform — a SculptGL-style sculpting app (Tauri + Leptos)."</p>
                </div>
            </Menu>
            <div class="spacer"></div>
            <button
                class="side-toggle"
                title=move || if ui.panel_right.get() { "Move the panel to the left" } else { "Move the panel to the right" }
                on:click=move |_| act(ui, Action::TogglePanelSide)
            >
                {move || if ui.panel_right.get() { "◧ Panel left" } else { "Panel right ◨" }}
            </button>
        </header>
    }
}

#[component]
fn SidePanel(ui: Ui) -> impl IntoView {
    let intensity = Signal::derive(move || ui.intensity.get()[ui.tool.get().index()]);
    let too_dense = move || ui.top_tris.get() * 4 > MAX_TRIANGLES;
    let subdiv_label = move || {
        format!("Subdivide → level {}  ({} tris)", ui.level_count.get() + 1, fmt_count(ui.top_tris.get() * 4))
    };
    let multi = Signal::derive(move || ui.level_count.get() > 1);
    // (comparisons stay outside view!: a bare `>` in an attribute ends the tag)
    let at_top = move || ui.level.get() + 1 >= ui.level_count.get();
    let sel_shape = RwSignal::new(clayform_core::editor::SelectShape::Lasso);
    view! {
        <aside class="panel">
            <tools_ui::ModeBar ui />
            <scene_ui::SubToolFolder ui />
            <conveniences::HistoryFolder ui />
            <Folder title="Sculpting">
                <label class="row">
                    <span class="label">"Tool"</span>
                    <select on:change=move |ev| {
                        if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                            ui.tool.set(Tool::ALL[i]);
                            ui.mode.set(Mode::Sculpt);
                        }
                    }>
                        {TOOL_CATEGORIES
                            .iter()
                            .enumerate()
                            .map(|(c, cat)| {
                                view! {
                                    <optgroup label=*cat>
                                        {Tool::ALL
                                            .iter()
                                            .filter(|t| t.category() == c)
                                            .map(|t| {
                                                let t = *t;
                                                let label = move || {
                                                    let map = ui.keymap.get();
                                                    match map.get(&format!("tool:{}", t.name())).and_then(|k| k.first()) {
                                                        Some(k) => format!("{}  ({k})", t.name()),
                                                        None => t.name().to_string(),
                                                    }
                                                };
                                                view! {
                                                    <option value=t.index().to_string() title=t.description() selected=move || ui.tool.get() == t>
                                                        {label}
                                                    </option>
                                                }
                                            })
                                            .collect_view()}
                                    </optgroup>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <div class="row info tool-desc">{move || ui.tool.get().description()}</div>
                <Show when=move || ui.tool.get() == Tool::Anchors>
                    <div class="row">
                        <span class="label">"Anchor"</span>
                        <select on:change=move |ev| {
                            if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                                ui.anchor_mode.set(AnchorMode::ALL[i]);
                            }
                        }>
                            {AnchorMode::ALL
                                .iter()
                                .enumerate()
                                .map(|(i, m)| {
                                    let m = *m;
                                    view! { <option value=i.to_string() selected=move || ui.anchor_mode.get() == m>{m.name()}</option> }
                                })
                                .collect_view()}
                        </select>
                    </div>
                </Show>
                <Slider
                    label="Radius"
                    min=4f32.ln() max=400f32.ln() step=0.01
                    value=Signal::derive(move || ui.radius.get().ln())
                    on_change=move |v| ui.radius.set(v.exp())
                    format=|v| format!("{:.0}", v.exp())
                />
                <Slider
                    label="Intensity"
                    min=0.0 max=1.0 step=0.01
                    value=intensity
                    on_change=move |v| ui.intensity.update(|a| a[ui.tool.get_untracked().index()] = v)
                />
                <Check label="Negative  (N / hold Ctrl)" value=ui.negative on_change=move |v| ui.negative.set(v) />
                <div class="subhead">"Symmetry"</div>
                <div class="row">
                    <span class="label">"Mirror"</span>
                    <div class="segmented toggles">
                        {(0..3usize)
                            .map(|a| {
                                view! {
                                    <button
                                        title=format!("Mirror across {}{}", AXIS_NAMES[a], if a == 0 { "  (X key)" } else { "" })
                                        class:on=move || ui.symmetry.get().mirror[a]
                                        on:click=move |_| ui.symmetry.update(|s| s.mirror[a] = !s.mirror[a])
                                    >{AXIS_NAMES[a]}</button>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>
                <Slider
                    label="Radial"
                    min=1.0 max=MAX_RADIAL as f32 step=1.0
                    value=Signal::derive(move || ui.symmetry.get().radial as f32)
                    on_change=move |v| ui.symmetry.update(|s| s.radial = v as u32)
                    format=|v| if v < 1.5 { "Off".to_string() } else { format!("{v:.0}×") }
                />
                <div class="row">
                    <span class="label">"Around"</span>
                    <div class="segmented">
                        {(0..3usize)
                            .map(|a| {
                                view! {
                                    <button
                                        title=format!("Radial copies around the {} axis", AXIS_NAMES[a])
                                        class:on=move || ui.symmetry.get().radial_axis == a
                                        on:click=move |_| ui.symmetry.update(|s| s.radial_axis = a)
                                    >{AXIS_NAMES[a]}</button>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>
                <Check
                    label="Around the model's centre"
                    value=Signal::derive(move || ui.symmetry.get().local)
                    on_change=move |v| ui.symmetry.update(|s| s.local = v)
                />
            </Folder>
            <brush_ui::BrushTipFolder ui />
            <brush_ui::PresetsFolder ui />
            <detail_ui::LayersFolder ui />
            <detail_ui::SurfaceFolder ui />
            <topo_ui::TopologyToolsFolder ui />
            <paint_ui::PaintFolder ui />
            <paint_ui::TextureFolder ui />
            <special_ui::ZSpheresFolder ui />
            <special_ui::TextFolder ui />
            <special_ui::FiberFolder ui />
            <special_ui::DynamicsFolder ui />
            <special_ui::ShadowboxFolder ui />
            <select_ui::MaskingFolder ui shape=sel_shape />
            <select_ui::VisibilityFolder ui shape=sel_shape />
            <Folder title="Topology">
                <div class="subhead">"Multiresolution"</div>
                <div class="row">
                    <span class="label">"Level"</span>
                    <div class="stepper">
                        <button
                            title="Lower level  (Page Down)"
                            disabled=move || ui.level.get() == 0
                            on:click=move |_| act(ui, Action::LevelDown)
                        >"◀"</button>
                        <span class="level">{move || format!("{} of {}", ui.level.get() + 1, ui.level_count.get())}</span>
                        <button
                            title="Higher level  (Page Up)"
                            disabled=at_top
                            on:click=move |_| act(ui, Action::LevelUp)
                        >"▶"</button>
                    </div>
                </div>
                <div class="row buttons">
                    <button disabled=too_dense on:click=move |_| act(ui, Action::Subdivide)>{subdiv_label}</button>
                </div>
                <div class="row buttons">
                    <button
                        title="Make this level the lowest one"
                        disabled=move || ui.level.get() == 0
                        on:click=move |_| act(ui, Action::DeleteLower)
                    >"Delete lower"</button>
                    <button
                        title="Remove the levels above this one"
                        disabled=at_top
                        on:click=move |_| act(ui, Action::DeleteHigher)
                    >"Delete higher"</button>
                </div>
                <div class="subhead">"Dynamic topology"</div>
                <Check
                    label="Dynamic topology  (T)"
                    value=ui.dyntopo
                    on_change=move |v| ui.dyntopo.set(v)
                    disabled=multi
                />
                <div class="row">
                    <span class="label">"Size by"</span>
                    <div class="segmented">
                        <button
                            title="Triangle size follows the brush: zoom in or use a smaller brush for finer detail"
                            class:on=move || ui.detail_mode.get() == DetailMode::Brush
                            on:click=move |_| ui.detail_mode.set(DetailMode::Brush)
                        >"Brush"</button>
                        <button
                            title="Triangle size is fixed on the model, whatever the brush size or zoom"
                            class:on=move || ui.detail_mode.get() == DetailMode::Constant
                            on:click=move |_| ui.detail_mode.set(DetailMode::Constant)
                        >"Constant"</button>
                    </div>
                </div>
                <Slider
                    label="Detail"
                    min=0.0 max=1.0 step=0.01
                    value=ui.detail
                    on_change=move |v| ui.detail.set(v)
                    format=|v| format!("{:.0}%", v * 100.0)
                />
                <div class="row info">{move || {
                    let d = ui.detail.get();
                    match ui.detail_mode.get() {
                        DetailMode::Brush => format!("Triangles ≈ {:.0}% of the brush radius", Editor::detail_ratio(d) * 100.0),
                        DetailMode::Constant => format!("≈ {:.0} triangles across the model", Editor::constant_resolution(d)),
                    }
                }}</div>
                <Check
                    label="Smooth brush reduces detail"
                    value=ui.smooth_reduces
                    on_change=move |v| ui.smooth_reduces.set(v)
                />
                <Show when=move || multi.get()>
                    <div class="hint">"Works on a single level. Use Delete lower / Delete higher to merge the levels first."</div>
                </Show>
                <div class="subhead">"Voxel remesh (DynaMesh)"</div>
                <Slider
                    label="Resolution"
                    min=MIN_RESOLUTION as f32 max=MAX_RESOLUTION as f32 step=8.0
                    value=Signal::derive(move || ui.voxel.get().resolution as f32)
                    on_change=move |v| ui.voxel.update(|s| s.resolution = v as u32)
                    format=|v| format!("{v:.0}")
                />
                <Slider
                    label="Smooth"
                    min=0.0 max=4.0 step=1.0
                    value=Signal::derive(move || ui.voxel.get().smooth as f32)
                    on_change=move |v| ui.voxel.update(|s| s.smooth = v as u32)
                    format=|v| format!("{v:.0}")
                />
                <Check
                    label="Project onto the original shape"
                    value=Signal::derive(move || ui.voxel.get().project)
                    on_change=move |v| ui.voxel.update(|s| s.project = v)
                />
                <div class="row buttons">
                    <button class="primary" disabled=move || ui.busy.get() on:click=move |_| act(ui, Action::VoxelRemesh)>
                        {move || if ui.job.get().is_some_and(|j| j.0 == "Remeshing") { "Remeshing…" } else { "Remesh  (R)" }}
                    </button>
                </div>
                <div class="hint">"Rebuilds an even, closed mesh: merges overlapping parts, closes holes and evens out dynamic-topology strokes. Replaces all levels."</div>
                <div class="row info">
                    {move || {
                        let (v, t) = ui.stats.get();
                        format!("{} vertices · {} triangles", fmt_count(v), fmt_count(t))
                    }}
                </div>
            </Folder>
            <scene_ui::TransformFolder ui />
            <scene_ui::ArrayFolder ui />
            <tools_ui::ZModelerFolder ui />
            <tools_ui::PolygroupFolder ui />
            <tools_ui::RetopoFolder ui />
            <tools_ui::MannequinFolder ui />
            <tools_ui::SnapshotFolder ui />
            <tools_ui::ReferenceFolder ui />
            <output_ui::LightsFolder ui />
            <output_ui::RenderFolder ui />
            <output_ui::ExportFolder ui />
            <Folder title="Viewport display">
                <label class="row">
                    <span class="label">"Material"</span>
                    <select on:change=move |ev| {
                        if let Ok(i) = event_target_value(&ev).parse::<u32>() {
                            ui.view.update(|s| s.material = i);
                        }
                    }>
                        {MATERIALS
                            .iter()
                            .enumerate()
                            .map(|(i, name)| {
                                let i = i as u32;
                                view! {
                                    <option value=i.to_string() selected=move || ui.view.get().material == i>{*name}</option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <div class="row buttons">
                    <button on:click=move |_| act(ui, Action::LoadMatcap)>"Load matcap image…"</button>
                </div>
                <Check
                    label="Ambient occlusion"
                    value=Signal::derive(move || ui.view.get().ao)
                    on_change=move |v| ui.view.update(|s| s.ao = v)
                />
                <Slider
                    label="Strength"
                    min=0.0 max=1.0 step=0.01
                    value=Signal::derive(move || ui.view.get().ao_strength)
                    on_change=move |v| ui.view.update(|s| s.ao_strength = v)
                />
                <Check
                    label="Cavity shading"
                    value=Signal::derive(move || ui.view.get().cavity)
                    on_change=move |v| ui.view.update(|s| s.cavity = v)
                />
                <Slider
                    label="Strength"
                    min=0.0 max=1.0 step=0.01
                    value=Signal::derive(move || ui.view.get().cavity_strength)
                    on_change=move |v| ui.view.update(|s| s.cavity_strength = v)
                />
                <Check
                    label="Flat shading"
                    value=Signal::derive(move || ui.view.get().flat)
                    on_change=move |v| ui.view.update(|s| s.flat = v)
                />
                <Check
                    label="Wireframe  (W)"
                    value=Signal::derive(move || ui.view.get().wireframe)
                    on_change=move |v| ui.view.update(|s| s.wireframe = v)
                />
                <Check
                    label="Polygroup colours  (G with Shift)"
                    value=Signal::derive(move || ui.view.get().polygroups)
                    on_change=move |v| ui.view.update(|s| s.polygroups = v)
                />
            </Folder>
            <Folder title="Interface" closed=true>
                <div class="row">
                    <span class="label">"Panel side"</span>
                    <div class="segmented">
                        <button class:on=move || !ui.panel_right.get() on:click=move |_| ui.panel_right.set(false)>"Left"</button>
                        <button class:on=move || ui.panel_right.get() on:click=move |_| ui.panel_right.set(true)>"Right"</button>
                    </div>
                </div>
                <Slider
                    label="Autosave"
                    min=0.0 max=30.0 step=1.0
                    value=Signal::derive(move || ui.autosave_minutes.get() as f32)
                    on_change=move |v| conveniences::set_autosave_minutes(ui, v as u32)
                    format=|v| if v < 0.5 { "Off".to_string() } else { format!("every {v:.0} min") }
                />
                <div class="hint">"Autosave keeps a recovery copy of unsaved work (desktop app). If Clayform closes unexpectedly, it offers to recover it next time. Saving the project removes the copy."</div>
                <div class="row buttons">
                    <button on:click=move |_| ui.show_keys.set(true)>"Keyboard shortcuts…"</button>
                </div>
            </Folder>
        </aside>
    }
}

#[component]
fn App() -> impl IntoView {
    let ui = Ui::new();
    ui.refresh();

    // brush settings -> engine
    Effect::new(move || {
        let brush = Brush {
            tool: ui.tool.get(),
            radius_px: ui.radius.get(),
            intensity: ui.intensity.get(),
            negative: ui.negative.get(),
            symmetry: ui.symmetry.get(),
            dyntopo: ui.dyntopo.get(),
            detail: ui.detail.get(),
            detail_mode: ui.detail_mode.get(),
            smooth_reduces: ui.smooth_reduces.get(),
            anchor_mode: ui.anchor_mode.get(),
            stroke: ui.stroke.get(),
        };
        with_ed(|e| e.brush = brush);
        ui.update_overlay();
    });
    // falloff curve and alpha -> engine
    Effect::new(move || {
        let (c, a) = (ui.falloff.get(), ui.alpha.get());
        with_ed(|e| {
            e.falloff = c;
            e.alpha = a;
        });
    });
    brush_ui::restore_user_alphas(ui);
    // moving, resizing or changing a reference image updates the unsaved mark
    Effect::new(move || {
        let _ = ui.refs.get();
        ui.refresh();
    });
    // mode and ZModeler settings -> engine
    Effect::new(move || {
        let mode = ui.mode.get();
        with_ed(|e| e.set_mode(mode));
        ui.refresh();
    });
    Effect::new(move || {
        let zm = ui.zm.get();
        with_ed(|e| e.zm = zm);
        ui.update_overlay();
    });
    Effect::new(move || {
        let g = ui.gizmo.get();
        with_ed(|e| {
            e.gizmo = g;
            e.redraw = true;
        });
        ui.update_overlay();
    });
    // display settings -> redraw
    Effect::new(move || {
        ui.view.track();
        let fov = ui.fov.get();
        with_ed(|e| {
            e.camera.fov_y = fov.to_radians();
            e.redraw = true;
        });
    });
    // window title shows the project and unsaved changes
    Effect::new(move || {
        let dirty = ui.rev.get() != ui.saved_rev.get();
        host::set_title(&format!("{}{} — Clayform", ui.project_name.get(), if dirty { " •" } else { "" }));
    });
    // remember which side the panel is on
    Effect::new(move || {
        let right = ui.panel_right.get();
        if let Some(s) = storage() {
            let _ = s.set_item(PANEL_SIDE_KEY, if right { "right" } else { "left" });
        }
        with_ed(|e| e.redraw = true);
    });

    conveniences::start_autosave(ui);
    host::listen_progress(move |p| {
        ui.job.update(|j| {
            if let Some(j) = j {
                j.1 = j.1.max(p);
            }
        })
    });

    let keydown = window_event_listener(ev::keydown, move |ev| keys::handle_key(ui, &ev));
    let keyup = window_event_listener(ev::keyup, move |ev| ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key())));
    on_cleanup(move || {
        keydown.remove();
        keyup.remove();
    });

    view! {
        <div class="app" class:panel-right=move || ui.panel_right.get()>
            <MenuBar ui />
            <div class="main">
                <SidePanel ui />
                <Viewport ui />
            </div>
            <footer class="status">
                <span>{move || {
                    let (v, t) = ui.stats.get();
                    format!("{} verts · {} tris", fmt_count(v), fmt_count(t))
                }}</span>
                <span class="sep">"|"</span>
                <span>{move || {
                    let what = match ui.mode.get() {
                        Mode::Sculpt => ui.tool.get().name().to_string(),
                        Mode::ZModeler => {
                            let z = ui.zm.get();
                            format!("ZModeler: {} {}", z.action.name(), z.target.name())
                        }
                        m => m.name().to_string(),
                    };
                    let sym = ui.symmetry.get().label();
                    if sym.is_empty() { what } else { format!("{what} · {sym}") }
                }}</span>
                <span class="sep">"|"</span>
                <span>{move || {
                    if ui.level_count.get() > 1 {
                        format!("Level {} of {}", ui.level.get() + 1, ui.level_count.get())
                    } else if ui.dyntopo.get() {
                        format!("Dynamic topology · detail {:.0}%", ui.detail.get() * 100.0)
                    } else {
                        "1 level".to_string()
                    }
                }}</span>
                <span class="sep">"|"</span>
                <span class="msg">{move || ui.status.get()}</span>
                <span class="autosaved">{move || ui.autosaved_at.get().map(|ms| {
                    let d = js_sys::Date::new(&JsValue::from_f64(ms));
                    format!("autosaved {:02}:{:02}", d.get_hours(), d.get_minutes())
                })}</span>
            </footer>
            <Show when=move || ui.menu.get().is_some()>
                <div class="menu-backdrop" on:pointerdown=move |_| ui.menu.set(None)></div>
            </Show>
            <Show when=move || ui.show_keys.get()>
                <keys::ShortcutEditor ui />
            </Show>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
