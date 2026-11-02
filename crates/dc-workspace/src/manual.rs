//! The manual tools in the viewport: the label brush, the seed editor, the
//! tooth pickers and the panel with their parameters.
//!
//! Nothing here changes the case itself. Every interaction is turned into
//! the JSON parameters of a registered command of the module (the same
//! commands an agent runs from `dentalcraft-cli` or the MCP server) and run
//! through the workspace, so each action is one step on the undo stack:
//!
//! | Interaction | Command |
//! | --- | --- |
//! | click or drag on the scan with the brush (Shift: gingiva, Ctrl: fill the crown) | `aligners.preparation.segmentation.manual-brush` |
//! | click with the seed tool (Shift: delete the nearest seed) | `aligners.preparation.numbering.place-seeds` |
//! | number in order: click each tooth, `S` skip the tooth, Backspace back, Enter or Esc done | `aligners.preparation.numbering.sequence` (once, at the end) |
//! | clear the tooth marks (seed tool panel) | `aligners.preparation.numbering.clear-seeds` |
//! | Apply in the renumber / missing / rebuild / clean / adjust panels | the command of the tool |
//!
//! Clicking a tooth with any of the picking tools chooses its number. The
//! mapping from a pick to the parameters ([`handle`], [`params_for`]) is pure
//! and tested without a GPU. A command's report (the collisions of an adjusted
//! tooth, the teeth to rebuild) comes back through [`FeedbackCommand`].

use std::sync::{Arc, Mutex};

use dc_case::{CaseState, Fdi, ScanRole};
use dc_command::{Command, CommandError, JobCtx};
use dc_module_api::{CommandSpec, Toast, ToastLevel};
use dc_ui::theme::colors;
use dc_ui::widgets::Button;
use dc_viewport::{Pick, Viewport, ViewportResponse};
use dca_algo::anatomy::{Arch, numbering_sequence};
use egui::{Color32, PointerButton, Pos2, Stroke, Ui};
use glam::{DVec3, Vec3};
use serde_json::{Map, Value, json};

use crate::overlay;
use crate::t;
use crate::workspace::Workspace;

pub(crate) const BRUSH: &str = "aligners.preparation.segmentation.manual-brush";
pub(crate) const CLEAN: &str = "aligners.preparation.segmentation.clean-borders";
pub(crate) const SEEDS: &str = "aligners.preparation.numbering.place-seeds";
pub(crate) const SEQUENCE: &str = "aligners.preparation.numbering.sequence";
pub(crate) const CLEAR_SEEDS: &str = "aligners.preparation.numbering.clear-seeds";
pub(crate) const RENUMBER: &str = "aligners.preparation.numbering.renumber";
pub(crate) const MISSING: &str = "aligners.preparation.numbering.mark-missing";
pub(crate) const REBUILD: &str = "aligners.manual.rebuild-tooth";
pub(crate) const ADJUST: &str = "aligners.planning.movements";

/// Furthest a delete click may be from a seed (mm).
const SEED_REACH_MM: f64 = 4.0;

/// A manual tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// Paint, erase or fill labels on the scan.
    Brush,
    /// Remove the islands and holes of the labels.
    Clean,
    /// Place, move or delete seeds.
    Seeds,
    /// Renumber or swap teeth.
    Renumber,
    /// Mark a tooth missing or present.
    Missing,
    /// Rebuild one tooth.
    Rebuild,
    /// Move and turn one tooth.
    Adjust,
}

impl Tool {
    /// The tool that a menu command opens.
    pub(crate) fn from_command(id: &str) -> Option<Self> {
        [
            Self::Brush,
            Self::Clean,
            Self::Seeds,
            Self::Renumber,
            Self::Missing,
            Self::Rebuild,
            Self::Adjust,
        ]
        .into_iter()
        .find(|t| t.command_id() == id)
    }

    /// The command the tool runs.
    pub(crate) fn command_id(self) -> &'static str {
        match self {
            Self::Brush => BRUSH,
            Self::Clean => CLEAN,
            Self::Seeds => SEEDS,
            Self::Renumber => RENUMBER,
            Self::Missing => MISSING,
            Self::Rebuild => REBUILD,
            Self::Adjust => ADJUST,
        }
    }

    fn title_key(self) -> &'static str {
        match self {
            Self::Brush => "ws-manual-brush",
            Self::Clean => "ws-manual-clean",
            Self::Seeds => "ws-manual-seeds",
            Self::Renumber => "ws-manual-renumber",
            Self::Missing => "ws-manual-missing",
            Self::Rebuild => "ws-manual-rebuild",
            Self::Adjust => "ws-manual-adjust",
        }
    }

    fn hint_key(self) -> &'static str {
        match self {
            Self::Brush => "ws-manual-hint-brush",
            Self::Seeds => "ws-manual-hint-seeds",
            _ => "ws-manual-hint-pick",
        }
    }

    /// The tool acts on the scan with the pointer (a circle follows it).
    fn paints(self) -> bool {
        matches!(self, Self::Brush | Self::Seeds)
    }
}

/// Movement fields of the adjust panel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Fields {
    pub mesial_mm: f64,
    pub buccal_mm: f64,
    pub extrusion_mm: f64,
    pub torque_deg: f64,
    pub tip_deg: f64,
    pub rotation_deg: f64,
}

impl Fields {
    fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// State of the manual tools of a workspace.
pub(crate) struct Manual {
    /// The tool open, if any.
    pub tool: Option<Tool>,
    /// The tooth the tools work on: the brush label, the seed to place, the
    /// tooth to renumber, mark, rebuild or adjust (`0` = gingiva for the
    /// brush, all the teeth for the clean-up).
    pub tooth: u8,
    /// Brush radius (mm).
    pub radius_mm: f32,
    /// New number of the renumbered tooth.
    pub dst: u8,
    /// Mark missing (`true`) or present (`false`).
    pub missing: bool,
    /// Find the region again from the seed before the rebuild.
    pub resegment: bool,
    /// Rings of smoothing of the clean-up.
    pub rings: u32,
    /// The movement to apply.
    pub fields: Fields,
    /// Adjust the stage shown on the timeline instead of the setup.
    pub at_stage: bool,
    /// Keep the adjusted tooth where it is put when the setup is computed again.
    pub lock: bool,
    /// Refuse a move that makes the tooth collide.
    pub refuse_on_collision: bool,
    /// The numbering in order that is going on, if any (seed tool).
    pub sequence: Option<Sequence>,
    /// The case the numbering in order started on: when the undo stack moves
    /// away from it (undo, redo, another command) the numbering stops.
    seq_base: Option<Arc<CaseState>>,
    /// Number the third molars too in the next numbering in order.
    pub third_molars: bool,
    /// Picks of the stroke being drawn.
    stroke: Vec<Pick>,
    /// Their screen positions (for the feedback while drawing).
    stroke_screen: Vec<Pos2>,
    /// Reports of the commands run, filled from the job threads.
    sink: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Manual {
    /// The viewport should report the mesh under the pointer (the tool draws
    /// a circle there).
    pub(crate) fn wants_hover(&self) -> bool {
        self.tool.is_some_and(Tool::paints)
    }
}

impl Default for Manual {
    fn default() -> Self {
        Self {
            tool: None,
            tooth: 0,
            radius_mm: 1.0,
            dst: 0,
            missing: true,
            resegment: false,
            rings: 1,
            fields: Fields::default(),
            at_stage: false,
            lock: false,
            refuse_on_collision: false,
            sequence: None,
            seq_base: None,
            third_molars: false,
            stroke: Vec::new(),
            stroke_screen: Vec::new(),
            sink: Arc::default(),
        }
    }
}

/// Keys held when the pointer acts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mods {
    /// Shift: paint the gingiva, delete a seed.
    pub shift: bool,
    /// Ctrl (or Cmd): fill the crown.
    pub ctrl: bool,
}

impl From<egui::Modifiers> for Mods {
    fn from(m: egui::Modifiers) -> Self {
        Self {
            shift: m.shift,
            ctrl: m.command || m.ctrl,
        }
    }
}

/// What the pointer did in the viewport.
#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// A click on a mesh.
    Click(Pick, Mods),
    /// A drag over the scan, one pick per step.
    Stroke(Vec<Pick>, Mods),
}

/// What an [`Event`] means.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    /// Run this command with these parameters.
    Run(&'static str, Value),
    /// A tooth was chosen on the scan.
    Chose(u8),
    /// Tell the user (i18n key).
    Hint(&'static str),
    /// Nothing to do.
    None,
}

/// One step of the numbering in order: the tooth it is for and where it was
/// clicked (`None`: skipped, the tooth gets no mark now).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeqStep {
    pub fdi: u8,
    /// The click in case coordinates and in the world (to draw the marker).
    pub at: Option<(DVec3, Vec3)>,
}

/// The numbering in order of `oa.place_seeds` of the plugin: the teeth are
/// asked for one after the other, from the right distal to the left distal.
/// The clicks are kept here and sent in one command when the user finishes,
/// so the whole numbering is one step on the undo stack.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Sequence {
    /// `upper` or `lower`: known at the start when the case has one arch, else
    /// from the first click.
    pub arch: Option<&'static str>,
    pub third_molars: bool,
    /// The teeth still to number at the start, in order.
    order: Vec<u8>,
    pub steps: Vec<SeqStep>,
}

/// What a key does in the numbering in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeqKey {
    /// `S`: skip the tooth (no mark; it is not marked missing, as in the plugin).
    Skip,
    /// Backspace: take the last step back.
    Undo,
    /// Enter or Esc: done.
    Finish,
}

fn arch_of_name(name: &str) -> Arch {
    if name == "upper" {
        Arch::Upper
    } else {
        Arch::Lower
    }
}

impl Sequence {
    /// A numbering that starts now on `state`.
    pub(crate) fn start(state: &CaseState, third_molars: bool) -> Self {
        let arches: Vec<&'static str> = state
            .case
            .scans
            .iter()
            .filter_map(|s| arch_name(s.role))
            .collect();
        let mut seq = Self {
            third_molars,
            ..Self::default()
        };
        if let [one] = arches.as_slice() {
            seq.set_arch(state, one);
        }
        seq
    }

    /// Works on `arch`: the teeth to number are those the module will take
    /// (`numbering_sequence`, without the teeth that have a mark or are missing).
    fn set_arch(&mut self, state: &CaseState, arch: &'static str) {
        let taken: Vec<Fdi> = overlay::aligners(state)
            .map(|a| {
                a.seeds
                    .iter()
                    .map(|s| s.fdi)
                    .chain(a.missing.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        self.arch = Some(arch);
        self.order = numbering_sequence(arch_of_name(arch), self.third_molars, &taken)
            .into_iter()
            .map(Fdi::get)
            .collect();
    }

    /// The tooth the next step is for (`None`: all done, or the arch is not
    /// known yet).
    pub(crate) fn next(&self) -> Option<u8> {
        self.order.get(self.steps.len()).copied()
    }

    /// The parameters of the command for the steps so far.
    fn params(&self) -> Value {
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|s| match s.at {
                Some((p, _)) => json!({ "kind": "point", "position": vec3(p), "fdi": s.fdi }),
                None => json!({ "kind": "skip", "fdi": s.fdi }),
            })
            .collect();
        json!({
            "arch": self.arch,
            "third_molars": self.third_molars,
            "steps": steps,
        })
    }
}

/// The user is done (Enter or Esc, as in the plugin: the marks given are
/// kept): the command with all the steps, or nothing when no tooth was
/// clicked (skips alone change nothing).
fn sequence_finish(m: &mut Manual) -> Action {
    match m.sequence.take() {
        Some(seq) if seq.steps.iter().any(|s| s.at.is_some()) => {
            Action::Run(SEQUENCE, seq.params())
        }
        _ => Action::None,
    }
}

/// A click on a scan in the numbering in order: the next tooth gets that point.
pub(crate) fn sequence_click(m: &mut Manual, state: &CaseState, pick: &Pick, eye: Vec3) -> Action {
    let Some(seq) = m.sequence.as_mut() else {
        return Action::None;
    };
    let Some(target) = target_of(state, pick, eye) else {
        return Action::None;
    };
    let Some(arch) = arch_name(target.role) else {
        return Action::Hint("ws-manual-no-arch");
    };
    match seq.arch {
        None => seq.set_arch(state, arch),
        Some(a) if a != arch => return Action::Hint("ws-manual-seq-other-arch"),
        Some(_) => {}
    }
    let Some(fdi) = seq.next() else {
        return sequence_finish(m);
    };
    seq.steps.push(SeqStep {
        fdi,
        at: Some((target.point, pick.point)),
    });
    // the last tooth of the arch ends the numbering, as in the plugin
    if seq.next().is_none() {
        return sequence_finish(m);
    }
    Action::None
}

/// A key in the numbering in order.
pub(crate) fn sequence_key(m: &mut Manual, key: SeqKey) -> Action {
    let Some(seq) = m.sequence.as_mut() else {
        return Action::None;
    };
    match key {
        SeqKey::Skip => {
            let Some(fdi) = seq.next() else {
                return if seq.arch.is_none() {
                    Action::Hint("ws-manual-seq-first")
                } else {
                    sequence_finish(m)
                };
            };
            seq.steps.push(SeqStep { fdi, at: None });
            if seq.next().is_none() {
                return sequence_finish(m);
            }
            Action::None
        }
        SeqKey::Undo => {
            seq.steps.pop();
            Action::None
        }
        SeqKey::Finish => sequence_finish(m),
    }
}

/// A pick, as the scan sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Target {
    /// Scan that was hit.
    pub role: ScanRole,
    /// Hit point in the coordinates of the scan (case coordinates, mm).
    pub point: DVec3,
    /// Vertex of the hit triangle nearest to the hit point.
    pub vertex: u32,
    /// Tooth label of that vertex (`0` = gingiva or no labels).
    pub label: u16,
    /// Direction the viewer looks along at the hit, in scan coordinates.
    pub view_dir: DVec3,
}

/// Id of the viewport mesh of a scan (as `view::scene_meshes` assigns it).
pub(crate) fn scan_mesh_id(scan: &dc_case::Scan) -> u64 {
    let (a, b) = scan.id.as_u64_pair();
    (a ^ b) >> 2
}

/// The scan, point, vertex and label that a pick of the viewport hits. The
/// meshes are drawn with `world * import_xform`; the commands work in the
/// coordinates of the scan mesh, so the hit goes back through that transform.
pub(crate) fn target_of(state: &CaseState, pick: &Pick, eye: Vec3) -> Option<Target> {
    let scan = state
        .case
        .scans
        .iter()
        .find(|s| scan_mesh_id(s) == pick.mesh_id)?;
    let mesh = state.mesh(&scan.mesh)?;
    let world = state.case.world_frame.from_case.to_mat4().as_mat4();
    let xform = world * scan.import_xform.to_mat4().as_mat4();
    let inv = xform.inverse();
    let point = inv.transform_point3(pick.point).as_dvec3();
    let tri = *mesh.tris.get(pick.tri as usize)?;
    // the corner with the largest barycentric weight
    let w = pick.barycentric;
    let k = if w.x >= w.y && w.x >= w.z {
        0
    } else if w.y >= w.z {
        1
    } else {
        2
    };
    let vertex = tri[k];
    let label = overlay::scan_labels(state, scan.role, mesh)
        .and_then(|m| m.vertex_labels.as_ref())
        .and_then(|l| l.get(vertex as usize).copied())
        .unwrap_or(0);
    let dir_world = (pick.point - eye).normalize_or_zero();
    let view_dir = inv
        .transform_vector3(dir_world)
        .as_dvec3()
        .normalize_or_zero();
    Some(Target {
        role: scan.role,
        point,
        vertex,
        label,
        view_dir,
    })
}

fn arch_name(role: ScanRole) -> Option<&'static str> {
    match role {
        ScanRole::Upper => Some("upper"),
        ScanRole::Lower => Some("lower"),
        _ => None,
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn vec3(v: DVec3) -> Value {
    json!([round3(v.x), round3(v.y), round3(v.z)])
}

/// The seed nearest to `point` among those of the arch of `role`, if one
/// is within reach.
fn nearest_seed(state: &CaseState, role: ScanRole, point: DVec3) -> Option<u8> {
    overlay::aligners(state)?
        .seeds
        .iter()
        .filter(|s| (s.fdi.is_upper()) == (role == ScanRole::Upper))
        .map(|s| (s.position.distance(point), s.fdi.get()))
        .filter(|(d, _)| *d <= SEED_REACH_MM)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
}

/// What a pointer event means for the tool open. A tooth picked by a
/// picking tool is stored in `m.tooth` (the caller gets [`Action::Chose`]).
pub(crate) fn handle(m: &mut Manual, state: &CaseState, ev: &Event, eye: Vec3) -> Action {
    let Some(tool) = m.tool else {
        return Action::None;
    };
    let (picks, mods) = match ev {
        Event::Click(p, mods) => (std::slice::from_ref(p), *mods),
        Event::Stroke(p, mods) => (p.as_slice(), *mods),
    };
    let Some(first) = picks.first().and_then(|p| target_of(state, p, eye)) else {
        return Action::None;
    };
    match tool {
        Tool::Brush => {
            let Some(arch) = arch_name(first.role) else {
                return Action::Hint("ws-manual-no-arch");
            };
            let mut p = Map::new();
            p.insert("arch".into(), json!(arch));
            if mods.ctrl && matches!(ev, Event::Click(..)) {
                // Ctrl+click: the whole crown up to its margin
                p.insert("mode".into(), json!("fill"));
                p.insert("label".into(), json!(if mods.shift { 0 } else { m.tooth }));
                p.insert("vertex".into(), json!(first.vertex));
            } else {
                // Shift: the gingiva
                p.insert("label".into(), json!(if mods.shift { 0 } else { m.tooth }));
                p.insert("centre".into(), vec3(first.point));
                p.insert("radius_mm".into(), json!(round3(f64::from(m.radius_mm))));
                // the rest of the stroke, on the same scan
                let more: Vec<Value> = picks[1..]
                    .iter()
                    .filter_map(|p| target_of(state, p, eye))
                    .filter(|t| t.role == first.role)
                    .map(|t| vec3(t.point))
                    .collect();
                if !more.is_empty() {
                    p.insert("stroke".into(), Value::Array(more));
                }
                p.insert("view_dir".into(), vec3(first.view_dir));
            }
            Action::Run(BRUSH, Value::Object(p))
        }
        Tool::Seeds => {
            if !matches!(ev, Event::Click(..)) {
                return Action::None;
            }
            if mods.shift {
                return match nearest_seed(state, first.role, first.point) {
                    Some(f) => Action::Run(SEEDS, json!({ "op": "delete", "fdi": f })),
                    None => Action::Hint("ws-manual-no-seed"),
                };
            }
            let fdi = if m.tooth != 0 {
                m.tooth
            } else {
                u8::try_from(first.label).unwrap_or(0)
            };
            if fdi == 0 {
                return Action::Hint("ws-manual-no-tooth");
            }
            let has = overlay::aligners(state)
                .is_some_and(|a| a.seeds.iter().any(|s| s.fdi.get() == fdi));
            Action::Run(
                SEEDS,
                json!({
                    "op": if has { "move" } else { "add" },
                    "fdi": fdi,
                    "position": vec3(first.point),
                }),
            )
        }
        Tool::Clean | Tool::Renumber | Tool::Missing | Tool::Rebuild | Tool::Adjust => {
            if !matches!(ev, Event::Click(..)) {
                return Action::None;
            }
            match u8::try_from(first.label) {
                Ok(f) if f != 0 => {
                    m.tooth = f;
                    Action::Chose(f)
                }
                _ => Action::Hint("ws-manual-no-tooth"),
            }
        }
    }
}

/// The parameters of the command a panel runs, or the key of what is
/// missing. `stage` is the stage shown on the timeline (`0` = initial).
pub(crate) fn params_for(m: &Manual, tool: Tool, stage: u32) -> Result<Value, &'static str> {
    match tool {
        Tool::Brush | Tool::Seeds => Err("ws-manual-hint-pick"),
        Tool::Clean => {
            let mut p = json!({ "rings": m.rings });
            if m.tooth != 0 {
                p["fdi"] = json!(m.tooth);
            }
            Ok(p)
        }
        Tool::Renumber => {
            if m.tooth == 0 || m.dst == 0 {
                return Err("ws-manual-no-tooth");
            }
            Ok(json!({ "src": m.tooth, "dst": m.dst }))
        }
        Tool::Missing => {
            if m.tooth == 0 {
                return Err("ws-manual-no-tooth");
            }
            Ok(json!({ "fdi": m.tooth, "missing": m.missing }))
        }
        Tool::Rebuild => {
            if m.tooth == 0 {
                return Err("ws-manual-no-tooth");
            }
            Ok(json!({ "fdi": m.tooth, "resegment": m.resegment }))
        }
        Tool::Adjust => {
            if m.tooth == 0 {
                return Err("ws-manual-no-tooth");
            }
            if m.fields.is_zero() {
                return Err("ws-manual-no-movement");
            }
            let f = &m.fields;
            let mut p = Map::new();
            p.insert("fdi".into(), json!(m.tooth));
            for (k, v) in [
                ("mesial_mm", f.mesial_mm),
                ("buccal_mm", f.buccal_mm),
                ("extrusion_mm", f.extrusion_mm),
                ("torque_deg", f.torque_deg),
                ("tip_deg", f.tip_deg),
                ("rotation_deg", f.rotation_deg),
            ] {
                if v != 0.0 {
                    p.insert(k.into(), json!(v));
                }
            }
            if m.at_stage && stage > 0 {
                p.insert("stage".into(), json!(stage));
            } else if m.lock {
                p.insert("lock".into(), json!(true));
            }
            if m.refuse_on_collision {
                p.insert("refuse_on_collision".into(), json!(true));
            }
            Ok(Value::Object(p))
        }
    }
}

/// The toast for the report of a command, if it has something to say.
pub(crate) fn feedback_toast(id: &str, out: &Value) -> Option<Toast> {
    let list = |key: &str, f: &dyn Fn(&Value) -> Option<String>| -> Vec<String> {
        out[key]
            .as_array()
            .map(|a| a.iter().filter_map(f).collect())
            .unwrap_or_default()
    };
    match id {
        ADJUST => {
            let hits = list("collisions", &|c| {
                (c["collides"] == true).then(|| {
                    format!(
                        "{}: {:.2} mm",
                        c["with"],
                        c["penetration_mm"].as_f64().unwrap_or(0.0)
                    )
                })
            });
            if hits.is_empty() {
                Some(Toast::new(ToastLevel::Info, "ws-manual-adjusted"))
            } else {
                Some(
                    Toast::new(ToastLevel::Warning, "ws-manual-collision")
                        .with_detail(hits.join(", ")),
                )
            }
        }
        BRUSH | CLEAN => {
            let teeth: Vec<String> = match id {
                BRUSH => out["to_rebuild"].as_array(),
                _ => out["lower"]["to_rebuild"].as_array(),
            }
            .map(|a| a.iter().map(|v| v.to_string()).collect())
            .unwrap_or_default();
            (!teeth.is_empty()).then(|| {
                Toast::new(ToastLevel::Info, "ws-manual-to-rebuild").with_detail(teeth.join(", "))
            })
        }
        REBUILD => Some(
            Toast::new(ToastLevel::Success, "ws-manual-rebuilt")
                .with_detail(out["message"].as_str().unwrap_or_default()),
        ),
        SEQUENCE => {
            let marks = list("assigned", &|a| (a["kind"] == "point").then(String::new)).len();
            let mut detail = dc_i18n::tr_args(
                "ws-manual-seq-numbered-detail",
                &[("n", &marks.to_string())],
            );
            if let Some(next) = out["next_fdi"].as_u64() {
                detail.push(' ');
                detail.push_str(&dc_i18n::tr_args(
                    "ws-manual-seq-numbered-next",
                    &[("fdi", &next.to_string())],
                ));
            }
            Some(Toast::new(ToastLevel::Success, "ws-manual-seq-numbered").with_detail(detail))
        }
        CLEAR_SEEDS => Some(Toast::new(ToastLevel::Info, "ws-manual-cleared")),
        RENUMBER => Some(
            Toast::new(ToastLevel::Info, "ws-manual-renumbered")
                .with_detail(format!("{} / {}", out["src"], out["dst"])),
        ),
        MISSING => Some(Toast::new(
            ToastLevel::Info,
            if out["missing"] == true {
                "ws-manual-marked-missing"
            } else {
                "ws-manual-marked-present"
            },
        )),
        _ => None,
    }
}

/// The ids of `out[key]`, an array of `CLEARED_IDS`.
fn ids_of<'a>(out: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    out[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The notices for what a change of the tooth marks did to the results:
/// those it dropped (`dropped`: the automatic numbering, per arch in
/// `arches`) and those it left out of date (`invalidated`: the marks edited
/// by hand, kept as the plugin keeps them), in the `aligners-cleared-*` words
/// of the scan import; and the arches the automatic numbering could not
/// number (`failed`). `out` is the report of the place-seeds, sequence,
/// clear-seeds or numbering command, or of Find the teeth (its `steps`).
pub(crate) fn marks_toasts(out: &Value) -> Vec<Toast> {
    crate::loader();
    let mut reports = vec![out];
    reports.extend(
        out["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| &s["output"]),
    );
    let mut dropped = Vec::new();
    let mut invalidated = Vec::new();
    let mut failed = Vec::new();
    for r in reports {
        dropped.extend(ids_of(r, "dropped"));
        for a in r["arches"].as_array().into_iter().flatten() {
            dropped.extend(ids_of(a, "dropped"));
        }
        invalidated.extend(ids_of(r, "invalidated"));
        for f in r["failed"].as_array().into_iter().flatten() {
            if let Some(e) = f["error"].as_str() {
                failed.push(e.to_owned());
            }
        }
    }
    // one id each, in the order the module lists them
    let ordered = |ids: &[&str]| -> Vec<&'static str> {
        dca_model::CLEARED_IDS
            .into_iter()
            .filter(|id| ids.contains(id))
            .collect()
    };
    let mut toasts = Vec::new();
    for (ids, level, title, detail) in [
        (
            ordered(&dropped),
            ToastLevel::Warning,
            "ws-toast-marks-dropped",
            "ws-marks-dropped-detail",
        ),
        (
            ordered(&invalidated),
            ToastLevel::Info,
            "ws-toast-marks-outdated",
            "ws-marks-outdated-detail",
        ),
    ] {
        if !ids.is_empty() {
            let list = crate::import::cleared_list(dca_model::MODULE_ID, &ids);
            toasts.push(
                Toast::new(level, title).with_detail(dc_i18n::tr_args(detail, &[("list", &list)])),
            );
        }
    }
    if !failed.is_empty() {
        toasts.push(
            Toast::new(ToastLevel::Warning, "ws-toast-find-arch-failed").with_detail(
                dc_i18n::tr_args(
                    "ws-find-arch-failed-detail",
                    &[("reasons", &failed.join("; "))],
                ),
            ),
        );
    }
    toasts
}

/// A command of the module bound to its parameters whose report is kept for
/// the workspace (the undo stack drops it).
struct FeedbackCommand {
    spec: CommandSpec,
    params: Value,
    sink: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Command for FeedbackCommand {
    fn id(&self) -> &'static str {
        self.spec.id
    }

    fn label_key(&self) -> &str {
        self.spec.label_key
    }

    fn apply(&self, state: &CaseState, ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let (next, out) = self.spec.execute(state, &self.params, ctx)?;
        if let Ok(mut sink) = self.sink.lock() {
            sink.push((self.spec.id.to_owned(), out));
        }
        Ok(next)
    }
}

fn pos_color(c: dc_scene::Color) -> Color32 {
    let f = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(f(c.r), f(c.g), f(c.b), 255)
}

/// Reads the pointer in the viewport: the brush circle, the stroke being
/// drawn and the events of this frame.
pub(crate) fn collect(m: &mut Manual, ui: &Ui, vp: &Viewport, vr: &ViewportResponse) -> Vec<Event> {
    let Some(tool) = m.tool else {
        m.stroke.clear();
        m.stroke_screen.clear();
        return Vec::new();
    };
    let rect = vr.response.rect;
    let mods = Mods::from(ui.input(|i| i.modifiers));
    let c = *colors(ui);
    let tint = if tool == Tool::Brush && !mods.shift {
        pos_color(overlay::palette().color(u16::from(m.tooth)))
    } else {
        c.select_blue
    };
    let painter = ui.painter_at(rect);
    // the circle of the brush at the pointer
    if tool.paints()
        && let Some(h) = vr.hovered
    {
        let at = vp.screen_pos(rect, h.point);
        let radius = if tool == Tool::Brush {
            vp.screen_length(rect, h.point, m.radius_mm).max(3.0)
        } else {
            6.0
        };
        painter.circle_stroke(at, radius, Stroke::new(1.5, tint));
    }
    // the marks of the numbering in order, not sent yet
    if let Some(seq) = &m.sequence {
        for step in &seq.steps {
            let (color, at) = match step.at {
                Some((_, world)) => (tint, vp.screen_pos(rect, world)),
                None => continue,
            };
            painter.circle_filled(at, 5.0, color);
            painter.text(
                at + egui::vec2(7.0, -7.0),
                egui::Align2::LEFT_BOTTOM,
                step.fdi.to_string(),
                egui::FontId::proportional(13.0),
                c.text_primary,
            );
        }
    }
    let mut out = Vec::new();
    if tool == Tool::Brush {
        if vr.response.dragged_by(PointerButton::Primary) {
            if let Some(pos) = vr.response.interact_pointer_pos()
                && let Some(p) = vp.pick_at(rect, pos)
                && m.stroke.last().is_none_or(|l| {
                    l.mesh_id == p.mesh_id && l.point.distance(p.point) > 0.25 * m.radius_mm
                })
            {
                m.stroke.push(p);
                m.stroke_screen.push(pos);
            }
            for p in &m.stroke_screen {
                painter.circle_filled(*p, 3.0, tint);
            }
        }
        if vr.response.drag_stopped_by(PointerButton::Primary) {
            m.stroke_screen.clear();
            let picks = std::mem::take(&mut m.stroke);
            if !picks.is_empty() {
                out.push(Event::Stroke(picks, mods));
            }
        }
    }
    if let Some(p) = vr.pick {
        out.push(Event::Click(p, mods));
    }
    out
}

/// A labelled number field of the panel.
fn number<N: egui::emath::Numeric>(
    ui: &mut Ui,
    label: String,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    speed: f64,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        let l = ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                .range(range)
                .speed(speed)
                .suffix(suffix),
        )
        .labelled_by(l.id);
    });
}

impl Workspace {
    /// Opens a manual tool (a menu entry of the tool was activated).
    pub(crate) fn open_manual_tool(&mut self, tool: Tool) {
        self.manual.tool = Some(tool);
        self.manual.sequence = None;
        self.manual.seq_base = None;
        // the numbering in order searches the third molars as Find the teeth does
        if tool == Tool::Seeds {
            self.manual.third_molars = overlay::aligners(self.undo.current())
                .is_some_and(|a| a.settings.find_teeth.third_molars);
        }
    }

    /// Stops the numbering in order when the case is no longer the one it
    /// started on (undo, redo, another command): the teeth it asks for may
    /// have changed.
    pub(crate) fn sequence_follow_undo(&mut self) {
        let moved = match (&self.manual.sequence, &self.manual.seq_base) {
            (Some(_), Some(base)) => !Arc::ptr_eq(base, self.undo.current()),
            _ => false,
        };
        if moved {
            self.manual.sequence = None;
            self.manual.seq_base = None;
            self.toast(Toast::new(ToastLevel::Info, "ws-manual-seq-stopped"));
        }
    }

    /// Turns the events of the pointer into commands and runs them.
    pub(crate) fn manual_events(&mut self, events: Vec<Event>) {
        let eye = self
            .view
            .viewport
            .as_ref()
            .map_or(Vec3::ZERO, |vp| vp.camera().eye());
        for ev in events {
            let state = Arc::clone(self.undo.current());
            let action = match (&ev, self.manual.sequence.is_some()) {
                (Event::Click(pick, _), true) => {
                    sequence_click(&mut self.manual, &state, pick, eye)
                }
                (_, true) => Action::None,
                (_, false) => handle(&mut self.manual, &state, &ev, eye),
            };
            self.manual_action(action);
        }
    }

    fn manual_action(&mut self, action: Action) {
        match action {
            Action::Run(id, params) => self.run_manual(id, params),
            Action::Hint(key) => self.toast(Toast::new(ToastLevel::Info, key)),
            Action::Chose(_) | Action::None => {}
        }
    }

    /// The keys of the numbering in order: `S` (skip the tooth), Backspace (back),
    /// Enter and Esc (done). Read before the other shortcuts, which would
    /// take Esc; they are left alone while a text field has the keyboard.
    pub(crate) fn sequence_keys(&mut self, ui: &Ui) {
        if self.manual.sequence.is_none() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        for (key, what) in [
            (egui::Key::S, SeqKey::Skip),
            (egui::Key::Backspace, SeqKey::Undo),
            (egui::Key::Enter, SeqKey::Finish),
            (egui::Key::Escape, SeqKey::Finish),
        ] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                let action = sequence_key(&mut self.manual, what);
                self.manual_action(action);
            }
        }
    }

    /// Runs a command of the module with `params`, keeping its report.
    pub(crate) fn run_manual(&mut self, id: &str, params: Value) {
        let Some(spec) = self.module.command_specs().into_iter().find(|s| s.id == id) else {
            self.toast(Toast::new(
                ToastLevel::Warning,
                dc_module_api::NOT_AVAILABLE_KEY,
            ));
            return;
        };
        self.run_command(Box::new(FeedbackCommand {
            spec,
            params,
            sink: Arc::clone(&self.manual.sink),
        }));
    }

    /// Shows the reports of the commands that finished since the last frame.
    pub(crate) fn manual_feedback(&mut self) {
        let reports: Vec<(String, Value)> = match self.manual.sink.lock() {
            Ok(mut s) => std::mem::take(&mut *s),
            Err(_) => return,
        };
        for (id, out) in reports {
            if let Some(toast) = feedback_toast(&id, &out) {
                self.toast(toast);
            }
            if matches!(id.as_str(), SEEDS | SEQUENCE | CLEAR_SEEDS) {
                for toast in marks_toasts(&out) {
                    self.toast(toast);
                }
            }
        }
        self.sequence_follow_undo();
    }

    /// The panel of the open tool, at the top left of the viewport.
    pub(crate) fn manual_panel(&mut self, ui: &mut Ui) {
        let Some(tool) = self.manual.tool else {
            return;
        };
        let stage = self.timeline.current;
        let has_stages = self.timeline.current > 0;
        let mut apply = false;
        let mut close = false;
        let mut seq_key = None;
        let mut seq_cancel = false;
        let mut seq_start = false;
        let mut clear = false;
        let c = *colors(ui);
        egui::Frame::new()
            .fill(c.bg_raised)
            .stroke(Stroke::new(1.0, c.border_subtle))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_max_width(260.0);
                let m = &mut self.manual;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(t(tool.title_key())).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(t("ws-manual-close")).clicked() {
                            close = true;
                        }
                    });
                });
                let hint = if m.sequence.is_some() {
                    "ws-manual-hint-seq"
                } else {
                    tool.hint_key()
                };
                ui.label(egui::RichText::new(t(hint)).color(c.text_secondary).small());
                number(ui, t("ws-manual-tooth"), &mut m.tooth, 0..=48, 1.0, "");
                match tool {
                    Tool::Brush => {
                        number(
                            ui,
                            t("ws-manual-radius"),
                            &mut m.radius_mm,
                            0.1..=8.0,
                            0.05,
                            " mm",
                        );
                    }
                    Tool::Clean => number(ui, t("ws-manual-rings"), &mut m.rings, 0..=3, 1.0, ""),
                    Tool::Seeds => {
                        if let Some(seq) = &m.sequence {
                            let line = match (seq.next(), seq.arch) {
                                (Some(f), _) => dc_i18n::tr_args(
                                    "ws-manual-seq-next",
                                    &[("fdi", &f.to_string())],
                                ),
                                (None, None) => t("ws-manual-seq-first"),
                                (None, Some(_)) => t("ws-manual-seq-done"),
                            };
                            ui.label(egui::RichText::new(line).strong());
                            ui.horizontal_wrapped(|ui| {
                                if Button::secondary(t("ws-manual-seq-skip"))
                                    .show(ui)
                                    .clicked()
                                {
                                    seq_key = Some(SeqKey::Skip);
                                }
                                if Button::secondary(t("ws-manual-seq-back"))
                                    .show(ui)
                                    .clicked()
                                {
                                    seq_key = Some(SeqKey::Undo);
                                }
                            });
                            ui.horizontal_wrapped(|ui| {
                                if Button::primary(t("ws-manual-seq-finish"))
                                    .show(ui)
                                    .clicked()
                                {
                                    seq_key = Some(SeqKey::Finish);
                                }
                                if Button::ghost(t("ws-manual-seq-cancel")).show(ui).clicked() {
                                    seq_cancel = true;
                                }
                            });
                        } else {
                            ui.checkbox(&mut m.third_molars, t("ws-manual-seq-third"));
                            if Button::secondary(t("ws-manual-seq-start"))
                                .show(ui)
                                .clicked()
                            {
                                seq_start = true;
                            }
                            if Button::ghost(t("ws-manual-clear")).show(ui).clicked() {
                                clear = true;
                            }
                        }
                    }
                    Tool::Renumber => number(ui, t("ws-manual-dst"), &mut m.dst, 0..=48, 1.0, ""),
                    Tool::Missing => {
                        ui.checkbox(&mut m.missing, t("ws-manual-is-missing"));
                    }
                    Tool::Rebuild => {
                        ui.checkbox(&mut m.resegment, t("ws-manual-resegment"));
                    }
                    Tool::Adjust => {
                        let f = &mut m.fields;
                        for (key, v, unit) in [
                            ("ws-manual-mesial", &mut f.mesial_mm, " mm"),
                            ("ws-manual-buccal", &mut f.buccal_mm, " mm"),
                            ("ws-manual-extrusion", &mut f.extrusion_mm, " mm"),
                            ("ws-manual-torque", &mut f.torque_deg, "°"),
                            ("ws-manual-tip", &mut f.tip_deg, "°"),
                            ("ws-manual-rotation", &mut f.rotation_deg, "°"),
                        ] {
                            number(ui, t(key), v, -30.0..=30.0, 0.05, unit);
                        }
                        ui.add_enabled(
                            has_stages,
                            egui::Checkbox::new(&mut m.at_stage, t("ws-manual-at-stage")),
                        );
                        ui.checkbox(&mut m.lock, t("ws-manual-lock"));
                        ui.checkbox(&mut m.refuse_on_collision, t("ws-manual-refuse"));
                    }
                }
                if !matches!(tool, Tool::Brush | Tool::Seeds)
                    && dc_ui::widgets::Button::primary(t("ws-manual-apply"))
                        .show(ui)
                        .clicked()
                {
                    apply = true;
                }
            });
        if close {
            self.manual.tool = None;
        }
        if close || seq_cancel {
            self.manual.sequence = None;
            self.manual.seq_base = None;
        }
        if seq_start {
            let state = Arc::clone(self.undo.current());
            self.manual.sequence = Some(Sequence::start(&state, self.manual.third_molars));
            self.manual.seq_base = Some(state);
        }
        if let Some(key) = seq_key {
            let action = sequence_key(&mut self.manual, key);
            self.manual_action(action);
        }
        if clear {
            self.run_manual(CLEAR_SEEDS, json!({}));
        }
        if apply {
            match params_for(&self.manual, tool, stage) {
                Ok(params) => {
                    self.run_manual(tool.command_id(), params);
                    if tool == Tool::Adjust {
                        self.manual.fields = Fields::default();
                    }
                }
                Err(key) => self.toast(Toast::new(ToastLevel::Info, key)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::{Fdi, ScanRole};
    use dc_command::JobCtx;
    use dca_model::{AlignersState, MODULE_ID};
    use glam::Vec3;

    fn exec(id: &str, state: &CaseState, params: Value) -> CaseState {
        dca_module::commands::find(id)
            .unwrap()
            .execute(state, &params, &JobCtx::detached())
            .unwrap_or_else(|e| panic!("{id}: {e}"))
            .0
    }

    fn lower_demo() -> CaseState {
        exec(
            "aligners.demo",
            &CaseState::default(),
            json!({ "arch": "lower", "severity": "mild" }),
        )
    }

    fn data(state: &CaseState) -> &AlignersState {
        state.module_data::<AlignersState>(MODULE_ID).unwrap()
    }

    fn mesh_id(state: &CaseState) -> u64 {
        scan_mesh_id(state.case.scan(ScanRole::Lower).unwrap())
    }

    /// The pick of a ray from above that hits vertex `v` of the lower scan.
    fn pick_vertex(state: &CaseState, v: usize) -> Pick {
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let mesh = state.mesh(&scan.mesh).unwrap();
        let tri = mesh
            .tris
            .iter()
            .position(|t| t.contains(&(v as u32)))
            .unwrap();
        let t = mesh.tris[tri];
        let k = t.iter().position(|&i| i == v as u32).unwrap();
        let mut bary = Vec3::ZERO;
        bary[k] = 1.0;
        let world = state.case.world_frame.from_case.to_mat4().as_mat4();
        let p = world.transform_point3(mesh.positions[v]);
        Pick {
            mesh_id: mesh_id(state),
            tri: tri as u32,
            point: p,
            normal: Vec3::Z,
            barycentric: bary,
            distance: 50.0,
        }
    }

    fn eye_above(state: &CaseState, v: usize) -> Vec3 {
        let p = pick_vertex(state, v).point;
        p + Vec3::new(0.0, 0.0, 50.0)
    }

    /// The vertex of the scan nearest to the seed of tooth `label` (its occlusal face).
    fn vertex_of(state: &CaseState, label: u16) -> usize {
        let seed = data(state)
            .seeds
            .iter()
            .find(|s| u16::from(s.fdi.get()) == label)
            .unwrap()
            .position;
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let mesh = state.mesh(&scan.mesh).unwrap();
        (0..mesh.vertex_count())
            .min_by(|&i, &j| {
                mesh.positions[i]
                    .as_dvec3()
                    .distance(seed)
                    .total_cmp(&mesh.positions[j].as_dvec3().distance(seed))
            })
            .unwrap()
    }

    fn manual(tool: Tool, tooth: u8) -> Manual {
        Manual {
            tool: Some(tool),
            tooth,
            ..Manual::default()
        }
    }

    #[test]
    fn a_pick_goes_back_to_the_vertex_label_and_point_of_the_scan() {
        let state = lower_demo();
        let v = vertex_of(&state, 36);
        let pick = pick_vertex(&state, v);
        let t = target_of(&state, &pick, eye_above(&state, v)).unwrap();
        assert_eq!(t.role, ScanRole::Lower);
        assert_eq!(t.vertex as usize, v);
        assert_eq!(t.label, 36);
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let p = state.mesh(&scan.mesh).unwrap().positions[v].as_dvec3();
        assert!(t.point.distance(p) < 1e-3, "{:?} {:?}", t.point, p);
        // looking down
        assert!(t.view_dir.z < -0.9);
        // a pick of a mesh that is not a scan finds nothing
        let mut other = pick;
        other.mesh_id = 1;
        assert!(target_of(&state, &other, Vec3::ZERO).is_none());
    }

    #[test]
    fn a_click_with_the_brush_is_the_dab_command_and_it_paints_the_scan() {
        let state = lower_demo();
        let v = vertex_of(&state, 36);
        let ev = Event::Click(pick_vertex(&state, v), Mods::default());
        let mut m = manual(Tool::Brush, 35);
        m.radius_mm = 1.5;
        let Action::Run(id, params) = handle(&mut m, &state, &ev, eye_above(&state, v)) else {
            panic!("a command");
        };
        assert_eq!(id, BRUSH);
        assert_eq!(params["arch"], "lower");
        assert_eq!(params["label"], 35);
        assert_eq!(params["radius_mm"], 1.5);
        assert_eq!(params["centre"].as_array().unwrap().len(), 3);
        // the command accepts exactly these parameters
        let after = exec(id, &state, params);
        let before = state.mesh(&data(&state).labels[&ScanRole::Lower]).unwrap();
        let now = after.mesh(&data(&after).labels[&ScanRole::Lower]).unwrap();
        let (b, n) = (
            before.vertex_labels.as_ref().unwrap(),
            now.vertex_labels.as_ref().unwrap(),
        );
        assert_eq!(n[v], 35);
        assert!(b.iter().zip(n).filter(|(a, c)| a != c).count() > 3);
    }

    #[test]
    fn shift_paints_the_gingiva_and_ctrl_fills_the_crown() {
        let state = lower_demo();
        let v = vertex_of(&state, 36);
        let pick = pick_vertex(&state, v);
        let eye = eye_above(&state, v);
        let mut m = manual(Tool::Brush, 36);
        let shift = Event::Click(
            pick,
            Mods {
                shift: true,
                ctrl: false,
            },
        );
        let Action::Run(_, p) = handle(&mut m, &state, &shift, eye) else {
            panic!()
        };
        assert_eq!(p["label"], 0);
        assert!(p.get("mode").is_none());
        let ctrl = Event::Click(
            pick,
            Mods {
                shift: false,
                ctrl: true,
            },
        );
        let Action::Run(id, p) = handle(&mut m, &state, &ctrl, eye) else {
            panic!()
        };
        assert_eq!(
            (id, p["mode"].as_str(), p["vertex"].as_u64()),
            (BRUSH, Some("fill"), Some(v as u64))
        );
        let after = exec(id, &state, p);
        assert!(data(&after).labels.contains_key(&ScanRole::Lower));
        // Ctrl+Shift fills with the gingiva
        let both = Event::Click(
            pick,
            Mods {
                shift: true,
                ctrl: true,
            },
        );
        let Action::Run(_, p) = handle(&mut m, &state, &both, eye) else {
            panic!()
        };
        assert_eq!(
            (p["mode"].as_str(), p["label"].as_u64()),
            (Some("fill"), Some(0))
        );
    }

    #[test]
    fn a_drag_is_one_stroke_command_through_all_the_picks() {
        let state = lower_demo();
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        let mesh = state.mesh(&scan.mesh).unwrap();
        let v = vertex_of(&state, 36);
        // three picks on tooth 36 along the surface
        let near: Vec<usize> = (0..mesh.vertex_count())
            .filter(|&i| mesh.positions[i].distance(mesh.positions[v]) < 3.0)
            .step_by(50)
            .take(3)
            .collect();
        assert_eq!(near.len(), 3);
        let picks: Vec<Pick> = near.iter().map(|&i| pick_vertex(&state, i)).collect();
        let mut m = manual(Tool::Brush, 36);
        let ev = Event::Stroke(picks, Mods::default());
        let Action::Run(id, p) = handle(&mut m, &state, &ev, eye_above(&state, v)) else {
            panic!()
        };
        assert_eq!(id, BRUSH);
        assert_eq!(p["stroke"].as_array().unwrap().len(), 2, "{p}");
        assert_eq!(p["centre"], p["centre"]);
        exec(id, &state, p);
    }

    #[test]
    fn the_seed_tool_adds_moves_and_deletes_seeds() {
        let mut state = lower_demo();
        let v = vertex_of(&state, 36);
        // remove the seed of 36, then click on its crown with the tool set to 36: add
        let mut d = data(&state).clone();
        d.seeds.retain(|s| s.fdi != Fdi::new(36).unwrap());
        state.set_module(MODULE_ID, dc_case::ModuleState::new(d));
        let pick = pick_vertex(&state, v);
        let eye = eye_above(&state, v);
        let mut m = manual(Tool::Seeds, 36);
        let Action::Run(id, p) = handle(&mut m, &state, &Event::Click(pick, Mods::default()), eye)
        else {
            panic!()
        };
        assert_eq!(
            (id, p["op"].as_str(), p["fdi"].as_u64()),
            (SEEDS, Some("add"), Some(36))
        );
        let added = exec(id, &state, p);
        assert!(data(&added).seeds.iter().any(|s| s.fdi.get() == 36));
        // a change of the marks keeps the colours of the arch (out of date), so
        // the clicks below still read the tooth under the pointer
        assert_eq!(data(&added).labels, data(&state).labels);
        // clicking again moves it; tooth 0 takes the number of the tooth under the click
        let mut m = manual(Tool::Seeds, 0);
        let Action::Run(_, p) = handle(&mut m, &added, &Event::Click(pick, Mods::default()), eye)
        else {
            panic!()
        };
        assert_eq!(
            (p["op"].as_str(), p["fdi"].as_u64()),
            (Some("move"), Some(36))
        );
        exec(SEEDS, &added, p);
        // Shift+click near a seed deletes it; far from any seed there is nothing to delete
        let shift = Event::Click(
            pick,
            Mods {
                shift: true,
                ctrl: false,
            },
        );
        let Action::Run(_, p) = handle(&mut m, &added, &shift, eye) else {
            panic!()
        };
        assert_eq!(
            (p["op"].as_str(), p["fdi"].as_u64()),
            (Some("delete"), Some(36))
        );
        let gone = exec(SEEDS, &added, p);
        assert!(data(&gone).seeds.iter().all(|s| s.fdi.get() != 36));
        assert_eq!(
            handle(&mut m, &gone, &shift, eye),
            Action::Hint("ws-manual-no-seed")
        );
        // no tooth chosen and the click on gingiva: ask for a tooth
        let gum = (0..mesh_len(&state))
            .find(|&i| label_at(&state, i) == 0)
            .unwrap();
        let g = Event::Click(pick_vertex(&state, gum), Mods::default());
        assert_eq!(
            handle(&mut m, &state, &g, eye),
            Action::Hint("ws-manual-no-tooth")
        );
    }

    fn mesh_len(state: &CaseState) -> usize {
        let scan = state.case.scan(ScanRole::Lower).unwrap();
        state.mesh(&scan.mesh).unwrap().vertex_count()
    }

    fn label_at(state: &CaseState, v: usize) -> u16 {
        state
            .mesh(&data(state).labels[&ScanRole::Lower])
            .unwrap()
            .vertex_labels
            .as_ref()
            .unwrap()[v]
    }

    #[test]
    fn clicking_a_tooth_chooses_it_for_the_picking_tools() {
        let state = lower_demo();
        let v = vertex_of(&state, 41);
        let ev = Event::Click(pick_vertex(&state, v), Mods::default());
        for tool in [
            Tool::Clean,
            Tool::Renumber,
            Tool::Missing,
            Tool::Rebuild,
            Tool::Adjust,
        ] {
            let mut m = manual(tool, 0);
            assert_eq!(handle(&mut m, &state, &ev, Vec3::ZERO), Action::Chose(41));
            assert_eq!(m.tooth, 41);
        }
        // a stroke does not choose anything
        let mut m = manual(Tool::Adjust, 0);
        let s = Event::Stroke(vec![pick_vertex(&state, v)], Mods::default());
        assert_eq!(handle(&mut m, &state, &s, Vec3::ZERO), Action::None);
        // a closed tool ignores the pointer
        m.tool = None;
        assert_eq!(handle(&mut m, &state, &ev, Vec3::ZERO), Action::None);
    }

    #[test]
    fn the_panels_build_the_parameters_of_their_commands() {
        let mut m = manual(Tool::Renumber, 31);
        assert_eq!(params_for(&m, Tool::Renumber, 0), Err("ws-manual-no-tooth"));
        m.dst = 41;
        assert_eq!(
            params_for(&m, Tool::Renumber, 0),
            Ok(json!({ "src": 31, "dst": 41 }))
        );
        m.missing = false;
        assert_eq!(
            params_for(&m, Tool::Missing, 0),
            Ok(json!({ "fdi": 31, "missing": false }))
        );
        m.resegment = true;
        assert_eq!(
            params_for(&m, Tool::Rebuild, 0),
            Ok(json!({ "fdi": 31, "resegment": true }))
        );
        assert_eq!(
            params_for(&m, Tool::Clean, 0),
            Ok(json!({ "rings": 1, "fdi": 31 }))
        );
        m.tooth = 0;
        assert_eq!(params_for(&m, Tool::Clean, 0), Ok(json!({ "rings": 1 })));
        // adjust: only the non-zero components, the stage shown when asked
        let mut m = manual(Tool::Adjust, 33);
        assert_eq!(
            params_for(&m, Tool::Adjust, 0),
            Err("ws-manual-no-movement")
        );
        m.fields.mesial_mm = 0.5;
        m.fields.rotation_deg = -3.0;
        assert_eq!(
            params_for(&m, Tool::Adjust, 2),
            Ok(json!({ "fdi": 33, "mesial_mm": 0.5, "rotation_deg": -3.0 }))
        );
        m.at_stage = true;
        m.refuse_on_collision = true;
        let p = params_for(&m, Tool::Adjust, 2).unwrap();
        assert_eq!(
            (p["stage"].as_u64(), p["refuse_on_collision"].as_bool()),
            (Some(2), Some(true))
        );
        // the stage 0 is the initial situation: the setup is adjusted
        assert!(
            params_for(&m, Tool::Adjust, 0)
                .unwrap()
                .get("stage")
                .is_none()
        );
    }

    #[test]
    fn the_adjust_panel_runs_the_command_and_the_collision_is_reported() {
        let state = exec("aligners.teeth.frames", &lower_demo(), Value::Null);
        let mut m = manual(Tool::Adjust, 32);
        m.fields.mesial_mm = 3.0;
        let params = params_for(&m, Tool::Adjust, 0).unwrap();
        let (after, out) = dca_module::commands::find(ADJUST)
            .unwrap()
            .execute(&state, &params, &JobCtx::detached())
            .unwrap();
        assert!(
            data(&after)
                .setup
                .poses
                .contains_key(&Fdi::new(32).unwrap())
        );
        let toast = feedback_toast(ADJUST, &out).unwrap();
        assert_eq!(toast.level, ToastLevel::Warning);
        assert_eq!(toast.message_key, "ws-manual-collision");
        assert!(toast.detail.unwrap().contains("31"));
        // a move without collision is just reported
        let mut m = manual(Tool::Adjust, 32);
        m.fields.extrusion_mm = 0.2;
        let params = params_for(&m, Tool::Adjust, 0).unwrap();
        let (_, out) = dca_module::commands::find(ADJUST)
            .unwrap()
            .execute(&state, &params, &JobCtx::detached())
            .unwrap();
        // the demo is crowded: whatever is reported, it is a toast about the adjustment
        assert!(
            feedback_toast(ADJUST, &out)
                .unwrap()
                .message_key
                .starts_with("ws-manual-")
        );
        assert!(feedback_toast("aligners.unknown", &out).is_none());
    }

    const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

    fn workspace(state: CaseState) -> Workspace {
        let project = dc_project::Project::new(
            dc_project::AppInfo {
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

    fn labels_ref(ws: &Workspace) -> dc_case::BlobRef {
        data(ws.undo.current()).labels[&ScanRole::Lower]
    }

    #[test]
    fn a_pointer_event_runs_the_command_through_the_workspace_and_undo_restores_it() {
        let state = lower_demo();
        let v = vertex_of(&state, 36);
        let pick = pick_vertex(&state, v);
        let mut ws = workspace(state);
        let before = labels_ref(&ws);
        // the menu entry opens the tool instead of running the command
        ws.activate(BRUSH);
        assert_eq!(ws.selected(), Some(BRUSH));
        assert_eq!(ws.manual.tool, Some(Tool::Brush));
        assert!(ws.toast_keys().is_empty());
        assert!(ws.wait_for_jobs(WAIT));
        assert_eq!(labels_ref(&ws), before, "nothing ran");
        // a click paints tooth 35 on the scan
        ws.manual.tooth = 35;
        ws.manual_events(vec![Event::Click(pick, Mods::default())]);
        assert!(ws.wait_for_jobs(WAIT));
        let after = labels_ref(&ws);
        assert_ne!(after, before);
        let st = ws.undo.current().clone();
        assert_eq!(label_at(&st, v), 35);
        // one step on the undo stack
        assert!(ws.undo());
        assert_eq!(labels_ref(&ws), before);
        assert!(ws.redo());
        assert_eq!(labels_ref(&ws), after);
        // a click on nothing the tools know does nothing
        let mut other = pick;
        other.mesh_id = 3;
        ws.manual_events(vec![Event::Click(other, Mods::default())]);
        assert!(ws.wait_for_jobs(WAIT));
        assert_eq!(labels_ref(&ws), after);
    }

    #[test]
    fn the_adjust_panel_applies_a_move_and_warns_about_the_collision() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        struct App {
            ws: Workspace,
        }
        let state = exec("aligners.teeth.frames", &lower_demo(), Value::Null);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1600.0, 900.0))
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui_state(
                |ui, app: &mut App| {
                    app.ws.show(ui);
                },
                App {
                    ws: workspace(state),
                },
            );
        harness.state_mut().ws.activate(ADJUST);
        harness.run();
        // the panel of the tool is on screen
        harness.get_by_label("Mesial");
        {
            let ws = &mut harness.state_mut().ws;
            ws.manual.tooth = 32;
            ws.manual.fields.mesial_mm = 3.0;
        }
        harness.run();
        harness.get_by_label("Apply").click();
        // the click starts a job and the app keeps repainting while it runs:
        // take one frame, wait for the job, then let the UI settle
        harness.step();
        assert!(harness.state_mut().ws.wait_for_jobs(WAIT));
        harness.run();
        // the move is stored and undoable
        let poses = |ws: &Workspace| data(ws.undo.current()).setup.poses.len();
        assert_eq!(poses(&harness.state().ws), 1);
        harness.run();
        assert!(
            harness
                .state()
                .ws
                .toast_keys()
                .contains(&"ws-manual-collision".to_owned()),
            "{:?}",
            harness.state().ws.toast_keys()
        );
        assert!(harness.state_mut().ws.undo());
        assert_eq!(poses(&harness.state().ws), 0);
    }

    #[test]
    fn the_tools_open_from_their_menu_entries() {
        for tool in [
            Tool::Brush,
            Tool::Clean,
            Tool::Seeds,
            Tool::Renumber,
            Tool::Missing,
            Tool::Rebuild,
            Tool::Adjust,
        ] {
            assert_eq!(Tool::from_command(tool.command_id()), Some(tool));
            assert!(dca_module::commands::is_registered(tool.command_id()));
        }
        assert_eq!(Tool::from_command("aligners.demo"), None);
    }

    // ---- numbering in order -------------------------------------------------------------------

    fn cleared_demo() -> CaseState {
        exec(CLEAR_SEEDS, &lower_demo(), json!({}))
    }

    fn click_vertex(m: &mut Manual, state: &CaseState, v: usize) -> Action {
        sequence_click(m, state, &pick_vertex(state, v), eye_above(state, v))
    }

    #[test]
    fn the_numbering_in_order_asks_for_the_teeth_from_the_right_distal() {
        let state = cleared_demo();
        let v = vertex_of(&lower_demo(), 36);
        let w = vertex_of(&lower_demo(), 35);
        let mut m = manual(Tool::Seeds, 0);
        m.sequence = Some(Sequence::start(&state, false));
        // one arch in the case: it is known from the start; no third molar
        let seq = m.sequence.as_ref().unwrap();
        assert_eq!(seq.arch, Some("lower"));
        assert_eq!(seq.next(), Some(47));
        // a click puts the next tooth, S skips the next one (it is not marked missing)
        assert_eq!(click_vertex(&mut m, &state, v), Action::None);
        assert_eq!(sequence_key(&mut m, SeqKey::Skip), Action::None);
        assert_eq!(click_vertex(&mut m, &state, w), Action::None);
        let seq = m.sequence.as_ref().unwrap();
        assert_eq!(
            seq.steps.iter().map(|s| s.fdi).collect::<Vec<_>>(),
            [47, 46, 45]
        );
        assert_eq!(seq.next(), Some(44));
        // Backspace takes the last step back, and the tooth is asked for again
        assert_eq!(sequence_key(&mut m, SeqKey::Undo), Action::None);
        assert_eq!(m.sequence.as_ref().unwrap().next(), Some(45));
        // nothing has changed in the case until it is finished
        assert!(data(&state).seeds.is_empty());
        // Enter: one command with the steps, and the numbering is over
        let Action::Run(id, params) = sequence_key(&mut m, SeqKey::Finish) else {
            panic!("a command");
        };
        assert_eq!(id, SEQUENCE);
        assert!(m.sequence.is_none());
        assert_eq!(params["arch"], "lower");
        assert_eq!(params["steps"][0]["kind"], "point");
        // each step names the tooth it was for: the module refuses a mismatch
        assert_eq!(params["steps"][0]["fdi"], 47);
        assert_eq!(params["steps"][1], json!({ "kind": "skip", "fdi": 46 }));
        assert_eq!(params["steps"].as_array().unwrap().len(), 2);
        // the command takes exactly these parameters
        let after = exec(id, &state, params);
        let nums: Vec<u8> = data(&after).seeds.iter().map(|s| s.fdi.get()).collect();
        assert_eq!(nums, [47]);
        assert!(data(&after).missing.is_empty(), "S skips the tooth only");
        // finishing without a step sends nothing
        m.sequence = Some(Sequence::start(&state, false));
        assert_eq!(sequence_key(&mut m, SeqKey::Finish), Action::None);
        assert!(m.sequence.is_none());
    }

    #[test]
    fn the_numbering_in_order_skips_the_teeth_that_are_taken_and_ends_with_the_arch() {
        // the demo has all its marks: nothing is left to number, a click or S ends it
        let state = lower_demo();
        let v = vertex_of(&state, 36);
        let mut m = manual(Tool::Seeds, 0);
        m.sequence = Some(Sequence::start(&state, false));
        assert_eq!(m.sequence.as_ref().unwrap().next(), None);
        assert_eq!(sequence_key(&mut m, SeqKey::Skip), Action::None);
        assert!(m.sequence.is_none());
        // the last tooth of the order finishes by itself
        let state = cleared_demo();
        m.sequence = Some(Sequence::start(&state, true));
        m.sequence.as_mut().unwrap().order = vec![48];
        let Action::Run(id, params) = click_vertex(&mut m, &state, v) else {
            panic!("the last click finishes");
        };
        assert_eq!(id, SEQUENCE);
        assert_eq!(params["third_molars"], true);
        assert!(m.sequence.is_none());
        // no numbering open: clicks and keys do nothing
        assert_eq!(click_vertex(&mut m, &state, v), Action::None);
        assert_eq!(sequence_key(&mut m, SeqKey::Skip), Action::None);
    }

    #[test]
    fn the_numbering_in_order_with_two_arches_waits_for_the_first_click() {
        let state = exec(
            "aligners.demo",
            &lower_demo(),
            json!({ "arch": "upper", "severity": "mild" }),
        );
        let state = exec(CLEAR_SEEDS, &state, json!({}));
        let mut m = manual(Tool::Seeds, 0);
        m.sequence = Some(Sequence::start(&state, false));
        let seq = m.sequence.as_ref().unwrap();
        assert_eq!((seq.arch, seq.next()), (None, None));
        // S before the arch is known asks for a click
        assert_eq!(
            sequence_key(&mut m, SeqKey::Skip),
            Action::Hint("ws-manual-seq-first")
        );
        // the first click on the lower scan chooses the arch
        let lower = state.case.scan(ScanRole::Lower).unwrap();
        let v = state.mesh(&lower.mesh).unwrap().tris[0][0] as usize;
        assert_eq!(click_vertex(&mut m, &state, v), Action::None);
        let seq = m.sequence.as_ref().unwrap();
        assert_eq!((seq.arch, seq.next()), (Some("lower"), Some(46)));
    }

    #[test]
    fn the_steps_sent_are_refused_when_the_marks_changed_meanwhile() {
        let state = cleared_demo();
        let v = vertex_of(&lower_demo(), 36);
        let mut m = manual(Tool::Seeds, 0);
        m.sequence = Some(Sequence::start(&state, false));
        assert_eq!(click_vertex(&mut m, &state, v), Action::None);
        let Action::Run(id, params) = sequence_key(&mut m, SeqKey::Finish) else {
            panic!("a command");
        };
        // the same steps on the case they were made on: fine
        assert!(data(&exec(id, &state, params.clone())).seeds.len() == 1);
        // after 47 got a mark some other way, the step for 47 is for 46 now
        let other = exec(id, &state, params.clone());
        let spec = dca_module::commands::find(id).unwrap();
        let err = spec
            .execute(&other, &params, &JobCtx::detached())
            .unwrap_err()
            .to_string();
        assert!(err.contains("47") && err.contains("46"), "{err}");
    }

    #[test]
    fn the_notices_of_the_marks_name_what_was_dropped_or_left_out_of_date() {
        dca_module::loader();
        // marks edited by hand: kept, out of date, in the words of the import
        let out = json!({ "invalidated": ["labels", "tooth_frames"] });
        let t = marks_toasts(&out);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].message_key, "ws-toast-marks-outdated");
        let detail = t[0].detail.clone().unwrap_or_default();
        assert!(
            detail.contains("teeth and gum") && detail.contains("tooth axes"),
            "{detail}"
        );
        // Find the teeth: what the numbering dropped (per arch, once each) and
        // the arch that failed
        let out = json!({ "steps": [
            { "id": "aligners.preparation.numbering.automatic", "output": {
                "arches": [
                    { "arch": "lower", "dropped": ["labels", "setup"] },
                    { "arch": "upper", "dropped": ["labels"] },
                ],
                "failed": [{ "arch": "upper", "error": "no dental arch found in the upper scan" }],
            } },
            { "id": "aligners.preparation.orientation.automatic", "output": {} },
        ] });
        let t = marks_toasts(&out);
        let keys: Vec<&str> = t.iter().map(|t| t.message_key.as_str()).collect();
        assert_eq!(
            keys,
            ["ws-toast-marks-dropped", "ws-toast-find-arch-failed"]
        );
        let detail = t[0].detail.clone().unwrap_or_default();
        assert_eq!(detail.matches("teeth and gum").count(), 1, "{detail}");
        assert!(detail.contains("setup"), "{detail}");
        assert!(
            t[1].detail
                .clone()
                .unwrap_or_default()
                .contains("upper scan")
        );
        // nothing changed: no notice
        assert!(marks_toasts(&json!({ "invalidated": [], "dropped": [] })).is_empty());
        assert!(marks_toasts(&json!({ "arches": [{ "dropped": [] }], "failed": [] })).is_empty());
    }
}
