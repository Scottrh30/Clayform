//! Rendering and output: lights, rendered images, turntables, mesh export
//! and 3D printing.

use super::*;
use clayform_core::print::{self, Unit};
use render::{DepthOfField, LIGHT_NAMES};
use std::cell::Cell;

thread_local! {
    /// Set by the progress card's Cancel button for jobs running in the page.
    static CANCEL: Cell<bool> = const { Cell::new(false) };
}

pub fn cancel_local_job() {
    CANCEL.with(|c| c.set(true));
}

fn cancelled() -> bool {
    CANCEL.with(|c| c.get())
}

/// Image size presets: (label, width, height); 0 × 0 = the viewport's size.
pub const IMAGE_SIZES: [(&str, u32, u32); 8] = [
    ("Viewport size", 0, 0),
    ("Viewport × 2", 1, 1),
    ("1280 × 720", 1280, 720),
    ("1920 × 1080 (HD)", 1920, 1080),
    ("2560 × 1440", 2560, 1440),
    ("3840 × 2160 (4K)", 3840, 2160),
    ("1080 × 1080 square", 1080, 1080),
    ("2048 × 2048 square", 2048, 2048),
];

pub const TURNTABLE_SIZES: [(&str, u32, u32); 5] = [
    ("512 × 512", 512, 512),
    ("1080 × 1080", 1080, 1080),
    ("1280 × 720", 1280, 720),
    ("1920 × 1080 (HD)", 1920, 1080),
    ("640 × 360", 640, 360),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TurnFormat {
    Video,
    Gif,
    PngZip,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderSettings {
    pub size: usize,
    pub transparent: bool,
    pub dof: bool,
    /// Depth-of-field strength 0..1.
    pub blur: f32,
    pub turn_size: usize,
    pub turn_frames: u32,
    pub turn_fps: u32,
    pub turn_format: TurnFormat,
    pub export_format: usize,
    pub print_size: f32,
    pub print_unit: Unit,
    /// Export every visible SubTool (not just the active one).
    pub export_all: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            size: 3,
            transparent: false,
            dof: false,
            blur: 0.4,
            turn_size: 0,
            turn_frames: 120,
            turn_fps: 30,
            turn_format: TurnFormat::Video,
            export_format: 0,
            print_size: 100.0,
            print_unit: Unit::Mm,
            export_all: true,
        }
    }
}

fn png(w: u32, h: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Renders the sculpt off screen, the camera turned `turn` radians about
/// its target. Returns RGBA rows.
async fn render_rgba(ui: Ui, size: [u32; 2], turn: f32, rs: &RenderSettings) -> Result<Vec<u8>, String> {
    let mut r = RENDERER.with(|r| r.borrow_mut().take()).ok_or("The 3D view isn't ready")?;
    let view = ui.view.get_untracked();
    let (cam_view, proj, far, dof, ao) = with_ed(|e| {
        sync_scene(&mut r, e, &view);
        let mut cam = e.camera.clone();
        cam.yaw += turn;
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        // focus on whatever is under the centre of the image
        let focus = e
            .sculptor
            .raycast(cam.eye(), cam.forward())
            .map_or(cam.dist, |h| (h.point - cam.eye()).dot(cam.forward()));
        let dof = rs.dof.then_some(DepthOfField { focus, amount: rs.blur * 0.025 });
        (cam.view(), cam.proj(aspect), cam.dist * 100.0, dof, cam.dist * 0.06)
    });
    let res = r.render_image(size, cam_view, proj, far, &view, ao, dof, rs.transparent).await;
    RENDERER.with(|slot| *slot.borrow_mut() = Some(r));
    with_ed(|e| e.redraw = true);
    res
}

fn image_size(ui: Ui, preset: usize) -> [u32; 2] {
    let (_, w, h) = IMAGE_SIZES[preset.min(IMAGE_SIZES.len() - 1)];
    let (vw, vh) = ui.vp_size.get_untracked();
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0) as f32;
    let (vw, vh) = ((vw * dpr).round().max(16.0) as u32, (vh * dpr).round().max(16.0) as u32);
    match (w, h) {
        (0, 0) => [vw, vh],
        (1, 1) => [vw * 2, vh * 2],
        _ => [w, h],
    }
}

pub async fn render_still(ui: Ui, rs: RenderSettings) {
    if ui.busy.get_untracked() {
        ui.status.set("Please wait for the current job to finish (or cancel it)".into());
        return;
    }
    let size = image_size(ui, rs.size);
    start_job(ui, "Rendering");
    sleep_ms(30).await;
    let t0 = now_ms();
    let result = render_rgba(ui, size, 0.0, &rs).await.and_then(|px| png(size[0], size[1], px));
    end_job(ui);
    match result {
        Ok(bytes) => {
            let secs = (now_ms() - t0) / 1000.0;
            match host::save_file("render.png", "png", "Save render", bytes).await {
                Ok(Some(p)) => ui.status.set(format!("Rendered {}×{} in {secs:.1} s — saved {p}", size[0], size[1])),
                Ok(None) => ui.status.set("Render not saved".into()),
                Err(e) => ui.status.set(format!("Save failed: {e}")),
            }
        }
        Err(e) => ui.status.set(format!("Render failed: {e}")),
    }
}

/// A ZIP archive with stored (uncompressed) entries.
fn zip_store(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
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
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend(header(0x0201_4b50, true));
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Plays PNG frames into a canvas at `fps` while a MediaRecorder records it.
async fn record_video(ui: Ui, frames: &[Vec<u8>], w: u32, h: u32, fps: u32) -> Result<(Vec<u8>, &'static str), String> {
    use js_sys::{Array, Uint8Array};
    use std::rc::Rc;
    use wasm_bindgen_futures::JsFuture;
    let err = |e: JsValue| e.as_string().unwrap_or_else(|| format!("{e:?}"));
    let window = web_sys::window().ok_or("no window")?;
    let doc = window.document().ok_or("no document")?;
    let mimes = [
        ("video/mp4;codecs=avc1.640028", "mp4"),
        ("video/mp4", "mp4"),
        ("video/webm;codecs=vp9", "webm"),
        ("video/webm", "webm"),
    ];
    let (mime, ext) = mimes
        .iter()
        .copied()
        .find(|(m, _)| web_sys::MediaRecorder::is_type_supported(m))
        .ok_or("This window can't record video; use GIF or PNG frames instead")?;
    let canvas: web_sys::HtmlCanvasElement =
        doc.create_element("canvas").map_err(err)?.dyn_into().map_err(|_| "not a canvas")?;
    canvas.set_width(w);
    canvas.set_height(h);
    let _ = canvas.set_attribute("style", "position:fixed;left:-100000px;top:0");
    doc.body().ok_or("no body")?.append_child(&canvas).map_err(err)?;
    let ctx: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(err)?
        .ok_or("no 2D canvas")?
        .dyn_into()
        .map_err(|_| "no 2D canvas")?;
    let stream = canvas.capture_stream_with_frame_request_rate(0.0).map_err(err)?;
    let track: web_sys::CanvasCaptureMediaStreamTrack =
        stream.get_video_tracks().get(0).dyn_into().map_err(|_| "no video track")?;
    let opts = web_sys::MediaRecorderOptions::new();
    opts.set_mime_type(mime);
    let bitrate = ((w as f64 * h as f64 * fps as f64) * 0.25).clamp(2e6, 4e7) as u32;
    opts.set_video_bits_per_second(bitrate);
    let rec = web_sys::MediaRecorder::new_with_media_stream_and_media_recorder_options(&stream, &opts).map_err(err)?;
    let chunks: Rc<RefCell<Vec<web_sys::Blob>>> = Default::default();
    let c2 = chunks.clone();
    let on_data = Closure::<dyn FnMut(web_sys::BlobEvent)>::new(move |e: web_sys::BlobEvent| {
        if let Some(b) = e.data() {
            c2.borrow_mut().push(b);
        }
    });
    rec.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
    let stopped = js_sys::Promise::new(&mut |resolve, _| {
        rec.set_onstop(Some(&resolve));
    });
    rec.start_with_time_slice(1000).map_err(err)?;
    let frame_ms = 1000.0 / fps as f64;
    let t0 = now_ms();
    for (i, f) in frames.iter().enumerate() {
        if cancelled() {
            break;
        }
        let parts = Array::of1(&Uint8Array::from(f.as_slice()));
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type("image/png");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &bag).map_err(err)?;
        let bmp: web_sys::ImageBitmap = JsFuture::from(window.create_image_bitmap_with_blob(&blob).map_err(err)?)
            .await
            .map_err(err)?
            .dyn_into()
            .map_err(|_| "not an image")?;
        // keep to the frame clock so the video plays at the right speed
        let due = t0 + i as f64 * frame_ms;
        let wait = due - now_ms();
        if wait > 1.0 {
            sleep_ms(wait as i32).await;
        }
        ctx.draw_image_with_image_bitmap(&bmp, 0.0, 0.0).map_err(err)?;
        track.request_frame();
        ui.job.set(Some(("Encoding video", 0.75 + 0.25 * i as f32 / frames.len() as f32)));
    }
    sleep_ms(frame_ms as i32).await;
    rec.stop().map_err(err)?;
    JsFuture::from(stopped).await.map_err(err)?;
    canvas.remove();
    drop(on_data);
    let parts = Array::new();
    for b in chunks.borrow().iter() {
        parts.push(b);
    }
    let all = web_sys::Blob::new_with_blob_sequence(&parts).map_err(err)?;
    let buf = JsFuture::from(all.array_buffer()).await.map_err(err)?;
    Ok((Uint8Array::new(&buf).to_vec(), ext))
}

pub async fn render_turntable(ui: Ui, rs: RenderSettings) {
    if ui.busy.get_untracked() {
        ui.status.set("Please wait for the current job to finish (or cancel it)".into());
        return;
    }
    CANCEL.with(|c| c.set(false));
    let (_, mut w, mut h) = TURNTABLE_SIZES[rs.turn_size.min(TURNTABLE_SIZES.len() - 1)];
    if rs.turn_format == TurnFormat::Gif && w.max(h) > 640 {
        // GIFs get huge fast
        let k = 640.0 / w.max(h) as f32;
        w = (w as f32 * k) as u32;
        h = (h as f32 * k) as u32;
    }
    let n = rs.turn_frames.clamp(8, 720);
    let label = "Rendering turntable";
    start_job(ui, label);
    sleep_ms(30).await;
    let share = if rs.turn_format == TurnFormat::Video { 0.75 } else { 1.0 };
    let mut pngs: Vec<Vec<u8>> = Vec::new();
    let mut gif_bytes: Vec<u8> = Vec::new();
    let mut result: Result<(), String> = Ok(());
    {
        let mut gif = (rs.turn_format == TurnFormat::Gif).then(|| {
            let mut e = image::codecs::gif::GifEncoder::new_with_speed(&mut gif_bytes, 10);
            let _ = e.set_repeat(image::codecs::gif::Repeat::Infinite);
            e
        });
        for i in 0..n {
            if cancelled() {
                result = Err(CANCELLED.into());
                break;
            }
            let turn = std::f32::consts::TAU * i as f32 / n as f32;
            match render_rgba(ui, [w, h], turn, &rs).await {
                Ok(px) => {
                    if let Some(g) = gif.as_mut() {
                        let Some(img) = image::RgbaImage::from_raw(w, h, px) else {
                            result = Err("bad frame".into());
                            break;
                        };
                        let delay = image::Delay::from_numer_denom_ms(1000, rs.turn_fps.max(1));
                        if let Err(e) = g.encode_frame(image::Frame::from_parts(img, 0, 0, delay)) {
                            result = Err(e.to_string());
                            break;
                        }
                    } else {
                        match png(w, h, px) {
                            Ok(p) => pngs.push(p),
                            Err(e) => {
                                result = Err(e);
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
            ui.job.set(Some((label, share * (i + 1) as f32 / n as f32)));
            // let the window breathe between frames
            sleep_ms(0).await;
        }
    }
    let out: Result<(Vec<u8>, &str), String> = match (result, rs.turn_format) {
        (Err(e), _) => Err(e),
        (Ok(()), TurnFormat::Gif) => Ok((gif_bytes, "gif")),
        (Ok(()), TurnFormat::PngZip) => {
            let files: Vec<(String, Vec<u8>)> =
                pngs.into_iter().enumerate().map(|(i, p)| (format!("turntable_{i:04}.png"), p)).collect();
            Ok((zip_store(&files), "zip"))
        }
        (Ok(()), TurnFormat::Video) => {
            ui.job.set(Some(("Encoding video", 0.75)));
            record_video(ui, &pngs, w, h, rs.turn_fps).await
        }
    };
    let cancelled_now = cancelled();
    end_job(ui);
    match out {
        Ok(_) if cancelled_now => ui.status.set("Turntable cancelled".into()),
        Ok((bytes, ext)) => {
            let mb = bytes.len() as f64 / 1e6;
            match host::save_file(&format!("turntable.{ext}"), ext, "Save turntable", bytes).await {
                Ok(Some(p)) => ui.status.set(format!("Turntable: {n} frames, {w}×{h}, {mb:.1} MB — saved {p}")),
                Ok(None) => ui.status.set("Turntable not saved".into()),
                Err(e) => ui.status.set(format!("Save failed: {e}")),
            }
        }
        Err(e) => ui.status.set(job_error("Turntable", &e)),
    }
}

/// The mesh to export: all visible SubTools (arrays included) or just the active one.
fn export_source(e: &Editor, all: bool) -> Mesh {
    if all && e.subtool_count() > 1 {
        e.visible_mesh()
    } else {
        e.baked_mesh(e.active_subtool())
    }
}

pub async fn export_mesh(ui: Ui, ext: &'static str) {
    let all = ui.render.get_untracked().export_all;
    let bytes = with_ed(|e| {
        e.cancel_drag();
        io::save(&export_source(e, all), ext)
    });
    let name = format!("{}.{ext}", ui.project_name.get_untracked());
    match host::save_file(&name, ext, "Export mesh", bytes).await {
        Ok(Some(p)) => ui.status.set(format!("Exported {p}")),
        Ok(None) => {}
        Err(e) => ui.status.set(format!("Export failed: {e}")),
    }
}

async fn export_print(ui: Ui, rs: RenderSettings) {
    let (bytes, check) = with_ed(|e| {
        e.cancel_drag();
        let m = export_source(e, rs.export_all);
        let pl = print::placement(&m, rs.print_size, rs.print_unit);
        (io::save_placed(&m, "stl", &pl), print::check(&m))
    });
    let name = format!("{}_print.stl", ui.project_name.get_untracked());
    match host::save_file(&name, "stl", "Export for 3D printing", bytes).await {
        Ok(Some(p)) => ui.status.set(if check.watertight() {
            format!("Saved {p} (millimetres, Z up)")
        } else {
            format!("Saved {p} — warning: the mesh isn't watertight; a voxel remesh will fix it")
        }),
        Ok(None) => {}
        Err(e) => ui.status.set(format!("Export failed: {e}")),
    }
}

pub fn lighting_json(ui: Ui) -> serde_json::Value {
    let l = ui.view.get_untracked().lighting;
    serde_json::json!({
        "follow_camera": l.follow_camera, "ambient": l.ambient, "shadows": l.shadows, "softness": l.softness,
        "lights": l.lights.iter().map(|x| serde_json::json!({
            "on": x.on, "azimuth": x.azimuth, "elevation": x.elevation, "color": x.color, "intensity": x.intensity,
        })).collect::<Vec<_>>(),
    })
}

pub fn apply_lighting_json(ui: Ui, v: &serde_json::Value) {
    let f = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let b = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_bool());
    ui.view.update(|s| {
        let l = &mut s.lighting;
        l.follow_camera = b(v, "follow_camera").unwrap_or(l.follow_camera);
        l.ambient = f(v, "ambient").unwrap_or(l.ambient);
        l.shadows = b(v, "shadows").unwrap_or(l.shadows);
        l.softness = f(v, "softness").unwrap_or(l.softness);
        if let Some(arr) = v.get("lights").and_then(|x| x.as_array()) {
            for (i, o) in arr.iter().enumerate().take(4) {
                let x = &mut l.lights[i];
                x.on = b(o, "on").unwrap_or(x.on);
                x.azimuth = f(o, "azimuth").unwrap_or(x.azimuth);
                x.elevation = f(o, "elevation").unwrap_or(x.elevation);
                x.intensity = f(o, "intensity").unwrap_or(x.intensity);
                if let Some(c) = o.get("color").and_then(|c| c.as_array()) {
                    for (k, cv) in c.iter().enumerate().take(3) {
                        x.color[k] = cv.as_f64().unwrap_or(1.0) as f32;
                    }
                }
            }
        }
    });
}

pub fn render_json(ui: Ui) -> serde_json::Value {
    let r = ui.render.get_untracked();
    serde_json::json!({
        "size": r.size, "transparent": r.transparent, "dof": r.dof, "blur": r.blur,
        "turn_size": r.turn_size, "turn_frames": r.turn_frames, "turn_fps": r.turn_fps,
        "turn_format": match r.turn_format { TurnFormat::Video => 0, TurnFormat::Gif => 1, TurnFormat::PngZip => 2 },
        "export_format": r.export_format, "print_size": r.print_size,
        "print_unit": Unit::ALL.iter().position(|u| *u == r.print_unit),
    })
}

pub fn apply_render_json(ui: Ui, v: &serde_json::Value) {
    let u = |k: &str| v.get(k).and_then(|x| x.as_u64());
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool());
    ui.render.update(|r| {
        r.size = u("size").map_or(r.size, |x| (x as usize).min(IMAGE_SIZES.len() - 1));
        r.transparent = b("transparent").unwrap_or(r.transparent);
        r.dof = b("dof").unwrap_or(r.dof);
        r.blur = f("blur").unwrap_or(r.blur);
        r.turn_size = u("turn_size").map_or(r.turn_size, |x| (x as usize).min(TURNTABLE_SIZES.len() - 1));
        r.turn_frames = u("turn_frames").map_or(r.turn_frames, |x| x as u32);
        r.turn_fps = u("turn_fps").map_or(r.turn_fps, |x| x as u32);
        r.turn_format = match u("turn_format") {
            Some(1) => TurnFormat::Gif,
            Some(2) => TurnFormat::PngZip,
            Some(_) => TurnFormat::Video,
            None => r.turn_format,
        };
        r.export_format = u("export_format").map_or(r.export_format, |x| (x as usize).min(io::EXPORT_FORMATS.len() - 1));
        r.print_size = f("print_size").unwrap_or(r.print_size);
        if let Some(i) = u("print_unit").and_then(|i| Unit::ALL.get(i as usize)) {
            r.print_unit = *i;
        }
    });
}

fn hex(c: [f32; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

fn from_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

// ------------------------------------------------------------------ panels

#[component]
pub fn LightsFolder(ui: Ui) -> impl IntoView {
    let lit = move || ui.view.get().material >= render::FIRST_LIT;
    let light = move |i: usize| ui.view.get().lighting.lights[i];
    view! {
        <Folder title="Lights" closed=true>
            <Show when=move || !lit()>
                <div class="hint">"Lights and shadows shade the Lit materials (Viewport display › Material)."</div>
                <div class="row buttons">
                    <button on:click=move |_| ui.view.update(|v| v.material = render::FIRST_LIT)>"Use Lit clay"</button>
                </div>
            </Show>
            <Check
                label="Lights turn with the camera"
                value=Signal::derive(move || ui.view.get().lighting.follow_camera)
                on_change=move |v| ui.view.update(|s| s.lighting.follow_camera = v)
            />
            <Slider
                label="Ambient"
                min=0.0 max=1.0 step=0.01
                value=Signal::derive(move || ui.view.get().lighting.ambient)
                on_change=move |v| ui.view.update(|s| s.lighting.ambient = v)
            />
            <Check
                label="Shadows (from the key light)"
                value=Signal::derive(move || ui.view.get().lighting.shadows)
                on_change=move |v| ui.view.update(|s| s.lighting.shadows = v)
            />
            <Slider
                label="Softness"
                min=0.5 max=4.0 step=0.1
                value=Signal::derive(move || ui.view.get().lighting.softness)
                on_change=move |v| ui.view.update(|s| s.lighting.softness = v)
                format=|v| format!("{v:.1}")
            />
            {(0..4usize)
                .map(|i| {
                    view! {
                        <div class="light-block">
                            <div class="row">
                                <label class="check light-on">
                                    <input
                                        type="checkbox"
                                        prop:checked=move || light(i).on
                                        on:change=move |ev| {
                                            let on = event_target_checked(&ev);
                                            ui.view.update(|s| s.lighting.lights[i].on = on)
                                        }
                                    />
                                    <span>{LIGHT_NAMES[i]}</span>
                                </label>
                                <input
                                    type="color"
                                    class="light-color"
                                    prop:value=move || hex(light(i).color)
                                    on:input=move |ev| {
                                        if let Some(c) = from_hex(&event_target_value(&ev)) {
                                            ui.view.update(|s| s.lighting.lights[i].color = c)
                                        }
                                    }
                                />
                            </div>
                            <Slider
                                label="Around"
                                min=-180.0 max=180.0 step=1.0
                                value=Signal::derive(move || light(i).azimuth)
                                on_change=move |v| ui.view.update(|s| s.lighting.lights[i].azimuth = v)
                                format=|v| format!("{v:.0}°")
                            />
                            <Slider
                                label="Height"
                                min=-89.0 max=89.0 step=1.0
                                value=Signal::derive(move || light(i).elevation)
                                on_change=move |v| ui.view.update(|s| s.lighting.lights[i].elevation = v)
                                format=|v| format!("{v:.0}°")
                            />
                            <Slider
                                label="Strength"
                                min=0.0 max=5.0 step=0.05
                                value=Signal::derive(move || light(i).intensity)
                                on_change=move |v| ui.view.update(|s| s.lighting.lights[i].intensity = v)
                                format=|v| format!("{v:.2}")
                            />
                        </div>
                    }
                })
                .collect_view()}
            <div class="row buttons">
                <button on:click=move |_| ui.view.update(|s| s.lighting = render::Lighting::default())>"Reset lights"</button>
            </div>
        </Folder>
    }
}

#[component]
pub fn RenderFolder(ui: Ui) -> impl IntoView {
    let rs = ui.render;
    let size_label = move || {
        let s = image_size(ui, rs.get().size);
        format!("{} × {} pixels", s[0], s[1])
    };
    view! {
        <Folder title="Render" closed=true>
            <label class="row">
                <span class="label">"Size"</span>
                <select on:change=move |ev| {
                    if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                        rs.update(|r| r.size = i);
                    }
                }>
                    {IMAGE_SIZES
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _, _))| view! { <option value=i.to_string() selected=move || rs.get().size == i>{*name}</option> })
                        .collect_view()}
                </select>
            </label>
            <div class="row info">{size_label}</div>
            <Check label="Transparent background" value=Signal::derive(move || rs.get().transparent) on_change=move |v| rs.update(|r| r.transparent = v) />
            <Check label="Depth of field" value=Signal::derive(move || rs.get().dof) on_change=move |v| rs.update(|r| r.dof = v) />
            <Slider
                label="Blur"
                min=0.05 max=1.0 step=0.01
                value=Signal::derive(move || rs.get().blur)
                on_change=move |v| rs.update(|r| r.blur = v)
                format=|v| format!("{:.0}%", v * 100.0)
            />
            <div class="hint">"Focus falls on the model at the centre of the view. Lit materials add shadows; pick one in Viewport display."</div>
            <div class="row buttons">
                <button class="primary" disabled=move || ui.busy.get() on:click=move |_| spawn_local(render_still(ui, rs.get_untracked()))>
                    "Render image…"
                </button>
            </div>
            <div class="subhead">"Turntable"</div>
            <label class="row">
                <span class="label">"Size"</span>
                <select on:change=move |ev| {
                    if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                        rs.update(|r| r.turn_size = i);
                    }
                }>
                    {TURNTABLE_SIZES
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _, _))| view! { <option value=i.to_string() selected=move || rs.get().turn_size == i>{*name}</option> })
                        .collect_view()}
                </select>
            </label>
            <div class="row">
                <span class="label">"Format"</span>
                <div class="segmented">
                    <button class:on=move || rs.get().turn_format == TurnFormat::Video on:click=move |_| rs.update(|r| r.turn_format = TurnFormat::Video)>"Video"</button>
                    <button class:on=move || rs.get().turn_format == TurnFormat::Gif on:click=move |_| rs.update(|r| r.turn_format = TurnFormat::Gif)>"GIF"</button>
                    <button class:on=move || rs.get().turn_format == TurnFormat::PngZip on:click=move |_| rs.update(|r| r.turn_format = TurnFormat::PngZip)>"PNG frames"</button>
                </div>
            </div>
            <Slider
                label="Frames"
                min=24.0 max=360.0 step=6.0
                value=Signal::derive(move || rs.get().turn_frames as f32)
                on_change=move |v| rs.update(|r| r.turn_frames = v as u32)
                format=|v| format!("{v:.0}")
            />
            <div class="row">
                <span class="label">"Speed"</span>
                <div class="segmented">
                    {[24u32, 30, 60]
                        .into_iter()
                        .map(|f| view! {
                            <button class:on=move || rs.get().turn_fps == f on:click=move |_| rs.update(|r| r.turn_fps = f)>{format!("{f} fps")}</button>
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="row info">{move || {
                let r = rs.get();
                format!("One turn in {:.1} s", r.turn_frames as f32 / r.turn_fps as f32)
            }}</div>
            <div class="row buttons">
                <button class="primary" disabled=move || ui.busy.get() on:click=move |_| spawn_local(render_turntable(ui, rs.get_untracked()))>
                    "Render turntable…"
                </button>
            </div>
            <div class="hint">"The camera circles the model with your current view, lights and material. Video is MP4 where the window supports it (WebM otherwise); GIFs are limited to 640 pixels; PNG frames come in a ZIP for video editors."</div>
        </Folder>
    }
}

#[component]
pub fn ExportFolder(ui: Ui) -> impl IntoView {
    let rs = ui.render;
    let check = RwSignal::new(None::<print::MeshCheck>);
    let dims = move || {
        let _ = ui.rev.get();
        let r = rs.get();
        let d = with_ed(|e| print::printed_size(&export_source(e, r.export_all), r.print_size));
        format!("{:.1} × {:.1} × {:.1} {}", d.x, d.y, d.z, r.print_unit.name())
    };
    view! {
        <Folder title="Export" closed=true>
            <label class="row">
                <span class="label">"Format"</span>
                <select on:change=move |ev| {
                    if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                        rs.update(|r| r.export_format = i);
                    }
                }>
                    {io::EXPORT_FORMATS
                        .iter()
                        .enumerate()
                        .map(|(i, (_, name))| view! { <option value=i.to_string() selected=move || rs.get().export_format == i>{*name}</option> })
                        .collect_view()}
                </select>
            </label>
            <Check label="All visible SubTools" value=Signal::derive(move || rs.get().export_all) on_change=move |v| rs.update(|r| r.export_all = v) />
            <div class="row buttons">
                <button class="primary" on:click=move |_| {
                    let ext = io::EXPORT_FORMATS[rs.get_untracked().export_format].0;
                    spawn_local(export_mesh(ui, ext))
                }>"Export…"</button>
            </div>
            <div class="hint">"OBJ, PLY and FBX keep quads; OBJ keeps polygroups as groups; PLY and glTF carry polygroup colours as vertex colours. Meshes keep the scale they were imported at."</div>
            <div class="subhead">"3D printing"</div>
            <Slider
                label="Size"
                min=5.0 max=500.0 step=1.0
                value=Signal::derive(move || rs.get().print_size)
                on_change=move |v| rs.update(|r| r.print_size = v)
                format=|v| format!("{v:.0}")
            />
            <div class="row">
                <span class="label">"Units"</span>
                <div class="segmented">
                    {Unit::ALL
                        .into_iter()
                        .map(|u| view! {
                            <button class:on=move || rs.get().print_unit == u on:click=move |_| rs.update(|r| r.print_unit = u)>{u.name()}</button>
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="row info">{dims}</div>
            <div class="row buttons">
                <button on:click=move |_| check.set(Some(with_ed(|e| print::check(&export_source(e, rs.get_untracked().export_all)))))>"Check mesh"</button>
                <button class="primary" on:click=move |_| spawn_local(export_print(ui, rs.get_untracked()))>"Export STL for printing…"</button>
            </div>
            {move || {
                check.get().map(|c| {
                    let ok = c.watertight();
                    let text = if ok {
                        format!("Watertight ✓ — {} piece{}, ready to print", c.shells, if c.shells == 1 { "" } else { "s" })
                    } else {
                        let mut parts = Vec::new();
                        if c.open_edges > 0 {
                            parts.push(format!("{} open edges (holes)", c.open_edges));
                        }
                        if c.nonmanifold_edges > 0 {
                            parts.push(format!("{} edges shared by 3+ faces", c.nonmanifold_edges));
                        }
                        if c.volume <= 0.0 {
                            parts.push("inside out".to_string());
                        }
                        format!("Not watertight: {}", parts.join(", "))
                    };
                    view! {
                        <div class="row info" class:ok=ok class:bad=!ok>{text}</div>
                        <Show when=move || !ok>
                            <div class="row buttons">
                                <button on:click=move |_| {
                                    act(ui, Action::VoxelRemesh);
                                    check.set(None);
                                }>"Make watertight (voxel remesh)"</button>
                            </div>
                        </Show>
                    }
                })
            }}
            <div class="hint">"STL in millimetres with Z up, centred on the plate and sitting at Z = 0, which is what slicers expect."</div>
        </Folder>
    }
}

