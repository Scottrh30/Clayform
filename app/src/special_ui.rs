//! Special tools: ZSpheres, 3D text and shapes, FiberMesh, Dynamics and
//! Shadowbox.

use crate::{host, scene_ui, with_ed, Check, Folder, Slider, Ui};
use base64::Engine as _;
use clayform_core::dynamics::DynSettings;
use clayform_core::editor::{Mode, ZsTool};
use clayform_core::fibers::FiberSettings;
use clayform_core::glam::Vec3;
use clayform_core::shadowbox::{Shadowbox, SIZE};
use clayform_core::text3d::{self, TextSettings, SHAPES};
use clayform_core::zspheres::ZNode;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value};

// ------------------------------------------------------------------ zspheres

/// ZSpheres for the project file.
pub fn zs_json() -> Value {
    with_ed(|e| {
        Value::Array(
            e.zspheres
                .nodes
                .iter()
                .map(|n| json!([n.pos.x, n.pos.y, n.pos.z, n.radius, n.parent.map_or(-1, |p| p as i64)]))
                .collect(),
        )
    })
}

pub fn apply_zs_json(v: &Value) {
    let Some(a) = v.as_array() else { return };
    let nodes: Vec<ZNode> = a
        .iter()
        .filter_map(|n| {
            let n = n.as_array()?;
            let f = |i: usize| n.get(i).and_then(|x| x.as_f64()).map(|x| x as f32);
            let parent = n.get(4).and_then(|x| x.as_i64()).filter(|p| *p >= 0).map(|p| p as usize);
            Some(ZNode { pos: Vec3::new(f(0)?, f(1)?, f(2)?), radius: f(3)?, parent })
        })
        .collect();
    // parents must come first
    if nodes.iter().enumerate().all(|(i, n)| n.parent.is_none_or(|p| p < i)) {
        with_ed(|e| e.zspheres.nodes = nodes);
    }
}

#[component]
pub fn ZSpheresFolder(ui: Ui) -> impl IntoView {
    let tool = RwSignal::new(ZsTool::Draw);
    let res = RwSignal::new(128f32);
    Effect::new(move || {
        let t = tool.get();
        with_ed(|e| e.zs_tool = t);
    });
    let make = move |as_subtool: bool| {
        let r = with_ed(|e| e.zs_make_mesh(res.get_untracked() as usize, as_subtool));
        match r {
            Ok(n) => {
                ui.mode.set(Mode::Sculpt);
                ui.status.set(format!("ZSpheres skinned: {} triangles{}", crate::fmt_count(n), if as_subtool { " as a new SubTool" } else { "" }));
            }
            Err(m) => ui.status.set(m),
        }
        ui.refresh();
    };
    view! {
        <Folder title="ZSpheres" closed=true>
            <Show when=move || ui.mode.get() != Mode::ZSpheres>
                <div class="row buttons">
                    <button class="primary" on:click=move |_| ui.mode.set(Mode::ZSpheres)>"Build with ZSpheres"</button>
                </div>
            </Show>
            <div class="row">
                <span class="label">"Tool"</span>
                <div class="segmented wrap">
                    {ZsTool::ALL.iter().map(|t| {
                        let t = *t;
                        view! { <button class:on=move || tool.get() == t on:click=move |_| tool.set(t)>{t.name()}</button> }
                    }).collect_view()}
                </div>
            </div>
            <div class="row info">"Click in the view for the first sphere. Draw: click a sphere to grow a new one from it, dragging to size it. Move and Scale: drag a sphere. Mirror X makes pairs. Ctrl+Z steps back. The skin previews live."</div>
            <Slider label="Detail" min=48.0 max=256.0 step=8.0
                value=res on_change=move |v| res.set(v)
                format=|v| format!("{v:.0}") />
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| make(true)>"Make mesh (new SubTool)"</button>
                <button on:click=move |_| make(false)>"Replace sculpt"</button>
                <button on:click=move |_| { with_ed(|e| { e.zspheres.clear(); e.redraw = true; }); ui.refresh(); }>"Clear"</button>
            </div>
            <div class="row info">"The skin is quads, ready to sculpt or QRemesh. The spheres are kept in the project."</div>
        </Folder>
    }
}

// ------------------------------------------------------------------ text

#[component]
pub fn TextFolder(ui: Ui) -> impl IntoView {
    let text = RwSignal::new("Clay".to_string());
    let font = RwSignal::new(0usize);
    let custom = RwSignal::new(None::<(String, std::sync::Arc<Vec<u8>>)>);
    let depth = RwSignal::new(0.25f32);
    let spacing = RwSignal::new(0.0f32);
    let segments = RwSignal::new(6f32);
    let settings = move || TextSettings { depth: depth.get_untracked(), spacing: spacing.get_untracked(), segments: segments.get_untracked() as u32 };
    let add_text = move || {
        let s = settings();
        let t = text.get_untracked();
        let r = match (font.get_untracked(), custom.get_untracked()) {
            (99, Some((_, bytes))) => text3d::text_mesh(&bytes, &t, &s, 1.0),
            (i, _) => text3d::text_mesh(text3d::builtin_fonts()[i.min(2)].1, &t, &s, 1.0),
        };
        match r {
            Ok(m) => {
                let n = m.tris.len();
                scene_ui::append(ui, m, "Text");
                ui.status.set(format!("Added \"{t}\" ({} triangles). Remesh it to sculpt on it.", crate::fmt_count(n)));
            }
            Err(e) => ui.status.set(e),
        }
    };
    let load_font = move |_| {
        spawn_local(async move {
            match host::open_font().await {
                Ok(Some(f)) => {
                    if text3d::text_outline(&f.bytes, "A", &TextSettings::default()).is_ok() {
                        custom.set(Some((f.name.clone(), std::sync::Arc::new(f.bytes))));
                        font.set(99);
                        ui.status.set(format!("Font {} loaded", f.name));
                    } else {
                        ui.status.set("Clayform can't read that font (TrueType or OpenType outlines needed)".into());
                    }
                }
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't open the font: {e}")),
            }
        });
    };
    let pick_font = move |ev: ev::Event| {
        if let Ok(i) = event_target_value(&ev).parse::<usize>() {
            font.set(i);
        }
    };
    view! {
        <Folder title="3D text & shapes" closed=true>
            <textarea class="text-input" rows="2" prop:value=move || text.get()
                on:input=move |ev| text.set(event_target_value(&ev))
                on:keydown=move |ev: ev::KeyboardEvent| ev.stop_propagation()></textarea>
            <div class="row">
                <span class="label">"Font"</span>
                <select on:change=pick_font>
                    {text3d::builtin_fonts().iter().enumerate().map(|(i, (n, _))| {
                        view! { <option value=i.to_string() selected=move || font.get() == i>{*n}</option> }
                    }).collect_view()}
                    {move || custom.get().map(|(n, _)| view! { <option value="99" selected=move || font.get() == 99>{n}</option> })}
                </select>
            </div>
            <Slider label="Depth" min=0.02 max=1.0 step=0.01 value=depth on_change=move |v| depth.set(v) />
            <Slider label="Spacing" min=-0.1 max=0.4 step=0.01 value=spacing on_change=move |v| spacing.set(v) />
            <Slider label="Smoothness" min=1.0 max=16.0 step=1.0 value=segments on_change=move |v| segments.set(v) format=|v| format!("{v:.0}") />
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| add_text()>"Add text"</button>
                <button on:click=load_font>"Load font…"</button>
            </div>
            <div class="row info">"Noto Emoji makes symbols (try ★ ♥ ⚙ ☂ ♞). Any TrueType or OpenType font file works with Load font."</div>
            <div class="subhead">"Shapes"</div>
            <div class="segmented wrap">
                {SHAPES.iter().map(|name| {
                    let name = *name;
                    view! {
                        <button on:click=move |_| {
                            match text3d::extrude(&text3d::shape_outline(name), depth.get_untracked()) {
                                Ok(m) => scene_ui::append(ui, m, name),
                                Err(e) => ui.status.set(e),
                            }
                        }>{name}</button>
                    }
                }).collect_view()}
            </div>
        </Folder>
    }
}

// ------------------------------------------------------------------ fibers

#[component]
pub fn FiberFolder(ui: Ui) -> impl IntoView {
    let s = RwSignal::new(FiberSettings::default());
    view! {
        <Folder title="FiberMesh" closed=true>
            <Slider label="Count" min=100f32.ln() max=50000f32.ln() step=0.01
                value=Signal::derive(move || (s.get().count as f32).ln())
                on_change=move |v| s.update(|x| x.count = v.exp() as u32)
                format=|v| crate::fmt_count(v.exp() as usize) />
            <Slider label="Length" min=0.01 max=0.5 step=0.005
                value=Signal::derive(move || s.get().length) on_change=move |v| s.update(|x| x.length = v)
                format=|v| format!("{:.1}%", v * 100.0) />
            <Slider label="Thickness" min=0.005 max=0.2 step=0.005
                value=Signal::derive(move || s.get().thickness) on_change=move |v| s.update(|x| x.thickness = v) />
            <Slider label="Gravity" min=0.0 max=2.0 step=0.05
                value=Signal::derive(move || s.get().gravity) on_change=move |v| s.update(|x| x.gravity = v) />
            <Slider label="Random" min=0.0 max=1.0 step=0.05
                value=Signal::derive(move || s.get().random) on_change=move |v| s.update(|x| x.random = v) />
            <Slider label="Taper" min=0.0 max=1.0 step=0.05
                value=Signal::derive(move || s.get().taper) on_change=move |v| s.update(|x| x.taper = v) />
            <Slider label="Segments" min=1.0 max=12.0 step=1.0
                value=Signal::derive(move || s.get().segments as f32) on_change=move |v| s.update(|x| x.segments = v as u32)
                format=|v| format!("{v:.0}") />
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| {
                    let r = with_ed(|e| e.grow_fibers(&s.get_untracked()));
                    ui.refresh();
                    ui.status.set(match r {
                        Ok(n) => format!("Grew fibres: {} triangles in a new SubTool", crate::fmt_count(n)),
                        Err(m) => m,
                    });
                }>"Grow fibres"</button>
                <button on:click=move |_| s.update(|x| x.seed += 1)>"New random"</button>
            </div>
            <div class="row info">"Mask where the hair or fur should grow (stronger mask = denser), then Grow. The strands are a SubTool of their own: delete it and grow again to try other settings."</div>
        </Folder>
    }
}

// ------------------------------------------------------------------ dynamics

#[component]
pub fn DynamicsFolder(ui: Ui) -> impl IntoView {
    let s = RwSignal::new(DynSettings::default());
    let running = RwSignal::new(false);
    Effect::new(move || {
        let v = s.get();
        with_ed(|e| e.dyn_settings(v));
    });
    // notice when something else ended the run (a stroke, undo, ...)
    Effect::new(move || {
        let _ = ui.rev.get();
        let r = with_ed(|e| e.dyn_running());
        if r != running.get_untracked() {
            running.set(r);
        }
    });
    view! {
        <Folder title="Dynamics" closed=true>
            <Slider label="Gravity" min=0.0 max=3.0 step=0.05
                value=Signal::derive(move || s.get().gravity) on_change=move |v| s.update(|x| x.gravity = v) />
            <Slider label="Stiffness" min=0.1 max=1.0 step=0.01
                value=Signal::derive(move || s.get().stiffness) on_change=move |v| s.update(|x| x.stiffness = v) />
            <Slider label="Bending" min=0.0 max=1.0 step=0.01
                value=Signal::derive(move || s.get().bending) on_change=move |v| s.update(|x| x.bending = v) />
            <Slider label="Inflate" min=0.0 max=0.5 step=0.01
                value=Signal::derive(move || s.get().inflate) on_change=move |v| s.update(|x| x.inflate = v) />
            <Slider label="Floor" min=-0.05 max=1.0 step=0.01
                value=Signal::derive(move || s.get().floor) on_change=move |v| s.update(|x| x.floor = v)
                format=|v| if v < 0.0 { "none".to_string() } else { format!("{:.0}%", v * 100.0) } />
            <Check label="Collide with visible SubTools" value=Signal::derive(move || s.get().collide_subtools) on_change=move |v| s.update(|x| x.collide_subtools = v) />
            <div class="row buttons tight">
                <Show when=move || running.get() fallback=move || view! {
                    <button class="primary" on:click=move |_| {
                        with_ed(|e| e.dyn_start(s.get_untracked()));
                        running.set(true);
                        ui.status.set("Simulating: Apply keeps the result, Cancel puts it back".into());
                    }>"Simulate"</button>
                }>
                    <button class="primary" on:click=move |_| { with_ed(|e| e.dyn_apply()); running.set(false); ui.refresh(); ui.status.set("Dynamics applied".into()); }>"Apply"</button>
                    <button on:click=move |_| { with_ed(|e| e.dyn_cancel()); running.set(false); ui.refresh(); }>"Cancel"</button>
                </Show>
            </div>
            <div class="row info">"The active SubTool falls as cloth (high bending keeps its shape, Inflate makes a balloon) onto a floor below it and the other visible SubTools. Masked points are pinned in place. Settings change live while it runs."</div>
        </Folder>
    }
}

// ------------------------------------------------------------------ shadowbox

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn mask_png(m: &[bool]) -> String {
    let px: Vec<u8> = m.iter().map(|b| if *b { 225 } else { 30 }).collect();
    let mut buf = Vec::new();
    if let Some(img) = image::GrayImage::from_raw(SIZE as u32, SIZE as u32, px) {
        let _ = image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    }
    format!("data:image/png;base64,{}", B64.encode(buf))
}

#[component]
fn MaskPad(label: &'static str, which: usize, sb: RwSignal<Shadowbox>, brush: RwSignal<f32>, erase: RwSignal<bool>) -> impl IntoView {
    let painting = RwSignal::new(false);
    let paint = move |ev: &ev::PointerEvent| {
        let Some(el) = ev.current_target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()) else { return };
        let r = el.get_bounding_client_rect();
        let x = (ev.client_x() as f64 - r.left()) / r.width().max(1.0) * SIZE as f64;
        let y = (ev.client_y() as f64 - r.top()) / r.height().max(1.0) * SIZE as f64;
        let on = !(erase.get_untracked() || ev.ctrl_key());
        sb.update(|s| {
            let img = match which {
                0 => &mut s.front,
                1 => &mut s.side,
                _ => &mut s.top,
            };
            Shadowbox::paint(img, x as f32, y as f32, brush.get_untracked(), on);
        });
    };
    let src = move || {
        let s = sb.get();
        mask_png(match which {
            0 => &s.front,
            1 => &s.side,
            _ => &s.top,
        })
    };
    view! {
        <div class="mask-pad">
            <img src=src alt=label draggable="false"
                on:pointerdown=move |ev| {
                    ev.prevent_default();
                    if let Some(el) = ev.current_target().and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()) {
                        let _ = el.set_pointer_capture(ev.pointer_id());
                    }
                    painting.set(true);
                    paint(&ev);
                }
                on:pointermove=move |ev| if painting.get_untracked() { paint(&ev) }
                on:pointerup=move |_| painting.set(false) />
            <span>{label}</span>
        </div>
    }
}

#[component]
pub fn ShadowboxFolder(ui: Ui) -> impl IntoView {
    let sb = RwSignal::new(Shadowbox::default());
    let brush = RwSignal::new(5f32);
    let erase = RwSignal::new(false);
    let res = RwSignal::new(128f32);
    let build = move |as_subtool: bool| {
        let s = sb.get_untracked();
        let half = with_ed(|e| {
            let (mn, mx) = e.sculptor.mesh.bounds();
            ((mx - mn).max_element() * 0.5).max(0.2)
        });
        match s.build(res.get_untracked() as usize, half) {
            Some(m) => {
                let n = m.tris.len();
                if as_subtool {
                    scene_ui::append(ui, m, "Shadowbox");
                } else {
                    with_ed(|e| {
                        e.sculptor.set_label("Shadowbox");
                        e.sculptor.set_mesh(m);
                        e.frame_all();
                    });
                    ui.refresh();
                }
                ui.status.set(format!("Shadowbox: {} triangles", crate::fmt_count(n)));
            }
            None => ui.status.set("The three silhouettes don't overlap anywhere".into()),
        }
    };
    view! {
        <Folder title="Shadowbox" closed=true>
            <div class="mask-pads">
                <MaskPad label="Front" which=0 sb brush erase />
                <MaskPad label="Side" which=1 sb brush erase />
                <MaskPad label="Top" which=2 sb brush erase />
            </div>
            <Slider label="Brush" min=1.0 max=20.0 step=0.5 value=brush on_change=move |v| brush.set(v) format=|v| format!("{v:.0} px") />
            <Check label="Erase (or hold Ctrl)" value=erase on_change=move |v| erase.set(v) />
            <div class="row buttons tight">
                <button on:click=move |_| sb.set(with_ed(|e| Shadowbox::from_mesh(&e.sculptor.mesh)))>"From model"</button>
                <button on:click=move |_| sb.set(Shadowbox::empty())>"Clear"</button>
                <button on:click=move |_| sb.set(Shadowbox::default())>"Reset"</button>
            </div>
            <Slider label="Detail" min=48.0 max=256.0 step=8.0 value=res on_change=move |v| res.set(v) format=|v| format!("{v:.0}") />
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| build(true)>"Build (new SubTool)"</button>
                <button on:click=move |_| build(false)>"Replace sculpt"</button>
            </div>
            <div class="row info">"Paint the outline seen from the front, the side (front of the model to the right) and the top (front at the bottom). The solid is what's inside all three, like ZBrush's Shadowbox. From model fills them with the current SubTool's silhouettes."</div>
        </Folder>
    }
}
