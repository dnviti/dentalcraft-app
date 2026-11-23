//! Centre area: view toolbar, 3D viewport (or a placeholder when headless)
//! and its overlays (caption chip, view cube, legend, scale bar, toasts).

use std::sync::Arc;
use std::time::Duration;

use dc_case::{CaseState, ScanRole};
use dc_module_api::{Module, NOT_AVAILABLE_KEY, Toast, ToastLevel};
use dc_scene::{Color, MeshColoring, SceneEntry, SceneItem};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets;
use dc_viewport::{FitInsets, StdView, ViewMesh, Viewport};
use egui::{
    Align2, Color32, CornerRadius, Frame, Id, Margin, Pos2, Rect, Role, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2, WidgetInfo, pos2, vec2,
};
use glam::{Quat, Vec3};

use crate::labels::{self, Anchor, Drawn, Gem, Hit};
use crate::manual::{self, Action};
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

    /// Name of the view for agents (`app.window.view`).
    fn name(self) -> &'static str {
        match self {
            ViewChoice::Occlusal => "occlusal",
            ViewChoice::Front => "front",
            ViewChoice::Left => "left",
            ViewChoice::Right => "right",
            ViewChoice::Back => "back",
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

/// What the viewport shows of an arch whose teeth were built: the case scan
/// or the closed teeth and the gingiva (the module's scene entries that
/// replace the scan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Model {
    Scan,
    #[default]
    Teeth,
}

impl Model {
    /// Name of the choice for agents (`app.window.view`).
    fn name(self) -> &'static str {
        match self {
            Model::Scan => "scan",
            Model::Teeth => "teeth",
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
    teeth: bool,
    /// The tooth whose region is shown on the scan (`region`).
    region: Option<u8>,
    /// The only scan shown ("Three points": the one the next point goes on).
    focus: Option<uuid::Uuid>,
    /// The study models are drawn ([`Workspace::study_shown`]).
    study: bool,
}

/// Contact map computed for one case state (by pointer).
type ContactCache = Option<(usize, Result<Arc<ContactColors>, String>)>;

/// Anchors of the tooth numbers computed for one case state (by pointer)
/// and for the 3D teeth or the scan.
type AnchorCache = Option<(usize, bool, Arc<Vec<Anchor>>)>;

/// Viewport and view options.
#[derive(Default)]
pub(crate) struct ViewState {
    pub viewport: Option<Viewport>,
    synced: Option<SceneKey>,
    pub display: Display,
    pub view: ViewChoice,
    /// Arches shown, whatever the view (a view preset only moves the camera).
    pub arches: Arches,
    /// Camera orientation shown by the view cube when there is no viewport.
    pub headless_view: Option<StdView>,
    /// Frame the meshes at the next frame.
    pub refit: bool,
    /// Meshes currently shown.
    pub mesh_count: usize,
    /// Layers over the scans (labels, seeds, contact map).
    pub layers: Layers,
    /// Scan or 3D teeth, as the user chose (see [`Workspace::teeth_shown`]).
    pub model: Model,
    /// Contact map of the last state it was asked for.
    contacts: ContactCache,
    /// Anchors of the tooth numbers for the case state (by pointer).
    anchors: AnchorCache,
    /// Scans of the last scene (a new set is framed).
    scan_ids: Vec<uuid::Uuid>,
    /// The scan shown alone in the last scene (a change is framed).
    focus: Option<uuid::Uuid>,
    /// The meshes of the last scene (also without a viewport).
    pub meshes: Vec<ViewMesh>,
    /// Counts the scenes built: the selection decorates the scene of this
    /// generation (`select`).
    pub generation: u64,
    /// The colour of the aligner of the view in the theme shown (the
    /// `model_aligner` token; the dark one until a frame sets it).
    pub aligner_color: Option<Color>,
}

/// Which arches are shown. Kept for the workspace session, apart from the
/// camera: choosing a view never changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Arches {
    pub upper: bool,
    pub lower: bool,
}

impl Default for Arches {
    fn default() -> Self {
        Self {
            upper: true,
            lower: true,
        }
    }
}

impl Arches {
    /// Whether the arch is shown.
    pub fn shown(self, upper: bool) -> bool {
        if upper { self.upper } else { self.lower }
    }

    /// The arches after a click on one of them. The last arch that can be
    /// seen stays on: `other_available` says whether the other one can be
    /// (it has a scan, or the case has none yet). Returns `false` when the
    /// click was refused.
    pub fn toggle(&mut self, upper: bool, other_available: bool) -> bool {
        if self.shown(upper) && !(self.shown(!upper) && other_available) {
            return false;
        }
        if upper {
            self.upper = !self.upper;
        } else {
            self.lower = !self.lower;
        }
        true
    }

    /// The arches after the scans of the case changed (`has_upper`,
    /// `has_lower`: the case holds a scan of that arch): when the only arch
    /// shown has no scan any more and the hidden one has, the hidden one is
    /// shown, so the view never goes empty (an arch scan removed while the
    /// other was hidden). Returns `true` when it changed.
    pub fn follow_scans(&mut self, has_upper: bool, has_lower: bool) -> bool {
        let (show_upper, show_lower) = match (self.upper, self.lower) {
            (true, false) => (false, !has_upper && has_lower),
            (false, true) => (!has_lower && has_upper, false),
            _ => (false, false),
        };
        self.upper |= show_upper;
        self.lower |= show_lower;
        show_upper || show_lower
    }
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

pub(crate) fn scene_color(c: Color32) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::rgba(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    )
}

/// The selection colour of the design (an sRGB token) as the linear colour
/// the viewport draws the outline with.
fn outline_color(c: Color32) -> Color {
    let l = egui::Rgba::from(c);
    Color::rgba(l.r(), l.g(), l.b(), 1.0)
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
    /// Colour of the ghost of the final position (`layers.target`).
    pub target: Color,
    /// Colour of the aligner of the view (`aligners.view.aligner`).
    pub aligner: Color,
    /// Stage shown on the timeline (0 = initial position).
    pub stage: u32,
    /// Ghost of the initial position over a later stage.
    pub overlay0: bool,
    pub layers: Layers,
    /// Contact map colours (drawn instead of the labels when given).
    pub contacts: Option<&'a ContactColors>,
    /// Draw the module's entries that replace the scan of their arch (the
    /// built teeth and gingiva) instead of it; `false`: the scan, and none of
    /// the entries of its arch but the aligner of the view, which replaces
    /// nothing.
    pub teeth: bool,
    /// Wait for the slow part of the module entries (the deformed gingiva at
    /// a stage); `false` (the UI thread): draw them at rest meanwhile and
    /// report the scene incomplete.
    pub wait: bool,
    /// The tooth whose region is shown: the scan of its arch is drawn with
    /// that tooth in its colour and the rest dimmed, every other labelled
    /// scan dimmed (`region::region_colors`), whatever the contact map says.
    pub region: Option<u8>,
    /// The only scan shown, alone (no other scan, mark or module entry):
    /// the scan "Three points" takes the next point on.
    pub focus: Option<uuid::Uuid>,
    /// Draw the study models (the "Study models" layer, when nothing needs
    /// the scans themselves): at the initial position, each in place of the
    /// scan, the 3D teeth and the gingiva of its arch.
    pub study: bool,
}

/// Most notifications the stack draws, those that stay until closed (failed
/// files, arch warnings) always included, even past this number.
const MAX_TOASTS_ON_SCREEN: usize = 4;

/// Seed marker size (mm).
const SEED_RADIUS: f32 = 0.8;
/// Delay before the scene is asked again while the module computes.
const SCENE_RETRY: Duration = Duration::from_millis(60);
/// Opacity of the stage 0 ghost and of the final position ghost.
const GHOST_OPACITY: f32 = 0.25;
/// Bits of the viewport mesh ids of the final position ghost (a scan or a
/// built tooth id with them is neither: no pick or selection names it).
pub(crate) const FINAL_GHOST: u64 = 3 << 62;

/// What the tooth numbers and the Alt+click need of the workspace.
struct TeethOnScreen<'a> {
    state: &'a CaseState,
    anchors: &'a [Anchor],
    /// Stage shown, when it moves the teeth (`None` = initial position).
    stage: Option<&'a dca_model::Stage>,
    layers: Layers,
    meshes: &'a [ViewMesh],
    /// The tooth the brush works on, while the brush is open.
    brush: Option<u8>,
}

/// The scan of the arch, or one of its built teeth, is among the meshes
/// shown. The numbers, the seed gems and the Alt+click of a hidden arch are
/// left out with it.
fn arch_mesh_shown(state: &CaseState, meshes: &[ViewMesh], upper: bool) -> bool {
    let role = if upper {
        ScanRole::Upper
    } else {
        ScanRole::Lower
    };
    let scan = state.case.scan(role).map(manual::scan_mesh_id);
    meshes.iter().filter(|m| m.visible).any(|m| {
        Some(m.id) == scan
            || manual::tooth_of_mesh(m.id).is_some_and(|f| labels::upper_tooth(f) == upper)
    })
}

/// Draws the number chips over the viewport and returns what an Alt+click
/// did (choosing the brush tooth), or [`Action::None`], and the tooth whose
/// chip or mark a plain click landed on (for "Delete a tooth").
///
/// Occlusion: a number (or a gem) is hidden when a ray from the camera to
/// its anchor meets the scan more than [`labels::OCCLUSION_TOLERANCE_MM`]
/// before it (`Viewport::pick_at` on the BVH of the scans, one ray per
/// tooth).
fn tooth_labels(
    ui: &Ui,
    vp: &Viewport,
    vr: &dc_viewport::ViewportResponse,
    s: &TeethOnScreen<'_>,
) -> (Action, Option<u8>) {
    let rect = vr.response.rect;
    let c = *colors(ui);
    let cam = vp.camera();
    let eye = cam.eye();
    let fwd = (cam.target - eye).normalize_or_zero();
    let arch_shown = |upper: bool| arch_mesh_shown(s.state, s.meshes, upper);
    // in front of the camera, and not behind the scan
    let visible = |p: Vec3, tol: f32| -> bool {
        if fwd.dot(p - eye) <= 0.0 {
            return false;
        }
        let hit = vp.pick_at(rect, vp.screen_pos(rect, p)).map(|h| h.point);
        !labels::covers(fwd, hit, p, tol)
    };
    let mut drawn = Drawn::default();
    if s.layers.numbers {
        let mut spots: Vec<(f32, u8, Pos2)> = Vec::new();
        for a in s.anchors.iter().filter(|a| arch_shown(a.upper)) {
            let p = match s.stage {
                Some(st) => labels::posed_point(s.state, st, a.fdi, a.world),
                None => a.world,
            };
            if visible(p, labels::OCCLUSION_TOLERANCE_MM) {
                spots.push((fwd.dot(p - eye), a.fdi, vp.screen_pos(rect, p)));
            }
        }
        // the nearest keep their place when two numbers meet
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        let spots: Vec<(u8, Pos2)> = spots.into_iter().map(|(_, f, p)| (f, p)).collect();
        drawn.chips = labels::place_chips(&spots, labels::CHIP_SIZE, labels::LIFT, rect);
    }
    if s.layers.ipr {
        let marks = labels::ipr_marks(s.state);
        let mut spots: Vec<(f32, u8, Pos2)> = Vec::new();
        for (i, m) in marks
            .iter()
            .enumerate()
            .filter(|(_, m)| arch_shown(m.upper))
        {
            let p = m.point(s.state, s.stage);
            if visible(p, labels::IPR_OCCLUSION_MM) {
                let i = u8::try_from(i).unwrap_or(u8::MAX);
                spots.push((fwd.dot(p - eye), i, vp.screen_pos(rect, p)));
            }
        }
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        let spots: Vec<(u8, Pos2)> = spots.into_iter().map(|(_, i, p)| (i, p)).collect();
        let chips = labels::place_chips(&spots, labels::IPR_CHIP_SIZE, labels::LIFT, rect);
        labels::paint_ipr(ui, rect, &spots, &chips, &marks);
    }
    if s.layers.seeds
        && let Some(al) = overlay::aligners(s.state)
    {
        let world = s.state.case.world_frame.from_case;
        for sd in al
            .seeds
            .iter()
            .filter(|sd| arch_shown(labels::upper_tooth(sd.fdi.get())))
        {
            let p = world.apply_point(sd.position).as_vec3();
            if visible(p, 2.0 * SEED_RADIUS) {
                drawn.gems.push(Gem {
                    fdi: sd.fdi.get(),
                    center: vp.screen_pos(rect, p),
                    radius: vp.screen_length(rect, p, SEED_RADIUS),
                });
            }
        }
    }
    let alt = ui.input(|i| i.modifiers.alt);
    let hot =
        alt.then(|| vr.response.hover_pos())
            .flatten()
            .and_then(
                |pos| match labels::hit_test(pos, &drawn.chips, &drawn.gems, None) {
                    Hit::Label(f) | Hit::Gem(f) => Some(f),
                    _ => None,
                },
            );
    labels::paint(
        ui,
        rect,
        &drawn.chips,
        c.border_strong,
        c.select_blue,
        s.brush,
        hot,
    );
    let click = vr
        .response
        .clicked_by(egui::PointerButton::Primary)
        .then(|| vr.response.interact_pointer_pos())
        .flatten();
    let Some(pos) = click else {
        return (Action::None, None);
    };
    if alt {
        // the scan under the pointer, or a built tooth
        let surface = vr.pick.and_then(|p| {
            manual::target_of(s.state, &p, eye)
                .map(|t| t.label)
                .or_else(|| manual::tooth_of_mesh(p.mesh_id).map(u16::from))
        });
        let hit = labels::hit_test(pos, &drawn.chips, &drawn.gems, surface);
        return (labels::alt_click_action(hit), None);
    }
    match labels::hit_test(pos, &drawn.chips, &drawn.gems, None) {
        Hit::Label(f) | Hit::Gem(f) => (Action::None, Some(f)),
        _ => (Action::None, None),
    }
}

/// `mesh` with only the triangles of the teeth (every corner labelled with a
/// tooth), for the ghost of the crowns of a scan.
fn crowns_only(mut mesh: dc_mesh::TriMesh, labels: &[u16]) -> dc_mesh::TriMesh {
    mesh.tris.retain(|t| {
        t.iter()
            .all(|&v| labels.get(v as usize).is_some_and(|&l| l != 0))
    });
    mesh.face_labels = None;
    mesh
}

/// Meshes to show: the scans in the world frame (upper and lower arch per
/// the toggles; a bite scan never, see below), coloured by tooth label or by
/// the contact map and posed at the stage shown, the seed markers and the
/// module's scene entries at that stage (`Module::scene_at`).
///
/// - A bite scan is purely functional (it only puts the arches in
///   occlusion): it is not in the scene at all, so it is neither drawn nor
///   pickable, unless it is the [`SceneInput::focus`] (the scan the next
///   point of "Three points" goes on).
/// - An entry of a hidden arch is not drawn nor pickable.
/// - With [`SceneInput::teeth`], a scan is left out while an entry that
///   replaces it (`hides_scan`) is shown for its arch; without it, the
///   entries of such an arch are left out and the scan is drawn. An arch
///   whose built teeth are out of date (teeth corrected by hand since they
///   were built, [`overlay::outdated_arches`]) is drawn as without it: its
///   scan, colours and tooth marks show the corrections, not the old teeth.
/// - A tooth entry gets the id [`manual::tooth_mesh_id`] of its FDI number,
///   so a pick on it names the tooth; with the labels layer off it takes the
///   enamel colour. Over a later stage with the stage 0 ghost, the teeth are
///   ghosted at the initial position too.
/// - The module's entries at a later stage are only asked for when something
///   of them is drawn: with the scans shown, the entries of an arch they
///   replace are left out, so the stage (and the deformation it costs) is
///   skipped when no other entry exists and no aligner is shown. `hides_scan`
///   and `arch` of an entry do not depend on the stage.
/// - The aligner of the view (`aligners.view.aligner`, while the case shows
///   it) replaces nothing: it is drawn over whatever its arch shows, the
///   scan, the 3D teeth or the study model, at the stage shown, translucent
///   and never picked (`crate::aligner`). Only a hidden arch, a scan shown
///   alone ([`SceneInput::focus`]) or the region of a tooth
///   ([`SceneInput::region`], at the initial position) leaves it out.
/// - With the target layer and a final position, whatever the stage, a
///   translucent ghost in [`SceneInput::target`] shows the teeth at their
///   final pose (the plugin's `ghost_final`): the built teeth that are
///   drawn, or the crowns of a scan, moved as a stage moves them
///   ([`overlay::tooth_move`]: about the centre of resistance of each
///   tooth). It is not pickable; its ids carry [`FINAL_GHOST`].
/// - With [`SceneInput::study`], at the initial position (stage 0), an arch
///   with a study model is drawn as its study model: its scan, 3D teeth and
///   gingiva are left out (the model overlaps them). Otherwise the study
///   models are not drawn.
///
/// The flag is `false` while the module still works on an entry (see
/// [`SceneInput::wait`]).
pub(crate) fn scene_meshes(input: &SceneInput<'_>) -> (Vec<ViewMesh>, bool) {
    let state = input.state;
    let world = state.case.world_frame.from_case.to_mat4().as_mat4();
    let al = overlay::aligners(state);
    let palette = overlay::palette();
    // the arches shown are the user's choice: no view changes them
    let (upper, lower) = (input.upper, input.lower);
    let shown = |role: ScanRole| match role {
        ScanRole::Upper => upper,
        ScanRole::Lower => lower,
        ScanRole::Bite => false,
    };
    let posed_stage = al.and_then(|a| overlay::stage(a, input.stage).map(|s| (a, s)));
    // the final position, for its ghost
    let at_final = al
        .filter(|_| input.layers.target)
        .and_then(overlay::final_stage);
    let final_stage = al.zip(at_final.as_ref());
    let rest = input.module.scene_at(state, None);
    // the arches with built teeth, and those the built teeth replace now
    let built: Vec<ScanRole> = rest
        .iter()
        .filter(|e| e.hides_scan)
        .filter_map(|e| e.arch)
        .collect();
    let outdated = overlay::outdated_arches(state);
    let replacing: Vec<ScanRole> = built
        .iter()
        .copied()
        .filter(|a| input.teeth && !outdated.contains(a))
        .collect();
    let keep = |e: &SceneEntry| {
        e.arch
            .is_none_or(|a| !built.contains(&a) || replacing.contains(&a))
    };
    // the arches drawn as their study model, and the entries of the models
    let study: Vec<ScanRole> = al
        .filter(|_| input.study && input.stage == 0 && input.focus.is_none())
        .map(|a| {
            a.study_models
                .iter()
                .filter(|(_, b)| state.blobs.contains(b))
                .map(|(r, _)| *r)
                .collect()
        })
        .unwrap_or_default();
    let is_study = |e: &SceneEntry| match &e.item {
        SceneItem::Mesh { blob, .. } => e
            .arch
            .is_some_and(|a| al.is_some_and(|d| d.study_models.get(&a) == Some(blob))),
        _ => false,
    };
    // the aligner of the view, drawn whatever the arch shows
    let is_aligner = |e: &SceneEntry| match &e.item {
        SceneItem::Mesh { blob, .. } => crate::aligner::is_view_aligner(al, blob),
        _ => false,
    };
    let aligner_shown =
        al.is_some_and(|a| a.aligner_view.shown && !a.aligner_view.bases.is_empty());
    let mut complete = true;
    let staged = aligner_shown || rest.iter().any(|e| keep(e) && !is_study(e));
    let entries = if input.stage == 0 || !staged {
        rest.clone()
    } else if input.wait {
        input.module.scene_at(state, Some(input.stage))
    } else {
        let (entries, done) = input.module.scene_at_nowait(state, Some(input.stage));
        complete = done;
        entries
    };
    let mut out = Vec::new();
    for scan in &state.case.scans {
        // the bite scan is not a 3D object: only "Three points" shows it
        if scan.role == ScanRole::Bite && input.focus != Some(scan.id) {
            continue;
        }
        if replacing.contains(&scan.role) && shown(scan.role) && input.focus.is_none() {
            continue;
        }
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
        let coloring = match (
            input.region,
            labelled.and_then(|m| m.vertex_labels.as_ref()),
        ) {
            (Some(f), Some(l)) => {
                let own = (scan.role != ScanRole::Bite
                    && labels::upper_tooth(f) == (scan.role == ScanRole::Upper))
                    .then_some(f);
                MeshColoring::PerVertex(Arc::new(crate::region::region_colors(l, &palette, own)))
            }
            _ => coloring,
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
        // the crowns of the scan at the final position, at any stage
        if let (Some((a, fs)), Some(l)) =
            (final_stage, labelled.and_then(|m| m.vertex_labels.as_ref()))
            && scan.role != ScanRole::Bite
            && let Some(at_final) = overlay::posed_mesh(mesh, l, &a.tooth_frames, fs)
        {
            let mut ghost = ViewMesh::new(id | FINAL_GHOST, Arc::new(crowns_only(at_final, l)))
                .with_xform(xform)
                .with_coloring(MeshColoring::Uniform(input.target))
                .with_opacity(GHOST_OPACITY.min(input.opacity));
            ghost.visible = shown(scan.role) && input.focus.is_none();
            ghost.pickable = false;
            out.push(ghost);
        }
        // the study model of the arch is drawn in its place
        if study.contains(&scan.role) {
            continue;
        }
        let mut vm = ViewMesh::new(id, posed.unwrap_or_else(|| Arc::clone(mesh)))
            .with_xform(xform)
            .with_coloring(coloring)
            .with_opacity(input.opacity);
        vm.visible = match input.focus {
            Some(f) => f == scan.id,
            None => shown(scan.role),
        };
        out.push(vm);
    }
    if input.layers.seeds
        && let Some(a) = al
    {
        // The plugin hides the seeds once the teeth are built: no marker on
        // an arch whose scan the 3D teeth replace.
        let seeds = a.seeds.iter().filter(|s| {
            let role = if s.fdi.is_upper() {
                ScanRole::Upper
            } else {
                ScanRole::Lower
            };
            shown(role) && !replacing.contains(&role) && input.focus.is_none()
        });
        if let Some(m) = overlay::seed_markers(seeds, SEED_RADIUS) {
            let mut vm = ViewMesh::new(1 << 61, Arc::new(m))
                .with_xform(world)
                .with_coloring(MeshColoring::VertexLabels(palette));
            vm.pickable = false;
            out.push(vm);
        }
    }
    let entry_mesh = |i: usize, e: &SceneEntry| -> Option<ViewMesh> {
        let SceneItem::Mesh {
            blob,
            coloring,
            xform,
            pickable,
        } = &e.item
        else {
            return None;
        };
        let mesh = e.resolve_mesh(|b| state.blobs.get(b))?;
        // a tooth or the gingiva of an arch has an id that names it (a pick
        // and the selection read it back)
        let gingiva = e
            .arch
            .filter(|a| e.tooth.is_none() && al.is_some_and(|d| d.gingiva.get(a) == Some(blob)));
        let id = match (e.tooth, gingiva) {
            (Some(f), _) => manual::tooth_mesh_id(f),
            (None, Some(a)) => crate::select::gingiva_mesh_id(a),
            (None, None) => (1 << 63) | i as u64,
        };
        let coloring = match e.tooth {
            Some(_) if !input.layers.labels => MeshColoring::Uniform(input.enamel),
            _ => coloring.clone(),
        };
        let mut vm = ViewMesh::new(id, Arc::clone(mesh))
            .with_xform(world * xform.to_mat4().as_mat4())
            .with_coloring(coloring)
            .with_opacity(input.opacity);
        vm.visible = e.arch.is_none_or(shown) && input.focus.is_none();
        vm.pickable = *pickable && vm.visible;
        // the aligner of the view: translucent, in its colour, never picked
        if crate::aligner::is_view_aligner(al, blob) {
            vm.coloring = MeshColoring::Uniform(input.aligner);
            vm.opacity = crate::aligner::ALIGNER_OPACITY.min(input.opacity);
            vm.pickable = false;
        }
        Some(vm)
    };
    if input.teeth && input.overlay0 && input.stage > 0 {
        for (i, e) in rest
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tooth.is_some() && keep(e))
        {
            if let Some(mut ghost) = entry_mesh(i, e) {
                ghost.id |= 1 << 62;
                ghost.coloring = MeshColoring::Uniform(input.enamel);
                ghost.opacity = GHOST_OPACITY.min(input.opacity);
                ghost.pickable = false;
                out.push(ghost);
            }
        }
    }
    // the built teeth at the final position, at any stage: moved as a stage
    // moves them (`overlay::tooth_move`, about the centre of resistance)
    if let Some((a, fs)) = final_stage.filter(|_| input.teeth) {
        for (i, e) in rest
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tooth.is_some() && keep(e))
        {
            let Some(mut ghost) = entry_mesh(i, e) else {
                continue;
            };
            let moved = e
                .tooth
                .and_then(|f| dc_case::Fdi::new(f).ok())
                .and_then(|f| {
                    Some(overlay::tooth_move(
                        a.tooth_frames.get(&f)?,
                        fs.poses.get(&f)?,
                    ))
                });
            if let Some(m) = moved {
                // the entry's own placement, then the move to the final position
                ghost.xform = world * m.to_mat4().as_mat4() * world.inverse() * ghost.xform;
            }
            ghost.id |= FINAL_GHOST;
            ghost.coloring = MeshColoring::Uniform(input.target);
            ghost.opacity = GHOST_OPACITY.min(input.opacity);
            ghost.pickable = false;
            out.push(ghost);
        }
    }
    for (i, e) in entries.iter().enumerate().filter(|(_, e)| {
        if is_study(e) {
            e.arch.is_some_and(|a| study.contains(&a))
        } else if is_aligner(e) {
            input.region.is_none()
        } else {
            keep(e) && e.arch.is_none_or(|a| !study.contains(&a))
        }
    }) {
        if let Some(vm) = entry_mesh(i, e) {
            out.push(vm);
        }
    }
    (out, complete)
}

impl Workspace {
    /// Command id of the "Import scans" menu entry, if the module has one.
    pub(crate) fn import_command_id(&self) -> Option<&'static str> {
        self.menu
            .command_ids()
            .into_iter()
            .find(|c| dc_module_api::is_import_scans_command(c))
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

    /// [`Workspace::sync_scene_with`] for the UI thread: the module's slow
    /// entries (the gingiva at a stage) are computed in the background and
    /// drawn at rest until then; the scene is rebuilt as they come in.
    pub(crate) fn sync_scene(&mut self, ui: &Ui) {
        let c = colors(ui);
        let (enamel, target) = (scene_color(c.model_enamel), scene_color(c.model_target));
        self.view.aligner_color = Some(scene_color(c.model_aligner));
        if !self.sync_scene_mode(enamel, target, false) {
            ui.ctx().request_repaint_after(SCENE_RETRY);
        }
    }

    /// Rebuilds the meshes when the state, the stage, the toggles or the
    /// view changed, waiting for the module's slow entries.
    #[cfg(test)]
    pub(crate) fn sync_scene_with(&mut self, enamel: Color) {
        let target = scene_color(dc_ui::tokens::tokens().dark.model_target);
        self.sync_scene_mode(enamel, target, true);
    }

    /// The layers drawn: the user's, with the final position ghosted while
    /// the arch line is shown (the plugin shows the teeth at their final
    /// position with it).
    fn layers_drawn(&self) -> Layers {
        let mut layers = self.view.layers;
        layers.target |= self.arch_line_shown();
        layers
    }

    /// Rebuilds the meshes when needed; `false` when the module still works
    /// on an entry (only without `wait`) and the scene is to be rebuilt.
    fn sync_scene_mode(&mut self, enamel: Color, target: Color, wait: bool) -> bool {
        let contacts = self.contact_map();
        // the arch shown lost its scan: the hidden one comes back
        let case = &self.undo.current().case;
        let has = |r| case.scan(r).is_some();
        let (upper, lower) = (has(ScanRole::Upper), has(ScanRole::Lower));
        self.view.arches.follow_scans(upper, lower);
        // the region of a tooth is shown on the scan, at the initial position,
        // and so are the points of "Three points" (they go on the scans at rest)
        let region = self.region_tooth();
        let focus = self.bite_points_focus();
        let (stage, overlay0) = if region.is_some() || focus.is_some() {
            (0, false)
        } else {
            (self.timeline.current, self.timeline.overlay0)
        };
        let key = SceneKey {
            state: Arc::as_ptr(self.undo.current()) as usize,
            upper: self.view.arches.upper,
            lower: self.view.arches.lower,
            display: self.view.display,
            stage,
            overlay0,
            layers: self.layers_drawn(),
            teeth: self.teeth_shown(),
            region,
            focus,
            study: self.study_shown(),
        };
        if self.view.synced == Some(key) {
            return true;
        }
        let opacity = if self.view.display == Display::Transparent {
            0.5
        } else {
            1.0
        };
        let state = Arc::clone(self.undo.current());
        let (meshes, complete) = scene_meshes(&SceneInput {
            state: &state,
            module: self.module.as_ref(),
            upper: self.view.arches.upper,
            lower: self.view.arches.lower,
            opacity,
            enamel,
            target,
            aligner: self
                .view
                .aligner_color
                .unwrap_or_else(|| scene_color(dc_ui::tokens::tokens().dark.model_aligner)),
            stage: key.stage,
            overlay0: key.overlay0,
            layers: key.layers,
            contacts: contacts.as_deref(),
            teeth: key.teeth,
            region: key.region,
            focus: key.focus,
            wait,
            study: key.study,
        });
        // an incomplete scene is built again at the next frame
        self.view.synced = complete.then_some(key);
        self.view.mesh_count = meshes.iter().filter(|m| m.visible).count();
        // a new set of scans (import, demo, a command through the bridge) is framed
        let ids: Vec<uuid::Uuid> = state.case.scans.iter().map(|s| s.id).collect();
        let new_scans = ids != self.view.scan_ids && !ids.is_empty();
        // the scan of the next point of "Three points" (and back) is framed
        let refocus = self.view.focus != key.focus;
        self.view.focus = key.focus;
        self.view.scan_ids = ids;
        self.view.meshes = meshes.clone();
        self.view.generation += 1;
        if let Some(vp) = &mut self.view.viewport {
            vp.settings.wireframe = self.view.display == Display::Wireframe;
            vp.set_meshes(meshes);
            if new_scans || refocus {
                vp.fit();
            }
        }
        complete
    }

    /// Stage shown on the timeline (`0` = initial position).
    pub fn stage_shown(&self) -> u32 {
        self.timeline.current
    }

    /// The scans themselves must be drawn: a tool that works on the scan is
    /// open (the brush, the clean-up and the seeds paint and read the scan
    /// labels, "Three points" picks on it), the contact map (of the scans)
    /// is on or a tooth region is shown ([`Workspace::region_tooth`]).
    fn scans_needed(&self) -> bool {
        let on_scan = matches!(
            self.manual.tool,
            Some(
                manual::Tool::Brush
                    | manual::Tool::Clean
                    | manual::Tool::Seeds
                    | manual::Tool::ThreePoints
            )
        );
        on_scan || self.view.layers.contacts || self.region_tooth().is_some()
    }

    /// The viewport draws the built teeth and gingiva instead of the scans
    /// they replace: the user chose [`Model::Teeth`] and nothing needs the
    /// scans ([`Workspace::scans_needed`]).
    pub fn teeth_shown(&self) -> bool {
        self.view.model == Model::Teeth && !self.scans_needed()
    }

    /// The viewport draws the study models (at the initial position, in
    /// place of the rest of their arch): the "Study models" layer is on and
    /// nothing needs the scans ([`Workspace::scans_needed`]).
    pub fn study_shown(&self) -> bool {
        self.view.layers.study && !self.scans_needed()
    }

    /// Anchors of the tooth numbers of the current state (computed once per
    /// state: the crown tops read every label of the scans), on the 3D teeth
    /// or on the scan, whichever is drawn.
    fn tooth_anchors(&mut self) -> Arc<Vec<Anchor>> {
        let built = self.teeth_shown();
        let state = self.undo.current();
        let ptr = Arc::as_ptr(state) as usize;
        match &self.view.anchors {
            Some((p, b, a)) if *p == ptr && *b == built => Arc::clone(a),
            _ => {
                let a = Arc::new(labels::anchors_of(state, built));
                self.view.anchors = Some((ptr, built, Arc::clone(&a)));
                a
            }
        }
    }

    /// The occlusal view looks at the upper arch: it is shown and exists
    /// (or the case has no lower arch either).
    fn upper_looked_at(&self) -> bool {
        let case = &self.undo.current().case;
        self.view.arches.upper
            && (case.scan(ScanRole::Upper).is_some() || case.scan(ScanRole::Lower).is_none())
    }

    /// The arch can be seen: it has a scan (or the case has none yet).
    fn arch_has_scan(&self, upper: bool) -> bool {
        let case = &self.undo.current().case;
        let role = if upper {
            ScanRole::Upper
        } else {
            ScanRole::Lower
        };
        case.scan(role).is_some() || case.scans.is_empty()
    }

    /// The arch can be hidden: the other one is shown and can be seen.
    fn can_hide_arch(&self, upper: bool) -> bool {
        self.view.arches.shown(!upper) && self.arch_has_scan(!upper)
    }

    /// Shows or hides an arch. The last arch that can be seen stays on. The
    /// occlusal camera follows the arch it looks at (from below at the upper
    /// one, from above at the lower one); every other view stays as it is.
    /// Returns whether the arch changed.
    pub(crate) fn toggle_arch(&mut self, upper: bool) -> bool {
        let changed = self.view.arches.toggle(upper, self.arch_has_scan(!upper));
        if changed {
            let rot = self.camera_rotation();
            let occlusal = [StdView::OcclusalUpper, StdView::OcclusalLower];
            if occlusal.iter().any(|v| v.rotation() == rot) {
                let sv = ViewChoice::Occlusal.std_view(self.upper_looked_at());
                if sv.rotation() != rot {
                    self.apply_std_view(sv);
                }
            }
        }
        changed
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
        // the selection: outline, tint, gizmo, and the change being dragged
        self.selection_follow_case();
        self.selection_to_viewport();
        let rect = ui.available_rect_before_wrap();
        let inset = OVERLAY_INSET;
        let inner = rect.shrink(inset);
        // the toolbar wraps into rows that stop short of the view cube
        // the measuring tools (not available yet), the layers of the marks
        // and the attachments, the legend and the scale bar are advanced
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        let toolbar_rows = toolbar_rows(ui, toolbar_max_width(inner.width()), advanced);
        let mut px_per_mm = None;
        let mut manual_events = Vec::new();
        let mut alt_action = Action::None;
        let mut sel_input = crate::select::SelInput::default();
        let anchors = self.tooth_anchors();
        let region_overlay = self.region_overlay();
        let line_shown = self.arch_line_shown();
        if line_shown {
            let state = Arc::clone(self.undo.current());
            self.archline.follow(&state);
        }
        match &mut self.view.viewport {
            Some(vp) => {
                vp.settings.background = Some(scene_color(c.bg_viewport));
                // a painting tool, or the selection (no tool open), reads the
                // mesh under the pointer
                vp.settings.hover_pick = self.manual.wants_hover() || self.manual.tool.is_none();
                vp.settings.outline_color = outline_color(c.select_blue);
                vp.settings.outline_width = dc_ui::tokens::tokens().stroke.selected_tooth;
                vp.settings.fit_insets = fit_insets(toolbar_height(toolbar_rows.len()));
                if std::mem::take(&mut self.view.refit) {
                    vp.fit();
                }
                // the clicks and strokes go to the manual tool that is open,
                // else to the selection and its gizmo
                let vr = vp.ui(ui);
                self.panels.layout.view = vr.response.rect;
                // the tooth numbers, the Alt+click that chooses the brush tooth
                // and the click on a number or a mark ("Delete a tooth")
                let mark_click;
                (alt_action, mark_click) = tooth_labels(
                    ui,
                    vp,
                    &vr,
                    &TeethOnScreen {
                        state: self.undo.current(),
                        anchors: &anchors,
                        stage: overlay::aligners(self.undo.current())
                            .and_then(|a| overlay::stage(a, self.timeline.current))
                            .filter(|_| self.timeline.current > 0),
                        layers: self.view.layers,
                        meshes: &self.view.meshes,
                        brush: (self.manual.tool == Some(manual::Tool::Brush))
                            .then_some(self.manual.tooth),
                    },
                );
                manual_events = crate::manual::collect(&mut self.manual, ui, vp, &vr, mark_click);
                if self.manual.tool.is_none() {
                    let alt = ui.input(|i| i.modifiers.alt);
                    let clicked = vr.response.clicked_by(egui::PointerButton::Primary)
                        && !vr.gizmo_consumed
                        && !alt;
                    sel_input = crate::select::SelInput {
                        pick: vr.pick.filter(|_| clicked && mark_click.is_none()),
                        hovered: vr.hovered.filter(|_| !vr.gizmo_hovered),
                        empty_click: clicked && vr.pick.is_none() && mark_click.is_none(),
                        chip: mark_click.filter(|_| clicked),
                        gizmo: vr.gizmo,
                        eye: vp.camera().eye(),
                    };
                }
                // the margin line of the tooth region, in front of the mesh
                if let Some((data, xform)) = &region_overlay {
                    crate::region::paint_margin(
                        ui,
                        vp,
                        vr.response.rect,
                        data,
                        *xform,
                        c.model_margin,
                    );
                }
                // the arch line of the final position and its points, in front
                if line_shown {
                    crate::archline::viewport(
                        &mut self.archline,
                        ui,
                        vp,
                        &vr,
                        &crate::archline::LineView {
                            state: self.undo.current(),
                            upper: self.view.arches.upper,
                            lower: self.view.arches.lower,
                            color: c.model_arch_line,
                            halo: Color32::from_black_alpha(160),
                        },
                    );
                }
                // the gizmo over the numbers
                vp.paint_transform_gizmo(ui.painter(), vr.response.hover_pos());
                let half = vp.camera().half_height();
                if half > 0.0 {
                    px_per_mm = Some(rect.height() / (2.0 * half));
                }
            }
            None => {
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, c.bg_viewport);
                self.panels.layout.view = ui.allocate_rect(rect, Sense::hover()).rect;
            }
        }

        self.manual_action(alt_action);
        self.manual_events(manual_events);
        self.selection_input(sel_input, Some(ui.ctx()));
        self.manual_feedback();

        // toolbar + caption (top left)
        let mut top = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        self.toolbar(&mut top, &toolbar_rows);
        top.add_space(8.0);
        self.caption(&mut top);

        // view cube (top right)
        let cube_rect = Rect::from_min_size(
            pos2(inner.right() - CUBE_SIZE, inner.top()),
            vec2(CUBE_SIZE, CUBE_SIZE),
        );
        if let Some(sv) = view_cube(ui, cube_rect, self.camera_rotation()) {
            self.apply_std_view(sv);
        }

        // empty state (centre)
        if self.undo.current().case.scans.is_empty() {
            self.empty_state(ui, rect);
        }

        // legend (bottom left): the contact ramp when the map is shown (it
        // stays while the map does, whatever the options)
        if self.view.layers.contacts {
            contact_legend(ui, inner);
        } else if advanced {
            legend(ui, inner);
        }

        // scale bar (bottom right)
        if let Some(px) = px_per_mm.filter(|_| advanced) {
            scale_bar(ui, inner, px);
        }

        // files held over the window: where they will go
        if self.confirm_exit.is_none() && ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
            drop_overlay(ui, rect);
        }

        // toasts (bottom right, above the scale bar)
        self.toasts_ui(
            ui,
            Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - 40.0)),
        );
    }

    fn toolbar(&mut self, ui: &mut Ui, rows: &[Vec<ToolbarItem>]) {
        let c = *colors(ui);
        let frame = Frame::new()
            .fill(c.bg_raised)
            .stroke(Stroke::new(1.0, c.border_subtle))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::same(TOOLBAR_MARGIN as i8))
            .shadow(dc_ui::theme::shadow(
                &dc_ui::tokens::tokens().shadow.e1_popover,
            ));
        frame.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = TOOLBAR_SPACING;
            for row in rows {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = TOOLBAR_SPACING;
                    for (i, item) in row.iter().enumerate() {
                        if i > 0 && row[i - 1].gap_to(*item) > TOOLBAR_SPACING {
                            separator(ui);
                        }
                        match item {
                            ToolbarItem::View(v) => {
                                if text_segment(
                                    ui,
                                    &t(v.key()),
                                    Role::RadioButton,
                                    self.view.view == *v,
                                )
                                .clicked()
                                {
                                    self.set_view(*v);
                                }
                            }
                            ToolbarItem::Group(g) => self.toolbar_group(ui, *g),
                        }
                    }
                });
            }
        });
    }

    fn toolbar_group(&mut self, ui: &mut Ui, group: ToolbarGroup) {
        match group {
            ToolbarGroup::Arch => {
                for upper in [true, false] {
                    let (label, tip) = if upper {
                        ("ws-arch-upper", "ws-arch-upper-tip")
                    } else {
                        ("ws-arch-lower", "ws-arch-lower-tip")
                    };
                    let on = self.view.arches.shown(upper);
                    let locked = on && !self.can_hide_arch(upper);
                    let mut tooltip = t(tip);
                    if locked {
                        tooltip = format!("{tooltip}. {}", t("ws-arch-keep"));
                    }
                    if text_segment(ui, &t(label), Role::CheckBox, on)
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        self.toggle_arch(upper);
                    }
                }
            }
            ToolbarGroup::Model => {
                let shown = if self.teeth_shown() {
                    Model::Teeth
                } else {
                    Model::Scan
                };
                let forced = self.view.model == Model::Teeth && shown == Model::Scan;
                // the arches whose built teeth are out of date show their scan
                let outdated = overlay::outdated_arch_names(self.undo.current())
                    .filter(|_| shown == Model::Teeth)
                    .map(|arches| dc_i18n::tr_args("ws-model-outdated", &[("arches", &arches)]));
                for (m, label, tip) in [
                    (Model::Scan, "ws-model-scan", "ws-model-scan-tip"),
                    (Model::Teeth, "ws-model-teeth", "ws-model-teeth-tip"),
                ] {
                    let mut tooltip = t(tip);
                    if forced {
                        tooltip = format!("{tooltip}. {}", t("ws-model-forced"));
                    }
                    if let Some(note) = &outdated {
                        tooltip = format!("{tooltip}. {note}");
                    }
                    if text_segment(ui, &t(label), Role::RadioButton, shown == m)
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        self.view.model = m;
                    }
                }
            }
            ToolbarGroup::Display => {
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
            }
            ToolbarGroup::Tools => {
                for (icon, key) in [
                    (ic::RULER, "ws-tool-measure"),
                    (ic::ANGLE, "ws-tool-angle"),
                    (ic::SPLIT_HORIZONTAL, "ws-tool-section"),
                ] {
                    if icon_toggle(ui, icon, &t(key), false).clicked() {
                        self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                    }
                }
            }
            ToolbarGroup::Layers => {
                let layers = &mut self.view.layers;
                for (icon, key, on) in [
                    (ic::TOOTH, "ws-layer-labels", &mut layers.labels),
                    (ic::MAP_PIN, "ws-layer-seeds", &mut layers.seeds),
                    (ic::HASH, "ws-layer-numbers", &mut layers.numbers),
                    (ic::GRID_NINE, "ws-layer-contacts", &mut layers.contacts),
                ] {
                    if icon_toggle(ui, icon, &t(key), *on).clicked() {
                        *on = !*on;
                    }
                }
            }
            ToolbarGroup::Marks => {
                // the attachment shapes are not modelled yet (ANO-001)
                if dc_module_api::advanced_ui(ui.ctx())
                    && icon_toggle(ui, ic::SQUARE, &t("ws-layer-attachments"), false).clicked()
                {
                    self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                }
                let on = self.view.layers.ipr;
                if icon_toggle(ui, ic::DIAMOND, &t("ws-layer-ipr"), on).clicked() {
                    self.toggle_ipr_layer();
                }
            }
            ToolbarGroup::Solids => {
                // whole models over the arches, named as a group for screen
                // readers
                ui.scope(|ui| {
                    let label = t("ws-toolbar-solids");
                    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                        node.set_role(egui::Role::Group);
                        node.set_label(label);
                    });
                    let on = self.view.layers.study;
                    if icon_toggle(ui, ic::CUBE_FOCUS, &t("ws-layer-study"), on).clicked() {
                        self.toggle_study_layer();
                    }
                    let on = self.view.layers.target;
                    if icon_toggle(ui, ic::CIRCLE_DASHED, &t("ws-layer-target"), on).clicked() {
                        self.toggle_target_layer();
                    }
                    // the aligner over the teeth, next to the final ghost as
                    // in the plugin (`ui.py:75-76`): its command builds it
                    let on = self.aligner_shown();
                    if icon_toggle(ui, ic::CUBE_TRANSPARENT, &t("ws-layer-aligner"), on).clicked() {
                        self.toggle_aligner_layer();
                    }
                    // the group's bounds: the three buttons (points, as egui
                    // gives its widgets)
                    let r = ui.min_rect();
                    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                        node.set_bounds(egui::accesskit::Rect {
                            x0: r.min.x.into(),
                            y0: r.min.y.into(),
                            x1: r.max.x.into(),
                            y1: r.max.y.into(),
                        });
                    });
                });
            }
            ToolbarGroup::Fit => {
                if icon_toggle(ui, ic::CORNERS_OUT, &t("ws-fit"), false).clicked() {
                    self.view.refit = true;
                }
            }
        }
    }

    /// The target layer on or off: the ghost of the teeth at their final
    /// position. A case without a final position keeps it off and says so.
    pub(crate) fn toggle_target_layer(&mut self) {
        let layers = &mut self.view.layers;
        if layers.target {
            layers.target = false;
            return;
        }
        let has_final =
            overlay::aligners(self.undo.current()).is_some_and(|a| !a.setup.poses.is_empty());
        if has_final {
            self.view.layers.target = true;
        } else {
            self.toast(Toast::new(ToastLevel::Info, "ws-toast-target-none"));
        }
    }

    /// The study model layer on or off: the study models of the case, each
    /// in place of the rest of its arch at the initial position. Turned on
    /// in a case without study models, it says how to make them (and stays
    /// on, ready for them).
    pub(crate) fn toggle_study_layer(&mut self) {
        let layers = &mut self.view.layers;
        layers.study = !layers.study;
        let none = overlay::aligners(self.undo.current()).is_none_or(|a| a.study_models.is_empty());
        if layers.study && none {
            self.toast(Toast::new(ToastLevel::Info, "ws-toast-study-none"));
        }
    }

    /// Sets the view of the window as the toolbar and the timeline do, for
    /// an agent connected to the app (`app.window.view`): `view` a standard
    /// view (`occlusal`, `front`, `left`, `right`, `back`), `stage` the
    /// stage shown (at most the last one; the playback stops), `study` and
    /// `target` the "Study models" and "Final position" layers (the final
    /// position stays off, with its notice, in a case without one), `model`
    /// the "Scan" or "3D teeth" choice (`scan`, `teeth`; a tool that works on
    /// the scan still shows the scan, see `teeth_shown`). What is `None`
    /// stays. Answers what the view shows then.
    ///
    /// # Errors
    /// An unknown view or model; nothing changes.
    pub fn set_window_view(
        &mut self,
        view: Option<&str>,
        stage: Option<u32>,
        study: Option<bool>,
        target: Option<bool>,
        model: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let choice = match view {
            None => None,
            Some(v) => Some(
                ViewChoice::ALL
                    .into_iter()
                    .find(|c| c.name() == v)
                    .ok_or_else(|| {
                        format!("unknown view {v}: occlusal, front, left, right or back")
                    })?,
            ),
        };
        let model = match model {
            None => None,
            Some(m) => Some(
                [Model::Scan, Model::Teeth]
                    .into_iter()
                    .find(|c| c.name() == m)
                    .ok_or_else(|| format!("unknown model {m}: scan or teeth"))?,
            ),
        };
        if let Some(c) = choice {
            self.set_view(c);
        }
        if let Some(m) = model {
            self.view.model = m;
        }
        let last = TimelineModel::from_state(self.undo.current()).last();
        if let Some(s) = stage {
            self.timeline.current = s.min(last);
            self.timeline.playing = false;
        }
        if study.is_some_and(|on| on != self.view.layers.study) {
            self.toggle_study_layer();
        }
        if target.is_some_and(|on| on != self.view.layers.target) {
            self.toggle_target_layer();
        }
        Ok(serde_json::json!({
            "view": self.view.view.name(),
            "stage": self.timeline.current,
            "last_stage": last,
            "study": self.view.layers.study,
            "target": self.view.layers.target,
            "model": self.view.model.name(),
            "study_shown": self.study_shown(),
            "teeth_shown": self.teeth_shown(),
        }))
    }

    /// The IPR layer on or off: a mark at each contact where the final
    /// position plans IPR. A case without IPR keeps it off and says so.
    pub(crate) fn toggle_ipr_layer(&mut self) {
        let layers = &mut self.view.layers;
        if layers.ipr {
            layers.ipr = false;
            return;
        }
        let planned =
            overlay::aligners(self.undo.current()).is_some_and(|a| !a.setup.ipr.is_empty());
        if planned {
            self.view.layers.ipr = true;
        } else {
            self.toast(Toast::new(ToastLevel::Info, "ws-toast-ipr-none"));
        }
    }

    /// Text of the viewport caption chip.
    pub(crate) fn caption_text(&self) -> String {
        let jaw = match (self.view.arches.upper, self.view.arches.lower) {
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
        let card = Rect::from_center_size(rect.center(), vec2(400.0, 180.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(card)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let mut import = false;
        let mut scan = false;
        let offers_scanner = self.offers_scanner();
        widgets::card_frame(&child).show(&mut child, |ui| {
            ui.set_width(360.0);
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
                // the files, and beside them the scan with a scanner (alpha)
                // when the module lets the scans come from one
                let import_label = t("ws-import-scans");
                let import_w = ui
                    .painter()
                    .layout_no_wrap(
                        import_label.clone(),
                        fonts::semibold(13.0),
                        Color32::PLACEHOLDER,
                    )
                    .size()
                    .x
                    + 16.0
                    + 8.0
                    + 32.0;
                let row = if offers_scanner {
                    import_w + 8.0 + dc_scan_ui::scan_button_width(ui)
                } else {
                    import_w
                };
                ui.allocate_ui_with_layout(
                    vec2(row, 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if widgets::Button::primary(import_label)
                            .icon(ic::UPLOAD_SIMPLE)
                            .show(ui)
                            .clicked()
                        {
                            import = true;
                        }
                        if offers_scanner {
                            scan = dc_scan_ui::scan_button(ui, true);
                        }
                    },
                );
            });
        });
        // the button of the empty state runs the import: the file dialog, with
        // its inspector (the role of the next files) on the right
        if import {
            match self.import_command_id() {
                Some(id) => {
                    self.select_command(id);
                    self.run_leaf(id);
                }
                None => self.run_leaf(dc_module_api::IMPORT_SCANS_SUFFIX),
            }
        }
        if scan {
            if let Some(id) = self.import_command_id() {
                self.select_command(id);
            }
            self.open_scan();
        }
    }

    fn toasts_ui(&mut self, ui: &mut Ui, area: Rect) {
        let c = *colors(ui);
        let now = ui.input(|i| i.time);
        self.toasts
            .retain(|t| t.stays() || t.born.is_none_or(|b| now - b < 6.0));
        // the newest few, and every one that stays until closed
        let shown = crate::workspace::visible_toasts(&self.toasts, MAX_TOASTS_ON_SCREEN);
        let mut y = area.bottom();
        let mut close = None;
        let width = 340.0_f32.min(area.width());
        for (i, st) in self.toasts.iter_mut().enumerate().rev() {
            if !shown.contains(&i) {
                continue;
            }
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
            // the detail wraps: the card grows with it (at most six lines)
            let detail = st.toast.detail.as_ref().map(|d| {
                ui.painter()
                    .layout(d.clone(), fonts::ui(12.0), c.text_secondary, width - 80.0)
            });
            let h = detail.as_ref().map_or(44.0, |g| {
                (34.0 + g.size().y.min(6.0 * 15.0) + 10.0).max(60.0)
            });
            if !st.stays() && y - h < area.top() {
                // no room left: only what stays until closed is never dropped
                continue;
            }
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
            if let Some(g) = detail {
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

/// Overlay of the viewport while files are held over the window: a dashed
/// blue frame on a faint blue wash, the way the drop zones of the forms
/// light up, with what to do.
fn drop_overlay(ui: &mut Ui, rect: Rect) {
    let c = *colors(ui);
    let frame = rect.shrink(8.0);
    let p = ui.painter();
    p.rect_filled(
        frame,
        CornerRadius::same(8),
        c.select_blue.gamma_multiply(0.10),
    );
    let corners = [
        frame.left_top(),
        frame.right_top(),
        frame.right_bottom(),
        frame.left_bottom(),
        frame.left_top(),
    ];
    p.extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(2.0, c.select_blue),
        6.0,
        6.0,
    ));
    let mid = frame.center();
    p.text(
        mid - vec2(0.0, 36.0),
        Align2::CENTER_CENTER,
        ic::UPLOAD_SIMPLE,
        fonts::icon(40.0),
        c.select_blue,
    );
    p.text(
        mid + vec2(0.0, 4.0),
        Align2::CENTER_CENTER,
        t("ws-drop-title"),
        fonts::semibold(18.0),
        c.text_primary,
    );
    p.text(
        mid + vec2(0.0, 30.0),
        Align2::CENTER_CENTER,
        t("ws-drop-body"),
        fonts::ui(13.0),
        c.text_secondary,
    );
}

/// Space between the overlays and the edges of the viewport (the design's
/// 16 pt inset).
const OVERLAY_INSET: f32 = 16.0;
/// Side of the view cube (top right).
const CUBE_SIZE: f32 = 100.0;
/// Free space kept between the toolbar and the view cube.
const CUBE_GAP: f32 = 8.0;
/// Inner margin of the toolbar frame.
const TOOLBAR_MARGIN: f32 = 4.0;
/// Space between the widgets of the toolbar, and between its rows.
const TOOLBAR_SPACING: f32 = 2.0;
/// Height of a toolbar button.
const TOOLBAR_BUTTON: f32 = 32.0;
/// Width of a toolbar separator (the widget, not the spacing round it).
const SEPARATOR_WIDTH: f32 = 9.0;
/// Breathing room kept between the framed scans and what floats over the view.
const FIT_MARGIN: f32 = 16.0;

/// A run of related toolbar widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarGroup {
    /// Upper / Lower: which arches are shown (not a view).
    Arch,
    /// Scan / 3D teeth: what is drawn of a built arch.
    Model,
    Display,
    Tools,
    Layers,
    Marks,
    /// Whole models over the arches: study models, final ghost, aligner.
    Solids,
    Fit,
}

/// What the toolbar breaks into rows: each view segment (a segment bar that
/// is too wide for the room breaks between segments) and each other group
/// (never split).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolbarItem {
    View(ViewChoice),
    Group(ToolbarGroup),
}

impl ToolbarItem {
    /// Every item, in the order of the toolbar; without `advanced` (the
    /// advanced options off) not the measuring tools nor the layers of the
    /// marks.
    fn all(advanced: bool) -> Vec<ToolbarItem> {
        let views = ViewChoice::ALL.iter().map(|v| ToolbarItem::View(*v));
        let groups = [
            ToolbarGroup::Arch,
            ToolbarGroup::Model,
            ToolbarGroup::Display,
            ToolbarGroup::Tools,
            ToolbarGroup::Layers,
            ToolbarGroup::Marks,
            ToolbarGroup::Solids,
            ToolbarGroup::Fit,
        ]
        .into_iter()
        .filter(|g| advanced || !matches!(g, ToolbarGroup::Tools | ToolbarGroup::Layers))
        .map(ToolbarItem::Group);
        views.chain(groups).collect()
    }

    /// Width of the item at most (a selected view segment is a little
    /// wider than the others).
    fn width(self, ui: &Ui, advanced: bool) -> f32 {
        let buttons =
            |n: usize| n as f32 * TOOLBAR_BUTTON + n.saturating_sub(1) as f32 * TOOLBAR_SPACING;
        match self {
            ToolbarItem::View(v) => text_segment_width(ui, &t(v.key())),
            ToolbarItem::Group(ToolbarGroup::Arch) => {
                text_segment_width(ui, &t("ws-arch-upper"))
                    + text_segment_width(ui, &t("ws-arch-lower"))
                    + TOOLBAR_SPACING
            }
            ToolbarItem::Group(ToolbarGroup::Model) => {
                text_segment_width(ui, &t("ws-model-scan"))
                    + text_segment_width(ui, &t("ws-model-teeth"))
                    + TOOLBAR_SPACING
            }
            ToolbarItem::Group(ToolbarGroup::Fit) => buttons(1),
            // labels, seeds, numbers, contacts
            ToolbarItem::Group(ToolbarGroup::Layers) => buttons(4),
            // attachments (advanced), IPR
            ToolbarItem::Group(ToolbarGroup::Marks) => buttons(if advanced { 2 } else { 1 }),
            // display: solid, transparent, wireframe; tools: measure, angle,
            // section plane; solids: study models, final ghost, aligner
            ToolbarItem::Group(_) => buttons(3),
        }
    }

    /// Space taken between `self` and the item after it: the segments of the
    /// views touch, everything else is parted by a separator.
    fn gap_to(self, next: ToolbarItem) -> f32 {
        match (self, next) {
            (ToolbarItem::View(_), ToolbarItem::View(_)) => TOOLBAR_SPACING,
            // a separator and the spacing on both of its sides
            _ => SEPARATOR_WIDTH + 2.0 * TOOLBAR_SPACING,
        }
    }
}

/// The most the toolbar frame can be wide in a viewport overlay area of
/// `inner_width`: what is left of it beside the view cube.
pub(crate) fn toolbar_max_width(inner_width: f32) -> f32 {
    (inner_width - CUBE_SIZE - CUBE_GAP).max(0.0)
}

/// The toolbar items packed into rows that fit in `max_width` (frame
/// included). An item wider than the room stays alone on its row.
fn toolbar_rows(ui: &Ui, max_width: f32, advanced: bool) -> Vec<Vec<ToolbarItem>> {
    let widths: Vec<(ToolbarItem, f32)> = ToolbarItem::all(advanced)
        .into_iter()
        .map(|i| (i, i.width(ui, advanced)))
        .collect();
    pack_toolbar_rows(&widths, max_width)
}

/// [`toolbar_rows`] on known item widths.
fn pack_toolbar_rows(widths: &[(ToolbarItem, f32)], max_width: f32) -> Vec<Vec<ToolbarItem>> {
    // frame margins and border on both sides
    let room = max_width - 2.0 * (TOOLBAR_MARGIN + 1.0);
    let mut rows: Vec<Vec<ToolbarItem>> = Vec::new();
    let mut used = 0.0;
    for &(item, w) in widths {
        let next_used = rows
            .last()
            .and_then(|row| row.last())
            .map(|prev| used + prev.gap_to(item) + w);
        match (rows.last_mut(), next_used) {
            (Some(row), Some(n)) if n <= room => {
                row.push(item);
                used = n;
            }
            _ => {
                rows.push(vec![item]);
                used = w;
            }
        }
    }
    rows
}

/// Height of the toolbar frame with `rows` rows.
pub(crate) fn toolbar_height(rows: usize) -> f32 {
    let rows = rows.max(1) as f32;
    rows * TOOLBAR_BUTTON + (rows - 1.0) * TOOLBAR_SPACING + 2.0 * (TOOLBAR_MARGIN + 1.0)
}

/// What the overlays cover of the viewport, for the framing of the scans:
/// the toolbar along the top (and the rows it wrapped into), plus a margin
/// all round. The caption, the cube and the legend sit in the corners, which
/// the bounding box of an arch leaves empty.
pub(crate) fn fit_insets(toolbar_height: f32) -> FitInsets {
    FitInsets {
        top: OVERLAY_INSET + toolbar_height + CUBE_GAP,
        ..FitInsets::NONE
    }
    .grown(FIT_MARGIN)
}

fn separator(ui: &mut Ui) {
    let c = *colors(ui);
    let (r, _) = ui.allocate_exact_size(vec2(SEPARATOR_WIDTH, 24.0), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, c.border_subtle));
}

/// Width of a view segment with this label (selected, the wider font).
fn text_segment_width(ui: &Ui, label: &str) -> f32 {
    let g = ui.painter().layout_no_wrap(
        label.to_owned(),
        fonts::semibold(13.0),
        Color32::PLACEHOLDER,
    );
    g.size().x + 20.0
}

fn text_segment(ui: &mut Ui, label: &str, role: Role, selected: bool) -> egui::Response {
    let c = *colors(ui);
    let font = if selected {
        fonts::semibold(13.0)
    } else {
        fonts::medium(13.0)
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, TOOLBAR_BUTTON), Sense::click());
    let l = label.to_owned();
    resp.widget_info(|| WidgetInfo::selected(role, true, selected, &l));
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
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(TOOLBAR_BUTTON), Sense::click());
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

    fn widths(view: f32) -> Vec<(ToolbarItem, f32)> {
        ToolbarItem::all(true)
            .into_iter()
            .map(|i| match i {
                ToolbarItem::View(_) => (i, view),
                ToolbarItem::Group(ToolbarGroup::Fit) => (i, 32.0),
                ToolbarItem::Group(_) => (i, 100.0),
            })
            .collect()
    }

    /// Width of a packed row, frame included.
    fn row_width(row: &[ToolbarItem], widths: &[(ToolbarItem, f32)]) -> f32 {
        let w = |i: &ToolbarItem| widths.iter().find(|(x, _)| x == i).unwrap().1;
        let items: f32 = row.iter().map(w).sum();
        let gaps: f32 = row.windows(2).map(|p| p[0].gap_to(p[1])).sum();
        items + gaps + 2.0 * (TOOLBAR_MARGIN + 1.0)
    }

    #[test]
    fn the_toolbar_breaks_into_rows_that_fit() {
        let w = widths(80.0);
        // room for everything: one row, every item once, in order
        let rows = pack_toolbar_rows(&w, 2000.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ToolbarItem::all(true));
        for room in [900.0, 600.0, 420.0, 312.0, 200.0, 130.0] {
            let rows = pack_toolbar_rows(&w, room);
            let flat: Vec<ToolbarItem> = rows.iter().flatten().copied().collect();
            assert_eq!(
                flat,
                ToolbarItem::all(true),
                "nothing lost or moved at {room}"
            );
            for row in &rows {
                assert!(
                    row.len() == 1 || row_width(row, &w) <= room,
                    "{room}: a row of {} items is {} wide",
                    row.len(),
                    row_width(row, &w)
                );
            }
        }
        // a longer language needs more rows, never fewer
        let narrow = pack_toolbar_rows(&w, 312.0).len();
        assert!(narrow > pack_toolbar_rows(&w, 900.0).len());
        assert!(toolbar_height(narrow) > toolbar_height(1));
    }

    #[test]
    fn the_advanced_options_off_leave_the_tools_and_the_marks_out_of_the_toolbar() {
        let simple = ToolbarItem::all(false);
        assert!(!simple.contains(&ToolbarItem::Group(ToolbarGroup::Tools)));
        assert!(!simple.contains(&ToolbarItem::Group(ToolbarGroup::Layers)));
        // the rest, in the same order
        let full: Vec<ToolbarItem> = ToolbarItem::all(true)
            .into_iter()
            .filter(|i| simple.contains(i))
            .collect();
        assert_eq!(full, simple);
        assert!(simple.contains(&ToolbarItem::Group(ToolbarGroup::Solids)));
        assert!(simple.contains(&ToolbarItem::Group(ToolbarGroup::Fit)));
    }

    #[test]
    fn the_toolbar_leaves_the_cube_its_corner() {
        // the cube is 100 wide at the top right of an area of 920
        assert_eq!(toolbar_max_width(920.0), 920.0 - 100.0 - 8.0);
        assert_eq!(toolbar_max_width(50.0), 0.0);
    }

    #[test]
    fn the_fit_keeps_clear_of_the_toolbar() {
        let one = fit_insets(toolbar_height(1));
        let two = fit_insets(toolbar_height(2));
        // below the toolbar (and its inset) plus a margin; room at the sides
        assert!(one.top >= OVERLAY_INSET + toolbar_height(1) + FIT_MARGIN);
        assert!(two.top > one.top);
        assert!(one.left >= FIT_MARGIN && one.right >= FIT_MARGIN && one.bottom >= FIT_MARGIN);
    }

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
    fn view_presets_follow_the_world_frame() {
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
    }

    fn shown(ws: &Workspace) -> (bool, bool) {
        (
            scan_mesh(ws, ScanRole::Upper).visible,
            scan_mesh(ws, ScanRole::Lower).visible,
        )
    }

    #[test]
    fn the_arch_buttons_are_not_a_view_and_no_view_overrides_them() {
        let mut ws = workspace(staged_case());
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(shown(&ws), (true, true));
        // every preset keeps both arches, the occlusal one too
        for choice in ViewChoice::ALL {
            ws.set_view(choice);
            ws.sync_scene_with(Color::TOOTH);
            assert_eq!(shown(&ws), (true, true), "{choice:?}");
        }
        // hide the upper arch in the front view: the camera stays
        ws.set_view(ViewChoice::Front);
        assert!(ws.toggle_arch(true));
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(shown(&ws), (false, true));
        assert_eq!(ws.camera_rotation(), StdView::Front.rotation());
        // the other presets leave the choice alone
        for choice in ViewChoice::ALL {
            ws.set_view(choice);
            ws.sync_scene_with(Color::TOOTH);
            assert_eq!(shown(&ws), (false, true), "{choice:?}");
        }
        // only the lower arch shown: the occlusal view looks at it from above
        assert_eq!(ws.view.headless_view, Some(StdView::Back));
        ws.set_view(ViewChoice::Occlusal);
        assert_eq!(ws.view.headless_view, Some(StdView::OcclusalLower));
        // the lower arch is the last one: it stays
        assert!(!ws.toggle_arch(false));
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(shown(&ws), (false, true));
        // the upper arch back: the occlusal camera follows it
        assert!(ws.toggle_arch(true));
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(shown(&ws), (true, true));
        assert_eq!(ws.view.headless_view, Some(StdView::OcclusalUpper));
        // and the lower one hidden: still the upper arch from below
        assert!(ws.toggle_arch(false));
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(shown(&ws), (true, false));
        assert_eq!(ws.view.headless_view, Some(StdView::OcclusalUpper));
    }

    #[test]
    fn the_last_arch_that_exists_cannot_be_hidden() {
        let mut arches = Arches::default();
        assert!(arches.toggle(true, true));
        assert_eq!((arches.upper, arches.lower), (false, true));
        // the lower one is the last shown
        assert!(!arches.toggle(false, true));
        assert!(arches.toggle(true, true));
        // the other arch is on but has no scan: it cannot replace this one
        assert!(!arches.toggle(true, false));
        assert_eq!((arches.upper, arches.lower), (true, true));
        // a case with the lower scan only: the lower arch stays
        let mut ws = workspace(lower_demo());
        assert!(!ws.toggle_arch(false));
        ws.sync_scene_with(Color::TOOTH);
        assert!(scan_mesh(&ws, ScanRole::Lower).visible);
    }

    #[test]
    fn a_hidden_arch_is_not_drawn_picked_numbered_or_seeded() {
        let mut ws = workspace(staged_case());
        ws.sync_scene_with(Color::TOOTH);
        let state = Arc::clone(ws.undo.current());
        let seed_vertices = |ws: &Workspace| {
            ws.view
                .meshes
                .iter()
                .find(|m| m.id == 1 << 61)
                .map_or(0, |m| m.mesh.vertex_count())
        };
        let both = seed_vertices(&ws);
        assert!(arch_mesh_shown(&state, &ws.view.meshes, true));
        assert!(arch_mesh_shown(&state, &ws.view.meshes, false));
        assert!(ws.toggle_arch(true));
        ws.sync_scene_with(Color::TOOTH);
        let upper = scan_mesh(&ws, ScanRole::Upper);
        // the viewport draws and picks only the visible meshes
        assert!(!upper.visible);
        assert!(!arch_mesh_shown(&state, &ws.view.meshes, true));
        assert!(arch_mesh_shown(&state, &ws.view.meshes, false));
        // the seeds of the hidden arch go with it (the demo seeds one tooth)
        assert!(both > 0 && seed_vertices(&ws) < both);
        let pickable = |ws: &Workspace| {
            ws.view
                .meshes
                .iter()
                .filter(|m| m.visible && m.pickable)
                .count()
        };
        assert_eq!(pickable(&ws), 1);
        // the stage 0 ghost follows its arch
        ws.timeline.overlay0 = true;
        ws.timeline.current = 1;
        ws.sync_scene_with(Color::TOOTH);
        assert!(
            ws.view
                .meshes
                .iter()
                .filter(|m| m.id & (1 << 62) != 0)
                .all(|m| m.visible == (m.id == (scan_mesh(&ws, ScanRole::Lower).id | (1 << 62))))
        );
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
        // a click at the middle of the track picks the middle stage
        h.get_by_role(Role::Slider).click();
        h.run();
        assert_eq!(h.state().0.timeline.current, 1);
        // dragging scrubs: the ends of the track are the first and last stage
        // and the scene follows
        let track = h.get_by_role(Role::Slider).rect();
        let end = egui::pos2(track.right() - 7.0, track.center().y);
        let start = egui::pos2(track.left() + 7.0, track.center().y);
        h.hover_at(end);
        h.drag_at(end);
        h.run();
        assert_eq!(h.state().0.timeline.current, 2);
        assert_eq!(h.state().0.view.synced.map(|k| k.stage), Some(2));
        h.hover_at(start);
        h.run();
        assert_eq!(h.state().0.timeline.current, 0);
        assert_eq!(h.state().0.view.synced.map(|k| k.stage), Some(0));
        h.drop_at(start);
        h.run();
        assert_eq!(h.state().0.timeline.current, 0);
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

#[cfg(test)]
mod built_teeth_tests {
    use super::*;
    use crate::overlay::tests::{exec, lower_demo};
    use dc_case::{Fdi, ModuleState, Pose};
    use dc_mesh::TriMesh;
    use dc_project::{AppInfo, Project};
    use dc_viewport::Pick;
    use dca_model::{AlignersState, Stage};
    use glam::DVec3;
    use serde_json::{Value, json};

    /// Two demo arches with tooth frames; tooth 31 is built (a closed mesh
    /// standing in for it) and stage 2 moves it by 1 mm along X.
    fn built_case() -> CaseState {
        let s = exec(
            "aligners.demo",
            &lower_demo(),
            json!({ "arch": "upper", "seed": 11 }),
        );
        let mut s = exec("aligners.teeth.frames", &s, Value::Null);
        let mut al = s
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        let tooth = TriMesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]);
        let f31 = Fdi::new(31).unwrap();
        al.crowns.insert(f31, s.blobs.insert(tooth));
        al.stages = (0..3).map(Stage::new).collect();
        al.stages[2]
            .poses
            .insert(f31, Pose::from_translation(DVec3::X));
        s.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        s
    }

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

    fn mesh(ws: &Workspace, id: u64) -> Option<&ViewMesh> {
        ws.view.meshes.iter().find(|m| m.id == id)
    }

    fn scan_id(ws: &Workspace, role: ScanRole) -> u64 {
        manual::scan_mesh_id(ws.undo.current().case.scan(role).unwrap())
    }

    #[test]
    fn built_teeth_replace_the_scan_of_their_arch() {
        let mut ws = workspace(built_case());
        ws.sync_scene_with(Color::TOOTH);
        let tooth = manual::tooth_mesh_id(31);
        let t = mesh(&ws, tooth).expect("the tooth mesh");
        assert!(t.visible && t.pickable);
        // the lower scan is replaced, the upper (no built tooth) stays
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_none());
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Upper)).is_some());
        // the seed markers are hidden over the 3D teeth (as in the plugin)...
        let markers = |ws: &Workspace, upper: bool| {
            let n = overlay::aligners(ws.undo.current())
                .unwrap()
                .seeds
                .iter()
                .filter(|s| s.fdi.is_upper() == upper)
                .count();
            (
                n,
                mesh(ws, 1 << 61).map_or(0, |m| m.mesh.vertex_count() / 6),
            )
        };
        // only the upper arch (scan drawn) keeps its markers
        let (up, drawn) = markers(&ws, true);
        let all = overlay::aligners(ws.undo.current()).unwrap().seeds.len();
        assert!(up > 0 && up < all);
        assert_eq!(drawn, up, "lower seeds hidden over built teeth");
        assert!(arch_mesh_shown(ws.undo.current(), &ws.view.meshes, false));
        // the toggle shows the scan instead, and none of the entries of its arch
        ws.view.model = Model::Scan;
        ws.sync_scene_with(Color::TOOTH);
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_some());
        assert!(mesh(&ws, tooth).is_none());
        // ...and drawn again in the Scan view
        assert_eq!(markers(&ws, true).1, all, "seeds in the scan view");
    }

    /// The meshes drawn with the colour of the study models.
    fn study_meshes(ws: &Workspace) -> Vec<&ViewMesh> {
        ws.view
            .meshes
            .iter()
            .filter(|m| m.coloring == MeshColoring::Uniform(dca_module::STUDY_MODEL_COLOR))
            .collect()
    }

    #[test]
    fn the_study_models_stand_for_their_arch_at_the_initial_position() {
        // tooth 31 built, a study model of each arch
        let mut state = built_case();
        let mut al = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        for (role, z) in [(ScanRole::Upper, 1.0), (ScanRole::Lower, -1.0)] {
            let cast = TriMesh::new(
                vec![Vec3::Z * z, Vec3::X + Vec3::Z * z, Vec3::Y],
                vec![[0, 1, 2]],
            );
            al.study_models.insert(role, state.blobs.insert(cast));
        }
        state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let mut ws = workspace(state);
        assert!(ws.view.layers.study, "the layer is on by default");
        ws.sync_scene_with(Color::TOOTH);
        // both arches are their study model: no scan, no 3D tooth
        let tooth = manual::tooth_mesh_id(31);
        assert_eq!(study_meshes(&ws).len(), 2);
        assert!(study_meshes(&ws).iter().all(|m| m.visible));
        for role in [ScanRole::Upper, ScanRole::Lower] {
            assert!(mesh(&ws, scan_id(&ws, role)).is_none(), "{role:?}");
        }
        assert!(mesh(&ws, tooth).is_none());
        // the scan view too
        ws.view.model = Model::Scan;
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(study_meshes(&ws).len(), 2);
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_none());
        ws.view.model = Model::Teeth;
        // a hidden arch hides its study model
        ws.view.arches.upper = false;
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(study_meshes(&ws).iter().filter(|m| m.visible).count(), 1);
        ws.view.arches.upper = true;
        // a later stage shows the teeth where they are then
        ws.timeline.current = 2;
        ws.sync_scene_with(Color::TOOTH);
        assert!(study_meshes(&ws).is_empty());
        assert!(mesh(&ws, tooth).is_some());
        ws.timeline.current = 0;
        // the brush works on the scan: the scan is drawn
        ws.manual.tool = Some(manual::Tool::Brush);
        assert!(!ws.study_shown());
        ws.sync_scene_with(Color::TOOTH);
        assert!(study_meshes(&ws).is_empty());
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_some());
        ws.manual.tool = None;
        // the layer off: the scans and the 3D teeth as before
        ws.toggle_study_layer();
        ws.sync_scene_with(Color::TOOTH);
        assert!(study_meshes(&ws).is_empty());
        assert!(mesh(&ws, tooth).is_some());
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Upper)).is_some());
        // on again: back, no notice (the case has study models)
        let toasts = ws.toasts.len();
        ws.toggle_study_layer();
        assert_eq!(ws.toasts.len(), toasts);
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(study_meshes(&ws).len(), 2);
    }

    #[test]
    fn the_study_layer_says_how_to_make_study_models_when_there_are_none() {
        let mut ws = workspace(built_case());
        ws.toggle_study_layer();
        assert!(!ws.view.layers.study);
        let toasts = ws.toasts.len();
        ws.toggle_study_layer();
        assert!(ws.view.layers.study);
        assert_eq!(ws.toasts.len(), toasts + 1);
        ws.sync_scene_with(Color::TOOTH);
        assert!(study_meshes(&ws).is_empty());
    }

    /// [`built_case`] (tooth 31 built: its 3D tooth replaces the lower scan)
    /// with a study model of the lower arch and, shown in the view, the
    /// aligner of the lower arch built at stages 0, 1 and 2 for the case as
    /// it is; with the meshes of those three aligners.
    fn aligner_case() -> (CaseState, Vec<dc_case::BlobRef>) {
        let mut state = built_case();
        let mut al = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        let cast = TriMesh::new(
            vec![-Vec3::Z, Vec3::X - Vec3::Z, Vec3::Y - Vec3::Z],
            vec![[0, 1, 2]],
        );
        al.study_models
            .insert(ScanRole::Lower, state.blobs.insert(cast));
        let key = dca_model::view_key(&state, &al);
        // a closed shell over the teeth, one higher at every stage
        let shells: Vec<dc_case::BlobRef> = (0..3u8)
            .map(|k| {
                let z = 0.5 + f32::from(k);
                state.blobs.insert(TriMesh::new(
                    vec![
                        Vec3::new(-1.0, -1.0, z),
                        Vec3::new(2.0, -1.0, z),
                        Vec3::new(0.0, 2.0, z),
                        Vec3::new(0.0, 0.0, z + 1.0),
                    ],
                    vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]],
                ))
            })
            .collect();
        al.aligner_view.shown = true;
        al.aligner_view.bases = shells
            .iter()
            .zip(0u32..)
            .map(|(mesh, stage)| dca_model::AlignerBase {
                arch: ScanRole::Lower,
                stage,
                mesh: *mesh,
                exact: false,
                voxel_mm: 0.4,
                tolerance_mm: 0.08,
                key: key.clone(),
                seconds: 1.0,
            })
            .collect();
        state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        (state, shells)
    }

    /// The aligners drawn: for each, the stage of the one of `shells` it is
    /// and its mesh.
    fn aligners_drawn<'a>(
        ws: &'a Workspace,
        shells: &[dc_case::BlobRef],
    ) -> Vec<(usize, &'a ViewMesh)> {
        let state = ws.undo.current();
        ws.view
            .meshes
            .iter()
            .filter_map(|m| {
                shells
                    .iter()
                    .position(|b| state.mesh(b).is_some_and(|s| Arc::ptr_eq(s, &m.mesh)))
                    .map(|k| (k, m))
            })
            .collect()
    }

    #[test]
    fn the_aligner_shown_is_drawn_over_whatever_its_arch_shows_at_the_stage_shown() {
        let (state, shells) = aligner_case();
        let world = state.case.world_frame.from_case.to_mat4().as_mat4();
        let color = scene_color(dc_ui::tokens::tokens().dark.model_aligner);
        let mut ws = workspace(state);
        assert!(ws.aligner_shown());
        // the stage drawn, translucent in its colour, where it was built, the
        // only aligner of the arch, never picked
        let drawn_at = |ws: &mut Workspace, model: Model, stage: u32| {
            ws.view.model = model;
            ws.timeline.current = stage;
            ws.sync_scene_with(Color::TOOTH);
            let drawn = aligners_drawn(ws, &shells);
            assert_eq!(drawn.len(), 1, "{model:?} at {stage}");
            let (k, vm) = drawn[0];
            assert!(vm.visible && !vm.pickable, "{model:?} at {stage}");
            assert_eq!(vm.coloring, MeshColoring::Uniform(color));
            assert!((vm.opacity - crate::aligner::ALIGNER_OPACITY).abs() < 1e-6);
            assert!(close_mats(vm.xform, world), "{model:?} at {stage}");
            k
        };
        // over the 3D teeth, the aligner of the stage the timeline shows
        assert_eq!(drawn_at(&mut ws, Model::Teeth, 1), 1);
        assert!(mesh(&ws, manual::tooth_mesh_id(31)).is_some());
        assert_eq!(drawn_at(&mut ws, Model::Teeth, 2), 2);
        // over the scan, as after "Separate teeth and gum": the same aligners
        assert_eq!(drawn_at(&mut ws, Model::Scan, 2), 2);
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_some());
        assert!(mesh(&ws, manual::tooth_mesh_id(31)).is_none());
        assert_eq!(drawn_at(&mut ws, Model::Scan, 1), 1);
        // a tool on the scan keeps it
        ws.manual.tool = Some(manual::Tool::Brush);
        assert_eq!(drawn_at(&mut ws, Model::Teeth, 2), 2);
        ws.manual.tool = None;
        // the region of a tooth shows the scan at the initial position alone
        // (the aligner of the stage shown would hide what it shows); after
        // it, the aligner is back
        ws.view.model = Model::Teeth;
        ws.timeline.current = 2;
        region_on(&mut ws, 31);
        assert_eq!(ws.region_tooth(), Some(31));
        ws.sync_scene_with(Color::TOOTH);
        assert!(aligners_drawn(&ws, &shells).is_empty());
        ws.set_region_view(false);
        assert_eq!(drawn_at(&mut ws, Model::Teeth, 2), 2);
        // at the initial position the arch is its study model, under it
        assert_eq!(drawn_at(&mut ws, Model::Teeth, 0), 0);
        assert_eq!(study_meshes(&ws).len(), 1);
        assert_eq!(drawn_at(&mut ws, Model::Scan, 0), 0);
        // a hidden arch hides its aligner
        ws.timeline.current = 2;
        ws.view.arches.lower = false;
        ws.sync_scene_with(Color::TOOTH);
        assert!(aligners_drawn(&ws, &shells).iter().all(|(_, m)| !m.visible));
        ws.view.arches.lower = true;
        // hidden (the layer of the toolbar, the button of the inspector): gone
        ws.toggle_aligner_layer();
        assert!(ws.wait_for_jobs(std::time::Duration::from_secs(60)));
        assert!(!ws.aligner_shown());
        for (model, stage) in [(Model::Teeth, 2), (Model::Scan, 2), (Model::Teeth, 0)] {
            ws.view.model = model;
            ws.timeline.current = stage;
            ws.sync_scene_with(Color::TOOTH);
            assert!(
                ws.view
                    .meshes
                    .iter()
                    .all(|m| m.opacity != crate::aligner::ALIGNER_OPACITY
                        && m.coloring != MeshColoring::Uniform(color)),
                "{model:?} at {stage}"
            );
        }
    }

    #[test]
    fn an_arch_corrected_by_hand_after_the_build_shows_its_scan_until_built_again() {
        // tooth 31 was painted with the brush after it was built: its 3D tooth
        // is out of date (no measured tips: the crown tops are the anchors)
        let mut state = built_case();
        let mut al = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.tooth_geometry.clear();
        let up_to_date = {
            let mut s = state.clone();
            s.set_module(dca_model::MODULE_ID, ModuleState::new(al.clone()));
            s
        };
        al.dirty.insert(Fdi::new(31).unwrap());
        state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let mut ws = workspace(state);
        assert!(ws.teeth_shown(), "the 3D teeth are the choice");
        ws.sync_scene_with(Color::TOOTH);
        // the lower arch shows its scan, its colours and every mark; its old
        // 3D tooth is not drawn
        let tooth = manual::tooth_mesh_id(31);
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_some());
        assert!(mesh(&ws, tooth).is_none());
        let all = overlay::aligners(ws.undo.current()).unwrap().seeds.len();
        let marks = mesh(&ws, 1 << 61).map_or(0, |m| m.mesh.vertex_count() / 6);
        assert_eq!(marks, all, "every tooth mark drawn");
        // the number of the tooth sits on its crown on the scan
        let on_scan = labels::anchors_of(ws.undo.current(), false);
        let a = ws.tooth_anchors();
        assert_eq!(
            a.iter().find(|a| a.fdi == 31),
            on_scan.iter().find(|a| a.fdi == 31)
        );
        // built again (nothing out of date): the 3D tooth replaces the scan
        ws.undo.push(Arc::new(up_to_date), "rebuilt");
        ws.sync_scene_with(Color::TOOTH);
        assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_none());
        assert!(mesh(&ws, tooth).is_some());
    }

    #[test]
    fn the_3d_teeth_inspector_says_they_are_out_of_date() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut state = built_case();
        let mut al = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.dirty.insert(Fdi::new(31).unwrap());
        state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let mut ws = workspace(state);
        ws.select_item("aligners.teeth.build");
        let mut h = Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui_state(
                |ui, ws: &mut Workspace| {
                    ws.show(ui);
                },
                ws,
            );
        h.run();
        assert!(h.query_by_label("Out of date").is_some());
        assert!(
            h.query_by_label_contains(
                "Changed after the 3D teeth were built (Lower: teeth corrected by hand)"
            )
            .is_some()
        );
        // the toggle says why the lower arch shows its scan
        let teeth = h.get_by_role_and_label(Role::RadioButton, "3D teeth");
        teeth.hover();
        h.run();
        assert!(
            h.query_by_label_contains("it shows the scan until you build the 3D teeth again")
                .is_some()
        );
    }

    #[test]
    fn the_numbers_anchor_on_the_scan_crown_while_the_scan_is_drawn() {
        // no measured tips: the crown tops are the anchors
        let mut state = built_case();
        let mut al = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.tooth_geometry.clear();
        state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let on = |built: bool| {
            labels::anchors_of(&state, built)
                .into_iter()
                .find(|a| a.fdi == 31)
                .unwrap()
        };
        let (teeth, scan) = (on(true), on(false));
        assert_eq!(teeth.kind, labels::AnchorKind::Crown);
        assert_eq!(scan.kind, labels::AnchorKind::Crown);
        // the built tooth is a triangle at the origin of the case; the scan crown is elsewhere
        assert!(
            (teeth.world - scan.world).length() > 1.0,
            "{teeth:?} {scan:?}"
        );
        // the workspace picks the set that is drawn
        let mut ws = workspace(state);
        let a = ws.tooth_anchors();
        assert_eq!(a.iter().find(|a| a.fdi == 31), Some(&teeth));
        ws.view.model = Model::Scan;
        let a = ws.tooth_anchors();
        assert_eq!(a.iter().find(|a| a.fdi == 31), Some(&scan));
    }

    #[test]
    fn the_ui_sync_does_not_wait_and_a_scene_without_slow_entries_is_complete() {
        let mut ws = workspace(built_case());
        ws.view.layers.contacts = false;
        assert!(ws.sync_scene_mode(Color::TOOTH, Color::TOOTH, false));
        assert_eq!(ws.view.synced.map(|k| k.stage), Some(0));
        ws.timeline.current = 2;
        assert!(ws.sync_scene_mode(Color::TOOTH, Color::TOOTH, false));
        assert_eq!(ws.view.synced.map(|k| k.stage), Some(2));
        let t = mesh(&ws, manual::tooth_mesh_id(31)).unwrap();
        assert!((t.xform.transform_point3(Vec3::ZERO)).length() > 0.5);
    }

    #[test]
    fn a_hidden_arch_hides_its_teeth_and_they_are_not_pickable() {
        let mut ws = workspace(built_case());
        assert!(ws.toggle_arch(false));
        ws.sync_scene_with(Color::TOOTH);
        let t = mesh(&ws, manual::tooth_mesh_id(31)).unwrap();
        assert!(!t.visible && !t.pickable);
        assert!(!arch_mesh_shown(ws.undo.current(), &ws.view.meshes, false));
        // the scan of the hidden arch is not drawn either
        assert!(
            mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_none_or(|m| !m.visible),
            "lower scan shown"
        );
    }

    #[test]
    fn a_tool_on_the_scan_and_the_contact_map_show_the_scan() {
        let mut ws = workspace(built_case());
        assert!(ws.teeth_shown());
        for tool in [
            manual::Tool::Brush,
            manual::Tool::Clean,
            manual::Tool::Seeds,
        ] {
            ws.manual.tool = Some(tool);
            assert!(!ws.teeth_shown(), "{tool:?}");
            ws.sync_scene_with(Color::TOOTH);
            assert!(mesh(&ws, scan_id(&ws, ScanRole::Lower)).is_some());
            assert!(mesh(&ws, manual::tooth_mesh_id(31)).is_none());
        }
        // a tool that picks a tooth works on the 3D teeth
        ws.manual.tool = Some(manual::Tool::Missing);
        assert!(ws.teeth_shown());
        ws.manual.tool = None;
        ws.view.layers.contacts = true;
        assert!(!ws.teeth_shown());
    }

    #[test]
    fn the_stage_shown_moves_the_built_tooth_and_ghosts_its_rest() {
        let mut ws = workspace(built_case());
        ws.timeline.current = 2;
        ws.timeline.overlay0 = true;
        ws.sync_scene_with(Color::TOOTH);
        let state = Arc::clone(ws.undo.current());
        let world = state.case.world_frame.from_case;
        // the pose is relative to the rest frame of the tooth, as for the scan
        let st = overlay::stage(overlay::aligners(&state).unwrap(), 2).unwrap();
        let rest0 = world.apply_point(DVec3::ZERO).as_vec3();
        let expect = labels::posed_point(&state, st, 31, rest0);
        assert!((expect - rest0).length() > 0.5);
        let t = mesh(&ws, manual::tooth_mesh_id(31)).unwrap();
        let p = t.xform.transform_point3(Vec3::ZERO);
        assert!((p - expect).length() < 1e-4, "{p:?} vs {expect:?}");
        let ghost = mesh(&ws, manual::tooth_mesh_id(31) | (1 << 62)).unwrap();
        assert!(!ghost.pickable && ghost.opacity <= GHOST_OPACITY);
        let g = ghost.xform.transform_point3(Vec3::ZERO);
        assert!((g - rest0).length() < 1e-4);
    }

    /// [`built_case`] with a final position that moves and turns tooth 31.
    fn with_final(pose: Pose) -> CaseState {
        let mut s = built_case();
        let mut al = s
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.setup.poses.insert(Fdi::new(31).unwrap(), pose);
        s.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        s
    }

    fn close_mats(a: glam::Mat4, b: glam::Mat4) -> bool {
        a.to_cols_array()
            .iter()
            .zip(b.to_cols_array())
            .all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn the_target_layer_ghosts_the_built_teeth_at_their_final_pose_at_any_stage() {
        let pose = Pose::new(
            DVec3::new(0.6, -0.4, 0.3),
            glam::DQuat::from_rotation_z(0.25) * glam::DQuat::from_rotation_x(0.1),
        );
        let mut ws = workspace(with_final(pose));
        let ghost_id = manual::tooth_mesh_id(31) | FINAL_GHOST;
        // off by default
        ws.sync_scene_with(Color::TOOTH);
        assert!(mesh(&ws, ghost_id).is_none());
        ws.toggle_target_layer();
        assert!(ws.view.layers.target);
        let state = Arc::clone(ws.undo.current());
        let al = overlay::aligners(&state).unwrap();
        let f31 = Fdi::new(31).unwrap();
        let world = state.case.world_frame.from_case;
        // rest · pose · rest⁻¹, the rest frame with its origin at the centre
        // of resistance: the centre moves by the pose translation in the
        // tooth axes and the tooth turns about it
        let rest = dca_algo::teeth::rest_pose(&al.tooth_frames[&f31]);
        let moved = rest.compose(&pose).compose(&rest.inverse());
        let expect = (world.to_mat4() * moved.to_mat4()).as_mat4();
        let cr = world.apply_point(rest.t).as_vec3();
        let cr_final = world.apply_point(rest.apply_point(pose.t)).as_vec3();
        let target = scene_color(dc_ui::tokens::tokens().dark.model_target);
        for stage in [0, 1, 2] {
            ws.timeline.current = stage;
            ws.sync_scene_with(Color::TOOTH);
            let g = mesh(&ws, ghost_id).unwrap();
            assert!(!g.pickable && g.opacity <= GHOST_OPACITY, "{stage}");
            assert_eq!(g.coloring, MeshColoring::Uniform(target));
            assert!(close_mats(g.xform, expect), "{stage}: {:?}", g.xform);
            assert!((g.xform.transform_point3(cr) - cr_final).length() < 1e-4);
            // never taken for the tooth by a pick or the selection
            assert_eq!(manual::tooth_of_mesh(g.id), None);
        }
        // the ghost is where a stage with the same pose puts the tooth
        let mut s = (*state).clone();
        let mut al2 = al.clone();
        al2.stages[2].poses.insert(f31, pose);
        s.set_module(dca_model::MODULE_ID, ModuleState::new(al2));
        let mut staged = workspace(s);
        staged.timeline.current = 2;
        staged.sync_scene_with(Color::TOOTH);
        let t = mesh(&staged, manual::tooth_mesh_id(31)).unwrap();
        assert!(close_mats(t.xform, expect));
        // a hidden arch hides its ghost; the layer off removes it
        assert!(ws.toggle_arch(false));
        ws.sync_scene_with(Color::TOOTH);
        assert!(!mesh(&ws, ghost_id).unwrap().visible);
        ws.toggle_target_layer();
        assert!(!ws.view.layers.target);
        ws.sync_scene_with(Color::TOOTH);
        assert!(mesh(&ws, ghost_id).is_none());
    }

    #[test]
    fn the_target_layer_needs_a_final_position() {
        let mut ws = workspace(built_case());
        ws.toggle_target_layer();
        assert!(!ws.view.layers.target);
        assert!(ws.toast_keys().iter().any(|k| k == "ws-toast-target-none"));
        ws.sync_scene_with(Color::TOOTH);
        assert!(
            ws.view
                .meshes
                .iter()
                .all(|m| m.id & FINAL_GHOST != FINAL_GHOST)
        );
    }

    #[test]
    fn the_ipr_layer_needs_a_planned_ipr_and_is_no_longer_unavailable() {
        let mut ws = workspace(built_case());
        ws.toggle_ipr_layer();
        assert!(!ws.view.layers.ipr);
        let keys = ws.toast_keys();
        assert!(keys.iter().any(|k| k == "ws-toast-ipr-none"), "{keys:?}");
        assert!(!keys.iter().any(|k| k == NOT_AVAILABLE_KEY));
        let mut s = built_case();
        let mut al = s
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap()
            .clone();
        al.setup.ipr.push(dca_model::IprStep {
            between: [Fdi::new(41).unwrap(), Fdi::new(31).unwrap()],
            amount_mm: 0.2,
        });
        s.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let mut ws = workspace(s);
        ws.toggle_ipr_layer();
        assert!(ws.view.layers.ipr);
        assert_eq!(crate::labels::ipr_marks(ws.undo.current()).len(), 1);
        ws.toggle_ipr_layer();
        assert!(!ws.view.layers.ipr);
    }

    #[test]
    fn on_the_scan_the_ghost_is_the_crowns_at_their_final_pose() {
        let pose = Pose::from_translation(DVec3::new(0.0, 1.5, 0.0));
        let mut ws = workspace(with_final(pose));
        ws.view.model = Model::Scan;
        ws.view.layers.target = true;
        ws.sync_scene_with(Color::TOOTH);
        let state = Arc::clone(ws.undo.current());
        let id = scan_id(&ws, ScanRole::Lower) | FINAL_GHOST;
        let g = mesh(&ws, id).unwrap();
        assert!(!g.pickable);
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let rest = state.mesh(&scan.mesh).unwrap();
        let al = overlay::aligners(&state).unwrap();
        let labels = state.mesh(&al.labels[&ScanRole::Lower]).unwrap();
        let labels = labels.vertex_labels.as_ref().unwrap();
        // only the crowns, the gingiva left out
        assert!(!g.mesh.tris.is_empty() && g.mesh.tris.len() < rest.tris.len());
        assert!(
            g.mesh
                .tris
                .iter()
                .flatten()
                .all(|&v| labels[v as usize] != 0)
        );
        // tooth 31 moved as the stage display moves it, the others at rest
        let f31 = Fdi::new(31).unwrap();
        let m = overlay::tooth_move(&al.tooth_frames[&f31], &pose);
        for (i, &l) in labels.iter().enumerate() {
            let want = if l == 31 {
                m.apply_point(rest.positions[i].as_dvec3()).as_vec3()
            } else {
                rest.positions[i]
            };
            assert!((g.mesh.positions[i] - want).length() < 1e-4, "{i}");
        }
    }

    #[test]
    fn a_click_on_a_built_tooth_names_it() {
        assert_eq!(manual::tooth_of_mesh(manual::tooth_mesh_id(31)), Some(31));
        assert_eq!(manual::tooth_of_mesh(manual::tooth_mesh_id(0)), None);
        assert_eq!(manual::tooth_of_mesh(1 << 61), None);
        let state = built_case();
        let pick = Pick {
            mesh_id: manual::tooth_mesh_id(31),
            tri: 0,
            point: Vec3::ZERO,
            normal: Vec3::Z,
            barycentric: Vec3::X,
            distance: 1.0,
        };
        let mut m = manual::Manual::default();
        m.tool = Some(manual::Tool::Renumber);
        let ev = manual::Event::Click(pick, manual::Mods::default());
        assert_eq!(
            manual::handle(&mut m, &state, &ev, Vec3::Z * 100.0),
            Action::Chose(31)
        );
        assert_eq!(m.tooth, 31);
    }

    #[test]
    fn the_model_toggle_has_both_languages() {
        for key in [
            "ws-model-scan",
            "ws-model-teeth",
            "ws-model-scan-tip",
            "ws-model-teeth-tip",
            "ws-model-forced",
        ] {
            assert_ne!(t(key), key);
        }
    }

    // ---- the region of a tooth on the scan ----

    fn region_on(ws: &mut Workspace, fdi: u8) {
        ws.select_item(crate::leaf::BUILD_TEETH);
        ws.select_component(Some(crate::select::Component::Tooth(fdi)));
        ws.set_region_view(true);
    }

    fn lower_labels(state: &CaseState) -> Vec<u16> {
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let mesh = state.mesh(&scan.mesh).unwrap();
        overlay::scan_labels(state, ScanRole::Lower, mesh)
            .and_then(|m| m.vertex_labels.clone())
            .unwrap()
    }

    #[test]
    fn the_region_view_shows_the_scan_with_the_tooth_in_colour_and_ends_back_where_it_was() {
        let mut ws = workspace(built_case());
        ws.timeline.current = 2;
        ws.sync_scene_with(Color::TOOTH);
        let state = Arc::clone(ws.undo.current());
        let before: Vec<(u64, bool)> = ws.view.meshes.iter().map(|m| (m.id, m.visible)).collect();
        let scan = scan_id(&ws, ScanRole::Lower);
        // the 3D tooth, posed at aligner 2, replaces the scan
        assert!(ws.teeth_shown() && mesh(&ws, scan).is_none());
        assert!(mesh(&ws, manual::tooth_mesh_id(31)).is_some());

        region_on(&mut ws, 31);
        assert_eq!(ws.region_tooth(), Some(31));
        assert!(!ws.teeth_shown());
        ws.sync_scene_with(Color::TOOTH);
        // the scan is drawn, at the initial position, and the 3D tooth is gone
        let vm = mesh(&ws, scan).expect("the scan of the arch");
        assert!(mesh(&ws, manual::tooth_mesh_id(31)).is_none());
        let scan_mesh = state
            .mesh(&state.case.scan(ScanRole::Lower).unwrap().mesh)
            .unwrap();
        assert!(Arc::ptr_eq(&vm.mesh, scan_mesh), "not posed");
        // the tooth in its colour, everything else dimmed
        let p = overlay::palette();
        let colors = dc_viewport::resolve_colors(&vm.mesh, &vm.coloring);
        let labels = lower_labels(&state);
        assert!(labels.contains(&31) && labels.contains(&0));
        for (l, c) in labels.iter().zip(&colors) {
            if *l == 31 {
                assert_eq!(*c, p.color(31));
            } else {
                assert_ne!(*c, p.color(*l), "label {l} is dimmed");
            }
        }
        // the other arch is dimmed as a whole
        let upper = mesh(&ws, scan_id(&ws, ScanRole::Upper)).unwrap();
        let up_colors = dc_viewport::resolve_colors(&upper.mesh, &upper.coloring);
        let up_labels = {
            let s = ws.undo.current();
            let m = s.mesh(&s.case.scan(ScanRole::Upper).unwrap().mesh).unwrap();
            overlay::scan_labels(s, ScanRole::Upper, m)
                .and_then(|m| m.vertex_labels.clone())
                .unwrap()
        };
        assert!(
            up_labels
                .iter()
                .zip(&up_colors)
                .all(|(l, c)| *c != p.color(*l))
        );
        // it only read the case: nothing was run, nothing to undo
        assert!(Arc::ptr_eq(&state, ws.undo.current()));
        assert!(!ws.is_busy());
        assert!(!ws.undo.can_undo());

        // End: the view of before, the stage included
        ws.set_region_view(false);
        assert!(ws.teeth_shown());
        ws.sync_scene_with(Color::TOOTH);
        let after: Vec<(u64, bool)> = ws.view.meshes.iter().map(|m| (m.id, m.visible)).collect();
        assert_eq!(after, before);
        assert_eq!(ws.timeline.current, 2);
        assert!(Arc::ptr_eq(&state, ws.undo.current()));
    }

    #[test]
    fn the_region_view_gives_back_the_model_and_the_layers_it_overrode() {
        let mut ws = workspace(built_case());
        ws.view.model = Model::Scan;
        ws.view.layers.labels = false;
        ws.sync_scene_with(Color::TOOTH);
        let lower = scan_id(&ws, ScanRole::Lower);
        // the label layer is off: the scan is the enamel colour
        assert_eq!(
            mesh(&ws, lower).unwrap().coloring,
            MeshColoring::Uniform(Color::TOOTH)
        );
        region_on(&mut ws, 31);
        ws.sync_scene_with(Color::TOOTH);
        // while the view lasts the region colours are drawn
        let vm = mesh(&ws, lower).unwrap();
        let state = ws.undo.current();
        let p = overlay::palette();
        let colors = dc_viewport::resolve_colors(&vm.mesh, &vm.coloring);
        let labels = lower_labels(state);
        assert!(
            labels
                .iter()
                .zip(&colors)
                .any(|(l, c)| *l == 31 && *c == p.color(31))
        );
        // End: the model and the layer are as the user left them
        ws.set_region_view(false);
        ws.sync_scene_with(Color::TOOTH);
        assert_eq!(ws.view.model, Model::Scan);
        assert!(!ws.view.layers.labels);
        assert_eq!(
            mesh(&ws, lower).unwrap().coloring,
            MeshColoring::Uniform(Color::TOOTH)
        );
    }

    #[test]
    fn the_region_view_ends_with_escape_a_deselection_a_tool_or_another_function() {
        use crate::select::Component;
        let mut ws = workspace(built_case());
        // no tooth selected: nothing to show, and it does not stay on
        ws.select_item(crate::leaf::BUILD_TEETH);
        ws.set_region_view(true);
        assert_eq!(ws.region_tooth(), None);
        assert!(ws.teeth_shown());
        ws.region_follow();
        assert!(!ws.region.on);
        // Escape ends the view first, the selection next
        region_on(&mut ws, 31);
        assert!(ws.selection_escape());
        assert!(!ws.region.on);
        assert_eq!(ws.selection.component, Some(Component::Tooth(31)));
        assert!(ws.selection_escape());
        assert_eq!(ws.selection.component, None);
        // a deselection
        region_on(&mut ws, 31);
        ws.select_component(None);
        ws.selection_follow_case();
        assert!(!ws.region.on);
        // another tooth moves the view to it
        region_on(&mut ws, 31);
        ws.select_component(Some(Component::Tooth(32)));
        ws.selection_follow_case();
        assert_eq!(ws.region_tooth(), Some(32));
        // the same function keeps it, another function or a tool ends it
        ws.select_item(crate::leaf::BUILD_TEETH);
        assert!(ws.region.on);
        ws.select_item(crate::leaf::SETUP);
        assert!(!ws.region.on);
        region_on(&mut ws, 31);
        assert!(ws.open_tool(manual::Tool::Rebuild));
        assert!(!ws.region.on && ws.manual.tool == Some(manual::Tool::Rebuild));
        // and the view closes the tool
        region_on(&mut ws, 31);
        assert_eq!(ws.manual.tool, None);
        ws.clear_selection();
        assert!(!ws.region.on);
        assert!(!ws.is_busy() && !ws.undo.can_undo());
    }

    /// The demo with a crown-sized patch of its gum painted as tooth 38: a
    /// patch of the gum shell has a boundary, the demo teeth (shells of their
    /// own) have none.
    fn with_patch() -> (CaseState, usize) {
        let s = lower_demo();
        let scan = s.mesh(&s.case.scan(ScanRole::Lower).unwrap().mesh).unwrap();
        let labels = lower_labels(&s);
        let gum: Vec<usize> = (0..labels.len()).filter(|&i| labels[i] == 0).collect();
        let g = scan.positions[gum[gum.len() / 2]];
        let patch: Vec<usize> = gum
            .into_iter()
            .filter(|&i| scan.positions[i].distance(g) < 5.0)
            .collect();
        assert!(patch.len() > 30);
        let n = patch.len();
        let s = exec(
            "aligners.preparation.segmentation.manual-brush",
            &s,
            json!({ "label": 38, "vertices": patch }),
        );
        (s, n)
    }

    #[test]
    fn the_margin_line_comes_from_the_query_once_per_state_and_changes_nothing() {
        use crate::select::Component;
        let (s, n) = with_patch();
        let mut ws = workspace(s);
        // the patch is tooth 38 on the scan; select it as the view does
        ws.select_component(Some(Component::Tooth(38)));
        ws.region.on = true;
        let state = Arc::clone(ws.undo.current());
        let d = ws.region_data().expect("the answer");
        assert_eq!((d.fdi, d.vertices as usize), (38, n));
        assert!(d.error.is_none());
        // closed loops of the margin, in the coordinates of the scan
        assert!(!d.lines.is_empty() && d.lines.iter().all(|(p, closed)| *closed && p.len() > 2));
        assert!(d.length_mm > 8.0);
        assert!(d.check.is_some());
        // asked once: the same answer while the case and the tooth stay
        assert!(Arc::ptr_eq(&d, &ws.region_data().unwrap()));
        // the transform of the arch's scan mesh, from the scene
        ws.sync_scene_with(Color::TOOTH);
        let (data, xform) = ws.region_overlay().expect("the overlay");
        assert!(Arc::ptr_eq(&data, &d));
        assert_eq!(
            xform,
            mesh(&ws, scan_id(&ws, ScanRole::Lower)).unwrap().xform
        );
        // no change of the case
        assert!(Arc::ptr_eq(&state, ws.undo.current()));
        assert!(!ws.undo.can_undo() && !ws.is_busy());
        // a tooth that is a shell of its own has no boundary with the gum: no line
        ws.select_component(Some(Component::Tooth(36)));
        let shell = ws.region_data().unwrap();
        assert!(shell.vertices > 0 && shell.lines.is_empty() && shell.length_mm == 0.0);
        // a tooth with no region: the answer says so
        ws.select_component(Some(Component::Tooth(18)));
        let none = ws.region_data().unwrap();
        assert_eq!(none.vertices, 0);
    }

    #[test]
    fn the_region_view_has_both_languages() {
        for key in [
            "ws-region-show",
            "ws-region-end",
            "ws-region-pick",
            "ws-region-hint",
            "ws-region-no-margin",
            "ws-region-unavailable",
        ] {
            assert_ne!(t(key), key);
        }
        for key in [
            "ws-region-on",
            "ws-region-no-region",
            "ws-region-stats",
            "ws-region-margin",
        ] {
            assert_ne!(dc_i18n::tr_args(key, &[("fdi", "36")]), key);
        }
    }

    #[test]
    fn the_hidden_arch_comes_back_only_when_the_shown_one_lost_its_scan() {
        let only = |upper, lower| Arches { upper, lower };
        // the arch shown still has its scan, or the other has none: no change
        for (arches, has) in [
            (only(true, false), (true, true)),
            (only(true, false), (false, false)),
            (only(false, true), (true, true)),
            (only(true, true), (false, true)),
        ] {
            let mut a = arches;
            assert!(!a.follow_scans(has.0, has.1), "{arches:?} {has:?}");
            assert_eq!(a, arches);
        }
        // the arch shown lost its scan and the hidden one has one
        for (arches, has) in [
            (only(true, false), (false, true)),
            (only(false, true), (true, false)),
        ] {
            let mut a = arches;
            assert!(a.follow_scans(has.0, has.1), "{arches:?}");
            assert_eq!(a, Arches::default());
        }
    }
}
