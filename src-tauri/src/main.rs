// Clayform's desktop shell: opens the window that shows the Leptos interface
// and provides what a web page can't do itself — native Open / Save dialogs
// and file access. All sculpting happens in the interface (WebAssembly).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine as _;
use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![open_mesh, save_mesh])
        .run(tauri::generate_context!())
        .expect("error while running Clayform");
}
