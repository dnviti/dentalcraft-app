//! Headless tests of the workspace: rendering (egui_kittest, no GPU), menu
//! search, every menu command, import with undo/redo, save/load.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dc_case::{Case, CaseState, Fdi, ModuleState, Pose, RoleSource, Scan, ScanRole};
use dc_mesh::{MeshFormat, TriMesh, Units};
use dc_module_api::{
    CommandDesc, MenuTree, Module, ModuleCodec, ModuleCtx, ModuleError, NOT_AVAILABLE_KEY,
};
use dc_project::{AppInfo, Project};
use dc_workspace::{Workspace, WorkspaceAction};
use dca_model::{AlignersState, IprStep, Seed, Stage};
use dca_module::AlignersModule;
use egui::{Key, Modifiers, Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::{DVec3, Vec3};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(20);

struct App {
    ws: Workspace,
    actions: Vec<WorkspaceAction>,
}

fn harness(ws: Workspace) -> Harness<'static, App> {
    Harness::builder()
        .with_size(vec2(1920.0, 1080.0))
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

/// Settle the UI after an action that may have started a job: the app keeps
/// repainting while a job runs, so `Harness::run` alone can exceed max_steps.
/// Take one frame, wait for the job, then let the UI settle.
fn settle(h: &mut Harness<'static, App>) {
    h.step();
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    h.run();
}

fn module() -> Box<dyn Module> {
    Box::new(AlignersModule::new())
}

/// Number of leaves of the module's menu (the total of the workflow footer).
fn leaf_count() -> usize {
    module().menu().command_ids().len()
}

/// The aligners module that records the ids passed to [`Module::select`].
struct Spy {
    inner: AlignersModule,
    selected: Arc<Mutex<Vec<String>>>,
}

impl Module for Spy {
    fn id(&self) -> &'static str {
        self.inner.id()
    }
    fn name_key(&self) -> &'static str {
        self.inner.name_key()
    }
    fn version(&self) -> &'static str {
        self.inner.version()
    }
    fn menu(&self) -> MenuTree {
        self.inner.menu()
    }
    fn commands(&self) -> Vec<CommandDesc> {
        self.inner.commands()
    }
    fn invoke(&mut self, id: &str, ctx: &mut ModuleCtx<'_>) -> Result<(), ModuleError> {
        self.inner.invoke(id, ctx)
    }
    fn select(&mut self, id: &str) {
        self.selected.lock().unwrap().push(id.to_owned());
        Module::select(&mut self.inner, id);
    }
    fn inspector(&mut self, ui: &mut egui::Ui, ctx: &mut ModuleCtx<'_>) {
        self.inner.inspector(ui, ctx);
    }
    fn scene(&self, state: &CaseState) -> Vec<dc_scene::SceneItem> {
        self.inner.scene(state)
    }
    fn data_codec(&self) -> ModuleCodec {
        self.inner.data_codec()
    }
}

fn spy() -> (Box<dyn Module>, Arc<Mutex<Vec<String>>>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let m = Spy {
        inner: AlignersModule::new(),
        selected: log.clone(),
    };
    (Box::new(m), log)
}

/// Closed tetrahedron, `size` units wide.
fn tetra(size: f32) -> TriMesh {
    TriMesh::new(
        vec![
            Vec3::ZERO,
            Vec3::new(size, 0.0, 0.0),
            Vec3::new(0.0, size, 0.0),
            Vec3::new(0.0, 0.0, size),
        ],
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
    )
}

/// Project with an upper scan and three stages (IPR at stage 2).
fn synthetic_project() -> Project {
    let mut state = CaseState::new(Case::new("AL-TEST-01"));
    let blob = state.blobs.insert(tetra(50.0));
    state.case.scans.push(Scan {
        id: uuid::Uuid::new_v4(),
        role: ScanRole::Upper,
        role_source: RoleSource::Explicit,
        source_name: "upper.stl".into(),
        source_units: Units::Millimeters,
        import_xform: Pose::default(),
        mesh: blob,
    });
    let mut al = AlignersState::default();
    for i in 0..3 {
        al.stages.push(Stage::new(i));
    }
    al.stages[2].ipr.push(IprStep {
        between: [Fdi::new(11).unwrap(), Fdi::new(21).unwrap()],
        amount_mm: 0.2,
    });
    state.set_module(dca_model::MODULE_ID, ModuleState::new(al));
    Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    )
}

fn write_stl(dir: &Path, name: &str, mesh: &TriMesh) -> PathBuf {
    let path = dir.join(name);
    dc_mesh::io::write(&path, mesh, MeshFormat::Stl).unwrap();
    path
}

#[test]
fn renders_without_project() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    assert!(h.query_by_label("Case").is_some());
    assert!(h.query_by_label("No scans yet").is_some());
    // menu entry (Case group open by default) and empty-state button
    assert_eq!(h.get_all_by_label("Import scans").count(), 2);
    let label = format!("Treatment workflow 0/{}", leaf_count());
    assert!(h.query_by_label(&label).is_some());
    assert!(h.query_by_label_contains("No stages yet").is_some());
    assert!(!h.state().ws.is_dirty());
}

#[test]
fn renders_with_synthetic_project() {
    let ws = Workspace::new_headless(module(), Some(synthetic_project()), None);
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("No scans yet").is_none());
    assert!(h.query_by_label("AL-TEST-01").is_some());
    assert!(h.query_by_label("Stage 00").is_some());
    assert!(
        h.query_by_label_contains("Both jaws, occlusal view from below, stage 00")
            .is_some()
    );
    // import counts as done once the case has scans
    let label = format!("Treatment workflow 1/{}", leaf_count());
    assert!(h.query_by_label(&label).is_some());
    // scrub to the last stage with End
    h.key_press(Key::End);
    h.run();
    assert!(h.query_by_label("Stage 02").is_some());
    h.key_press(Key::Comma);
    h.run();
    assert!(h.query_by_label("Stage 01").is_some());
    // jaw toggle changes the caption
    h.get_by_label("Lower").click();
    h.run();
    assert!(h.query_by_label_contains("Upper jaw, occlusal").is_some());
}

#[test]
fn menu_search_filters() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    assert!(h.query_by_label("Case info").is_some());
    // `/` focuses the filter, typing filters the tree
    h.key_press(Key::Slash);
    h.run();
    h.event(egui::Event::Text("brush".into()));
    h.run();
    assert_eq!(h.state().ws.filter(), "brush");
    assert!(h.query_by_label("Case info").is_none());
    assert!(h.query_by_label("Manual brush").is_some());
    assert!(h.query_by_label("Preparation").is_some());
    // Escape clears the filter
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().ws.filter(), "");
    // nothing matches
    h.state_mut().ws.set_filter("zzz-nothing");
    h.run();
    assert!(h.query_by_label("No tool matches the search.").is_some());
    let ids = h.state().ws.visible_menu_ids();
    assert!(ids.is_empty());
    h.state_mut().ws.set_filter("ipr");
    assert!(
        h.state()
            .ws
            .visible_menu_ids()
            .contains(&"aligners.planning.ipr")
    );
}

#[test]
fn every_leaf_can_be_activated() {
    let mut ws = Workspace::new_headless(module(), None, None);
    let leaves = ws.module().menu().command_ids();
    assert!(leaves.len() > 40);
    for id in &leaves {
        ws.activate(id);
        assert_eq!(ws.selected(), Some(*id));
    }
    // placeholders say they are not available; a leaf with a registered
    // command runs it as a job (the next ones find the workspace busy) and
    // the import asks for its files
    let stubs = leaves
        .iter()
        .filter(|id| {
            !dca_module::commands::is_registered(id) && !dc_module_api::is_import_scans_command(id)
        })
        .count();
    let keys = ws.toast_keys();
    let not_available = keys.iter().filter(|k| *k == NOT_AVAILABLE_KEY).count();
    assert_eq!(not_available, stubs, "{keys:?}");
    assert!(
        keys.iter()
            .all(|k| k == NOT_AVAILABLE_KEY || k == "ws-toast-busy"),
        "{keys:?}"
    );
    assert_eq!(ws.dialog_requests(), 1);
    assert!(ws.wait_for_jobs(WAIT));
}

#[test]
fn clicking_every_row_does_not_panic() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.expand_all();
    let mut h = harness(ws);
    h.run();
    let n = h.get_all_by_role(Role::TreeItem).count();
    assert!(n > 50);
    // bottom-up: collapsing a group only removes rows below it
    for i in (0..n).rev() {
        let nodes: Vec<_> = h.get_all_by_role(Role::TreeItem).collect();
        nodes[i].click();
        settle(&mut h);
    }
    assert!(h.state().ws.dialog_requests() >= 1);
    assert!(!h.state().ws.toast_keys().is_empty());
    // view toolbar, timeline and inspector buttons
    for label in [
        "Front",
        "Left",
        "Right",
        "Back",
        "Occlusal",
        "Wireframe",
        "Measure",
        "Help",
    ] {
        h.get_by_label(label).click();
        settle(&mut h);
        if label == "Help" {
            assert!(h.query_by_label("Keyboard shortcuts").is_some());
            h.key_press(Key::Escape);
            h.run();
        }
    }
    assert!(h.query_by_label("Keyboard shortcuts").is_none());
}

#[test]
fn menu_clicks_select_in_the_module() {
    let (m, log) = spy();
    let mut ws = Workspace::new_headless(m, None, None);
    ws.expand_all();
    let mut h = harness(ws);
    h.run();
    let n = h.get_all_by_role(Role::TreeItem).count();
    for i in (0..n).rev() {
        let nodes: Vec<_> = h.get_all_by_role(Role::TreeItem).collect();
        nodes[i].click();
        settle(&mut h);
    }
    let log = log.lock().unwrap();
    // a group header and a leaf
    assert!(log.iter().any(|id| id == "aligners.diagnosis"), "{log:?}");
    assert!(
        log.iter().any(|id| id == "aligners.diagnosis.bolton"),
        "{log:?}"
    );
}

#[test]
fn module_shortcuts_run_their_command() {
    let (m, log) = spy();
    let mut h = harness(Workspace::new_headless(m, None, None));
    h.run();
    h.key_press(Key::S);
    settle(&mut h);
    assert_eq!(
        h.state().ws.selected(),
        Some("aligners.planning.staging.automatic")
    );
    assert_eq!(
        log.lock().unwrap().last().map(String::as_str),
        Some("aligners.planning.staging.automatic")
    );
    h.key_press(Key::A);
    settle(&mut h);
    assert_eq!(
        h.state().ws.selected(),
        Some("aligners.preparation.segmentation.automatic")
    );
    // a placeholder says it is not available; a registered command runs and
    // fails on the empty case
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    let keys = h.state().ws.toast_keys();
    assert!(
        keys.iter()
            .any(|k| k == NOT_AVAILABLE_KEY || k == "ws-toast-command-failed"),
        "{keys:?}"
    );
}

#[test]
fn import_then_undo_redo() {
    let dir = tempfile::tempdir().unwrap();
    // 0.05 wide: detected as metres and converted to 50 mm
    let path = write_stl(dir.path(), "patient_upper.stl", &tetra(0.05));
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    h.state_mut().ws.import_files(vec![path]);
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    h.run();
    {
        let ws = &h.state().ws;
        let st = ws.state();
        assert_eq!(st.case.scans.len(), 1);
        let scan = &st.case.scans[0];
        assert_eq!(scan.role, ScanRole::Upper);
        assert_eq!(scan.source_units, Units::Meters);
        let size = st
            .mesh(&scan.mesh)
            .unwrap()
            .aabb()
            .unwrap()
            .size()
            .max_element();
        assert!((size - 50.0).abs() < 1e-3, "size {size}");
        assert!(ws.is_dirty());
        // named upper: the role comes from the file name
        assert_eq!(scan.role_source, RoleSource::FileName);
        // the report of the shared pipeline: what it noticed, then the count
        let keys = ws.toast_keys();
        for k in ["ws-toast-warn-rescaled", "ws-toast-warn-low-polygons"] {
            assert!(keys.contains(&k.to_owned()), "{keys:?}");
        }
        assert_eq!(keys.last().map(String::as_str), Some("ws-toast-imported"));
    }
    assert!(h.query_by_label("Unsaved changes").is_some());
    assert!(h.query_by_label("No scans yet").is_none());

    // Ctrl+Z / Ctrl+Shift+Z
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(h.state().ws.state().case.scans.is_empty());
    assert!(!h.state().ws.is_dirty());
    assert!(h.query_by_label("No scans yet").is_some());
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().ws.state().case.scans.len(), 1);
    // buttons
    h.get_by_label("Undo").click();
    h.run();
    assert!(h.state().ws.state().case.scans.is_empty());
    h.get_by_label("Redo").click();
    h.run();
    assert_eq!(h.state().ws.state().case.scans.len(), 1);
}

#[test]
fn unreadable_file_reports_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.stl");
    std::fs::write(&path, b"solid nope\nfacet garbage\n").unwrap();
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.import_files(vec![path]);
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.state().case.scans.is_empty());
    assert!(
        ws.toast_keys()
            .contains(&"ws-toast-import-failed".to_owned())
    );
}

#[test]
fn save_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut ws = Workspace::new_headless(module(), Some(synthetic_project()), None);
    let lower = write_stl(dir.path(), "lower.stl", &tetra(40.0));
    ws.import_files(vec![lower]);
    assert!(ws.wait_for_jobs(WAIT));
    assert!(ws.is_dirty());

    ws.save_to(dir.path().join("case"));
    assert!(ws.wait_for_jobs(WAIT));
    let path = dir.path().join("case.dcprj");
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.clone()));
    assert!(!ws.is_dirty());
    assert_eq!(ws.path(), Some(path.as_path()));

    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(&path, &codecs).unwrap();
    assert!(!loaded.read_only, "{:?}", loaded.issues);
    let original = ws.state();
    let st = &loaded.project.case_state;
    assert_eq!(st.case, original.case);
    assert_eq!(st.case.scans.len(), 2);
    for scan in &st.case.scans {
        assert_eq!(st.mesh(&scan.mesh), original.mesh(&scan.mesh));
    }
    assert_eq!(
        st.module_data::<AlignersState>(dca_model::MODULE_ID),
        original.module_data::<AlignersState>(dca_model::MODULE_ID)
    );

    // reopen and save again in place
    let mut ws2 = Workspace::new_headless(module(), Some(loaded.project), Some(path.clone()));
    assert!(!ws2.is_dirty());
    assert_eq!(ws2.state().case, ws.state().case);
    ws2.save();
    assert!(ws2.wait_for_jobs(WAIT));
    assert_eq!(ws2.take_action(), WorkspaceAction::Saved(path.clone()));
    assert!(dc_project::load(&path, &codecs).is_ok());
}

#[test]
fn save_without_path_asks_for_one() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.save();
    assert_eq!(ws.dialog_requests(), 1);
    let mut ws =
        Workspace::new_headless(module(), None, Some(PathBuf::from("/nonexistent/x.dcprj")));
    ws.set_read_only(true);
    ws.save();
    assert_eq!(ws.dialog_requests(), 1);
}

#[test]
fn home_asks_before_discarding_changes() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    h.get_by_label("Home").click();
    h.run();
    assert_eq!(h.state().actions, vec![WorkspaceAction::GoHome]);

    let mut h = harness(Workspace::new_headless(
        module(),
        Some(synthetic_project()),
        None,
    ));
    h.run();
    let dir = tempfile::tempdir().unwrap();
    let p = write_stl(dir.path(), "lower.stl", &tetra(40.0));
    h.state_mut().ws.import_files(vec![p]);
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    h.run();
    h.get_by_label("Home").click();
    h.run();
    assert!(h.state().actions.is_empty());
    assert!(h.query_by_label("Don't save").is_some());
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().actions.is_empty());
    h.get_by_label("Home").click();
    h.run();
    h.get_by_label("Don't save").click();
    h.run();
    assert_eq!(h.state().actions, vec![WorkspaceAction::GoHome]);
}

#[test]
fn ui_with_context_entry_point() {
    let ctx = egui::Context::default();
    dc_ui::theme::install(&ctx, dc_ui::Mode::Light);
    let mut ws = Workspace::new_headless(module(), Some(synthetic_project()), None);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    for _ in 0..3 {
        let mut action = WorkspaceAction::None;
        ctx.run_pass(input(), |ctx| action = ws.ui(ctx))
            .drop_without_applying_deltas();
        assert_eq!(action, WorkspaceAction::None);
    }
}

// ---------------------------------------------------------------- scan import

const IMPORT_ID: &str = "aligners.case.import-scans";

/// A mesh with an extra vertex per call so two files differ.
fn scan_mesh(size: f32) -> TriMesh {
    tetra(size)
}

/// Project whose upper scan has results the module clears when it changes.
fn project_with_results() -> Project {
    let mut p = synthetic_project();
    let mut al = p
        .case_state
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .unwrap()
        .clone();
    al.seeds.push(Seed {
        fdi: Fdi::new(11).unwrap(),
        position: DVec3::new(1.0, -30.0, 2.0),
    });
    p.case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(al));
    p
}

fn aligners(ws: &Workspace) -> AlignersState {
    ws.state()
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .unwrap()
        .clone()
}

/// A file handle for `Event`-less drops.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

#[test]
fn import_leaf_asks_for_files_and_queues_nothing() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.activate(IMPORT_ID);
    assert_eq!(ws.selected(), Some(IMPORT_ID));
    assert_eq!(ws.dialog_requests(), 1);
    assert!(!ws.is_busy(), "no command was queued");
    assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
}

#[test]
fn partial_batch_imports_the_good_file_and_reports_the_bad_one() {
    let dir = tempfile::tempdir().unwrap();
    let good = write_stl(dir.path(), "patient_lower.stl", &scan_mesh(40.0));
    let bad = dir.path().join("broken.stl");
    std::fs::write(&bad, b"solid nope\nfacet garbage\n").unwrap();
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.import_files(vec![good, bad]);
    assert!(ws.wait_for_jobs(WAIT));
    let st = ws.state();
    assert_eq!(st.case.scans.len(), 1);
    assert_eq!(st.case.scans[0].role, ScanRole::Lower);
    assert_eq!(st.case.scans[0].role_source, RoleSource::FileName);
    let details = ws.toast_details();
    let failed: Vec<_> = details
        .iter()
        .filter(|(k, _)| k == "ws-toast-import-file-failed")
        .collect();
    assert_eq!(failed.len(), 1, "{details:?}");
    assert!(
        failed[0].1.as_deref().unwrap().contains("broken.stl"),
        "{details:?}"
    );
    assert!(
        details
            .iter()
            .any(|(k, d)| k == "ws-toast-imported"
                && d.as_deref().is_some_and(|d| d.starts_with('1')))
    );
    // one undo step for the whole batch
    assert!(ws.undo());
    assert!(ws.state().case.scans.is_empty());
}

#[test]
fn dropped_files_are_filtered_by_extension() {
    let dir = tempfile::tempdir().unwrap();
    let stl = write_stl(dir.path(), "UPPER.STL", &scan_mesh(40.0));
    let txt = dir.path().join("notes.txt");
    std::fs::write(&txt, b"not a scan").unwrap();
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    h.input_mut().dropped_files.extend([
        Arc::new(Dropped(txt)) as egui::DroppedFileHandle,
        Arc::new(Dropped(stl)),
    ]);
    settle(&mut h);
    let ws = &h.state().ws;
    assert_eq!(ws.state().case.scans.len(), 1);
    assert_eq!(ws.state().case.scans[0].source_name, "UPPER.STL");
    let details = ws.toast_details();
    let ignored = details
        .iter()
        .find(|(k, _)| k == "ws-toast-drop-ignored")
        .unwrap_or_else(|| panic!("{details:?}"));
    assert_eq!(ignored.1.as_deref(), Some("notes.txt"));

    // only unsupported files: nothing is imported, nothing runs
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.import_dropped(vec![PathBuf::from("a.txt"), PathBuf::from("b.3mf")]);
    assert!(!ws.is_busy());
    assert!(ws.state().case.scans.is_empty());
    assert_eq!(ws.toast_keys(), ["ws-toast-drop-ignored"]);
}

#[test]
fn dropped_files_are_ignored_while_busy_and_hovering_draws() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_stl(dir.path(), "a_upper.stl", &scan_mesh(40.0));
    let b = write_stl(dir.path(), "b_lower.stl", &scan_mesh(41.0));
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.import_files(vec![a]);
    assert!(ws.is_busy());
    ws.import_dropped(vec![b]);
    assert_eq!(ws.toast_keys(), ["ws-toast-busy"]);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans.len(), 1);

    // the overlay while files are held over the window
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.input_mut().hovered_files.push(egui::HoveredFile {
        path: None,
        mime: String::new(),
    });
    h.step();
    h.input_mut().hovered_files.clear();
    h.run();
}

#[test]
fn import_and_commands_exclude_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_stl(dir.path(), "a_upper.stl", &scan_mesh(40.0));
    let b = write_stl(dir.path(), "b_lower.stl", &scan_mesh(41.0));
    let c = write_stl(dir.path(), "c_bite.stl", &scan_mesh(42.0));
    let command = |path: &Path| {
        let spec = dca_module::commands::find(IMPORT_ID).unwrap();
        Box::new(spec.command(json!({ "path": path.display().to_string() })))
    };

    // an import runs: a command, a second import and undo wait
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.import_files(vec![a.clone()]);
    ws.run_command(command(&b));
    ws.import_files(vec![c.clone()]);
    assert!(!ws.undo());
    assert!(!ws.redo());
    let busy = ws
        .toast_keys()
        .iter()
        .filter(|k| *k == "ws-toast-busy")
        .count();
    assert_eq!(busy, 2, "{:?}", ws.toast_keys());
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans.len(), 1);
    assert_eq!(ws.state().case.scans[0].source_name, "a_upper.stl");

    // a command runs: an import and undo wait
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.run_command(command(&a));
    ws.import_files(vec![b]);
    assert!(!ws.undo());
    assert_eq!(ws.toast_keys(), ["ws-toast-busy"]);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans.len(), 1);
    assert_eq!(ws.state().case.scans[0].source_name, "a_upper.stl");
    // the one result is one undo step
    assert!(ws.undo());
    assert!(ws.state().case.scans.is_empty());
    assert!(!ws.undo());
}

#[test]
fn replacing_a_scan_says_what_was_cleared_and_undo_restores_everything() {
    let dir = tempfile::tempdir().unwrap();
    let new_upper = write_stl(dir.path(), "new_upper.stl", &scan_mesh(55.0));
    let mut h = harness(Workspace::new_headless(
        module(),
        Some(project_with_results()),
        None,
    ));
    h.run();
    let before = h.state().ws.state().clone();
    let old_id = before.case.scans[0].id;
    assert!(!aligners(&h.state().ws).seeds.is_empty());

    h.state_mut().ws.import_files(vec![new_upper]);
    settle(&mut h);
    {
        let ws = &h.state().ws;
        assert_eq!(ws.state().case.scans.len(), 1);
        assert_ne!(ws.state().case.scans[0].id, old_id);
        assert_eq!(ws.state().case.scans[0].source_name, "new_upper.stl");
        let al = aligners(ws);
        assert!(al.seeds.is_empty() && al.stages.is_empty());
        let details = ws.toast_details();
        let cleared = details
            .iter()
            .find(|(k, _)| k == "ws-toast-import-cleared")
            .unwrap_or_else(|| panic!("{details:?}"));
        let text = cleared.1.as_deref().unwrap();
        assert!(text.contains("tooth seeds"), "{text}");
        assert!(text.contains("aligner stages"), "{text}");
        assert!(!text.to_lowercase().contains("reset"), "{text}");
        let keys = ws.toast_keys();
        assert!(
            keys.contains(&"ws-toast-warn-replaced".to_owned()),
            "{keys:?}"
        );
        assert_eq!(keys.last().map(String::as_str), Some("ws-toast-imported"));
    }

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    let ws = &h.state().ws;
    assert_eq!(ws.state().case.scans.len(), 1);
    assert_eq!(ws.state().case.scans[0].id, old_id);
    assert_eq!(aligners(ws), aligners_of(&before));
    assert!(!ws.is_dirty());
}

fn aligners_of(state: &CaseState) -> AlignersState {
    state
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .unwrap()
        .clone()
}

#[test]
fn the_role_choice_reaches_the_command_and_resets() {
    let dir = tempfile::tempdir().unwrap();
    // no hint in the name: "auto" would make it provisional
    let a = write_stl(dir.path(), "scan1.stl", &scan_mesh(40.0));
    let b = write_stl(dir.path(), "scan2.stl", &scan_mesh(41.0));
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.state_mut().ws.activate(IMPORT_ID);
    h.run();
    assert_eq!(h.state().ws.import_role(), None);
    // the control in the inspector
    h.get_by_label("Bite").click();
    h.run();
    assert_eq!(h.state().ws.import_role(), Some(ScanRole::Bite));

    h.state_mut().ws.import_files(vec![a]);
    settle(&mut h);
    {
        let scan = &h.state().ws.state().case.scans[0];
        assert_eq!(scan.role, ScanRole::Bite);
        assert_eq!(scan.role_source, RoleSource::Explicit);
    }
    // back to auto: the next file is a guess
    assert_eq!(h.state().ws.import_role(), None);
    h.state_mut().ws.import_files(vec![b]);
    settle(&mut h);
    let st = h.state().ws.state().clone();
    let second = st.case.scans.iter().find(|s| s.source_name == "scan2.stl");
    assert_eq!(second.unwrap().role_source, RoleSource::Provisional);

    // an import refused as busy keeps the choice
    h.state_mut().ws.import_files(vec![PathBuf::from("x.stl")]);
    h.state_mut().ws.set_import_role(Some(ScanRole::Upper));
    h.state_mut().ws.import_files(vec![PathBuf::from("y.stl")]);
    assert_eq!(h.state().ws.import_role(), Some(ScanRole::Upper));
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
    // a file that cannot be read still used the choice
    h.state_mut().ws.import_files(vec![PathBuf::from("z.stl")]);
    assert_eq!(h.state().ws.import_role(), None);
    assert!(h.state_mut().ws.wait_for_jobs(WAIT));
}

#[test]
fn a_provisional_scan_is_marked_and_the_step_is_not_done() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_stl(dir.path(), "scanA.stl", &scan_mesh(40.0));
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.state_mut().ws.activate(IMPORT_ID);
    h.state_mut().ws.import_files(vec![a]);
    settle(&mut h);
    assert_eq!(h.state().ws.state().case.scans[0].role, ScanRole::Lower);
    assert!(h.query_by_label("scanA.stl").is_some());
    assert!(h.query_by_label("Provisional").is_some());
    assert!(h.query_by_label_contains("The arch is a guess").is_some());
    // a guess is not a finished step
    let label = format!("Treatment workflow 0/{}", leaf_count());
    assert!(h.query_by_label(&label).is_some());

    // confirming it settles the step, and one undo takes it back
    h.get_by_label("Confirm").click();
    settle(&mut h);
    assert_eq!(
        h.state().ws.state().case.scans[0].role_source,
        RoleSource::Explicit
    );
    assert!(h.query_by_label("Provisional").is_none());
    let label = format!("Treatment workflow 1/{}", leaf_count());
    assert!(h.query_by_label(&label).is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(
        h.state().ws.state().case.scans[0].role_source,
        RoleSource::Provisional
    );
    assert!(h.query_by_label("Provisional").is_some());
}

#[test]
fn changing_a_role_in_the_inspector_is_undoable() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_stl(dir.path(), "scanA.stl", &scan_mesh(40.0));
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.state_mut().ws.activate(IMPORT_ID);
    h.state_mut().ws.import_files(vec![a]);
    settle(&mut h);
    let id = h.state().ws.state().case.scans[0].id;
    assert_eq!(h.state().ws.state().case.scans[0].role, ScanRole::Lower);

    // the scan's row comes after the control for the next import
    let uppers: Vec<_> = h
        .get_all_by_role_and_label(Role::RadioButton, "Upper")
        .collect();
    assert_eq!(uppers.len(), 2);
    uppers.last().unwrap().click();
    settle(&mut h);
    {
        let scan = &h.state().ws.state().case.scans[0];
        assert_eq!(scan.id, id);
        assert_eq!(scan.role, ScanRole::Upper);
        assert_eq!(scan.role_source, RoleSource::Explicit);
    }
    assert!(h.state().ws.is_dirty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    let scan = &h.state().ws.state().case.scans[0];
    assert_eq!(scan.role, ScanRole::Lower);
    assert_eq!(scan.role_source, RoleSource::Provisional);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().ws.state().case.scans[0].role, ScanRole::Upper);
}

#[test]
fn setting_a_role_reports_what_the_module_cleared() {
    let mut ws = Workspace::new_headless(module(), Some(project_with_results()), None);
    let upper = ws.state().case.scans[0].id;
    ws.set_scan_role(upper, ScanRole::Upper, ScanRole::Lower);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans[0].role, ScanRole::Lower);
    assert!(aligners(&ws).seeds.is_empty());
    // a role change is not a replacement: the notice says what happened
    assert_eq!(ws.toast_keys(), ["ws-toast-role-changed"]);
    let text = ws.toast_details()[0].1.clone().unwrap();
    assert!(text.contains("tooth seeds"), "{text}");
    assert!(text.contains("Ctrl+Z brings the previous roles"), "{text}");
    for word in ["replaced", "reset"] {
        assert!(!text.to_lowercase().contains(word), "{text}");
    }
    assert!(ws.sticky_toast_keys().is_empty());
    // there is no scan on that role any more: the command fails with a toast
    ws.set_scan_role(upper, ScanRole::Upper, ScanRole::Lower);
    assert!(ws.wait_for_jobs(WAIT));
    assert!(
        ws.toast_keys()
            .contains(&"ws-toast-command-failed".to_owned())
    );
    assert!(ws.undo());
    assert_eq!(ws.state().case.scans[0].role, ScanRole::Upper);
    assert!(!aligners(&ws).seeds.is_empty());
}

#[test]
fn show_import_report_is_bounded_and_public() {
    let mut ws = Workspace::new_headless(module(), None, None);
    let warnings: Vec<_> = (0..12)
        .map(|n| json!({ "code": "tiny", "key": "k", "args": {}, "message": format!("warning {n}") }))
        .collect();
    ws.show_import_report(&json!({
        "imported": [{ "scan_id": "x" }, { "scan_id": "y" }],
        "warnings": warnings,
        "failed": [{ "path": "a.stl", "code": "unreadable", "reason": "a.stl: cannot read", "detail": null }],
        "module_cleared": [],
    }));
    let keys = ws.toast_keys();
    assert_eq!(keys.len(), 4, "{keys:?}");
    assert_eq!(keys[0], "ws-toast-import-file-failed");
    assert!(keys.contains(&"ws-toast-import-more".to_owned()));
    assert_eq!(keys[3], "ws-toast-imported");
    let details = ws.toast_details();
    assert_eq!(details[3].1.as_deref(), Some("2 scans added to the case."));
    // nothing imported and nothing to say
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.show_import_report(&json!({}));
    assert!(ws.toast_keys().is_empty());
}

#[test]
fn a_move_without_replacement_is_a_role_change_and_the_bite_registration_is_to_redo() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.show_import_report(&json!({
        "imported": [{ "scan_id": "x" }],
        "replaced": false,
        "moved": [{ "from": "upper", "to": "lower" }],
        "module_cleared": ["seeds", "bite_registered"],
    }));
    let keys = ws.toast_keys();
    assert_eq!(keys, ["ws-toast-role-changed", "ws-toast-imported"]);
    let text = ws.toast_details()[0].1.clone().unwrap();
    assert!(text.contains("tooth seeds"), "{text}");
    assert!(
        text.contains("bite registration (to be redone"),
        "the registration is work to redo, not data removed: {text}"
    );
    assert!(!text.to_lowercase().contains("replaced"), "{text}");
    // a replacement keeps its own wording
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.show_import_report(&json!({
        "replaced": true,
        "module_cleared": ["bite_registered"],
    }));
    assert_eq!(ws.toast_keys(), ["ws-toast-import-cleared"]);
}

#[test]
fn arch_warnings_and_failed_files_stay_in_full_and_the_rest_is_grouped() {
    let mut ws = Workspace::new_headless(module(), None, None);
    let long = format!("{} fine", "parola ".repeat(80));
    let mut warnings = vec![
        json!({ "code": "bite_limit", "key": "k", "args": {}, "message": "limite" }),
        json!({ "code": "tiny", "key": "k", "args": {}, "message": "piccolo" }),
        json!({ "code": "provisional", "key": "aligners-arch-warn-to-be-checked", "args": {}, "message": long }),
        json!({ "code": "arch_conflict", "key": "k", "args": {}, "message": "arcata occupata" }),
    ];
    warnings.extend(
        (0..6).map(|n| json!({ "code": "low_polygons", "key": "k", "args": {}, "message": format!("poche {n}") })),
    );
    ws.show_import_report(&json!({
        "imported": [{ "scan_id": "x" }],
        "failed": [{ "reason": "a.stl: unreadable" }],
        "module_cleared": ["seeds"],
        "replaced": true,
        "warnings": warnings,
    }));
    let keys = ws.toast_keys();
    assert_eq!(
        keys[..4],
        [
            "ws-toast-import-file-failed",
            "ws-toast-warn-arch-conflict",
            "ws-toast-warn-provisional",
            "ws-toast-import-cleared"
        ],
        "{keys:?}"
    );
    assert_eq!(keys.last().map(String::as_str), Some("ws-toast-imported"));
    // sticky: the failed file and the two arch warnings, none else
    assert_eq!(
        ws.sticky_toast_keys(),
        [
            "ws-toast-import-file-failed",
            "ws-toast-warn-arch-conflict",
            "ws-toast-warn-provisional"
        ]
    );
    let details = ws.toast_details();
    assert_eq!(details[1].1.as_deref(), Some("arcata occupata"));
    assert_eq!(details[2].1.as_deref(), Some(long.as_str()), "not cut");
}

#[test]
fn the_close_control_dismisses_a_sticky_arch_warning() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.show_import_report(&json!({
        "warnings": [
            { "code": "provisional", "key": "aligners-arch-warn-to-be-checked", "args": {}, "message": "check the arch" },
        ],
    }));
    assert_eq!(ws.sticky_toast_keys(), ["ws-toast-warn-provisional"]);
    let mut h = harness(ws);
    h.run();
    h.get_by_role_and_label(Role::Button, "Close Check the arch")
        .click();
    h.run();
    assert!(h.state().ws.toast_keys().is_empty());
}
