//! The scan popup, laid out like the scanning apps of the scanner makers:
//! the scanner and its status along the top, the 3D view in the middle
//! (the live scan, then the points being cleaned, then the surface) and,
//! on the right, the turntable (alpha, [`turntable`]) and the steps in the
//! order of the work:
//!
//! 1. Acquisition: start, pause / resume, stop, start over, the automatic
//!    scan with the turntable (with the quality of its mesh: resolution,
//!    refinement, finish; its result is the finished mesh, shown framed,
//!    with a report), and the settings of the acquisition;
//! 2. Point cloud: stray points, largest piece, smoothing, thinning, a
//!    rectangle selection to delete, a cut below a level;
//! 3. Surface: its resolution (Fine 0.15, Standard 0.3, Fast 0.6 mm or a
//!    custom value) and the button that builds it, and the rebuild from the
//!    stops of an automatic scan at a finer resolution;
//! 4. Surface tools: finish (clean, close the holes, optimise), holes,
//!    smoothing, loose pieces, simplification;
//! 5. Import: one of the roles of the [`crate::ScanTarget`], and the
//!    hand-over of the surface to the host (`scan.import`).
//!
//! Each control runs a `scan.*` or `turntable.*` command
//! ([`ScanPopup::command`]); the drawing only reads the controller.

mod framing;
mod turntable;
mod vendor;

use std::sync::Arc;
use std::time::Duration;

use dc_case::ScanRole;
use dc_mesh::TriMesh;
use dc_scanner::control::{
    SCAN_AUTO, SCAN_CLOUD, SCAN_CONNECT, SCAN_DEVICES, SCAN_IMPORT, SCAN_MESH, SCAN_MESH_BUILD,
    SCAN_PAUSE, SCAN_RECONSTRUCT, SCAN_REDO, SCAN_RESET, SCAN_RESUME, SCAN_SETTINGS, SCAN_START,
    SCAN_STOP, SCAN_UNDO, TURNTABLE_CONNECT, TURNTABLE_DEVICES,
};
use dc_scanner::{
    CloudOp, DeviceInfo, LivePreview, MeshOp, PointCloud, ScanSettings, ScanState, ScanStatus,
};
use dc_scene::MeshColoring;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::panels::{Side, SidePanel};
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_viewport::camera::look_rotation;
use dc_viewport::{ViewMesh, ViewPoints, Viewport};
use egui::{
    Align, Align2, Color32, Id, Key, Layout, Margin, Modal, Modifiers, PointerButton, Pos2, Rect,
    RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, vec2,
};
use framing::Follow;
use glam::Vec3;
use serde_json::{Value, json};
pub(crate) use turntable::OPEN_SEARCH_S;
use turntable::{
    CalSnap, FinishSnap, JobSnap, ReconSnap, TableSnap, TableUi, finish_report, recon_report,
    sub_heading,
};

use crate::{
    ImportWords, ScanFailure, ScanPopup, ScanTarget, caption, scene_color, select, t, tr_n,
};

/// Resolutions of the surface (mm): Fine, Standard, Fast; the fourth choice
/// is the custom value.
const RESOLUTIONS: [f32; 3] = [0.15, 0.3, 0.6];
/// The width of the steps panel the user chose (every scan window shares
/// it), and whether they resized it since the host last saved it.
static STEPS_WIDTH: std::sync::Mutex<(Option<f32>, bool)> = std::sync::Mutex::new((None, false));

fn steps_width() -> f32 {
    let saved = STEPS_WIDTH.lock().map_or(None, |s| s.0);
    saved.unwrap_or(dc_ui::tokens::tokens().size.scan_steps_width)
}

fn set_steps_width(width: f32, resized: bool) {
    if let Ok(mut s) = STEPS_WIDTH.lock() {
        s.0 = Some(width);
        s.1 |= resized;
    }
}

/// Sets the width of the steps panel of the scan windows (the saved one).
pub fn restore_steps_width(width: f32) {
    let sz = dc_ui::tokens::tokens().size;
    if width.is_finite()
        && let Ok(mut s) = STEPS_WIDTH.lock()
    {
        s.0 = Some(width.clamp(sz.scan_steps_min_width, sz.scan_steps_max_width));
    }
}

/// The width of the steps panel once the user finished resizing it (a drag
/// of its edge, or a double-click that restores the default): the host
/// saves it.
pub fn take_resized_steps_width() -> Option<f32> {
    let mut s = STEPS_WIDTH.lock().ok()?;
    std::mem::take(&mut s.1).then(|| s.0).flatten()
}
/// Frames per second of the live view while scanning.
const LIVE_FRAME: Duration = Duration::from_millis(66);
/// Height of the 3D view's starting viewpoint above the platter, as an
/// angle from the scanner's side (degrees): about where the scanner of a
/// turntable rig looks from.
const VIEW_ELEVATION_DEG: f32 = 25.0;

/// Ids of the clouds of the 3D view.
const CLOUD_ID: u64 = 1;
const FRAME_ID: u64 = 2;
const SELECTION_ID: u64 = 3;

/// What the controller holds, read when it is free.
#[derive(Clone)]
struct Snapshot {
    status: ScanStatus,
    settings: ScanSettings,
    preview: Option<LivePreview>,
    cloud: Option<Arc<PointCloud>>,
    mesh: Option<Arc<TriMesh>>,
    can_undo: bool,
    can_redo: bool,
    table: TableSnap,
    /// The latest automatic scan and calibration of the rig.
    auto: Option<JobSnap>,
    calibrating: Option<JobSnap>,
    /// The stored calibration of the rig.
    calibration: Option<CalSnap>,
}

/// A value of the controller as JSON (what an agent reads), `null` when it
/// does not serialise.
macro_rules! as_json {
    ($v:expr) => {
        serde_json::to_value($v).unwrap_or(Value::Null)
    };
}

impl Snapshot {
    fn read(c: &dc_scanner::ScanController) -> Self {
        let s = c.session();
        let e = c.editor();
        Self {
            status: s.status(),
            settings: s.settings().clone(),
            preview: s.preview(),
            cloud: e.map(|e| Arc::clone(e.cloud())),
            mesh: e.and_then(|e| e.mesh().cloned()),
            can_undo: e.is_some_and(dc_scanner::ScanEditor::can_undo),
            can_redo: e.is_some_and(dc_scanner::ScanEditor::can_redo),
            table: TableSnap::from_json(&as_json!(c.turntable_status())),
            auto: c
                .auto_status()
                .and_then(|j| JobSnap::from_json(&as_json!(j))),
            calibrating: c
                .calibration_job()
                .and_then(|j| JobSnap::from_json(&as_json!(j))),
            calibration: c
                .calibration()
                .and_then(|cal| CalSnap::from_json(&as_json!(cal))),
        }
    }

    /// Frames are coming in (scanning or paused).
    fn live(&self) -> bool {
        matches!(self.status.state, ScanState::Scanning | ScanState::Paused)
    }

    /// Something moves on its own: the scan, the turntable, a job of the
    /// rig (the popup repaints at the pace of the live view).
    fn moving(&self) -> bool {
        self.live()
            || self.table.busy
            || JobSnap::running(self.auto.as_ref()).is_some()
            || JobSnap::running(self.calibrating.as_ref()).is_some()
    }

    /// The error of the latest job of the rig, when it failed.
    fn job_error(&self) -> Option<&ScanFailure> {
        self.auto
            .as_ref()
            .and_then(|j| j.error.as_ref())
            .or_else(|| self.calibrating.as_ref().and_then(|j| j.error.as_ref()))
    }

    /// A job of the rig is reconnecting the turntable whose link dropped.
    fn reconnecting(&self) -> bool {
        [self.auto.as_ref(), self.calibrating.as_ref()]
            .into_iter()
            .any(|j| JobSnap::running(j).is_some_and(|j| j.reconnecting))
    }

    /// The automatic scan is building the surface from its stops: the
    /// acquisition is stopped (or about to be), nothing is tracked or
    /// turning, only the phase of the job shows.
    fn finishing(&self) -> bool {
        JobSnap::running(self.auto.as_ref())
            .is_some_and(|j| j.finishing() || self.status.state == ScanState::Stopped)
    }
}

/// What the 3D view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Content {
    Nothing,
    /// The scan being acquired and the latest frame.
    Live,
    /// The points of the stopped scan.
    Points,
    /// The surface built from them.
    Surface,
}

/// What was handed to the viewport last (to hand it again only on change).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShownKey {
    content: Content,
    generation: u64,
    cloud: usize,
    mesh: usize,
    selection: usize,
    wireframe: bool,
}

/// Points of a cloud as the viewport takes them, kept while the cloud is
/// the same.
#[derive(Clone)]
struct Uploaded {
    of: usize,
    points: Arc<Vec<Vec3>>,
    normals: Option<Arc<Vec<Vec3>>>,
}

impl Uploaded {
    fn of(cloud: &Arc<PointCloud>) -> Self {
        Self {
            of: addr(cloud),
            points: Arc::new(cloud.points.clone()),
            normals: cloud.normals.clone().map(Arc::new),
        }
    }
}

/// The points chosen with the rectangle.
struct Selection {
    /// The cloud they index.
    cloud: usize,
    indices: Vec<u32>,
    points: Arc<Vec<Vec3>>,
}

fn addr<T>(a: &Arc<T>) -> usize {
    Arc::as_ptr(a) as *const () as usize
}

/// What the live view frames: the cylinder of the region of interest when
/// the scan world is the calibrated rig frame (+Z the turntable axis), else
/// the robust bounds of the fused cloud (the points of the latest frame
/// only when there is no cloud yet).
fn live_box(snap: &Snapshot) -> Option<(Vec3, Vec3)> {
    let preview = snap.preview.as_ref()?;
    if snap.settings.use_roi && snap.status.rig_world {
        return Some(turntable::roi_box(
            snap.settings.roi_radius_mm,
            snap.settings.roi_height_mm,
        ));
    }
    framing::robust_box(&preview.cloud.points).or_else(|| framing::bounds(&preview.frame_points))
}

/// A control of the popup that was used this frame.
enum Act {
    Run(&'static str, Value),
    Close,
    Cancel,
}

/// State of the scan popup.
pub(crate) struct ScanUi {
    pub open: bool,
    confirm_close: bool,
    render_state: Option<egui_wgpu::RenderState>,
    viewport: Option<Viewport>,
    snap: Option<Snapshot>,
    devices: Vec<DeviceInfo>,
    /// Scanner kinds that cannot be used here, with the reason code.
    unavailable: Vec<String>,
    device: usize,
    error: Option<ScanFailure>,
    show_surface: bool,
    wireframe: bool,
    /// The acquisition settings being edited (from the controller).
    settings: Option<ScanSettings>,
    exposure_auto: bool,
    exposure_us: f32,
    std_ratio: f32,
    isolate_mm: f32,
    smooth_strength: f32,
    spacing_mm: f32,
    selecting: bool,
    selection: Option<Selection>,
    drag: Option<Pos2>,
    cutting: bool,
    cut_mm: f32,
    resolution: usize,
    custom_mm: f32,
    hole_edges: f32,
    passes: f32,
    island_pct: f32,
    keep_pct: f32,
    /// The role chosen in the step "Import" (index in the target's roles).
    role: usize,
    /// What the popup is opened for (kept when it closes).
    target: ScanTarget,
    /// The turntable block and the automatic scan.
    table: TableUi,
    /// The surface handed over to the host last.
    imported: Option<Arc<TriMesh>>,
    shown: Option<ShownKey>,
    live: Option<(u64, Uploaded, Arc<Vec<Vec3>>)>,
    edited: Option<Uploaded>,
    /// Frame the view on the next content.
    fit: bool,
    /// Whether the live view follows the growing scan.
    follow: Follow,
    /// A calibration was running when the view was last drawn.
    calibrating_before: bool,
    /// An automatic scan was seen running: its result is shown (and the
    /// view framed) once it ends.
    auto_pending: bool,
    /// What the latest manual mesh finish and rebuild reported.
    manual_finish: Option<FinishSnap>,
    manual_recon: Option<ReconSnap>,
}

impl ScanUi {
    pub fn new(render_state: Option<egui_wgpu::RenderState>) -> Self {
        Self {
            open: false,
            confirm_close: false,
            render_state,
            viewport: None,
            snap: None,
            devices: Vec::new(),
            unavailable: Vec::new(),
            device: 0,
            error: None,
            show_surface: false,
            wireframe: false,
            settings: None,
            exposure_auto: true,
            exposure_us: 8000.0,
            std_ratio: 2.0,
            isolate_mm: 1.0,
            smooth_strength: 0.5,
            spacing_mm: 0.2,
            selecting: false,
            selection: None,
            drag: None,
            cutting: false,
            cut_mm: 0.0,
            resolution: 1,
            custom_mm: 0.3,
            hole_edges: 200.0,
            passes: 5.0,
            island_pct: 10.0,
            keep_pct: 50.0,
            role: 0,
            target: ScanTarget::default(),
            table: TableUi::default(),
            imported: None,
            shown: None,
            live: None,
            edited: None,
            fit: true,
            follow: Follow::default(),
            calibrating_before: false,
            auto_pending: false,
            manual_finish: None,
            manual_recon: None,
        }
    }

    /// The popup closes: what belongs to this scan goes (not the target).
    pub fn close(&mut self) {
        let mut keep = Self::new(self.render_state.take());
        keep.target = std::mem::take(&mut self.target);
        let viewport = self.viewport.take();
        *self = keep;
        if let Some(mut vp) = viewport {
            vp.set_points(Vec::new());
            vp.set_meshes(Vec::new());
            self.viewport = Some(vp);
        }
    }

    /// What the popup is for; the chosen role starts over when the roles
    /// change.
    pub fn set_target(&mut self, target: ScanTarget) {
        if target.offered() != self.target.offered() {
            self.role = 0;
        }
        self.target = target;
    }

    /// A word of the step "Import": the host's, or the popup's own `key`.
    fn word(&self, pick: fn(&ImportWords) -> &String, key: &str) -> String {
        self.target
            .words
            .as_ref()
            .map_or_else(|| t(key), |w| pick(w).clone())
    }

    /// The surface the view holds (what `scan.import` writes).
    pub fn shown_mesh(&self) -> Option<Arc<TriMesh>> {
        self.snap.as_ref().and_then(|s| s.mesh.clone())
    }

    pub fn failed(&mut self, f: ScanFailure) {
        self.error = Some(f);
    }

    pub fn imported(&mut self, mesh: Option<Arc<TriMesh>>) {
        self.error = None;
        self.imported = mesh;
    }

    /// The popup follows the command `id` that ended.
    pub fn after(&mut self, id: &str, r: &Result<Value, ScanFailure>) {
        let out = match r {
            Ok(out) => out,
            Err(f) if f.code == "cancelled" => {
                self.error = None;
                return;
            }
            Err(f) => {
                self.error = Some(f.clone());
                return;
            }
        };
        self.error = None;
        match id {
            SCAN_DEVICES => {
                self.devices = out["devices"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|d| serde_json::from_value(d.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                self.unavailable = out["backends"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["available"] == json!(false))
                    .map(|b| {
                        let id = b["id"].as_str().unwrap_or_default();
                        match b["error_code"].as_str() {
                            Some(code) => format!("{id}: {code}"),
                            None => id.to_owned(),
                        }
                    })
                    .collect();
                // a hardware scanner first, the simulated ones after
                if let Some(i) = self.devices.iter().position(|d| d.backend != "sim") {
                    self.device = i;
                }
                self.device = self.device.min(self.devices.len().saturating_sub(1));
            }
            SCAN_CONNECT => {
                // the scanner an agent connected may not be listed yet
                if let Ok(d) = serde_json::from_value::<DeviceInfo>(out["device"].clone()) {
                    self.device = match self.devices.iter().position(|x| x.id == d.id) {
                        Some(i) => i,
                        None => {
                            self.devices.push(d);
                            self.devices.len() - 1
                        }
                    };
                }
                self.settings = None;
            }
            TURNTABLE_DEVICES => self.table.listed(out),
            TURNTABLE_CONNECT => self.table.connected(out),
            SCAN_START | SCAN_AUTO => {
                self.manual_finish = None;
                self.manual_recon = None;
                self.refit();
            }
            SCAN_STOP => {
                self.show_surface = false;
                self.selection = None;
            }
            SCAN_RESET => {
                self.selection = None;
                self.imported = None;
                self.manual_finish = None;
                self.manual_recon = None;
                self.refit();
            }
            SCAN_SETTINGS => {
                if let Ok(s) = serde_json::from_value::<ScanSettings>(out.clone()) {
                    self.settings = Some(s);
                }
            }
            SCAN_CLOUD => {
                self.show_surface = false;
                self.selection = None;
            }
            // from the stops (a fine reconstruction) or from the cloud
            SCAN_MESH_BUILD => {
                self.manual_recon = out.get("reconstruction").and_then(ReconSnap::from_json);
                self.manual_finish = None;
                self.show_surface = true;
            }
            // a rebuild from the stops replaces the surface
            SCAN_RECONSTRUCT => {
                self.manual_recon = out.get("reconstruction").and_then(ReconSnap::from_json);
                self.manual_finish = None;
                self.show_surface = true;
                self.refit();
            }
            // the report of a finish; any other mesh tool leaves none
            SCAN_MESH => {
                self.manual_finish = out.get("finish").and_then(FinishSnap::from_json);
            }
            SCAN_UNDO | SCAN_REDO => self.selection = None,
            _ => {}
        }
    }

    /// `true` when closing would lose a scan that is not in the case.
    fn unimported(&self) -> bool {
        let Some(s) = &self.snap else { return false };
        if s.live() && s.status.points > 0 {
            return true;
        }
        match (&s.cloud, &s.mesh, &self.imported) {
            (None, _, _) => false,
            (Some(_), Some(m), Some(i)) => !Arc::ptr_eq(m, i),
            (Some(c), _, _) => !c.is_empty(),
        }
    }

    fn state(&self) -> ScanState {
        self.snap
            .as_ref()
            .map_or(ScanState::Idle, |s| s.status.state)
    }

    fn content(&self) -> Content {
        let Some(s) = &self.snap else {
            return Content::Nothing;
        };
        if s.live() || s.cloud.is_none() {
            return match &s.preview {
                Some(p) if !p.cloud.is_empty() || !p.frame_points.is_empty() => Content::Live,
                _ => Content::Nothing,
            };
        }
        if self.show_surface && s.mesh.is_some() {
            Content::Surface
        } else {
            Content::Points
        }
    }

    /// Hands what the view shows to the viewport, when it changed.
    fn sync_view(&mut self, ui: &Ui) {
        let Some(snap) = self.snap.clone() else {
            return;
        };
        // a calibration that ended: the rig and the view start afresh
        let calibrating = JobSnap::running(snap.calibrating.as_ref()).is_some();
        if std::mem::replace(&mut self.calibrating_before, calibrating) && !calibrating {
            self.refit();
        }
        // an automatic scan that ended shows its result, framed: the
        // finished mesh when it made one, else the points
        if JobSnap::running(snap.auto.as_ref()).is_some() {
            self.auto_pending = true;
        } else if self.auto_pending && !snap.live() && snap.cloud.is_some() {
            self.auto_pending = false;
            self.show_surface =
                snap.mesh.is_some() && snap.auto.as_ref().is_some_and(|j| j.finish.is_some());
            self.refit();
        }
        let content = self.content();
        let c = *colors(ui);
        let key = ShownKey {
            content,
            generation: snap.preview.as_ref().map_or(0, |p| p.generation),
            cloud: snap.cloud.as_ref().map_or(0, addr),
            mesh: snap.mesh.as_ref().map_or(0, addr),
            selection: self.selection.as_ref().map_or(0, |s| addr(&s.points)),
            wireframe: self.wireframe,
        };
        if self.shown == Some(key) && !self.fit {
            return;
        }
        let mut points = Vec::new();
        let mut meshes = Vec::new();
        match content {
            Content::Nothing => {}
            Content::Live => {
                if let Some(p) = &snap.preview {
                    let stale = self
                        .live
                        .as_ref()
                        .is_none_or(|(g, _, _)| *g != p.generation);
                    if stale {
                        self.live = Some((
                            p.generation,
                            Uploaded::of(&p.cloud),
                            Arc::clone(&p.frame_points),
                        ));
                    }
                }
                if let Some((_, cloud, frame)) = &self.live {
                    points.push(cloud_points(cloud, linear(c.model_enamel), 2.0));
                    points.push(
                        ViewPoints::new(FRAME_ID, Arc::clone(frame))
                            .with_color(linear(c.accent_amber))
                            .with_size_px(2.5),
                    );
                }
            }
            Content::Points => {
                if let Some(cloud) = &snap.cloud {
                    if self.edited.as_ref().is_none_or(|u| u.of != addr(cloud)) {
                        self.edited = Some(Uploaded::of(cloud));
                    }
                    if let Some(u) = &self.edited {
                        points.push(cloud_points(u, linear(c.model_enamel), 2.0));
                    }
                    if let Some(sel) = self.selection.as_ref().filter(|s| s.cloud == addr(cloud)) {
                        points.push(
                            ViewPoints::new(SELECTION_ID, Arc::clone(&sel.points))
                                .with_color(linear(c.select_blue))
                                .with_size_px(3.0),
                        );
                    }
                }
            }
            Content::Surface => {
                if let Some(m) = &snap.mesh {
                    meshes.push(
                        ViewMesh::new(1, Arc::clone(m))
                            .with_coloring(MeshColoring::Uniform(scene_color(c.model_enamel))),
                    );
                }
            }
        }
        let Some(vp) = self.viewport.as_mut() else {
            return;
        };
        vp.settings.wireframe = self.wireframe && content == Content::Surface;
        vp.set_points(points);
        vp.set_meshes(meshes);
        if content != Content::Nothing {
            let live = content == Content::Live;
            let points = snap.preview.as_ref().map_or(0, |p| p.cloud.len());
            if self.fit || (live && self.follow.due(points)) {
                if !live {
                    vp.fit();
                    self.fit = false;
                } else if let Some((lo, hi)) = live_box(&snap) {
                    vp.fit_box(lo, hi);
                    self.follow.framed(points);
                    self.fit = false;
                }
            }
        }
        self.shown = Some(key);
    }

    /// The view is framed again from the next content on, and follows the
    /// scan again if the user had moved the camera.
    fn refit(&mut self) {
        self.fit = true;
        self.follow.restart();
    }

    // ------------------------------------------------------------ drawing

    fn contents(&mut self, ui: &mut Ui, busy: bool, out: &mut Vec<Act>) {
        self.top_bar(ui, busy, out);
        ui.add_space(8.0);
        self.status_row(ui);
        if let Some(v) = self.snap.as_ref().and_then(|s| s.status.vendor.as_ref()) {
            ui.add_space(4.0);
            vendor::row(ui, v);
        }
        ui.add_space(10.0);
        // the steps at the right, as wide as the user dragged their edge
        // (double-click: back to the default); the 3D view keeps the rest
        let body = ui.available_rect_before_wrap();
        ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
            let sz = dc_ui::tokens::tokens().size;
            let mut width = steps_width();
            let r = SidePanel::new(
                Side::Right,
                "dc-scan-steps",
                sz.scan_steps_width,
                sz.scan_steps_min_width..=sz.scan_steps_max_width,
            )
            .reserve(sz.viewport_min_width)
            .frame(egui::Frame::new().inner_margin(Margin {
                left: 12,
                ..Margin::ZERO
            }))
            .show(ui, &mut width, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("dc-scan-steps-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.steps(ui, busy, out));
            });
            set_steps_width(width, r.resized);
            let view = ui.available_rect_before_wrap();
            ui.scope_builder(UiBuilder::new().max_rect(view), |ui| {
                self.view(ui, view);
            });
        });
        ui.allocate_rect(body, Sense::hover());
    }

    fn top_bar(&mut self, ui: &mut Ui, busy: bool, out: &mut Vec<Act>) {
        let c = *colors(ui);
        let state = self.state();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(
                RichText::new(t("scan-title"))
                    .font(fonts::semibold(18.0))
                    .color(c.text_primary),
            );
            widgets::badge_icon(ui, BadgeKind::Amber, None, t("scan-alpha"));
            // what the host opened it for
            if let Some(context) = &self.target.context {
                ui.label(
                    RichText::new(context)
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(t("scan-device"))
                    .font(fonts::medium(12.0))
                    .color(c.text_secondary),
            );
            let connected = state != ScanState::Idle;
            ui.add_enabled_ui(!connected && !busy, |ui| {
                let connected_name = self
                    .snap
                    .as_ref()
                    .and_then(|s| s.status.device.as_ref())
                    .map(|d| d.name.clone());
                let shown = connected_name.unwrap_or_else(|| {
                    self.devices
                        .get(self.device)
                        .map_or_else(|| t("scan-no-device"), |d| d.name.clone())
                });
                egui::ComboBox::from_id_salt("dc-scan-device")
                    .width(240.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (i, d) in self.devices.iter().enumerate() {
                            ui.selectable_value(&mut self.device, i, &d.name);
                        }
                    });
            });
            let refresh = widgets::icon_button(ui, ic::ARROWS_CLOCKWISE, t("scan-refresh"));
            // the kinds of scanner that cannot be used here, and why
            let refresh = if self.unavailable.is_empty() {
                refresh
            } else {
                refresh.on_hover_text(self.unavailable.join("\n"))
            };
            if refresh.clicked() && !busy {
                out.push(Act::Run(SCAN_DEVICES, Value::Null));
            }
            if connected {
                if Button::secondary(t("scan-disconnect"))
                    .icon(ic::PLUG)
                    .enabled(!busy)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(dc_scanner::control::SCAN_DISCONNECT, Value::Null));
                }
            } else {
                let params = self.devices.get(self.device).map_or(
                    Value::Null,
                    |d| json!({ "backend": d.backend, "device": d.id }),
                );
                let primary = self.snap.as_ref().is_none_or(|s| s.cloud.is_none());
                let b = if primary {
                    Button::primary(t("scan-connect"))
                } else {
                    Button::secondary(t("scan-connect"))
                };
                if b.icon(ic::PLUGS_CONNECTED)
                    .enabled(!busy)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(SCAN_CONNECT, params));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, ic::X, t("scan-close")).clicked() {
                    out.push(Act::Close);
                }
            });
        });
    }

    fn status_row(&self, ui: &mut Ui) {
        let c = *colors(ui);
        let state = self.state();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (kind, key) = match state {
                ScanState::Idle => (BadgeKind::Neutral, "scan-state-idle"),
                ScanState::Connected => (BadgeKind::Info, "scan-state-connected"),
                ScanState::Scanning => (BadgeKind::Success, "scan-state-scanning"),
                ScanState::Paused => (BadgeKind::Warning, "scan-state-paused"),
                ScanState::Stopped => (BadgeKind::Neutral, "scan-state-stopped"),
            };
            widgets::badge(ui, kind, t(key));
            let text = |ui: &mut Ui, s: String| {
                ui.label(
                    RichText::new(s)
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
            };
            let Some(s) = &self.snap else { return };
            if s.live() {
                text(ui, tr_n("scan-fps", format!("{:.0}", s.status.fps)));
            }
            if s.status.frames > 0 {
                text(ui, tr_n("scan-fused", s.status.fused_frames.to_string()));
            }
            let points = if s.live() {
                s.status.points
            } else {
                s.cloud.as_ref().map_or(s.status.points, |c| c.len())
            };
            if points > 0 {
                text(ui, tr_n("scan-points", points.to_string()));
            }
            if let Some([tilt, yaw]) = s.status.scanner_deg {
                text(
                    ui,
                    dc_i18n::tr_args(
                        "scan-scanner-pose",
                        &[
                            ("tilt", &turntable::deg(tilt)),
                            ("yaw", &turntable::deg(yaw)),
                        ],
                    ),
                );
            }
            if let Some(m) = s.mesh.as_ref().filter(|_| !s.live()) {
                text(ui, tr_n("scan-triangles", m.face_count().to_string()));
            }
            if s.live() && !s.finishing() {
                let (dot, key) = if s.status.tracking_ok {
                    (c.status_success, "scan-tracking-ok")
                } else {
                    (c.status_danger, "scan-tracking-lost")
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    widgets::status_dot(ui, dot);
                    ui.label(RichText::new(t(key)).font(fonts::medium(13.0)).color(
                        if s.status.tracking_ok {
                            c.text_primary
                        } else {
                            c.status_danger
                        },
                    ));
                });
            }
            let stream = s.status.error.as_ref().map(|code| ScanFailure {
                code: code.clone(),
                detail: s.status.error_detail.clone().unwrap_or_default(),
            });
            if s.reconnecting() {
                // not a failure: the job reconnects and goes on
                widgets::badge(ui, BadgeKind::Warning, t("turntable-reconnecting"));
            } else if let Some(f) = self
                .error
                .as_ref()
                .or(stream.as_ref())
                .or_else(|| s.job_error())
            {
                widgets::badge(ui, BadgeKind::Danger, t(&f.key())).on_hover_text(&f.detail);
            }
        });
    }

    fn view(&mut self, ui: &mut Ui, rect: Rect) {
        let c = *colors(ui);
        let content = self.content();
        ui.painter().rect_filled(rect, 8.0, c.bg_viewport);
        if self.viewport.is_none()
            && let Some(rs) = &self.render_state
        {
            let mut vp = Viewport::new(rs);
            vp.settings.hover_pick = false;
            // as the scanner sees the scan world (+Z up, the scanner on +X):
            // from its side, a little from above
            let e = VIEW_ELEVATION_DEG.to_radians();
            vp.camera_mut().rotation = look_rotation(Vec3::new(-e.cos(), 0.0, -e.sin()), Vec3::Z);
            self.viewport = Some(vp);
        }
        self.sync_view(ui);
        let mut response = None;
        if let Some(vp) = self.viewport.as_mut() {
            vp.settings.background = Some(scene_color(c.bg_viewport));
            let r = vp.ui(ui);
            // a camera that moved without a framing is the user's: the view
            // stays where they put it until the next framing is asked for
            if r.camera_changed && !r.fit_applied {
                self.follow.user_moved = true;
            }
            response = Some(r.response);
        } else {
            ui.allocate_rect(rect, Sense::hover());
            centered(ui, rect, &t("scan-no-gpu"), c.text_disabled);
        }
        if content == Content::Nothing {
            let key = if self.state() == ScanState::Idle {
                "scan-empty-idle"
            } else {
                "scan-empty-connected"
            };
            centered(ui, rect, &t(key), c.text_secondary);
        }
        // tracking lost: the view says so all around
        if content == Content::Live
            && self.snap.as_ref().is_some_and(|s| {
                s.status.state == ScanState::Scanning && !s.status.tracking_ok && !s.finishing()
            })
        {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                8.0,
                Stroke::new(3.0, c.status_danger),
                StrokeKind::Inside,
            );
        }
        if content == Content::Points {
            if let Some(r) = &response {
                self.rectangle_selection(ui, rect, r);
            }
            self.paint_cut(ui, rect);
        }
        self.view_toolbar(ui, rect, content);
    }

    /// The tools above the view: points or surface, edges, fit.
    fn view_toolbar(&mut self, ui: &mut Ui, rect: Rect, content: Content) {
        let has_mesh =
            self.snap.as_ref().is_some_and(|s| s.mesh.is_some()) && content != Content::Live;
        let bar = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(rect.width() - 20.0, 36.0));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(bar)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if has_mesh {
                    let mut which = usize::from(self.show_surface);
                    let labels = [t("scan-view-points"), t("scan-view-surface")];
                    if widgets::segmented(ui, &mut which, &labels, 28.0).changed() {
                        self.show_surface = which == 1;
                    }
                    if self.show_surface {
                        widgets::toggle(ui, &mut self.wireframe, t("scan-wireframe"));
                    }
                }
                if content != Content::Nothing
                    && widgets::icon_button(ui, ic::CORNERS_OUT, t("scan-fit")).clicked()
                {
                    self.refit();
                    self.shown = None;
                }
            },
        );
    }

    /// Left drag in the view while "Select with a rectangle" is on.
    fn rectangle_selection(&mut self, ui: &Ui, rect: Rect, r: &egui::Response) {
        if !self.selecting {
            self.drag = None;
            return;
        }
        let c = *colors(ui);
        if r.drag_started_by(PointerButton::Primary) {
            self.drag = r.interact_pointer_pos();
        }
        let Some(start) = self.drag else { return };
        let now = ui.input(|i| i.pointer.latest_pos()).unwrap_or(start);
        let sel = Rect::from_two_pos(start, now).intersect(rect);
        ui.painter().rect(
            sel,
            0.0,
            c.select_blue.gamma_multiply(0.15),
            Stroke::new(1.0, c.select_blue),
            StrokeKind::Inside,
        );
        if r.drag_stopped_by(PointerButton::Primary) || !ui.input(|i| i.pointer.primary_down()) {
            self.drag = None;
            let add = ui.input(|i| i.modifiers.shift);
            self.select(rect, sel, add);
        }
    }

    fn select(&mut self, rect: Rect, sel: Rect, add: bool) {
        let (Some(vp), Some(cloud)) = (
            self.viewport.as_ref(),
            self.snap.as_ref().and_then(|s| s.cloud.clone()),
        ) else {
            return;
        };
        let vp_matrix = select::view_proj(vp, rect);
        let mut indices = select::in_rect(&cloud.points, vp_matrix, rect, sel);
        if add && let Some(old) = self.selection.as_ref().filter(|s| s.cloud == addr(&cloud)) {
            indices.extend_from_slice(&old.indices);
            indices.sort_unstable();
            indices.dedup();
        }
        self.selection = (!indices.is_empty()).then(|| Selection {
            cloud: addr(&cloud),
            points: Arc::new(indices.iter().map(|&i| cloud.points[i as usize]).collect()),
            indices,
        });
    }

    /// The cutting level and its plane, from the camera of the view.
    fn cut(&self) -> Option<(Vec3, Vec3, f32)> {
        let vp = self.viewport.as_ref()?;
        let (lo, hi) = self.snap.as_ref()?.cloud.as_ref()?.aabb()?;
        let half = (hi - lo).length() * 0.5;
        let (p, n) = select::cut_plane(vp.camera(), lo, hi, self.cut_mm.clamp(-half, half));
        Some((p, n, half))
    }

    fn paint_cut(&self, ui: &Ui, rect: Rect) {
        if !self.cutting {
            return;
        }
        let (Some(vp), Some((p, _, half))) = (self.viewport.as_ref(), self.cut()) else {
            return;
        };
        let c = *colors(ui);
        let m = select::view_proj(vp, rect);
        let corners: Option<Vec<Pos2>> = select::cut_square(vp.camera(), p, half)
            .iter()
            .map(|&q| select::screen(m, rect, q))
            .collect();
        if let Some(corners) = corners {
            ui.painter().with_clip_rect(rect).add(Shape::convex_polygon(
                corners,
                c.status_danger.gamma_multiply(0.18),
                Stroke::new(1.5, c.status_danger),
            ));
        }
    }

    fn steps(&mut self, ui: &mut Ui, busy: bool, out: &mut Vec<Act>) {
        let c = *colors(ui);
        let Some(snap) = self.snap.clone() else {
            caption(ui, t("scan-empty-idle"));
            return;
        };
        let state = snap.status.state;
        let live = snap.live();
        let has_cloud = snap.cloud.is_some() && !live;
        let has_mesh = snap.mesh.is_some() && !live;
        // the next thing to do is the one primary button
        let next = if !has_cloud {
            1
        } else if !has_mesh {
            3
        } else {
            5
        };

        if busy {
            ui.horizontal(|ui| {
                widgets::spinner(ui, 14.0);
                ui.label(
                    RichText::new(t("scan-job"))
                        .font(fonts::medium(13.0))
                        .color(c.text_primary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::ghost(t("scan-cancel")).show(ui).clicked() {
                        out.push(Act::Cancel);
                    }
                });
            });
            ui.add_space(8.0);
        }
        // the surface being built: no other work meanwhile (a new job is
        // refused anyway); Stop below cancels the job and keeps its stops
        let busy = busy || snap.finishing();
        if has_cloud {
            ui.horizontal(|ui| {
                let edit = !busy;
                if Button::ghost(t("scan-undo"))
                    .icon(ic::ARROW_COUNTER_CLOCKWISE)
                    .enabled(edit && snap.can_undo)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(SCAN_UNDO, Value::Null));
                }
                if Button::ghost(t("scan-redo"))
                    .icon(ic::ARROW_CLOCKWISE)
                    .enabled(edit && snap.can_redo)
                    .show(ui)
                    .clicked()
                {
                    out.push(Act::Run(SCAN_REDO, Value::Null));
                }
            });
        }

        self.turntable_ui(ui, &snap, busy, out);

        // 1 acquisition
        step(ui, 1, "scan-step-acquire");
        // with a turntable connected, the automatic scan is the next thing
        let table = snap.table.connected;
        ui.horizontal_wrapped(|ui| {
            let (start_key, can_start) = match state {
                ScanState::Stopped => ("scan-continue", true),
                ScanState::Connected => ("scan-start", true),
                _ => ("scan-start", false),
            };
            let start = if next == 1 && !table {
                Button::primary(t(start_key))
            } else {
                Button::secondary(t(start_key))
            };
            if start
                .icon(ic::PLAY)
                .enabled(can_start && !busy)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(SCAN_START, Value::Null));
            }
            let (pause_key, pause_id, icon) = if state == ScanState::Paused {
                ("scan-resume", SCAN_RESUME, ic::PLAY)
            } else {
                ("scan-pause", SCAN_PAUSE, ic::PAUSE)
            };
            if Button::secondary(t(pause_key))
                .icon(icon)
                .enabled(live && !busy)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(pause_id, Value::Null));
            }
            if Button::secondary(t("scan-stop"))
                .icon(ic::STOP)
                .enabled(live && !busy)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(SCAN_STOP, Value::Null));
            }
            let something = live || state == ScanState::Stopped || snap.cloud.is_some();
            if Button::ghost(t("scan-reset"))
                .icon(ic::ARROW_COUNTER_CLOCKWISE)
                .enabled(something && !busy)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(SCAN_RESET, Value::Null));
            }
        });
        self.auto_ui(ui, &snap, busy, next == 1 && table, out);
        ui.add_space(8.0);
        self.settings_ui(ui, &snap, busy, out);

        // 2 point cloud
        step(ui, 2, "scan-step-cloud");
        if !has_cloud {
            caption(ui, t("scan-needs-stop"));
        }
        ui.add_enabled_ui(has_cloud && !busy, |ui| self.cloud_ui(ui, out));

        // 3 surface
        step(ui, 3, "scan-step-surface");
        ui.add_enabled_ui(has_cloud && !busy, |ui| {
            let labels = [
                t("scan-res-fine"),
                t("scan-res-standard"),
                t("scan-res-fast"),
                t("scan-res-custom"),
            ];
            widgets::segmented_fill(ui, &mut self.resolution, &labels, 32.0);
            ui.add_space(6.0);
            let mm = match RESOLUTIONS.get(self.resolution) {
                Some(&mm) => {
                    caption(ui, tr_n("scan-resolution-mm", format!("{mm:.2}")));
                    mm
                }
                None => {
                    widgets::labeled_slider(
                        ui,
                        &t("scan-resolution"),
                        &mut self.custom_mm,
                        0.1..=2.0,
                        0.05,
                        2,
                        "mm",
                        0.3,
                    );
                    self.custom_mm
                }
            };
            ui.add_space(6.0);
            let key = if has_mesh {
                "scan-rebuild"
            } else {
                "scan-build"
            };
            let b = if next == 3 {
                Button::primary(t(key))
            } else {
                Button::secondary(t(key))
            };
            if b.icon(ic::CUBE)
                .enabled(has_cloud && !busy)
                .full_width(true)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(SCAN_MESH_BUILD, json!({ "resolution_mm": mm })));
            }
        });
        ui.add_enabled_ui(has_cloud && !busy, |ui| {
            // a scan with stops (the automatic scan) can be rebuilt finer
            ui.add_space(12.0);
            sub_heading(ui, "scan-rebuild-title");
            caption(ui, t("scan-rebuild-hint"));
            ui.add_space(6.0);
            self.table.quality_ui(ui, false);
            ui.add_space(6.0);
            if Button::secondary(t("scan-rebuild-stops"))
                .icon(ic::ARROWS_CLOCKWISE)
                .enabled(has_cloud && !busy)
                .full_width(true)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(
                    SCAN_RECONSTRUCT,
                    json!({
                        "resolution_mm": self.table.resolution_mm(),
                        "refine": self.table.refine,
                    }),
                ));
            }
            if let Some(r) = &self.manual_recon {
                ui.add_space(6.0);
                recon_report(ui, r);
            }
        });

        // 4 surface tools
        step(ui, 4, "scan-step-tools");
        if !has_mesh {
            caption(ui, t("scan-needs-surface"));
        }
        ui.add_enabled_ui(has_mesh && !busy, |ui| {
            self.mesh_ui(ui, has_mesh && !busy, out)
        });

        // 5 import: the scan goes to the host, with one of the roles it takes
        step(ui, 5, "scan-step-import");
        let roles = self.target.offered().to_vec();
        let (button, hint) = (
            self.word(|w| &w.button, "scan-import"),
            self.word(|w| &w.hint, "scan-import-hint"),
        );
        ui.add_enabled_ui(has_mesh && !busy, |ui| {
            // one role: nothing to choose
            if roles.len() > 1 {
                let labels: Vec<String> = roles.iter().map(|&r| t(role_key(r))).collect();
                widgets::segmented_fill(ui, &mut self.role, &labels, 32.0);
                ui.add_space(8.0);
            }
            let role = roles[self.role.min(roles.len() - 1)];
            let b = if next == 5 {
                Button::primary(button)
            } else {
                Button::secondary(button)
            };
            if b.icon(ic::DOWNLOAD_SIMPLE)
                .enabled(has_mesh && !busy)
                .full_width(true)
                .show(ui)
                .clicked()
            {
                out.push(Act::Run(
                    SCAN_IMPORT,
                    json!({ "role": crate::role_id(role) }),
                ));
            }
            ui.add_space(6.0);
            caption(ui, hint);
        });
        if let (Some(m), Some(i)) = (&snap.mesh, &self.imported)
            && Arc::ptr_eq(m, i)
        {
            ui.add_space(6.0);
            let done = self.word(|w| &w.done, "scan-imported");
            widgets::badge(ui, BadgeKind::Success, done);
        }
        ui.add_space(12.0);
    }

    fn settings_ui(&mut self, ui: &mut Ui, snap: &Snapshot, busy: bool, out: &mut Vec<Act>) {
        // loaded from the controller once, then edited here (an applied
        // change comes back with the answer of `scan.settings`)
        if self.settings.is_none() {
            let s = snap.settings.clone();
            self.exposure_auto = s.exposure_us.is_none();
            if let Some(us) = s.exposure_us {
                self.exposure_us = us as f32;
            }
            self.settings = Some(s);
        }
        let s = self
            .settings
            .clone()
            .unwrap_or_else(|| snap.settings.clone());
        let editable = !snap.live() && !busy;
        if snap.live() {
            caption(ui, t("scan-settings-locked"));
            ui.add_space(4.0);
        }
        let mut apply = false;
        let mut s = s;
        ui.add_enabled_ui(editable, |ui| {
            let mut released = |r: egui::Response| {
                apply |= r.drag_stopped() || (r.changed() && !r.dragged());
            };
            released(widgets::labeled_slider(
                ui,
                &t("scan-detail"),
                &mut s.live_voxel_mm,
                0.1..=2.0,
                0.05,
                2,
                "mm",
                0.3,
            ));
            released(widgets::labeled_slider(
                ui,
                &t("scan-depth-min"),
                &mut s.min_depth_mm,
                0.0..=500.0,
                5.0,
                0,
                "mm",
                50.0,
            ));
            released(widgets::labeled_slider(
                ui,
                &t("scan-depth-max"),
                &mut s.max_depth_mm,
                20.0..=1000.0,
                5.0,
                0,
                "mm",
                300.0,
            ));
            ui.add_space(4.0);
            if widgets::toggle(ui, &mut self.exposure_auto, t("scan-exposure-auto")).changed() {
                apply = true;
            }
            if !self.exposure_auto {
                ui.add_space(4.0);
                let mut released = |r: egui::Response| {
                    apply |= r.drag_stopped() || (r.changed() && !r.dragged());
                };
                released(widgets::labeled_slider(
                    ui,
                    &t("scan-exposure"),
                    &mut self.exposure_us,
                    100.0..=30000.0,
                    100.0,
                    0,
                    "µs",
                    8000.0,
                ));
            }
            // the region of the turntable, once the rig is calibrated
            if snap.calibration.is_some() {
                ui.add_space(8.0);
                if widgets::toggle(ui, &mut s.use_roi, t("scan-roi")).changed() {
                    apply = true;
                }
                ui.add_space(4.0);
                caption(ui, t("scan-roi-hint"));
                if s.use_roi {
                    ui.add_space(4.0);
                    let mut released = |r: egui::Response| {
                        apply |= r.drag_stopped() || (r.changed() && !r.dragged());
                    };
                    released(widgets::labeled_slider(
                        ui,
                        &t("scan-roi-radius"),
                        &mut s.roi_radius_mm,
                        10.0..=150.0,
                        1.0,
                        0,
                        "mm",
                        45.0,
                    ));
                    released(widgets::labeled_slider(
                        ui,
                        &t("scan-roi-height"),
                        &mut s.roi_height_mm,
                        10.0..=200.0,
                        1.0,
                        0,
                        "mm",
                        60.0,
                    ));
                }
            }
        });
        if apply {
            let exposure = if self.exposure_auto {
                0
            } else {
                self.exposure_us.round() as u32
            };
            s.exposure_us = (exposure > 0).then_some(exposure);
            out.push(Act::Run(
                SCAN_SETTINGS,
                json!({
                    "live_voxel_mm": s.live_voxel_mm,
                    "min_depth_mm": s.min_depth_mm,
                    "max_depth_mm": s.max_depth_mm,
                    "exposure_us": exposure,
                    "use_roi": s.use_roi,
                    "roi_radius_mm": s.roi_radius_mm,
                    "roi_height_mm": s.roi_height_mm,
                }),
            ));
        }
        self.settings = Some(s);
    }

    fn cloud_ui(&mut self, ui: &mut Ui, out: &mut Vec<Act>) {
        let run =
            |out: &mut Vec<Act>, op: CloudOp| out.push(Act::Run(SCAN_CLOUD, json!({ "op": op })));
        widgets::labeled_slider(
            ui,
            &t("scan-strictness"),
            &mut self.std_ratio,
            0.5..=5.0,
            0.1,
            1,
            "",
            2.0,
        );
        if Button::secondary(t("scan-outliers"))
            .icon(ic::BROOM)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            run(
                out,
                CloudOp::RemoveStatistical {
                    k: 16,
                    std_ratio: self.std_ratio,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider_mm(
            ui,
            &t("scan-isolate-gap"),
            &mut self.isolate_mm,
            0.2..=5.0,
            1.0,
        );
        if Button::secondary(t("scan-isolate"))
            .icon(ic::SELECTION)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            run(
                out,
                CloudOp::Isolate {
                    radius_mm: self.isolate_mm,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider(
            ui,
            &t("scan-strength"),
            &mut self.smooth_strength,
            0.1..=1.0,
            0.05,
            2,
            "",
            0.5,
        );
        if Button::secondary(t("scan-smooth-points"))
            .icon(ic::WAVE_SINE)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            run(
                out,
                CloudOp::Smooth {
                    k: 12,
                    iterations: 1,
                    strength: self.smooth_strength,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider_mm(
            ui,
            &t("scan-spacing"),
            &mut self.spacing_mm,
            0.05..=1.0,
            0.2,
        );
        if Button::secondary(t("scan-thin"))
            .icon(ic::DOTS_NINE)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            run(
                out,
                CloudOp::Downsample {
                    voxel_mm: self.spacing_mm,
                },
            );
        }
        ui.add_space(10.0);
        if widgets::toggle(ui, &mut self.selecting, t("scan-select")).changed() && !self.selecting {
            self.selection = None;
        }
        if self.selecting {
            ui.add_space(4.0);
            caption(ui, t("scan-select-hint"));
            let n = self.selection.as_ref().map_or(0, |s| s.indices.len());
            ui.add_space(4.0);
            caption(ui, tr_n("scan-selected", n.to_string()));
            ui.horizontal(|ui| {
                if Button::secondary(t("scan-delete-selected"))
                    .icon(ic::TRASH)
                    .enabled(n > 0)
                    .show(ui)
                    .clicked()
                    && let Some(sel) = self.selection.take()
                {
                    run(
                        out,
                        CloudOp::DeleteIndices {
                            indices: sel.indices,
                        },
                    );
                }
                if Button::ghost(t("scan-clear-selection"))
                    .enabled(n > 0)
                    .show(ui)
                    .clicked()
                {
                    self.selection = None;
                }
            });
        }
        ui.add_space(10.0);
        widgets::toggle(ui, &mut self.cutting, t("scan-cut"));
        if self.cutting {
            ui.add_space(4.0);
            caption(ui, t("scan-cut-hint"));
            let half = self.cut().map_or(50.0, |(_, _, h)| h);
            widgets::labeled_slider_mm(
                ui,
                &t("scan-cut-level"),
                &mut self.cut_mm,
                -half..=half,
                0.0,
            );
            if Button::secondary(t("scan-cut-apply"))
                .icon(ic::SCISSORS)
                .full_width(true)
                .show(ui)
                .clicked()
                && let Some((p, n, _)) = self.cut()
            {
                run(
                    out,
                    CloudOp::DeletePlane {
                        point: p.to_array(),
                        normal: n.to_array(),
                    },
                );
            }
        }
    }

    fn mesh_ui(&mut self, ui: &mut Ui, enabled: bool, out: &mut Vec<Act>) {
        let run =
            |out: &mut Vec<Act>, op: MeshOp| out.push(Act::Run(SCAN_MESH, json!({ "op": op })));
        let button = |ui: &mut Ui, key: &str, icon: &'static str| {
            Button::secondary(t(key))
                .icon(icon)
                .enabled(enabled)
                .full_width(true)
                .show(ui)
                .clicked()
        };
        if button(ui, "scan-finish-mesh", ic::MAGIC_WAND) {
            out.push(Act::Run(SCAN_MESH, json!({ "op": "finish" })));
        }
        ui.add_space(2.0);
        caption(ui, t("scan-finish-mesh-hint"));
        if let Some(f) = &self.manual_finish {
            ui.add_space(4.0);
            finish_report(ui, f);
        }
        ui.add_space(10.0);
        widgets::labeled_slider(
            ui,
            &t("scan-hole-size"),
            &mut self.hole_edges,
            10.0..=2000.0,
            10.0,
            0,
            "",
            200.0,
        );
        if button(ui, "scan-fill-holes", ic::BANDAIDS) {
            run(
                out,
                MeshOp::FillHoles {
                    max_edges: self.hole_edges.round() as usize,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider(
            ui,
            &t("scan-passes"),
            &mut self.passes,
            1.0..=20.0,
            1.0,
            0,
            "",
            5.0,
        );
        if button(ui, "scan-smooth-surface", ic::WAVE_SINE) {
            run(
                out,
                MeshOp::Smooth {
                    iterations: self.passes.round() as u32,
                    lambda: 0.5,
                    mu: -0.53,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider(
            ui,
            &t("scan-island-size"),
            &mut self.island_pct,
            1.0..=50.0,
            1.0,
            0,
            "%",
            10.0,
        );
        if button(ui, "scan-islands", ic::BROOM) {
            run(
                out,
                MeshOp::RemoveIslands {
                    min_fraction: self.island_pct / 100.0,
                },
            );
        }
        ui.add_space(8.0);
        widgets::labeled_slider(
            ui,
            &t("scan-keep"),
            &mut self.keep_pct,
            10.0..=100.0,
            5.0,
            0,
            "%",
            50.0,
        );
        if button(ui, "scan-simplify", ic::POLYGON) {
            run(
                out,
                MeshOp::Decimate {
                    ratio: self.keep_pct / 100.0,
                },
            );
        }
    }
}

/// The heading of a step: its number in a circle and its name.
fn step(ui: &mut Ui, n: u8, key: &str) {
    let c = *colors(ui);
    ui.add_space(14.0);
    widgets::hairline(ui);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter()
            .circle_stroke(rect.center(), 10.0, Stroke::new(1.5, c.select_blue));
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            n.to_string(),
            fonts::semibold(12.0),
            c.select_blue,
        );
        ui.label(
            RichText::new(t(key))
                .font(fonts::semibold(14.0))
                .color(c.text_primary),
        );
    });
    ui.add_space(8.0);
}

fn centered(ui: &Ui, rect: Rect, text: &str, color: Color32) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        fonts::ui(14.0),
        color,
    );
}

/// Linear RGBA of a token colour (the viewport's point colours).
fn linear(c: Color32) -> [f32; 4] {
    let r = egui::Rgba::from(c);
    [r.r(), r.g(), r.b(), 1.0]
}

fn cloud_points(u: &Uploaded, color: [f32; 4], size: f32) -> ViewPoints {
    let mut p = ViewPoints::new(CLOUD_ID, Arc::clone(&u.points))
        .with_color(color)
        .with_size_px(size);
    if let Some(n) = &u.normals {
        p = p.with_normals(Arc::clone(n));
    }
    p
}

/// The i18n key of the name of `role`.
fn role_key(role: ScanRole) -> &'static str {
    match role {
        ScanRole::Upper => "scan-role-upper",
        ScanRole::Lower => "scan-role-lower",
        ScanRole::Bite => "scan-role-bite",
    }
}

/// The "Scan" button: secondary, with its alpha badge on its right. It only
/// says it was clicked: the host opens the popup ([`ScanPopup::open`]),
/// which starts nothing by itself.
pub fn scan_button(ui: &mut Ui, enabled: bool) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let clicked = Button::secondary(t("scan-button"))
            .icon(ic::SCAN)
            .enabled(enabled)
            .show(ui)
            .on_hover_text(t("scan-button-tip"))
            .clicked();
        widgets::badge_icon(ui, BadgeKind::Amber, None, t("scan-alpha"));
        clicked
    })
    .inner
}

/// Width of [`scan_button`], to lay it out beside another button.
pub fn scan_button_width(ui: &Ui) -> f32 {
    let w = |text: String, font| {
        ui.painter()
            .layout_no_wrap(text, font, Color32::PLACEHOLDER)
            .size()
            .x
    };
    let button = 16.0 + 8.0 + w(t("scan-button"), fonts::semibold(13.0)) + 32.0;
    let badge = 8.0 + w(t("scan-alpha"), fonts::medium(12.0)) + 8.0;
    button + 6.0 + badge
}

impl ScanPopup {
    /// Draws the window, when open, and its close confirmation, and runs
    /// what its controls ask for. Call it every frame, under the host's own
    /// dialogs (a running command asks for the next frame). It polls too;
    /// the host still takes the outputs ([`ScanPopup::take_outputs`]).
    pub fn show(&mut self, ui: &mut Ui) {
        self.poll();
        let ctx = ui.ctx().clone();
        if self.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if !self.ui.open {
            return;
        }
        if let Some(s) = self.try_with(Snapshot::read) {
            self.ui.snap = Some(s);
        }
        if self.ui.snap.as_ref().is_some_and(Snapshot::moving) {
            ctx.request_repaint_after(LIVE_FRAME);
        }
        let busy = self.is_busy();
        let screen = ctx.content_rect();
        let size = vec2(screen.width() * 0.9, screen.height() * 0.9) - vec2(32.0, 32.0);
        let mut acts = Vec::new();
        let resp = Modal::new(Id::unique("dc-scan-popup"))
            .frame(widgets::card_frame(ui).inner_margin(Margin::same(16)))
            .show(&ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                self.ui.contents(ui, busy, &mut acts);
            });
        // keys of the popup: Escape closes, Ctrl+Z / Ctrl+Shift+Z undo and redo
        // the scan edits
        if resp.is_top_modal && !resp.any_popup_open && !self.ui.confirm_close {
            let (esc, redo, undo) = ctx.input_mut(|i| {
                let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(cmd_shift, Key::Z),
                    i.consume_key(Modifiers::COMMAND, Key::Z),
                )
            });
            if esc {
                acts.push(Act::Close);
            }
            if !busy && redo {
                acts.push(Act::Run(SCAN_REDO, Value::Null));
            } else if !busy && undo {
                acts.push(Act::Run(SCAN_UNDO, Value::Null));
            }
        }
        self.ui.auto_confirm(ui, &mut acts);
        for act in acts {
            match act {
                Act::Run(id, params) => self.command(id, params),
                Act::Cancel => self.cancel(),
                Act::Close if self.ui.unimported() => self.ui.confirm_close = true,
                Act::Close => self.close(),
            }
        }
        if self.ui.confirm_close {
            self.confirm_close(ui);
        }
    }

    fn confirm_close(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let body = self.ui.word(|w| &w.unsaved, "scan-confirm-body");
        let resp = Modal::new(Id::unique("dc-scan-confirm-close"))
            .frame(widgets::card_frame(ui).inner_margin(Margin::same(24)))
            .show(ui.ctx(), |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(t("scan-confirm-title"))
                        .font(fonts::semibold(18.0))
                        .color(c.text_primary),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(body)
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::primary(t("scan-confirm-keep")).show(ui).clicked() {
                        return Some(false);
                    }
                    if Button::secondary(t("scan-confirm-discard"))
                        .show(ui)
                        .clicked()
                    {
                        return Some(true);
                    }
                    None
                })
                .inner
            });
        match resp.inner {
            Some(true) => self.close(),
            Some(false) => self.ui.confirm_close = false,
            None if resp.should_close() => self.ui.confirm_close = false,
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_scanner::{VendorStatus, control::ScanController};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// The popup drawn with the status of a simulated session, whose vendor
    /// layer is `vendor` (the simulated device has none: a fake one stands
    /// in for a scanner that reports it).
    fn drawn(vendor: Option<VendorStatus>) -> Harness<'static> {
        let controller = ScanController::new();
        let mut snap = Snapshot::read(&controller);
        assert!(snap.status.vendor.is_none(), "no device, no vendor layer");
        snap.status.vendor = vendor;
        let mut popup = ScanUi::new(None);
        popup.open = true;
        popup.snap = Some(snap);
        let mut h = Harness::builder()
            .with_size(egui::vec2(1200.0, 800.0))
            // the device boxes of the popup have no accessible name (not
            // what is tested here)
            .with_accessibility_check(false)
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui(move |ui| popup.contents(ui, false, &mut Vec::new()));
        h.run();
        h
    }

    fn fake(calibration: &'static str, fast: &'static str) -> VendorStatus {
        VendorStatus {
            fill_light: "on".into(),
            fill_light_note: None,
            fast_exposure: fast.into(),
            fast_exposure_note: Some("note of the device".into()),
            calibration,
            calibration_note: None,
            fx: 1142.37,
            fy: 1142.37,
            cx: 163.19,
            cy: 329.85,
        }
    }

    #[test]
    fn the_device_line_of_the_popup_shows_what_the_vendor_layer_did() {
        let h = drawn(Some(fake("factory", "applied")));
        assert!(
            h.query_by_label(
                "Light on · Fast exposure applied · Calibration: factory (fx 1142.4, cx 163.2, cy 329.9)"
            )
            .is_some()
        );
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_none()
        );
        assert!(h.query_by_label_contains("did not apply").is_none());
    }

    #[test]
    fn the_popup_warns_of_an_estimate_and_of_an_error() {
        let h = drawn(Some(fake("estimate", "applied")));
        assert!(
            h.query_by_label_contains("Approximate calibration")
                .is_some()
        );
        let h = drawn(Some(fake("factory", "error:device")));
        assert!(h.query_by_label_contains("did not apply").is_some());
    }

    #[test]
    fn a_device_without_a_vendor_layer_shows_no_line() {
        let h = drawn(None);
        assert!(h.query_by_label_contains("Calibration:").is_none());
        assert!(h.query_by_label_contains("Light").is_none());
    }
}
