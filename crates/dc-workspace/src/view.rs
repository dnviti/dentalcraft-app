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
}

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

/// Meshes to show for `state`: the scans (in the world frame; upper and
/// lower arch per the toggles, the bite scan with both) and the module's
/// scene meshes.
pub(crate) fn scene_meshes(
    state: &CaseState,
    module: &dyn Module,
    upper: bool,
    lower: bool,
    opacity: f32,
    enamel: Color,
) -> Vec<ViewMesh> {
    let world = state.case.world_frame.from_case.to_mat4().as_mat4();
    let mut out = Vec::new();
    for scan in &state.case.scans {
        let shown = match scan.role {
            ScanRole::Upper => upper,
            ScanRole::Lower => lower,
            ScanRole::Bite => upper && lower,
        };
        let Some(mesh) = state.mesh(&scan.mesh) else {
            continue;
        };
        let (a, b) = scan.id.as_u64_pair();
        let mut vm = ViewMesh::new((a ^ b) >> 1, Arc::clone(mesh))
            .with_xform(world * scan.import_xform.to_mat4().as_mat4())
            .with_coloring(MeshColoring::Uniform(enamel))
            .with_opacity(opacity);
        vm.visible = shown;
        out.push(vm);
    }
    for (i, item) in module.scene(state).iter().enumerate() {
        if let Some(vm) = ViewMesh::from_scene_item((1 << 63) | i as u64, item, &state.blobs) {
            out.push(vm.with_opacity(opacity));
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

    fn sync_scene(&mut self, ui: &Ui) {
        let key = SceneKey {
            state: Arc::as_ptr(self.undo.current()) as usize,
            upper: self.timeline.upper,
            lower: self.timeline.lower,
            display: self.view.display,
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
        let enamel = scene_color(colors(ui).model_enamel);
        let meshes = scene_meshes(
            self.undo.current(),
            self.module.as_ref(),
            self.timeline.upper,
            self.timeline.lower,
            opacity,
            enamel,
        );
        self.view.mesh_count = meshes.iter().filter(|m| m.visible).count();
        if let Some(vp) = &mut self.view.viewport {
            vp.settings.wireframe = self.view.display == Display::Wireframe;
            vp.set_meshes(meshes);
        }
    }

    fn set_view(&mut self, choice: ViewChoice) {
        self.view.view = choice;
        let sv = choice.std_view(self.timeline.upper);
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
                .unwrap_or_else(|| self.view.view.std_view(self.timeline.upper))
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

        // legend (bottom left)
        legend(ui, inner);

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
        let view = match self.view.view.std_view(self.timeline.upper) {
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
