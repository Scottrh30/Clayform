//! SubTools, booleans, arrays and the Transform gizmo panels.

use super::*;
use clayform_core::deform::Deformer;
use clayform_core::glam::Vec3;
use clayform_core::scene::{ArrayKind, ArraySettings, BoolOp, SubToolInfo};
use clayform_core::sculpt::GpuDirty;
use std::cell::Cell;

thread_local! {
    /// Live Boolean: on/off, and the latest preview mesh.
    static LIVE: Cell<bool> = const { Cell::new(false) };
    static PREVIEW: RefCell<Option<(Mesh, GpuDirty)>> = const { RefCell::new(None) };
    /// Scene revision the preview was made from.
    static PREVIEW_REV: Cell<u64> = const { Cell::new(u64::MAX) };
    static PREVIEW_BUSY: Cell<bool> = const { Cell::new(false) };
}

const PREVIEW_ID: u64 = u64::MAX;

/// Whether the viewport shows the boolean preview instead of the SubTools
/// (not while dragging, so edits stay visible).
pub fn live_preview_active(dragging: bool) -> bool {
    !dragging && LIVE.with(|l| l.get()) && PREVIEW.with(|p| p.borrow().is_some())
}

pub fn sync_preview(r: &mut Renderer, view: &ViewSettings, dragging: bool) {
    if !LIVE.with(|l| l.get()) {
        return;
    }
    PREVIEW.with(|p| {
        if let Some((m, d)) = p.borrow_mut().as_mut() {
            let ov = render::ObjectView { id: PREVIEW_ID, visible: !dragging, dim: false, instances: &[], paint: view.paint };
            r.sync_object(&ov, m, d, view.wireframe, false);
        }
    });
}

pub fn info_json(i: &SubToolInfo) -> String {
    let array = i.array.map(|a| {
        serde_json::json!({
            "kind": ArrayKind::ALL.iter().position(|k| *k == a.kind), "count": a.count,
            "offset": a.offset.to_array(), "turn": a.turn.to_array(), "scale": a.scale,
            "axis": a.axis, "radius": a.radius, "sweep": a.sweep, "grid": a.grid, "gap": a.gap,
        })
    });
    serde_json::json!({ "name": i.name, "visible": i.visible, "op": i.op.code().to_string(), "array": array }).to_string()
}

pub fn parse_info(json: &str, fallback: &str) -> SubToolInfo {
    let mut info = SubToolInfo { id: 0, name: fallback.into(), visible: true, op: BoolOp::Add, array: None };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(json) else { return info };
    if let Some(n) = j.get("name").and_then(|v| v.as_str()) {
        info.name = n.into();
    }
    info.visible = j.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
    info.op = j.get("op").and_then(|v| v.as_str()).and_then(|s| s.chars().next()).map_or(BoolOp::Add, BoolOp::from_code);
    if let Some(a) = j.get("array").filter(|a| a.is_object()) {
        let f = |k: &str, d: f32| a.get(k).and_then(|v| v.as_f64()).map_or(d, |v| v as f32);
        let v3 = |k: &str, d: Vec3| {
            a.get(k).and_then(|v| v.as_array()).map_or(d, |arr| {
                let g = |i: usize| arr.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                Vec3::new(g(0), g(1), g(2))
            })
        };
        let d = ArraySettings::default();
        let grid = a.get("grid").and_then(|v| v.as_array()).map_or(d.grid, |g| {
            let k = |i: usize| g.get(i).and_then(|x| x.as_u64()).unwrap_or(1) as u32;
            [k(0), k(1), k(2)]
        });
        info.array = Some(ArraySettings {
            kind: a.get("kind").and_then(|v| v.as_u64()).and_then(|i| ArrayKind::ALL.get(i as usize).copied()).unwrap_or(d.kind),
            count: a.get("count").and_then(|v| v.as_u64()).map_or(d.count, |v| v as u32),
            offset: v3("offset", d.offset),
            turn: v3("turn", d.turn),
            scale: f("scale", d.scale),
            axis: a.get("axis").and_then(|v| v.as_u64()).map_or(d.axis, |v| v as usize),
            radius: f("radius", d.radius),
            sweep: f("sweep", d.sweep),
            grid,
            gap: f("gap", d.gap),
        });
    }
    info
}

/// The whole scene as project bytes.
pub fn encode_project(settings: &str) -> Vec<u8> {
    with_ed(|e| {
        let data: Vec<project::SubToolData<&Mesh>> = e
            .scene_levels()
            .into_iter()
            .map(|(info, levels, active, extras)| project::SubToolData { info_json: info_json(&info), levels, active, extras })
            .collect();
        project::encode_scene(&data, e.active_subtool(), settings)
    })
}

/// Runs a voxel boolean in the background (desktop) or here (browser).
async fn csg_job(mesh: Mesh, parts: Vec<(usize, BoolOp)>, settings: VoxelSettings) -> Result<Mesh, String> {
    if host::in_desktop_app() {
        let bytes = wire::encode(&mesh, &[]);
        let spec: Vec<String> = parts.iter().map(|(e, op)| format!("{e}:{}", op.code())).collect();
        let headers = [
            ("resolution", settings.resolution.to_string()),
            ("smooth", settings.smooth.to_string()),
            ("project", "0".to_string()),
            ("parts", spec.join(",")),
        ];
        let out = host::invoke_bytes("remesh", &bytes, &headers).await?;
        let (m, _) = wire::decode(&out)?;
        Ok(m)
    } else {
        sleep_ms(20).await;
        clayform_core::remesh::voxel_csg_with(&mesh, &settings, &parts, &mut |_| true)
    }
}

/// Keeps the Live Boolean preview up to date while it's on.
fn start_live_loop(ui: Ui) {
    spawn_local(async move {
        while LIVE.with(|l| l.get()) {
            sleep_ms(400).await;
            if PREVIEW_BUSY.with(|b| b.get()) || ui.busy.get_untracked() {
                continue;
            }
            let (rev, dragging) = with_ed(|e| (e.scene_revision(), e.drag() != Drag::None));
            if dragging || rev == PREVIEW_REV.with(|r| r.get()) {
                continue;
            }
            let Some((mesh, parts)) = with_ed(|e| e.boolean_parts()) else {
                PREVIEW.with(|p| *p.borrow_mut() = None);
                PREVIEW_REV.with(|r| r.set(rev));
                with_ed(|e| e.redraw = true);
                continue;
            };
            PREVIEW_BUSY.with(|b| b.set(true));
            let settings = VoxelSettings { resolution: ui.bool_res.get_untracked(), smooth: 1, project: false, signed: false };
            let result = csg_job(mesh, parts, settings).await;
            PREVIEW_BUSY.with(|b| b.set(false));
            if !LIVE.with(|l| l.get()) {
                break;
            }
            match result {
                Ok(m) => {
                    PREVIEW.with(|p| *p.borrow_mut() = Some((m, GpuDirty { topology: true, ..Default::default() })));
                    PREVIEW_REV.with(|r| r.set(rev));
                }
                Err(e) => {
                    ui.status.set(format!("Live Boolean: {e}"));
                    PREVIEW_REV.with(|r| r.set(rev));
                }
            }
            with_ed(|e| e.redraw = true);
        }
        PREVIEW.with(|p| *p.borrow_mut() = None);
        with_ed(|e| e.redraw = true);
    });
}

fn set_live(ui: Ui, on: bool) {
    let was = LIVE.with(|l| l.replace(on));
    ui.live_bool.set(on);
    PREVIEW_REV.with(|r| r.set(u64::MAX));
    if on && !was {
        start_live_loop(ui);
        ui.status.set("Live Boolean: showing the result of Add / Subtract / Intersect; it updates when you let go".into());
    } else if !on && was {
        ui.status.set("Live Boolean off: showing the SubTools themselves".into());
    }
    with_ed(|e| e.redraw = true);
}

async fn make_boolean(ui: Ui) {
    if ui.busy.get_untracked() {
        return;
    }
    let Some((mesh, parts)) = with_ed(|e| e.boolean_parts()) else {
        ui.status.set("Show at least two SubTools to combine".into());
        return;
    };
    let mut settings = ui.voxel.get_untracked();
    settings.project = false;
    settings.smooth = settings.smooth.max(1);
    start_job(ui, "Making boolean mesh");
    let t0 = now_ms();
    let result = csg_job(mesh, parts, settings).await;
    match result {
        Ok(m) => {
            let n = m.tris.len();
            with_ed(|e| {
                // hide the parts, add the result
                for i in 0..e.subtool_count() {
                    e.subtool_info_mut(i).visible = false;
                }
                e.append_subtool(m, "Boolean");
            });
            set_live(ui, false);
            ui.status.set(format!("Boolean mesh — {} triangles in {:.1} s; the parts are hidden", fmt_count(n), (now_ms() - t0) / 1000.0));
        }
        Err(e) => ui.status.set(job_error("Boolean", &e)),
    }
    end_job(ui);
}

async fn append_import(ui: Ui) {
    match host::open_mesh().await {
        Ok(Some(f)) => match io::load(&f.name, &f.bytes) {
            Ok(m) => {
                let name = f.name.rsplit_once('.').map_or(f.name.clone(), |(n, _)| n.to_string());
                with_ed(|e| e.append_subtool(m, &name));
                ui.refresh();
            }
            Err(e) => ui.status.set(format!("Import failed: {e}")),
        },
        Ok(None) => {}
        Err(e) => ui.status.set(format!("Import failed: {e}")),
    }
}

fn append(ui: Ui, m: Mesh, name: &str) {
    with_ed(|e| {
        // new parts sit beside what's there, the size of the active SubTool
        let (mn, mx) = e.scene_bounds();
        let (amn, amx) = e.sculptor.mesh.bounds();
        let size = (amx - amn).max_element().max(0.2);
        let mut m = m;
        let (bmn, bmx) = m.bounds();
        let k = size / (bmx - bmn).max_element().max(1e-6);
        let c = (bmn + bmx) * 0.5;
        let at = Vec3::new(mx.x + size * 0.6, (mn.y + mx.y) * 0.5, (mn.z + mx.z) * 0.5);
        for p in &mut m.positions {
            *p = (*p - c) * k + at;
        }
        m.compute_all_normals();
        e.append_subtool(m, name);
    });
    ui.refresh();
}

#[component]
pub fn SubToolFolder(ui: Ui) -> impl IntoView {
    let editing = RwSignal::new(None::<usize>);
    let confirm = RwSignal::new(None::<&'static str>);
    let ask = move |what: &'static str, f: Box<dyn Fn()>| {
        if confirm.get_untracked() == Some(what) {
            confirm.set(None);
            f();
        } else {
            confirm.set(Some(what));
        }
    };
    let rows = move || {
        let list = ui.subtools.get();
        let active = ui.active_subtool.get();
        let live = ui.live_bool.get();
        list.into_iter()
            .enumerate()
            .map(|(i, info)| {
                let is_active = i == active;
                let name = info.name.clone();
                let op = info.op;
                view! {
                    <div class="st-row" class:active=is_active class:hidden=!info.visible>
                        <button class="st-eye" title="Show / hide" on:click=move |_| {
                            with_ed(|e| {
                                let v = e.subtool_info(i).visible;
                                if i != e.active_subtool() || !v {
                                    e.subtool_info_mut(i).visible = !v;
                                }
                            });
                            ui.refresh();
                        }>{if info.visible { "●" } else { "○" }}</button>
                        {move || if editing.get() == Some(i) {
                            let n = name.clone();
                            view! {
                                <input class="st-name-edit" type="text" prop:value=n
                                    on:change=move |ev| {
                                        let v = event_target_value(&ev);
                                        if !v.trim().is_empty() {
                                            with_ed(|e| e.subtool_info_mut(i).name = v.trim().to_string());
                                        }
                                        editing.set(None);
                                        ui.refresh();
                                    }
                                    on:blur=move |_| editing.set(None)
                                />
                            }.into_any()
                        } else {
                            let n = name.clone();
                            view! {
                                <button class="st-name" title="Click to sculpt this SubTool, double-click to rename (Alt+click in the view picks one too)"
                                    on:click=move |_| {
                                        with_ed(|e| e.select_subtool(i));
                                        ui.refresh();
                                    }
                                    on:dblclick=move |_| editing.set(Some(i))
                                >{n}</button>
                            }.into_any()
                        }}
                        <Show when=move || live>
                            <button class="st-op" title="Boolean: Add, Subtract or Intersect (click to change)" class:sub=op == BoolOp::Subtract class:int=op == BoolOp::Intersect
                                on:click=move |_| {
                                    with_ed(|e| {
                                        let o = e.subtool_info(i).op;
                                        let k = BoolOp::ALL.iter().position(|x| *x == o).unwrap_or(0);
                                        e.subtool_info_mut(i).op = BoolOp::ALL[(k + 1) % 3];
                                    });
                                    PREVIEW_REV.with(|r| r.set(u64::MAX));
                                    ui.refresh();
                                }>{op.name()}</button>
                        </Show>
                    </div>
                }
            })
            .collect_view()
    };
    let count = move || ui.subtools.get().len();
    view! {
        <Folder title="SubTools">
            <div class="st-list">{rows}</div>
            <div class="row buttons">
                <button title="Move the active SubTool up" on:click=move |_| { with_ed(|e| e.move_subtool(e.active_subtool(), true)); ui.refresh(); }>"▲"</button>
                <button title="Move the active SubTool down" on:click=move |_| { with_ed(|e| e.move_subtool(e.active_subtool(), false)); ui.refresh(); }>"▼"</button>
                <button on:click=move |_| { with_ed(|e| e.duplicate_subtool()); ui.refresh(); }>"Duplicate"</button>
                <button disabled=move || count() < 2 class:confirm=move || confirm.get() == Some("delete")
                    on:click=move |_| ask("delete", Box::new(move || { with_ed(|e| { let a = e.active_subtool(); e.delete_subtool(a); }); ui.refresh(); }))>
                    {move || if confirm.get() == Some("delete") { "Delete — sure?" } else { "Delete" }}
                </button>
            </div>
            <div class="row">
                <span class="label">"Append"</span>
                <select on:change=move |ev| {
                    let v = event_target_value(&ev);
                    match v.as_str() {
                        "sphere" => append(ui, Mesh::icosphere(5), "Sphere"),
                        "cube" => append(ui, Mesh::cube(5), "Cube"),
                        "qcube" => append(ui, clayform_core::poly::Polys::qcube(2).to_mesh(), "QCube"),
                        "qcyl" => append(ui, clayform_core::poly::Polys::qcylinder(16, 2).to_mesh(), "QCylinder"),
                        "plane" => append(ui, Mesh::plane(6), "Plane"),
                        "import" => spawn_local(append_import(ui)),
                        _ => {}
                    }
                    if let Some(t) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok()) {
                        t.set_value("");
                    }
                }>
                    <option value="" selected>"Add a SubTool…"</option>
                    <option value="sphere">"Sphere"</option>
                    <option value="cube">"Cube"</option>
                    <option value="qcube">"QCube (quads)"</option>
                    <option value="qcyl">"QCylinder (quads)"</option>
                    <option value="plane">"Plane"</option>
                    <option value="import">"Import OBJ / STL…"</option>
                </select>
            </div>
            <div class="row buttons">
                <button disabled=move || count() < 2 class:confirm=move || confirm.get() == Some("merge")
                    title="Combine the visible SubTools into one (overlaps stay separate surfaces; use a voxel remesh or a boolean to fuse them)"
                    on:click=move |_| ask("merge", Box::new(move || { with_ed(|e| e.merge_visible()); ui.refresh(); }))>
                    {move || if confirm.get() == Some("merge") { "Merge — sure?" } else { "Merge visible" }}
                </button>
                <button class:confirm=move || confirm.get() == Some("split")
                    title="One SubTool per polygroup of the active SubTool (its levels are flattened)"
                    on:click=move |_| ask("split", Box::new(move || {
                        let n = with_ed(|e| e.split_by_groups());
                        ui.status.set(if n > 1 { format!("Split into {n} SubTools") } else { "Only one polygroup: nothing to split".into() });
                        ui.refresh();
                    }))>
                    {move || if confirm.get() == Some("split") { "Split — sure?" } else { "Split by polygroups" }}
                </button>
            </div>
            <div class="subhead">"Live Boolean"</div>
            <Check label="Show the boolean result" value=ui.live_bool on_change=move |v| set_live(ui, v) />
            <Slider
                label="Preview"
                min=32.0 max=256.0 step=8.0
                value=Signal::derive(move || ui.bool_res.get() as f32)
                on_change=move |v| { ui.bool_res.set(v as u32); PREVIEW_REV.with(|r| r.set(u64::MAX)); }
                format=|v| format!("{v:.0} voxels")
            />
            <div class="row buttons">
                <button class="primary" disabled=move || ui.busy.get() || count() < 2 on:click=move |_| spawn_local(make_boolean(ui))>"Make boolean mesh"</button>
            </div>
            <div class="hint">"Visible SubTools combine top to bottom: each Adds to, Subtracts from or Intersects what's above it. The preview updates when you let go; Make boolean mesh builds it at the Topology remesh resolution as a new SubTool."</div>
        </Folder>
    }
}

#[component]
pub fn ArrayFolder(ui: Ui) -> impl IntoView {
    let arr = move || {
        let a = ui.active_subtool.get();
        ui.subtools.get().get(a).and_then(|i| i.array)
    };
    let set = move |f: &dyn Fn(&mut ArraySettings)| {
        with_ed(|e| {
            let a = e.active_subtool();
            if let Some(s) = e.subtool_info_mut(a).array.as_mut() {
                f(s);
            }
        });
        ui.refresh();
    };
    let on = Signal::derive(move || arr().is_some());
    let kind = move || arr().map(|a| a.kind);
    let num = move |label: &'static str, min: f32, max: f32, step: f32, get: fn(&ArraySettings) -> f32, put: fn(&mut ArraySettings, f32), fmt: fn(f32) -> String| {
        view! {
            <Slider label=label min=min max=max step=step
                value=Signal::derive(move || arr().map_or(0.0, |a| get(&a)))
                on_change=move |v| set(&|s| put(s, v))
                format=fmt
            />
        }
    };
    let int = |v: f32| format!("{v:.0}");
    let dec = |v: f32| format!("{v:.2}");
    let deg = |v: f32| format!("{v:.0}°");
    view! {
        <Folder title="Array" closed=true>
            <Check label="Repeat the active SubTool" value=on on_change=move |v| {
                with_ed(|e| {
                    let a = e.active_subtool();
                    e.subtool_info_mut(a).array = v.then(ArraySettings::default);
                });
                ui.refresh();
            } />
            <Show when=move || on.get()>
                <div class="row">
                    <span class="label">"Layout"</span>
                    <div class="segmented">
                        {ArrayKind::ALL.into_iter().map(|k| view! {
                            <button class:on=move || kind() == Some(k) on:click=move |_| set(&|s| s.kind = k)>{k.name()}</button>
                        }).collect_view()}
                    </div>
                </div>
                <Show when=move || kind() != Some(ArrayKind::Grid)>
                    {num("Copies", 1.0, 64.0, 1.0, |a| a.count as f32, |a, v| a.count = v as u32, int)}
                </Show>
                <Show when=move || kind() == Some(ArrayKind::Line)>
                    {num("Offset X", -3.0, 3.0, 0.01, |a| a.offset.x, |a, v| a.offset.x = v, dec)}
                    {num("Offset Y", -3.0, 3.0, 0.01, |a| a.offset.y, |a, v| a.offset.y = v, dec)}
                    {num("Offset Z", -3.0, 3.0, 0.01, |a| a.offset.z, |a, v| a.offset.z = v, dec)}
                    {num("Turn X", -180.0, 180.0, 1.0, |a| a.turn.x, |a, v| a.turn.x = v, deg)}
                    {num("Turn Y", -180.0, 180.0, 1.0, |a| a.turn.y, |a, v| a.turn.y = v, deg)}
                    {num("Turn Z", -180.0, 180.0, 1.0, |a| a.turn.z, |a, v| a.turn.z = v, deg)}
                    {num("Scale", 0.5, 1.5, 0.01, |a| a.scale, |a, v| a.scale = v, dec)}
                </Show>
                <Show when=move || kind() == Some(ArrayKind::Circle)>
                    <div class="row">
                        <span class="label">"Around"</span>
                        <div class="segmented">
                            {(0..3usize).map(|k| view! {
                                <button class:on=move || arr().is_some_and(|a| a.axis == k) on:click=move |_| set(&|s| s.axis = k)>{AXIS_NAMES[k]}</button>
                            }).collect_view()}
                        </div>
                    </div>
                    {num("Radius", 0.0, 5.0, 0.01, |a| a.radius, |a, v| a.radius = v, dec)}
                    {num("Sweep", 10.0, 360.0, 1.0, |a| a.sweep, |a, v| a.sweep = v, deg)}
                </Show>
                <Show when=move || kind() == Some(ArrayKind::Grid)>
                    {num("Across X", 1.0, 8.0, 1.0, |a| a.grid[0] as f32, |a, v| a.grid[0] = v as u32, int)}
                    {num("Across Y", 1.0, 8.0, 1.0, |a| a.grid[1] as f32, |a, v| a.grid[1] = v as u32, int)}
                    {num("Across Z", 1.0, 8.0, 1.0, |a| a.grid[2] as f32, |a, v| a.grid[2] = v as u32, int)}
                    {num("Gap", 0.0, 2.0, 0.01, |a| a.gap, |a, v| a.gap = v, dec)}
                </Show>
                <div class="row buttons">
                    <button class="primary" on:click=move |_| { with_ed(|e| e.convert_array()); ui.refresh(); }>"Convert to mesh"</button>
                </div>
                <div class="hint">"Copies are live: sculpt the original and they follow. Offsets and radius are in object sizes. Convert makes them real geometry (undoable). Exports and booleans include them."</div>
            </Show>
        </Folder>
    }
}

#[component]
pub fn TransformFolder(ui: Ui) -> impl IntoView {
    let gz = move || ui.gizmo.get();
    let set_def = move |d: Option<Deformer>| {
        ui.gizmo.update(|g| g.deformer = d);
        ui.mode.set(Mode::Transform);
    };
    view! {
        <Folder title="Transform" closed=true>
            <Show when=move || ui.mode.get() != Mode::Transform>
                <div class="row buttons">
                    <button class="primary" on:click=move |_| ui.mode.set(Mode::Transform)>"Switch to Transform mode"</button>
                </div>
            </Show>
            <div class="row buttons tight">
                <button class:on=move || gz().deformer.is_none() on:click=move |_| set_def(None)>"Gizmo"</button>
                {Deformer::ALL.into_iter().map(|d| view! {
                    <button class:on=move || gz().deformer == Some(d) on:click=move |_| set_def(Some(d))>{d.name()}</button>
                }).collect_view()}
            </div>
            <Show when=move || gz().deformer.is_some()>
                <div class="row">
                    <span class="label">"Axis"</span>
                    <div class="segmented">
                        {(0..3usize).map(|k| view! {
                            <button class:on=move || gz().axis == k on:click=move |_| ui.gizmo.update(|g| g.axis = k)>{AXIS_NAMES[k]}</button>
                        }).collect_view()}
                    </div>
                </div>
            </Show>
            <div class="hint">{move || if gz().deformer.is_some() {
                "Drag left or right anywhere in the view to deform along the axis. Masked areas stay put."
            } else {
                "Drag an arrow to move along it, a ring to turn, an end box to stretch one axis, the outer circle to scale, or the centre dot to move freely. Masked areas stay put, so mask to transform just a part."
            }}</div>
        </Folder>
    }
}
