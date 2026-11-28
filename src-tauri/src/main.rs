// Clayform's desktop shell: opens the window that shows the Leptos interface
// and provides what a web page can't do itself — native Open / Save dialogs,
// file access, and heavy jobs (voxel remesh, subdivision) on a background
// thread with progress and cancel. Sculpting itself happens in the
// interface (WebAssembly).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine as _;
use clayform_core::remesh::{voxel_remesh_with, VoxelSettings, CANCELLED};
use clayform_core::wire;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// Lets the interface cancel the running job.
#[derive(Default)]
struct Jobs {
    cancel: Arc<AtomicBool>,
}

fn header<'a>(req: &'a Request<'_>, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

fn raw_body(req: &Request<'_>) -> Result<Vec<u8>, String> {
    match req.body() {
        InvokeBody::Raw(b) => Ok(b.clone()),
        _ => Err("expected binary mesh data".into()),
    }
}

/// Runs `job` on a blocking thread, sending throttled progress events and
/// honouring cancel requests.
async fn run_job(
    app: tauri::AppHandle,
    jobs: &Jobs,
    job: impl FnOnce(&mut dyn FnMut(f32) -> bool) -> Result<Vec<u8>, String> + Send + 'static,
) -> Result<Response, String> {
    jobs.cancel.store(false, Ordering::SeqCst);
    let cancel = jobs.cancel.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut progress = |p: f32| {
            if last.elapsed() >= Duration::from_millis(40) {
                let _ = app.emit("clayform-progress", p);
                last = Instant::now();
            }
            !cancel.load(Ordering::SeqCst)
        };
        job(&mut progress)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Response::new(out))
}

/// Voxel remesh. Body: mesh (wire format). Headers: resolution, smooth,
/// project, signed, and `parts` for booleans ("end:op,end:op…", op = a / s / i).
#[tauri::command]
async fn remesh(app: tauri::AppHandle, jobs: State<'_, Jobs>, request: Request<'_>) -> Result<Response, String> {
    let bytes = raw_body(&request)?;
    let defaults = VoxelSettings::default();
    let settings = VoxelSettings {
        resolution: header(&request, "resolution").and_then(|v| v.parse().ok()).unwrap_or(defaults.resolution),
        smooth: header(&request, "smooth").and_then(|v| v.parse().ok()).unwrap_or(defaults.smooth),
        project: header(&request, "project").map_or(defaults.project, |v| v == "1"),
        signed: header(&request, "signed").is_some_and(|v| v == "1"),
    };
    let parts: Vec<(usize, clayform_core::scene::BoolOp)> = header(&request, "parts")
        .map(percent_decode)
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| {
            let (end, op) = p.split_once(':')?;
            Some((end.parse().ok()?, clayform_core::scene::BoolOp::from_code(op.chars().next()?)))
        })
        .collect();
    run_job(app, &jobs, move |progress| {
        let (mesh, _) = wire::decode(&bytes)?;
        let out = if parts.len() > 1 {
            clayform_core::remesh::voxel_csg_with(&mesh, &settings, &parts, progress)?
        } else {
            voxel_remesh_with(&mesh, &settings, progress)?
        };
        Ok(wire::encode(&out, &[]))
    })
    .await
}

/// Quad remesh or decimation. Body: mesh. Headers: `op` (quad | decimate),
/// `target` (polygon / triangle count), `groups` (1 = keep polygroups).
#[tauri::command]
async fn topology(app: tauri::AppHandle, jobs: State<'_, Jobs>, request: Request<'_>) -> Result<Response, String> {
    let bytes = raw_body(&request)?;
    let op = header(&request, "op").unwrap_or_default().to_string();
    let target: usize = header(&request, "target").and_then(|v| v.parse().ok()).unwrap_or(5000);
    let groups = header(&request, "groups").is_some_and(|v| v == "1");
    run_job(app, &jobs, move |progress| {
        let (mesh, _) = wire::decode(&bytes)?;
        let out = match op.as_str() {
            "quad" => clayform_core::topo::quad_remesh(&mesh, target, groups, progress)?,
            _ => clayform_core::topo::decimate(&mesh, target, groups, progress)?,
        };
        Ok(wire::encode(&out, &[]))
    })
    .await
}

/// One level of Loop subdivision. Body: mesh; returns mesh + stencils.
#[tauri::command]
async fn subdivide(app: tauri::AppHandle, jobs: State<'_, Jobs>, request: Request<'_>) -> Result<Response, String> {
    let bytes = raw_body(&request)?;
    run_job(app, &jobs, move |progress| {
        let (mesh, _) = wire::decode(&bytes)?;
        let (fine, odd) = mesh.subdivide_with_progress(true, progress).map_err(|_| CANCELLED.to_string())?;
        Ok(wire::encode(&fine, &odd))
    })
    .await
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Asks for a file. Returns the path (u32 length + UTF-8) followed by the
/// file's bytes, or an empty reply if the user cancelled.
/// For automated tests only: when set, Open/Save use this file instead of
/// showing a dialog (CI machines have no one to click the dialogs).
fn test_file() -> Option<std::path::PathBuf> {
    std::env::var_os("CLAYFORM_TEST_FILE").map(std::path::PathBuf::from)
}

fn pick_and_read(app: &tauri::AppHandle, title: &str, filter: &str, exts: &[&str]) -> Result<Response, String> {
    let path = match test_file() {
        Some(p) => p,
        None => {
            let picked = app.dialog().file().set_title(title).add_filter(filter, exts).blocking_pick_file();
            let Some(path) = picked else { return Ok(Response::new(Vec::new())) };
            path.into_path().map_err(|e| e.to_string())?
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let p = path.display().to_string();
    let mut out = Vec::with_capacity(4 + p.len() + bytes.len());
    out.extend_from_slice(&(p.len() as u32).to_le_bytes());
    out.extend_from_slice(p.as_bytes());
    out.extend_from_slice(&bytes);
    Ok(Response::new(out))
}

#[tauri::command]
async fn open_project(app: tauri::AppHandle) -> Result<Response, String> {
    pick_and_read(&app, "Open project", "Clayform project", &[clayform_core::project::EXTENSION])
}

#[tauri::command]
async fn open_font(app: tauri::AppHandle) -> Result<Response, String> {
    pick_and_read(&app, "Load font", "Fonts", &["ttf", "otf", "TTF", "OTF"])
}

#[tauri::command]
async fn open_brush(app: tauri::AppHandle) -> Result<Response, String> {
    pick_and_read(&app, "Import brush preset", "Clayform brush", &["claybrush"])
}

#[tauri::command]
async fn open_image(app: tauri::AppHandle) -> Result<Response, String> {
    pick_and_read(&app, "Open image", "Images (PNG, JPEG)", &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"])
}

/// Saves a project. Body: file bytes. Headers: `path` (URL-encoded; empty =
/// ask with a Save dialog) and `name` (suggested file name). Returns the path
/// written, or null if the user cancelled.
#[tauri::command]
async fn save_project(app: tauri::AppHandle, request: Request<'_>) -> Result<Option<String>, String> {
    let bytes = raw_body(&request)?;
    let given = header(&request, "path").map(percent_decode).unwrap_or_default();
    let path = if let (true, Some(t)) = (given.is_empty(), test_file()) {
        t
    } else if given.is_empty() {
        let name = header(&request, "name").map(percent_decode).unwrap_or_else(|| "sculpt.clayform".into());
        let picked = app
            .dialog()
            .file()
            .set_title("Save project")
            .set_file_name(&name)
            .add_filter("Clayform project", &[clayform_core::project::EXTENSION])
            .blocking_save_file();
        let Some(p) = picked else { return Ok(None) };
        let mut p = p.into_path().map_err(|e| e.to_string())?;
        if p.extension().is_none() {
            p.set_extension(clayform_core::project::EXTENSION);
        }
        p
    } else {
        std::path::PathBuf::from(given)
    };
    // write next to the target first, so a failed save never leaves a half file
    let tmp = path.with_extension("clayform.saving");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

/// Saves any file. Body: file bytes. Headers: `name` (suggested file name),
/// `ext` (extension for the filter) and `title` (dialog title), URL-encoded.
/// Returns the path written, or null if the user cancelled.
#[tauri::command]
async fn save_file(app: tauri::AppHandle, request: Request<'_>) -> Result<Option<String>, String> {
    let bytes = raw_body(&request)?;
    let name = header(&request, "name").map(percent_decode).unwrap_or_else(|| "export".into());
    let ext = header(&request, "ext").map(percent_decode).unwrap_or_default();
    let title = header(&request, "title").map(percent_decode).unwrap_or_else(|| "Save".into());
    let path = match std::env::var_os("CLAYFORM_TEST_SAVE_DIR") {
        // scripted tests: no dialog
        Some(dir) => std::path::PathBuf::from(dir).join(&name),
        None => {
            let mut d = app.dialog().file().set_title(&title).set_file_name(&name);
            if !ext.is_empty() {
                d = d.add_filter(ext.to_uppercase(), &[ext.as_str()]);
            }
            let Some(p) = d.blocking_save_file() else { return Ok(None) };
            let mut p = p.into_path().map_err(|e| e.to_string())?;
            if p.extension().is_none() && !ext.is_empty() {
                p.set_extension(&ext);
            }
            p
        }
    };
    let tmp = path.with_extension(format!("{ext}.saving"));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

fn autosave_file(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("autosave.clayform"))
}

/// Writes the crash-recovery copy of the open sculpt. Body: project bytes.
#[tauri::command]
async fn autosave_write(app: tauri::AppHandle, request: Request<'_>) -> Result<(), String> {
    let bytes = raw_body(&request)?;
    let path = autosave_file(&app)?;
    let tmp = path.with_extension("saving");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct AutosaveInfo {
    size: u64,
    /// Milliseconds since 1970.
    modified: f64,
}

/// The recovery copy's size and age, or null if there's none.
#[tauri::command]
fn autosave_info(app: tauri::AppHandle) -> Result<Option<AutosaveInfo>, String> {
    let path = autosave_file(&app)?;
    let Ok(meta) = std::fs::metadata(&path) else { return Ok(None) };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_millis() as f64);
    Ok(Some(AutosaveInfo { size: meta.len(), modified }))
}

#[tauri::command]
async fn autosave_read(app: tauri::AppHandle) -> Result<Response, String> {
    let path = autosave_file(&app)?;
    std::fs::read(&path).map(Response::new).map_err(|e| e.to_string())
}

#[tauri::command]
fn autosave_clear(app: tauri::AppHandle) -> Result<(), String> {
    let path = autosave_file(&app)?;
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Reads a project by path (Open recent). Header: `path` (URL-encoded).
#[tauri::command]
async fn open_project_path(request: Request<'_>) -> Result<Response, String> {
    let path = header(&request, "path").map(percent_decode).ok_or("no path")?;
    std::fs::read(&path).map(Response::new).map_err(|e| format!("Could not open {path}: {e}"))
}

#[tauri::command]
fn cancel_job(jobs: State<'_, Jobs>) {
    jobs.cancel.store(true, Ordering::SeqCst);
}

#[derive(Serialize)]
struct Opened {
    name: String,
    data: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Asks for an OBJ / STL file and returns its name and contents (base64),
/// or None if the user cancelled.
#[tauri::command]
async fn open_mesh(app: tauri::AppHandle) -> Result<Option<Opened>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("Import mesh")
        .add_filter("Meshes (OBJ, STL)", &["obj", "stl", "OBJ", "STL"])
        .blocking_pick_file();
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Some(Opened { name, data: b64().encode(bytes) }))
}

/// Asks where to save and writes the file; returns the chosen path, or None
/// if the user cancelled.
fn main() {
    tauri::Builder::default()
        .manage(Jobs::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![open_mesh, save_file, autosave_write, autosave_info, autosave_read, autosave_clear, open_project_path, remesh, topology, subdivide, cancel_job, open_project, save_project, open_image, open_brush, open_font])
        .run(tauri::generate_context!())
        .expect("error while running Clayform");
}
