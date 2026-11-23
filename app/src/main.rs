//! Clayform's interface (Leptos, compiled to WebAssembly). The sculpting
//! engine runs right here in the page so strokes never wait on the host;
//! the Tauri shell only provides native file dialogs.

mod host;
mod render;

use clayform_core::editor::{Brush, Button, Drag, Editor, Mods, ViewDir};
use clayform_core::glam::Vec2;
use clayform_core::io;
use clayform_core::mesh::Mesh;
use clayform_core::sculpt::{Tool, MAX_TRIANGLES};
use leptos::ev;
use leptos::prelude::*;
use leptos::task::spawn_local;
use render::{Renderer, ViewSettings, BACKGROUNDS, MATERIALS};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

thread_local! {
    static EDITOR: RefCell<Editor> = RefCell::new(Editor::new());
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

fn with_ed<R>(f: impl FnOnce(&mut Editor) -> R) -> R {
    EDITOR.with(|e| f(&mut e.borrow_mut()))
}

const PANEL_SIDE_KEY: &str = "clayform.panelSide";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// Interface state. Every field is a cheap, copyable signal handle.
#[derive(Clone, Copy)]
struct Ui {
    tool: RwSignal<Tool>,
    radius: RwSignal<f32>,
    intensity: RwSignal<[f32; Tool::ALL.len()]>,
    negative: RwSignal<bool>,
    symmetry: RwSignal<bool>,
    view: RwSignal<ViewSettings>,
    fov: RwSignal<f32>,
    panel_right: RwSignal<bool>,
    sphere_detail: RwSignal<u32>,
    stats: RwSignal<(usize, usize)>,
    can_undo: RwSignal<bool>,
    can_redo: RwSignal<bool>,
    status: RwSignal<String>,
    menu: RwSignal<Option<&'static str>>,
    cursor: RwSignal<Option<(f32, f32)>>,
    mirror: RwSignal<Option<(f32, f32)>>,
    mods: RwSignal<(bool, bool)>,
    dragging: RwSignal<Drag>,
    gpu: RwSignal<Option<String>>,
}

impl Ui {
    fn new() -> Self {
        let b = Brush::default();
        Self {
            tool: RwSignal::new(b.tool),
            radius: RwSignal::new(b.radius_px),
            intensity: RwSignal::new(b.intensity),
            negative: RwSignal::new(b.negative),
            symmetry: RwSignal::new(b.symmetry),
            view: RwSignal::new(ViewSettings { material: 0, show_mask: true, flat: false, wireframe: false, background: 0 }),
            fov: RwSignal::new(40.0),
            panel_right: RwSignal::new(
                storage().and_then(|s| s.get_item(PANEL_SIDE_KEY).ok().flatten()).is_some_and(|v| v == "right"),
            ),
            sphere_detail: RwSignal::new(6),
            stats: RwSignal::new((0, 0)),
            can_undo: RwSignal::new(false),
            can_redo: RwSignal::new(false),
            status: RwSignal::new("Ready — drag on the sphere to sculpt".into()),
            menu: RwSignal::new(None),
            cursor: RwSignal::new(None),
            mirror: RwSignal::new(None),
            mods: RwSignal::new((false, false)),
            dragging: RwSignal::new(Drag::None),
            gpu: RwSignal::new(None),
        }
    }

    /// Pulls mesh statistics and undo state from the engine.
    fn refresh(self) {
        let (v, t, u, r) = with_ed(|e| {
            (e.sculptor.mesh.vertex_count(), e.sculptor.tri_count(), e.sculptor.can_undo(), e.sculptor.can_redo())
        });
        self.stats.set((v, t));
        self.can_undo.set(u);
        self.can_redo.set(r);
    }
}

fn fmt_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

// ----------------------------------------------------------------- actions

#[derive(Clone, Copy)]
enum Action {
    NewSphere,
    NewCube,
    NewPlane,
    Import,
    ExportObj,
    ExportStl,
    Undo,
    Redo,
    Subdivide,
    ClearMask,
    InvertMask,
    FrameMesh,
    ResetCamera,
    View(ViewDir),
    TogglePanelSide,
}

fn act(ui: Ui, a: Action) {
    ui.menu.set(None);
    match a {
        Action::NewSphere => new_mesh(ui, Mesh::icosphere(ui.sphere_detail.get_untracked()), "sphere"),
        Action::NewCube => new_mesh(ui, Mesh::cube(6), "cube"),
        Action::NewPlane => new_mesh(ui, Mesh::plane(7), "plane"),
        Action::Import => spawn_local(async move {
            match host::open_mesh().await {
                Ok(Some(f)) => match io::load(&f.name, &f.bytes) {
                    Ok(m) => new_mesh(ui, m, &f.name),
                    Err(e) => ui.status.set(format!("Import failed: {e}")),
                },
                Ok(None) => {}
                Err(e) => ui.status.set(format!("Import failed: {e}")),
            }
        }),
        Action::ExportObj | Action::ExportStl => {
            let ext = if matches!(a, Action::ExportStl) { "stl" } else { "obj" };
            let bytes = with_ed(|e| {
                e.pointer_up();
                io::save(&e.sculptor.mesh, ext)
            });
            spawn_local(async move {
                match host::save_mesh(&format!("sculpt.{ext}"), ext, bytes).await {
                    Ok(Some(p)) => ui.status.set(format!("Saved {p}")),
                    Ok(None) => {}
                    Err(e) => ui.status.set(format!("Export failed: {e}")),
                }
            });
        }
        Action::Undo => with_ed(|e| e.undo()),
        Action::Redo => with_ed(|e| e.redo()),
        Action::Subdivide => {
            let r = with_ed(|e| {
                let r = e.sculptor.subdivide();
                e.redraw = true;
                r
            });
            match r {
                Ok(()) => ui.status.set(format!("Subdivided — {} triangles", fmt_count(with_ed(|e| e.sculptor.tri_count())))),
                Err(e) => ui.status.set(e),
            }
        }
        Action::ClearMask => with_ed(|e| {
            e.sculptor.clear_mask();
            e.redraw = true;
        }),
        Action::InvertMask => with_ed(|e| {
            e.sculptor.invert_mask();
            e.redraw = true;
        }),
        Action::FrameMesh => with_ed(|e| e.frame_mesh()),
        Action::ResetCamera => with_ed(|e| e.reset_camera()),
        Action::View(d) => with_ed(|e| e.view_from(d)),
        Action::TogglePanelSide => ui.panel_right.update(|r| *r = !*r),
    }
    ui.refresh();
}

fn new_mesh(ui: Ui, m: Mesh, name: &str) {
    let tris = m.tris.len();
    with_ed(|e| e.set_mesh(m));
    ui.status.set(format!("{name} — {} triangles", fmt_count(tris)));
    ui.refresh();
}

// ------------------------------------------------------------------- frame loop

fn request_frame(f: &Closure<dyn FnMut()>) {
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

fn start_frame_loop(canvas: web_sys::HtmlCanvasElement, ui: Ui) {
    let cell: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let next = cell.clone();
    *cell.borrow_mut() = Some(Closure::new(move || {
        draw_frame(&canvas, ui);
        if let Some(f) = next.borrow().as_ref() {
            request_frame(f);
        }
    }));
    let first = cell.borrow();
    if let Some(f) = first.as_ref() {
        request_frame(f);
    }
}

fn draw_frame(canvas: &web_sys::HtmlCanvasElement, ui: Ui) {
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let (cw, ch) = (canvas.client_width() as f64, canvas.client_height() as f64);
    if cw < 1.0 || ch < 1.0 {
        return;
    }
    let (w, h) = ((cw * dpr).round() as u32, (ch * dpr).round() as u32);
    let mut resized = false;
    if canvas.width() != w || canvas.height() != h {
        canvas.set_width(w);
        canvas.set_height(h);
        resized = true;
    }
    let view = ui.view.get_untracked();
    with_ed(|e| {
        if !(e.redraw || resized) {
            return;
        }
        e.redraw = false;
        RENDERER.with(|r| {
            if let Some(r) = r.borrow_mut().as_mut() {
                r.sync_mesh(&e.sculptor.mesh, &mut e.sculptor.dirty, view.wireframe);
                r.render([w, h], e.camera.view(), e.camera.proj((cw / ch) as f32), &view);
            }
        });
    });
}

// ----------------------------------------------------------------- viewport

fn local_pos(ev: &web_sys::MouseEvent) -> Vec2 {
    Vec2::new(ev.offset_x() as f32, ev.offset_y() as f32)
}

fn canvas_size(ev: &web_sys::Event) -> Vec2 {
    ev.current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el| Vec2::new(el.client_width() as f32, el.client_height() as f32))
        .unwrap_or(Vec2::ONE)
}

#[component]
fn Viewport(ui: Ui) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let started = StoredValue::new(false);
    Effect::new(move || {
        let Some(canvas) = canvas_ref.get() else { return };
        if started.get_value() {
            return;
        }
        started.set_value(true);
        let canvas: web_sys::HtmlCanvasElement = canvas;
        spawn_local(async move {
            match Renderer::new(canvas.clone()).await {
                Ok(r) => {
                    log(&format!("Clayform renderer: {}", r.adapter_name));
                    RENDERER.with(|slot| *slot.borrow_mut() = Some(r));
                    with_ed(|e| e.redraw = true);
                    start_frame_loop(canvas, ui);
                }
                Err(e) => ui.gpu.set(Some(e)),
            }
        });
    });

    let on_down = move |ev: ev::PointerEvent| {
        ev.prevent_default();
        ui.menu.set(None);
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        let button = match ev.button() {
            1 => Button::Middle,
            2 => Button::Right,
            _ => Button::Left,
        };
        let mods = Mods { shift: ev.shift_key(), ctrl: ev.ctrl_key() || ev.meta_key(), alt: ev.alt_key() };
        let size = canvas_size(&ev);
        let d = with_ed(|e| e.pointer_down(button, local_pos(&ev), size, mods));
        ui.dragging.set(d);
    };
    let on_move = move |ev: ev::PointerEvent| {
        let p = local_pos(&ev);
        let size = canvas_size(&ev);
        ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key()));
        let mirror = with_ed(|e| {
            if e.drag() != Drag::None {
                e.pointer_move(p, size);
            }
            e.mirror_screen_point(p, size)
        });
        ui.cursor.set(Some((p.x, p.y)));
        ui.mirror.set(mirror.map(|m| (m.x, m.y)));
    };
    let on_up = move |_ev: ev::PointerEvent| {
        with_ed(|e| e.pointer_up());
        ui.dragging.set(Drag::None);
        ui.refresh();
    };
    let on_wheel = move |ev: ev::WheelEvent| {
        ev.prevent_default();
        let scale = if ev.delta_mode() == 1 { 30.0 } else { 1.0 };
        with_ed(|e| e.wheel(-(ev.delta_y() as f32) * scale));
    };

    let brush_visible = move || {
        matches!(ui.dragging.get(), Drag::None | Drag::Sculpt) && ui.cursor.get().is_some()
    };
    let brush_color = move || {
        let (shift, ctrl) = ui.mods.get();
        let _ = (ui.tool.get(), ui.negative.get(), ui.dragging.get());
        let neg = with_ed(|e| e.effective_negative(Mods { shift, ctrl, alt: false }));
        if neg { "#6eaaff" } else { "#ff965a" }
    };

    view! {
        <div class="viewport">
            <canvas
                node_ref=canvas_ref
                on:pointerdown=on_down
                on:pointermove=on_move
                on:pointerup=on_up
                on:pointercancel=on_up
                on:pointerleave=move |_| if ui.dragging.get_untracked() == Drag::None { ui.cursor.set(None); ui.mirror.set(None) }
                on:wheel=on_wheel
                on:contextmenu=|ev: ev::MouseEvent| ev.prevent_default()
            ></canvas>
            <svg class="overlay" style:display=move || if brush_visible() { "block" } else { "none" }>
                <circle
                    cx=move || ui.cursor.get().map(|c| c.0).unwrap_or(-999.0)
                    cy=move || ui.cursor.get().map(|c| c.1).unwrap_or(-999.0)
                    r=move || ui.radius.get()
                    fill="none" stroke=brush_color stroke-width="1.5"
                ></circle>
                <circle
                    cx=move || ui.cursor.get().map(|c| c.0).unwrap_or(-999.0)
                    cy=move || ui.cursor.get().map(|c| c.1).unwrap_or(-999.0)
                    r="1.5" fill=brush_color
                ></circle>
                <circle
                    cx=move || ui.mirror.get().map(|c| c.0).unwrap_or(-999.0)
                    cy=move || ui.mirror.get().map(|c| c.1).unwrap_or(-999.0)
                    r=move || ui.radius.get()
                    fill="none" stroke=brush_color stroke-width="1" stroke-opacity="0.45"
                ></circle>
            </svg>
            <Show when=move || ui.gpu.get().is_some()>
                <div class="gpu-error">
                    <b>"The 3D view could not start"</b>
                    <p>{move || ui.gpu.get().unwrap_or_default()}</p>
                </div>
            </Show>
        </div>
    }
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

// ------------------------------------------------------------------ widgets

/// A top-bar dropdown menu (SculptGL-style).
#[component]
fn Menu(ui: Ui, id: &'static str, label: &'static str, children: Children) -> impl IntoView {
    let open = move || ui.menu.get() == Some(id);
    view! {
        <div class="menu" class:open=open>
            <button
                class="menu-title"
                on:click=move |_| ui.menu.update(|m| *m = if *m == Some(id) { None } else { Some(id) })
                on:pointerenter=move |_| if ui.menu.get_untracked().is_some() { ui.menu.set(Some(id)) }
            >{label}</button>
            <div class="dropdown">{children()}</div>
        </div>
    }
}

#[component]
fn Item(
    ui: Ui,
    action: Action,
    label: &'static str,
    #[prop(optional)] key: &'static str,
    #[prop(optional, into)] enabled: Option<Signal<bool>>,
) -> impl IntoView {
    let disabled = move || enabled.map(|e| !e.get()).unwrap_or(false);
    view! {
        <button class="item" disabled=disabled on:click=move |_| act(ui, action)>
            <span>{label}</span>
            <span class="key">{key}</span>
        </button>
    }
}

/// A collapsible folder in the side panel (SculptGL-style).
#[component]
fn Folder(title: &'static str, #[prop(optional)] closed: bool, children: Children) -> impl IntoView {
    let open = RwSignal::new(!closed);
    view! {
        <section class="folder" class:collapsed=move || !open.get()>
            <button class="folder-title" on:click=move |_| open.update(|o| *o = !*o)>
                <span class="chev">{move || if open.get() { "▾" } else { "▸" }}</span>
                {title}
            </button>
            <div class="folder-body">{children()}</div>
        </section>
    }
}

#[component]
fn Slider(
    label: &'static str,
    min: f32,
    max: f32,
    step: f32,
    #[prop(into)] value: Signal<f32>,
    on_change: impl Fn(f32) + 'static,
    #[prop(optional)] format: Option<fn(f32) -> String>,
) -> impl IntoView {
    let fmt = format.unwrap_or(|v| format!("{v:.2}"));
    view! {
        <label class="row slider">
            <span class="label">{label}</span>
            <input
                type="range"
                min=min.to_string()
                max=max.to_string()
                step=step.to_string()
                prop:value=move || value.get().to_string()
                on:input=move |ev| {
                    if let Ok(v) = event_target_value(&ev).parse::<f32>() {
                        on_change(v)
                    }
                }
            />
            <span class="value">{move || fmt(value.get())}</span>
        </label>
    }
}

#[component]
fn Check(label: &'static str, #[prop(into)] value: Signal<bool>, on_change: impl Fn(bool) + 'static) -> impl IntoView {
    view! {
        <label class="row check">
            <input type="checkbox" prop:checked=move || value.get() on:change=move |ev| on_change(event_target_checked(&ev)) />
            <span>{label}</span>
        </label>
    }
}

// ----------------------------------------------------------------------- app

#[component]
fn MenuBar(ui: Ui) -> impl IntoView {
    let can_undo = Signal::derive(move || ui.can_undo.get());
    let can_redo = Signal::derive(move || ui.can_redo.get());
    view! {
        <header class="menubar">
            <span class="brand">"Clayform"</span>
            <Menu ui id="file" label="File">
                <Item ui action=Action::Import label="Import OBJ / STL…" key="Ctrl+O" />
                <div class="sep"></div>
                <Item ui action=Action::ExportObj label="Export OBJ…" key="Ctrl+S" />
                <Item ui action=Action::ExportStl label="Export STL…" />
            </Menu>
            <Menu ui id="edit" label="Edit">
                <Item ui action=Action::Undo label="Undo" key="Ctrl+Z" enabled=can_undo />
                <Item ui action=Action::Redo label="Redo" key="Ctrl+Shift+Z" enabled=can_redo />
            </Menu>
            <Menu ui id="scene" label="Scene">
                <Item ui action=Action::NewSphere label="New sphere" />
                <Item ui action=Action::NewCube label="New cube" />
                <Item ui action=Action::NewPlane label="New plane" />
                <div class="sep"></div>
                <div class="menu-widget">
                    <Slider
                        label="Sphere detail"
                        min=3.0 max=8.0 step=1.0
                        value=Signal::derive(move || ui.sphere_detail.get() as f32)
                        on_change=move |v| ui.sphere_detail.set(v as u32)
                        format=|v| format!("{v:.0}")
                    />
                </div>
            </Menu>
            <Menu ui id="camera" label="Camera">
                <Item ui action=Action::FrameMesh label="Frame mesh" key="Home" />
                <Item ui action=Action::ResetCamera label="Reset camera" />
                <div class="sep"></div>
                <Item ui action=Action::View(ViewDir::Front) label="Front" key="1" />
                <Item ui action=Action::View(ViewDir::Back) label="Back" key="Ctrl+1" />
                <Item ui action=Action::View(ViewDir::Right) label="Right" key="3" />
                <Item ui action=Action::View(ViewDir::Left) label="Left" key="Ctrl+3" />
                <Item ui action=Action::View(ViewDir::Top) label="Top" key="7" />
                <Item ui action=Action::View(ViewDir::Bottom) label="Bottom" key="Ctrl+7" />
                <div class="sep"></div>
                <div class="menu-widget">
                    <Slider
                        label="Field of view"
                        min=15.0 max=90.0 step=1.0
                        value=ui.fov
                        on_change=move |v| ui.fov.set(v)
                        format=|v| format!("{v:.0}°")
                    />
                </div>
            </Menu>
            <Menu ui id="background" label="Background">
                {BACKGROUNDS
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _, _))| {
                        let i = i as u32;
                        view! {
                            <button
                                class="item radio"
                                class:checked=move || ui.view.get().background == i
                                on:click=move |_| {
                                    ui.view.update(|v| v.background = i);
                                    ui.menu.set(None);
                                }
                            >
                                <span>{*name}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </Menu>
            <Menu ui id="help" label="Help">
                <div class="help">
                    <table>
                        <tr><td>"Left drag on mesh"</td><td>"Sculpt"</td></tr>
                        <tr><td>"Shift + drag"</td><td>"Smooth"</td></tr>
                        <tr><td>"Ctrl + drag"</td><td>"Invert brush"</td></tr>
                        <tr><td>"Left drag off mesh, Alt + left, right drag"</td><td>"Rotate"</td></tr>
                        <tr><td>"Middle drag, Shift + right drag"</td><td>"Pan"</td></tr>
                        <tr><td>"Wheel"</td><td>"Zoom"</td></tr>
                        <tr><td>"D C S F I P E G M"</td><td>"Brush tools"</td></tr>
                        <tr><td>"[  ]"</td><td>"Brush radius"</td></tr>
                        <tr><td>"X / W / N"</td><td>"Symmetry / wireframe / negative"</td></tr>
                        <tr><td>"Ctrl+D"</td><td>"Subdivide"</td></tr>
                    </table>
                    <p class="about">"Clayform — a SculptGL-style sculpting app (Tauri + Leptos)."</p>
                </div>
            </Menu>
            <div class="spacer"></div>
            <button
                class="side-toggle"
                title=move || if ui.panel_right.get() { "Move the panel to the left" } else { "Move the panel to the right" }
                on:click=move |_| act(ui, Action::TogglePanelSide)
            >
                {move || if ui.panel_right.get() { "◧ Panel left" } else { "Panel right ◨" }}
            </button>
        </header>
    }
}

#[component]
fn SidePanel(ui: Ui) -> impl IntoView {
    let intensity = Signal::derive(move || ui.intensity.get()[ui.tool.get().index()]);
    let too_dense = move || ui.stats.get().1 * 4 > MAX_TRIANGLES;
    let subdiv_label = move || format!("Subdivide  ({} tris)", fmt_count(ui.stats.get().1 * 4));
    view! {
        <aside class="panel">
            <Folder title="Sculpting">
                <label class="row">
                    <span class="label">"Tool"</span>
                    <select on:change=move |ev| {
                        if let Ok(i) = event_target_value(&ev).parse::<usize>() {
                            ui.tool.set(Tool::ALL[i]);
                        }
                    }>
                        {Tool::ALL
                            .iter()
                            .map(|t| {
                                let t = *t;
                                view! {
                                    <option value=t.index().to_string() selected=move || ui.tool.get() == t>
                                        {format!("{}  ({})", t.name(), t.shortcut())}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <Slider
                    label="Radius"
                    min=4f32.ln() max=400f32.ln() step=0.01
                    value=Signal::derive(move || ui.radius.get().ln())
                    on_change=move |v| ui.radius.set(v.exp())
                    format=|v| format!("{:.0}", v.exp())
                />
                <Slider
                    label="Intensity"
                    min=0.0 max=1.0 step=0.01
                    value=intensity
                    on_change=move |v| ui.intensity.update(|a| a[ui.tool.get_untracked().index()] = v)
                />
                <Check label="Negative  (N / hold Ctrl)" value=ui.negative on_change=move |v| ui.negative.set(v) />
                <Check label="Symmetry X  (X)" value=ui.symmetry on_change=move |v| ui.symmetry.set(v) />
            </Folder>
            <Folder title="Masking">
                <div class="row buttons">
                    <button on:click=move |_| ui.tool.set(Tool::Mask)>"Mask brush"</button>
                    <button on:click=move |_| act(ui, Action::ClearMask)>"Clear"</button>
                    <button on:click=move |_| act(ui, Action::InvertMask)>"Invert"</button>
                </div>
                <Check
                    label="Show mask"
                    value=Signal::derive(move || ui.view.get().show_mask)
                    on_change=move |v| ui.view.update(|s| s.show_mask = v)
                />
            </Folder>
            <Folder title="Topology">
                <div class="row buttons">
                    <button
                        disabled=too_dense
                        on:click=move |_| act(ui, Action::Subdivide)
                    >{subdiv_label}</button>
                </div>
                <div class="row info">
                    {move || {
                        let (v, t) = ui.stats.get();
                        format!("{} vertices · {} triangles", fmt_count(v), fmt_count(t))
                    }}
                </div>
            </Folder>
            <Folder title="Rendering">
                <label class="row">
                    <span class="label">"Material"</span>
                    <select on:change=move |ev| {
                        if let Ok(i) = event_target_value(&ev).parse::<u32>() {
                            ui.view.update(|s| s.material = i);
                        }
                    }>
                        {MATERIALS
                            .iter()
                            .enumerate()
                            .map(|(i, name)| {
                                let i = i as u32;
                                view! {
                                    <option value=i.to_string() selected=move || ui.view.get().material == i>{*name}</option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <Check
                    label="Flat shading"
                    value=Signal::derive(move || ui.view.get().flat)
                    on_change=move |v| ui.view.update(|s| s.flat = v)
                />
                <Check
                    label="Wireframe  (W)"
                    value=Signal::derive(move || ui.view.get().wireframe)
                    on_change=move |v| ui.view.update(|s| s.wireframe = v)
                />
            </Folder>
            <Folder title="Interface" closed=true>
                <div class="row">
                    <span class="label">"Panel side"</span>
                    <div class="segmented">
                        <button class:on=move || !ui.panel_right.get() on:click=move |_| ui.panel_right.set(false)>"Left"</button>
                        <button class:on=move || ui.panel_right.get() on:click=move |_| ui.panel_right.set(true)>"Right"</button>
                    </div>
                </div>
            </Folder>
        </aside>
    }
}

fn handle_key(ui: Ui, ev: &web_sys::KeyboardEvent) {
    ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key()));
    if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
        let tag = el.tag_name();
        let text_input = tag == "TEXTAREA"
            || tag == "SELECT"
            || (tag == "INPUT" && matches!(el.get_attribute("type").as_deref(), Some("text") | Some("number") | None));
        if text_input {
            return;
        }
    }
    let ctrl = ev.ctrl_key() || ev.meta_key();
    let key = ev.key();
    let k = key.to_ascii_lowercase();
    let mut handled = true;
    if ctrl {
        match k.as_str() {
            "z" if ev.shift_key() => act(ui, Action::Redo),
            "z" => act(ui, Action::Undo),
            "y" => act(ui, Action::Redo),
            "o" => act(ui, Action::Import),
            "s" => act(ui, Action::ExportObj),
            "d" => act(ui, Action::Subdivide),
            "1" => act(ui, Action::View(ViewDir::Back)),
            "3" => act(ui, Action::View(ViewDir::Left)),
            "7" => act(ui, Action::View(ViewDir::Bottom)),
            _ => handled = false,
        }
    } else if ev.alt_key() {
        handled = false;
    } else {
        match k.as_str() {
            "[" => ui.radius.update(|r| *r = (*r / 1.15).max(4.0)),
            "]" => ui.radius.update(|r| *r = (*r * 1.15).min(400.0)),
            "w" => ui.view.update(|s| s.wireframe = !s.wireframe),
            "x" => ui.symmetry.update(|s| *s = !*s),
            "n" => ui.negative.update(|s| *s = !*s),
            "home" | "." => act(ui, Action::FrameMesh),
            "1" => act(ui, Action::View(ViewDir::Front)),
            "3" => act(ui, Action::View(ViewDir::Right)),
            "7" => act(ui, Action::View(ViewDir::Top)),
            "escape" => ui.menu.set(None),
            _ => {
                if let Some(t) = Tool::ALL.iter().find(|t| t.shortcut().to_ascii_lowercase().to_string() == k) {
                    ui.tool.set(*t);
                } else {
                    handled = false;
                }
            }
        }
    }
    if handled {
        ev.prevent_default();
    }
}

#[component]
fn App() -> impl IntoView {
    let ui = Ui::new();
    ui.refresh();

    // brush settings -> engine
    Effect::new(move || {
        let brush = Brush {
            tool: ui.tool.get(),
            radius_px: ui.radius.get(),
            intensity: ui.intensity.get(),
            negative: ui.negative.get(),
            symmetry: ui.symmetry.get(),
        };
        with_ed(|e| e.brush = brush);
    });
    // display settings -> redraw
    Effect::new(move || {
        ui.view.track();
        let fov = ui.fov.get();
        with_ed(|e| {
            e.camera.fov_y = fov.to_radians();
            e.redraw = true;
        });
    });
    // remember which side the panel is on
    Effect::new(move || {
        let right = ui.panel_right.get();
        if let Some(s) = storage() {
            let _ = s.set_item(PANEL_SIDE_KEY, if right { "right" } else { "left" });
        }
        with_ed(|e| e.redraw = true);
    });

    let keydown = window_event_listener(ev::keydown, move |ev| handle_key(ui, &ev));
    let keyup = window_event_listener(ev::keyup, move |ev| ui.mods.set((ev.shift_key(), ev.ctrl_key() || ev.meta_key())));
    on_cleanup(move || {
        keydown.remove();
        keyup.remove();
    });

    view! {
        <div class="app" class:panel-right=move || ui.panel_right.get()>
            <MenuBar ui />
            <div class="main">
                <SidePanel ui />
                <Viewport ui />
            </div>
            <footer class="status">
                <span>{move || {
                    let (v, t) = ui.stats.get();
                    format!("{} verts · {} tris", fmt_count(v), fmt_count(t))
                }}</span>
                <span class="sep">"|"</span>
                <span>{move || {
                    let t = ui.tool.get();
                    format!("{}{}", t.name(), if ui.symmetry.get() { " · Sym X" } else { "" })
                }}</span>
                <span class="sep">"|"</span>
                <span class="msg">{move || ui.status.get()}</span>
            </footer>
            <Show when=move || ui.menu.get().is_some()>
                <div class="menu-backdrop" on:pointerdown=move |_| ui.menu.set(None)></div>
            </Show>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
