//! DentalCraft 3D viewport for egui.
//!
//! [`Viewport`] renders triangle meshes with wgpu inside an egui `Ui`
//! (offscreen pass recorded by an [`egui_wgpu::CallbackTrait`] paint
//! callback, MSAA and depth), and handles navigation (orbit, pan, zoom to
//! cursor, fit, [`StdView`] standard dental views, perspective/orthographic),
//! an axis gizmo and CPU picking through a per-mesh [`Bvh`]. A selection is
//! drawn with a screen-space outline around the selected meshes
//! ([`Viewport::set_highlight`]), and a [`TransformGizmo`] moves, turns or
//! scales it ([`Viewport::set_transform_gizmo`], [`GizmoEvent`]).
//!
//! Meshes are uploaded once (and cached by `Arc` identity across
//! [`Viewport::set_meshes`] calls); a frame only rewrites the camera and
//! per-mesh uniforms, and the 3D pass is skipped entirely when nothing
//! changed since the last frame.
//!
//! World frame: occlusal plane XY, incisors towards −Y, patient's right on
//! −X, lower arch teeth up (+Z); see [`camera`].
//!
//! ```no_run
//! # fn demo(render_state: &egui_wgpu::RenderState, ui: &mut egui::Ui, mesh: std::sync::Arc<dc_mesh::TriMesh>) {
//! use dc_viewport::{StdView, ViewMesh, Viewport};
//! let mut viewport = Viewport::new(render_state);
//! viewport.set_meshes(vec![ViewMesh::new(1, mesh)]);
//! viewport.set_view(StdView::OcclusalLower);
//! viewport.fit();
//! let response = viewport.ui(ui);
//! if let Some(pick) = response.pick {
//!     println!("clicked mesh {} at {}", pick.mesh_id, pick.point);
//! }
//! # }
//! ```

pub mod bvh;
pub mod camera;
pub mod coloring;
mod gizmo;
pub mod pick;
mod render;
mod transform;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use dc_case::BlobStore;
use dc_mesh::TriMesh;
use dc_scene::{Color, MeshColoring, SceneItem};
use egui::{PointerButton, Pos2, Rect, Sense};
use egui_wgpu::wgpu;
use glam::{Mat4, Vec2, Vec3};

pub use bvh::Bvh;
pub use camera::{Camera, FitInsets, Projection, Ray, Sphere, StdView};
pub use coloring::{label_coloring, label_colors, resolve_colors};
pub use pick::{Pick, PickTarget};
pub use transform::{GizmoDelta, GizmoEvent, GizmoMode, TransformGizmo};

use render::{
    CameraUniform, ColorData, DrawItem, FrameCallback, Gpu, GpuMesh, OutlinePass, Targets,
};

/// Caller-chosen identifier of a mesh in the viewport.
pub type MeshId = u64;

/// A mesh to show.
#[derive(Debug, Clone)]
pub struct ViewMesh {
    /// Identifier reported by picks.
    pub id: MeshId,
    /// Geometry (mesh-local coordinates, mm). Uploads are cached by `Arc`
    /// identity: pass the same `Arc` again to avoid re-uploading.
    pub mesh: Arc<TriMesh>,
    /// Local to world transform.
    pub xform: Mat4,
    /// Colouring (uniform, per-vertex colours or labels with a palette).
    pub coloring: MeshColoring,
    /// Drawn and pickable only when `true`.
    pub visible: bool,
    /// Opacity in `[0, 1]`; below 1 the mesh is drawn blended after the
    /// opaque ones without writing depth.
    pub opacity: f32,
    /// Can be picked.
    pub pickable: bool,
}

impl ViewMesh {
    /// Visible, opaque, pickable mesh in the default tooth colour.
    pub fn new(id: MeshId, mesh: Arc<TriMesh>) -> Self {
        Self {
            id,
            mesh,
            xform: Mat4::IDENTITY,
            coloring: MeshColoring::Uniform(Color::TOOTH),
            visible: true,
            opacity: 1.0,
            pickable: true,
        }
    }

    /// Builder: transform.
    pub fn with_xform(mut self, xform: Mat4) -> Self {
        self.xform = xform;
        self
    }

    /// Builder: colouring.
    pub fn with_coloring(mut self, coloring: MeshColoring) -> Self {
        self.coloring = coloring;
        self
    }

    /// Builder: opacity.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Converts a [`SceneItem::Mesh`], looking its blob up in `blobs`.
    /// Returns `None` for other items or a missing blob.
    pub fn from_scene_item(id: MeshId, item: &SceneItem, blobs: &BlobStore) -> Option<Self> {
        match item {
            SceneItem::Mesh {
                blob,
                coloring,
                xform,
                pickable,
            } => Some(Self {
                id,
                mesh: blobs.get(blob)?.clone(),
                xform: xform.to_mat4().as_mat4(),
                coloring: coloring.clone(),
                visible: true,
                opacity: 1.0,
                pickable: *pickable,
            }),
            _ => None,
        }
    }
}

/// How the back side of triangles is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackfaceMode {
    /// Lit like the front side.
    Show,
    /// Tinted with [`ViewportSettings::backface_color`] (shows the inside of
    /// open scans).
    #[default]
    Tint,
    /// Not drawn.
    Cull,
}

/// Mouse bindings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Navigation {
    /// Button that orbits (with Shift held it pans).
    pub orbit_button: PointerButton,
    /// Button that pans.
    pub pan_button: PointerButton,
    /// Orbit speed in radians per point dragged.
    pub orbit_speed: f32,
    /// Zoom speed per point scrolled.
    pub zoom_speed: f32,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            orbit_button: PointerButton::Secondary,
            pan_button: PointerButton::Middle,
            orbit_speed: 0.008,
            zoom_speed: 0.0025,
        }
    }
}

/// Corner of the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Corner {
    /// Top left.
    TopLeft,
    /// Top right.
    TopRight,
    /// Bottom left.
    #[default]
    BottomLeft,
    /// Bottom right.
    BottomRight,
}

impl Corner {
    /// Centre of an element of `radius` points placed in this corner of
    /// `rect` with an 8 pt margin.
    fn anchor(self, rect: Rect, radius: f32) -> Pos2 {
        let off = radius + 8.0;
        let x = match self {
            Self::TopLeft | Self::BottomLeft => rect.left() + off,
            Self::TopRight | Self::BottomRight => rect.right() - off,
        };
        let y = match self {
            Self::TopLeft | Self::TopRight => rect.top() + off,
            Self::BottomLeft | Self::BottomRight => rect.bottom() - off,
        };
        Pos2::new(x, y)
    }
}

/// Display options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportSettings {
    /// Draw triangle edges over the surfaces.
    pub wireframe: bool,
    /// Wireframe colour (alpha is used).
    pub wire_color: Color,
    /// Back side drawing.
    pub backface: BackfaceMode,
    /// Tint of back faces (alpha = strength) with [`BackfaceMode::Tint`].
    pub backface_color: Color,
    /// Background; `None` uses the egui theme's extreme background colour.
    pub background: Option<Color>,
    /// Pick under the cursor while hovering ([`ViewportResponse::hovered`]).
    pub hover_pick: bool,
    /// Where to draw the built-in axis gizmo; `None` hides it (a host with
    /// its own orientation widget turns it off).
    pub gizmo: Option<Corner>,
    /// Margin factor used by [`Viewport::fit`].
    pub fit_margin: f32,
    /// What floats over the edges of the viewport (points): [`Viewport::fit`]
    /// frames the meshes in the free area left inside it.
    pub fit_insets: FitInsets,
    /// Colour of the selection outline (linear; alpha = opacity), also the
    /// tint of the mesh under the pointer.
    pub outline_color: Color,
    /// Width of the selection outline (points).
    pub outline_width: f32,
    /// Strength of the outline around the parts of the selection other
    /// meshes cover (`0` = none, `1` = as around the visible parts).
    pub outline_hidden: f32,
    /// Strength of the tint of the highlighted mesh under the pointer.
    pub hover_tint: f32,
}

impl Default for ViewportSettings {
    fn default() -> Self {
        Self {
            wireframe: false,
            wire_color: Color::rgba(0.05, 0.05, 0.08, 0.45),
            backface: BackfaceMode::Tint,
            backface_color: Color::rgba(0.25, 0.32, 0.55, 0.65),
            background: None,
            hover_pick: false,
            gizmo: Some(Corner::BottomLeft),
            fit_margin: 1.05,
            fit_insets: FitInsets::NONE,
            // the design's select/blue (#6F9BFF), linear
            outline_color: Color::rgb(0.159, 0.332, 1.0),
            outline_width: 3.0,
            outline_hidden: 0.35,
            hover_tint: 0.22,
        }
    }
}

/// What happened in the viewport this frame.
#[derive(Debug, Clone)]
pub struct ViewportResponse {
    /// The egui response of the viewport area.
    pub response: egui::Response,
    /// Primary click on a mesh (never on a handle of the transform gizmo).
    pub pick: Option<Pick>,
    /// Mesh under the cursor (only with [`ViewportSettings::hover_pick`]).
    pub hovered: Option<Pick>,
    /// The camera moved this frame.
    pub camera_changed: bool,
    /// What the transform gizmo did this frame.
    pub gizmo: Option<GizmoEvent>,
    /// The pointer is over a handle of the transform gizmo.
    pub gizmo_hovered: bool,
    /// The primary button belongs to the transform gizmo this frame (a press
    /// on a handle, or a drag of it): the host leaves the click alone.
    pub gizmo_consumed: bool,
}

struct Entry {
    id: MeshId,
    mesh: Arc<TriMesh>,
    gpu: Arc<GpuMesh>,
    bvh: Arc<OnceLock<Bvh>>,
    coloring: MeshColoring,
    colors: ColorData,
    local_box: Option<(Vec3, Vec3)>,
    xform: Mat4,
    visible: bool,
    opacity: f32,
    pickable: bool,
}

/// Inputs of the last rendered frame: when they do not change, the 3D pass
/// is skipped and egui keeps showing the previous image.
#[derive(Clone, PartialEq)]
struct RenderKey {
    camera: Camera,
    settings: ViewportSettings,
    clear: Color,
    size: [u32; 2],
    scene_version: u64,
    highlight_version: u64,
}

/// The 3D viewport widget.
pub struct Viewport {
    gpu: Arc<Gpu>,
    renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    max_texture_size: u32,
    texture: Option<egui::TextureId>,
    targets: Option<Arc<Targets>>,
    entries: Vec<Entry>,
    bounds: Option<(Vec3, Vec3)>,
    camera: Camera,
    /// Display options.
    pub settings: ViewportSettings,
    /// Mouse bindings.
    pub navigation: Navigation,
    scene_version: u64,
    last_render: Option<RenderKey>,
    pending_fit: bool,
    fitted_once: bool,
    last_hover: Option<(Pos2, Camera, u64)>,
    hovered: Option<Pick>,
    /// Meshes drawn with the selection outline.
    selected: Vec<MeshId>,
    /// Meshes tinted as under the pointer.
    hover_ids: Vec<MeshId>,
    highlight_version: u64,
    /// The transform gizmo shown, its drag and the handle under the pointer.
    transform: Option<TransformGizmo>,
    gizmo_drag: Option<transform::Drag>,
    gizmo_hot: Option<transform::Handle>,
    /// Where the viewport was drawn last (the gizmo is painted there).
    last_rect: Rect,
}

impl Viewport {
    /// Creates the GPU pipelines on the device of `render_state`
    /// (from `eframe::Frame::wgpu_render_state`).
    pub fn new(render_state: &egui_wgpu::RenderState) -> Self {
        let gpu = Gpu::new(
            &render_state.device,
            &render_state.queue,
            &render_state.adapter,
        );
        Self {
            max_texture_size: render_state.device.limits().max_texture_dimension_2d,
            gpu: Arc::new(gpu),
            renderer: render_state.renderer.clone(),
            texture: None,
            targets: None,
            entries: Vec::new(),
            bounds: None,
            camera: Camera::default(),
            settings: ViewportSettings::default(),
            navigation: Navigation::default(),
            scene_version: 0,
            last_render: None,
            pending_fit: false,
            fitted_once: false,
            last_hover: None,
            hovered: None,
            selected: Vec::new(),
            hover_ids: Vec::new(),
            highlight_version: 0,
            transform: None,
            gizmo_drag: None,
            gizmo_hot: None,
            last_rect: Rect::NOTHING,
        }
    }

    /// MSAA sample count in use (1 when unsupported).
    pub fn sample_count(&self) -> u32 {
        self.gpu.sample_count
    }

    /// Replaces the shown meshes. Geometry already uploaded for the same id
    /// and the same `Arc<TriMesh>` is reused; changed colourings are
    /// re-uploaded in place; transforms and opacity are uniforms only.
    pub fn set_meshes(&mut self, meshes: Vec<ViewMesh>) {
        let mut old: HashMap<MeshId, Entry> = self.entries.drain(..).map(|e| (e.id, e)).collect();
        for vm in meshes {
            let entry = match old.remove(&vm.id) {
                Some(mut e) if Arc::ptr_eq(&e.mesh, &vm.mesh) => {
                    if !same_coloring(&e.coloring, &vm.coloring) {
                        e.colors = ColorData::new(&vm.mesh, &vm.coloring);
                        self.gpu.update_colors(&e.gpu, &e.colors);
                        e.coloring = vm.coloring;
                    }
                    e.xform = vm.xform;
                    e.visible = vm.visible;
                    e.opacity = vm.opacity;
                    e.pickable = vm.pickable;
                    e
                }
                _ => {
                    let colors = ColorData::new(&vm.mesh, &vm.coloring);
                    Entry {
                        id: vm.id,
                        gpu: Arc::new(self.gpu.upload(&vm.mesh, &colors)),
                        bvh: Arc::new(OnceLock::new()),
                        local_box: vm.mesh.aabb().map(|b| (b.min, b.max)),
                        mesh: vm.mesh,
                        coloring: vm.coloring,
                        colors,
                        xform: vm.xform,
                        visible: vm.visible,
                        opacity: vm.opacity,
                        pickable: vm.pickable,
                    }
                }
            };
            self.entries.push(entry);
        }
        self.bounds = self
            .entries
            .iter()
            .filter(|e| e.visible)
            .filter_map(|e| {
                e.local_box
                    .map(|(lo, hi)| render::transform_box(lo, hi, e.xform))
            })
            .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)));
        self.scene_version += 1;
        self.hovered = None;
        self.last_hover = None;
        if !self.fitted_once && self.bounds.is_some() {
            self.pending_fit = true;
        }
    }

    /// Ids of the meshes currently set.
    pub fn mesh_ids(&self) -> impl Iterator<Item = MeshId> + '_ {
        self.entries.iter().map(|e| e.id)
    }

    /// World bounding box of the visible meshes.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bounds
    }

    /// The camera.
    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// Mutable access to the camera.
    pub fn camera_mut(&mut self) -> &mut Camera {
        &mut self.camera
    }

    /// Switches to a standard view (keeps the orbit centre and distance).
    pub fn set_view(&mut self, view: StdView) {
        self.camera.set_view(view);
    }

    /// Sets the projection.
    pub fn set_projection(&mut self, projection: Projection) {
        self.camera.projection = projection;
    }

    /// Toggles perspective / orthographic.
    pub fn toggle_projection(&mut self) {
        self.camera.projection = match self.camera.projection {
            Projection::Perspective => Projection::Orthographic,
            Projection::Orthographic => Projection::Perspective,
        };
    }

    /// Frames the visible meshes (applied at the next [`Viewport::ui`], when
    /// the size of the area is known).
    pub fn fit(&mut self) {
        self.pending_fit = true;
    }

    fn bounding_sphere(&self) -> Option<Sphere> {
        self.bounds.map(|(lo, hi)| Sphere {
            center: (lo + hi) * 0.5,
            radius: (hi - lo).length() * 0.5,
        })
    }

    /// Picks along a world ray (visible, pickable meshes). The BVH of a mesh
    /// is built on its first pick.
    pub fn pick_ray(&self, ray: &Ray) -> Option<Pick> {
        let targets = self
            .entries
            .iter()
            .filter(|e| e.visible && e.pickable && e.opacity > 0.0)
            .map(|e| PickTarget {
                id: e.id,
                mesh: &e.mesh,
                bvh: e.bvh.get_or_init(|| Bvh::build(&e.mesh)),
                xform: e.xform,
            });
        pick::pick(ray, targets)
    }

    /// Size in points, in a viewport of `rect`, of a length of `length_mm`
    /// across the view at world point `p` (the radius of a brush circle).
    pub fn screen_length(&self, rect: Rect, p: Vec3, length_mm: f32) -> f32 {
        self.camera.screen_length(
            p,
            length_mm,
            Vec2::new(rect.width(), rect.height()),
            self.bounding_sphere(),
        )
    }

    /// Screen position, inside the viewport `rect`, of world point `p`.
    pub fn screen_pos(&self, rect: Rect, p: Vec3) -> Pos2 {
        let ndc = self
            .camera
            .project_ndc(p, aspect_of(rect), self.bounding_sphere());
        Pos2::new(
            rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
        )
    }

    /// Picks under a screen position inside the viewport `rect`.
    pub fn pick_at(&self, rect: Rect, pos: Pos2) -> Option<Pick> {
        let ray = self
            .camera
            .ray(ndc_of(rect, pos), aspect_of(rect), self.bounding_sphere());
        self.pick_ray(&ray)
    }

    /// Draws the selection outline around the meshes `selected` and tints
    /// the meshes `hovered` (the object under the pointer). Ids that are not
    /// shown are ignored.
    pub fn set_highlight(&mut self, selected: &[MeshId], hovered: &[MeshId]) {
        if self.selected != selected || self.hover_ids != hovered {
            self.selected = selected.to_vec();
            self.hover_ids = hovered.to_vec();
            self.highlight_version += 1;
        }
    }

    /// Shows the transform gizmo (`None` hides it and drops a drag going
    /// on). A drag keeps going while the gizmo stays in the same mode, its
    /// pivot may follow the preview of the drag.
    pub fn set_transform_gizmo(&mut self, gizmo: Option<TransformGizmo>) {
        if gizmo.map(|g| g.mode) != self.transform.map(|g| g.mode) {
            self.gizmo_drag = None;
            self.gizmo_hot = None;
        }
        self.transform = gizmo;
    }

    /// A handle of the transform gizmo is being dragged.
    pub fn transform_dragging(&self) -> bool {
        self.gizmo_drag.is_some()
    }

    /// Drops the drag of the transform gizmo going on (Escape); `true` when
    /// there was one. No [`GizmoEvent::Commit`] follows.
    pub fn cancel_transform_drag(&mut self) -> bool {
        self.gizmo_hot = None;
        self.gizmo_drag.take().is_some()
    }

    /// Draws the transform gizmo over the last frame of the viewport, with
    /// `painter` (the host draws it after its own overlays, so the gizmo
    /// stays on top of them). Nothing when no gizmo is set or its pivot is
    /// behind the camera.
    pub fn paint_transform_gizmo(&self, painter: &egui::Painter, pointer: Option<Pos2>) {
        let Some(g) = self.transform else { return };
        let rect = self.last_rect;
        if !rect.is_positive() || self.camera.forward().dot(g.pivot - self.camera.eye()) <= 0.0 {
            return;
        }
        let proj = self.projector(rect);
        transform::paint(
            &painter.with_clip_rect(rect),
            &g,
            &proj,
            self.gizmo_hot,
            self.gizmo_drag.as_ref().map(|d| &d.last),
            pointer,
        );
    }

    fn projector(&self, rect: Rect) -> transform::Projector {
        transform::Projector {
            camera: self.camera,
            rect,
            bounds: self.bounding_sphere(),
        }
    }

    /// The transform gizmo's part of the input: a drag that starts on a
    /// handle, goes on or ends, and the handle under the pointer. Returns the
    /// event and whether the primary button belongs to the gizmo.
    fn gizmo_input(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        response: &egui::Response,
    ) -> (Option<GizmoEvent>, bool) {
        let Some(g) = self.transform else {
            self.gizmo_drag = None;
            self.gizmo_hot = None;
            return (None, false);
        };
        if self.camera.forward().dot(g.pivot - self.camera.eye()) <= 0.0 {
            self.gizmo_drag = None;
            self.gizmo_hot = None;
            return (None, false);
        }
        let proj = self.projector(rect);
        let press = ui.input(|i| i.pointer.press_origin());
        let on_handle = |pos: Pos2| transform::hit(&g, &proj, pos);
        let primary = PointerButton::Primary;
        let mut consumed = false;
        if self.gizmo_drag.is_none()
            && response.drag_started_by(primary)
            && let Some(origin) = press
            && let Some(h) = on_handle(origin)
        {
            self.gizmo_drag = Some(transform::Drag::start(h, &g, &proj, origin));
        }
        let mut event = None;
        if let Some(drag) = &mut self.gizmo_drag {
            consumed = true;
            let snap = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
            let pointer = response
                .interact_pointer_pos()
                .or_else(|| ui.input(|i| i.pointer.latest_pos()));
            if let Some(pos) = pointer {
                drag.update(&proj, pos, snap);
            }
            if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                event = Some(GizmoEvent::Commit(drag.last));
                self.gizmo_drag = None;
            } else {
                event = Some(GizmoEvent::Drag(drag.last));
            }
        }
        // a click on a handle is not a pick
        if (response.clicked_by(primary) || response.double_clicked_by(primary))
            && press.and_then(on_handle).is_some()
        {
            consumed = true;
        }
        self.gizmo_hot = self
            .gizmo_drag
            .map(|d| d.handle)
            .or_else(|| response.hover_pos().and_then(on_handle));
        (event, consumed)
    }

    /// Shows the viewport in all the available space and handles input.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> ViewportResponse {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let aspect = aspect_of(rect);
        let before = self.camera;

        if self.pending_fit && rect.width() > 1.0 && rect.height() > 1.0 {
            self.pending_fit = false;
            if let Some((lo, hi)) = self.bounds {
                self.camera.fit_box_in(
                    lo,
                    hi,
                    Vec2::new(rect.width(), rect.height()),
                    self.settings.fit_insets,
                    self.settings.fit_margin,
                );
                self.fitted_once = true;
            }
        }

        let gizmo_center = self
            .settings
            .gizmo
            .map(|corner| corner.anchor(rect, gizmo::RADIUS));
        let (gizmo_event, gizmo_consumed) = self.gizmo_input(ui, rect, &response);
        let gizmo_hovered = self.gizmo_hot.is_some();
        let mut pick = None;
        self.handle_input(
            ui,
            rect,
            &response,
            aspect,
            gizmo_center,
            gizmo_consumed,
            &mut pick,
        );

        // hover picking, only when the pointer, the camera or the scene changed
        if self.settings.hover_pick && !gizmo_hovered {
            match response.hover_pos() {
                Some(p) => {
                    let key = (p, self.camera, self.scene_version);
                    if self.last_hover != Some(key) {
                        self.hovered = self.pick_at(rect, p);
                        self.last_hover = Some(key);
                    }
                }
                None => {
                    self.hovered = None;
                    self.last_hover = None;
                }
            }
        } else {
            self.hovered = None;
        }

        self.last_rect = rect;
        self.paint(ui, rect);
        if let Some(center) = gizmo_center
            && rect.height() > 4.0 * gizmo::RADIUS
        {
            gizmo::paint(
                ui.painter(),
                center,
                self.camera.rotation,
                response.hover_pos(),
            );
        }

        ViewportResponse {
            camera_changed: self.camera != before,
            response,
            pick,
            hovered: self.hovered,
            gizmo: gizmo_event,
            gizmo_hovered,
            gizmo_consumed,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_input(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        response: &egui::Response,
        aspect: f32,
        gizmo_center: Option<Pos2>,
        gizmo_consumed: bool,
        pick: &mut Option<Pick>,
    ) {
        let nav = self.navigation;
        let shift = ui.input(|i| i.modifiers.shift);
        let delta = response.drag_delta();
        if delta != egui::Vec2::ZERO {
            let pan = response.dragged_by(nav.pan_button)
                || (shift && response.dragged_by(nav.orbit_button));
            if pan {
                self.camera
                    .pan_pixels(Vec2::new(delta.x, delta.y), rect.height());
            } else if response.dragged_by(nav.orbit_button) {
                self.camera
                    .orbit(-delta.x * nav.orbit_speed, -delta.y * nav.orbit_speed);
            }
        }

        if let Some(hover) = response.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let mut factor = 1.0;
            if scroll != 0.0 {
                factor *= (-scroll * nav.zoom_speed).exp();
            }
            if zoom != 1.0 && zoom > 0.0 {
                factor /= zoom;
            }
            if factor != 1.0 {
                self.camera.zoom_at(factor, ndc_of(rect, hover), aspect);
            }
        }

        let gizmo_center = gizmo_center.filter(|_| rect.height() > 4.0 * gizmo::RADIUS);
        if response.clicked_by(PointerButton::Primary)
            && !gizmo_consumed
            && let Some(pos) = response.interact_pointer_pos()
        {
            match gizmo_center.and_then(|c| gizmo::hit(c, self.camera.rotation, pos)) {
                Some(view) => self.camera.set_view(view),
                None => *pick = self.pick_at(rect, pos),
            }
        }
        if response.double_clicked_by(PointerButton::Primary)
            && !gizmo_consumed
            && let Some(pos) = response.interact_pointer_pos()
        {
            // double click: orbit around the point under the cursor, or fit
            match self.pick_at(rect, pos) {
                Some(p) => {
                    let eye = self.camera.eye();
                    self.camera.target = p.point;
                    self.camera.distance = (eye - p.point).length().max(1e-3);
                    let dir = (p.point - eye).normalize_or_zero();
                    if dir != Vec3::ZERO {
                        let up = self.camera.up();
                        if dir.cross(up).length_squared() > 1e-8 {
                            self.camera.rotation = camera::look_rotation(dir, up);
                        }
                    }
                }
                None => self.pending_fit = true,
            }
        }
    }

    fn paint(&mut self, ui: &egui::Ui, rect: Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let max = self.max_texture_size.max(1);
        let size = [
            ((rect.width() * ppp).round() as u32).clamp(1, max),
            ((rect.height() * ppp).round() as u32).clamp(1, max),
        ];
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let targets = Arc::new(self.gpu.targets(size));
            let mut renderer = self.renderer.write();
            match self.texture {
                Some(id) => renderer.update_egui_texture_from_wgpu_texture(
                    &self.gpu.device,
                    &targets.sample_view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture = Some(renderer.register_native_texture(
                        &self.gpu.device,
                        &targets.sample_view,
                        wgpu::FilterMode::Linear,
                    ));
                }
            }
            self.targets = Some(targets);
            self.last_render = None;
        }
        let (Some(targets), Some(texture)) = (self.targets.clone(), self.texture) else {
            return;
        };

        let clear = self.settings.background.unwrap_or_else(|| {
            let c = egui::Rgba::from(ui.visuals().extreme_bg_color);
            Color::rgb(c.r(), c.g(), c.b())
        });
        let key = RenderKey {
            camera: self.camera,
            settings: self.settings,
            clear,
            size,
            scene_version: self.scene_version,
            highlight_version: self.highlight_version,
        };
        if self.last_render.as_ref() != Some(&key) && ui.is_rect_visible(rect) {
            let callback = self.frame_callback(targets, clear, aspect_of(rect), ppp, size);
            ui.painter()
                .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
            // a discarded pass never reaches the GPU: render again next pass
            if !ui.ctx().will_discard() {
                self.last_render = Some(key);
            }
        }
        ui.painter().image(
            texture,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    fn frame_callback(
        &self,
        targets: Arc<Targets>,
        clear: Color,
        aspect: f32,
        ppp: f32,
        size: [u32; 2],
    ) -> FrameCallback {
        let cam = &self.camera;
        let (near, far) = cam.clip_planes(self.bounding_sphere());
        let view = cam.view_matrix();
        let proj = cam.projection_matrix(aspect, near, far);
        let s = &self.settings;
        let back = s.backface_color;
        let camera = CameraUniform {
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            params: [
                f32::from(u8::from(cam.projection == Projection::Orthographic)),
                // about 2e-4 of the clip range in front of the surface
                2e-4,
                0.0,
                0.0,
            ],
            wire_color: s.wire_color.to_array(),
            back_color: back.to_array(),
            hover_color: s.outline_color.with_alpha(s.hover_tint).to_array(),
        };
        let backface = match s.backface {
            BackfaceMode::Show => 0,
            BackfaceMode::Tint => 1,
            BackfaceMode::Cull => 2,
        };
        let shown = |e: &&Entry| e.visible && e.opacity > 0.0;
        let items = self
            .entries
            .iter()
            .filter(shown)
            .map(|e| {
                if s.wireframe {
                    self.gpu.ensure_edges(&e.gpu, &e.mesh);
                }
                let center = e.local_box.map_or(Vec3::ZERO, |(lo, hi)| {
                    e.xform.transform_point3((lo + hi) * 0.5)
                });
                let transparent = e.opacity < 0.999 || e.colors.base.a < 0.999;
                let hovered = self.hover_ids.contains(&e.id) && !self.selected.contains(&e.id);
                DrawItem {
                    mesh: e.gpu.clone(),
                    uniform: render::mesh_uniform(
                        e.xform, view, &e.colors, e.opacity, backface, hovered,
                    ),
                    transparent,
                    wireframe: s.wireframe,
                    depth: view.transform_point3(center).z,
                }
            })
            .collect();
        let selected: Vec<&Entry> = self
            .entries
            .iter()
            .filter(shown)
            .filter(|e| self.selected.contains(&e.id))
            .collect();
        let outline = (!selected.is_empty()).then(|| {
            let view_proj = proj * view;
            let bbox = selected
                .iter()
                .map(|e| {
                    e.local_box
                        .map(|(lo, hi)| render::transform_box(lo, hi, e.xform))
                })
                .try_fold(None, |acc: Option<[f32; 4]>, b| {
                    let px = pixel_box(b?, view_proj, size)?;
                    Some(Some(match acc {
                        Some(a) => [
                            a[0].min(px[0]),
                            a[1].min(px[1]),
                            a[2].max(px[2]),
                            a[3].max(px[3]),
                        ],
                        None => px,
                    }))
                })
                .flatten();
            OutlinePass::new(
                selected.iter().map(|e| e.gpu.clone()).collect(),
                s.outline_color,
                s.outline_width * ppp,
                s.outline_hidden,
                bbox,
                size,
            )
        });
        FrameCallback {
            gpu: self.gpu.clone(),
            targets,
            camera,
            clear,
            items,
            outline,
        }
    }
}

/// The pixel box (`[min x, min y, max x, max y]`, y down) of the world box
/// `b` in a target of `size` pixels; `None` when a corner is behind the
/// camera (the caller then uses the whole target).
fn pixel_box(b: (Vec3, Vec3), view_proj: Mat4, size: [u32; 2]) -> Option<[f32; 4]> {
    let (lo, hi) = b;
    let mut out = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for i in 0..8 {
        let p = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let c = view_proj * p.extend(1.0);
        if c.w <= 1e-6 {
            return None;
        }
        let x = (c.x / c.w * 0.5 + 0.5) * size[0] as f32;
        let y = (0.5 - c.y / c.w * 0.5) * size[1] as f32;
        out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
    }
    Some(out)
}

impl Drop for Viewport {
    fn drop(&mut self) {
        if let Some(id) = self.texture.take() {
            self.renderer.write().free_texture(&id);
        }
    }
}

fn aspect_of(rect: Rect) -> f32 {
    rect.width().max(1.0) / rect.height().max(1.0)
}

/// Normalised device coordinates of `pos` in `rect` (y up).
fn ndc_of(rect: Rect, pos: Pos2) -> Vec2 {
    Vec2::new(
        (pos.x - rect.left()) / rect.width().max(1.0) * 2.0 - 1.0,
        1.0 - (pos.y - rect.top()) / rect.height().max(1.0) * 2.0,
    )
}

/// Same colouring without comparing large shared arrays element by element.
fn same_coloring(a: &MeshColoring, b: &MeshColoring) -> bool {
    match (a, b) {
        (MeshColoring::PerVertex(x), MeshColoring::PerVertex(y)) => Arc::ptr_eq(x, y) || x == y,
        (
            MeshColoring::Labels {
                labels: x,
                palette: p,
            },
            MeshColoring::Labels {
                labels: y,
                palette: q,
            },
        ) => p == q && (Arc::ptr_eq(x, y) || x == y),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::Pose;
    use dc_scene::LabelPalette;
    use glam::{DQuat, DVec3};

    #[test]
    fn ndc_corners() {
        let r = Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(200.0, 100.0));
        assert_eq!(ndc_of(r, r.left_top()), Vec2::new(-1.0, 1.0));
        assert_eq!(ndc_of(r, r.right_bottom()), Vec2::new(1.0, -1.0));
        assert_eq!(ndc_of(r, r.center()), Vec2::ZERO);
        assert_eq!(aspect_of(r), 2.0);
    }

    #[test]
    fn gizmo_corners_and_default() {
        let r = Rect::from_min_size(Pos2::new(10.0, 20.0), egui::vec2(200.0, 100.0));
        assert_eq!(Corner::TopLeft.anchor(r, 2.0), Pos2::new(20.0, 30.0));
        assert_eq!(Corner::TopRight.anchor(r, 2.0), Pos2::new(200.0, 30.0));
        assert_eq!(Corner::BottomLeft.anchor(r, 2.0), Pos2::new(20.0, 110.0));
        assert_eq!(Corner::BottomRight.anchor(r, 2.0), Pos2::new(200.0, 110.0));
        assert_eq!(ViewportSettings::default().gizmo, Some(Corner::BottomLeft));
    }

    #[test]
    fn scene_item_conversion() {
        let mut blobs = BlobStore::new();
        let mesh = TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]);
        let blob = blobs.insert(mesh);
        let item = SceneItem::Mesh {
            blob,
            coloring: MeshColoring::VertexLabels(LabelPalette::default()),
            xform: Pose::new(DVec3::new(1.0, 2.0, 3.0), DQuat::IDENTITY),
            pickable: false,
        };
        let vm = ViewMesh::from_scene_item(9, &item, &blobs).unwrap();
        assert_eq!(vm.id, 9);
        assert!(!vm.pickable);
        assert_eq!(
            vm.xform.transform_point3(Vec3::ZERO),
            Vec3::new(1.0, 2.0, 3.0)
        );
        let lines = SceneItem::Lines {
            segments: vec![],
            color: Color::BLACK,
            width: 1.0,
        };
        assert!(ViewMesh::from_scene_item(1, &lines, &blobs).is_none());
        let missing = SceneItem::mesh(dc_case::BlobRef::of_bytes(b"x"), Color::TOOTH);
        assert!(ViewMesh::from_scene_item(1, &missing, &blobs).is_none());
    }

    #[test]
    fn coloring_comparison_uses_identity_first() {
        let colors = Arc::new(vec![Color::WHITE; 4]);
        let a = MeshColoring::PerVertex(colors.clone());
        let b = MeshColoring::PerVertex(colors);
        assert!(same_coloring(&a, &b));
        assert!(!same_coloring(&a, &MeshColoring::Uniform(Color::WHITE)));
        assert!(same_coloring(
            &MeshColoring::Uniform(Color::TOOTH),
            &MeshColoring::Uniform(Color::TOOTH)
        ));
    }
}
