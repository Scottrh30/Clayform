//! Everyday conveniences: autosave and crash recovery, recent files and the
//! undo history list.

use super::*;
use std::cell::Cell;

const RECENT_KEY: &str = "clayform.recent";
const AUTOSAVE_KEY: &str = "clayform.autosaveMinutes";
const MAX_RECENT: usize = 10;

thread_local! {
    /// When the user last pressed a key or moved the pointer (ms).
    static LAST_INPUT: Cell<f64> = const { Cell::new(0.0) };
}

/// Notes user activity (autosave waits for a pause).
pub fn touch() {
    LAST_INPUT.with(|t| t.set(now_ms()));
}

// ---------------------------------------------------------------- recent files

pub fn load_recent() -> Vec<String> {
    storage()
        .and_then(|s| s.get_item(RECENT_KEY).ok().flatten())
        .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
        .unwrap_or_default()
}

fn store_recent(list: &[String]) {
    if let Some(s) = storage() {
        let _ = s.set_item(RECENT_KEY, &serde_json::to_string(list).unwrap_or_default());
    }
}

/// Puts a project path at the top of the recent list.
pub fn add_recent(ui: Ui, path: &str) {
    let mut list = load_recent();
    list.retain(|p| p != path);
    list.insert(0, path.to_string());
    list.truncate(MAX_RECENT);
    store_recent(&list);
    ui.recent.set(list);
}

pub fn forget_recent(ui: Ui, path: &str) {
    let mut list = load_recent();
    list.retain(|p| p != path);
    store_recent(&list);
    ui.recent.set(list);
}

pub fn clear_recent(ui: Ui) {
    store_recent(&[]);
    ui.recent.set(Vec::new());
}

pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

pub async fn open_recent(ui: Ui, path: String) {
    if ui.busy.get_untracked() {
        return;
    }
    match host::open_project_path(&path).await {
        Ok(bytes) => open_project_bytes(ui, file_name(&path), Some(path), bytes).await,
        Err(e) => {
            forget_recent(ui, &path);
            ui.status.set(format!("{e} — removed it from Open recent"));
        }
    }
}

/// Loads project bytes (from a dialog, the recent list or a recovery copy).
pub async fn open_project_bytes(ui: Ui, name: String, path: Option<String>, bytes: Vec<u8>) {
    let parts = match project::decode(&bytes) {
        Ok(p) => p,
        Err(e) => {
            ui.status.set(format!("Open failed: {e}"));
            return;
        }
    };
    start_job(ui, "Opening");
    let mut scene = Vec::with_capacity(parts.subtools.len());
    let base = name.trim_end_matches(".clayform").to_string();
    for (k, st) in parts.subtools.into_iter().enumerate() {
        let active = st.active;
        let mut levels: Vec<LevelData<Mesh>> = Vec::with_capacity(st.levels.len());
        let mut active_bvh = None;
        for (i, l) in st.levels.into_iter().enumerate() {
            let (mesh, stencils, bvh) = build_in_slices(ui, l.mesh).await;
            if i == active {
                active_bvh = Some(bvh);
            }
            levels.push(LevelData { mesh, stencils, details: l.details });
        }
        let fallback = if k == 0 { base.clone() } else { format!("{base} {}", k + 1) };
        let info = scene_ui::parse_info(&st.info_json, &fallback);
        let mut sc = clayform_core::sculpt::Sculptor::from_levels(levels, active, active_bvh);
        sc.import_extras(&st.extras);
        scene.push((info, sc));
    }
    with_ed(|e| {
        e.load_scene(scene, parts.active_subtool);
        // projects without a saved camera open framed on everything
        e.frame_all();
    });
    apply_settings(ui, &parts.settings_json);
    if let Some(p) = &path {
        add_recent(ui, p);
    }
    ui.project_path.set(path);
    ui.project_name.set(name.trim_end_matches(".clayform").to_string());
    end_job(ui);
    ui.saved_rev.set(ui.rev.get_untracked());
    ui.status.set(format!("Opened {name}"));
}

// -------------------------------------------------------------------- autosave

pub fn autosave_minutes() -> u32 {
    storage()
        .and_then(|s| s.get_item(AUTOSAVE_KEY).ok().flatten())
        .and_then(|v| v.parse().ok())
        .unwrap_or(3)
}

pub fn set_autosave_minutes(ui: Ui, m: u32) {
    ui.autosave_minutes.set(m);
    if let Some(s) = storage() {
        let _ = s.set_item(AUTOSAVE_KEY, &m.to_string());
    }
}

/// The project as bytes, with a note of which file it came from.
fn encode_for_autosave(ui: Ui) -> Vec<u8> {
    let mut settings: serde_json::Value =
        serde_json::from_str(&settings_json(ui)).unwrap_or_else(|_| serde_json::json!({}));
    settings["autosave_of"] = serde_json::json!({
        "name": ui.project_name.get_untracked(),
        "path": ui.project_path.get_untracked(),
    });
    let settings = settings.to_string();
    scene_ui::encode_project(&settings)
}

/// Runs for the life of the window: every so often, if there are unsaved
/// changes and the user has paused, writes a recovery copy.
pub fn start_autosave(ui: Ui) {
    if !host::in_desktop_app() {
        return;
    }
    spawn_local(async move {
        // offer to recover what the last session left behind
        if let Some((size, when)) = host::autosave_info().await {
            if size > 0.0 {
                ui.recovery.set(Some(when));
            }
        }
        let mut last_save = now_ms();
        let mut saved_rev = ui.rev.get_untracked();
        loop {
            sleep_ms(5000).await;
            let minutes = ui.autosave_minutes.get_untracked();
            if minutes == 0 || ui.recovery.get_untracked().is_some() {
                continue;
            }
            let rev = ui.rev.get_untracked();
            let dirty = rev != ui.saved_rev.get_untracked() && rev != saved_rev;
            let idle = now_ms() - LAST_INPUT.with(|t| t.get()) > 2500.0;
            let due = now_ms() - last_save > minutes as f64 * 60_000.0;
            let free = !ui.busy.get_untracked() && with_ed(|e| e.drag() == Drag::None);
            if dirty && idle && due && free {
                let bytes = encode_for_autosave(ui);
                match host::autosave_write(&bytes).await {
                    Ok(()) => {
                        last_save = now_ms();
                        saved_rev = rev;
                        ui.autosaved_at.set(Some(js_sys::Date::now()));
                    }
                    Err(e) => log(&format!("autosave failed: {e}")),
                }
            }
        }
    });
}

/// After a real save the recovery copy is no longer needed.
pub fn saved(ui: Ui, path: Option<&str>) {
    if let Some(p) = path {
        add_recent(ui, p);
    }
    spawn_local(host::autosave_clear());
}

async fn recover(ui: Ui) {
    ui.recovery.set(None);
    match host::autosave_read().await {
        Ok(bytes) => {
            let of = project::decode(&bytes)
                .ok()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(&p.settings_json).ok())
                .and_then(|j| j.get("autosave_of").cloned());
            let name = of
                .as_ref()
                .and_then(|o| o.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("Recovered")
                .to_string();
            let path = of.as_ref().and_then(|o| o.get("path")).and_then(|p| p.as_str()).map(str::to_string);
            open_project_bytes(ui, name.clone(), None, bytes).await;
            // keep pointing at the original file, but show it as unsaved
            ui.project_path.set(path);
            ui.project_name.set(name);
            ui.saved_rev.set(u64::MAX);
            ui.status.set("Recovered your unsaved work — save it to keep it".into());
        }
        Err(e) => ui.status.set(format!("Couldn't read the recovery copy: {e}")),
    }
}

fn ago(ms: f64) -> String {
    let now = js_sys::Date::now();
    let mins = ((now - ms) / 60_000.0).max(0.0);
    if mins < 1.0 {
        "less than a minute ago".into()
    } else if mins < 90.0 {
        format!("{:.0} minutes ago", mins)
    } else if mins < 48.0 * 60.0 {
        format!("{:.0} hours ago", mins / 60.0)
    } else {
        let d = js_sys::Date::new(&JsValue::from_f64(ms));
        format!("on {}", String::from(d.to_locale_string("default", &JsValue::UNDEFINED)))
    }
}

/// "Recover unsaved work?" banner over the viewport.
#[component]
pub fn RecoveryBanner(ui: Ui) -> impl IntoView {
    view! {
        <Show when=move || ui.recovery.get().is_some()>
            <div class="recovery">
                <span>{move || format!(
                    "Clayform found work that wasn't saved (autosaved {}).",
                    ui.recovery.get().map(ago).unwrap_or_default()
                )}</span>
                <button class="primary" on:click=move |_| spawn_local(recover(ui))>"Recover"</button>
                <button on:click=move |_| {
                    ui.recovery.set(None);
                    spawn_local(host::autosave_clear());
                    ui.status.set("Discarded the recovery copy".into());
                }>"Discard"</button>
            </div>
        </Show>
    }
}

// ----------------------------------------------------------------- history

#[component]
pub fn HistoryFolder(ui: Ui) -> impl IntoView {
    let hist = move || {
        ui.rev.get();
        ui.can_undo.get();
        ui.can_redo.get();
        with_ed(|e| e.sculptor.history())
    };
    let goto = move |done: usize| {
        if ui.busy.get_untracked() {
            return;
        }
        with_ed(|e| e.goto_history(done));
        ui.refresh();
    };
    let list_ref = NodeRef::<leptos::html::Div>::new();
    // keep the current step in view
    Effect::new(move || {
        let _ = hist();
        if let Some(el) = list_ref.get() {
            if let Some(cur) = el.query_selector(".hist-item.current").ok().flatten() {
                let el: web_sys::HtmlElement = el.into();
                let cur: web_sys::HtmlElement = cur.unchecked_into();
                let top = cur.offset_top() - el.offset_top();
                if top < el.scroll_top() || top > el.scroll_top() + el.client_height() - 24 {
                    el.set_scroll_top(top - el.client_height() / 2);
                }
            }
        }
    });
    view! {
        <Folder title="History" closed=true>
            <div class="hist-list" node_ref=list_ref>
                {move || {
                    let (done, undone) = hist();
                    let n = done.len();
                    let first = view! {
                        <button class="hist-item" class:current=n == 0 on:click=move |_| goto(0)>
                            "Start"
                        </button>
                    };
                    let done_items = done
                        .into_iter()
                        .enumerate()
                        .map(|(i, l)| {
                            let at = i + 1;
                            view! {
                                <button class="hist-item" class:current=at == n on:click=move |_| goto(at)>{l}</button>
                            }
                        })
                        .collect_view();
                    let undone_items = undone
                        .into_iter()
                        .enumerate()
                        .map(|(i, l)| {
                            let at = n + i + 1;
                            view! {
                                <button class="hist-item undone" on:click=move |_| goto(at)>{l}</button>
                            }
                        })
                        .collect_view();
                    view! { {first} {done_items} {undone_items} }
                }}
            </div>
            <div class="hint">"Click any step to go back (or forward) to it. Greyed steps were undone; a new change replaces them. The last 100 steps are kept (fewer for very big meshes)."</div>
        </Folder>
    }
}
