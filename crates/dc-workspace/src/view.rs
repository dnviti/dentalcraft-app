//! Centre area: view toolbar, 3D viewport (or a placeholder when headless)
//! and its overlays (caption chip, view cube, legend, scale bar, toasts).

use std::sync::Arc;

use dc_case::{CaseState, ScanRole};
use dc_module_api::{Module, NOT_AVAILABLE_KEY, Toast, ToastLevel};
use dc_scene::{Color, MeshColoring};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets;
use dc_viewport::{StdView, ViewMesh, Viewport};
use egui::{
    Align2, Color32, CornerRadius, Frame, Id, Margin, Pos2, Rect, Role, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use glam::{Quat, Vec3};

use crate::overlay::{self, ContactColors, Layers};
use crate::t;
use crate::timeline::TimelineModel;
use crate::workspace::Workspace;

/// Display mode of the meshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Display {
    #[default]
    Solid,
    Transparent,
    Wireframe,
}

/// Standard views offered by the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewChoice {
    #[default]
    Occlusal,
    Front,
    Left,
    Right,
    Back,
}

impl ViewChoice {
    const ALL: [ViewChoice; 5] = [
        ViewChoice::Occlusal,
        ViewChoice::Front,
        ViewChoice::Left,
        ViewChoice::Right,
        ViewChoice::Back,
    ];

    fn key(self) -> &'static str {
        match self {
            ViewChoice::Occlusal => "ws-view-occlusal",
            ViewChoice::Front => "ws-view-front",
            ViewChoice::Left => "ws-view-left",
            ViewChoice::Right => "ws-view-right",
            ViewChoice::Back => "ws-view-back",
        }
    }

    /// The dc-viewport view; the occlusal view looks at the upper arch from
    /// below unless only the lower arch is shown.
    fn std_view(self, upper_shown: bool) -> StdView {
        match self {
            ViewChoice::Occlusal if upper_shown => StdView::OcclusalUpper,
            ViewChoice::Occlusal => StdView::OcclusalLower,
            ViewChoice::Front => StdView::Front,
            ViewChoice::Left => StdView::Left,
            ViewChoice::Right => StdView::Right,
            ViewChoice::Back => StdView::Back,
        }
    }

    fn from_std(v: StdView) -> Option<Self> {
        match v {
            StdView::OcclusalUpper | StdView::OcclusalLower => Some(ViewChoice::Occlusal),
            StdView::Front => Some(ViewChoice::Front),
            StdView::Left => Some(ViewChoice::Left),
            StdView::Right => Some(ViewChoice::Right),
            StdView::Back => Some(ViewChoice::Back),
            StdView::Top => None,
        }
    }
}

/// What the meshes were last built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneKey {
    state: usize,
    upper: bool,
    lower: bool,
    display: Display,
    stage: u32,
    overlay0: bool,
    layers: Layers,
    occlusal: bool,
}

/// Contact map computed for one case state (by pointer).
type ContactCache = Option<(usize, Result<Arc<ContactColors>, String>)>;

/// Viewport and view options.
#[derive(Default)]
pub(crate) struct ViewState {
    pub viewport: Option<Viewport>,
    synced: Option<SceneKey>,
    pub display: Display,
    pub view: ViewChoice,
    /// Camera orientation shown by the view cube when there is no viewport.
    pub headless_view: Option<StdView>,
    /// Frame the meshes at the next frame.
    pub refit: bool,
    /// Meshes currently shown.
    pub mesh_count: usize,
    /// Layers over the scans (labels, seeds, contact map).
    pub layers: Layers,
    /// Contact map of the last state it was asked for.
    contacts: ContactCache,
    /// Scans of the last scene (a new set is framed).
    scan_ids: Vec<uuid::Uuid>,
    /// The meshes of the last scene (also without a viewport).
    pub meshes: Vec<ViewMesh>,
}

impl ViewState {
    pub fn with_viewport(render_state: &egui_wgpu::RenderState) -> Self {
        let mut vp = Viewport::new(render_state);
        // the workspace draws its own view cube (top right, per the design)
        vp.settings.gizmo = None;
        vp.set_view(StdView::OcclusalUpper);
        Self {
            viewport: Some(vp),
            ..Self::default()
        }
    }
}

fn scene_color(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::rgba(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    )
}

/// What a scene is built from.
pub(crate) struct SceneInput<'a> {
    pub state: &'a CaseState,
    pub module: &'a dyn Module,
    /// Upper arch shown.
    pub upper: bool,
    /// Lower arch shown.
    pub lower: bool,
    pub opacity: f32,
    pub enamel: Color,
    /// Stage shown on the timeline (0 = initial position).
    pub stage: u32,
    /// Ghost of the initial position over a later stage.
    pub overlay0: bool,
    pub layers: Layers,
    /// Contact map colours (drawn instead of the labels when given).
    pub contacts: Option<&'a ContactColors>,
    /// Occlusal view: only the arch looked at is shown (the other one would
    /// cover its occlusal surface).
    pub occlusal: bool,
}

/// Seed marker size (mm).
const SEED_RADIUS: f32 = 0.8;
/// Opacity of the stage 0 ghost.
const GHOST_OPACITY: f32 = 0.25;

/// Meshes to show: the scans in the world frame (upper and lower arch per
/// the toggles, the bite scan with both), coloured by tooth label or by the
/// contact map and posed at the stage shown, the seed markers and the
/// module's scene meshes.
pub(crate) fn scene_meshes(input: &SceneInput<'_>) -> Vec<ViewMesh> {
    let state = input.state;
    let world = state.case.world_frame.from_case.to_mat4().as_mat4();
    let al = overlay::aligners(state);
    let palette = overlay::palette();
    // in the occlusal view the antagonist hides the arch looked at
    let looks_at_upper = input.upper && state.case.scan(ScanRole::Upper).is_some();
    let (upper, lower) = if input.occlusal && looks_at_upper {
        (true, false)
    } else {
        (input.upper, input.lower)
    };
    let shown = |role: ScanRole| match role {
        ScanRole::Upper => upper,
        ScanRole::Lower => lower,
        ScanRole::Bite => upper && lower,
    };
    let posed_stage = al.and_then(|a| overlay::stage(a, input.stage).map(|s| (a, s)));
    let mut out = Vec::new();
    for scan in &state.case.scans {
        let Some(mesh) = state.mesh(&scan.mesh) else {
            continue;
        };
        let (a, b) = scan.id.as_u64_pair();
        let id = (a ^ b) >> 2;
        let xform = world * scan.import_xform.to_mat4().as_mat4();
        let labelled = overlay::scan_labels(state, scan.role, mesh);
        let coloring = match (input.contacts.and_then(|c| c.of(scan.role)), labelled) {
            (Some(c), _) if c.len() == mesh.vertex_count() => MeshColoring::PerVertex(c.clone()),
            (_, Some(lm)) if input.layers.labels => dc_viewport::label_coloring(mesh, lm, &palette)
                .unwrap_or(MeshColoring::Uniform(input.enamel)),
            _ => MeshColoring::Uniform(input.enamel),
        };
        let posed = match (posed_stage, labelled.and_then(|m| m.vertex_labels.as_ref())) {
            (Some((a, st)), Some(l)) if input.stage > 0 => {
                overlay::posed_mesh(mesh, l, &a.tooth_frames, st).map(Arc::new)
            }
            _ => None,
        };
        if posed.is_some() && input.overlay0 {
            let mut ghost = ViewMesh::new(id | (1 << 62), Arc::clone(mesh))
                .with_xform(xform)
                .with_coloring(MeshColoring::Uniform(input.enamel))
                .with_opacity(GHOST_OPACITY.min(input.opacity));
            ghost.visible = shown(scan.role);
            ghost.pickable = false;
            out.push(ghost);
        }
        let mut vm = ViewMesh::new(id, posed.unwrap_or_else(|| Arc::clone(mesh)))
            .with_xform(xform)
            .with_coloring(coloring)
            .with_opacity(input.opacity);
        vm.visible = shown(scan.role);
        out.push(vm);
    }
    if input.layers.seeds
        && let Some(a) = al
    {
        let seeds = a.seeds.iter().filter(|s| {
            shown(if s.fdi.is_upper() {
                ScanRole::Upper
            } else {
                ScanRole::Lower
            })
        });
        if let Some(m) = overlay::seed_markers(seeds, SEED_RADIUS) {
            let mut vm = ViewMesh::new(1 << 61, Arc::new(m))
                .with_xform(world)
                .with_coloring(MeshColoring::VertexLabels(palette));
            vm.pickable = false;
            out.push(vm);
        }
    }
    for (i, item) in input.module.scene(state).iter().enumerate() {
        if let Some(vm) = ViewMesh::from_scene_item((1 << 63) | i as u64, item, &state.blobs) {
            out.push(vm.with_opacity(input.opacity));
        }
    }
    out
}

impl Workspace {
    /// Command id of the "Import scans" menu entry, if the module has one.
    pub(crate) fn import_command_id(&self) -> Option<&'static str> {
        self.menu
            .command_ids()
            .into_iter()
            .find(|c| crate::import::is_import_command(c))
    }

    /// Contact map of the current state (computed once per state); `None`
    /// when the layer is off. A case without a map turns the layer off and
    /// says why.
    fn contact_map(&mut self) -> Option<Arc<ContactColors>> {
        if !self.view.layers.contacts {
            return None;
        }
        let state = self.undo.current();
        let ptr = Arc::as_ptr(state) as usize;
        if self.view.contacts.as_ref().is_none_or(|(p, _)| *p != ptr) {
            let r = overlay::contact_colors(state).map(Arc::new);
            self.view.contacts = Some((ptr, r));
        }
        match &self.view.contacts {
            Some((_, Ok(c))) => Some(Arc::clone(c)),
            Some((_, Err(e))) => {
                let detail = e.clone();
                self.view.layers.contacts = false;
                self.view.contacts = None;
                self.toast(
                    Toast::new(ToastLevel::Warning, "ws-toast-contacts-unavailable")
                        .with_detail(detail),
                );
                None
            }
            None => None,
        }
    }

    pub(crate) fn sync_scene(&mut self, ui: &Ui) {
        let enamel = scene_color(colors(ui).model_enamel);
        self.sync_scene_with(enamel);
    }

    /// Rebuilds the meshes when the state, the stage, the toggles or the
    /// view changed.
    pub(crate) fn sync_scene_with(&mut self, enamel: Color) {
        let contacts = self.contact_map();
        let key = SceneKey {
            state: Arc::as_ptr(self.undo.current()) as usize,
            upper: self.timeline.upper,
            lower: self.timeline.lower,
            display: self.view.display,
            stage: self.timeline.current,
            overlay0: self.timeline.overlay0,
            layers: self.view.layers,
            occlusal: self.view.view == ViewChoice::Occlusal,
        };
        if self.view.synced == Some(key) {
            return;
        }
        self.view.synced = Some(key);
        let opacity = if self.view.display == Display::Transparent {
            0.5
        } else {
            1.0
        };
        let state = Arc::clone(self.undo.current());
        let meshes = scene_meshes(&SceneInput {
            state: &state,
            module: self.module.as_ref(),
            upper: self.timeline.upper,
            lower: self.timeline.lower,
            opacity,
            enamel,
            stage: self.timeline.current,
            overlay0: self.timeline.overlay0,
            layers: self.view.layers,
            contacts: contacts.as_deref(),
            occlusal: key.occlusal,
        });
        self.view.mesh_count = meshes.iter().filter(|m| m.visible).count();
        // a new set of scans (import, demo, a command through the bridge) is framed
        let ids: Vec<uuid::Uuid> = state.case.scans.iter().map(|s| s.id).collect();
        let new_scans = ids != self.view.scan_ids && !ids.is_empty();
        self.view.scan_ids = ids;
        self.view.meshes = meshes.clone();
        if let Some(vp) = &mut self.view.viewport {
            vp.settings.wireframe = self.view.display == Display::Wireframe;
            vp.set_meshes(meshes);
            if new_scans {
                vp.fit();
            }
        }
    }

    /// The occlusal view looks at the upper arch: it is shown and exists
    /// (or the case has no lower arch either).
    fn upper_looked_at(&self) -> bool {
        let case = &self.undo.current().case;
        self.timeline.upper
            && (case.scan(ScanRole::Upper).is_some() || case.scan(ScanRole::Lower).is_none())
    }

    fn set_view(&mut self, choice: ViewChoice) {
        self.view.view = choice;
        let sv = choice.std_view(self.upper_looked_at());
        self.apply_std_view(sv);
    }

    fn apply_std_view(&mut self, sv: StdView) {
        if let Some(c) = ViewChoice::from_std(sv) {
            self.view.view = c;
        }
        match &mut self.view.viewport {
            Some(vp) => {
                vp.set_view(sv);
                vp.fit();
            }
            None => self.view.headless_view = Some(sv),
        }
    }

    fn camera_rotation(&self) -> Quat {
        match &self.view.viewport {
            Some(vp) => vp.camera().rotation,
            None => self
                .view
                .headless_view
                .unwrap_or_else(|| self.view.view.std_view(self.upper_looked_at()))
                .rotation(),
        }
    }

    /// Draws the centre area.
    pub(crate) fn center_ui(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        self.sync_scene(ui);
        let rect = ui.available_rect_before_wrap();
        let mut px_per_mm = None;
        match &mut self.view.viewport {
            Some(vp) => {
                vp.settings.background = Some(scene_color(c.bg_viewport));
                if std::mem::take(&mut self.view.refit) {
                    vp.fit();
                }
                // picks are ignored: tooth selection needs the segmentation
                let _ = vp.ui(ui);
                let half = vp.camera().half_height();
                if half > 0.0 {
                    px_per_mm = Some(rect.height() / (2.0 * half));
                }
            }
            None => {
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, c.bg_viewport);
                ui.allocate_rect(rect, Sense::hover());
            }
        }

        let inset = 16.0;
        let inner = rect.shrink(inset);
        // toolbar + caption (top left)
        let mut top = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        self.toolbar(&mut top);
        top.add_space(8.0);
        self.caption(&mut top);

        // view cube (top right)
        let cube_rect =
            Rect::from_min_size(pos2(inner.right() - 100.0, inner.top()), vec2(100.0, 100.0));
        if let Some(sv) = view_cube(ui, cube_rect, self.camera_rotation()) {
            self.apply_std_view(sv);
        }

        // empty state (centre)
        if self.undo.current().case.scans.is_empty() {
            self.empty_state(ui, rect);
        }

        // legend (bottom left): the contact ramp when the map is shown
        if self.view.layers.contacts {
            contact_legend(ui, inner);
        } else {
            legend(ui, inner);
        }

        // scale bar (bottom right)
        if let Some(px) = px_per_mm {
            scale_bar(ui, inner, px);
        }

        // toasts (bottom right, above the scale bar)
        self.toasts_ui(
            ui,
            Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - 40.0)),
        );
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let frame = Frame::new()
            .fill(c.bg_raised)
            .stroke(Stroke::new(1.0, c.border_subtle))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(4))
            .shadow(dc_ui::theme::shadow(
                &dc_ui::tokens::tokens().shadow.e1_popover,
            ));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for v in ViewChoice::ALL {
                    if text_segment(ui, &t(v.key()), self.view.view == v).clicked() {
                        self.set_view(v);
                    }
                }
                separator(ui);
                for (d, icon, key) in [
                    (Display::Solid, ic::CUBE, "ws-display-solid"),
                    (
                        Display::Transparent,
                        ic::CUBE_TRANSPARENT,
                        "ws-display-transparent",
                    ),
                    (Display::Wireframe, ic::POLYGON, "ws-display-wireframe"),
                ] {
                    if icon_toggle(ui, icon, &t(key), self.view.display == d).clicked() {
                        self.view.display = d;
                    }
                }
                separator(ui);
                for (icon, key) in [
                    (ic::RULER, "ws-tool-measure"),
                    (ic::ANGLE, "ws-tool-angle"),
                    (ic::SPLIT_HORIZONTAL, "ws-tool-section"),
                ] {
                    if icon_toggle(ui, icon, &t(key), false).clicked() {
                        self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                    }
                }
                separator(ui);
                let layers = &mut self.view.layers;
                for (icon, key, on) in [
                    (ic::TOOTH, "ws-layer-labels", &mut layers.labels),
                    (ic::MAP_PIN, "ws-layer-seeds", &mut layers.seeds),
                    (ic::GRID_NINE, "ws-layer-contacts", &mut layers.contacts),
                ] {
                    if icon_toggle(ui, icon, &t(key), *on).clicked() {
                        *on = !*on;
                    }
                }
                separator(ui);
                for (icon, key) in [
                    (ic::SQUARE, "ws-layer-attachments"),
                    (ic::DIAMOND, "ws-layer-ipr"),
                    (ic::CIRCLE_DASHED, "ws-layer-target"),
                ] {
                    if icon_toggle(ui, icon, &t(key), false).clicked() {
                        self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                    }
                }
                separator(ui);
                if icon_toggle(ui, ic::CORNERS_OUT, &t("ws-fit"), false).clicked() {
                    self.view.refit = true;
                }
            });
        });
    }

    /// Text of the viewport caption chip.
    pub(crate) fn caption_text(&self) -> String {
        let jaw = match (self.timeline.upper, self.timeline.lower) {
            (true, true) => t("ws-jaw-both"),
            (true, false) => t("ws-jaw-upper"),
            (false, true) => t("ws-jaw-lower"),
            (false, false) => t("ws-jaw-none"),
        };
        let view = match self.view.view.std_view(self.upper_looked_at()) {
            StdView::OcclusalUpper => t("ws-caption-occlusal-upper"),
            StdView::OcclusalLower => t("ws-caption-occlusal-lower"),
            StdView::Front => t("ws-caption-front"),
            StdView::Left => t("ws-caption-left"),
            StdView::Right => t("ws-caption-right"),
            StdView::Back | StdView::Top => t("ws-caption-back"),
        };
        let mut s = format!("{jaw}, {view}");
        if !TimelineModel::from_state(self.undo.current()).is_empty() {
            s.push_str(", ");
            s.push_str(&dc_i18n::tr_args(
                "ws-caption-stage",
                &[("stage", &format!("{:02}", self.timeline.current))],
            ));
        }
        s
    }

    fn caption(&self, ui: &mut Ui) {
        let c = *colors(ui);
        Frame::new()
            .fill(c.bg_panel.gamma_multiply(0.9))
            .stroke(Stroke::new(1.0, c.border_subtle))
            .corner_radius(CornerRadius::same(255))
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(
                        egui::RichText::new(ic::EYE)
                            .font(fonts::icon(14.0))
                            .color(c.text_secondary),
                    );
                    ui.label(
                        egui::RichText::new(self.caption_text())
                            .font(fonts::medium(12.0))
                            .color(c.text_primary),
                    );
                });
            });
    }

    fn empty_state(&mut self, ui: &mut Ui, rect: Rect) {
        let c = *colors(ui);
        let card = Rect::from_center_size(rect.center(), vec2(360.0, 180.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(card)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let mut import = false;
        widgets::card_frame(&child).show(&mut child, |ui| {
            ui.set_width(320.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(ic::UPLOAD_SIMPLE)
                        .font(fonts::icon(40.0))
                        .color(c.text_secondary),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(t("ws-empty-title"))
                        .font(fonts::semibold(18.0))
                        .color(c.text_primary),
                );
                ui.label(
                    egui::RichText::new(t("ws-empty-body"))
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
                if self.view.viewport.is_none() {
                    ui.label(
                        egui::RichText::new(t("ws-empty-headless"))
                            .font(fonts::ui(12.0))
                            .color(c.text_disabled),
                    );
                }
                ui.add_space(10.0);
                if widgets::Button::primary(t("ws-import-scans"))
                    .icon(ic::UPLOAD_SIMPLE)
                    .show(ui)
                    .clicked()
                {
                    import = true;
                }
            });
        });
        if import {
            match self.import_command_id() {
                Some(id) => self.activate(id),
                None => self.activate(crate::import::IMPORT_SCANS_SUFFIX),
            }
        }
    }

    fn toasts_ui(&mut self, ui: &mut Ui, area: Rect) {
        let c = *colors(ui);
        let now = ui.input(|i| i.time);
        self.toasts
            .retain(|t| t.toast.level == ToastLevel::Error || t.born.is_none_or(|b| now - b < 6.0));
        let mut y = area.bottom();
        let mut close = None;
        let width = 340.0_f32.min(area.width());
        for (i, st) in self.toasts.iter_mut().enumerate().rev().take(4) {
            st.born.get_or_insert(now);
            let tone = match st.toast.level {
                ToastLevel::Info => c.select_blue,
                ToastLevel::Success => c.status_success,
                ToastLevel::Warning => c.status_warning,
                ToastLevel::Error => c.status_danger,
            };
            let icon = match st.toast.level {
                ToastLevel::Info => ic::INFO,
                ToastLevel::Success => ic::CHECK_CIRCLE,
                ToastLevel::Warning => ic::WARNING,
                ToastLevel::Error => ic::WARNING_CIRCLE,
            };
            let h = if st.toast.detail.is_some() {
                60.0
            } else {
                44.0
            };
            let r = Rect::from_min_size(pos2(area.right() - width, y - h), vec2(width, h));
            y -= h + 8.0;
            let p = ui.painter();
            p.add(egui::Shape::Rect(egui::epaint::RectShape::new(
                r,
                CornerRadius::same(6),
                c.bg_raised,
                Stroke::new(1.0, c.border_subtle),
                StrokeKind::Inside,
            )));
            p.rect_filled(
                Rect::from_min_size(r.min, vec2(3.0, h)),
                CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                },
                tone,
            );
            p.text(
                pos2(r.left() + 24.0, r.top() + 22.0),
                Align2::CENTER_CENTER,
                icon,
                fonts::icon(20.0),
                tone,
            );
            let title = t(&st.toast.message_key);
            p.text(
                pos2(r.left() + 44.0, r.top() + 22.0),
                Align2::LEFT_CENTER,
                &title,
                fonts::semibold(13.0),
                c.text_primary,
            );
            if let Some(d) = &st.toast.detail {
                let g = p.layout(
                    d.clone(),
                    fonts::ui(12.0),
                    c.text_secondary,
                    r.width() - 80.0,
                );
                p.with_clip_rect(r).galley(
                    pos2(r.left() + 44.0, r.top() + 34.0),
                    g,
                    c.text_secondary,
                );
            }
            let x_rect =
                Rect::from_center_size(pos2(r.right() - 18.0, r.top() + 22.0), Vec2::splat(24.0));
            let resp = ui.interact(
                x_rect,
                Id::unique(("dc-workspace-toast", i)),
                Sense::click(),
            );
            resp.widget_info(|| {
                WidgetInfo::labeled(Role::Button, true, format!("{} {title}", t("close")))
            });
            ui.painter().text(
                x_rect.center(),
                Align2::CENTER_CENTER,
                ic::X,
                fonts::icon(14.0),
                c.text_secondary,
            );
            if resp.clicked() {
                close = Some(i);
            }
            let body = ui.interact(
                r,
                Id::unique(("dc-workspace-toast-body", i)),
                Sense::hover(),
            );
            body.widget_info(|| WidgetInfo::labeled(Role::Alert, true, &title));
        }
        if let Some(i) = close {
            self.toasts.remove(i);
        }
        if !self.toasts.is_empty() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

fn separator(ui: &mut Ui) {
    let c = *colors(ui);
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 24.0), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, c.border_subtle));
}

fn text_segment(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let c = *colors(ui);
    let font = if selected {
        fonts::semibold(13.0)
    } else {
        fonts::medium(13.0)
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 32.0), Sense::click());
    let l = label.to_owned();
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, &l));
    let p = ui.painter();
    if selected {
        p.rect_filled(r, CornerRadius::same(4), c.bg_hover);
    } else if resp.hovered() {
        p.rect_filled(r, CornerRadius::same(4), c.bg_hover.gamma_multiply(0.6));
    }
    let color = if selected {
        c.text_primary
    } else {
        c.text_secondary
    };
    p.galley(r.center() - g.size() / 2.0, g, color);
    widgets::focus_ring(ui, &resp, r, CornerRadius::same(4));
    resp
}

fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> egui::Response {
    let c = *colors(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let tip = tooltip.to_owned();
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, on, &tip));
    let p = ui.painter();
    if on {
        p.rect(
            r,
            CornerRadius::same(4),
            c.select_soft,
            Stroke::new(1.0, c.select_blue),
            StrokeKind::Inside,
        );
    } else if resp.hovered() {
        p.rect_filled(r, CornerRadius::same(4), c.bg_hover);
    }
    let color = if on { c.select_blue } else { c.text_primary };
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        icon,
        fonts::icon(18.0),
        color,
    );
    widgets::focus_ring(ui, &resp, r, CornerRadius::same(4));
    resp.on_hover_text(tooltip)
}

/// A visible face of the view cube.
struct CubeFace {
    /// Camera-space depth of the face centre (drawn far to near).
    depth: f32,
    /// Screen corners.
    corners: [Pos2; 4],
    label: &'static str,
    /// View that looks at this face.
    view: StdView,
    /// How much the face faces the viewer on screen (with the display tilt).
    facing: f32,
    /// How much the face faces the real camera.
    aligned: f32,
}

/// Camera-aware view cube; returns the view of a clicked face.
fn view_cube(ui: &mut Ui, rect: Rect, rotation: Quat) -> Option<StdView> {
    let c = *colors(ui);
    let to_cam = rotation.inverse();
    // fixed display tilt so three faces are always visible (screen-up face
    // towards the viewer, then the screen-right one)
    let tilted = Quat::from_rotation_x(0.45) * Quat::from_rotation_y(-0.6) * to_cam;
    let center = rect.center();
    let scale = 24.0;
    let project = |p: Vec3| -> (Pos2, f32) {
        let q = tilted * p;
        (pos2(center.x + q.x * scale, center.y - q.y * scale), q.z)
    };
    // world faces: normal, label, view looking at that face
    let faces = [
        (Vec3::NEG_Y, "FRONT", StdView::Front),
        (Vec3::Y, "BACK", StdView::Back),
        (Vec3::NEG_X, "R", StdView::Right),
        (Vec3::X, "L", StdView::Left),
        (Vec3::NEG_Z, "OCC", StdView::OcclusalUpper),
        (Vec3::Z, "OCC L", StdView::OcclusalLower),
    ];
    let resp = ui.interact(rect, Id::unique("dc-workspace-view-cube"), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, t("ws-view-cube")));
    let hover = resp.hover_pos();
    let mut visible: Vec<CubeFace> = Vec::new();
    for (n, label, view) in faces {
        let facing = (tilted * n).z;
        let aligned = (to_cam * n).z;
        if facing <= 0.05 {
            continue;
        }
        let (u, v) = if n.x.abs() > 0.5 {
            (Vec3::Y, Vec3::Z)
        } else if n.y.abs() > 0.5 {
            (Vec3::X, Vec3::Z)
        } else {
            (Vec3::X, Vec3::Y)
        };
        let corners = [n - u - v, n + u - v, n + u + v, n - u + v].map(|p| project(p).0);
        let depth = project(n).1;
        visible.push(CubeFace {
            depth,
            corners,
            label,
            view,
            facing,
            aligned,
        });
    }
    visible.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    let mut clicked = None;
    let p = ui.painter();
    let best = visible.iter().map(|f| f.aligned).fold(f32::MIN, f32::max);
    for CubeFace {
        corners,
        label,
        view,
        facing,
        aligned,
        ..
    } in &visible
    {
        let inside = hover.is_some_and(|h| point_in_quad(h, corners));
        let fill = if inside {
            c.select_soft
        } else {
            c.bg_raised.gamma_multiply(0.92)
        };
        let stroke = if (*aligned - best).abs() < 1e-3 {
            Stroke::new(1.5, c.select_blue)
        } else {
            Stroke::new(1.0, c.border_strong)
        };
        p.add(egui::Shape::convex_polygon(corners.to_vec(), fill, stroke));
        let mid = corners.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / 4.0;
        if *facing > 0.35 {
            p.text(
                mid.to_pos2(),
                Align2::CENTER_CENTER,
                *label,
                fonts::mono(10.0),
                c.text_primary,
            );
        }
        if inside && resp.clicked() {
            clicked = Some(*view);
        }
    }
    clicked
}

fn point_in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let mut sign = 0.0_f32;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let cross = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if cross.abs() < f32::EPSILON {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

fn contact_legend(ui: &mut Ui, inner: Rect) {
    let c = *colors(ui);
    let rows = overlay::contact_legend();
    let h = 40.0 + rows.len() as f32 * 20.0;
    let r = Rect::from_min_size(pos2(inner.left(), inner.bottom() - h), vec2(236.0, h));
    let p = ui.painter();
    p.rect(
        r,
        CornerRadius::same(6),
        c.bg_panel.gamma_multiply(0.92),
        Stroke::new(1.0, c.border_subtle),
        StrokeKind::Inside,
    );
    p.text(
        pos2(r.left() + 12.0, r.top() + 16.0),
        Align2::LEFT_CENTER,
        t("ws-contacts-title"),
        fonts::semibold(12.0),
        c.text_primary,
    );
    for (k, (color, key, range)) in rows.iter().enumerate() {
        let y = r.top() + 40.0 + k as f32 * 20.0;
        let [rr, gg, bb, aa] = color.to_array();
        let fill = Color32::from(egui::Rgba::from_rgba_unmultiplied(rr, gg, bb, aa));
        p.rect_filled(
            Rect::from_center_size(pos2(r.left() + 20.0, y), vec2(14.0, 12.0)),
            CornerRadius::same(2),
            fill,
        );
        p.text(
            pos2(r.left() + 36.0, y),
            Align2::LEFT_CENTER,
            t(key),
            fonts::ui(12.0),
            c.text_primary,
        );
        p.text(
            pos2(r.right() - 10.0, y),
            Align2::RIGHT_CENTER,
            range,
            fonts::mono(11.0),
            c.text_secondary,
        );
    }
    let resp = ui.interact(r, Id::unique("dc-workspace-contact-legend"), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, t("ws-contacts-title")));
}

fn legend(ui: &mut Ui, inner: Rect) {
    let c = *colors(ui);
    let rows: [(&str, u8); 4] = [
        ("ws-legend-selected", 0),
        ("ws-legend-final", 1),
        ("ws-legend-attachment", 2),
        ("ws-legend-ipr", 3),
    ];
    let h = 16.0 + rows.len() as f32 * 24.0;
    let r = Rect::from_min_size(pos2(inner.left(), inner.bottom() - h), vec2(196.0, h));
    let p = ui.painter();
    p.rect(
        r,
        CornerRadius::same(6),
        c.bg_panel.gamma_multiply(0.92),
        Stroke::new(1.0, c.border_subtle),
        StrokeKind::Inside,
    );
    for (k, (key, shape)) in rows.iter().enumerate() {
        let y = r.top() + 20.0 + k as f32 * 24.0;
        let s = pos2(r.left() + 20.0, y);
        match shape {
            0 => {
                p.circle(s, 6.0, c.model_enamel, Stroke::new(2.0, c.select_blue));
            }
            1 => {
                let pts: Vec<Pos2> = (0..12)
                    .map(|i| {
                        let a = i as f32 / 12.0 * std::f32::consts::TAU;
                        s + vec2(a.cos(), a.sin()) * 6.0
                    })
                    .collect();
                for i in (0..12).step_by(2) {
                    p.line_segment(
                        [pts[i], pts[(i + 1) % 12]],
                        Stroke::new(1.5, c.text_primary),
                    );
                }
            }
            2 => {
                p.rect_filled(
                    Rect::from_center_size(s, vec2(14.0, 6.0)),
                    CornerRadius::same(1),
                    c.model_attachment,
                );
            }
            _ => {
                p.line_segment(
                    [s - vec2(0.0, 7.0), s + vec2(0.0, 7.0)],
                    Stroke::new(2.0, c.model_ipr),
                );
            }
        }
        p.text(
            pos2(r.left() + 38.0, y),
            Align2::LEFT_CENTER,
            t(key),
            fonts::ui(12.0),
            c.text_primary,
        );
    }
}

/// Nice scale bar length (mm) for `px_per_mm`, aiming at 60 to 160 px.
pub(crate) fn scale_length(px_per_mm: f32) -> f32 {
    [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0]
        .into_iter()
        .find(|l| l * px_per_mm >= 60.0)
        .unwrap_or(200.0)
}

fn scale_bar(ui: &mut Ui, inner: Rect, px_per_mm: f32) {
    let c = *colors(ui);
    let mm = scale_length(px_per_mm);
    let w = (mm * px_per_mm).min(inner.width() / 2.0);
    let y = inner.bottom() - 6.0;
    let x1 = inner.right();
    let x0 = x1 - w;
    let p = ui.painter();
    let stroke = Stroke::new(2.0, c.text_secondary);
    p.line_segment([pos2(x0, y), pos2(x1, y)], stroke);
    p.line_segment([pos2(x0, y - 4.0), pos2(x0, y + 1.0)], stroke);
    p.line_segment([pos2(x1, y - 4.0), pos2(x1, y + 1.0)], stroke);
    let text = if mm < 1.0 {
        format!("{mm:.1} mm")
    } else {
        format!("{mm:.0} mm")
    };
    p.text(
        pos2(x1, y - 8.0),
        Align2::RIGHT_BOTTOM,
        text,
        fonts::mono(11.0),
        c.text_secondary,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::overlay::tests::{exec, lower_demo};
    use dc_project::{AppInfo, Project};
    use dca_model::{AlignersState, Stage};
    use glam::DQuat;
    use serde_json::{Value, json};

    fn workspace(state: CaseState) -> Workspace {
        let project = Project::new(
            AppInfo {
                version: "0.1.0".into(),
                commit: "test".into(),
            },
            crate::now_rfc3339(),
            state,
        );
        Workspace::new_headless(
            Box::new(dca_module::AlignersModule::new()),
            Some(project),
            None,
        )
    }

    /// Two segmented demo arches in virtual occlusion with tooth frames and
    /// three stages; stage 2 moves tooth 31 by 1 mm.
    fn staged_case() -> CaseState {
        let s = exec(
            "aligners.demo",
            &lower_demo(),
            json!({ "arch": "upper", "seed": 11 }),
        );
        let s = exec("aligners.occlusion.virtual-bite", &s, Value::Null);
        let mut s = exec("aligners.teeth.frames", &s, Value::Null);
        let mut al = s
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.stages = (0..3).map(Stage::new).collect();
        al.stages[2].poses.insert(
            dc_case::Fdi::new(31).unwrap(),
            dc_case::Pose::new(glam::DVec3::X, DQuat::IDENTITY),
        );
        s.set_module(dca_model::MODULE_ID, dc_case::ModuleState::new(al));
        s
    }

    fn scan_mesh(ws: &Workspace, role: ScanRole) -> &ViewMesh {
        let st = ws.undo.current();
        let scan = st.case.scan(role).unwrap();
        let (a, b) = scan.id.as_u64_pair();
        let id = (a ^ b) >> 2;
        ws.view.meshes.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn scans_are_coloured_by_their_tooth_labels() {
        let mut ws = workspace(lower_demo());
        ws.sync_scene_with(Color::TOOTH);
        let vm = scan_mesh(&ws, ScanRole::Lower);
        let colors = dc_viewport::resolve_colors(&vm.mesh, &vm.coloring);
        let st = ws.undo.current();
        let al = overlay::aligners(st).unwrap();
        let labels = st.mesh(&al.labels[&ScanRole::Lower]).unwrap();
        let labels = labels.vertex_labels.as_ref().unwrap();
        let p = overlay::palette();
        assert!(labels.iter().zip(&colors).all(|(l, c)| p.color(*l) == *c));
        // seed markers: one octahedron per seed, coloured by tooth number
        let seeds = ws.view.meshes.iter().find(|m| m.id == 1 << 61).unwrap();
        assert_eq!(seeds.mesh.vertex_count(), al.seeds.len() * 6);
        assert!(!seeds.pickable);
        // the label layer off: the enamel colour
        ws.view.layers.labels = false;
        ws.view.layers.seeds = false;
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(
            scan_mesh(&ws, ScanRole::Lower).coloring,
            MeshColoring::Uniform(Color::TOOTH)
        );
        assert!(ws.view.meshes.iter().all(|m| m.id != 1 << 61));
    }

    #[test]
    fn the_stage_shown_poses_the_teeth_and_the_contact_map_colours_them() {
        let mut ws = workspace(staged_case());
        ws.sync_scene_with(Color::TOOTH);
        let rest = Arc::clone(&scan_mesh(&ws, ScanRole::Lower).mesh);
        // stage 1 moves nothing; stage 2 moves tooth 31
        ws.timeline.current = 1;
        ws.sync_scene_with(Color::TOOTH);
        assert!(Arc::ptr_eq(&scan_mesh(&ws, ScanRole::Lower).mesh, &rest));
        ws.timeline.current = 2;
        ws.sync_scene_with(Color::TOOTH);
        let posed = Arc::clone(&scan_mesh(&ws, ScanRole::Lower).mesh);
        let moved = posed
            .positions
            .iter()
            .zip(&rest.positions)
            .filter(|(a, b)| a.distance(**b) > 0.9)
            .count();
        assert!(moved > 100, "{moved}");
        // with the stage 0 overlay, a ghost of the rest position is drawn
        ws.timeline.overlay0 = true;
        ws.sync_scene_with(Color::TOOTH);
        assert!(
            ws.view
                .meshes
                .iter()
                .any(|m| m.opacity < 0.5 && Arc::ptr_eq(&m.mesh, &rest))
        );
        // the contact map replaces the label colours on both arches
        ws.view.layers.contacts = true;
        ws.sync_scene_with(Color::TOOTH);
        for role in [ScanRole::Lower, ScanRole::Upper] {
            assert!(matches!(
                scan_mesh(&ws, role).coloring,
                MeshColoring::PerVertex(_)
            ));
        }
        assert!(ws.view.layers.contacts);
        // without an upper arch the map is not available: the layer turns off
        let mut one = workspace(lower_demo());
        one.view.layers.contacts = true;
        one.sync_scene_with(Color::TOOTH);
        assert!(!one.view.layers.contacts);
        assert!(
            one.toast_keys()
                .iter()
                .any(|k| k == "ws-toast-contacts-unavailable")
        );
    }

    #[test]
    fn view_presets_follow_the_world_frame_and_the_occlusal_view_shows_one_arch() {
        let mut ws = workspace(staged_case());
        for (choice, sv) in [
            (ViewChoice::Front, StdView::Front),
            (ViewChoice::Left, StdView::Left),
            (ViewChoice::Right, StdView::Right),
            (ViewChoice::Back, StdView::Back),
            (ViewChoice::Occlusal, StdView::OcclusalUpper),
        ] {
            ws.set_view(choice);
            assert_eq!(ws.view.headless_view, Some(sv));
            assert_eq!(ws.camera_rotation(), sv.rotation());
        }
        // front: looking from the incisors (−Y) with the upper arch (+Z) up
        let (dir, up) = StdView::Front.direction_and_up();
        assert_eq!(dir, -dc_case::WorldFrame::ANTERIOR.as_vec3());
        assert_eq!(up, -dc_case::WorldFrame::TOWARDS_LOWER.as_vec3());
        // occlusal view of the upper arch: the lower arch would cover it
        ws.set_view(ViewChoice::Occlusal);
        ws.sync_scene_with(Color::TOOTH);
        assert!(scan_mesh(&ws, ScanRole::Upper).visible);
        assert!(!scan_mesh(&ws, ScanRole::Lower).visible);
        // only the lower arch shown: the occlusal view looks at it from above
        ws.timeline.upper = false;
        ws.set_view(ViewChoice::Occlusal);
        assert_eq!(ws.view.headless_view, Some(StdView::OcclusalLower));
        ws.sync_scene_with(Color::TOOTH);
        assert!(scan_mesh(&ws, ScanRole::Lower).visible);
        assert!(!scan_mesh(&ws, ScanRole::Upper).visible);
        // the front view shows both arches
        ws.timeline.upper = true;
        ws.set_view(ViewChoice::Front);
        ws.sync_scene_with(Color::TOOTH);
        assert!(scan_mesh(&ws, ScanRole::Lower).visible && scan_mesh(&ws, ScanRole::Upper).visible);
    }

    #[test]
    fn commands_run_from_outside_refresh_the_scene() {
        // the --control bridge runs case commands through Workspace::run_command
        let run = |ws: &mut Workspace, id: &str, params: Value| {
            let spec = dca_module::commands::find(id).unwrap();
            ws.run_command(Box::new(spec.command(params)));
            assert!(ws.wait_for_jobs(std::time::Duration::from_secs(60)));
        };
        let mut ws = workspace(CaseState::default());
        ws.sync_scene_with(Color::TOOTH);
        assert!(ws.view.meshes.is_empty());
        run(&mut ws, "aligners.demo", json!({ "arch": "lower" }));
        run(
            &mut ws,
            "aligners.demo",
            json!({ "arch": "upper", "seed": 11 }),
        );
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(ws.view.scan_ids.len(), 2);
        let before = scan_mesh(&ws, ScanRole::Upper).mesh.positions[0];
        assert!(matches!(
            scan_mesh(&ws, ScanRole::Upper).coloring,
            MeshColoring::Labels { .. }
        ));
        // the virtual bite moves the upper arch: the next frame shows it moved
        run(&mut ws, "aligners.occlusion.virtual-bite", Value::Null);
        ws.sync_scene_with(Color::TOOTH);
        let after = scan_mesh(&ws, ScanRole::Upper).mesh.positions[0];
        assert!(before.distance(after) > 1.0);
        assert_eq!(
            ws.view.synced.map(|k| k.state),
            Some(Arc::as_ptr(ws.undo.current()) as usize)
        );
    }

    #[test]
    fn the_stage_slider_is_bound_to_the_aligners_stages() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        struct App(Workspace);
        let mut h = Harness::builder()
            .with_size(vec2(1600.0, 1000.0))
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui_state(
                |ui, app: &mut App| {
                    let _ = app.0.show(ui);
                },
                App(workspace(staged_case())),
            );
        h.run();
        let model = TimelineModel::from_state(h.state().0.undo.current());
        assert_eq!(model.last(), 2);
        // the slider is the row of stage cells; a click picks the stage under it
        h.get_by_role(Role::Slider).click();
        h.run();
        assert_eq!(h.state().0.timeline.current, 1);
        h.get_by_label("Last stage").click();
        h.run();
        assert_eq!(h.state().0.timeline.current, 2);
        // the scene follows the slider: tooth 31 is posed
        let ws = &h.state().0;
        assert_eq!(ws.view.synced.map(|k| k.stage), Some(2));
        let caption = ws.caption_text();
        assert!(caption.contains("02"), "{caption}");
        // a new state with more stages (e.g. from the bridge) extends the slider
        let mut more = ws.undo.current().as_ref().clone();
        let mut al = more
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.stages.push(Stage::new(3));
        more.set_module(dca_model::MODULE_ID, dc_case::ModuleState::new(al));
        h.state_mut().0.undo.push(Arc::new(more), "test");
        h.run();
        h.get_by_label("Last stage").click();
        h.run();
        assert_eq!(h.state().0.timeline.current, 3);
        assert_eq!(h.state().0.view.synced.map(|k| k.stage), Some(3));
    }

    #[test]
    fn scale_lengths() {
        assert_eq!(scale_length(10.0), 10.0);
        assert_eq!(scale_length(100.0), 1.0);
        assert_eq!(scale_length(0.01), 200.0);
    }

    #[test]
    fn quad_hit_test() {
        let q = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(10.0, 10.0),
            pos2(0.0, 10.0),
        ];
        assert!(point_in_quad(pos2(5.0, 5.0), &q));
        assert!(!point_in_quad(pos2(15.0, 5.0), &q));
    }
}
