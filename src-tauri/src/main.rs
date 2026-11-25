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
use tauri::{Emitter, State};
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

/// Voxel remesh. Body: mesh (wire format). Headers: resolution, smooth, project.
#[tauri::command]
async fn remesh(app: tauri::AppHandle, jobs: State<'_, Jobs>, request: Request<'_>) -> Result<Response, String> {
    let bytes = raw_body(&request)?;
    let defaults = VoxelSettings::default();
    let settings = VoxelSettings {
        resolution: header(&request, "resolution").and_then(|v| v.parse().ok()).unwrap_or(defaults.resolution),
        smooth: header(&request, "smooth").and_then(|v| v.parse().ok()).unwrap_or(defaults.smooth),
        project: header(&request, "project").map_or(defaults.project, |v| v == "1"),
    };
    run_job(app, &jobs, move |progress| {
        let (mesh, _) = wire::decode(&bytes)?;
        let out = voxel_remesh_with(&mesh, &settings, progress)?;
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
#[tauri::command]
async fn save_mesh(
    app: tauri::AppHandle,
    suggested_name: String,
    extension: String,
    data: String,
) -> Result<Option<String>, String> {
    let bytes = b64().decode(data).map_err(|e| e.to_string())?;
    let picked = app
        .dialog()
        .file()
        .set_title("Export mesh")
        .set_file_name(&suggested_name)
        .add_filter(extension.to_uppercase(), &[extension.as_str()])
        .blocking_save_file();
    let Some(path) = picked else { return Ok(None) };
    let mut path = path.into_path().map_err(|e| e.to_string())?;
    if path.extension().is_none() {
        path.set_extension(&extension);
    }
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

fn main() {
    tauri::Builder::default()
        .manage(Jobs::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![open_mesh, save_mesh, remesh, subdivide, cancel_job])
        .run(tauri::generate_context!())
        .expect("error while running Clayform");
}
