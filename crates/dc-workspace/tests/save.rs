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
use dca_model::{AlignersState, MODULE_ID};
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
/// the case of the workspace and the step history.
fn save_and_compare(ws: &mut Workspace, path: &Path, what: &str) {
    ws.save();
    assert!(ws.wait_for_jobs(WAIT), "{what}: save");
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.to_path_buf()));
    assert!(!ws.is_dirty(), "{what}: still unsaved");
    let errors = ws.sticky_toast_keys();
    assert!(errors.is_empty(), "{what}: {:?}", ws.toast_details());
    let p = reopen(path);
    assert_same(&p.case_state, ws.state(), what);
    assert_eq!(p.history, step_history(), "{what}: step history");
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

/// The registration of a bite needs the bite scans of a patient that fit
/// both arches; what it changes (a scan moved, the flag of the bite) is the
/// same data the other steps change and the round trip compares.
const BITE_SCANS: &str = "needs the bite scans of a patient";

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
        // with both arches the bite comes before the final position
        ("aligners.occlusion.virtual-bite", Run(|_, _| json!({}))),
        ("aligners.planning.setup.automatic", Run(|_, _| json!({}))),
        ("aligners.planning.staging.automatic", Run(|_, _| json!({}))),
        (
            "aligners.planning.schedule",
            Run(|_, _| json!({ "start_date": "2026-11-16" })),
        ),
        ("aligners.planning.aligners", Run(|_, _| json!({}))),
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
        // the whole pipeline as one step
        ("aligners.auto", Run(null)),
        // corrections of the built teeth and of the plan
        (
            "aligners.preparation.reconstruction.rebuild-tooth",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 1) })),
        ),
        (
            "aligners.manual.rebuild-tooth",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 2) })),
        ),
        (
            "aligners.planning.movements",
            Run(|ws, _| json!({ "fdi": lower_crown(ws, 3), "mesial_mm": 0.3 })),
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
        // what only writes files
        ("aligners.output.export-stl", Skip(FILES)),
        ("aligners.output.print-check", Skip(FILES)),
        ("aligners.output.manifest", Skip(FILES)),
        ("aligners.output.report", Skip(FILES)),
        ("aligners.output.study-models", Skip(FILES)),
        ("aligners.output.export-all", Skip(FILES)),
        ("aligners.production.shell.automatic", Skip(FILES)),
        ("aligners.production.base-labels", Skip(FILES)),
        ("aligners.production.blockout", Skip(FILES)),
        ("aligners.production.trim-line", Skip(FILES)),
        ("aligners.production.stamp", Skip(FILES)),
    ]
}

#[test]
fn every_command_that_can_change_the_case_is_in_the_round_trip() {
    let ws = Workspace::new_headless(module(), None, None);
    let mut registered: BTreeSet<&str> = ws.module().command_specs().iter().map(|s| s.id).collect();
    // the shell's commands the workspace runs on the case: the scan import
    // (the module's own replaces it) and the role of a scan
    registered.insert("case.import");
    registered.insert("case.set-role");
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
        if id == "aligners.preparation.segmentation.clean-borders" {
            assert!(ws.undo());
            save_and_compare(&mut ws, &path, "undo");
            assert!(ws.redo());
            save_and_compare(&mut ws, &path, "redo");
        }
    }
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
