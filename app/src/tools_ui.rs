//! Panels and overlays for the modelling tools: modes, ZModeler,
//! polygroups, retopology, mannequins, Snapshot 3D and reference images.

use super::*;
use clayform_core::editor::OverlayStyle;
use clayform_core::glam::{Mat3, Vec3};
use clayform_core::mannequin::PRESETS;
use clayform_core::poly::{Action as ZAction, Primitive, Target};
use clayform_core::snapshot::{self, Style};

thread_local! {
    /// The image Snapshot 3D works from: (width, height, RGBA).
    static SNAP_IMAGE: RefCell<Option<(u32, u32, Vec<u8>)>> = const { RefCell::new(None) };
}

/// A reference image pinned over the viewport (Spotlight-style).
#[derive(Clone, Debug, PartialEq)]
pub struct RefImage {
    pub id: u32,
    pub name: String,
    /// data: URL of the image.
    pub src: String,
    /// Position and width in viewport points.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub opacity: f32,
    /// Locked images let clicks through to the model.
    pub locked: bool,
    pub visible: bool,
}

fn data_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn refs_json(ui: Ui) -> serde_json::Value {
    serde_json::Value::Array(
        ui.refs
            .get_untracked()
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name, "src": r.src, "x": r.x, "y": r.y, "w": r.w,
                    "opacity": r.opacity, "locked": r.locked, "visible": r.visible,
                })
            })
            .collect(),
    )
}

pub fn apply_refs_json(ui: Ui, v: &serde_json::Value) {
    let Some(arr) = v.as_array() else { return };
    let f = |o: &serde_json::Value, k: &str, d: f32| o.get(k).and_then(|x| x.as_f64()).map_or(d, |x| x as f32);
    let refs = arr
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            Some(RefImage {
                id: i as u32 + 1,
                name: o.get("name")?.as_str()?.to_string(),
                src: o.get("src")?.as_str()?.to_string(),
                x: f(o, "x", 20.0),
                y: f(o, "y", 20.0),
                w: f(o, "w", 240.0),
                opacity: f(o, "opacity", 0.5),
                locked: o.get("locked").and_then(|x| x.as_bool()).unwrap_or(false),
                visible: o.get("visible").and_then(|x| x.as_bool()).unwrap_or(true),
            })
        })
        .collect();
    ui.refs.set(refs);
}

// ---------------------------------------------------------------- modes

fn mode_hint(m: Mode) -> &'static str {
    match m {
        Mode::Sculpt => "Brushes shape the surface. Pick one in Sculpting.",
        Mode::ZModeler => "Hover a polygon to see the target, then click or drag. Mirror X applies.",
        Mode::Retopo => "Click on the model to place points: four make a quad. Drag points to move them, Ctrl+click deletes.",
        Mode::Mannequin => "Drag the joints to pose the figure. Convert it to a sculpt when you're happy.",
    }
}

#[component]
pub fn ModeBar(ui: Ui) -> impl IntoView {
    view! {
        <div class="modebar">
            <div class="segmented modes">
                {Mode::ALL
                    .iter()
                    .map(|m| {
                        let m = *m;
                        view! {
                            <button
                                title=mode_hint(m)
                                class:on=move || ui.mode.get() == m
                                on:click=move |_| ui.mode.set(m)
                            >{if m == Mode::Retopo { "Retopo" } else { m.name() }}</button>
                        }
                    })
                    .collect_view()}
            </div>
            <div class="hint">{move || mode_hint(ui.mode.get())}</div>
        </div>
    }
}

fn style_class(s: OverlayStyle) -> &'static str {
    match s {
        OverlayStyle::Target => "ov-target",
        OverlayStyle::Pending => "ov-pending",
        OverlayStyle::Mesh => "ov-mesh",
        OverlayStyle::Building => "ov-building",
        OverlayStyle::Joint => "ov-joint",
        OverlayStyle::Active => "ov-active",
        OverlayStyle::Curve => "ov-curve",
    }
}

/// Hover targets, retopology and mannequin joints drawn over the model.
#[component]
pub fn ModeOverlay(ui: Ui) -> impl IntoView {
    view! {
        <svg class="overlay mode-overlay">
            {move || {
                let o = ui.overlay.get();
                let polys = o
                    .polys
                    .iter()
                    .map(|p| {
                        let pts = p.iter().map(|q| format!("{:.1},{:.1}", q.x, q.y)).collect::<Vec<_>>().join(" ");
                        view! { <polygon class="ov-face" points=pts></polygon> }
                    })
                    .collect_view();
                let lines = o
                    .lines
                    .iter()
                    .map(|(a, b, s)| {
                        view! { <line class=style_class(*s) x1=a.x y1=a.y x2=b.x y2=b.y></line> }
                    })
                    .collect_view();
                let points = o
                    .points
                    .iter()
                    .map(|(p, s)| {
                        let r = if matches!(s, OverlayStyle::Joint | OverlayStyle::Active) { 5.5 } else { 3.5 };
                        view! { <circle class=style_class(*s) cx=p.x cy=p.y r=r></circle> }
                    })
                    .collect_view();
                view! { {polys} {lines} {points} }
            }}
        </svg>
    }
}

// ------------------------------------------------------------- ZModeler

#[component]
pub fn ZModelerFolder(ui: Ui) -> impl IntoView {
    let is_mode = move || ui.mode.get() == Mode::ZModeler;
    view! {
        <Folder title="ZModeler" closed=true>
            <Show when=move || !is_mode()>
                <div class="row buttons">
                    <button class="primary" on:click=move |_| ui.mode.set(Mode::ZModeler)>"Switch to ZModeler mode"</button>
                </div>
            </Show>
            <label class="row">
                <span class="label">"Action"</span>
                <select on:change=move |ev| {
                    if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                        ui.zm.update(|z| z.action = ZAction::ALL[i]);
                        ui.mode.set(Mode::ZModeler);
                    }
                }>
                    {ZAction::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let a = *a;
                            view! { <option value=i.to_string() title=a.description() selected=move || ui.zm.get().action == a>{a.name()}</option> }
                        })
                        .collect_view()}
                </select>
            </label>
            <div class="row info tool-desc">{move || ui.zm.get().action.description()}</div>
            <div class="row">
                <span class="label">"Target"</span>
                <div class="segmented">
                    {Target::ALL
                        .iter()
                        .map(|t| {
                            let t = *t;
                            view! {
                                <button
                                    class:on=move || ui.zm.get().target == t
                                    on:click=move |_| ui.zm.update(|z| z.target = t)
                                >{t.name()}</button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="subhead">"Quad primitives"</div>
            <Slider
                label="Divisions"
                min=1.0 max=12.0 step=1.0
                value=Signal::derive(move || ui.zm.get().divisions as f32)
                on_change=move |v| ui.zm.update(|z| z.divisions = v as u32)
                format=|v| format!("{v:.0}")
            />
            <div class="row buttons">
                {Primitive::ALL
                    .iter()
                    .map(|p| {
                        let p = *p;
                        view! { <button on:click=move |_| act(ui, Action::NewPrimitive(p))>{p.name()}</button> }
                    })
                    .collect_view()}
            </div>
            <div class="hint">"Quads stay quads through ZModeler, OBJ export and projects. Sculpting with dynamic topology or a voxel remesh makes triangles again."</div>
        </Folder>
    }
}

#[component]
pub fn PolygroupFolder(ui: Ui) -> impl IntoView {
    let groups = move || {
        ui.rev.get();
        with_ed(|e| {
            let mut g = e.sculptor.mesh.groups.clone();
            g.sort_unstable();
            g.dedup();
            g.len()
        })
    };
    view! {
        <Folder title="Polygroups" closed=true>
            <Check
                label="Show polygroup colours  (Shift+G)"
                value=Signal::derive(move || ui.view.get().polygroups)
                on_change=move |v| ui.view.update(|s| s.polygroups = v)
            />
            <div class="row info">{move || format!("{} polygroups", groups())}</div>
            <Slider
                label="Angle"
                min=5.0 max=90.0 step=1.0
                value=ui.group_angle
                on_change=move |v| ui.group_angle.set(v)
                format=|v| format!("{v:.0}°")
            />
            <div class="row buttons">
                <button title="Split the surface where it turns by more than the angle" on:click=move |_| {
                    let a = ui.group_angle.get_untracked();
                    with_ed(|e| e.auto_groups(a));
                    ui.view.update(|s| s.polygroups = true);
                    ui.refresh();
                }>"Group by normals"</button>
                <button title="Faces under the mask become one new polygroup" on:click=move |_| {
                    with_ed(|e| e.group_masked());
                    ui.view.update(|s| s.polygroups = true);
                    ui.refresh();
                }>"Group masked"</button>
                <button on:click=move |_| {
                    with_ed(|e| e.clear_groups());
                    ui.refresh();
                }>"Clear"</button>
            </div>
            <div class="row buttons">
                <button on:click=move |_| {
                    ui.tool.set(Tool::GroupPaint);
                    ui.mode.set(Mode::Sculpt);
                    ui.view.update(|s| s.polygroups = true);
                }>"Polygroup Paint brush"</button>
                <button on:click=move |_| {
                    ui.zm.update(|z| z.action = ZAction::GroupIt);
                    ui.mode.set(Mode::ZModeler);
                    ui.view.update(|s| s.polygroups = true);
                }>"PolyGroupIt (click)"</button>
            </div>
        </Folder>
    }
}

// ----------------------------------------------------------- retopology

#[component]
pub fn RetopoFolder(ui: Ui) -> impl IntoView {
    let empty = move || ui.retopo_count.get().1 == 0;
    let levels_label = move || {
        let l = ui.retopo_levels.get();
        if l == 0 { "Apply (replace the sculpt)".to_string() } else { format!("Apply + {l} projected levels") }
    };
    let apply = move |_| {
        if ui.busy.get_untracked() {
            return;
        }
        let levels = ui.retopo_levels.get_untracked();
        match with_ed(|e| e.retopo_apply(levels)) {
            Ok(()) => {
                ui.mode.set(Mode::Sculpt);
                ui.status.set(if levels > 0 {
                    format!("Retopology applied with {levels} levels projected onto the old sculpt")
                } else {
                    "Retopology applied".into()
                });
            }
            Err(e) => ui.status.set(e),
        }
        ui.refresh();
    };
    view! {
        <Folder title="Retopology" closed=true>
            <Show when=move || ui.mode.get() != Mode::Retopo>
                <div class="row buttons">
                    <button class="primary" on:click=move |_| ui.mode.set(Mode::Retopo)>"Start drawing topology"</button>
                </div>
            </Show>
            <div class="hint">
                "Click on the model to place points — every four make a quad (click the first point again for a triangle, or press Enter). Click an existing point to reuse it. Drag points to move them; Ctrl+click deletes. Ctrl+Z steps back."
            </div>
            <div class="row info">{move || {
                let (v, f) = ui.retopo_count.get();
                format!("{v} points · {f} faces")
            }}</div>
            <div class="row buttons">
                <button disabled=empty title="Even out the spacing, keeping points on the model" on:click=move |_| {
                    with_ed(|e| e.retopo_relax());
                    ui.refresh();
                }>"Relax"</button>
                <button disabled=empty title="Copy the faces to the other side of X" on:click=move |_| {
                    with_ed(|e| e.retopo_mirror());
                    ui.refresh();
                }>"Mirror X"</button>
                <button disabled=move || ui.retopo_count.get().0 == 0 on:click=move |_| {
                    with_ed(|e| {
                        e.retopo.clear();
                        e.redraw = true;
                    });
                    ui.refresh();
                }>"Clear"</button>
            </div>
            <Slider
                label="Levels"
                min=0.0 max=5.0 step=1.0
                value=Signal::derive(move || ui.retopo_levels.get() as f32)
                on_change=move |v| ui.retopo_levels.set(v as u32)
                format=|v| format!("{v:.0}")
            />
            <div class="row buttons">
                <button class="primary" disabled=empty on:click=apply>{levels_label}</button>
            </div>
            <div class="hint">"Applying replaces the sculpt (undoable). Each projected level is subdivided and snapped onto the old surface, so its detail carries over onto the new topology."</div>
        </Folder>
    }
}

// ------------------------------------------------------------ mannequin

#[component]
pub fn MannequinFolder(ui: Ui) -> impl IntoView {
    let convert = move |_| {
        if ui.busy.get_untracked() {
            return;
        }
        let mut settings = ui.voxel.get_untracked();
        settings.project = false;
        settings.resolution = settings.resolution.max(160);
        ui.mode.set(Mode::Sculpt);
        run_remesh(ui, None, settings, "Mannequin converted");
    };
    view! {
        <Folder title="Mannequin" closed=true>
            <div class="row buttons">
                <button class="primary" on:click=move |_| act(ui, Action::NewMannequin)>"New mannequin"</button>
                <button on:click=move |_| ui.mode.set(Mode::Mannequin)>"Pose"</button>
            </div>
            <div class="subhead">"Poses"</div>
            <div class="row buttons">
                {PRESETS
                    .iter()
                    .map(|p| {
                        let p = *p;
                        view! {
                            <button on:click=move |_| {
                                with_ed(|e| e.mannequin_preset(p));
                                ui.mode.set(Mode::Mannequin);
                                ui.refresh();
                            }>{p}</button>
                        }
                    })
                    .collect_view()}
            </div>
            <Slider
                label="Build"
                min=0.6 max=1.6 step=0.01
                value=ui.bulk
                on_change=move |v| {
                    ui.bulk.set(v);
                    with_ed(|e| {
                        e.mannequin.bulk = v;
                        if e.mannequin_shown() {
                            e.mannequin_changed();
                        }
                    });
                    ui.refresh();
                }
                format=|v| format!("{:.0}%", v * 100.0)
            />
            <div class="row buttons">
                <button class="primary" disabled=move || ui.busy.get() on:click=convert>"Convert to sculpt"</button>
            </div>
            <div class="hint">"Converting merges the capsules into one closed surface with a voxel remesh (at least resolution 160), ready to sculpt. Each bone is its own polygroup until then."</div>
        </Folder>
    }
}

// ------------------------------------------------------------ Snapshot 3D

fn load_snapshot_image(ui: Ui, name: &str, bytes: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width().max(img.height()) > 1024 { img.resize(1024, 1024, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    SNAP_IMAGE.with(|s| *s.borrow_mut() = Some((w, h, rgba.into_raw())));
    ui.snap_thumb.set(Some(data_url(bytes)));
    ui.status.set(format!("Snapshot image: {name} ({w}×{h})"));
    Ok(())
}

/// 0 = new sculpt, 1 = add to the sculpt, 2 = cut from it.
fn run_snapshot(ui: Ui, how: u8) {
    if ui.busy.get_untracked() {
        ui.status.set("Please wait for the current job to finish (or cancel it)".into());
        return;
    }
    let mut settings = ui.snap.get_untracked();
    let scale = ui.snap_scale.get_untracked();
    // where it goes: Add sits on the surface facing the camera (half sunk
    // in); Subtract is centred in the model and deep enough to cut through
    let (center, size, basis) = with_ed(|e| {
        let (mn, mx) = e.sculptor.mesh.bounds();
        let extent = (mx - mn).max_element();
        let f = e.camera.forward();
        let right = f.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = right.cross(f);
        let mid = (mn + mx) * 0.5;
        let center = if how == 1 {
            e.sculptor.raycast(e.camera.eye(), f).map_or(mid, |h| h.point)
        } else {
            mid
        };
        (center, extent * scale, Mat3::from_cols(right, up, -f))
    });
    if how == 2 {
        let extent = with_ed(|e| {
            let (mn, mx) = e.sculptor.mesh.bounds();
            (mx - mn).length()
        });
        // thickness in world units = 2 * depth * (size / 2)
        settings.depth = settings.depth.max(extent * 1.2 / size.max(1e-6));
    }
    let built = SNAP_IMAGE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|(w, h, px)| snapshot::build(*w as usize, *h as usize, px, &settings))
    });
    let mut m = match built {
        None => {
            ui.status.set("Load an image for Snapshot 3D first".into());
            return;
        }
        Some(Err(e)) => {
            ui.status.set(e);
            return;
        }
        Some(Ok(m)) => m,
    };
    ui.mode.set(Mode::Sculpt);
    if how == 0 {
        let n = m.tris.len();
        with_ed(|e| e.set_mesh(m));
        ui.status.set(format!("Snapshot 3D — {} triangles", fmt_count(n)));
        ui.refresh();
        return;
    }
    snapshot::place(&mut m, center, size, basis);
    let combined = with_ed(|e| snapshot::combine(&e.sculptor.mesh, &m, how == 2));
    let mut vs = ui.voxel.get_untracked();
    vs.project = false;
    vs.smooth = vs.smooth.max(2);
    vs.signed = true;
    run_remesh(ui, Some(combined), vs, if how == 2 { "Snapshot cut out" } else { "Snapshot added" });
}

#[component]
pub fn SnapshotFolder(ui: Ui) -> impl IntoView {
    let load = move |_| {
        spawn_local(async move {
            match host::open_image().await {
                Ok(Some(f)) => {
                    if let Err(e) = load_snapshot_image(ui, &f.name, &f.bytes) {
                        ui.status.set(format!("Couldn't use that image: {e}"));
                    }
                }
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't open the image: {e}")),
            }
        })
    };
    let no_image = move || ui.snap_thumb.get().is_none();
    view! {
        <Folder title="Snapshot 3D" closed=true>
            <div class="row buttons">
                <button on:click=load>"Load image…"</button>
            </div>
            <Show when=move || !no_image()>
                <div class="row snap-thumb">
                    <img src=move || ui.snap_thumb.get().unwrap_or_default() />
                </div>
            </Show>
            <Slider
                label="Threshold"
                min=0.05 max=0.95 step=0.01
                value=Signal::derive(move || ui.snap.get().threshold)
                on_change=move |v| ui.snap.update(|s| s.threshold = v)
                format=|v| format!("{:.0}%", v * 100.0)
            />
            <Check label="Invert (light shapes)" value=Signal::derive(move || ui.snap.get().invert) on_change=move |v| ui.snap.update(|s| s.invert = v) />
            <div class="row">
                <span class="label">"Shape"</span>
                <div class="segmented">
                    <button class:on=move || ui.snap.get().style == Style::Inflate on:click=move |_| ui.snap.update(|s| s.style = Style::Inflate)>"Inflate"</button>
                    <button class:on=move || ui.snap.get().style == Style::Extrude on:click=move |_| ui.snap.update(|s| s.style = Style::Extrude)>"Extrude"</button>
                </div>
            </div>
            <Slider
                label="Depth"
                min=0.02 max=1.0 step=0.01
                value=Signal::derive(move || ui.snap.get().depth)
                on_change=move |v| ui.snap.update(|s| s.depth = v)
                format=|v| format!("{:.0}%", v * 100.0)
            />
            <Slider
                label="Resolution"
                min=48.0 max=320.0 step=8.0
                value=Signal::derive(move || ui.snap.get().resolution as f32)
                on_change=move |v| ui.snap.update(|s| s.resolution = v as u32)
                format=|v| format!("{v:.0}")
            />
            <Slider
                label="Size"
                min=0.1 max=1.5 step=0.01
                value=ui.snap_scale
                on_change=move |v| ui.snap_scale.set(v)
                format=|v| format!("{:.0}%", v * 100.0)
            />
            <div class="row buttons">
                <button class="primary" disabled=no_image on:click=move |_| run_snapshot(ui, 0)>"New"</button>
                <button disabled=move || no_image() || ui.busy.get() on:click=move |_| run_snapshot(ui, 1)>"Add"</button>
                <button disabled=move || no_image() || ui.busy.get() on:click=move |_| run_snapshot(ui, 2)>"Subtract"</button>
            </div>
            <div class="hint">"Dark pixels (or opaque ones, for transparent PNGs) make the shape. Add places it on the surface facing you; Subtract cuts it straight through the model. Both merge with a voxel remesh at the Topology resolution."</div>
        </Folder>
    }
}

// ------------------------------------------------------- reference images

#[component]
pub fn ReferenceFolder(ui: Ui) -> impl IntoView {
    let add = move |_| {
        spawn_local(async move {
            match host::open_image().await {
                Ok(Some(f)) => {
                    let id = ui.refs.get_untracked().iter().map(|r| r.id).max().unwrap_or(0) + 1;
                    let n = ui.refs.get_untracked().len() as f32;
                    ui.refs.update(|r| {
                        r.push(RefImage {
                            id,
                            name: f.name.clone(),
                            src: data_url(&f.bytes),
                            x: 24.0 + n * 24.0,
                            y: 24.0 + n * 24.0,
                            w: 260.0,
                            opacity: 0.6,
                            locked: false,
                            visible: true,
                        })
                    });
                    ui.status.set(format!("Reference {} — drag it to place it, drag its corner to size it, lock it to sculpt through", f.name));
                }
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't open the image: {e}")),
            }
        })
    };
    let ref_ids = move || -> Vec<u32> { ui.refs.get().into_iter().map(|r| r.id).collect() };
    view! {
        <Folder title="Reference images" closed=true>
            <div class="row buttons">
                <button on:click=add>"Add image…"</button>
            </div>
            <For
                each=ref_ids
                key=|id| *id
                children=move |id| {
                    let get = move || ui.refs.get().into_iter().find(|r| r.id == id);
                    let set = move |f: &dyn Fn(&mut RefImage)| ui.refs.update(|v| {
                        if let Some(r) = v.iter_mut().find(|r| r.id == id) {
                            f(r)
                        }
                    });
                    view! {
                        <div class="ref-item">
                            <div class="row">
                                <span class="ref-name" title=move || get().map(|r| r.name).unwrap_or_default()>{move || get().map(|r| r.name).unwrap_or_default()}</span>
                                <button class="mini" class:on=move || get().is_some_and(|r| r.visible) title="Show / hide"
                                    on:click=move |_| set(&|r| r.visible = !r.visible)>"👁"</button>
                                <button class="mini" class:on=move || get().is_some_and(|r| r.locked) title="Lock: clicks go through to the model"
                                    on:click=move |_| set(&|r| r.locked = !r.locked)>"🔒"</button>
                                <button class="mini" title="Remove" on:click=move |_| ui.refs.update(|v| v.retain(|r| r.id != id))>"✕"</button>
                            </div>
                            <Slider
                                label="Opacity"
                                min=0.05 max=1.0 step=0.01
                                value=Signal::derive(move || get().map_or(0.5, |r| r.opacity))
                                on_change=move |v| set(&|r| r.opacity = v)
                                format=|v| format!("{:.0}%", v * 100.0)
                            />
                        </div>
                    }
                }
            />
            <div class="hint">"Images float over the viewport. Unlocked, drag to move and drag the corner to resize; locked, you sculpt through them. They're saved with the project."</div>
        </Folder>
    }
}

/// Reference images over the viewport.
#[component]
pub fn References(ui: Ui) -> impl IntoView {
    // (id, resize?, pointer start, original x, y, w)
    type RefDrag = Option<(u32, bool, (f32, f32), f32, f32, f32)>;
    let drag: StoredValue<RefDrag> = StoredValue::new(None);
    let ref_ids = move || -> Vec<u32> { ui.refs.get().into_iter().map(|r| r.id).collect() };
    view! {
        <div class="refs">
            <For
                each=ref_ids
                key=|id| *id
                children=move |id| {
                    let get = move || ui.refs.get().into_iter().find(|r| r.id == id);
                    let down = move |ev: ev::PointerEvent, resize: bool| {
                        ev.prevent_default();
                        ev.stop_propagation();
                        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                            let _ = el.set_pointer_capture(ev.pointer_id());
                        }
                        if let Some(r) = get() {
                            drag.set_value(Some((id, resize, (ev.client_x() as f32, ev.client_y() as f32), r.x, r.y, r.w)));
                        }
                    };
                    let moved = move |ev: ev::PointerEvent| {
                        let Some((did, resize, (sx, sy), x, y, w)) = drag.get_value() else { return };
                        if did != id {
                            return;
                        }
                        let (dx, dy) = (ev.client_x() as f32 - sx, ev.client_y() as f32 - sy);
                        ui.refs.update(|v| {
                            if let Some(r) = v.iter_mut().find(|r| r.id == id) {
                                if resize {
                                    r.w = (w + dx).max(40.0);
                                } else {
                                    r.x = x + dx;
                                    r.y = y + dy;
                                }
                            }
                        });
                    };
                    let up = move |_ev: ev::PointerEvent| drag.set_value(None);
                    view! {
                        <div
                            class="ref-image"
                            class:locked=move || get().is_some_and(|r| r.locked)
                            style:display=move || if get().is_some_and(|r| r.visible) { "block" } else { "none" }
                            style:left=move || format!("{}px", get().map_or(0.0, |r| r.x))
                            style:top=move || format!("{}px", get().map_or(0.0, |r| r.y))
                            style:width=move || format!("{}px", get().map_or(100.0, |r| r.w))
                            style:opacity=move || get().map_or(0.5, |r| r.opacity).to_string()
                            on:pointerdown=move |ev| down(ev, false)
                            on:pointermove=moved
                            on:pointerup=up
                        >
                            <img src=move || get().map(|r| r.src).unwrap_or_default() draggable="false" />
                            <div
                                class="ref-resize"
                                on:pointerdown=move |ev| down(ev, true)
                                on:pointermove=moved
                                on:pointerup=up
                            ></div>
                        </div>
                    }
                }
            />
        </div>
    }
}
