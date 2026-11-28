//! Brush tip panel (stroke type, lazy mouse, pen pressure, backface masking,
//! falloff curve editor, alphas, chisel shapes) and brush presets.

use crate::{host, storage, Check, Folder, Slider, Ui};
use base64::Engine as _;
use clayform_core::brush::{Alpha, ChiselShape, FalloffCurve, StrokeKind, StrokeSettings, BUILTIN_ALPHAS, CURVE_PRESETS};
use clayform_core::glam::Vec2;
use clayform_core::sculpt::Tool;
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value};
use std::sync::Arc;
use wasm_bindgen::JsCast;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const PRESETS_KEY: &str = "clayform.brush_presets";
const ALPHAS_KEY: &str = "clayform.alphas";
/// Loaded images are reduced to at most this size.
const ALPHA_MAX: u32 = 256;

thread_local! {
    static BUILTINS: Vec<Arc<Alpha>> = BUILTIN_ALPHAS.iter().filter_map(|n| Alpha::builtin(n)).map(Arc::new).collect();
}

pub fn builtin_alpha(name: &str) -> Option<Arc<Alpha>> {
    BUILTINS.with(|b| b.iter().find(|a| a.name == name).cloned())
}

/// PNG data URL of an alpha, for thumbnails.
fn thumb(a: &Alpha) -> String {
    let img = image::GrayImage::from_raw(a.w as u32, a.h as u32, a.to_luma()).unwrap_or_default();
    let img = image::imageops::resize(&img, 48, 48, image::imageops::FilterType::Triangle);
    let mut png = Vec::new();
    let _ = image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
    format!("data:image/png;base64,{}", B64.encode(png))
}

// ------------------------------------------------------------------ JSON

pub fn alpha_json(a: &Alpha) -> Value {
    if BUILTIN_ALPHAS.contains(&a.name.as_str()) && builtin_alpha(&a.name).is_some_and(|b| *b == *a) {
        json!({ "name": a.name })
    } else {
        json!({ "name": a.name, "w": a.w, "h": a.h, "data": B64.encode(a.to_luma()) })
    }
}

pub fn parse_alpha(v: &Value) -> Option<Arc<Alpha>> {
    let name = v.get("name")?.as_str()?;
    match v.get("data").and_then(|d| d.as_str()) {
        Some(d) => {
            let w = v.get("w")?.as_u64()? as usize;
            let h = v.get("h")?.as_u64()? as usize;
            let bytes = B64.decode(d).ok()?;
            Alpha::from_luma(name, w, h, &bytes).map(Arc::new)
        }
        None => builtin_alpha(name),
    }
}

fn stroke_json(s: &StrokeSettings) -> Value {
    json!({
        "kind": StrokeKind::ALL.iter().position(|k| *k == s.kind),
        "spacing": s.spacing, "lazy": s.lazy, "lazy_radius": s.lazy_radius,
        "scatter": s.scatter, "jitter": s.jitter,
        "pressure_size": s.pressure_size, "pressure_intensity": s.pressure_intensity,
        "backface": s.backface, "alpha_angle": s.alpha_angle, "alpha_follow": s.alpha_follow,
        "chisel": ChiselShape::ALL.iter().position(|c| *c == s.chisel),
    })
}

fn apply_stroke_json(v: &Value, s: &mut StrokeSettings) {
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    if let Some(k) = v.get("kind").and_then(|x| x.as_u64()).and_then(|i| StrokeKind::ALL.get(i as usize)) {
        s.kind = *k;
    }
    if let Some(c) = v.get("chisel").and_then(|x| x.as_u64()).and_then(|i| ChiselShape::ALL.get(i as usize)) {
        s.chisel = *c;
    }
    s.spacing = f("spacing").unwrap_or(s.spacing).clamp(0.02, 2.0);
    s.lazy = b("lazy").unwrap_or(s.lazy);
    s.lazy_radius = f("lazy_radius").unwrap_or(s.lazy_radius).clamp(1.0, 400.0);
    s.scatter = f("scatter").unwrap_or(s.scatter).clamp(0.0, 2.0);
    s.jitter = f("jitter").unwrap_or(s.jitter).clamp(0.0, 1.0);
    s.pressure_size = b("pressure_size").unwrap_or(s.pressure_size);
    s.pressure_intensity = b("pressure_intensity").unwrap_or(s.pressure_intensity);
    s.backface = b("backface").unwrap_or(s.backface);
    s.alpha_angle = f("alpha_angle").unwrap_or(s.alpha_angle);
    s.alpha_follow = b("alpha_follow").unwrap_or(s.alpha_follow);
}

/// Stroke, falloff and alpha settings (saved in projects and presets).
pub fn tip_json(ui: Ui) -> Value {
    json!({
        "stroke": stroke_json(&ui.stroke.get_untracked()),
        "falloff": ui.falloff.get_untracked().to_text(),
        "alpha": ui.alpha.get_untracked().map(|a| alpha_json(&a)),
    })
}

pub fn apply_tip_json(ui: Ui, v: &Value) {
    if let Some(s) = v.get("stroke") {
        ui.stroke.update(|st| apply_stroke_json(s, st));
    }
    if let Some(c) = v.get("falloff").and_then(|x| x.as_str()).and_then(FalloffCurve::from_text) {
        ui.falloff.set(c);
    }
    match v.get("alpha") {
        Some(Value::Null) => ui.alpha.set(None),
        Some(a) => {
            if let Some(a) = parse_alpha(a) {
                remember_user_alpha(ui, &a);
                ui.alpha.set(Some(a));
            }
        }
        None => {}
    }
}

// ------------------------------------------------------------------ user alphas

fn remember_user_alpha(ui: Ui, a: &Arc<Alpha>) {
    if builtin_alpha(&a.name).is_some_and(|b| *b == **a) {
        return;
    }
    ui.user_alphas.update(|list| {
        list.retain(|x| x.name != a.name);
        list.insert(0, a.clone());
        list.truncate(8);
    });
    store_user_alphas(ui);
}

fn store_user_alphas(ui: Ui) {
    let list: Vec<Value> = ui.user_alphas.get_untracked().iter().map(|a| alpha_json(a)).collect();
    if let Some(s) = storage() {
        let _ = s.set_item(ALPHAS_KEY, &Value::Array(list).to_string());
    }
}

pub fn restore_user_alphas(ui: Ui) {
    let Some(text) = storage().and_then(|s| s.get_item(ALPHAS_KEY).ok().flatten()) else { return };
    if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&text) {
        ui.user_alphas.set(a.iter().filter_map(parse_alpha).collect());
    }
}

async fn load_alpha_image(ui: Ui) {
    match host::open_image().await {
        Ok(Some(f)) => match image::load_from_memory(&f.bytes) {
            Ok(img) => {
                let img = if img.width().max(img.height()) > ALPHA_MAX { img.resize(ALPHA_MAX, ALPHA_MAX, image::imageops::FilterType::Triangle) } else { img };
                let g = img.to_luma8();
                let name = f.name.rsplit_once('.').map_or(f.name.clone(), |(n, _)| n.to_string());
                match Alpha::from_luma(&name, g.width() as usize, g.height() as usize, g.as_raw()) {
                    Some(a) => {
                        let a = Arc::new(a);
                        remember_user_alpha(ui, &a);
                        ui.alpha.set(Some(a));
                        ui.status.set(format!("Alpha {name} loaded: white raises, black leaves the surface alone"));
                    }
                    None => ui.status.set("That image is empty".into()),
                }
            }
            Err(e) => ui.status.set(format!("Couldn't read the image: {e}")),
        },
        Ok(None) => {}
        Err(e) => ui.status.set(format!("Couldn't open the image: {e}")),
    }
}

// ------------------------------------------------------------------ presets

/// Everything a brush preset holds.
fn preset_json(ui: Ui, name: &str) -> Value {
    let tool = ui.tool.get_untracked();
    json!({
        "name": name,
        "tool": tool.name(),
        "radius": ui.radius.get_untracked(),
        "intensity": ui.intensity.get_untracked()[tool.index()],
        "negative": ui.negative.get_untracked(),
        "tip": tip_json(ui),
    })
}

fn apply_preset(ui: Ui, p: &Value) {
    if let Some(t) = p.get("tool").and_then(|x| x.as_str()).and_then(|n| Tool::ALL.iter().find(|t| t.name() == n)) {
        ui.tool.set(*t);
        ui.mode.set(clayform_core::editor::Mode::Sculpt);
        if let Some(i) = p.get("intensity").and_then(|x| x.as_f64()) {
            ui.intensity.update(|a| a[t.index()] = (i as f32).clamp(0.0, 1.0));
        }
    }
    if let Some(r) = p.get("radius").and_then(|x| x.as_f64()) {
        ui.radius.set((r as f32).clamp(4.0, 400.0));
    }
    if let Some(n) = p.get("negative").and_then(|x| x.as_bool()) {
        ui.negative.set(n);
    }
    match p.get("tip") {
        Some(t) => apply_tip_json(ui, t),
        None => {}
    }
    if p.get("tip").and_then(|t| t.get("alpha")).is_none() {
        ui.alpha.set(None);
    }
    let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("preset");
    ui.status.set(format!("Brush preset {name} loaded"));
}

/// Starter presets, shown until replaced by your own.
fn starter_presets() -> Vec<Value> {
    let tip = |kind: usize, alpha: Option<&str>, falloff: usize, extra: Value| {
        let mut st = stroke_json(&StrokeSettings { kind: StrokeKind::ALL[kind], ..Default::default() });
        if let (Value::Object(a), Value::Object(b)) = (&mut st, extra) {
            a.extend(b);
        }
        json!({ "stroke": st, "falloff": FalloffCurve::preset(falloff).to_text(), "alpha": alpha.map(|n| json!({ "name": n })) })
    };
    vec![
        json!({ "name": "Skin pores (spray)", "tool": "Draw", "radius": 30.0, "intensity": 0.25, "negative": true,
                "tip": tip(3, Some("Pores"), 0, json!({ "scatter": 0.8, "jitter": 0.7 })) }),
        json!({ "name": "Reptile scales (drag)", "tool": "Draw", "radius": 80.0, "intensity": 0.5, "negative": false,
                "tip": tip(2, Some("Scales"), 4, json!({})) }),
        json!({ "name": "Fabric weave", "tool": "Layer", "radius": 70.0, "intensity": 0.15, "negative": false,
                "tip": tip(2, Some("Weave"), 5, json!({})) }),
        json!({ "name": "Clean lines (lazy)", "tool": "Dam Standard", "radius": 18.0, "intensity": 0.45, "negative": false,
                "tip": tip(0, None, 0, json!({ "lazy": true, "lazy_radius": 50.0 })) }),
        json!({ "name": "Rivets", "tool": "Chisel", "radius": 20.0, "intensity": 0.4, "negative": false,
                "tip": tip(2, None, 0, json!({ "chisel": 0 })) }),
        json!({ "name": "Stitched seam", "tool": "Draw", "radius": 22.0, "intensity": 0.35, "negative": true,
                "tip": tip(0, Some("Stitches"), 4, json!({ "spacing": 0.9, "alpha_follow": true })) }),
    ]
}

fn load_presets() -> Vec<Value> {
    storage()
        .and_then(|s| s.get_item(PRESETS_KEY).ok().flatten())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_else(starter_presets)
}

fn store_presets(list: &[Value]) {
    if let Some(s) = storage() {
        let _ = s.set_item(PRESETS_KEY, &Value::Array(list.to_vec()).to_string());
    }
}

// ------------------------------------------------------------------ curve editor

const CW: f32 = 220.0;
const CH: f32 = 110.0;
const PAD: f32 = 8.0;

fn to_svg(p: Vec2) -> (f32, f32) {
    (PAD + p.x * CW, PAD + (1.0 - p.y) * CH)
}

#[component]
fn CurveEditor(ui: Ui) -> impl IntoView {
    let dragging = RwSignal::new(None::<usize>);
    let svg_ref = NodeRef::<leptos::svg::Svg>::new();
    let local = move |ev: &ev::PointerEvent| -> Option<Vec2> {
        let el: web_sys::Element = svg_ref.get_untracked()?.unchecked_into();
        let r = el.get_bounding_client_rect();
        // the SVG scales with the panel width
        let sx = (CW + 2.0 * PAD) / r.width().max(1.0) as f32;
        let x = (ev.client_x() as f32 - r.left() as f32) * sx;
        let y = (ev.client_y() as f32 - r.top() as f32) * sx;
        Some(Vec2::new((x - PAD) / CW, 1.0 - (y - PAD) / CH).clamp(Vec2::ZERO, Vec2::ONE))
    };
    let set_point = move |i: usize, mut p: Vec2| {
        ui.falloff.update(|c| {
            let n = c.points.len();
            if i == 0 {
                p.x = 0.0;
            } else if i == n - 1 {
                p.x = 1.0;
            } else {
                p.x = p.x.clamp(c.points[i - 1].x + 0.01, c.points[i + 1].x - 0.01);
            }
            c.points[i] = p;
        });
    };
    let on_down = move |ev: ev::PointerEvent| {
        ev.prevent_default();
        let Some(p) = local(&ev) else { return };
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        let c = ui.falloff.get_untracked();
        // grab the nearest point, or add one on the curve here
        let near = c.points.iter().enumerate().map(|(i, q)| (i, ((*q - p) * Vec2::new(CW, CH)).length())).min_by(|a, b| a.1.total_cmp(&b.1));
        match near {
            Some((i, d)) if d < 10.0 => dragging.set(Some(i)),
            _ => {
                let mut c = c;
                c.points.push(p);
                c.tidy();
                let i = c.points.iter().position(|q| (q.x - p.x).abs() < 1e-4).unwrap_or(0);
                ui.falloff.set(c);
                dragging.set(Some(i));
            }
        }
    };
    let on_move = move |ev: ev::PointerEvent| {
        if let (Some(i), Some(p)) = (dragging.get_untracked(), local(&ev)) {
            set_point(i, p);
        }
    };
    let on_up = move |_| dragging.set(None);
    let on_dbl = move |ev: ev::MouseEvent| {
        let Some(el) = svg_ref.get_untracked() else { return };
        let el: web_sys::Element = el.unchecked_into();
        let r = el.get_bounding_client_rect();
        let sx = (CW + 2.0 * PAD) / r.width().max(1.0) as f32;
        let p = Vec2::new(
            ((ev.client_x() as f32 - r.left() as f32) * sx - PAD) / CW,
            1.0 - ((ev.client_y() as f32 - r.top() as f32) * sx - PAD) / CH,
        );
        ui.falloff.update(|c| {
            let n = c.points.len();
            if let Some(i) = (1..n.saturating_sub(1)).find(|&i| ((c.points[i] - p) * Vec2::new(CW, CH)).length() < 10.0) {
                c.points.remove(i);
            }
        });
    };
    let path = move || {
        let c = ui.falloff.get();
        (0..=64)
            .map(|k| {
                let x = k as f32 / 64.0;
                let (sx, sy) = to_svg(Vec2::new(x, c.eval(x)));
                format!("{}{sx:.1},{sy:.1}", if k == 0 { "M" } else { "L" })
            })
            .collect::<String>()
    };
    let points = move || {
        ui.falloff
            .get()
            .points
            .iter()
            .map(|p| {
                let (x, y) = to_svg(*p);
                view! { <circle class="curve-pt" cx=x cy=y r="4.5" /> }
            })
            .collect_view()
    };
    let view_box = format!("0 0 {} {}", CW + 2.0 * PAD, CH + 2.0 * PAD);
    let (gx0, gy0) = to_svg(Vec2::ZERO);
    let (gx1, gy1) = to_svg(Vec2::ONE);
    let grid_w = gx1 - gx0;
    let grid_h = gy0 - gy1;
    view! {
        <svg
            node_ref=svg_ref
            class="curve-editor"
            viewBox=view_box
            on:pointerdown=on_down
            on:pointermove=on_move
            on:pointerup=on_up
            on:pointercancel=on_up
            on:dblclick=on_dbl
        >
            <rect class="curve-bg" x=gx0 y=gy1 width=grid_w height=grid_h />
            <line class="curve-grid" x1=gx0 y1=(gy0 + gy1) / 2.0 x2=gx1 y2=(gy0 + gy1) / 2.0 />
            <line class="curve-grid" x1=(gx0 + gx1) / 2.0 y1=gy1 x2=(gx0 + gx1) / 2.0 y2=gy0 />
            <path class="curve-line" d=path />
            {points}
        </svg>
    }
}

// ------------------------------------------------------------------ panels

#[component]
pub fn BrushTipFolder(ui: Ui) -> impl IntoView {
    let st = ui.stroke;
    let kind = move || st.get().kind;
    let tool = move || ui.tool.get();
    let chisel_tool = move || ui.tool.get() == Tool::Chisel;
    let stroke_hint = move || match (tool(), kind()) {
        (t, _) if t.is_drag() || t.is_curve() => "This brush ignores the stroke type.",
        (Tool::Chisel, _) => "Chisel: press where the shape goes, drag out its size and turn.",
        (_, StrokeKind::Freehand) => "Even dabs along your stroke.",
        (_, StrokeKind::Dots) => "A dab at each pointer position: fast strokes leave gaps.",
        (_, StrokeKind::DragRect) => "Press where the stamp goes, drag out its size and turn, release to keep it.",
        (_, StrokeKind::Spray) => "Scattered dabs that vary in size, strength and turn.",
    };
    let curve_name = move || ui.falloff.get().preset_name().unwrap_or("Custom").to_string();
    let alpha_name = move || ui.alpha.get().map_or("None".to_string(), |a| a.name.clone());
    let tiles = move || {
        let current = ui.alpha.get();
        let mut all: Vec<Arc<Alpha>> = BUILTINS.with(|b| b.clone());
        all.extend(ui.user_alphas.get());
        let none_on = current.is_none();
        let none = view! {
            <button class="alpha-tile none" class:on=none_on title="No alpha" on:click=move |_| ui.alpha.set(None)>"Off"</button>
        };
        let rest = all
            .into_iter()
            .map(|a| {
                let on = current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &a) || **c == *a);
                let src = THUMBS.with(|t| t.borrow_mut().entry(a.name.clone() + &a.w.to_string()).or_insert_with(|| thumb(&a)).clone());
                let name = a.name.clone();
                let pick = a.clone();
                view! {
                    <button class="alpha-tile" class:on=on title=name on:click=move |_| ui.alpha.set(Some(pick.clone()))>
                        <img src=src alt="" />
                    </button>
                }
            })
            .collect_view();
        view! { {none}{rest} }
    };
    view! {
        <Folder title="Stroke & alpha" closed=true>
            <div class="row">
                <span class="label">"Stroke"</span>
                <div class="segmented wrap">
                    {StrokeKind::ALL
                        .iter()
                        .map(|k| {
                            let k = *k;
                            view! {
                                <button class:on=move || kind() == k on:click=move |_| st.update(|s| s.kind = k)>{k.name()}</button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="row info">{stroke_hint}</div>
            <Show when=move || matches!(kind(), StrokeKind::Freehand | StrokeKind::Spray)>
                <Slider label="Spacing" min=0.02 max=1.5 step=0.01
                    value=Signal::derive(move || st.get().spacing)
                    on_change=move |v| st.update(|s| s.spacing = v)
                    format=|v| format!("{:.0}%", v * 100.0) />
            </Show>
            <Show when=move || kind() == StrokeKind::Spray>
                <Slider label="Scatter" min=0.0 max=2.0 step=0.01
                    value=Signal::derive(move || st.get().scatter)
                    on_change=move |v| st.update(|s| s.scatter = v) />
                <Slider label="Variation" min=0.0 max=1.0 step=0.01
                    value=Signal::derive(move || st.get().jitter)
                    on_change=move |v| st.update(|s| s.jitter = v) />
            </Show>
            <Show when=move || kind() == StrokeKind::Freehand>
                <Check label="Lazy mouse (smooth lines)" value=Signal::derive(move || st.get().lazy) on_change=move |v| st.update(|s| s.lazy = v) />
                <Show when=move || st.get().lazy>
                    <Slider label="Lazy radius" min=4.0 max=200.0 step=1.0
                        value=Signal::derive(move || st.get().lazy_radius)
                        on_change=move |v| st.update(|s| s.lazy_radius = v)
                        format=|v| format!("{v:.0} px") />
                </Show>
            </Show>
            <div class="subhead">"Pen pressure"</div>
            <Check label="Pressure changes the size" value=Signal::derive(move || st.get().pressure_size) on_change=move |v| st.update(|s| s.pressure_size = v) />
            <Check label="Pressure changes the strength" value=Signal::derive(move || st.get().pressure_intensity) on_change=move |v| st.update(|s| s.pressure_intensity = v) />
            <div class="row info">{move || {
                let p = ui.pressure.get();
                if p.0 { format!("Pen detected, pressure {:.0}%", p.1 * 100.0) } else { "Works with drawing tablets and pens; a mouse always presses fully.".to_string() }
            }}</div>
            <Check label="Backface masking (spare the far side of thin parts)" value=Signal::derive(move || st.get().backface) on_change=move |v| st.update(|s| s.backface = v) />

            <div class="subhead">"Falloff"</div>
            <CurveEditor ui />
            <div class="row">
                <span class="label">"Shape"</span>
                <select on:change=move |ev| {
                    if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                        ui.falloff.set(FalloffCurve::preset(i));
                    }
                }>
                    <option value="" disabled=true selected=move || curve_name() == "Custom">"Custom"</option>
                    {CURVE_PRESETS
                        .iter()
                        .enumerate()
                        .map(|(i, (n, _))| {
                            let n = *n;
                            view! { <option value=i.to_string() selected=move || curve_name() == n>{n}</option> }
                        })
                        .collect_view()}
                </select>
            </div>
            <div class="row info">"Drag the points; click the line to add one, double-click a point to remove it. Left is the brush centre, right its edge."</div>

            <Show when=chisel_tool fallback=move || view! {
                <div class="subhead">{move || format!("Alpha: {}", alpha_name())}</div>
                <div class="alpha-grid">{tiles}</div>
                <div class="row buttons tight">
                    <button on:click=move |_| spawn_local(load_alpha_image(ui))>"Load image…"</button>
                </div>
                <Show when=move || ui.alpha.get().is_some()>
                    <Slider label="Turn" min=-180.0 max=180.0 step=1.0
                        value=Signal::derive(move || st.get().alpha_angle)
                        on_change=move |v| st.update(|s| s.alpha_angle = v)
                        format=|v| format!("{v:.0}°") />
                    <Check label="Turn with the stroke" value=Signal::derive(move || st.get().alpha_follow) on_change=move |v| st.update(|s| s.alpha_follow = v) />
                </Show>
                <div class="row info">"White raises (or carves, with Negative), black leaves the surface alone. Alphas look best with Drag Rect or Spray on a dense mesh."</div>
            }>
                <div class="subhead">"Chisel shape"</div>
                <div class="segmented wrap">
                    {ChiselShape::ALL
                        .iter()
                        .map(|c| {
                            let c = *c;
                            view! {
                                <button class:on=move || st.get().chisel == c on:click=move |_| st.update(|s| s.chisel = c)>{c.name()}</button>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="row info">"Intensity sets the depth; Ctrl presses the shape in. Needs a dense mesh for crisp edges."</div>
            </Show>
        </Folder>
    }
}

thread_local! {
    static THUMBS: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
}

#[component]
pub fn PresetsFolder(ui: Ui) -> impl IntoView {
    let presets = RwSignal::new(load_presets());
    let name = RwSignal::new(String::new());
    let save = move || {
        let n = name.get_untracked().trim().to_string();
        let n = if n.is_empty() { format!("{} brush", ui.tool.get_untracked().name()) } else { n };
        let p = preset_json(ui, &n);
        presets.update(|l| {
            l.retain(|x| x.get("name").and_then(|v| v.as_str()) != Some(&n));
            l.insert(0, p);
        });
        store_presets(&presets.get_untracked());
        name.set(String::new());
        ui.status.set(format!("Saved brush preset {n}"));
    };
    let export = move |i: usize| {
        let Some(p) = presets.get_untracked().get(i).cloned() else { return };
        let n = p.get("name").and_then(|v| v.as_str()).unwrap_or("brush").to_string();
        let file: String = n.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        spawn_local(async move {
            let bytes = serde_json::to_vec_pretty(&p).unwrap_or_default();
            match host::save_file(&format!("{file}.claybrush"), "claybrush", "Export brush preset", bytes).await {
                Ok(Some(w)) => ui.status.set(format!("Brush preset saved to {w}")),
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't save: {e}")),
            }
        });
    };
    let import = move |_| {
        spawn_local(async move {
            match host::open_brush().await {
                Ok(Some(f)) => match serde_json::from_slice::<Value>(&f.bytes) {
                    Ok(p) if p.get("tool").is_some() => {
                        let n = p.get("name").and_then(|v| v.as_str()).unwrap_or("Imported").to_string();
                        presets.update(|l| {
                            l.retain(|x| x.get("name").and_then(|v| v.as_str()) != Some(&n));
                            l.insert(0, p.clone());
                        });
                        store_presets(&presets.get_untracked());
                        apply_preset(ui, &p);
                    }
                    _ => ui.status.set("That isn't a Clayform brush preset".into()),
                },
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Couldn't open: {e}")),
            }
        });
    };
    view! {
        <Folder title="Brush presets" closed=true>
            <div class="preset-list">
                {move || presets
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let n = p.get("name").and_then(|v| v.as_str()).unwrap_or("Brush").to_string();
                        let tool = p.get("tool").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let pc = p.clone();
                        let tip = format!("Load ({tool})");
                        view! {
                            <div class="preset-row">
                                <button class="preset-name" title=tip on:click=move |_| apply_preset(ui, &pc)>
                                    {n}<span class="preset-tool">{tool}</span>
                                </button>
                                <button class="icon" title="Export to a .claybrush file to share" on:click=move |_| export(i)>"⤓"</button>
                                <button class="icon" title="Delete" on:click=move |_| {
                                    presets.update(|l| { if i < l.len() { l.remove(i); } });
                                    store_presets(&presets.get_untracked());
                                }>"✕"</button>
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
            <div class="row">
                <input type="text" placeholder="Preset name" prop:value=move || name.get()
                    on:input=move |ev| name.set(event_target_value(&ev))
                    on:keydown=move |ev: ev::KeyboardEvent| { ev.stop_propagation(); if ev.key() == "Enter" { save(); } } />
            </div>
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| save()>"Save current brush"</button>
                <button on:click=import>"Import…"</button>
            </div>
            <div class="row buttons tight">
                <button on:click=move |_| { presets.set(starter_presets()); store_presets(&presets.get_untracked()); }>"Restore starters"</button>
            </div>
            <div class="row info">"A preset keeps the brush, size, strength, stroke, falloff and alpha. Export (⤓) writes a .claybrush file you can share; Import loads one."</div>
        </Folder>
    }
}
