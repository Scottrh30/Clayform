//! Keyboard shortcuts: every command has an id, a label and default keys;
//! the user can rebind them (saved in local storage).

use super::*;
use std::collections::HashMap;

const KEYS_KEY: &str = "clayform.keys";

/// A command that can have shortcuts: (id, group, label, default keys).
pub struct Command {
    pub id: String,
    pub group: &'static str,
    pub label: String,
    pub defaults: Vec<String>,
}

fn cmd(id: &str, group: &'static str, label: &str, defaults: &[&str]) -> Command {
    Command { id: id.into(), group, label: label.into(), defaults: defaults.iter().map(|s| s.to_string()).collect() }
}

pub const GROUPS: [&str; 6] = ["File", "Edit", "View", "Brush", "Modes", "Brushes"];

pub fn commands() -> Vec<Command> {
    let mut v = vec![
        cmd("open", "File", "Open project", &["Ctrl+O"]),
        cmd("save", "File", "Save project", &["Ctrl+S"]),
        cmd("save_as", "File", "Save project as", &["Ctrl+Shift+S"]),
        cmd("import", "File", "Import mesh", &["Ctrl+I"]),
        cmd("export", "File", "Export OBJ", &["Ctrl+E"]),
        cmd("render", "File", "Render image", &["F12"]),
        cmd("turntable", "File", "Render turntable", &[]),
        cmd("undo", "Edit", "Undo", &["Ctrl+Z"]),
        cmd("redo", "Edit", "Redo", &["Ctrl+Shift+Z", "Ctrl+Y"]),
        cmd("subdivide", "Edit", "Subdivide (add a level)", &["Ctrl+D"]),
        cmd("level_up", "Edit", "Higher level", &["PageUp"]),
        cmd("level_down", "Edit", "Lower level", &["PageDown"]),
        cmd("dyntopo", "Edit", "Dynamic topology on/off", &["T"]),
        cmd("remesh", "Edit", "Voxel remesh", &["R"]),
        cmd("clear_mask", "Edit", "Clear mask", &[]),
        cmd("invert_mask", "Edit", "Invert mask", &[]),
        cmd("front", "View", "Front view", &["1"]),
        cmd("back", "View", "Back view", &["Ctrl+1"]),
        cmd("right", "View", "Right view", &["3"]),
        cmd("left", "View", "Left view", &["Ctrl+3"]),
        cmd("top", "View", "Top view", &["7"]),
        cmd("bottom", "View", "Bottom view", &["Ctrl+7"]),
        cmd("frame", "View", "Frame mesh", &["Home", "."]),
        cmd("wireframe", "View", "Wireframe on/off", &["W"]),
        cmd("polygroups", "View", "Polygroup colours on/off", &["Shift+G"]),
        cmd("radius_down", "Brush", "Smaller brush", &["["]),
        cmd("radius_up", "Brush", "Bigger brush", &["]"]),
        cmd("mirror_x", "Brush", "Mirror X on/off", &["X"]),
        cmd("negative", "Brush", "Negative on/off", &["N"]),
        cmd("next_mode", "Modes", "Next mode", &["Tab"]),
        cmd("prev_mode", "Modes", "Previous mode", &["Shift+Tab"]),
        cmd("close_face", "Modes", "Retopology: close the face", &["Enter"]),
    ];
    for m in Mode::ALL {
        v.push(Command {
            id: format!("mode:{}", m.name()),
            group: "Modes",
            label: format!("{} mode", m.name()),
            defaults: Vec::new(),
        });
    }
    for t in Tool::ALL {
        v.push(Command {
            id: format!("tool:{}", t.name()),
            group: "Brushes",
            label: t.name().to_string(),
            defaults: t.key().map(|c| vec![c.to_string()]).unwrap_or_default(),
        });
    }
    v
}

/// The current bindings: command id -> keys.
pub type KeyMap = HashMap<String, Vec<String>>;

pub fn defaults() -> KeyMap {
    commands().into_iter().map(|c| (c.id, c.defaults)).collect()
}

/// Defaults overlaid with the user's changes.
pub fn load() -> KeyMap {
    let mut map = defaults();
    if let Some(saved) = storage().and_then(|s| s.get_item(KEYS_KEY).ok().flatten()) {
        if let Ok(v) = serde_json::from_str::<HashMap<String, Vec<String>>>(&saved) {
            for (k, keys) in v {
                if map.contains_key(&k) {
                    map.insert(k, keys);
                }
            }
        }
    }
    map
}

/// Stores only the commands that differ from the defaults.
pub fn save(map: &KeyMap) {
    let d = defaults();
    let changed: HashMap<&String, &Vec<String>> = map.iter().filter(|(k, v)| d.get(*k) != Some(*v)).collect();
    if let Some(s) = storage() {
        let _ = s.set_item(KEYS_KEY, &serde_json::to_string(&changed).unwrap_or_default());
    }
}

/// "Ctrl+Shift+Z" from a key press (physical keys, so it works on any layout).
/// None for a lone modifier.
pub fn combo(ev: &web_sys::KeyboardEvent) -> Option<String> {
    let code = ev.code();
    let key = match code.as_str() {
        "ControlLeft" | "ControlRight" | "ShiftLeft" | "ShiftRight" | "AltLeft" | "AltRight" | "MetaLeft"
        | "MetaRight" => return None,
        c if c.starts_with("Key") => c[3..].to_string(),
        c if c.starts_with("Digit") => c[5..].to_string(),
        c if c.starts_with("Numpad") && c.len() == 7 => c[6..].to_string(),
        "BracketLeft" => "[".into(),
        "BracketRight" => "]".into(),
        "Period" => ".".into(),
        "Comma" => ",".into(),
        "Slash" => "/".into(),
        "Semicolon" => ";".into(),
        "Quote" => "'".into(),
        "Minus" => "-".into(),
        "Equal" => "=".into(),
        "Backquote" => "`".into(),
        "Backslash" => "\\".into(),
        "Space" => "Space".into(),
        "NumpadEnter" => "Enter".into(),
        "" => ev.key(),
        c => c.to_string(),
    };
    let mut s = String::new();
    if ev.ctrl_key() || ev.meta_key() {
        s.push_str("Ctrl+");
    }
    if ev.alt_key() {
        s.push_str("Alt+");
    }
    if ev.shift_key() {
        s.push_str("Shift+");
    }
    s.push_str(&key);
    Some(s)
}

/// Keys shown for a command ("—" if none).
pub fn keys_label(map: &KeyMap, id: &str) -> String {
    match map.get(id) {
        Some(k) if !k.is_empty() => k.join(" / "),
        _ => "—".into(),
    }
}

fn run(ui: Ui, id: &str) {
    match id {
        "open" => act(ui, Action::OpenProject),
        "save" => act(ui, Action::SaveProject),
        "save_as" => act(ui, Action::SaveProjectAs),
        "import" => act(ui, Action::Import),
        "export" => act(ui, Action::ExportObj),
        "render" => act(ui, Action::RenderImage),
        "turntable" => act(ui, Action::RenderTurntable),
        "undo" => act(ui, Action::Undo),
        "redo" => act(ui, Action::Redo),
        "subdivide" => act(ui, Action::Subdivide),
        "level_up" => act(ui, Action::LevelUp),
        "level_down" => act(ui, Action::LevelDown),
        "dyntopo" => act(ui, Action::ToggleDyntopo),
        "remesh" => act(ui, Action::VoxelRemesh),
        "clear_mask" => act(ui, Action::ClearMask),
        "invert_mask" => act(ui, Action::InvertMask),
        "front" => act(ui, Action::View(ViewDir::Front)),
        "back" => act(ui, Action::View(ViewDir::Back)),
        "right" => act(ui, Action::View(ViewDir::Right)),
        "left" => act(ui, Action::View(ViewDir::Left)),
        "top" => act(ui, Action::View(ViewDir::Top)),
        "bottom" => act(ui, Action::View(ViewDir::Bottom)),
        "frame" => act(ui, Action::FrameMesh),
        "wireframe" => ui.view.update(|s| s.wireframe = !s.wireframe),
        "polygroups" => ui.view.update(|s| s.polygroups = !s.polygroups),
        "radius_down" => ui.radius.update(|r| *r = (*r / 1.15).max(4.0)),
        "radius_up" => ui.radius.update(|r| *r = (*r * 1.15).min(400.0)),
        "mirror_x" => ui.symmetry.update(|s| s.mirror[0] = !s.mirror[0]),
        "negative" => ui.negative.update(|s| *s = !*s),
        "next_mode" | "prev_mode" => {
            let m = ui.mode.get_untracked();
            let i = Mode::ALL.iter().position(|x| *x == m).unwrap_or(0);
            let n = Mode::ALL.len();
            let next = if id == "prev_mode" { (i + n - 1) % n } else { (i + 1) % n };
            ui.mode.set(Mode::ALL[next]);
        }
        "close_face" => {
            with_ed(|e| e.retopo_close_face());
            ui.refresh();
        }
        _ => {
            if let Some(name) = id.strip_prefix("mode:") {
                if let Some(m) = Mode::ALL.iter().find(|m| m.name() == name) {
                    ui.mode.set(*m);
                }
            } else if let Some(name) = id.strip_prefix("tool:") {
                if let Some(t) = Tool::ALL.iter().find(|t| t.name() == name) {
                    ui.tool.set(*t);
                    ui.mode.set(Mode::Sculpt);
                }
            }
        }
    }
}

pub fn handle_key(ui: Ui, ev: &web_sys::KeyboardEvent) {
    ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key()));
    super::conveniences::touch();
    // the shortcut editor is waiting for a key
    if let Some(id) = ui.key_capture.get_untracked() {
        ev.prevent_default();
        ev.stop_propagation();
        match ev.code().as_str() {
            "Escape" => ui.key_capture.set(None),
            "Backspace" | "Delete" => {
                ui.keymap.update(|m| {
                    m.insert(id.clone(), Vec::new());
                });
                save(&ui.keymap.get_untracked());
                ui.key_capture.set(None);
            }
            _ => {
                if let Some(c) = combo(ev) {
                    let mut taken_from = None;
                    ui.keymap.update(|m| {
                        for (other, keys) in m.iter_mut() {
                            if *other != id && keys.contains(&c) {
                                keys.retain(|k| *k != c);
                                taken_from = Some(other.clone());
                            }
                        }
                        let keys = m.entry(id.clone()).or_default();
                        if !keys.contains(&c) {
                            keys.push(c.clone());
                        }
                    });
                    save(&ui.keymap.get_untracked());
                    ui.key_capture.set(None);
                    if let Some(o) = taken_from {
                        let label = commands().into_iter().find(|x| x.id == o).map(|x| x.label).unwrap_or(o);
                        ui.status.set(format!("{c} now does this; it was taken from “{label}”"));
                    }
                }
            }
        }
        return;
    }
    if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
        let tag = el.tag_name();
        let text_input = tag == "TEXTAREA"
            || tag == "SELECT"
            || (tag == "INPUT" && matches!(el.get_attribute("type").as_deref(), Some("text") | Some("number") | None));
        if text_input {
            return;
        }
    }
    if ev.code() == "Escape" {
        ui.menu.set(None);
        ui.show_keys.set(false);
        with_ed(|e| e.retopo.pending.clear());
        ui.refresh();
        return;
    }
    let Some(c) = combo(ev) else { return };
    let map = ui.keymap.get_untracked();
    if let Some((id, _)) = map.iter().find(|(_, keys)| keys.contains(&c)) {
        // Enter only means something in retopology
        if id == "close_face" && ui.mode.get_untracked() != Mode::Retopo {
            return;
        }
        let id = id.clone();
        ev.prevent_default();
        run(ui, &id);
    }
}

/// The shortcut editor (a dialog over the window).
#[component]
pub fn ShortcutEditor(ui: Ui) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let all = commands();
    let rows = move || {
        let f = filter.get().to_lowercase();
        let map = ui.keymap.get();
        let capture = ui.key_capture.get();
        GROUPS
            .iter()
            .map(|g| {
                let items: Vec<_> = all
                    .iter()
                    .filter(|c| c.group == *g && (f.is_empty() || c.label.to_lowercase().contains(&f)))
                    .map(|c| {
                        let id = c.id.clone();
                        let id2 = c.id.clone();
                        let waiting = capture.as_deref() == Some(c.id.as_str());
                        let keys = keys_label(&map, &c.id);
                        let changed = map.get(&c.id) != Some(&c.defaults);
                        let defaults = c.defaults.clone();
                        view! {
                            <div class="key-row">
                                <span class="key-label">{c.label.clone()}</span>
                                <button class="key-chip" class:waiting=waiting title="Click, then press the new key (Backspace clears, Esc cancels)"
                                    on:click=move |_| ui.key_capture.set(Some(id.clone()))>
                                    {if waiting { "Press a key…".to_string() } else { keys }}
                                </button>
                                <button class="mini" disabled=!changed title="Back to the default" on:click=move |_| {
                                    ui.keymap.update(|m| {
                                        m.insert(id2.clone(), defaults.clone());
                                    });
                                    save(&ui.keymap.get_untracked());
                                }>"↺"</button>
                            </div>
                        }
                    })
                    .collect();
                let empty = items.is_empty();
                view! {
                    <Show when=move || !empty>
                        <div class="key-group">{*g}</div>
                    </Show>
                    {items}
                }
            })
            .collect_view()
    };
    view! {
        <div class="modal-backdrop" on:pointerdown=move |_| {
            ui.key_capture.set(None);
            ui.show_keys.set(false);
        }></div>
        <div class="modal keys-modal">
            <div class="modal-head">
                <b>"Keyboard shortcuts"</b>
                <input type="text" placeholder="Search…" prop:value=move || filter.get() on:input=move |ev| filter.set(event_target_value(&ev)) />
                <button on:click=move |_| {
                    ui.keymap.set(defaults());
                    save(&ui.keymap.get_untracked());
                }>"Reset all"</button>
                <button class="primary" on:click=move |_| {
                    ui.key_capture.set(None);
                    ui.show_keys.set(false);
                }>"Done"</button>
            </div>
            <div class="hint">"Click a shortcut, then press the keys you want. A key already in use moves to the new command. Backspace removes a shortcut; Esc cancels. Mouse controls: left drag sculpts, right drag or Alt+left rotates, middle drag or Shift+right pans, wheel zooms, Shift smooths, Ctrl inverts."</div>
            <div class="keys-list">{rows}</div>
        </div>
    }
}
