//! What the page needs from its host: open and save files. In the Tauri app
//! this goes through native dialogs; in a plain browser it falls back to the
//! file picker and a download.

use base64::Engine as _;
use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

pub struct Opened {
    pub name: String,
    pub bytes: Vec<u8>,
}

fn js_err(v: JsValue) -> String {
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}

fn tauri_core() -> Option<JsValue> {
    let w = web_sys::window()?;
    let t = Reflect::get(&w, &"__TAURI__".into()).ok().filter(|v| !v.is_undefined())?;
    Reflect::get(&t, &"core".into()).ok().filter(|v| !v.is_undefined())
}

pub fn in_desktop_app() -> bool {
    tauri_core().is_some()
}

async fn invoke(cmd: &str, args: serde_json::Value) -> Result<JsValue, String> {
    let core = tauri_core().ok_or("Not running in the desktop app")?;
    let f: Function = Reflect::get(&core, &"invoke".into())
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;
    let args = js_sys::JSON::parse(&args.to_string()).map_err(js_err)?;
    let p: Promise = f
        .call2(&core, &cmd.into(), &args)
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| "invoke did not return a promise".to_string())?;
    JsFuture::from(p).await.map_err(js_err)
}

fn field(v: &JsValue, name: &str) -> Option<String> {
    Reflect::get(v, &name.into()).ok()?.as_string()
}

pub async fn open_mesh() -> Result<Option<Opened>, String> {
    if in_desktop_app() {
        let v = invoke("open_mesh", serde_json::json!({})).await?;
        if v.is_null() || v.is_undefined() {
            return Ok(None);
        }
        let name = field(&v, "name").unwrap_or_default();
        let data = field(&v, "data").ok_or("missing file data")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
        Ok(Some(Opened { name, bytes }))
    } else {
        browser_pick(".obj,.stl").await
    }
}

async fn browser_pick(accept: &str) -> Result<Option<Opened>, String> {
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let input: web_sys::HtmlInputElement =
        doc.create_element("input").map_err(js_err)?.dyn_into().map_err(|_| "not an input".to_string())?;
    input.set_type("file");
    input.set_accept(accept);
    let target = input.clone();
    let done = Promise::new(&mut |resolve: Function, _reject: Function| {
        let r1 = resolve.clone();
        let on_change = Closure::once_into_js(move || {
            let _ = r1.call0(&JsValue::NULL);
        });
        target.set_onchange(Some(on_change.unchecked_ref()));
        let on_cancel = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let _ = target.add_event_listener_with_callback("cancel", on_cancel.unchecked_ref());
    });
    input.click();
    JsFuture::from(done).await.map_err(js_err)?;
    let Some(file) = input.files().and_then(|f| f.get(0)) else { return Ok(None) };
    let buf = JsFuture::from(file.array_buffer()).await.map_err(js_err)?;
    Ok(Some(Opened { name: file.name(), bytes: Uint8Array::new(&buf).to_vec() }))
}

fn browser_download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let parts = Array::of1(&Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js_err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js_err)?;
    let a: web_sys::HtmlAnchorElement =
        doc.create_element("a").map_err(js_err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// Calls a backend command with a binary body (and string headers) and
/// returns its binary reply.
pub async fn invoke_bytes(cmd: &str, body: &[u8], headers: &[(&str, String)]) -> Result<Vec<u8>, String> {
    let v = invoke_body(cmd, body, headers).await?;
    Ok(Uint8Array::new(&v).to_vec())
}

async fn invoke_body(cmd: &str, body: &[u8], headers: &[(&str, String)]) -> Result<JsValue, String> {
    let core = tauri_core().ok_or("Not running in the desktop app")?;
    let f: Function = Reflect::get(&core, &"invoke".into())
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| "invoke is not a function".to_string())?;
    let hdrs = js_sys::Object::new();
    for (k, v) in headers {
        Reflect::set(&hdrs, &(*k).into(), &v.into()).map_err(js_err)?;
    }
    let opts = js_sys::Object::new();
    Reflect::set(&opts, &"headers".into(), &hdrs).map_err(js_err)?;
    let p: Promise = f
        .call3(&core, &cmd.into(), &Uint8Array::from(body), &opts)
        .map_err(js_err)?
        .dyn_into()
        .map_err(|_| "invoke did not return a promise".to_string())?;
    JsFuture::from(p).await.map_err(js_err)
}

fn uri(s: &str) -> String {
    js_sys::encode_uri_component(s).into()
}

pub struct OpenedProject {
    pub name: String,
    /// Where it lives on disk (desktop app only).
    pub path: Option<String>,
    pub bytes: Vec<u8>,
}

pub async fn open_project() -> Result<Option<OpenedProject>, String> {
    open_with("open_project", ".clayform").await
}

/// Asks for a PNG or JPEG image.
pub async fn open_image() -> Result<Option<OpenedProject>, String> {
    open_with("open_image", "image/png,image/jpeg").await
}

async fn open_with(cmd: &str, accept: &str) -> Result<Option<OpenedProject>, String> {
    if in_desktop_app() {
        let v = invoke(cmd, serde_json::json!({})).await?;
        let b = Uint8Array::new(&v).to_vec();
        if b.len() < 4 {
            return Ok(None);
        }
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let path = String::from_utf8_lossy(b.get(4..4 + n).ok_or("bad reply")?).into_owned();
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
        Ok(Some(OpenedProject { name, path: Some(path), bytes: b[4 + n..].to_vec() }))
    } else {
        Ok(browser_pick(accept).await?.map(|o| OpenedProject { name: o.name, path: None, bytes: o.bytes }))
    }
}

/// Saves a project to `path` (or asks where, if None). Returns the path or
/// file name written, or None if the user cancelled.
pub async fn save_project(bytes: Vec<u8>, path: Option<&str>, suggested_name: &str) -> Result<Option<String>, String> {
    if in_desktop_app() {
        let headers = [("path", uri(path.unwrap_or(""))), ("name", uri(suggested_name))];
        let v = invoke_body("save_project", &bytes, &headers).await?;
        Ok(v.as_string())
    } else {
        browser_download(suggested_name, &bytes)?;
        Ok(Some(suggested_name.to_string()))
    }
}

/// Saves any file (asking where in the desktop app, downloading in a
/// browser). Returns where it went, or None if the user cancelled.
pub async fn save_file(suggested_name: &str, extension: &str, title: &str, bytes: Vec<u8>) -> Result<Option<String>, String> {
    if in_desktop_app() {
        let headers = [("name", uri(suggested_name)), ("ext", uri(extension)), ("title", uri(title))];
        let v = invoke_body("save_file", &bytes, &headers).await?;
        Ok(v.as_string())
    } else {
        browser_download(suggested_name, &bytes)?;
        Ok(Some(suggested_name.to_string()))
    }
}

/// Window title (the native title bar in the desktop app).
pub fn set_title(title: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        doc.set_title(title);
    }
    let Some(w) = web_sys::window() else { return };
    let win = Reflect::get(&w, &"__TAURI__".into())
        .ok()
        .filter(|t| !t.is_undefined())
        .and_then(|t| Reflect::get(&t, &"window".into()).ok())
        .filter(|v| !v.is_undefined());
    let Some(win) = win else { return };
    let Ok(get) = Reflect::get(&win, &"getCurrentWindow".into()).and_then(|f| f.dyn_into::<Function>().map_err(|v| v.into()))
    else {
        return;
    };
    let Ok(current) = get.call0(&win) else { return };
    if let Ok(set) = Reflect::get(&current, &"setTitle".into()).and_then(|f| f.dyn_into::<Function>().map_err(|v| v.into())) {
        let _ = set.call1(&current, &title.into());
    }
}

pub async fn cancel_job() {
    let _ = invoke("cancel_job", serde_json::json!({})).await;
}

/// Calls `on_progress` with each progress value (0..1) the backend reports.
pub fn listen_progress(on_progress: impl Fn(f32) + 'static) {
    let Some(w) = web_sys::window() else { return };
    let Some(event) = Reflect::get(&w, &"__TAURI__".into())
        .ok()
        .filter(|t| !t.is_undefined())
        .and_then(|t| Reflect::get(&t, &"event".into()).ok())
        .filter(|e| !e.is_undefined())
    else {
        return;
    };
    let Ok(listen) = Reflect::get(&event, &"listen".into()).and_then(|f| f.dyn_into::<Function>().map_err(|v| v.into()))
    else {
        return;
    };
    let cb = Closure::<dyn Fn(JsValue)>::new(move |e: JsValue| {
        if let Some(p) = Reflect::get(&e, &"payload".into()).ok().and_then(|p| p.as_f64()) {
            on_progress(p as f32);
        }
    });
    let _ = listen.call2(&event, &"clayform-progress".into(), cb.as_ref());
    cb.forget();
}
