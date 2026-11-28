//! Polypaint and texturing panels: colour, fill, pick, Spotlight from a
//! reference image, UV unwrapping and baking maps.

use crate::{end_job, host, sleep_ms, start_job, with_ed, Check, Folder, Slider, Ui};
use base64::Engine as _;
use clayform_core::bake::{BakeSettings, Baker};
use clayform_core::glam::{Vec2, Vec3};
use clayform_core::paint::Spotlight;
use clayform_core::sculpt::Tool;
use clayform_core::uv::UnwrapSettings;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::sync::Arc;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn hex(c: Vec3) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.x), b(c.y), b(c.z))
}

fn parse_hex(s: &str) -> Option<Vec3> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some(Vec3::new(n(0)?, n(2)?, n(4)?))
}

thread_local! {
    /// Decoded reference images for Spotlight: (ref id, src length) -> pixels.
    static SPOT_CACHE: std::cell::RefCell<Option<(u32, usize, Arc<(usize, usize, Vec<u8>)>)>> = const { std::cell::RefCell::new(None) };
}

fn decode_ref(id: u32, src: &str) -> Option<Arc<(usize, usize, Vec<u8>)>> {
    if let Some(c) = SPOT_CACHE.with(|c| c.borrow().clone()).filter(|c| c.0 == id && c.1 == src.len()) {
        return Some(c.2);
    }
    let data = src.split_once(',')?.1;
    let bytes = B64.decode(data).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width().max(img.height()) > 1024 { img.resize(1024, 1024, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let px = Arc::new((rgba.width() as usize, rgba.height() as usize, rgba.into_raw()));
    SPOT_CACHE.with(|c| *c.borrow_mut() = Some((id, src.len(), px.clone())));
    Some(px)
}

/// Keeps the engine's Spotlight image and its place on screen up to date.
fn sync_spotlight(ui: Ui) {
    let on = ui.spot_on.get();
    let id = ui.spot_ref.get();
    let refs = ui.refs.get();
    let r = refs.iter().find(|r| Some(r.id) == id).or_else(|| refs.first());
    let spot = match (on, r) {
        (true, Some(r)) => decode_ref(r.id, &r.src).map(|px| {
            let (w, h, ref rgba) = *px;
            let height = r.w * h as f32 / w.max(1) as f32;
            Arc::new(Spotlight { w, h, rgba: rgba.clone(), rect: [r.x, r.y, r.w, height] })
        }),
        _ => None,
    };
    let active = on && spot.is_some();
    with_ed(|e| {
        e.spotlight = spot;
        e.spotlight_on = active;
    });
}

/// In colour-pick mode, a click on the model takes its colour. Returns
/// true when the click was used.
pub fn try_pick(ui: Ui, p: Vec2, size: Vec2) -> bool {
    if !ui.picking.get_untracked() {
        return false;
    }
    ui.picking.set(false);
    let c = with_ed(|e| {
        let (o, d) = e.camera.ray(p, size);
        e.sculptor.raycast(o, d).and_then(|h| e.sculptor.paint_at(h.tri, h.point))
    });
    match c {
        Some(c) => {
            ui.paint_color.set(c);
            ui.status.set(format!("Picked {}", hex(c)));
        }
        None => ui.status.set("No paint there".into()),
    }
    true
}

#[component]
pub fn PaintFolder(ui: Ui) -> impl IntoView {
    Effect::new(move || {
        let c = ui.paint_color.get();
        with_ed(|e| e.paint_color = c);
    });
    Effect::new(move || sync_spotlight(ui));
    let swatches = [
        Vec3::new(0.85, 0.2, 0.15),
        Vec3::new(0.95, 0.6, 0.2),
        Vec3::new(0.95, 0.85, 0.3),
        Vec3::new(0.3, 0.65, 0.3),
        Vec3::new(0.2, 0.45, 0.85),
        Vec3::new(0.45, 0.3, 0.7),
        Vec3::new(0.9, 0.72, 0.6),
        Vec3::new(0.35, 0.22, 0.15),
        Vec3::new(0.08, 0.08, 0.08),
        Vec3::new(0.95, 0.95, 0.95),
    ];
    view! {
        <Folder title="Paint" closed=true>
            <div class="row">
                <span class="label">"Colour"</span>
                <input type="color" class="color-input" prop:value=move || hex(ui.paint_color.get())
                    on:input=move |ev| if let Some(c) = parse_hex(&event_target_value(&ev)) { ui.paint_color.set(c) } />
                <button class:on=move || ui.picking.get() title="Click on the model to take its colour"
                    on:click=move |_| ui.picking.update(|p| *p = !*p)>"Pick"</button>
            </div>
            <div class="swatches">
                {swatches.into_iter().map(|c| view! {
                    <button class="swatch" style=format!("background:{}", hex(c)) title=hex(c) on:click=move |_| ui.paint_color.set(c)></button>
                }).collect_view()}
            </div>
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| { ui.tool.set(Tool::Paint); ui.mode.set(clayform_core::editor::Mode::Sculpt); }>"Paint brush"</button>
                <button title="Paint everything that isn't masked" on:click=move |_| {
                    let c = ui.paint_color.get_untracked();
                    with_ed(|e| { e.sculptor.fill_color(c); e.redraw = true; });
                    ui.refresh();
                }>"Fill"</button>
                <button on:click=move |_| { with_ed(|e| { e.sculptor.clear_paint(); e.redraw = true; }); ui.refresh(); }>"Clear paint"</button>
            </div>
            <Check label="Show polypaint" value=Signal::derive(move || ui.view.get().paint) on_change=move |v| ui.view.update(|s| s.paint = v) />
            <div class="row info">"The Paint brush uses its radius, intensity, falloff and alpha; Ctrl erases. Masked areas aren't painted. Paint is kept per level, through remeshes, and in projects, and exports as vertex colours (OBJ, PLY, glTF)."</div>

            <div class="subhead">"Spotlight (paint from a reference image)"</div>
            <Show when=move || !ui.refs.get().is_empty() fallback=|| view! {
                <div class="row info">"Add a reference image (Reference images folder), place it over the model, then paint through it."</div>
            }>
                <div class="row">
                    <span class="label">"Image"</span>
                    <select on:change=move |ev| ui.spot_ref.set(event_target_value(&ev).parse().ok())>
                        {move || ui.refs.get().into_iter().map(|r| {
                            let id = r.id;
                            view! { <option value=id.to_string() selected=move || ui.spot_ref.get() == Some(id)>{r.name}</option> }
                        }).collect_view()}
                    </select>
                </div>
                <Check label="Paint brush takes colours from the image" value=ui.spot_on on_change=move |v| {
                    ui.spot_on.set(v);
                    if v { ui.tool.set(Tool::Paint); }
                } />
                <div class="row info">"Each point is painted with the image colour that lies over it on screen. Lower the image's opacity to see the model through it."</div>
            </Show>
        </Folder>
    }
}

// ------------------------------------------------------------------ texture

/// A ZIP archive (stored, no compression) of named files.
fn zip(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let off = out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date (1980-01-01)
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
                h.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                h.extend_from_slice(&off.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_off = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn png_rgb(w: usize, data: Vec<u8>, channels: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let r = match channels {
        3 => image::RgbImage::from_raw(w as u32, w as u32, data).map(image::DynamicImage::ImageRgb8),
        4 => image::RgbaImage::from_raw(w as u32, w as u32, data).map(image::DynamicImage::ImageRgba8),
        _ => image::GrayImage::from_raw(w as u32, w as u32, data).map(image::DynamicImage::ImageLuma8),
    };
    if let Some(img) = r {
        let _ = img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    }
    buf
}

fn png16(w: usize, data: Vec<u16>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(img) = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(w as u32, w as u32, data) {
        let _ = image::DynamicImage::ImageLuma16(img).write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    }
    buf
}

async fn bake(ui: Ui, settings: BakeSettings) {
    if ui.busy.get_untracked() {
        return;
    }
    let pair = with_ed(|e| {
        e.cancel_drag();
        e.sculptor.bake_meshes()
    });
    let (low, high) = match pair {
        Ok(p) => p,
        Err(m) => {
            ui.status.set(m);
            return;
        }
    };
    start_job(ui, "Baking maps");
    sleep_ms(30).await;
    let obj = clayform_core::bake::obj_with_uvs(&low);
    let mut b = match Baker::new(low, high, &settings) {
        Ok(b) => b,
        Err(m) => {
            ui.status.set(m);
            end_job(ui);
            return;
        }
    };
    let t0 = crate::now_ms();
    loop {
        let start = crate::now_ms();
        let mut done = false;
        while crate::now_ms() - start < 30.0 {
            if b.step(256) {
                done = true;
                break;
            }
        }
        ui.job.set(Some(("Baking maps", b.progress() * 0.9)));
        if done {
            break;
        }
        if !ui.busy.get_untracked() {
            ui.status.set("Baking cancelled".into());
            return;
        }
        sleep_ms(0).await;
    }
    let maps = b.finish();
    let n = maps.size;
    let mut files: Vec<(&str, Vec<u8>)> = vec![
        ("clayform_low.obj", obj.into_bytes()),
        ("clayform_bake.mtl", clayform_core::bake::mtl(maps.color.is_some()).into_bytes()),
        ("clayform_normal.png", png_rgb(n, maps.normal, 3)),
        ("clayform_displacement.png", png16(n, maps.displacement)),
    ];
    if let Some(c) = maps.color {
        files.push(("clayform_color.png", png_rgb(n, c, 4)));
    }
    let readme = format!(
        "Clayform bake ({n} x {n})\n\nclayform_low.obj: the unwrapped level\nclayform_normal.png: tangent-space normal map (OpenGL, +Y up)\nclayform_displacement.png: 16-bit displacement; mid-grey (32768) = on the surface, white = +{:.5}, black = -{:.5} model units\n{}",
        maps.disp_range / low_scale(),
        maps.disp_range / low_scale(),
        if files.len() > 4 { "clayform_color.png: polypaint\n" } else { "" }
    );
    files.push(("README.txt", readme.into_bytes()));
    let bytes = zip(&files);
    ui.job.set(Some(("Saving", 0.95)));
    let name = format!("{}_maps.zip", ui.project_name.get_untracked());
    match host::save_file(&name, "zip", "Save baked maps", bytes).await {
        Ok(Some(w)) => ui.status.set(format!("Baked {n}×{n} maps in {:.1} s → {w}", (crate::now_ms() - t0) / 1000.0)),
        Ok(None) => ui.status.set("Bake not saved".into()),
        Err(e) => ui.status.set(format!("Couldn't save the maps: {e}")),
    }
    end_job(ui);
}

/// Export scale of the active sculpt (displacement is reported in export units).
fn low_scale() -> f32 {
    with_ed(|e| e.sculptor.mesh.export_scale).max(1e-9)
}

#[component]
pub fn TextureFolder(ui: Ui) -> impl IntoView {
    let angle = RwSignal::new(UnwrapSettings::default().max_angle);
    let by_groups = RwSignal::new(false);
    let size = RwSignal::new(1024usize);
    let reach = RwSignal::new(0.03f32);
    let layout = RwSignal::new(None::<String>);
    let uv_info = RwSignal::new(String::new());
    let refresh_layout = move || {
        let (img, info) = with_ed(|e| {
            let lvl = e.sculptor.uv_level();
            let img = e.sculptor.has_uvs().then(|| clayform_core::uv::layout_image(&e.sculptor.mesh, 256));
            let info = match lvl {
                Some(l) if l == e.sculptor.level() => format!("UVs on this level ({} triangles)", e.sculptor.mesh.tris.len()),
                Some(l) => format!("UVs are on level {}", l + 1),
                None => "Not unwrapped yet".to_string(),
            };
            (img, info)
        });
        layout.set(img.map(|i| format!("data:image/png;base64,{}", B64.encode(png_rgb(256, i, 1)))));
        uv_info.set(info);
    };
    Effect::new(move || {
        let _ = (ui.rev.get(), ui.level.get());
        refresh_layout();
    });
    view! {
        <Folder title="Texture" closed=true>
            <div class="subhead">"UV unwrap"</div>
            <Slider label="Chart angle" min=20.0 max=90.0 step=1.0
                value=angle on_change=move |v| angle.set(v)
                format=|v| format!("{v:.0}°") />
            <Check label="Split charts at polygroups" value=by_groups on_change=move |v| by_groups.set(v) />
            <div class="row buttons tight">
                <button class="primary" on:click=move |_| {
                    let s = UnwrapSettings { max_angle: angle.get_untracked(), by_groups: by_groups.get_untracked(), ..Default::default() };
                    let n = with_ed(|e| { let n = e.sculptor.unwrap_uvs(&s); e.redraw = true; n });
                    ui.refresh();
                    refresh_layout();
                    ui.status.set(format!("Unwrapped into {n} charts"));
                }>"Unwrap"</button>
            </div>
            <div class="row info">{move || uv_info.get()}</div>
            {move || layout.get().map(|src| view! { <img class="uv-layout" src=src alt="UV layout" /> })}
            <div class="row info">"Unwrap the lowest level (or a QRemeshed base) before subdividing; higher levels inherit its UVs. A wider chart angle makes fewer, more stretched charts; polygroups let you choose where seams go."</div>

            <div class="subhead">"Bake maps"</div>
            <div class="row">
                <span class="label">"Size"</span>
                <div class="segmented">
                    {[512usize, 1024, 2048, 4096].into_iter().map(|s| view! {
                        <button class:on=move || size.get() == s on:click=move |_| size.set(s)>{s.to_string()}</button>
                    }).collect_view()}
                </div>
            </div>
            <Slider label="Reach" min=0.005 max=0.15 step=0.005
                value=reach on_change=move |v| reach.set(v)
                format=|v| format!("{:.1}%", v * 100.0) />
            <div class="row buttons tight">
                <button class="primary" disabled=move || ui.busy.get() on:click=move |_| {
                    let s = BakeSettings { size: size.get_untracked(), reach: reach.get_untracked() };
                    spawn_local(bake(ui, s));
                }>"Bake and save…"</button>
            </div>
            <div class="row info">"Bakes the highest level onto the unwrapped level: a tangent-space normal map, a 16-bit displacement map and (if painted) a colour map, saved with the low mesh (OBJ + MTL) in a ZIP. Reach is how far from the low surface detail is looked for."</div>
        </Folder>
    }
}
