//! Headless tests of the workspace: rendering (egui_kittest, no GPU), menu
//! search, every menu command, import with undo/redo, save/load.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dc_case::{Case, CaseState, Fdi, ModuleState, Pose, Scan, ScanRole};
use dc_mesh::{MeshFormat, TriMesh, Units};
use dc_module_api::{
    CommandDesc, MenuTree, Module, ModuleCodec, ModuleCtx, ModuleError, NOT_AVAILABLE_KEY,
};
use dc_project::{AppInfo, Project};
use dc_workspace::{Workspace, WorkspaceAction};
use dca_model::{AlignersState, IprStep, Stage};
use dca_module::AlignersModule;
use egui::{Key, Modifiers, Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;

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

fn module() -> Box<dyn Module> {
    Box::new(AlignersModule::new())
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
    assert!(h.query_by_label("Treatment workflow 0/48").is_some());
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
    assert!(h.query_by_label("Treatment workflow 1/48").is_some());
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
    // all leaves are placeholders except the import, run by the workspace
    let keys = ws.toast_keys();
    assert_eq!(keys.len(), leaves.len() - 1);
    assert!(keys.iter().all(|k| k == NOT_AVAILABLE_KEY));
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
        h.run();
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
        h.run();
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
        h.run();
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
    h.run();
    assert_eq!(
        h.state().ws.selected(),
        Some("aligners.planning.staging.automatic")
    );
    assert_eq!(
        log.lock().unwrap().last().map(String::as_str),
        Some("aligners.planning.staging.automatic")
    );
    h.key_press(Key::A);
    h.run();
    assert_eq!(
        h.state().ws.selected(),
        Some("aligners.preparation.segmentation.automatic")
    );
    assert!(
        h.state()
            .ws
            .toast_keys()
            .iter()
            .any(|k| k == NOT_AVAILABLE_KEY)
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
        assert!(ws.toast_keys().contains(&"ws-toast-imported".to_owned()));
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
