//! egui application: panels, viewport input and per-frame orchestration.

use crate::camera::Camera;
use crate::mesh::Mesh;
use crate::render::{Renderer, ViewSettings};
use crate::sculpt::{mirror_x, DabParams, Sculptor, Tool};
use egui::{Color32, Key, Modifiers, PointerButton, Pos2, Rect, Sense, Stroke};
use glam::{Vec2, Vec3};
use std::path::PathBuf;
use std::time::Instant;

#[derive(PartialEq, Clone, Copy)]
enum Drag {
    None,
    Sculpt,
    Orbit,
    Pan,
}

struct GrabAnchor {
    point: Vec3,
    normal: Vec3,
}

const PANEL_SIDE_KEY: &str = "panel_side";

const MATERIALS: [&str; 6] = ["Clay", "Red wax", "Pearl", "Jade", "Chrome", "Normals"];

pub struct App {
    sculptor: Sculptor,
    renderer: Option<Renderer>,
    camera: Camera,
    tool: Tool,
    intensity: [f32; Tool::ALL.len()],
    radius_px: f32,
    negative: bool,
    symmetry: bool,
    view: ViewSettings,
    drag: Drag,
    stroke_tool: Tool,
    stroke_negative: bool,
    last_dab: Vec2,
    grab: Option<GrabAnchor>,
    sphere_level: u32,
    /// Tools / settings panel docked on the right of the viewport.
    panel_right: bool,
    saved_panel_right: bool,
    status: String,
    status_at: Instant,
    frame_ms: f32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        setup_style(&cc.egui_ctx);
        let renderer = cc.wgpu_render_state.as_ref().map(|rs| Renderer::new(&rs.device, &rs.queue));
        let mut app = Self {
            sculptor: Sculptor::new(Mesh::icosphere(6)),
            renderer,
            camera: Camera::default(),
            tool: Tool::Draw,
            intensity: Tool::ALL.map(|t| t.default_intensity()),
            radius_px: 60.0,
            negative: false,
            symmetry: true,
            view: ViewSettings { material: 0, show_mask: true, flat: false, wireframe: false },
            drag: Drag::None,
            stroke_tool: Tool::Draw,
            stroke_negative: false,
            last_dab: Vec2::ZERO,
            grab: None,
            sphere_level: 6,
            panel_right: cc
                .storage
                .and_then(|s| s.get_string(PANEL_SIDE_KEY))
                .is_some_and(|v| v == "right"),
            saved_panel_right: false,
            status: "Ready — drag on the sphere to sculpt".into(),
            status_at: Instant::now(),
            frame_ms: 0.0,
        };
        app.saved_panel_right = app.panel_right;
        if let Some(p) = open {
            app.load(p);
        }
        app
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Instant::now();
    }

    fn frame_mesh(&mut self) {
        let (mn, mx) = self.sculptor.mesh.bounds();
        self.camera.frame(mn, mx);
    }

    fn new_mesh(&mut self, m: Mesh, name: &str) {
        let tris = m.tris.len();
        self.sculptor.set_mesh(m);
        self.frame_mesh();
        self.set_status(format!("New {name} — {} triangles", fmt_count(tris)));
    }

    fn load(&mut self, path: PathBuf) {
        match crate::io::load(&path) {
            Ok(m) => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                self.new_mesh(m, &name);
            }
            Err(e) => self.set_status(format!("Import failed: {e}")),
        }
    }

    fn import_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Mesh", &["obj", "stl", "OBJ", "STL"]).pick_file() {
            self.load(p);
        }
    }

    fn export_dialog(&mut self, ext: &str) {
        self.sculptor.end_stroke();
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(ext.to_uppercase(), &[ext])
            .set_file_name(format!("sculpt.{ext}"))
            .save_file()
        {
            let p = if p.extension().is_none() { p.with_extension(ext) } else { p };
            match crate::io::save(&self.sculptor.mesh, &p) {
                Ok(()) => self.set_status(format!("Saved {}", p.display())),
                Err(e) => self.set_status(format!("Export failed: {e}")),
            }
        }
    }

    fn subdivide(&mut self) {
        match self.sculptor.subdivide() {
            Ok(()) => {
                let t = self.sculptor.tri_count();
                self.set_status(format!("Subdivided — {} triangles", fmt_count(t)));
            }
            Err(e) => self.set_status(e),
        }
    }

    // ------------------------------------------------------------------ input

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let (undo, redo, redo2, open, save, subdiv) = ctx.input_mut(|i| {
            (
                i.consume_key(cmd, Key::Z),
                i.consume_key(cmd_shift, Key::Z),
                i.consume_key(cmd, Key::Y),
                i.consume_key(cmd, Key::O),
                i.consume_key(cmd, Key::S),
                i.consume_key(cmd, Key::D),
            )
        });
        if undo && self.drag == Drag::None {
            self.sculptor.undo();
        }
        if (redo || redo2) && self.drag == Drag::None {
            self.sculptor.redo();
        }
        if open {
            self.import_dialog();
        }
        if save {
            self.export_dialog("obj");
        }
        if subdiv {
            self.subdivide();
        }
        ctx.input(|i| {
            if i.modifiers.command || i.modifiers.alt {
                return;
            }
            for t in Tool::ALL {
                if let Some(k) = Key::from_name(&t.shortcut().to_string()) {
                    if i.key_pressed(k) {
                        self.tool = t;
                    }
                }
            }
            if i.key_pressed(Key::OpenBracket) {
                self.radius_px = (self.radius_px / 1.15).max(4.0);
            }
            if i.key_pressed(Key::CloseBracket) {
                self.radius_px = (self.radius_px * 1.15).min(400.0);
            }
            if i.key_pressed(Key::W) {
                self.view.wireframe = !self.view.wireframe;
            }
            if i.key_pressed(Key::X) {
                self.symmetry = !self.symmetry;
            }
            if i.key_pressed(Key::N) {
                self.negative = !self.negative;
            }
        });
        if ctx.input(|i| i.key_pressed(Key::Home) || i.key_pressed(Key::Period)) {
            self.frame_mesh();
        }
    }

    fn dab_at(&mut self, p: Vec2, size: Vec2) {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return };
        let r = self.radius_px * self.camera.world_per_point(h.point, size.y);
        let mut params = DabParams {
            tool: self.stroke_tool,
            center: h.point,
            radius: r,
            intensity: self.intensity[self.stroke_tool.index()],
            negative: self.stroke_negative,
            seed_tri: h.tri,
        };
        self.sculptor.dab(&params);
        if self.symmetry {
            if let Some(hm) = self.sculptor.raycast(mirror_x(o), mirror_x(d)) {
                params.center = hm.point;
                params.seed_tri = hm.tri;
                self.sculptor.dab(&params);
            }
        }
    }

    fn begin_sculpt(&mut self, p: Vec2, size: Vec2, mods: Modifiers) -> bool {
        let (o, d) = self.camera.ray(p, size);
        let Some(h) = self.sculptor.raycast(o, d) else { return false };
        self.stroke_tool = if mods.shift && self.tool != Tool::Mask { Tool::Smooth } else { self.tool };
        self.stroke_negative = self.negative ^ mods.command;
        self.sculptor.begin_stroke();
        if self.stroke_tool == Tool::Grab {
            let r = self.radius_px * self.camera.world_per_point(h.point, size.y);
            self.sculptor.grab_begin(h.point, r, h.tri, false);
            if self.symmetry {
                if let Some(hm) = self.sculptor.raycast(mirror_x(o), mirror_x(d)) {
                    self.sculptor.grab_begin(hm.point, r, hm.tri, true);
                }
            }
            self.grab = Some(GrabAnchor { point: h.point, normal: self.camera.forward() });
        } else {
            self.dab_at(p, size);
            self.last_dab = p;
        }
        true
    }

    fn continue_sculpt(&mut self, p: Vec2, size: Vec2) {
        if self.stroke_tool == Tool::Grab {
            let Some(a) = &self.grab else { return };
            let (o, d) = self.camera.ray(p, size);
            let denom = d.dot(a.normal);
            if denom.abs() > 1e-6 {
                let t = (a.point - o).dot(a.normal) / denom;
                let delta = o + d * t - a.point;
                self.sculptor.grab_update(delta);
            }
            return;
        }
        let spacing = (self.radius_px * 0.1).max(1.0);
        let mut from = self.last_dab;
        let mut dist = (p - from).length();
        while dist >= spacing {
            let dir = (p - from) / dist;
            from += dir * spacing;
            dist -= spacing;
            self.dab_at(from, size);
        }
        self.last_dab = from;
    }

    fn viewport_input(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect) {
        let size = Vec2::new(rect.width(), rect.height());
        let (pos, delta, scroll, mods, pressed, down) = ctx.input(|i| {
            let pressed = [
                i.pointer.button_pressed(PointerButton::Primary),
                i.pointer.button_pressed(PointerButton::Secondary),
                i.pointer.button_pressed(PointerButton::Middle),
            ];
            let down = [
                i.pointer.button_down(PointerButton::Primary),
                i.pointer.button_down(PointerButton::Secondary),
                i.pointer.button_down(PointerButton::Middle),
            ];
            (i.pointer.latest_pos(), i.pointer.delta(), i.smooth_scroll_delta.y, i.modifiers, pressed, down)
        });
        let local = pos.map(|p| Vec2::new(p.x - rect.min.x, p.y - rect.min.y));
        let hovered = resp.hovered();
        let d = Vec2::new(delta.x, delta.y);

        if hovered && scroll != 0.0 && self.drag != Drag::Sculpt {
            self.camera.zoom(scroll);
        }

        // A release + new press can land in the same frame (slow frames):
        // finish the previous drag so the new press starts a fresh stroke.
        if self.drag != Drag::None && pressed.iter().any(|p| *p) {
            if self.drag == Drag::Sculpt {
                self.sculptor.end_stroke();
                self.grab = None;
            }
            self.drag = Drag::None;
        }

        if self.drag == Drag::None && hovered {
            if let Some(lp) = local {
                if pressed[0] {
                    self.drag = if mods.alt {
                        Drag::Orbit
                    } else if self.begin_sculpt(lp, size, mods) {
                        Drag::Sculpt
                    } else if mods.shift {
                        Drag::Pan
                    } else {
                        Drag::Orbit
                    };
                } else if pressed[1] {
                    self.drag = if mods.shift || mods.command { Drag::Pan } else { Drag::Orbit };
                } else if pressed[2] {
                    self.drag = Drag::Pan;
                }
            }
        }

        match self.drag {
            Drag::Orbit => self.camera.orbit(d),
            Drag::Pan => self.camera.pan(d, size.y),
            Drag::Sculpt => {
                if let Some(lp) = local {
                    if down[0] {
                        self.continue_sculpt(lp, size);
                    }
                }
            }
            Drag::None => {}
        }

        if !down.iter().any(|b| *b) && self.drag != Drag::None {
            if self.drag == Drag::Sculpt {
                self.sculptor.end_stroke();
                self.grab = None;
            }
            self.drag = Drag::None;
        }
    }

    // --------------------------------------------------------------------- UI

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New sphere").clicked() {
                        self.new_mesh(Mesh::icosphere(self.sphere_level), "sphere");
                        ui.close_menu();
                    }
                    if ui.button("New cube").clicked() {
                        self.new_mesh(Mesh::cube(6), "cube");
                        ui.close_menu();
                    }
                    if ui.button("New plane").clicked() {
                        self.new_mesh(Mesh::plane(7), "plane");
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Import OBJ / STL…   Ctrl+O").clicked() {
                        ui.close_menu();
                        self.import_dialog();
                    }
                    if ui.button("Export OBJ…   Ctrl+S").clicked() {
                        ui.close_menu();
                        self.export_dialog("obj");
                    }
                    if ui.button("Export STL…").clicked() {
                        ui.close_menu();
                        self.export_dialog("stl");
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.add_enabled(self.sculptor.can_undo(), egui::Button::new("Undo   Ctrl+Z")).clicked() {
                        self.sculptor.undo();
                        ui.close_menu();
                    }
                    if ui.add_enabled(self.sculptor.can_redo(), egui::Button::new("Redo   Ctrl+Shift+Z")).clicked() {
                        self.sculptor.redo();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Subdivide   Ctrl+D").clicked() {
                        self.subdivide();
                        ui.close_menu();
                    }
                    if ui.button("Clear mask").clicked() {
                        self.sculptor.clear_mask();
                        ui.close_menu();
                    }
                    if ui.button("Invert mask").clicked() {
                        self.sculptor.invert_mask();
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.view.wireframe, "Wireframe   W");
                    ui.checkbox(&mut self.view.flat, "Flat shading");
                    ui.checkbox(&mut self.view.show_mask, "Show mask");
                    ui.separator();
                    ui.label("Settings panel");
                    ui.radio_value(&mut self.panel_right, false, "Left of viewport");
                    ui.radio_value(&mut self.panel_right, true, "Right of viewport");
                    ui.separator();
                    if ui.button("Frame mesh   Home").clicked() {
                        self.frame_mesh();
                        ui.close_menu();
                    }
                });
            });
        });
    }

    fn tool_panel(&mut self, ctx: &egui::Context) {
        let panel = if self.panel_right {
            egui::SidePanel::right("tools")
        } else {
            egui::SidePanel::left("tools")
        };
        panel.resizable(false).exact_width(236.0).show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(if self.panel_right { "tools_scroll_r" } else { "tools_scroll_l" })
                .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("CLAYFORM").strong().size(18.0).color(accent()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (icon, tip) = if self.panel_right {
                            ("◀", "Move settings to the left of the viewport")
                        } else {
                            ("▶", "Move settings to the right of the viewport")
                        };
                        if ui.small_button(icon).on_hover_text(tip).clicked() {
                            self.panel_right = !self.panel_right;
                        }
                    });
                });
                ui.add_space(4.0);
                section(ui, "Brush");
                egui::Grid::new("tools_grid").num_columns(3).spacing([4.0, 4.0]).show(ui, |ui| {
                    for (k, t) in Tool::ALL.iter().enumerate() {
                        let sel = self.tool == *t;
                        let b = egui::Button::new(egui::RichText::new(t.name()).size(13.0))
                            .selected(sel)
                            .min_size(egui::vec2(70.0, 30.0));
                        if ui.add(b).on_hover_text(format!("{}  (key {})", t.name(), t.shortcut())).clicked() {
                            self.tool = *t;
                        }
                        if k % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
                ui.add_space(8.0);
                let ti = self.tool.index();
                ui.add(egui::Slider::new(&mut self.radius_px, 4.0..=400.0).logarithmic(true).text("Radius"));
                ui.add(egui::Slider::new(&mut self.intensity[ti], 0.0..=1.0).text("Intensity"));
                ui.add_enabled(self.tool.has_negative(), egui::Checkbox::new(&mut self.negative, "Negative (N / hold Ctrl)"));
                ui.checkbox(&mut self.symmetry, "Symmetry X   (X)");

                section(ui, "Mesh");
                let tris = self.sculptor.tri_count();
                ui.horizontal(|ui| {
                    if ui.button("Sphere").clicked() {
                        self.new_mesh(Mesh::icosphere(self.sphere_level), "sphere");
                    }
                    if ui.button("Cube").clicked() {
                        self.new_mesh(Mesh::cube(6), "cube");
                    }
                    if ui.button("Plane").clicked() {
                        self.new_mesh(Mesh::plane(7), "plane");
                    }
                });
                ui.add(egui::Slider::new(&mut self.sphere_level, 3..=8).text("Sphere detail"));
                let next = fmt_count(tris * 4);
                if ui
                    .add_enabled(tris * 4 <= crate::sculpt::MAX_TRIANGLES, egui::Button::new(format!("Subdivide  to  {next} tris")))
                    .clicked()
                {
                    self.subdivide();
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.sculptor.can_undo(), egui::Button::new("⟲ Undo")).clicked() {
                        self.sculptor.undo();
                    }
                    if ui.add_enabled(self.sculptor.can_redo(), egui::Button::new("⟳ Redo")).clicked() {
                        self.sculptor.redo();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Clear mask").clicked() {
                        self.sculptor.clear_mask();
                    }
                    if ui.button("Invert mask").clicked() {
                        self.sculptor.invert_mask();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Import…").clicked() {
                        self.import_dialog();
                    }
                    if ui.button("Export OBJ").clicked() {
                        self.export_dialog("obj");
                    }
                    if ui.button("STL").clicked() {
                        self.export_dialog("stl");
                    }
                });

                section(ui, "Material");
                ui.horizontal_wrapped(|ui| {
                    for (k, name) in MATERIALS.iter().enumerate() {
                        ui.selectable_value(&mut self.view.material, k as u32, *name);
                    }
                });
                ui.checkbox(&mut self.view.wireframe, "Wireframe   (W)");
                ui.checkbox(&mut self.view.flat, "Flat shading");
                ui.checkbox(&mut self.view.show_mask, "Show mask");
                ui.horizontal(|ui| {
                    ui.label("Panel side");
                    ui.selectable_value(&mut self.panel_right, false, "Left");
                    ui.selectable_value(&mut self.panel_right, true, "Right");
                });

                section(ui, "Controls");
                let help = [
                    ("Left drag on mesh", "Sculpt"),
                    ("Shift + drag", "Smooth"),
                    ("Ctrl + drag", "Invert brush"),
                    ("Left drag off mesh", "Rotate"),
                    ("Alt + left / right drag", "Rotate"),
                    ("Middle / Shift + right", "Pan"),
                    ("Wheel", "Zoom"),
                    ("[  ]", "Brush radius"),
                    ("Home", "Frame mesh"),
                ];
                egui::Grid::new("help").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                    for (a, b) in help {
                        ui.label(egui::RichText::new(a).small().weak());
                        ui.label(egui::RichText::new(b).small());
                        ui.end_row();
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} verts · {} tris",
                    fmt_count(self.sculptor.mesh.vertex_count()),
                    fmt_count(self.sculptor.tri_count())
                ));
                ui.separator();
                ui.label(format!("{:.1} ms", self.frame_ms));
                ui.separator();
                let tool = if self.drag == Drag::Sculpt { self.stroke_tool } else { self.tool };
                ui.label(format!("{}{}", tool.name(), if self.symmetry { " · Sym X" } else { "" }));
                ui.separator();
                let age = self.status_at.elapsed().as_secs_f32();
                if age < 8.0 {
                    ui.label(egui::RichText::new(&self.status).weak());
                }
            });
        });
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(PANEL_SIDE_KEY, if self.panel_right { "right" } else { "left" }.to_string());
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let t0 = Instant::now();
        self.sculptor.refresh_bvh();
        self.shortcuts(ctx);
        self.menu_bar(ctx);
        self.tool_panel(ctx);
        self.status_bar(ctx);

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
            let rect = ui.max_rect();
            let resp = ui.allocate_rect(rect, Sense::click_and_drag());
            self.viewport_input(ctx, &resp, rect);

            let ppp = ctx.pixels_per_point();
            let size_px = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
            let aspect = rect.width() / rect.height().max(1.0);
            let mut tex = None;
            if let (Some(rs), Some(r)) = (frame.wgpu_render_state(), self.renderer.as_mut()) {
                r.sync_mesh(&self.sculptor.mesh, &mut self.sculptor.dirty, self.view.wireframe);
                let mut er = rs.renderer.write();
                r.render(size_px, &mut er, self.camera.view(), self.camera.proj(aspect), &self.view, true);
                tex = r.texture_id;
            }
            let painter = ui.painter_at(rect);
            if let Some(id) = tex {
                painter.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            } else {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, "wgpu unavailable", egui::FontId::proportional(20.0), Color32::RED);
            }

            // brush cursor
            if (resp.hovered() || self.drag == Drag::Sculpt) && matches!(self.drag, Drag::None | Drag::Sculpt) {
                if let Some(p) = ctx.input(|i| i.pointer.latest_pos()) {
                    let mods = ctx.input(|i| i.modifiers);
                    let tool = if self.drag == Drag::Sculpt {
                        self.stroke_tool
                    } else if mods.shift && self.tool != Tool::Mask {
                        Tool::Smooth
                    } else {
                        self.tool
                    };
                    let neg = (self.negative ^ mods.command) && tool.has_negative();
                    let col = if neg { Color32::from_rgb(110, 170, 255) } else { Color32::from_rgb(255, 150, 90) };
                    painter.circle_stroke(p, self.radius_px, Stroke::new(1.5_f32, col));
                    painter.circle_filled(p, 1.5, col);
                    if self.symmetry {
                        if let Some(m) = self.mirror_screen_point(p, rect) {
                            painter.circle_stroke(m, self.radius_px, Stroke::new(1.0_f32, col.gamma_multiply(0.45)));
                        }
                    }
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            }
        });

        if self.drag != Drag::None {
            ctx.request_repaint();
        }
        // Persist the panel side right away instead of waiting for eframe's
        // periodic autosave (which only runs while frames are being drawn).
        if self.panel_right != self.saved_panel_right {
            if let Some(storage) = frame.storage_mut() {
                self.save(storage);
                storage.flush();
            }
            self.saved_panel_right = self.panel_right;
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.frame_ms = self.frame_ms * 0.9 + ms * 0.1;
    }
}

impl App {
    /// Screen position of the mirrored brush (where the symmetric dab lands).
    fn mirror_screen_point(&self, p: Pos2, rect: Rect) -> Option<Pos2> {
        let size = Vec2::new(rect.width(), rect.height());
        let lp = Vec2::new(p.x - rect.min.x, p.y - rect.min.y);
        let (o, d) = self.camera.ray(lp, size);
        let h = self.sculptor.raycast(o, d)?;
        let hm = mirror_x(h.point);
        let clip = self.camera.proj(size.x / size.y) * self.camera.view() * hm.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Pos2::new(
            rect.min.x + (ndc.x * 0.5 + 0.5) * size.x,
            rect.min.y + (0.5 - ndc.y * 0.5) * size.y,
        ))
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title.to_uppercase()).small().strong().color(Color32::from_gray(150)));
    ui.separator();
}

fn accent() -> Color32 {
    Color32::from_rgb(255, 140, 80)
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.selection.bg_fill = Color32::from_rgb(196, 96, 50);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.hovered.weak_bg_fill = Color32::from_gray(62);
    v.panel_fill = Color32::from_rgb(26, 27, 31);
    v.window_fill = Color32::from_rgb(30, 31, 36);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
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
