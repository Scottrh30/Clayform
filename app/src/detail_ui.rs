//! Surface detail and layers: sculpt layers, morph target, Surface Noise,
//! Project All and reprojecting higher levels.

use crate::{with_ed, Check, Folder, Slider, Ui};
use clayform_core::detail::{LayerInfo, NoiseKind, NoiseSettings};
use clayform_core::sculpt::Tool;
use leptos::ev;
use leptos::prelude::*;

/// What the panels show, read from the engine on every refresh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailState {
    pub layers: Vec<LayerInfo>,
    pub morph: bool,
    /// Level the morph target was stored on, if it's on another one.
    pub morph_elsewhere: Option<usize>,
    pub previewing: bool,
    pub reproject: Option<usize>,
    pub level: usize,
}

pub fn refresh(ui: Ui) {
    let st = with_ed(|e| {
        let s = &e.sculptor;
        DetailState {
            layers: s.layers(),
            morph: s.has_morph(),
            morph_elsewhere: s.morph_level().filter(|_| !s.has_morph()),
            previewing: s.noise_previewing(),
            reproject: s.reproject_levels(),
            level: s.level(),
        }
    });
    if st != ui.sdetail.get_untracked() {
        ui.sdetail.set(st);
    }
}

fn act(ui: Ui, f: impl FnOnce(&mut clayform_core::editor::Editor)) {
    with_ed(|e| {
        f(e);
        e.redraw = true;
    });
    ui.refresh();
}

#[component]
pub fn LayersFolder(ui: Ui) -> impl IntoView {
    let editing = RwSignal::new(None::<usize>);
    let rows = move || {
        let d = ui.sdetail.get();
        d.layers
            .into_iter()
            .enumerate()
            .map(|(i, l)| {
                let usable = l.usable;
                let name = l.name.clone();
                let name2 = l.name.clone();
                let strength = l.strength;
                let rec_title = if l.recording { "Recording: strokes go into this layer" } else { "Record strokes into this layer" };
                let note = (!usable).then(|| format!("level {}", l.level + 1));
                view! {
                    <div class="layer" class:rec=l.recording class:off=!usable>
                        <div class="layer-head">
                            <button class="st-eye" title="Show / hide" disabled=!usable
                                on:click=move |_| act(ui, |e| e.sculptor.set_layer_visible(i, !l.visible))>
                                {if l.visible { "●" } else { "○" }}
                            </button>
                            <Show when=move || editing.get() == Some(i) fallback=move || {
                                let n = name.clone();
                                view! { <span class="layer-name" title="Double-click to rename" on:dblclick=move |_| editing.set(Some(i))>{n}</span> }
                            }>
                                <input class="st-rename" type="text" prop:value=name2.clone()
                                    on:keydown=move |ev: ev::KeyboardEvent| {
                                        ev.stop_propagation();
                                        if ev.key() == "Enter" || ev.key() == "Escape" {
                                            if ev.key() == "Enter" {
                                                let v = event_target_value(&ev);
                                                act(ui, |e| e.sculptor.rename_layer(i, v.trim()));
                                            }
                                            editing.set(None);
                                        }
                                    }
                                    on:blur=move |ev| {
                                        let v = event_target_value(&ev);
                                        act(ui, |e| e.sculptor.rename_layer(i, v.trim()));
                                        editing.set(None);
                                    } />
                            </Show>
                            {note.map(|n| view! { <span class="layer-note">{n}</span> })}
                            <button class="rec" class:on=l.recording title=rec_title disabled=!usable
                                on:click=move |_| act(ui, |e| { if l.recording { e.sculptor.stop_recording(); } else { e.sculptor.record_layer(i); } })>
                                "REC"
                            </button>
                            <button class="icon" title="Bake into the sculpt (keep its effect, remove the layer)"
                                on:click=move |_| act(ui, |e| e.sculptor.bake_layer(i))>"⤵"</button>
                            <button class="icon" title="Delete the layer and its effect"
                                on:click=move |_| act(ui, |e| e.sculptor.delete_layer(i))>"✕"</button>
                        </div>
                        <label class="row slider layer-slider">
                            <input type="range" min="-1" max="2" step="0.01" disabled=!usable prop:value=strength.to_string()
                                on:input=move |ev| {
                                    // (no refresh while dragging: it would rebuild this slider)
                                    if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                                        with_ed(|e| { e.sculptor.set_layer_strength(i, v); e.redraw = true; });
                                    }
                                }
                                on:change=move |_| ui.refresh() />
                            <span class="value">{format!("{:.0}%", strength * 100.0)}</span>
                        </label>
                    </div>
                }
            })
            .collect_view()
    };
    let recording = move || ui.sdetail.get().layers.iter().any(|l| l.recording);
    view! {
        <Folder title="Layers" closed=true>
            <div class="layer-list">{rows}</div>
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| {
                    let n = ui.sdetail.get_untracked().layers.len() + 1;
                    act(ui, |e| { e.sculptor.add_layer(&format!("Layer {n}")); });
                    ui.status.set("New layer: your strokes now go into it (REC)".into());
                }>"New layer"</button>
                <button disabled=move || !recording() on:click=move |_| act(ui, |e| e.sculptor.stop_recording())>"Stop recording"</button>
            </div>
            <div class="row info">"While a layer records (REC), strokes go into it. Hide it with the dot, or fade it with its slider (negative inverts, above 100% exaggerates). Layers belong to one level and turn dynamic topology off; ⤵ bakes a layer into the sculpt."</div>
        </Folder>
    }
}

#[component]
pub fn SurfaceFolder(ui: Ui) -> impl IntoView {
    let noise = ui.noise;
    let d = move || ui.sdetail.get();
    let previewing = move || d().previewing;
    let preview = move || {
        let s = noise.get_untracked();
        act(ui, |e| e.sculptor.noise_preview(&s));
    };
    // live preview follows the settings
    Effect::new(move || {
        let s = noise.get();
        if ui.sdetail.get_untracked().previewing {
            with_ed(|e| {
                e.sculptor.noise_preview(&s);
                e.redraw = true;
            });
        }
    });
    let reach = RwSignal::new(0.05f32);
    let pstrength = RwSignal::new(1.0f32);
    view! {
        <Folder title="Surface detail" closed=true>
            <div class="subhead">"Morph target"</div>
            <div class="row buttons tight">
                <button on:click=move |_| { act(ui, |e| e.sculptor.store_morph()); ui.status.set("Morph target stored: the Morph brush paints back to this shape".into()); }>"Store"</button>
                <button disabled=move || !d().morph on:click=move |_| act(ui, |e| e.sculptor.switch_morph())>"Switch"</button>
                <button disabled=move || !d().morph && d().morph_elsewhere.is_none() on:click=move |_| act(ui, |e| e.sculptor.delete_morph())>"Delete"</button>
                <button disabled=move || !d().morph on:click=move |_| { ui.tool.set(Tool::Morph); ui.mode.set(clayform_core::editor::Mode::Sculpt); }>"Morph brush"</button>
            </div>
            <div class="row info">{move || match (d().morph, d().morph_elsewhere) {
                (true, _) => "Stored for this level. Switch swaps the sculpt with it.".to_string(),
                (false, Some(l)) => format!("Stored on level {}: go there to use it.", l + 1),
                _ => "Store the shape now, then sculpt; the Morph brush paints parts back.".to_string(),
            }}</div>

            <div class="subhead">"Surface noise"</div>
            <div class="row">
                <span class="label">"Type"</span>
                <div class="segmented wrap">
                    {NoiseKind::ALL.iter().map(|k| {
                        let k = *k;
                        view! { <button class:on=move || noise.get().kind == k on:click=move |_| noise.update(|s| s.kind = k)>{k.name()}</button> }
                    }).collect_view()}
                </div>
            </div>
            <Slider label="Size" min=0.005 max=0.5 step=0.001
                value=Signal::derive(move || noise.get().scale)
                on_change=move |v| noise.update(|s| s.scale = v)
                format=|v| format!("{:.1}%", v * 100.0) />
            <Slider label="Height" min=-0.05 max=0.05 step=0.0005
                value=Signal::derive(move || noise.get().strength)
                on_change=move |v| noise.update(|s| s.strength = v)
                format=|v| format!("{:.2}%", v * 100.0) />
            <Slider label="Detail" min=1.0 max=8.0 step=1.0
                value=Signal::derive(move || noise.get().octaves as f32)
                on_change=move |v| noise.update(|s| s.octaves = v as u32)
                format=|v| format!("{v:.0}") />
            <Slider label="Seed" min=1.0 max=100.0 step=1.0
                value=Signal::derive(move || noise.get().seed as f32)
                on_change=move |v| noise.update(|s| s.seed = v as u32)
                format=|v| format!("{v:.0}") />
            <Check label="Masked areas stay smooth" value=Signal::derive(move || noise.get().use_mask) on_change=move |v| noise.update(|s| s.use_mask = v) />
            <div class="row buttons tight">
                <Show when=previewing fallback=move || view! {
                    <button on:click=move |_| preview()>"Preview"</button>
                }>
                    <button class="primary" on:click=move |_| { act(ui, |e| e.sculptor.noise_apply()); ui.status.set("Surface noise applied".into()); }>"Apply"</button>
                    <button on:click=move |_| act(ui, |e| e.sculptor.noise_cancel())>"Cancel"</button>
                </Show>
            </div>
            <div class="row info">"Preview shows the noise and follows the sliders; Apply keeps it (one undo step). It's real geometry, so subdivide first for fine noise."</div>

            <div class="subhead">"Project All"</div>
            <Slider label="Distance" min=0.005 max=0.3 step=0.005
                value=reach on_change=move |v| reach.set(v)
                format=|v| format!("{:.1}%", v * 100.0) />
            <Slider label="Strength" min=0.0 max=1.0 step=0.01
                value=pstrength on_change=move |v| pstrength.set(v) />
            <div class="row buttons tight">
                <button on:click=move |_| {
                    let r = with_ed(|e| e.project_all(reach.get_untracked(), pstrength.get_untracked()));
                    ui.refresh();
                    match r {
                        Ok(n) => ui.status.set(format!("Projected {n} points onto the other visible SubTools")),
                        Err(m) => ui.status.set(m),
                    }
                }>"Project from visible SubTools"</button>
            </div>
            <div class="row info">"Pulls the active SubTool onto the other visible SubTools' surfaces along its normals, e.g. to put an old sculpt's detail onto a new, cleaner mesh. Mask areas to keep them."</div>

            <Show when=move || d().reproject.is_some()>
                <div class="subhead">"Reproject higher levels"</div>
                <div class="row buttons tight">
                    <button class="primary" on:click=move |_| {
                        ui.status.set("Rebuilding levels…".into());
                        let r = with_ed(|e| { let r = e.sculptor.reproject_higher(); e.redraw = true; r });
                        ui.refresh();
                        ui.status.set(match r { Ok(()) => "Levels rebuilt with the old detail".into(), Err(m) => m });
                    }>{move || format!("Rebuild {} levels", d().reproject.unwrap_or(0))}</button>
                    <button on:click=move |_| act(ui, |e| e.sculptor.forget_lost_levels())>"Forget"</button>
                </div>
                <div class="row info">"The last topology change removed the levels. This subdivides the new base again and projects the old highest level's detail onto it."</div>
            </Show>
        </Folder>
    }
}

pub fn default_noise() -> NoiseSettings {
    NoiseSettings::default()
}
