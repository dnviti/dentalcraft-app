//! Standalone viewer for the DentalCraft viewport.
//!
//! ```sh
//! cargo run -p dc-viewport --example view_mesh -- [MESH_FILE] [--dense]
//! ```
//!
//! Opens `MESH_FILE` (STL/OBJ/PLY through `dc_mesh::io`) when given and
//! readable; otherwise shows a generated lower arch of tooth-like shapes
//! (labelled with FDI numbers) and a translucent upper arch. `--dense`
//! makes the generated arches about 300k triangles (performance check).
//! Mouse: right drag orbit, middle drag or Shift + right drag pan, wheel
//! zoom to cursor, left click pick, double click recentre / fit.
//!
//! With `DC_VIEWPORT_SCREENSHOT=out.ppm` the window saves a screenshot after
//! a few frames and closes (smoke test of the rendering path).

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use dc_mesh::TriMesh;
use dc_scene::{Color, LabelPalette, MeshColoring};
use dc_viewport::{BackfaceMode, Corner, Pick, Projection, StdView, ViewMesh, Viewport};
use glam::Vec3;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dense = args.iter().any(|a| a == "--dense");
    let path = args.iter().find(|a| !a.starts_with("--")).cloned();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("DentalCraft viewport")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "dc-viewport view_mesh",
        options,
        Box::new(move |cc| {
            let rs = cc
                .wgpu_render_state
                .as_ref()
                .ok_or("the wgpu renderer is not available")?;
            Ok(Box::new(App::new(rs, path.as_deref(), dense)))
        }),
    )
}

struct App {
    viewport: Viewport,
    meshes: Vec<ViewMesh>,
    source: String,
    triangles: usize,
    upper_opacity: f32,
    last_pick: Option<Pick>,
    frames: u32,
    screenshot: Option<String>,
    started: Option<std::time::Instant>,
}

impl App {
    fn new(rs: &egui_wgpu::RenderState, path: Option<&str>, dense: bool) -> Self {
        let mut source = String::from("generated arches");
        let mut meshes = Vec::new();
        if let Some(path) = path {
            match dc_mesh::io::read(path) {
                Ok(m) => {
                    source = path.to_owned();
                    meshes.push(ViewMesh::new(1, Arc::new(m)));
                }
                Err(e) => {
                    eprintln!("cannot read {path}: {e}; showing the generated arches");
                    source = format!("generated arches ({path}: {e})");
                }
            }
        }
        if meshes.is_empty() {
            let res = if dense { 60 } else { 40 };
            let lower = Arc::new(arch(res, false));
            let upper = Arc::new(arch(res, true));
            meshes.push(
                ViewMesh::new(1, lower)
                    .with_coloring(MeshColoring::VertexLabels(LabelPalette::default())),
            );
            meshes.push(
                ViewMesh::new(2, upper)
                    .with_coloring(MeshColoring::Uniform(Color::TOOTH))
                    .with_opacity(0.55),
            );
        }
        let triangles = meshes.iter().map(|m| m.mesh.face_count()).sum();
        let mut viewport = Viewport::new(rs);
        viewport.settings.hover_pick = true;
        viewport.set_meshes(meshes.clone());
        viewport.set_view(StdView::Front);
        viewport.fit();
        Self {
            viewport,
            meshes,
            source,
            triangles,
            upper_opacity: 0.55,
            last_pick: None,
            frames: 0,
            screenshot: std::env::var("DC_VIEWPORT_SCREENSHOT").ok(),
            started: None,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("controls")
            .resizable(false)
            .show(ui, |ui| self.controls(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let r = self.viewport.ui(ui);
                if let Some(p) = r.pick {
                    self.last_pick = Some(p);
                }
                self.smoke_test(ui.ctx());
                if let Some(h) = r.hovered {
                    r.response.on_hover_text_at_pointer(format!(
                        "mesh {} tri {}\n{:.2} {:.2} {:.2}",
                        h.mesh_id, h.tri, h.point.x, h.point.y, h.point.z
                    ));
                }
            });
    }
}

impl App {
    fn smoke_test(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.screenshot else {
            return;
        };
        self.frames += 1;
        ctx.request_repaint();
        if self.frames > 10 {
            // keep the camera moving so that every frame renders the 3D pass
            self.viewport.camera_mut().orbit(0.02, 0.0);
        }
        if self.frames == 10 {
            self.started = Some(std::time::Instant::now());
            let path = path.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |img| {
                    let mut out = format!("P6 {} {} 255\n", img.width(), img.height()).into_bytes();
                    for p in &img.pixels {
                        out.extend_from_slice(&[p.r(), p.g(), p.b()]);
                    }
                    if let Err(e) = std::fs::write(&path, out) {
                        eprintln!("cannot write {path}: {e}");
                    }
                }),
            ));
        } else if self.frames == 130 {
            if let Some(t) = self.started {
                let ms = t.elapsed().as_secs_f64() * 1000.0 / 120.0;
                println!(
                    "{} triangles: {ms:.2} ms per orbiting frame",
                    self.triangles
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Viewport");
        ui.label(&self.source);
        ui.label(format!(
            "{} triangles, MSAA x{}",
            self.triangles,
            self.viewport.sample_count()
        ));
        let dt = ui.input(|i| i.unstable_dt);
        ui.label(format!("frame {:.1} ms", dt * 1000.0));
        ui.separator();
        ui.label("Views");
        egui::Grid::new("views").num_columns(2).show(ui, |ui| {
            for (i, v) in StdView::ALL.into_iter().enumerate() {
                if ui.button(v.name()).clicked() {
                    self.viewport.set_view(v);
                    self.viewport.fit();
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        if ui.button("Fit").clicked() {
            self.viewport.fit();
        }
        let mut ortho = self.viewport.camera().projection == Projection::Orthographic;
        if ui.checkbox(&mut ortho, "Orthographic").changed() {
            self.viewport.toggle_projection();
        }
        ui.separator();
        let s = &mut self.viewport.settings;
        ui.checkbox(&mut s.wireframe, "Wireframe");
        ui.checkbox(&mut s.hover_pick, "Pick on hover");
        let mut gizmo = s.gizmo.is_some();
        if ui.checkbox(&mut gizmo, "Axis gizmo").changed() {
            s.gizmo = gizmo.then(Corner::default);
        }
        egui::ComboBox::from_label("Back faces")
            .selected_text(format!("{:?}", s.backface))
            .show_ui(ui, |ui| {
                for m in [BackfaceMode::Show, BackfaceMode::Tint, BackfaceMode::Cull] {
                    ui.selectable_value(&mut s.backface, m, format!("{m:?}"));
                }
            });
        if self.meshes.len() > 1 {
            let mut changed = ui
                .add(egui::Slider::new(&mut self.upper_opacity, 0.0..=1.0).text("upper opacity"))
                .changed();
            let mut visible = self.meshes[1].visible;
            changed |= ui.checkbox(&mut visible, "upper arch").changed();
            if changed {
                self.meshes[1].opacity = self.upper_opacity;
                self.meshes[1].visible = visible;
                // same Arc<TriMesh>: only uniforms change, nothing is re-uploaded
                self.viewport.set_meshes(self.meshes.clone());
            }
        }
        ui.separator();
        match &self.last_pick {
            Some(p) => {
                ui.label(format!("picked mesh {} tri {}", p.mesh_id, p.tri));
                ui.label(format!(
                    "{:.2}, {:.2}, {:.2} mm",
                    p.point.x, p.point.y, p.point.z
                ));
            }
            None => {
                ui.label("click a mesh to pick");
            }
        }
    }
}

/// Point and unit tangent of the arch curve at `s` in `[-1, 1]`
/// (incisors at `s = 0` towards −Y, molars at the ends).
fn arch_curve(s: f32) -> (Vec3, Vec3) {
    let p = Vec3::new(25.0 * s, -24.0 + 34.0 * s * s, 0.0);
    let t = Vec3::new(25.0, 68.0 * s, 0.0).normalize();
    (p, t)
}

/// A generated arch: 14 tooth-like crowns on a gingiva band, per-vertex FDI
/// labels (0 = gingiva). `upper` mirrors it above the occlusal plane.
fn arch(res: u32, upper: bool) -> TriMesh {
    let mut pos: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut labels: Vec<u16> = Vec::new();
    // mesiodistal widths, central incisor to second molar (mm)
    const WIDTHS: [f32; 7] = [5.5, 6.0, 7.0, 7.0, 7.0, 10.5, 10.0];
    let total: f32 = WIDTHS.iter().sum();
    for side in [-1.0f32, 1.0] {
        let mut start = 0.0f32;
        for (i, w) in WIDTHS.iter().enumerate() {
            // centre of the tooth as a fraction of the half arch
            let s = (start + w * 0.5) / total;
            start += w;
            let (c, t) = arch_curve(side * s);
            // quadrant: patient's right is −X
            let quadrant = match (upper, side < 0.0) {
                (false, false) => 3,
                (false, true) => 4,
                (true, true) => 1,
                (true, false) => 2,
            };
            let fdi = quadrant * 10 + i as u16 + 1;
            let molar = i >= 5;
            tooth(
                &mut pos,
                &mut tris,
                &mut labels,
                fdi,
                c + Vec3::Z * 1.5,
                t,
                Vec3::new(
                    *w * 0.5,
                    if molar { 5.2 } else { 3.6 },
                    if i < 3 { 4.5 } else { 3.6 },
                ),
                res,
            );
        }
    }
    gingiva(&mut pos, &mut tris, &mut labels, res);
    if upper {
        for p in &mut pos {
            p.z = 9.0 - p.z;
        }
        for t in &mut tris {
            t.swap(1, 2);
        }
    }
    let mut m = TriMesh::new(pos, tris);
    m.vertex_labels = Some(labels);
    m
}

#[allow(clippy::too_many_arguments)]
fn tooth(
    pos: &mut Vec<Vec3>,
    tris: &mut Vec<[u32; 3]>,
    labels: &mut Vec<u16>,
    fdi: u16,
    center: Vec3,
    tangent: Vec3,
    radii: Vec3,
    res: u32,
) {
    let normal = Vec3::Z.cross(tangent);
    let base = pos.len() as u32;
    let (stacks, slices) = (res, res);
    for i in 0..=stacks {
        let phi = PI * i as f32 / stacks as f32; // 0 = top
        for j in 0..=slices {
            let theta = TAU * j as f32 / slices as f32;
            let (sp, cp) = phi.sin_cos();
            let (st, ct) = theta.sin_cos();
            let mut z = cp * radii.z;
            if cp > 0.0 {
                // flattened occlusal surface with cusps
                z = z * 0.75 + 0.35 * (2.0 * theta).cos().abs() * sp * radii.z * 0.3;
            }
            let p =
                center + tangent * (ct * sp * radii.x) + normal * (st * sp * radii.y) + Vec3::Z * z;
            pos.push(p);
            labels.push(fdi);
        }
    }
    let row = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let a = base + i * row + j;
            let b = a + row;
            tris.push([a, b, a + 1]);
            tris.push([a + 1, b, b + 1]);
        }
    }
}

fn gingiva(pos: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, labels: &mut Vec<u16>, res: u32) {
    let base = pos.len() as u32;
    let along = res * 6;
    let around = res.max(8);
    for i in 0..=along {
        let s = -1.08 + 2.16 * i as f32 / along as f32;
        let (c, t) = arch_curve(s);
        let n = Vec3::Z.cross(t);
        for j in 0..=around {
            // half tube below the occlusal plane, open at the bottom
            let a = PI * j as f32 / around as f32;
            let p = c + n * (a.cos() * 7.0) - Vec3::Z * (a.sin() * 6.0) + Vec3::Z * 0.5;
            pos.push(p);
            labels.push(0);
        }
    }
    let row = around + 1;
    for i in 0..along {
        for j in 0..around {
            let a = base + i * row + j;
            let b = a + row;
            tris.push([a, a + 1, b]);
            tris.push([a + 1, b + 1, b]);
        }
    }
}
