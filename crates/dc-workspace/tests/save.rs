//! Saving keeps everything (the report of 2026-11-13: brush strokes and tooth
//! marks seemed gone after reopening the project).
//!
//! - Round trip: every command that can change the case runs through the
//!   workspace as the window runs it, the project is saved with Save (Ctrl+S)
//!   and opened again as Home opens it, and the case, the data of the module,
//!   every mesh they use and the step history come back equal; undo after an
//!   edit too. Every registered command must be listed, so a new one cannot
//!   be left out of the round trip without a reason.
//! - The automatic save on a simulated clock: two quiet seconds, restarted by
//!   each change, never while a job changes the case (it follows the job),
//!   never for a project without a file, a failed save is shown.
//! - A change that lands after Save is pressed is saved too, and leaving or
//!   closing the window saves first.
//! - The step history follows the case: the runs and the corrections by hand
//!   are recorded, what they leave out of date goes, undo and redo bring back
//!   the history of their version, a function that leaves nothing in the case
//!   is done on the case its run left, a history the CLI saved with records
//!   of results the case no longer holds marks nothing done, and one that
//!   cannot be read keeps the file read-only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use dc_case::{CaseState, ScanRole};
use dc_command::{Command, CommandError, JobCtx, StepDef, StepGraph, StepHistory, StepOutcome};
use dc_mesh::{MeshFormat, TriMesh};
use dc_module_api::{CommandSpec, Module};
use dc_project::{AppInfo, Project};
use dc_workspace::autosave::DELAY_S;
use dc_workspace::{Workspace, WorkspaceAction};
use dca_model::{AlignersState, ClosedBy, MODULE_ID};
use dca_module::AlignersModule;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(900);

fn module() -> Box<dyn Module> {
    Box::new(AlignersModule::new())
}

fn demo_state(arches: &[&str]) -> CaseState {
    let mut state = CaseState::default();
    for arch in arches {
        state = dca_module::commands::find("aligners.demo")
            .unwrap()
            .execute(&state, &json!({ "arch": arch }), &JobCtx::detached())
            .unwrap()
            .0;
    }
    state
}

/// A step history with a step done automatically and one done by hand (at
/// a fixed time, so it can be compared).
fn step_history() -> Value {
    let g = StepGraph::new(vec![
        StepDef {
            id: "aligners.teeth.find",
            depends: &[],
        },
        StepDef {
            id: "aligners.preparation.segmentation.automatic",
            depends: &["aligners.teeth.find"],
        },
    ])
    .unwrap();
    let mut h = StepHistory::new();
    h.checkpoint(&g, "aligners.teeth.find", StepOutcome::default())
        .unwrap();
    h.mark_manual(
        &g,
        "aligners.preparation.segmentation.automatic",
        StepOutcome::default(),
    )
    .unwrap();
    let mut v = serde_json::to_value(&h).unwrap();
    for record in v["steps"].as_object_mut().unwrap().values_mut() {
        record["at"] = json!(1_794_600_000_u64);
    }
    v
}

fn project(state: CaseState) -> Project {
    let mut p = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    );
    p.history = step_history();
    p
}

/// The project at `path` as Home opens it.
fn reopen(path: &Path) -> Project {
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(path, &codecs).unwrap();
    assert!(!loaded.read_only, "{:?}", loaded.issues);
    loaded.project
}

/// A workspace on `state` with a file at `path` (saved once).
fn saved(state: CaseState, path: &Path) -> Workspace {
    let mut ws = Workspace::new_headless(module(), Some(project(state)), None);
    ws.save_to(path.to_path_buf());
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.to_path_buf()));
    ws
}

/// `a` and `b` are the same number, up to the last digits JSON text keeps.
fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0)
}

/// Where `a` and `b` differ (numbers compared with [`close`]), if they do.
fn json_diff(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) if close(x, y) => None,
            _ => Some(format!("{at}: {x} != {y}")),
        },
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{at}: {} != {} items", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (x, y))| json_diff(x, y, &format!("{at}/{i}")))
        }
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|k| match (x.get(k), y.get(k)) {
                (Some(x), Some(y)) => json_diff(x, y, &format!("{at}/{k}")),
                _ => Some(format!("{at}/{k}: only on one side")),
            })
        }
        _ if a == b => None,
        _ => Some(format!("{at}: {a} != {b}")),
    }
}

/// `file` holds the same case as `mem`: the case, the data of every module
/// and every mesh they use.
fn assert_same(file: &CaseState, mem: &CaseState, what: &str) {
    let case = |s: &CaseState| serde_json::to_value(&s.case).unwrap();
    if let Some(d) = json_diff(&case(file), &case(mem), "case") {
        panic!("{what}: {d}");
    }
    let ids = |s: &CaseState| s.modules.keys().cloned().collect::<Vec<_>>();
    assert_eq!(ids(file), ids(mem), "{what}: modules");
    for (id, m) in &mem.modules {
        let (f, m) = (file.modules[id].to_json().unwrap(), m.to_json().unwrap());
        if let Some(d) = json_diff(&f, &m, id) {
            panic!("{what}: {d}");
        }
    }
    let refs = |s: &CaseState| s.referenced_blobs().into_iter().collect::<BTreeSet<_>>();
    assert_eq!(refs(file), refs(mem), "{what}: meshes");
    for r in refs(mem) {
        assert_eq!(file.mesh(&r), mem.mesh(&r), "{what}: mesh {r}");
    }
}

/// Saves with Save (Ctrl+S), opens the file again and checks that it holds
/// the case of the workspace and its step history.
fn save_and_compare(ws: &mut Workspace, path: &Path, what: &str) {
    ws.save();
    assert!(ws.wait_for_jobs(WAIT), "{what}: save");
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.to_path_buf()));
    assert!(!ws.is_dirty(), "{what}: still unsaved");
    let errors = ws.sticky_toast_keys();
    assert!(errors.is_empty(), "{what}: {:?}", ws.toast_details());
    let p = reopen(path);
    assert_same(&p.case_state, ws.state(), what);
    assert_eq!(p.history, history(ws), "{what}: step history");
}

/// The step history of `ws` as its file holds it.
fn history(ws: &Workspace) -> Value {
    serde_json::to_value(ws.history()).unwrap()
}

/// The registered command `id`: the module's, or the shell's import and role
/// change the workspace runs.
fn spec(ws: &Workspace, id: &str) -> CommandSpec {
    ws.module()
        .command_specs()
        .into_iter()
        .chain(dc_session::builtins::specs())
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no command {id}"))
}

/// Runs `id` with `params` in the background as the window does and waits:
/// it lands as one step on the undo stack, without an error.
fn run(ws: &mut Workspace, id: &str, params: Value) {
    let before = Arc::clone(ws.state());
    let shown = ws.toast_keys().len();
    let spec = spec(ws, id);
    ws.run_command(Box::new(spec.command(params.clone())));
    assert!(ws.wait_for_jobs(WAIT), "{id}");
    let new: Vec<_> = ws.toast_details().split_off(shown);
    assert!(
        !Arc::ptr_eq(&before, ws.state()),
        "{id} {params}: nothing landed {new:?}"
    );
    assert!(ws.sticky_toast_keys().is_empty(), "{id} {params}: {new:?}");
}

fn aligners(ws: &Workspace) -> AlignersState {
    ws.state()
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone()
}

/// The vertices of the lower scan with label `fdi` (`0` = gingiva), at most
/// `n` of them.
fn lower_vertices(ws: &Workspace, fdi: u16, n: usize) -> Vec<u32> {
    let blob = aligners(ws).labels[&ScanRole::Lower];
    let labels = ws
        .state()
        .mesh(&blob)
        .unwrap()
        .vertex_labels
        .clone()
        .unwrap();
    labels
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == fdi)
        .take(n)
        .map(|(i, _)| i as u32)
        .collect()
}

/// A built tooth of the lower arch, after `skip` others.
fn lower_crown(ws: &Workspace, skip: usize) -> u8 {
    aligners(ws)
        .crowns
        .keys()
        .filter(|f| !f.is_upper())
        .nth(skip)
        .unwrap()
        .get()
}

fn seed_position(ws: &Workspace, fdi: u8) -> [f64; 3] {
    aligners(ws)
        .seeds
        .iter()
        .find(|s| s.fdi.get() == fdi)
        .unwrap()
        .position
        .to_array()
}

/// A small closed mesh 40 mm wide, saved as STL at `dir/name`.
fn write_tetra(dir: &Path, name: &str) -> PathBuf {
    let s = 40.0;
    let mesh = TriMesh::new(
        vec![
            Vec3::ZERO,
            Vec3::new(s, 0.0, 0.0),
            Vec3::new(0.0, s, 0.0),
            Vec3::new(0.0, 0.0, s),
        ],
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
    );
    let path = dir.join(name);
    dc_mesh::io::write(&path, &mesh, MeshFormat::Stl).unwrap();
    path
}

/// The lower scan as an STL file: the scan of a control visit.
fn write_lower_scan(ws: &Workspace, dir: &Path) -> PathBuf {
    let scan = ws.state().case.scan(ScanRole::Lower).unwrap();
    let mut mesh = TriMesh::clone(ws.state().mesh(&scan.mesh).unwrap());
    mesh.vertex_labels = None;
    let path = dir.join("visit-lower.stl");
    dc_mesh::io::write(&path, &mesh, MeshFormat::Stl).unwrap();
    path
}

/// The parameters a step runs with: they depend on the case it finds.
type Params = fn(&Workspace, &Path) -> Value;

/// What the round trip does with a command.
enum Plan {
    /// Runs it with these parameters through the workspace, then saves,
    /// opens the file again and compares.
    Run(Params),
    /// Not run here, and why.
    Skip(&'static str),
}

/// It writes files (print models, the export package, the reports) and gives
/// the case back as it got it: nothing of the case to save.
const FILES: &str = "writes files only; the case it returns is the one it got";

/// It lists what the case holds (the aligners to print and their print
/// status) and gives the case back as it got it: nothing of the case to save.
const LISTS: &str =
    "lists the aligners and their print status; the case it returns is the one it got";

/// A command of the scan window that acts on the scanner or on the scan
/// being edited, outside the case: nothing of the case to save.
const SCANNER: &str = "acts on the scanner and the scan being edited, not on the case";
/// A command of the scan window that acts on the turntable, outside the
/// case: nothing of the case to save.
const TURNTABLE: &str = "acts on the turntable, not on the case";
/// The calibration of the rig is a file of the computer (scanner and
/// turntable), not of the case.
const RIG: &str =
    "stores the calibration of the scanner and turntable on this computer, not in the case";

/// The registration of a bite needs the bite scans of a patient that fit
/// both arches; what it changes (a scan moved, the flag of the bite) is the
/// same data the other steps change and the round trip compares.
const BITE_SCANS: &str = "needs the bite scans of a patient";

/// A move of `mm` of the upper arch towards the lower one (from the centre
/// of the upper scan to the centre of the lower one).
fn towards_lower(ws: &Workspace, mm: f32) -> [f32; 3] {
    let centre = |role: ScanRole| {
        let s = ws.state();
        let m = s.mesh(&s.case.scan(role).unwrap().mesh).unwrap();
        m.positions.iter().copied().sum::<glam::Vec3>() / m.positions.len() as f32
    };
    let d = (centre(ScanRole::Lower) - centre(ScanRole::Upper)).normalize() * mm;
    d.to_array()
}

fn null(_: &Workspace, _: &Path) -> Value {
    Value::Null
}

/// Every command that can change the case, in the order the round trip runs
/// them (each on the case the previous ones left), or why it is not run.
fn plan() -> Vec<(&'static str, Plan)> {
    use Plan::{Run, Skip};
    vec![
        // the settings of the case
        (
            "aligners.settings.find-teeth",
            Run(|_, _| json!({ "third_molars": true, "missing": [18] })),
        ),
        (
            "aligners.settings.segmentation",
            Run(|_, _| json!({ "sigma": 2.5 })),
        ),
        (
            "aligners.settings.study-models",
            Run(|_, _| json!({ "style": "abo", "total_height_mm": 80.0 })),
        ),
        // the parameters of the final position and the steps of Move a tooth
        (
            "aligners.settings.setup",
            Run(
                |_, _| json!({ "ipr_zone": "anterior", "advance_max_mm": 1.5, "midline": "balance", "adjust_step_mm": 0.2 }),
            ),
        ),
        // the parameters of the aligners and of their calendar
        (
            "aligners.settings.staging",
            Run(
                |_, _| json!({ "pattern": "proportional", "step_linear_mm": 0.2, "overcorrection": true, "interval_policy": "adaptive", "interval_days": 10, "start_date": "2026-11-16" }),
            ),
        ),
        // the production settings: trim, shell, stamp, bite, ramps, quality
        (
            "aligners.settings.production",
            Run(|_, _| {
                json!({
                    "trim": { "mode": "scalloped", "margin_gap_mm": 1.0, "insertion_axis": "common" },
                    "shell": { "shell_thickness_mm": 0.8 },
                    "stamp": { "tooth": "M7L", "code": "CASE-1", "date": "start" },
                    "bite": { "mode": "manual", "thickness_mm": 0.6 },
                    "ramps": { "width_mm": 4.0 },
                    "quality": "light",
                })
            }),
        ),
        // a new demo arch in the place of the lower one
        (
            "aligners.demo",
            Run(|_, _| json!({ "arch": "lower", "seed": 5 })),
        ),
        // the steps, one by one
        ("aligners.case.detect-arch", Run(null)),
        ("aligners.frame", Run(|_, _| json!({}))),
        ("aligners.teeth.find", Run(|_, _| json!({}))),
        (
            "aligners.preparation.numbering.automatic",
            Run(|_, _| json!({ "arch": "lower" })),
        ),
        (
            "aligners.preparation.orientation.automatic",
            Run(|_, _| json!({})),
        ),
        // both arches: the new tooth marks left their colours out of date
        (
            "aligners.preparation.segmentation.automatic",
            Run(|_, _| json!({})),
        ),
        (
            "aligners.preparation.segmentation.check",
            Run(|_, _| json!({ "arch": "lower" })),
        ),
        ("aligners.teeth.frames", Run(|_, _| json!({}))),
        (
            "aligners.preparation.reconstruction.automatic",
            Run(|_, _| json!({ "arch": "lower" })),
        ),
        (
            "aligners.preparation.gingiva",
            Run(|_, _| json!({ "arch": "lower" })),
        ),
        ("aligners.teeth.build", Run(|_, _| json!({}))),
        ("aligners.diagnosis.space-analysis", Run(|_, _| json!({}))),
        ("aligners.diagnosis.little-index", Run(|_, _| json!({}))),
        ("aligners.diagnosis.bolton", Run(|_, _| json!({}))),
        ("aligners.diagnosis.complexity", Run(|_, _| json!({}))),
        // the clinician's own values over the computed ones: the complexity follows
        (
            "aligners.diagnosis.edit-values",
            Run(
                |_, _| json!({ "arch": "lower", "values": { "discrepancy": -9.5, "little": 11.0, "midline_offset": 3.0 } }),
            ),
        ),
        // with both arches the bite comes before the final position
        ("aligners.occlusion.virtual-bite", Run(|_, _| json!({}))),
        // the upper arch pushed into the lower one, then the interpenetration
        // corrected: the arch moved and the report of the bite with it
        (
            "aligners.components.transform",
            Run(|ws, _| {
                let t = towards_lower(ws, 0.4);
                json!({ "component": "scan", "arch": "upper", "move": { "t": t, "q": [0.0, 0.0, 0.0, 1.0] } })
            }),
        ),
        ("aligners.occlusion.bite-correct", Run(|_, _| json!({}))),
        // the aligner ends one tooth early on the lower right: the final
        // position keeps the tooth behind it at rest and records the end
        (
            "aligners.production.aligner-end",
            Run(|ws, _| {
                let mut right: Vec<_> = aligners(ws)
                    .arch_teeth(ScanRole::Lower)
                    .into_iter()
                    .filter(|f| f.quadrant() == 4)
                    .collect();
                right.sort_by_key(|f| f.position());
                json!({ "action": "set", "fdi": right[right.len() - 2].get() })
            }),
        ),
        ("aligners.planning.setup.automatic", Run(|_, _| json!({}))),
        // the arch line of the lower arch drawn by hand: kept, final position again
        (
            "aligners.planning.setup.line",
            Run(|ws, _| {
                let points: Vec<[f64; 3]> = aligners(ws).setup.lines[&ScanRole::Lower]
                    .iter()
                    .map(|p| [p.x, p.y - 0.5, p.z])
                    .collect();
                json!({ "arch": "lower", "points": points })
            }),
        ),
        ("aligners.planning.staging.automatic", Run(|_, _| json!({}))),
        (
            "aligners.planning.schedule",
            Run(|_, _| json!({ "start_date": "2026-11-16" })),
        ),
        ("aligners.planning.aligners", Run(|_, _| json!({}))),
        // a tooth slowed: the aligners of its arch computed again with it
        (
            "aligners.planning.tooth-speed",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 1), "speed": 0.5 })),
        ),
        // the print tracking: an aligner of the slowed arch printed, then to
        // print again (with its reason); the records stay with the case
        (
            "aligners.print.mark",
            Run(|_, _| json!({ "ids": ["INF-01"], "status": "printed" })),
        ),
        (
            "aligners.print.mark",
            Run(|_, _| json!({ "ids": ["INF-01"], "status": "reprint", "reason": "broken" })),
        ),
        // the clinical review: classified, one item decided, one value
        // chosen, the rest confirmed together
        ("aligners.review.classify", Run(|_, _| json!({}))),
        (
            "aligners.review.decide",
            Run(|ws, _| json!({ "key": aligners(ws).review[0].key() })),
        ),
        (
            "aligners.review.set-value",
            Run(|ws, _| {
                let r = aligners(ws)
                    .review
                    .into_iter()
                    .find(|r| r.kind == dca_model::ReviewKind::Attachment)
                    .unwrap();
                json!({ "key": r.key(), "value": "rotation" })
            }),
        ),
        ("aligners.review.bulk", Run(|_, _| json!({}))),
        ("aligners.occlusion.contact-map", Run(null)),
        (
            "aligners.occlusion.bite-with-aligners",
            Run(|_, _| json!({})),
        ),
        ("aligners.occlusion.bite-ramps", Run(|_, _| json!({}))),
        ("aligners.occlusion.bite-registration.icp", Skip(BITE_SCANS)),
        (
            "aligners.occlusion.bite-registration.three-points",
            Skip(BITE_SCANS),
        ),
        ("aligners.occlusion.align", Skip(BITE_SCANS)),
        // the final position of the upper arch back to the initial one; the
        // pipeline computes it again
        (
            "aligners.planning.setup.reset",
            Run(|_, _| json!({ "arch": "upper" })),
        ),
        // the whole pipeline as one step
        ("aligners.auto", Run(null)),
        // the aligner in the view: the preview of aligner 1 of both arches,
        // then its exact aligner at the draft quality, kept with the case
        ("aligners.view.aligner", Run(|_, _| json!({ "stage": 1 }))),
        (
            "aligners.view.aligner-exact",
            Run(|_, _| json!({ "stage": 1, "quality": "draft" })),
        ),
        // corrections of the built teeth and of the plan
        (
            "aligners.preparation.reconstruction.rebuild-tooth",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 1) })),
        ),
        (
            "aligners.manual.rebuild-tooth",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 2) })),
        ),
        // a query of the region and margin line of a tooth: the case is not touched
        (
            "aligners.manual.tooth-region",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 2) })),
        ),
        (
            "aligners.planning.movements",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 3), "mesial_mm": 0.3 })),
        ),
        // the exact values of a tooth, locked
        (
            "aligners.planning.movements",
            Run(
                |ws, _| json!({ "fdi": lower_crown(ws, 4), "exact": true, "rotation_deg": 4.0, "mesial_mm": -0.2, "lock": true }),
            ),
        ),
        // the gizmo of the viewport: a tooth scaled, the lower arch moved
        (
            "aligners.components.transform",
            Run(|ws, _| json!({ "component": "tooth", "fdi": lower_crown(ws, 3), "scale": 1.05 })),
        ),
        (
            "aligners.components.transform",
            Run(
                |_, _| json!({ "component": "gingiva", "arch": "lower", "move": { "t": [0.2, -0.1, 0.3], "q": [0.0, 0.0, 0.0087, 0.99996] } }),
            ),
        ),
        // the report of 2026-11-13: the brush over built teeth, then the marks
        (
            "aligners.preparation.segmentation.manual-brush",
            Run(|ws, _| {
                let f = lower_crown(ws, 0);
                json!({ "arch": "lower", "label": 0, "vertices": lower_vertices(ws, u16::from(f), 40) })
            }),
        ),
        (
            "aligners.preparation.segmentation.manual-brush",
            Run(|ws, _| {
                let f = lower_crown(ws, 1);
                json!({ "arch": "lower", "label": f, "mode": "erase", "vertices": lower_vertices(ws, u16::from(f), 30) })
            }),
        ),
        (
            "aligners.preparation.segmentation.manual-brush",
            Run(|ws, _| {
                let f = lower_crown(ws, 0);
                json!({ "arch": "lower", "label": f, "mode": "fill", "vertex": lower_vertices(ws, u16::from(f), 1)[0] })
            }),
        ),
        (
            "aligners.preparation.segmentation.clean-borders",
            Run(|_, _| json!({})),
        ),
        (
            "aligners.preparation.numbering.place-seeds",
            Run(|ws, _| {
                let f = lower_crown(ws, 4);
                let [x, y, z] = seed_position(ws, f);
                json!({ "op": "move", "fdi": f, "position": [x + 0.25, y, z] })
            }),
        ),
        (
            "aligners.preparation.numbering.delete-seed",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 5) })),
        ),
        (
            "aligners.preparation.numbering.place-seeds",
            Run(|ws, _| {
                // the tooth whose mark was deleted gets one again
                let f = lower_crown(ws, 5);
                let [x, y, z] = seed_position(ws, lower_crown(ws, 6));
                json!({ "op": "add", "fdi": f, "position": [x, y + 0.5, z] })
            }),
        ),
        (
            "aligners.preparation.numbering.renumber",
            Run(|ws, _| {
                let src = lower_crown(ws, 0);
                json!({ "src": src, "dst": if src == 38 { 48 } else { 38 } })
            }),
        ),
        (
            "aligners.preparation.numbering.mark-missing",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 2) })),
        ),
        (
            "aligners.preparation.numbering.mark-missing",
            Run(|ws, _| {
                let f = aligners(ws)
                    .missing
                    .iter()
                    .find(|f| !f.is_upper())
                    .unwrap()
                    .get();
                json!({ "fdi": f, "missing": false })
            }),
        ),
        (
            "aligners.preparation.numbering.clear-seeds",
            Run(|_, _| json!({ "arch": "upper" })),
        ),
        (
            "aligners.preparation.numbering.sequence",
            Run(|ws, _| {
                // the first tooth of the upper arch, where a built tooth is
                let tooth = aligners(ws)
                    .tooth_frames
                    .iter()
                    .find(|(f, _)| f.is_upper())
                    .map(|(_, fr)| fr.pose.t.to_array())
                    .unwrap();
                json!({
                    "arch": "upper",
                    "steps": [{ "kind": "point", "position": tooth }, { "kind": "skip" }],
                })
            }),
        ),
        // the next aligners from a control visit
        (
            "aligners.followup.visits",
            Run(|ws, dir| {
                let scan = write_lower_scan(ws, dir);
                json!({ "scan": scan.to_string_lossy(), "arch": "lower", "stage": 1 })
            }),
        ),
        (
            "aligners.followup.tracking",
            Run(|_, _| json!({ "arch": "lower" })),
        ),
        // the arch of a scan by hand: the occlusal side turned over and back,
        // then the two arches swapped (renumbered, their 3D teeth built again)
        (
            "aligners.case.set-arch",
            Run(|_, _| json!({ "scan": "lower", "flip_occlusal": true })),
        ),
        (
            "aligners.case.set-arch",
            Run(|_, _| json!({ "scan": "lower", "flip_occlusal": true })),
        ),
        (
            "aligners.case.set-arch",
            Run(|_, _| json!({ "scan": "lower", "arch": "upper" })),
        ),
        // the demo scans are closed: the case stays as it is (an open scan
        // closed and saved: `a_scan_closed_with_a_base_comes_back_after_save`)
        (
            "aligners.case.make-base",
            // the upper scan now: the old lower one, with all its tooth marks
            // (the swapped upper had its marks cleared: no arch frame)
            Run(|_, _| json!({ "arch": "upper", "height_mm": 2.0 })),
        ),
        // the scans
        (
            "case.import",
            Run(|_, dir| {
                let p = write_tetra(dir, "bite-one.stl");
                json!({ "path": p.to_string_lossy(), "role": "bite" })
            }),
        ),
        (
            "aligners.case.import-scans",
            Run(|_, dir| {
                let p = write_tetra(dir, "patient_bite.stl");
                json!({ "files": [{ "path": p.to_string_lossy() }] })
            }),
        ),
        (
            "case.set-role",
            Run(|ws, _| {
                // confirm the bite recognised from its file name
                let scan = ws
                    .state()
                    .case
                    .scans
                    .iter()
                    .find(|s| s.source_name.contains("patient_bite"))
                    .unwrap();
                json!({ "from": "bite", "to": "bite", "scan_id": scan.id.to_string() })
            }),
        ),
        (
            "case.remove-scan",
            Run(|ws, _| {
                // the bite scan imported first goes; the other one stays
                let scan = ws
                    .state()
                    .case
                    .scans
                    .iter()
                    .find(|s| s.source_name == "bite-one.stl")
                    .unwrap();
                json!({ "scan_id": scan.id.to_string() })
            }),
        ),
        // the study models are kept in the case (a coarse voxel keeps it short)
        (
            "aligners.output.study-models",
            Run(|_, _| json!({ "voxel_mm": 1.0, "tolerance_mm": 0.1 })),
        ),
        // what only writes files
        ("aligners.output.export-stl", Skip(FILES)),
        ("aligners.output.print-check", Skip(FILES)),
        ("aligners.output.manifest", Skip(FILES)),
        ("aligners.output.report", Skip(FILES)),
        (
            "aligners.output.export-all",
            Skip(
                "writes the files of every stage (minutes) and records their folder in the case (`export_dir`), a field of the module state; dca-model print.rs `the_tracking_and_the_export_folder_survive_the_case_file` checks that it is saved",
            ),
        ),
        ("aligners.production.shell.automatic", Skip(FILES)),
        ("aligners.production.base-labels", Skip(FILES)),
        ("aligners.production.blockout", Skip(FILES)),
        ("aligners.production.trim-line", Skip(FILES)),
        ("aligners.production.stamp", Skip(FILES)),
        // what only lists the aligners to print and their print status
        ("aligners.print.parts", Skip(LISTS)),
        ("aligners.print.status", Skip(LISTS)),
        ("aligners.production.print", Skip(LISTS)),
        // the scanner (the scan window, `scan.*`): its session and the scan
        // being edited live outside the case until `scan.import`
        ("scan.devices", Skip(SCANNER)),
        ("scan.connect", Skip(SCANNER)),
        ("scan.disconnect", Skip(SCANNER)),
        ("scan.start", Skip(SCANNER)),
        ("scan.pause", Skip(SCANNER)),
        ("scan.resume", Skip(SCANNER)),
        ("scan.stop", Skip(SCANNER)),
        ("scan.reset", Skip(SCANNER)),
        ("scan.status", Skip(SCANNER)),
        ("scan.settings", Skip(SCANNER)),
        ("scan.cloud", Skip(SCANNER)),
        ("scan.mesh-build", Skip(SCANNER)),
        ("scan.mesh", Skip(SCANNER)),
        ("scan.undo", Skip(SCANNER)),
        ("scan.redo", Skip(SCANNER)),
        ("scan.export", Skip(FILES)),
        ("scan.auto", Skip(SCANNER)),
        ("scan.reconstruct", Skip(SCANNER)),
        ("scan.close", Skip(SCANNER)),
        // the turntable of the scan window (`turntable.*`)
        ("turntable.devices", Skip(TURNTABLE)),
        ("turntable.connect", Skip(TURNTABLE)),
        ("turntable.disconnect", Skip(TURNTABLE)),
        ("turntable.move", Skip(TURNTABLE)),
        ("turntable.speed", Skip(TURNTABLE)),
        ("turntable.stop", Skip(TURNTABLE)),
        ("turntable.home", Skip(TURNTABLE)),
        ("turntable.status", Skip(TURNTABLE)),
        ("turntable.calibrate", Skip(RIG)),
        ("turntable.calibration", Skip(RIG)),
        (
            "scan.import",
            Skip(
                "adds the scan with the import of the module (run above); tests/scan.rs runs it from a simulated scan, saves and reopens",
            ),
        ),
    ]
}

#[test]
fn every_command_that_can_change_the_case_is_in_the_round_trip() {
    let ws = Workspace::new_headless(module(), None, None);
    let mut registered: BTreeSet<&str> = ws.module().command_specs().iter().map(|s| s.id).collect();
    // the shell's commands the workspace runs on the case: the scan import
    // (the module's own replaces it), the role of a scan, its removal, and
    // the commands of the scan window (the scanner, then the import of its
    // scan)
    registered.insert("case.import");
    registered.insert("case.set-role");
    registered.insert("case.remove-scan");
    registered.extend(dc_scanner::control::command_infos().iter().map(|i| i.id));
    let planned: BTreeSet<&str> = plan().iter().map(|(id, _)| *id).collect();
    let missing: Vec<_> = registered.difference(&planned).collect();
    let unknown: Vec<_> = planned.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "commands with no place in the round trip of `plan()` (run them, or say why not): {missing:?}"
    );
    assert!(unknown.is_empty(), "not registered: {unknown:?}");
}

#[test]
fn every_change_comes_back_after_save_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("round-trip.dcprj");
    let mut ws = saved(demo_state(&["lower", "upper"]), &path);
    for (id, step) in plan() {
        let params = match step {
            Plan::Run(params) => params,
            Plan::Skip(why) => {
                eprintln!("{id:<52} not run: {why}");
                continue;
            }
        };
        let start = Instant::now();
        let params = params(&ws, dir.path());
        run(&mut ws, id, params);
        let ran = start.elapsed();
        save_and_compare(&mut ws, &path, id);
        eprintln!(
            "{id:<52} {:>6.2} s run, {:>5.2} s save and reopen",
            ran.as_secs_f64(),
            (start.elapsed() - ran).as_secs_f64()
        );
        // undo after an edit: the file goes back with it
        if matches!(
            id,
            "aligners.preparation.segmentation.clean-borders" | "aligners.diagnosis.edit-values"
        ) {
            assert!(ws.undo());
            save_and_compare(&mut ws, &path, "undo");
            assert!(ws.redo());
            save_and_compare(&mut ws, &path, "redo");
        }
    }
    // the study models of both arches were saved and read back with the rest
    // (the last change, compared with the file above)
    assert_eq!(aligners(&ws).study_models.len(), 2);
}

/// The lower demo arch opened under the gum (the demo scan is closed): the
/// faces of its lowest 4 mm along the occlusal axis (+Z) removed.
fn open_lower(state: &CaseState) -> TriMesh {
    let mesh = state
        .mesh(&state.case.scan(ScanRole::Lower).unwrap().mesh)
        .unwrap();
    let lowest = mesh
        .positions
        .iter()
        .map(|p| p.z)
        .fold(f32::INFINITY, f32::min);
    let mut open = TriMesh::clone(mesh);
    open.tris.retain(|t| {
        !t.iter()
            .all(|&i| mesh.positions[i as usize].z < lowest + 4.0)
    });
    open
}

#[test]
fn a_scan_closed_with_a_base_comes_back_after_save() {
    let mut state = demo_state(&["lower"]);
    let role = ScanRole::Lower;
    let r = state.blobs.insert(open_lower(&state));
    state.case.scans[0].mesh = r;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("base.dcprj");
    let mut ws = saved(state, &path);
    run(
        &mut ws,
        "aligners.case.make-base",
        json!({ "height_mm": 2.5 }),
    );
    let closed = ws.state().mesh(&ws.state().case.scans[0].mesh).unwrap();
    assert!(closed.validate().is_watertight());
    let labels = ws.state().mesh(&aligners(&ws).labels[&role]).unwrap();
    assert_eq!(
        labels.vertex_labels.as_ref().unwrap().len(),
        closed.positions.len()
    );
    save_and_compare(&mut ws, &path, "make-base");
    // undo: the open scan, saved again
    assert!(ws.undo());
    assert!(
        !ws.state()
            .mesh(&ws.state().case.scans[0].mesh)
            .unwrap()
            .validate()
            .is_watertight()
    );
    save_and_compare(&mut ws, &path, "make-base undone");
}

#[test]
fn an_open_scan_closed_at_import_comes_back_after_save_undo_and_redo() {
    let dir = tempfile::tempdir().unwrap();
    let stl = dir.path().join("patient_lower.stl");
    let open = open_lower(&demo_state(&["lower"]));
    dc_mesh::io::write(&stl, &open, MeshFormat::Stl).unwrap();
    let path = dir.path().join("import.dcprj");
    let mut ws = saved(CaseState::default(), &path);
    run(
        &mut ws,
        "aligners.case.import-scans",
        json!({ "files": [{ "path": stl.to_string_lossy(), "role": "lower" }] }),
    );
    // the case scan is the closed solid, the scan as read its start
    let read = dc_case::import::prepare(&stl, &JobCtx::detached())
        .unwrap()
        .mesh;
    assert!(!read.validate().is_watertight());
    let check = |ws: &Workspace, by: ClosedBy| {
        let scan = ws.state().case.scan(ScanRole::Lower).unwrap();
        let closed = ws.state().mesh(&scan.mesh).unwrap();
        assert!(closed.validate().is_watertight());
        let record = aligners(ws).scan_base(&scan.mesh).unwrap();
        assert_eq!(record.closed_by, by);
        let back = dca_algo::scan_base::open_part(closed, &record).unwrap();
        assert_eq!(back, *read);
    };
    check(&ws, ClosedBy::Import);
    save_and_compare(&mut ws, &path, "import closed");
    // the import undone and done again, each saved: one step with its closure
    assert!(ws.undo());
    assert!(ws.state().case.scans.is_empty());
    save_and_compare(&mut ws, &path, "import undone");
    assert!(ws.redo());
    check(&ws, ClosedBy::Import);
    save_and_compare(&mut ws, &path, "import redone");
    // read back from the file: the record and the closed scan
    let p = reopen(&path);
    let file = p
        .case_state
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone();
    assert_eq!(file.scan_bases, aligners(&ws).scan_bases);
    assert_eq!(file.scan_bases.len(), 1);
    // a workspace on the file reopened goes on from it: close again by hand
    let mut ws = Workspace::new_headless(module(), Some(p), Some(path.clone()));
    check(&ws, ClosedBy::Import);
    run(
        &mut ws,
        "aligners.case.make-base",
        json!({ "height_mm": 4.0 }),
    );
    check(&ws, ClosedBy::Hand);
    save_and_compare(&mut ws, &path, "closed again by hand");
    // undo and redo, each saved: the record follows the scan
    assert!(ws.undo());
    check(&ws, ClosedBy::Import);
    save_and_compare(&mut ws, &path, "undo");
    assert!(ws.redo());
    check(&ws, ClosedBy::Hand);
    save_and_compare(&mut ws, &path, "redo");
}

/// A command that renames the patient at once: a change of the case.
struct Rename(&'static str);

impl Command for Rename {
    fn id(&self) -> &'static str {
        "test.rename"
    }
    fn label_key(&self) -> &str {
        "test-rename"
    }
    fn apply(&self, state: &CaseState, _ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let mut next = state.clone();
        next.case.patient_code = self.0.into();
        Ok(next)
    }
}

fn rename(ws: &mut Workspace, code: &'static str) {
    ws.run_command(Box::new(Rename(code)));
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.patient_code, code);
}

/// The gate of a [`Hold`]: shut until the test opens it.
type Gate = Arc<(Mutex<bool>, Condvar)>;

fn gate() -> Gate {
    Arc::new((Mutex::new(false), Condvar::new()))
}

fn open(gate: &Gate) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

/// A job that runs until its gate opens, then renames the patient (or fails
/// with `None`): a long command, or a brush stroke still running.
struct Hold(Gate, Option<&'static str>);

impl Command for Hold {
    fn id(&self) -> &'static str {
        "test.hold"
    }
    fn label_key(&self) -> &str {
        "test-hold"
    }
    fn apply(&self, state: &CaseState, _ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let (lock, cv) = &*self.0;
        let mut open = lock.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
        let Some(code) = self.1 else {
            return Err(CommandError::Failed("refused".into()));
        };
        let mut next = state.clone();
        next.case.patient_code = code.into();
        Ok(next)
    }
}

/// Polls until the save that runs ends (the other jobs keep running).
fn wait_for_save(ws: &mut Workspace) {
    let start = Instant::now();
    while ws.is_saving() {
        assert!(start.elapsed() < WAIT, "the save never ended");
        ws.wait_for_jobs(Duration::from_millis(5));
    }
}

fn saved_lower(dir: &Path) -> (Workspace, PathBuf) {
    let path = dir.join("auto.dcprj");
    (saved(demo_state(&["lower"]), &path), path)
}

#[test]
fn the_case_saves_itself_two_quiet_seconds_after_the_last_change() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ws, path) = saved_lower(dir.path());
    let t0 = 100.0;
    assert!(ws.autosaves());
    assert_eq!(ws.tick(t0), None, "nothing to save");
    rename(&mut ws, "A");
    // the timer starts when the change is seen
    let left = ws.tick(t0).unwrap();
    assert!((left.as_secs_f64() - DELAY_S).abs() < 1e-9, "{left:?}");
    assert!(ws.tick(t0 + 1.0).is_some());
    assert!(!ws.is_saving());
    // a new change starts it again
    rename(&mut ws, "B");
    assert!(ws.tick(t0 + 1.5).is_some());
    assert!(ws.tick(t0 + 3.0).is_some(), "1.5 s after the last change");
    assert!(!ws.is_saving());
    let saved_toasts = |ws: &Workspace| {
        ws.toast_keys()
            .iter()
            .filter(|k| *k == "ws-toast-saved")
            .count()
    };
    let said = saved_toasts(&ws);
    assert_eq!(ws.tick(t0 + 3.5), None);
    assert!(ws.is_saving(), "two quiet seconds after the last change");
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.clone()));
    assert!(!ws.is_dirty());
    assert_eq!(reopen(&path).case_state.case.patient_code, "B");
    // quiet: no notification when it works
    assert_eq!(saved_toasts(&ws), said);
    // nothing more to save
    assert_eq!(ws.tick(t0 + 30.0), None);
    assert!(!ws.is_saving());
    // undo is a change too, saved the same way
    assert!(ws.undo());
    assert!(ws.tick(t0 + 40.0).is_some());
    assert_eq!(ws.tick(t0 + 42.0), None);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(reopen(&path).case_state.case.patient_code, "A");
}

#[test]
fn nothing_saves_itself_while_a_job_runs_and_it_saves_once_the_job_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ws, path) = saved_lower(dir.path());
    let t0 = 0.0;
    rename(&mut ws, "A");
    ws.tick(t0);
    // a long job: well past the delay, nothing is saved while it runs
    let g = gate();
    ws.run_command(Box::new(Hold(Arc::clone(&g), Some("J"))));
    assert!(ws.is_busy());
    for t in [3.0, 10.0, 60.0] {
        assert_eq!(ws.tick(t0 + t), None);
        assert!(!ws.is_saving(), "saved at {t} s while the job ran");
    }
    // its change lands: the timer starts again from there
    open(&g);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.patient_code, "J");
    assert!(ws.tick(t0 + 61.0).is_some());
    assert_eq!(ws.tick(t0 + 63.0), None);
    assert!(ws.is_saving());
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(reopen(&path).case_state.case.patient_code, "J");

    // a job that changes nothing (refused): the change it held back is saved
    // as soon as it ends
    rename(&mut ws, "K");
    ws.tick(t0 + 70.0);
    let g = gate();
    ws.run_command(Box::new(Hold(Arc::clone(&g), None)));
    assert_eq!(ws.tick(t0 + 80.0), None);
    assert!(!ws.is_saving());
    open(&g);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.patient_code, "K");
    assert_eq!(ws.tick(t0 + 80.0), None);
    assert!(ws.is_saving(), "saved once the job ended");
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(reopen(&path).case_state.case.patient_code, "K");
}

#[test]
fn a_case_never_saved_or_read_only_does_not_save_itself_nor_ask_for_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut ws = Workspace::new_headless(module(), Some(project(demo_state(&["lower"]))), None);
    assert!(!ws.autosaves());
    rename(&mut ws, "A");
    for t in [0.0, 5.0, 100.0] {
        assert_eq!(ws.tick(t), None);
    }
    assert!(!ws.is_saving());
    assert_eq!(ws.dialog_requests(), 0, "nothing asks for a file");
    // saved once: from then on it saves itself
    let path = dir.path().join("first.dcprj");
    ws.save_to(path.clone());
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.autosaves());
    rename(&mut ws, "B");
    ws.tick(200.0);
    ws.tick(202.0);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(reopen(&path).case_state.case.patient_code, "B");

    // a file opened read-only is never written by itself
    let mut ws = Workspace::new_headless(module(), Some(reopen(&path)), Some(path.clone()));
    ws.set_read_only(true);
    assert!(!ws.autosaves());
    rename(&mut ws, "C");
    ws.tick(0.0);
    assert_eq!(ws.tick(10.0), None);
    assert!(!ws.is_saving());
    assert_eq!(reopen(&path).case_state.case.patient_code, "B");
}

#[test]
fn a_failed_automatic_save_is_shown_and_tried_again_at_the_next_change() {
    let dir = tempfile::tempdir().unwrap();
    // the folder of the project is gone: the save cannot write it
    let path = dir.path().join("gone").join("case.dcprj");
    let mut ws = Workspace::new_headless(
        module(),
        Some(project(demo_state(&["lower"]))),
        Some(path.clone()),
    );
    rename(&mut ws, "A");
    ws.tick(0.0);
    ws.tick(DELAY_S);
    assert!(ws.is_saving());
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.is_dirty());
    assert!(
        ws.sticky_toast_keys()
            .contains(&"ws-toast-autosave-failed".to_owned()),
        "never silent: {:?}",
        ws.toast_details()
    );
    // not tried again by itself: the error is on screen
    assert_eq!(ws.tick(30.0), None);
    assert!(!ws.is_saving());
    // the next change tries again
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    rename(&mut ws, "B");
    ws.tick(40.0);
    ws.tick(42.0);
    assert!(ws.is_saving());
    assert!(ws.wait_for_jobs(WAIT));
    assert!(!ws.is_dirty());
    assert_eq!(reopen(&path).case_state.case.patient_code, "B");
}

#[test]
fn a_save_whose_backup_cannot_be_kept_succeeds_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ws, path) = saved_lower(dir.path());
    // `<file>.bak` is a folder: the backup cannot replace it
    let bak = dir.path().join("auto.dcprj.bak");
    let _ = std::fs::remove_file(&bak);
    std::fs::create_dir_all(bak.join("keep")).unwrap();
    rename(&mut ws, "W");
    ws.save_to(path.clone());
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.clone()));
    assert!(!ws.is_dirty(), "the save counts as done");
    assert_eq!(reopen(&path).case_state.case.patient_code, "W");
    let details = ws.toast_details();
    assert!(
        details
            .iter()
            .any(|(k, d)| k == "ws-toast-save-warning"
                && d.as_deref().is_some_and(|d| !d.is_empty())),
        "the warning reaches the user: {details:?}"
    );
    assert!(!ws.toast_keys().contains(&"ws-toast-save-failed".to_owned()));
}

#[test]
fn a_change_that_lands_after_save_is_pressed_is_saved_too() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ws, path) = saved_lower(dir.path());
    // a brush stroke still running when Save is pressed (Save while the
    // previous save runs is not refused either)
    let g = gate();
    ws.run_command(Box::new(Hold(Arc::clone(&g), Some("S"))));
    ws.save();
    ws.save();
    wait_for_save(&mut ws);
    assert!(
        !ws.toast_keys().contains(&"ws-toast-busy".to_owned()),
        "{:?}",
        ws.toast_keys()
    );
    open(&g);
    assert!(ws.wait_for_jobs(WAIT));
    // the stroke came after the save: it is not in the file yet...
    assert!(ws.is_dirty());
    assert_ne!(reopen(&path).case_state.case.patient_code, "S");
    // ...and closing the window saves it first
    assert!(!ws.request_quit(), "it must not close before saving");
    assert!(ws.is_saving());
    assert!(ws.wait_for_jobs(WAIT));
    let mut actions = Vec::new();
    loop {
        match ws.take_action() {
            WorkspaceAction::None => break,
            a => actions.push(a),
        }
    }
    assert!(
        actions.ends_with(&[WorkspaceAction::Saved(path.clone()), WorkspaceAction::Quit]),
        "{actions:?}"
    );
    assert_eq!(reopen(&path).case_state.case.patient_code, "S");
    // nothing left to save: it may close at once
    assert!(ws.request_quit());
}

/// The workspace in a headless window, with the actions it returned.
struct App {
    ws: Workspace,
    actions: Vec<WorkspaceAction>,
}

fn harness(ws: Workspace) -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, app: &mut App| {
                let a = app.ws.show(ui);
                if a != WorkspaceAction::None {
                    app.actions.push(a);
                }
            },
            App {
                ws,
                actions: Vec::new(),
            },
        )
}

#[test]
fn the_top_bar_says_saved_unsaved_or_not_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, _) = saved_lower(dir.path());
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("Saved").is_some());
    rename(&mut h.state_mut().ws, "A");
    h.run();
    assert!(h.query_by_label("Unsaved changes").is_some());
    assert!(h.query_by_label("Saved").is_none());

    // a save that fails says so until the next change
    let mut ws = Workspace::new_headless(
        module(),
        Some(project(demo_state(&["lower"]))),
        Some(dir.path().join("gone").join("x.dcprj")),
    );
    rename(&mut ws, "A");
    ws.save();
    assert!(ws.wait_for_jobs(WAIT));
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("Not saved").is_some());
    assert!(h.query_by_label("Save failed").is_some());
}

#[test]
fn leaving_saves_a_project_with_a_file_and_asks_for_one_never_saved() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ws, path) = saved_lower(dir.path());
    rename(&mut ws, "H");
    let mut h = harness(ws);
    h.run();
    // Home: saved first, no question
    h.get_by_label("Home").click();
    h.step();
    assert!(h.query_by_label("Don't save").is_none());
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    h.run();
    assert_eq!(
        h.state().actions,
        vec![
            WorkspaceAction::Saved(path.clone()),
            WorkspaceAction::GoHome
        ]
    );
    assert_eq!(reopen(&path).case_state.case.patient_code, "H");

    // never saved: the window asks before closing
    let mut ws = Workspace::new_headless(module(), Some(project(demo_state(&["lower"]))), None);
    rename(&mut ws, "U");
    assert!(!ws.request_quit());
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("Don't save").is_some());
    h.get_by_label("Don't save").click();
    h.run();
    assert_eq!(h.state().actions, vec![WorkspaceAction::Quit]);
}

// ---------------------------------------------------------------- step history

const IMPORT: &str = "aligners.case.import-scans";
const FIND_TEETH: &str = "aligners.teeth.find";
const SEGMENT: &str = "aligners.preparation.segmentation.automatic";
const STUDY_MODELS: &str = "aligners.output.study-models";

/// The functions with a record in the step history of `ws`, sorted.
fn recorded(ws: &Workspace) -> Vec<String> {
    ws.history().records().keys().cloned().collect()
}

fn sorted(ids: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

/// The JSON of a step history another front end wrote: a run of each of
/// `runs` in turn, at a fixed time.
fn written(runs: &[&'static str]) -> Value {
    let g = StepGraph::new(
        runs.iter()
            .map(|&id| StepDef { id, depends: &[] })
            .collect(),
    )
    .unwrap();
    let mut h = StepHistory::new();
    for id in runs {
        h.checkpoint(&g, id, StepOutcome::default()).unwrap();
    }
    let mut v = serde_json::to_value(&h).unwrap();
    for record in v["steps"].as_object_mut().unwrap().values_mut() {
        record["at"] = json!(1_794_600_000_u64);
    }
    v
}

/// `true` when the inspector of function `id`, selected, says it is done.
fn says_done(h: &mut Harness<'static, App>, id: &'static str) -> bool {
    h.state_mut().ws.select_item(id);
    h.run();
    h.query_by_label("Done").is_some()
}

#[test]
fn a_run_that_fails_records_nothing_and_a_step_run_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let mut ws = Workspace::new_headless(module(), None, None);
    assert!(ws.history().records().is_empty());
    // the button of "Find the teeth" on a case without scans: refused, and
    // the case and its history are the ones before
    let before = history(&ws);
    assert!(ws.run_leaf_command(FIND_TEETH, json!({})));
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.take_leaf_answer().unwrap().is_err());
    assert!(!ws.is_dirty());
    assert_eq!(history(&ws), before);
    // a scan imported: "Import scans" ran, automatically
    ws.set_import_role(Some(ScanRole::Lower));
    ws.import_files(vec![write_tetra(dir.path(), "lower.stl")]);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans.len(), 1);
    assert_eq!(recorded(&ws), [IMPORT]);
    let record = ws.history().record(IMPORT).unwrap();
    assert!(!record.by_hand && record.failed.is_none() && record.rev == 1);
    // undo takes the run and its record back, redo brings both again
    assert!(ws.undo());
    assert!(ws.history().records().is_empty());
    assert!(ws.redo());
    assert_eq!(recorded(&ws), [IMPORT]);
}

#[test]
fn a_function_that_leaves_nothing_in_the_case_is_done_on_the_case_its_run_left() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.dcprj");
    // the history read with the case: the marks found and the colours by hand
    let mut ws = saved(demo_state(&["lower"]), &path);
    assert_eq!(recorded(&ws), sorted(&[FIND_TEETH, SEGMENT]));
    // the study models: their run is recorded, the case shows nothing of it
    run(
        &mut ws,
        STUDY_MODELS,
        json!({ "voxel_mm": 1.0, "tolerance_mm": 0.1 }),
    );
    assert_eq!(recorded(&ws), sorted(&[FIND_TEETH, SEGMENT, STUDY_MODELS]));
    let record = ws.history().record(STUDY_MODELS).unwrap();
    assert!(!record.by_hand && record.failed.is_none());
    assert!(record.rev > ws.history().record(SEGMENT).unwrap().rev);
    save_and_compare(&mut ws, &path, "study models");
    // opened again as Home opens it: the same history, and they are done
    let reopened = Workspace::new_headless(module(), Some(reopen(&path)), Some(path.clone()));
    let mut h = harness(reopened);
    assert_eq!(history(&h.state().ws), history(&ws));
    assert!(says_done(&mut h, STUDY_MODELS));
    // any change of the case: no longer done, and no longer recorded
    rename(&mut h.state_mut().ws, "R");
    assert!(!says_done(&mut h, STUDY_MODELS));
    assert_eq!(recorded(&h.state().ws), sorted(&[FIND_TEETH, SEGMENT]));
    // undo: the case their run left, done again
    assert!(h.state_mut().ws.undo());
    assert!(says_done(&mut h, STUDY_MODELS));
    assert_eq!(history(&h.state().ws), history(&ws));
}

#[test]
fn clearing_or_correcting_a_result_drops_exactly_the_records_computed_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleared.dcprj");
    let mut p = project(demo_state(&["lower", "upper"]));
    p.history = written(&[IMPORT, FIND_TEETH, SEGMENT, STUDY_MODELS]);
    let mut ws = Workspace::new_headless(module(), Some(p), Some(path.clone()));
    let opened = history(&ws);
    assert_eq!(
        recorded(&ws),
        sorted(&[IMPORT, FIND_TEETH, SEGMENT, STUDY_MODELS])
    );
    // the upper scan removed: the lower arch keeps its marks and colours,
    // the study models were made of both scans
    let upper = ws.state().case.scan(ScanRole::Upper).unwrap().id;
    run(
        &mut ws,
        "case.remove-scan",
        json!({ "scan_id": upper.to_string() }),
    );
    assert_eq!(recorded(&ws), sorted(&[IMPORT, FIND_TEETH, SEGMENT]));
    let removed = history(&ws);
    // a tooth mark moved by hand: the marks are by hand now, and the colours
    // made from the previous marks (kept, out of date) go from the history
    let seed = aligners(&ws)
        .seeds
        .iter()
        .find(|s| !s.fdi.is_upper())
        .copied()
        .unwrap();
    let [x, y, z] = seed.position.to_array();
    run(
        &mut ws,
        "aligners.preparation.numbering.place-seeds",
        json!({ "op": "move", "fdi": seed.fdi.get(), "position": [x + 0.25, y, z] }),
    );
    assert_eq!(recorded(&ws), sorted(&[IMPORT, FIND_TEETH]));
    assert!(ws.history().record(FIND_TEETH).unwrap().by_hand);
    assert!(!ws.history().record(IMPORT).unwrap().by_hand);
    let moved = history(&ws);
    // undo and redo: the case and its history go back and forth together
    assert!(ws.undo());
    assert_eq!(history(&ws), removed);
    assert!(ws.undo());
    assert_eq!(history(&ws), opened);
    assert!(ws.redo());
    assert!(ws.redo());
    assert_eq!(history(&ws), moved);
    save_and_compare(&mut ws, &path, "corrected by hand");
}

#[test]
fn a_history_the_cli_saved_with_results_the_case_no_longer_holds_marks_nothing_done() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cli.dcprj");
    // the app saves the marks found and the colours in its history
    let ws = saved(demo_state(&["lower"]), &path);
    let app = history(&ws);
    // the CLI reads that history, clears the marks and saves its history as
    // it read it
    let mut s = dc_session::Session::new();
    s.register_module(module()).unwrap();
    let out = s
        .execute("project.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    assert_eq!(out["read_only"], false, "{out}");
    let listed = s.execute("steps.list", Value::Null).unwrap();
    assert_eq!(listed["records"], app["steps"]);
    s.execute(
        "aligners.preparation.numbering.clear-seeds",
        json!({ "arch": "lower" }),
    )
    .unwrap();
    s.execute("project.save", Value::Null).unwrap();
    assert_eq!(reopen(&path).history, app, "the CLI kept the records");
    // the app: "Find the teeth" is not done and its record goes; the colours,
    // kept, stay
    let ws = Workspace::new_headless(module(), Some(reopen(&path)), Some(path.clone()));
    assert_eq!(recorded(&ws), [SEGMENT]);
    let mut h = harness(ws);
    h.run();
    assert!(
        h.query_by_label("Treatment workflow 2/9").is_some(),
        "import and the colours only"
    );
    // its next save writes the history as it is
    h.state_mut().ws.save();
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    assert_eq!(reopen(&path).history, history(&h.state().ws));
    assert!(reopen(&path).history["steps"].get(FIND_TEETH).is_none());
}

#[test]
fn a_history_that_cannot_be_read_keeps_the_file_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let (_, path) = saved_lower(dir.path());
    // the same file with a step history of another shape
    let mut p = reopen(&path);
    p.history = json!("not a step history");
    dc_project::save(&path, &p).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut ws = Workspace::new_headless(module(), Some(reopen(&path)), Some(path.clone()));
    // Home finds the file itself sound: the history keeps it read-only
    ws.set_read_only(false);
    assert!(ws.is_read_only() && !ws.autosaves());
    assert!(
        ws.sticky_toast_keys()
            .contains(&"ws-toast-history-unreadable".to_owned())
    );
    // never written over: not by itself, not by a save to that file
    rename(&mut ws, "X");
    ws.tick(0.0);
    assert_eq!(ws.tick(10.0), None);
    ws.save_to(path.clone());
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.toast_keys().contains(&"ws-toast-save-failed".to_owned()));
    assert!(ws.is_dirty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Save as another file: written, with a history that can be read
    let other = dir.path().join("other.dcprj");
    ws.save_to(other.clone());
    assert!(ws.wait_for_jobs(WAIT));
    assert!(!ws.is_read_only() && !ws.is_dirty());
    let q = reopen(&other);
    assert_eq!(q.case_state.case.patient_code, "X");
    assert_eq!(q.history, history(&ws));
    assert!(serde_json::from_value::<StepHistory>(q.history).is_ok());
}
