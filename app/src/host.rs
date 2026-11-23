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
        browser_pick().await
    }
}

/// Returns where the file went (a path in the app, the name in a browser),
/// or None if the user cancelled.
pub async fn save_mesh(suggested_name: &str, extension: &str, bytes: Vec<u8>) -> Result<Option<String>, String> {
    if in_desktop_app() {
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let v = invoke(
            "save_mesh",
            serde_json::json!({ "suggestedName": suggested_name, "extension": extension, "data": data }),
        )
        .await?;
        Ok(v.as_string())
    } else {
        browser_download(suggested_name, &bytes)?;
        Ok(Some(suggested_name.to_string()))
    }
}

async fn browser_pick() -> Result<Option<Opened>, String> {
    let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let input: web_sys::HtmlInputElement =
        doc.create_element("input").map_err(js_err)?.dyn_into().map_err(|_| "not an input".to_string())?;
    input.set_type("file");
    input.set_accept(".obj,.stl");
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
