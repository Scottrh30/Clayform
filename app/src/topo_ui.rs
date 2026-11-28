//! Topology tools panel: QRemesh (automatic quad retopology), decimation,
//! closing holes, mirror and weld, and edge creasing.

use crate::{build_in_slices, end_job, host, job_error, sleep_ms, start_job, with_ed, Check, Folder, Slider, Ui};
use clayform_core::bvh::Bvh;
use clayform_core::mesh::Mesh;
use clayform_core::topo::{self, CreaseMode};
use clayform_core::wire;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Runs a quad remesh / decimation in the background (desktop) or here (browser).
async fn topo_job(ui: Ui, op: &'static str, mesh: Mesh, target: usize, groups: bool) -> Result<(Mesh, Option<Bvh>), String> {
    if host::in_desktop_app() {
        let bytes = wire::encode(&mesh, &[]);
        let headers = [("op", op.to_string()), ("target", target.to_string()), ("groups", if groups { "1" } else { "0" }.to_string())];
        let parts = host::invoke_bytes("topology", &bytes, &headers).await.and_then(|b| wire::decode_parts(&b))?;
        let (m, _, bvh) = build_in_slices(ui, parts).await;
        Ok((m, Some(bvh)))
    } else {
        // let the status line paint before the (blocking) work
        sleep_ms(40).await;
        let m = match op {
            "quad" => topo::quad_remesh(&mesh, target, groups, &mut |_| true)?,
            _ => topo::decimate(&mesh, target, groups, &mut |_| true)?,
        };
        Ok((m, None))
    }
}

fn run(ui: Ui, label: &'static str, op: &'static str, target: usize, groups: bool) {
    if ui.busy.get_untracked() {
        return;
    }
    let mesh = with_ed(|e| {
        e.cancel_drag();
        e.sculptor.mesh.clone()
    });
    let before = mesh.live_tri_count();
    start_job(ui, label);
    spawn_local(async move {
        match topo_job(ui, op, mesh, target, groups).await {
            Ok((m, bvh)) => {
                let after = m.tris.len();
                let quads = m.partner.iter().filter(|p| **p != clayform_core::mesh::NONE).count() / 2;
                let lost = with_ed(|e| {
                    let lost = e.sculptor.level_count() > 1;
                    e.sculptor.set_label(label);
                    e.sculptor.set_mesh_built(m, bvh);
                    e.redraw = true;
                    lost
                });
                let what = if op == "quad" {
                    format!("{label}: {} quads", crate::fmt_count(quads))
                } else {
                    format!("{label}: {} → {} triangles", crate::fmt_count(before), crate::fmt_count(after))
                };
                let more = if lost { " — levels can be rebuilt in Surface detail" } else { "" };
                ui.status.set(format!("{what}{more}"));
            }
            Err(e) => ui.status.set(job_error(label, &e)),
        }
        end_job(ui);
    });
}

#[component]
pub fn TopologyToolsFolder(ui: Ui) -> impl IntoView {
    let target = RwSignal::new(4000f32);
    let keep_groups = RwSignal::new(true);
    let ratio = RwSignal::new(0.2f32);
    let dec_groups = RwSignal::new(false);
    let angle = RwSignal::new(40f32);
    let tris = move || ui.stats.get().1;
    let creases = RwSignal::new(0usize);
    let show = RwSignal::new(true);
    Effect::new(move || {
        let _ = ui.rev.get();
        creases.set(with_ed(|e| e.sculptor.crease_count()));
    });
    let crease = move |mode: CreaseMode, add: bool| {
        with_ed(|e| {
            e.sculptor.set_creases(mode, add);
            e.redraw = true;
        });
        ui.refresh();
        let n = with_ed(|e| e.sculptor.crease_count());
        ui.status.set(if n == 0 { "No creased edges".into() } else { format!("{n} creased edges: subdivision keeps them sharp") });
    };
    let simple = move |label: &'static str, f: fn(&Mesh) -> Result<(Mesh, String), String>| {
        let r = with_ed(|e| {
            e.cancel_drag();
            let r = f(&e.sculptor.mesh);
            if let Ok((m, _)) = &r {
                e.sculptor.set_label(label);
                e.sculptor.set_mesh(m.clone());
                e.redraw = true;
            }
            r.map(|x| x.1)
        });
        ui.refresh();
        ui.status.set(match r {
            Ok(m) => m,
            Err(e) => e,
        });
    };
    view! {
        <Folder title="Topology tools" closed=true>
            <div class="subhead">"QRemesh (automatic quads)"</div>
            <Slider label="Target" min=200f32.ln() max=200000f32.ln() step=0.01
                value=Signal::derive(move || target.get().ln())
                on_change=move |v| target.set(v.exp())
                format=|v| format!("{} quads", crate::fmt_count(v.exp() as usize)) />
            <Check label="Keep polygroups" value=keep_groups on_change=move |v| keep_groups.set(v) />
            <div class="row buttons tight">
                <button class="primary" disabled=move || ui.busy.get() on:click=move |_| run(ui, "QRemesh", "quad", target.get_untracked() as usize, keep_groups.get_untracked())>"QRemesh"</button>
            </div>
            <div class="row info">"Rebuilds the model as even quads at about the target count, relaxed onto the surface. The flow is even everywhere rather than following features like ZRemesher's. Levels are removed, but Surface detail › Rebuild levels puts the detail back."</div>

            <div class="subhead">"Decimate"</div>
            <Slider label="Keep" min=0.01 max=0.9 step=0.01
                value=ratio on_change=move |v| ratio.set(v)
                format=|v| format!("{:.0}%", v * 100.0) />
            <div class="row info">{move || format!("{} → about {} triangles", crate::fmt_count(tris()), crate::fmt_count((tris() as f32 * ratio.get()) as usize))}</div>
            <Check label="Keep polygroup borders" value=dec_groups on_change=move |v| dec_groups.set(v) />
            <div class="row buttons tight">
                <button class="primary" disabled=move || ui.busy.get() on:click=move |_| {
                    let t = (tris() as f32 * ratio.get_untracked()).max(8.0) as usize;
                    run(ui, "Decimate", "decimate", t, dec_groups.get_untracked());
                }>"Decimate"</button>
            </div>
            <div class="row info">"Cuts the triangle count while keeping the shape and detail (quadric error), for export, 3D printing or games. Open edges stay put."</div>

            <div class="subhead">"Repair"</div>
            <div class="row buttons tight">
                <button on:click=move |_| simple("Close holes", |m| {
                    let (out, n) = topo::close_holes(m);
                    if n == 0 { Err("No holes to close".into()) } else { Ok((out, format!("Closed {n} hole{}", if n == 1 { "" } else { "s" }))) }
                })>"Close holes"</button>
                <button title="Replace the -X half with a mirror of the +X half, welded down the middle" on:click=move |_| simple("Mirror and weld", |m| {
                    topo::mirror_weld(m).map(|out| (out, "Mirrored +X onto -X and welded the middle".to_string()))
                })>"Mirror and weld"</button>
            </div>

            <div class="subhead">{move || format!("Creases ({})", creases.get())}</div>
            <Slider label="Angle" min=5.0 max=120.0 step=1.0
                value=angle on_change=move |v| angle.set(v)
                format=|v| format!("{v:.0}°") />
            <div class="row buttons tight">
                <button on:click=move |_| crease(CreaseMode::Angle(angle.get_untracked()), true)>"By angle"</button>
                <button on:click=move |_| crease(CreaseMode::GroupBorders, true)>"Polygroup borders"</button>
                <button on:click=move |_| crease(CreaseMode::Borders, true)>"Open edges"</button>
                <button on:click=move |_| crease(CreaseMode::Clear, false)>"Uncrease all"</button>
            </div>
            <Check label="Show creased edges" value=show on_change=move |v| {
                show.set(v);
                with_ed(|e| { e.show_creases = v; e.redraw = true; });
                ui.update_overlay();
            } />
            <div class="row info">"Creased edges stay sharp when you subdivide (set them before subdividing, or on a lower level). ZModeler's Crease action creases single edges."</div>
        </Folder>
    }
}

#[component]
pub fn RetopoStrokes(ui: Ui) -> impl IntoView {
    let draw = RwSignal::new(false);
    view! {
        <div class="subhead">"Topology brush"</div>
        <Check label="Drag draws topology strokes" value=draw on_change=move |v| {
            draw.set(v);
            with_ed(|e| e.retopo_draw = v);
        } />
        <div class="row buttons tight">
            <button on:click=move |_| { with_ed(|e| { e.retopo_build_strokes(); }); ui.refresh(); }>"Make faces from strokes"</button>
            <button on:click=move |_| { with_ed(|e| e.retopo_clear_strokes()); ui.refresh(); }>"Clear strokes"</button>
        </div>
        <div class="row info">"Draw strokes over the model like a grid: where they cross become points and the cells between become faces. Shift+click an open edge to continue a strip of quads."</div>
    }
}
