//! Selecting a 3D component with the mouse and changing it with the gizmo.
//!
//! With no manual tool open, a click on the viewport selects the component
//! under the pointer: a 3D tooth, the gingiva of an arch, a scan, or, on a
//! scan whose teeth are coloured, the tooth whose colour is under the
//! pointer; a click on a tooth number selects that tooth. A click on empty
//! space, Escape or the close button of the inspector section clears it. A
//! selection never runs anything (the menu law); an open tool keeps the
//! pointer.
//!
//! The selection is drawn with a bold outline (`dc_viewport` draws it around
//! the selected meshes; a tooth on the scan gets a copy of its region to
//! outline), the component under the pointer with a light tint, and the
//! selection carries the transform gizmo: W move, E turn, R scale. A drag is
//! shown on the meshes as it goes, without touching the case; letting it go
//! runs `aligners.components.transform` once (one undo step, saved like any
//! change, the same command the CLI and the MCP server run). The inspector
//! starts with the section "Selected component": the name, the gizmo mode,
//! the position, rotation and size fields (each change one command), Reset
//! and Deselect.
//!
//! | Component | Move and turn | Scale |
//! | --- | --- | --- |
//! | tooth | its pose at the aligner shown on the timeline (stage > 0), as "Move a tooth" stores it; refused at the initial position, where the teeth are the scan | its 3D tooth, about its centre |
//! | gingiva | the whole arch (scan, colours, marks, 3D teeth, gingiva) | – |
//! | scan | the same for an upper or lower scan; a bite scan alone | – |
//!
//! Positions and rotations are in the world frame of the viewport (X to the
//! patient's left, Y backwards, Z up), the rotation as X, Y, Z angles in
//! degrees about the centre of the component; the command takes the change
//! in case coordinates.

use std::sync::Arc;

use dc_case::{CaseState, Pose, ScanRole};
use dc_mesh::TriMesh;
use dc_scene::MeshColoring;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, Button};
use dc_viewport::{GizmoDelta, GizmoEvent, GizmoMode, Pick, TransformGizmo, ViewMesh, Viewport};
use egui::{RichText, Ui};
use glam::{DQuat, DVec3, EulerRot, Quat, Vec3};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::function::caption;
use crate::labels;
use crate::manual;
use crate::overlay;
use crate::t;
use crate::workspace::Workspace;

/// The command of the gizmo.
pub(crate) const TRANSFORM: &str = "aligners.components.transform";

/// Bit of the viewport mesh ids of the gingiva of an arch
/// ([`gingiva_mesh_id`]).
const GINGIVA_MESH: u64 = 1 << 59;
/// Bit of the viewport mesh ids of the copy of a tooth region of the scan,
/// drawn over it to outline the tooth.
const REGION_MESH: u64 = 1 << 58;

/// Id of the viewport mesh of the gingiva of `role`.
pub(crate) fn gingiva_mesh_id(role: ScanRole) -> u64 {
    GINGIVA_MESH
        | match role {
            ScanRole::Upper => 1,
            ScanRole::Lower => 2,
            ScanRole::Bite => 3,
        }
}

/// The arch whose gingiva is the viewport mesh `id`.
pub(crate) fn gingiva_of_mesh(id: u64) -> Option<ScanRole> {
    [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .find(|r| gingiva_mesh_id(*r) == id)
}

fn region_mesh_id(fdi: u8) -> u64 {
    REGION_MESH | u64::from(fdi)
}

/// A component of the case that can be selected in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Component {
    /// A tooth: its 3D tooth, or its region on the scan.
    Tooth(u8),
    /// The gingiva of an arch (its built gum).
    Gingiva(ScanRole),
    /// A scan, by id.
    Scan(Uuid),
}

fn arch_word(role: ScanRole) -> &'static str {
    match role {
        ScanRole::Upper => "upper",
        ScanRole::Lower => "lower",
        ScanRole::Bite => "bite",
    }
}

impl Component {
    /// The name the inspector shows ("Tooth 36", "Lower gingiva").
    pub(crate) fn name(self, state: &CaseState) -> String {
        match self {
            Component::Tooth(f) => dc_i18n::tr_args("ws-sel-tooth", &[("fdi", &f.to_string())]),
            Component::Gingiva(role) => t(&format!("ws-sel-gingiva-{}", arch_word(role))),
            Component::Scan(id) => match state.case.scan_by_id(id).map(|s| s.role) {
                Some(ScanRole::Bite) => {
                    let n = state
                        .case
                        .scans_of(ScanRole::Bite)
                        .position(|s| s.id == id)
                        .map_or(1, |i| i + 1);
                    dc_i18n::tr_args("ws-sel-scan-bite", &[("n", &n.to_string())])
                }
                Some(role) => t(&format!("ws-sel-scan-{}", arch_word(role))),
                None => String::new(),
            },
        }
    }

    /// The component is still in the case.
    pub(crate) fn exists(self, state: &CaseState) -> bool {
        let al = overlay::aligners(state);
        match self {
            Component::Tooth(f) => al.is_some_and(|a| {
                a.crowns.keys().any(|k| k.get() == f)
                    || a.tooth_frames.keys().any(|k| k.get() == f)
                    || a.seeds.iter().any(|s| s.fdi.get() == f)
            }),
            Component::Gingiva(role) => al.is_some_and(|a| a.gingiva.contains_key(&role)),
            Component::Scan(id) => state.case.scan_by_id(id).is_some(),
        }
    }

    /// The arch a move of the component takes along (`None`: a bite scan,
    /// or a tooth, which moves alone).
    fn arch(self, state: &CaseState) -> Option<ScanRole> {
        match self {
            Component::Tooth(_) => None,
            Component::Gingiva(role) => Some(role),
            Component::Scan(id) => state
                .case
                .scan_by_id(id)
                .map(|s| s.role)
                .filter(|r| *r != ScanRole::Bite),
        }
    }

    /// The scan whose move the component's position and rotation are: the
    /// scan of its arch, or the bite scan itself.
    fn scan_id(self, state: &CaseState) -> Option<Uuid> {
        match self {
            Component::Tooth(_) => None,
            Component::Gingiva(role) => state.case.scan(role).map(|s| s.id),
            Component::Scan(id) => Some(id),
        }
    }
}

/// The component a pick lands on: a 3D tooth, a gingiva, the tooth whose
/// colour is under the pointer on a scan, else the scan.
pub(crate) fn component_at(state: &CaseState, pick: &Pick, eye: Vec3) -> Option<Component> {
    if let Some(f) = manual::tooth_of_mesh(pick.mesh_id) {
        return Some(Component::Tooth(f));
    }
    if let Some(role) = gingiva_of_mesh(pick.mesh_id) {
        return Some(Component::Gingiva(role));
    }
    let scan = state
        .case
        .scans
        .iter()
        .find(|s| manual::scan_mesh_id(s) == pick.mesh_id)?;
    // a tooth region of the scan selects that tooth
    if let Some(f) = manual::target_of(state, pick, eye)
        .and_then(|t| u8::try_from(t.label).ok())
        .filter(|&f| f != 0)
    {
        return Some(Component::Tooth(f));
    }
    Some(Component::Scan(scan.id))
}

/// What a tooth can do with the gizmo's move and turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToothMove {
    /// It moves at this aligner (the stage shown).
    At(u32),
    /// The stage shown is the initial position.
    Initial,
    /// The plan has no aligners yet.
    NoPlan,
    /// The tooth is not measured (no rest frame).
    NotMeasured,
}

pub(crate) fn tooth_move_at(state: &CaseState, fdi: u8, stage: u32) -> ToothMove {
    let Some(al) = overlay::aligners(state) else {
        return ToothMove::NotMeasured;
    };
    if !al.tooth_frames.keys().any(|f| f.get() == fdi) {
        return ToothMove::NotMeasured;
    }
    if !al.stages.iter().any(|s| s.index > 0) {
        return ToothMove::NoPlan;
    }
    if stage > 0 && al.stages.iter().any(|s| s.index == stage) {
        ToothMove::At(stage)
    } else {
        ToothMove::Initial
    }
}

/// The selection and what the gizmo does with it.
#[derive(Default)]
pub(crate) struct Selection {
    /// The selected component.
    pub component: Option<Component>,
    /// The component under the pointer.
    pub hovered: Option<Component>,
    /// What the gizmo does.
    pub mode: GizmoMode,
    /// The change shown while a handle or a field is dragged (world).
    pub preview: Option<GizmoDelta>,
    /// A change sent to the module, shown until its command ends.
    pub pending: Option<GizmoDelta>,
    /// The field being dragged and its value.
    field: Option<(Field, f64)>,
    /// What the viewport shows of the selection now.
    applied: Option<Decor>,
    /// Copies of tooth regions of the scan, for the scene they were made in.
    regions: Vec<(u64, u8, ViewMesh)>,
}

/// A numeric field of the inspector section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Position(usize),
    Rotation(usize),
    Scale,
}

/// What the viewport was given: the scene it decorates, the region copies
/// and the change shown.
#[derive(Debug, Clone, PartialEq)]
struct Decor {
    generation: u64,
    regions: Vec<u8>,
    delta: Option<GizmoDelta>,
}

/// The input of a frame for the selection.
#[derive(Debug, Default)]
pub(crate) struct SelInput {
    /// A primary click on a mesh.
    pub pick: Option<Pick>,
    /// The mesh under the pointer.
    pub hovered: Option<Pick>,
    /// A primary click that landed on nothing (no mesh, no number).
    pub empty_click: bool,
    /// The tooth number clicked.
    pub chip: Option<u8>,
    /// What the gizmo did.
    pub gizmo: Option<GizmoEvent>,
    /// Where the camera is (the side of a hit on the scan).
    pub eye: Vec3,
}

/// The world frame of the case, as a pose.
fn world_of(state: &CaseState) -> Pose {
    state.case.world_frame.from_case
}

/// A world change (rigid part) as a pose.
fn pose_of(delta: &GizmoDelta) -> Pose {
    let p = delta.pivot.as_dvec3();
    let q = delta.rotation.as_dquat();
    Pose::new(p + delta.translation.as_dvec3() - q * p, q)
}

/// A world move as a move in case coordinates.
fn to_case(state: &CaseState, world_move: &Pose) -> Pose {
    let w = world_of(state);
    w.inverse().compose(world_move).compose(&w)
}

fn pose_json(p: &Pose) -> Value {
    json!({ "t": p.t.to_array(), "q": p.q.to_array() })
}

/// The parameters of the command for `delta` on `c` (a change of where it
/// is), or `None` when it changes nothing.
pub(crate) fn transform_params(
    state: &CaseState,
    c: Component,
    stage: u32,
    delta: &GizmoDelta,
) -> Option<Value> {
    if delta.is_identity() {
        return None;
    }
    let mut p = target_params(state, c, stage)?;
    if (delta.scale - 1.0).abs() > 1e-6 {
        p["scale"] = json!(f64::from(delta.scale));
    } else {
        p["move"] = pose_json(&to_case(state, &pose_of(delta)));
    }
    Some(p)
}

/// The parameters that name the component (and the aligner of a tooth).
fn target_params(state: &CaseState, c: Component, stage: u32) -> Option<Value> {
    Some(match c {
        Component::Tooth(f) => {
            let mut p = json!({ "component": "tooth", "fdi": f });
            if let ToothMove::At(k) = tooth_move_at(state, f, stage) {
                p["stage"] = json!(k);
            }
            p
        }
        Component::Gingiva(role) => json!({ "component": "gingiva", "arch": arch_word(role) }),
        Component::Scan(id) => json!({ "component": "scan", "scan_id": id.to_string() }),
    })
}

/// The meshes of `meshes` shown for `c` (the outline goes around them).
fn shown_ids(state: &CaseState, c: Component, meshes: &[ViewMesh]) -> Vec<u64> {
    let shown = |id: u64| meshes.iter().any(|m| m.id == id && m.visible);
    let ids: Vec<u64> = match c {
        Component::Tooth(f) => vec![manual::tooth_mesh_id(f), region_mesh_id(f)],
        Component::Gingiva(role) => vec![gingiva_mesh_id(role)],
        Component::Scan(id) => state
            .case
            .scan_by_id(id)
            .map(|s| vec![manual::scan_mesh_id(s)])
            .unwrap_or_default(),
    };
    ids.into_iter().filter(|&id| shown(id)).collect()
}

/// The meshes that move with `c`: the component, and for an arch everything
/// of the arch.
fn moving_ids(state: &CaseState, c: Component, meshes: &[ViewMesh]) -> Vec<u64> {
    let ghost = 1u64 << 62;
    match c.arch(state) {
        Some(role) => {
            let upper = role == ScanRole::Upper;
            let scan = state.case.scan(role).map(manual::scan_mesh_id);
            meshes
                .iter()
                .map(|m| m.id)
                .filter(|&id| {
                    let id = id & !ghost;
                    Some(id) == scan
                        || id == gingiva_mesh_id(role)
                        || manual::tooth_of_mesh(id)
                            .is_some_and(|f| labels::upper_tooth(f) == upper)
                        || (id & !0xff == REGION_MESH
                            && u8::try_from(id & 0xff)
                                .is_ok_and(|f| labels::upper_tooth(f) == upper))
                })
                .collect()
        }
        None => match c {
            Component::Tooth(f) => vec![manual::tooth_mesh_id(f), region_mesh_id(f)],
            other => shown_ids(state, other, meshes),
        },
    }
}

/// The copy of the region of tooth `fdi` of the scan mesh `scan` (drawn
/// over it, same colours), or `None` when the scan has no such colour.
fn region_copy(scan: &ViewMesh, labels: &[u16], fdi: u8) -> Option<ViewMesh> {
    let mesh = &scan.mesh;
    let nv = mesh.vertex_count();
    if labels.len() != nv {
        return None;
    }
    let want = u16::from(fdi);
    let colors = dc_viewport::resolve_colors(mesh, &scan.coloring);
    let mut map = vec![u32::MAX; nv];
    let mut positions = Vec::new();
    let mut region_colors = Vec::new();
    let mut tris = Vec::new();
    for t in &mesh.tris {
        if t.iter().any(|&i| labels.get(i as usize) != Some(&want)) {
            continue;
        }
        tris.push(t.map(|i| {
            let i = i as usize;
            if map[i] == u32::MAX {
                map[i] = positions.len() as u32;
                positions.push(mesh.positions[i]);
                region_colors.push(colors[i]);
            }
            map[i]
        }));
    }
    if tris.is_empty() {
        return None;
    }
    let mut vm = ViewMesh::new(region_mesh_id(fdi), Arc::new(TriMesh::new(positions, tris)))
        .with_xform(scan.xform)
        .with_coloring(MeshColoring::PerVertex(Arc::new(region_colors)))
        .with_opacity(scan.opacity);
    vm.visible = scan.visible;
    vm.pickable = false;
    Some(vm)
}

/// A world-space transform as position (mm) and rotation (X, Y, Z degrees)
/// of a component whose centre was at `centre`.
fn readout(m: &Pose, centre: DVec3) -> ([f64; 3], [f64; 3]) {
    let d = m.apply_point(centre) - centre;
    let (x, y, z) = m.q.to_euler(EulerRot::XYZ);
    (
        d.to_array(),
        [x.to_degrees(), y.to_degrees(), z.to_degrees()],
    )
}

/// The world move of a component whose centre starts at `centre`, at
/// `position` (mm) and `rotation` (X, Y, Z degrees).
fn from_readout(centre: DVec3, position: [f64; 3], rotation: [f64; 3]) -> Pose {
    let q = DQuat::from_euler(
        EulerRot::XYZ,
        rotation[0].to_radians(),
        rotation[1].to_radians(),
        rotation[2].to_radians(),
    );
    Pose::new(centre + DVec3::from_array(position) - q * centre, q)
}

/// What the inspector section shows of the component: where it is (world)
/// and its size.
struct Readout {
    /// The total move (world) and the centre it started at (world).
    pose: Option<(Pose, DVec3)>,
    /// The size of a built tooth.
    scale: Option<f64>,
}

impl Workspace {
    /// The component selected, if any.
    pub fn selected_component(&self) -> Option<String> {
        let state = self.undo.current();
        self.selection.component.map(|c| c.name(state))
    }

    /// Selects `c` (or nothing); a selection never runs anything.
    pub(crate) fn select_component(&mut self, c: Option<Component>) {
        if self.selection.component != c {
            self.selection.component = c;
            self.selection.preview = None;
            self.selection.field = None;
            if let Some(vp) = &mut self.view.viewport {
                vp.cancel_transform_drag();
            }
        }
    }

    /// Escape: drops the drag going on, else the selection. `false` when
    /// there was neither (Escape then does what it did before).
    pub(crate) fn selection_escape(&mut self) -> bool {
        let dragging = self
            .view
            .viewport
            .as_mut()
            .is_some_and(Viewport::cancel_transform_drag);
        if dragging || self.selection.preview.is_some() {
            self.selection.preview = None;
            self.selection.field = None;
            return true;
        }
        // the view of a tooth region ends before the selection does
        if self.region_escape() {
            return true;
        }
        if self.selection.component.is_some() {
            self.select_component(None);
            return true;
        }
        false
    }

    /// The gizmo mode of a key (W, E, R).
    pub(crate) fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if self.selection.mode != mode {
            self.selection.mode = mode;
            self.selection.preview = None;
            self.selection.field = None;
        }
    }

    /// The selection follows the case: a component that left it is
    /// deselected, and a change sent is no longer shown once its command
    /// ended (the scene shows the case it gave).
    pub(crate) fn selection_follow_case(&mut self) {
        let state = Arc::clone(self.undo.current());
        if let Some(c) = self.selection.component
            && !c.exists(&state)
        {
            self.select_component(None);
        }
        // the view of a tooth region follows the selection
        self.region_follow();
        if self.selection.hovered.is_some_and(|c| !c.exists(&state)) {
            self.selection.hovered = None;
        }
        if !self.is_busy() {
            self.selection.pending = None;
        }
    }

    /// The gizmo of the selected component in mode `mode`, if it can act:
    /// its centre (world, with the change shown) and mode.
    fn gizmo_of(&self, shown: &[ViewMesh]) -> Option<TransformGizmo> {
        let c = self.selection.component?;
        if self.manual.tool.is_some() {
            return None;
        }
        let state = self.undo.current();
        let mode = self.selection.mode;
        let allowed = match (c, mode) {
            (Component::Tooth(f), GizmoMode::Scale) => shown
                .iter()
                .any(|m| m.id == manual::tooth_mesh_id(f) && m.visible),
            (_, GizmoMode::Scale) => false,
            (Component::Tooth(f), _) => matches!(
                tooth_move_at(state, f, self.timeline.current),
                ToothMove::At(_)
            ),
            _ => true,
        };
        if !allowed {
            return None;
        }
        let pivot = self.pivot_of(c, shown)?;
        let delta = self.selection.preview.or(self.selection.pending);
        let pivot = delta.map_or(pivot, |d| d.matrix().transform_point3(pivot));
        Some(TransformGizmo { pivot, mode })
    }

    /// The centre of the component as shown (world): the centre of its 3D
    /// tooth (the point it is scaled about), else the middle of what is
    /// shown of it.
    fn pivot_of(&self, c: Component, shown: &[ViewMesh]) -> Option<Vec3> {
        let state = self.undo.current();
        let ids = shown_ids(state, c, shown);
        let m = shown.iter().find(|m| ids.first() == Some(&m.id))?;
        if let Component::Tooth(f) = c
            && m.id == manual::tooth_mesh_id(f)
        {
            let centre = dca_model::tooth_centre(&m.mesh.positions).as_vec3();
            return Some(m.xform.transform_point3(centre));
        }
        let b = m.mesh.aabb()?;
        Some(m.xform.transform_point3((b.min + b.max) * 0.5))
    }

    /// Gives the viewport what it shows of the selection: the region copies,
    /// the change being dragged or sent on the meshes that move, the outline
    /// and the tint, and the gizmo.
    pub(crate) fn selection_to_viewport(&mut self) {
        let state = Arc::clone(self.undo.current());
        let generation = self.view.generation;
        // the region copies of the scan for the selected and hovered teeth
        let mut wanted: Vec<u8> = Vec::new();
        for c in [self.selection.component, self.selection.hovered]
            .into_iter()
            .flatten()
        {
            if let Component::Tooth(f) = c
                && !self
                    .view
                    .meshes
                    .iter()
                    .any(|m| m.id == manual::tooth_mesh_id(f) && m.visible)
                && !wanted.contains(&f)
            {
                wanted.push(f);
            }
        }
        self.selection
            .regions
            .retain(|(g, f, _)| *g == generation && wanted.contains(f));
        for &f in &wanted {
            if self.selection.regions.iter().any(|(_, r, _)| *r == f) {
                continue;
            }
            let upper = labels::upper_tooth(f);
            let role = if upper {
                ScanRole::Upper
            } else {
                ScanRole::Lower
            };
            let Some(scan) = state.case.scan(role) else {
                continue;
            };
            let Some(vm) = self
                .view
                .meshes
                .iter()
                .find(|m| m.id == manual::scan_mesh_id(scan))
            else {
                continue;
            };
            let Some(labels) = state
                .mesh(&scan.mesh)
                .and_then(|m| overlay::scan_labels(&state, role, m))
                .and_then(|m| m.vertex_labels.as_ref())
            else {
                continue;
            };
            if let Some(copy) = region_copy(vm, labels, f) {
                self.selection.regions.push((generation, f, copy));
            }
        }
        let delta = self.selection.preview.or(self.selection.pending);
        let decor = Decor {
            generation,
            regions: self.selection.regions.iter().map(|(_, f, _)| *f).collect(),
            delta,
        };
        let mut shown: Vec<ViewMesh> = self.view.meshes.clone();
        shown.extend(self.selection.regions.iter().map(|(_, _, m)| m.clone()));
        if let (Some(d), Some(c)) = (delta, self.selection.component) {
            let moving = moving_ids(&state, c, &shown);
            let m = d.matrix();
            for vm in shown.iter_mut().filter(|vm| moving.contains(&vm.id)) {
                vm.xform = m * vm.xform;
            }
        }
        let gizmo = self.gizmo_of(&shown);
        let selected = self
            .selection
            .component
            .map(|c| shown_ids(&state, c, &shown))
            .unwrap_or_default();
        // the component under the pointer, unless it is the selected one
        let hovered = self
            .selection
            .hovered
            .filter(|h| Some(*h) != self.selection.component && self.manual.tool.is_none())
            .map(|c| shown_ids(&state, c, &shown))
            .unwrap_or_default();
        let changed = self.selection.applied.as_ref() != Some(&decor);
        if let Some(vp) = &mut self.view.viewport {
            if changed {
                vp.set_meshes(shown);
            }
            vp.set_highlight(&selected, &hovered);
            vp.set_transform_gizmo(gizmo);
        }
        self.selection.applied = Some(decor);
    }

    /// The pointer of a frame for the selection: hover, click, the gizmo.
    pub(crate) fn selection_input(&mut self, input: SelInput, ctx: Option<&egui::Context>) {
        let state = Arc::clone(self.undo.current());
        if self.manual.tool.is_some() {
            self.selection.hovered = None;
            return;
        }
        self.selection.hovered = input
            .hovered
            .and_then(|p| component_at(&state, &p, input.eye));
        match input.gizmo {
            Some(GizmoEvent::Drag(d)) => {
                self.selection.preview = Some(d);
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            }
            Some(GizmoEvent::Commit(d)) => {
                self.selection.preview = None;
                self.commit(&d);
            }
            None => {}
        }
        if let Some(f) = input.chip {
            self.select_component(Some(Component::Tooth(f)));
        } else if let Some(p) = input.pick {
            let c = component_at(&state, &p, input.eye);
            self.select_component(c);
        } else if input.empty_click {
            self.select_component(None);
        }
    }

    /// Runs the command for the change `delta` of the selected component,
    /// shown on the meshes until it ends.
    fn commit(&mut self, delta: &GizmoDelta) {
        let Some(c) = self.selection.component else {
            return;
        };
        let state = Arc::clone(self.undo.current());
        if let Some(params) = transform_params(&state, c, self.timeline.current, delta) {
            if !self.is_busy() {
                self.selection.pending = Some(*delta);
            }
            self.run_manual(TRANSFORM, params);
        }
    }

    /// Runs the command that brings `c` to `params` (absolute values or a
    /// reset), merged with the parameters that name it.
    fn run_absolute(&mut self, c: Component, extra: Value) {
        let state = Arc::clone(self.undo.current());
        let Some(mut params) = target_params(&state, c, self.timeline.current) else {
            return;
        };
        if let (Some(p), Value::Object(e)) = (params.as_object_mut(), extra) {
            p.extend(e);
        }
        self.run_manual(TRANSFORM, params);
    }

    /// Where the component is (world) and its size, read from the case.
    fn readout(&self, c: Component) -> Readout {
        let state = self.undo.current();
        let world = world_of(state);
        let to_world = |m: &Pose| world.compose(m).compose(&world.inverse());
        let al = overlay::aligners(state);
        match c {
            Component::Tooth(f) => {
                let fdi = al.and_then(|a| a.tooth_frames.keys().copied().find(|k| k.get() == f));
                let scale = al.and_then(|a| {
                    let k = a.crowns.keys().copied().find(|k| k.get() == f)?;
                    Some(a.tooth_scale(k))
                });
                let pose = match (tooth_move_at(state, f, self.timeline.current), al, fdi) {
                    (ToothMove::At(stage), Some(a), Some(k)) => {
                        let fr = &a.tooth_frames[&k];
                        let local = a
                            .stages
                            .iter()
                            .find(|s| s.index == stage)
                            .and_then(|s| s.poses.get(&k).copied())
                            .unwrap_or(Pose::IDENTITY);
                        let centre = a
                            .crowns
                            .get(&k)
                            .and_then(|r| state.mesh(r))
                            .map_or(fr.pose.t, |m| dca_model::tooth_centre(&m.positions));
                        Some((
                            to_world(&overlay::tooth_move(fr, &local)),
                            world.apply_point(centre),
                        ))
                    }
                    _ => None,
                };
                Readout { pose, scale }
            }
            Component::Gingiva(_) | Component::Scan(_) => {
                let pose = c.scan_id(state).and_then(|id| {
                    let scan = state.case.scan_by_id(id)?;
                    let mesh = state.mesh(&scan.mesh)?;
                    let b = mesh.aabb()?;
                    let now = world.apply_point(((b.min + b.max) * 0.5).as_dvec3());
                    let moved =
                        al.map_or(Pose::IDENTITY, |a| a.components.scan_move(&id.to_string()));
                    let m = to_world(&moved);
                    Some((m, m.inverse().apply_point(now)))
                });
                Readout { pose, scale: None }
            }
        }
    }

    /// The inspector section "Selected component": name, gizmo mode, what
    /// it changes, position, rotation and size, Reset and Deselect.
    pub(crate) fn selection_inspector(&mut self, ui: &mut Ui) {
        let Some(c) = self.selection.component else {
            return;
        };
        let state = Arc::clone(self.undo.current());
        let col = *colors(ui);
        let busy = self.is_busy();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(ic::CURSOR)
                    .font(fonts::icon(16.0))
                    .color(col.select_blue),
            );
            ui.label(
                RichText::new(t("ws-sel-title"))
                    .font(fonts::semibold(13.0))
                    .color(col.text_primary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, ic::X, t("ws-sel-deselect")).clicked() {
                    self.select_component(None);
                }
            });
        });
        if self.selection.component.is_none() {
            return;
        }
        ui.label(
            RichText::new(c.name(&state))
                .font(fonts::semibold(16.0))
                .color(col.text_primary),
        );
        let shown = !shown_ids(&state, c, &self.view.meshes).is_empty();
        if !shown {
            caption(ui, t("ws-sel-hidden"));
        }
        ui.add_space(6.0);
        // the gizmo mode
        let modes = [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale];
        let mut idx = modes
            .iter()
            .position(|m| *m == self.selection.mode)
            .unwrap_or(0);
        let names = [
            t("ws-sel-mode-move"),
            t("ws-sel-mode-rotate"),
            t("ws-sel-mode-scale"),
        ];
        if widgets::segmented_fill(ui, &mut idx, &names, 28.0).changed() {
            self.set_gizmo_mode(modes[idx]);
        }
        ui.add_space(6.0);
        let now = self.readout(c);
        // what the move does, or why the tooth does not move here
        let what = match c {
            Component::Tooth(f) => match tooth_move_at(&state, f, self.timeline.current) {
                ToothMove::At(k) => {
                    dc_i18n::tr_args("ws-sel-what-tooth", &[("stage", &k.to_string())])
                }
                ToothMove::Initial => t("ws-sel-no-move-initial"),
                ToothMove::NoPlan => t("ws-sel-no-move-plan"),
                ToothMove::NotMeasured => t("ws-sel-no-move-measured"),
            },
            _ if c.arch(&state).is_some() => t("ws-sel-what-arch"),
            _ => t("ws-sel-what-bite"),
        };
        caption(ui, what);
        if self.selection.mode == GizmoMode::Scale && now.scale.is_none() {
            caption(ui, t("ws-sel-no-scale"));
        }
        ui.add_space(6.0);
        let mut commit: Option<Value> = None;
        let mut preview: Option<Option<GizmoDelta>> = None;
        ui.add_enabled_ui(!busy, |ui| {
            if let Some((pose, centre)) = now.pose {
                let (pos, rot) = readout(&pose, centre);
                let mut values = [pos, rot];
                for (row, (key, unit, speed)) in [
                    ("ws-sel-position", " mm", 0.05),
                    ("ws-sel-rotation", "°", 0.25),
                ]
                .into_iter()
                .enumerate()
                {
                    ui.label(
                        RichText::new(t(key))
                            .font(fonts::medium(12.0))
                            .color(col.text_secondary),
                    );
                    ui.horizontal(|ui| {
                        for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                            let field = if row == 0 {
                                Field::Position(axis)
                            } else {
                                Field::Rotation(axis)
                            };
                            let mut v = match self.selection.field {
                                Some((f, v)) if f == field => v,
                                _ => values[row][axis],
                            };
                            ui.label(
                                RichText::new(name)
                                    .font(fonts::mono(11.0))
                                    .color(col.text_secondary),
                            );
                            let r = ui.add(
                                egui::DragValue::new(&mut v)
                                    .speed(speed)
                                    .fixed_decimals(if row == 0 { 2 } else { 1 })
                                    .suffix(unit),
                            );
                            if (r.dragged() || r.has_focus())
                                && (v - values[row][axis]).abs() > 1e-9
                            {
                                self.selection.field = Some((field, v));
                                let mut next = values;
                                next[row][axis] = v;
                                let target = from_readout(centre, next[0], next[1]);
                                let step = target.compose(&pose.inverse());
                                preview = Some(Some(GizmoDelta {
                                    pivot: Vec3::ZERO,
                                    translation: step.t.as_vec3(),
                                    rotation: step.q.as_quat(),
                                    scale: 1.0,
                                }));
                            }
                            let done = r.drag_stopped() || r.lost_focus();
                            if done
                                && let Some((f, v)) = self.selection.field.take()
                                && f == field
                            {
                                values[row][axis] = v;
                                let target = from_readout(centre, values[0], values[1]);
                                let world = world_of(&state);
                                let case = world.inverse().compose(&target).compose(&world);
                                commit =
                                    Some(json!({ "absolute": true, "move": pose_json(&case) }));
                                preview = Some(None);
                            }
                        }
                    });
                }
            }
            if let Some(s) = now.scale {
                ui.label(
                    RichText::new(t("ws-sel-scale"))
                        .font(fonts::medium(12.0))
                        .color(col.text_secondary),
                );
                let mut v = match self.selection.field {
                    Some((Field::Scale, v)) => v,
                    _ => s,
                };
                let r = ui.add(
                    egui::DragValue::new(&mut v)
                        .speed(0.002)
                        .range(dca_model::TOOTH_SCALE_MIN..=dca_model::TOOTH_SCALE_MAX)
                        .fixed_decimals(3)
                        .prefix("× "),
                );
                if (r.dragged() || r.has_focus()) && (v - s).abs() > 1e-9 {
                    self.selection.field = Some((Field::Scale, v));
                    if let Some(pivot) = self.pivot_of(c, &self.view.meshes) {
                        preview = Some(Some(GizmoDelta {
                            pivot,
                            translation: Vec3::ZERO,
                            rotation: Quat::IDENTITY,
                            scale: (v / s) as f32,
                        }));
                    }
                }
                if (r.drag_stopped() || r.lost_focus())
                    && let Some((Field::Scale, v)) = self.selection.field.take()
                {
                    commit = Some(json!({ "absolute": true, "scale": v }));
                    preview = Some(None);
                }
            }
            ui.add_space(8.0);
            if Button::secondary(t("ws-sel-reset"))
                .icon(ic::ARROW_COUNTER_CLOCKWISE)
                .full_width(true)
                .enabled(now.pose.is_some() || now.scale.is_some())
                .show(ui)
                .clicked()
            {
                commit = Some(match (c, now.pose) {
                    // a tooth that does not move here: its size only
                    (Component::Tooth(_), None) => json!({ "absolute": true, "scale": 1.0 }),
                    _ => json!({ "reset": true }),
                });
            }
        });
        if let Some(p) = preview {
            self.selection.preview = p;
        }
        if let Some(extra) = commit {
            self.selection.field = None;
            self.run_absolute(c, extra);
        }
        ui.add_space(12.0);
        widgets::hairline(ui);
        ui.add_space(10.0);
    }

    /// The keys of the selection: W, E, R choose the gizmo mode while a
    /// component is selected and no text field has the keyboard.
    pub(crate) fn selection_keys(&mut self, ui: &Ui) {
        if self.selection.component.is_none() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        for (key, mode) in [
            (egui::Key::W, GizmoMode::Translate),
            (egui::Key::E, GizmoMode::Rotate),
            (egui::Key::R, GizmoMode::Scale),
        ] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                self.set_gizmo_mode(mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_ids_of_the_gingiva_and_the_regions_are_their_own() {
        for role in [ScanRole::Upper, ScanRole::Lower] {
            assert_eq!(gingiva_of_mesh(gingiva_mesh_id(role)), Some(role));
            assert_eq!(manual::tooth_of_mesh(gingiva_mesh_id(role)), None);
        }
        assert_eq!(gingiva_of_mesh(manual::tooth_mesh_id(11)), None);
        assert_eq!(manual::tooth_of_mesh(region_mesh_id(11)), None);
        assert_eq!(gingiva_of_mesh(region_mesh_id(11)), None);
    }

    #[test]
    fn readout_and_back_give_the_same_move() {
        let centre = DVec3::new(10.0, -20.0, 3.0);
        let m = from_readout(centre, [0.5, -1.0, 2.0], [5.0, -10.0, 20.0]);
        let (p, r) = readout(&m, centre);
        assert!((DVec3::from_array(p) - DVec3::new(0.5, -1.0, 2.0)).length() < 1e-9);
        assert!((DVec3::from_array(r) - DVec3::new(5.0, -10.0, 20.0)).length() < 1e-9);
        // the centre goes where the position says, the rest turns about it
        assert!((m.apply_point(centre) - centre - DVec3::new(0.5, -1.0, 2.0)).length() < 1e-9);
    }

    #[test]
    fn a_world_change_is_sent_in_case_coordinates() {
        let mut state = CaseState::default();
        state.case.world_frame.from_case =
            Pose::new(DVec3::new(0.0, 0.0, 5.0), DQuat::from_rotation_z(0.5));
        let d = GizmoDelta {
            pivot: Vec3::new(1.0, 2.0, 3.0),
            translation: Vec3::ZERO,
            rotation: Quat::from_rotation_x(0.2),
            scale: 1.0,
        };
        let world = pose_of(&d);
        let case = to_case(&state, &world);
        // the same point, moved in either frame, lands at the same place
        let w = world_of(&state);
        let p = DVec3::new(-4.0, 7.0, 1.0);
        let a = w.apply_point(case.apply_point(p));
        let b = world.apply_point(w.apply_point(p));
        assert!((a - b).length() < 1e-5, "{a:?} {b:?}");
        // the pivot stays where it is
        assert!((world.apply_point(d.pivot.as_dvec3()) - d.pivot.as_dvec3()).length() < 1e-5);
    }
}
