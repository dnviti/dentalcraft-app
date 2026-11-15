//! Headless tests of the workspace: rendering (egui_kittest, no GPU), menu
//! search, every menu command, import with undo/redo, save/load.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dc_case::{Case, CaseState, Fdi, ModuleState, Pose, RoleSource, Scan, ScanRole};
use dc_mesh::{MeshFormat, TriMesh, Units};
use dc_module_api::{CommandDesc, MenuTree, Module, ModuleCodec, ModuleCtx, ModuleError};
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

/// Clicks the widget labelled `label`, scrolled into view first: the
/// inspector of a function can be longer than the window.
fn click_in_view(h: &mut Harness<'static, App>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    h.run();
    h.get_by_label(label).click();
}

fn module() -> Box<dyn Module> {
    Box::new(AlignersModule::new())
}

/// Number of steps of the workflow (the total of the footer): not the
/// shortcuts, the exports and the corrections.
fn leaf_count() -> usize {
    let steps = module().menu().step_ids().len();
    assert_eq!(steps, 9);
    steps
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
    fn command_specs(&self) -> Vec<dc_module_api::CommandSpec> {
        self.inner.command_specs()
    }
    fn invoke(&mut self, id: &str, ctx: &mut ModuleCtx<'_>) -> Result<(), ModuleError> {
        self.inner.invoke(id, ctx)
    }
    fn leaf_done(&self, state: &CaseState, id: &str) -> bool {
        self.inner.leaf_done(state, id)
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
    synthetic_project_with(&[ScanRole::Upper])
}

/// The synthetic project with one tetrahedron scan per role.
fn synthetic_project_with(roles: &[ScanRole]) -> Project {
    let mut state = CaseState::new(Case::new("AL-TEST-01"));
    let blob = state.blobs.insert(tetra(50.0));
    for role in roles {
        state.case.scans.push(Scan {
            id: uuid::Uuid::new_v4(),
            role: *role,
            role_source: RoleSource::Explicit,
            source_name: format!("{role:?}.stl").to_lowercase(),
            source_units: Units::Millimeters,
            import_xform: Pose::default(),
            mesh: blob,
        });
    }
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
    // the arch button in the toolbar changes the caption
    h.get_by_label("Lower").click();
    h.run();
    assert!(h.query_by_label_contains("Upper jaw, occlusal").is_some());
}

#[test]
fn the_arch_buttons_choose_the_arches_and_no_view_changes_them() {
    let project = synthetic_project_with(&[ScanRole::Upper, ScanRole::Lower]);
    let ws = Workspace::new_headless(module(), Some(project), None);
    let mut h = harness(ws);
    h.run();
    let click = |h: &mut Harness<'static, App>, label: &str| {
        h.get_by_label(label).click();
        h.run();
    };
    let caption = |h: &Harness<'static, App>, s: &str| h.query_by_label_contains(s).is_some();
    assert!(caption(&h, "Both jaws, occlusal"));
    // the occlusal view keeps both arches
    click(&mut h, "Occlusal");
    assert!(caption(&h, "Both jaws, occlusal"));
    // lower only: the occlusal view looks at it from above
    click(&mut h, "Upper");
    assert!(caption(&h, "Lower jaw, occlusal view from above"));
    // front, left, back: the choice stays
    for (view, word) in [("Front", "front"), ("Left", "left"), ("Back", "back")] {
        click(&mut h, view);
        assert!(caption(&h, &format!("Lower jaw, {word} view")), "{view}");
    }
    // the last arch stays on
    click(&mut h, "Lower");
    assert!(caption(&h, "Lower jaw, back view"));
    // both again
    click(&mut h, "Upper");
    assert!(caption(&h, "Both jaws, back view"));
}

#[test]
fn menu_search_filters() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    assert!(h.query_by_label("Run everything automatically").is_some());
    // `/` focuses the filter, typing filters the tree
    h.key_press(Key::Slash);
    h.run();
    h.event(egui::Event::Text("gum".into()));
    h.run();
    assert_eq!(h.state().ws.filter(), "gum");
    assert!(h.query_by_label("Run everything automatically").is_none());
    assert!(h.query_by_label("Separate teeth and gum").is_some());
    assert!(h.query_by_label("Teeth").is_some());
    // Escape clears the filter
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(h.state().ws.filter(), "");
    // the manual tools are not entries of the menu
    h.state_mut().ws.set_filter("brush");
    h.run();
    assert!(h.query_by_label("No tool matches the search.").is_some());
    // nothing matches
    h.state_mut().ws.set_filter("zzz-nothing");
    h.run();
    assert!(h.query_by_label("No tool matches the search.").is_some());
    let ids = h.state().ws.visible_menu_ids();
    assert!(ids.is_empty());
    h.state_mut().ws.set_filter("bite");
    assert!(
        h.state()
            .ws
            .visible_menu_ids()
            .contains(&"aligners.occlusion.align")
    );
}

/// A tool of a function: the label of its button, its command and the label
/// of one of its controls.
type ToolRow = (&'static str, &'static str, &'static str);

/// The functions with manual corrections and their tools.
const TOOLS: [(&str, &[ToolRow]); 5] = [
    (
        "aligners.teeth.find",
        &[
            (
                "Mark a tooth",
                "aligners.preparation.numbering.place-seeds",
                "Number the teeth in order",
            ),
            (
                "Delete a tooth",
                "aligners.preparation.numbering.delete-seed",
                "Click a tooth number, a tooth mark or the tooth on the scan to delete its mark. The tooth is not marked missing.",
            ),
            (
                "Change a tooth number",
                "aligners.preparation.numbering.renumber",
                "New number",
            ),
            (
                "Missing tooth",
                "aligners.preparation.numbering.mark-missing",
                "The tooth is missing",
            ),
        ],
    ),
    (
        "aligners.preparation.segmentation.automatic",
        &[
            (
                "Tooth brush",
                "aligners.preparation.segmentation.manual-brush",
                "Radius",
            ),
            (
                "Clean borders",
                "aligners.preparation.segmentation.clean-borders",
                "Smoothing rings",
            ),
        ],
    ),
    (
        "aligners.teeth.build",
        &[(
            "Rebuild a tooth",
            "aligners.manual.rebuild-tooth",
            "Find the region again from the tooth mark",
        )],
    ),
    (
        "aligners.planning.setup.automatic",
        &[
            // no tooth chosen yet: the panel asks for one
            (
                "Move a tooth",
                "aligners.planning.movements",
                "Choose a tooth with 3D teeth built: click it in the view or enter its number.",
            ),
            (
                "Arch line",
                "aligners.planning.setup.line",
                "There is no arch line yet: compute the final position first.",
            ),
        ],
    ),
    (
        "aligners.occlusion.align",
        &[(
            "Three points",
            "aligners.occlusion.bite-registration.three-points",
            "Take the last point back",
        )],
    ),
];

#[test]
fn every_function_with_tools_shows_them_and_their_controls_in_the_inspector() {
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    let mut h = harness(ws);
    h.run();
    // the inspector is the panel on the right
    let inspector = 1920.0 - 360.0;
    for (function, tools) in TOOLS {
        h.state_mut().ws.select_item(function);
        h.run();
        assert!(
            h.query_by_label("Manual corrections").is_some(),
            "{function}"
        );
        for (button, command, control) in tools {
            assert!(h.query_by_label(control).is_none(), "{control} before");
            let r = h.get_by_label(button).rect();
            assert!(r.min.x >= inspector, "{button}: {r:?}");
            click_in_view(&mut h, button);
            h.run();
            let ws = &h.state().ws;
            assert_eq!(ws.active_tool(), Some(*command), "{button}");
            assert_eq!(ws.selected(), Some(function), "{button}");
            assert!(!ws.is_busy(), "opening {button} ran something");
            // its controls are in the inspector, not over the viewport
            let r = h.get_by_label(control).rect();
            assert!(r.min.x >= inspector, "{control}: {r:?}");
        }
        // a second click on the open tool closes it
        let (last, ..) = tools[tools.len() - 1];
        click_in_view(&mut h, last);
        h.run();
        assert_eq!(h.state().ws.active_tool(), None, "{last}");
    }
    // a function without tools has no such section
    h.state_mut().ws.select_item("aligners.output.study-models");
    h.run();
    assert!(h.query_by_label("Manual corrections").is_none());
    let ws = &h.state().ws;
    assert_eq!(ws.dialog_requests(), 0);
    assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
}

#[test]
fn a_leaf_that_needs_an_answer_asks_for_it_only_from_its_button() {
    // headless: the dialog is counted, not shown
    let mut ws = Workspace::new_headless(module(), None, None);
    for (n, id) in ["aligners.output.export-all", "aligners.followup.visits"]
        .into_iter()
        .enumerate()
    {
        // the selection asks for nothing
        ws.select_item(id);
        assert_eq!(ws.selected(), Some(id));
        assert_eq!(ws.dialog_requests(), n, "{id}");
        // the button of its inspector asks, and runs nothing before the answer
        ws.run_leaf(id);
        assert!(!ws.is_busy(), "{id}");
        assert_eq!(ws.dialog_requests(), n + 1, "{id}");
    }
    assert!(ws.toast_keys().is_empty());
}

/// The menu law: selecting a function, done or not, only shows its
/// inspector. No command, no dialog, no tool, no notice; the case is as it
/// was.
#[test]
fn selecting_any_function_runs_nothing_done_or_not() {
    for (project, done_at_least) in [(None, 0), (Some(demo_project(&["lower", "upper"])), 3)] {
        let mut ws = Workspace::new_headless(module(), project, None);
        let before = Arc::clone(ws.state());
        let leaves = ws.module().menu().command_ids();
        assert_eq!(leaves.len(), 14);
        let mut done = 0;
        for id in leaves {
            assert!(dca_module::commands::is_registered(id), "{id}");
            done += usize::from(ws.module().leaf_done(ws.state(), id));
            ws.select_item(id);
            assert_eq!(ws.selected(), Some(id));
            assert!(!ws.is_busy(), "{id} ran");
            assert_eq!(ws.dialog_requests(), 0, "{id} asked for a file");
            assert_eq!(ws.active_tool(), None, "{id} opened a tool");
            assert!(ws.toast_keys().is_empty(), "{id}: {:?}", ws.toast_keys());
        }
        assert!(done >= done_at_least, "{done} done");
        assert!(ws.wait_for_jobs(Duration::ZERO), "a job runs");
        assert!(Arc::ptr_eq(ws.state(), &before), "the case changed");
    }
}

#[test]
fn a_click_on_any_row_only_selects_it() {
    // the demo has steps done and steps to do
    let mut ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    ws.expand_all();
    let before = Arc::clone(ws.state());
    let mut h = harness(ws);
    h.run();
    let n = h.get_all_by_role(Role::TreeItem).count();
    assert_eq!(n, 14 + 6, "the functions and their groups, nothing else");
    // bottom-up: collapsing a group only removes rows below it
    for i in (0..n).rev() {
        let nodes: Vec<_> = h.get_all_by_role(Role::TreeItem).collect();
        nodes[i].click();
        h.run();
        let ws = &h.state().ws;
        assert!(ws.selected().is_some());
        assert!(!ws.is_busy(), "{:?} ran", ws.selected());
        assert_eq!(ws.dialog_requests(), 0, "{:?}", ws.selected());
        assert_eq!(ws.active_tool(), None, "{:?}", ws.selected());
        assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
    }
    assert!(Arc::ptr_eq(h.state().ws.state(), &before));
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
        log.iter()
            .any(|id| id == "aligners.diagnosis.space-analysis"),
        "{log:?}"
    );
}

#[test]
fn module_shortcuts_select_and_never_run() {
    let (m, log) = spy();
    let mut h = harness(Workspace::new_headless(m, None, None));
    h.run();
    h.key_press(Key::S);
    h.run();
    assert_eq!(h.state().ws.selected(), Some("aligners.planning.aligners"));
    assert_eq!(
        log.lock().unwrap().last().map(String::as_str),
        Some("aligners.planning.aligners")
    );
    assert!(!h.state().ws.is_busy(), "S ran the aligners");
    h.key_press(Key::A);
    h.run();
    assert_eq!(
        h.state().ws.selected(),
        Some("aligners.preparation.segmentation.automatic")
    );
    assert!(!h.state().ws.is_busy(), "A ran the separation");
    assert_eq!(h.state().ws.active_tool(), None);
    // B: the same function, with its brush open
    h.key_press(Key::S);
    h.run();
    h.key_press(Key::B);
    h.run();
    let ws = &h.state().ws;
    assert_eq!(
        ws.selected(),
        Some("aligners.preparation.segmentation.automatic")
    );
    assert_eq!(
        ws.active_tool(),
        Some("aligners.preparation.segmentation.manual-brush")
    );
    assert!(!ws.is_busy());
    // nothing ran, nothing was asked
    assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
    assert_eq!(ws.dialog_requests(), 0);
    assert!(h.query_by_label("Radius").is_some(), "the brush controls");
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
fn import_leaf_asks_for_files_only_from_its_button_and_queues_nothing() {
    let mut ws = Workspace::new_headless(module(), None, None);
    ws.select_item(IMPORT_ID);
    assert_eq!(ws.selected(), Some(IMPORT_ID));
    assert_eq!(ws.dialog_requests(), 0, "the selection asks for nothing");
    ws.run_leaf(IMPORT_ID);
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
        assert!(text.contains("tooth marks"), "{text}");
        assert!(text.contains("aligner sequence"), "{text}");
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
    h.state_mut().ws.select_item(IMPORT_ID);
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
    h.state_mut().ws.select_item(IMPORT_ID);
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
    h.state_mut().ws.select_item(IMPORT_ID);
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
fn moving_an_arch_scan_renumbers_it_and_a_bite_role_clears_it() {
    let mut ws = Workspace::new_headless(module(), Some(project_with_results()), None);
    let upper = ws.state().case.scans[0].id;
    // upper to lower: the module's set-arch renumbers the marks (the plugin's
    // arch_set) and says what is gone (the aligners of the arch)
    ws.set_scan_role(upper, ScanRole::Upper, ScanRole::Lower);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans[0].role, ScanRole::Lower);
    assert_eq!(ws.state().case.scans[0].role_source, RoleSource::Explicit);
    let fdis: Vec<u8> = aligners(&ws).seeds.iter().map(|s| s.fdi.get()).collect();
    assert_eq!(fdis, [41]);
    assert_eq!(
        ws.toast_keys(),
        ["ws-toast-role-changed", "ws-toast-arch-set"]
    );
    let text = ws.toast_details()[0].1.clone().unwrap();
    assert!(text.contains("aligner sequence"), "{text}");
    assert!(text.contains("Ctrl+Z brings the previous roles"), "{text}");
    let text = ws.toast_details()[1].1.clone().unwrap();
    assert!(text.contains("renumbered for the lower arch"), "{text}");
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
    let fdis: Vec<u8> = aligners(&ws).seeds.iter().map(|s| s.fdi.get()).collect();
    assert_eq!(fdis, [11]);

    // an arch scan that becomes the bite: the shell's set-role, the arch cleared
    ws.set_scan_role(upper, ScanRole::Upper, ScanRole::Bite);
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.state().case.scans[0].role, ScanRole::Bite);
    assert!(aligners(&ws).seeds.is_empty());
    assert!(
        ws.toast_keys()
            .contains(&"ws-toast-role-changed".to_owned())
    );
}

/// The demo lower arch opened under the gum (the demo scan is closed): the
/// faces of its lowest 4 mm along the occlusal axis (+Z) removed.
fn open_lower_project() -> Project {
    let mut p = demo_project(&["lower"]);
    let state = &mut p.case_state;
    let mesh = TriMesh::clone(state.mesh(&state.case.scans[0].mesh).unwrap());
    let low = mesh
        .positions
        .iter()
        .map(|p| p.z)
        .fold(f32::INFINITY, f32::min)
        + 4.0;
    let mut open = mesh.clone();
    open.tris
        .retain(|t| !t.iter().all(|&i| mesh.positions[i as usize].z < low));
    state.case.scans[0].mesh = state.blobs.insert(open);
    p
}

/// The line under the status of an arch scan open underneath.
const OPEN_HINT: &str = "The scan is open underneath: close it to see and use it as a solid model.";

#[test]
fn the_import_inspector_holds_the_flip_and_the_base_and_selecting_it_runs_nothing() {
    // a scan that came in closed: said so, and nothing to close
    let mut h = harness(Workspace::new_headless(
        module(),
        Some(demo_project(&["lower"])),
        None,
    ));
    h.state_mut().ws.select_item(IMPORT_ID);
    h.run();
    assert!(h.query_by_label("Closed as read").is_some());
    assert!(
        h.query_by_label("The scan came in closed: it needs no base.")
            .is_some()
    );
    assert!(h.query_by_label_contains("Close with a base").is_none());

    let mut h = harness(Workspace::new_headless(
        module(),
        Some(open_lower_project()),
        None,
    ));
    let before = Arc::clone(h.state().ws.state());
    // the menu law: selecting the function runs nothing
    h.state_mut().ws.select_item(IMPORT_ID);
    h.run();
    assert!(!h.state().ws.is_busy());
    assert!(
        Arc::ptr_eq(&before, h.state().ws.state()),
        "the selection changed the case"
    );
    // its manual corrections: the flip of the arch scan, and the base with
    // the status of the scan and its run button
    for label in [
        "Flip the occlusal side",
        "Base under the scans",
        "Base height",
        OPEN_HINT,
        "Close with a base \u{b7} Lower",
    ] {
        assert!(h.query_by_label_contains(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Flipped by hand").is_none());
    // the base: one undoable change; the arch has its tooth marks, so the
    // base is level with them, and the button runs it again
    click_in_view(&mut h, "Close with a base \u{b7} Lower");
    settle(&mut h);
    assert!(
        h.state()
            .ws
            .toast_keys()
            .contains(&"ws-toast-base-done".to_owned())
    );
    let scan = &h.state().ws.state().case.scans[0];
    assert!(
        h.state()
            .ws
            .state()
            .mesh(&scan.mesh)
            .unwrap()
            .validate()
            .is_watertight()
    );
    assert!(h.query_by_label("Closed on the tooth marks").is_some());
    assert!(h.query_by_label("Run again \u{b7} Lower").is_some());
    assert!(h.query_by_label("Close with a base \u{b7} Lower").is_none());
    // the flip: one undoable change, shown on the scan
    click_in_view(&mut h, "Flip the occlusal side");
    settle(&mut h);
    assert!(
        aligners(&h.state().ws)
            .occlusal_flipped
            .contains(&ScanRole::Lower)
    );
    assert!(h.query_by_label("Flipped by hand").is_some());
    assert!(
        h.state()
            .ws
            .toast_keys()
            .contains(&"ws-toast-flipped".to_owned())
    );
    // undone twice: the open scan again
    assert!(h.state_mut().ws.undo());
    assert!(h.state_mut().ws.undo());
    h.run();
    assert!(aligners(&h.state().ws).occlusal_flipped.is_empty());
    assert_eq!(h.state().ws.state().case, before.case);
    assert!(h.query_by_label(OPEN_HINT).is_some());
    assert!(h.query_by_label("Close with a base \u{b7} Lower").is_some());
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
    assert!(text.contains("tooth marks"), "{text}");
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

// ---------------------------------------------------------------- simple menu

/// A case with the two demo arches (marked, coloured), as a project.
fn demo_project(arches: &[&str]) -> Project {
    let mut state = CaseState::default();
    for arch in arches {
        state = dca_module::commands::find("aligners.demo")
            .unwrap()
            .execute(
                &state,
                &json!({ "arch": arch }),
                &dc_command::JobCtx::detached(),
            )
            .unwrap()
            .0;
    }
    Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    )
}

fn footer(h: &Harness<'static, App>, done: usize) -> bool {
    h.query_by_label(&format!("Treatment workflow {done}/9"))
        .is_some()
}

#[test]
fn the_footer_counts_the_nine_steps_that_have_their_result() {
    // a new case: nothing done; the demo arch has its scans, tooth marks and colours
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    assert!(footer(&h, 0));
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    let mut h = harness(ws);
    h.run();
    assert!(
        footer(&h, 3),
        "import, find the teeth, separate teeth and gum"
    );
}

#[test]
fn the_shortcuts_and_the_exports_do_not_count() {
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    let menu = ws.module().menu();
    let count = |id: &str| menu.find(id).unwrap().step_ids().len();
    assert_eq!(
        count("aligners.case"),
        1,
        "import; the automatic run is not a step"
    );
    assert_eq!(count("aligners.teeth"), 3, "the corrections are not steps");
    assert_eq!(count("aligners.diagnosis"), 1);
    assert_eq!(
        count("aligners.planning"),
        4,
        "bite, final position, aligners, clinical review; moving a tooth is not a step"
    );
    assert_eq!(
        count("aligners.production"),
        0,
        "nothing to count: no counter"
    );
    assert_eq!(count("aligners.followup"), 0);
}

#[test]
fn a_project_reopened_shows_the_ticks_of_what_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.dcprj");
    let mut ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    ws.save_to(path.clone());
    assert!(ws.wait_for_jobs(WAIT));
    // load it back like Home does
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(&path, &codecs).unwrap();
    assert!(!loaded.read_only, "{:?}", loaded.issues);
    let mut h = harness(Workspace::new_headless(
        module(),
        Some(loaded.project),
        Some(path),
    ));
    h.run();
    assert!(footer(&h, 3));
}

#[test]
fn run_everything_fills_the_steps_stops_on_a_guess_and_leads_to_the_analysis() {
    // both arches from the demo, then the whole pipeline from the menu
    let mut ws = Workspace::new_headless(module(), Some(demo_project(&["lower", "upper"])), None);
    ws.run_leaf("aligners.auto");
    assert!(
        ws.toast_keys().contains(&"ws-toast-auto-long".to_owned()),
        "it says it takes a while"
    );
    assert!(ws.wait_for_jobs(Duration::from_secs(900)));
    let keys = ws.toast_keys();
    assert!(keys.contains(&"ws-toast-auto-done".to_owned()), "{keys:?}");
    // the case analysis is where to look next, in the inspector
    assert_eq!(ws.selected(), Some("aligners.diagnosis.space-analysis"));
    let mut h = harness(ws);
    h.run();
    assert!(footer(&h, 9), "all the steps have their result");
    assert_eq!(
        h.get_all_by_label("Space available").count(),
        2,
        "one card per arch"
    );
    assert_eq!(h.get_all_by_label("Little index").count(), 2);
    assert!(h.query_by_label("Bolton ratio").is_some());
    assert!(h.query_by_label("Complexity of the case").is_some());
    assert!(h.query_by_label("Run again").is_some());
    // one undo step takes the whole run back
    assert!(h.state_mut().ws.undo());
    h.run();
    assert!(footer(&h, 3));
}

#[test]
fn run_everything_stops_with_a_clear_message_on_a_guessed_arch() {
    let mut project = demo_project(&["lower"]);
    // a scan the geometry cannot place stays a guess
    project.case_state.case.scans[0].role_source = RoleSource::Provisional;
    project.case_state.case.scans[0].source_name = "scan.stl".into();
    let blob = project.case_state.blobs.insert(tetra(40.0));
    project.case_state.case.scans[0].mesh = blob;
    let mut ws = Workspace::new_headless(module(), Some(project), None);
    ws.run_leaf("aligners.auto");
    assert!(ws.wait_for_jobs(WAIT));
    assert!(!ws.toast_keys().contains(&"ws-toast-auto-done".to_owned()));
    let details = ws.toast_details();
    let (key, detail) = details.last().unwrap();
    assert_eq!(key, "ws-toast-command-failed");
    let detail = detail.as_deref().unwrap();
    assert!(detail.contains("Stopped at Arch recognition"), "{detail}");
    assert!(!detail.contains("aligners."), "{detail}");
    assert!(!ws.is_dirty(), "the case is left as it was");
}

#[test]
fn the_final_position_asks_for_the_bite_first_with_two_arches_not_related() {
    const SETUP: &str = "aligners.planning.setup.automatic";
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower", "upper"])), None);
    let mut h = harness(ws);
    h.state_mut().ws.select_item(SETUP);
    h.run();
    // the notice points to Bite; the run button stays
    assert!(h.query_by_label("Bite first").is_some());
    click_in_view(&mut h, "Run");
    settle(&mut h);
    let details = h.state().ws.toast_details();
    let (key, detail) = details.last().unwrap();
    assert_eq!(key, "ws-toast-command-failed");
    let detail = detail.as_deref().unwrap();
    assert!(detail.contains("the bite comes first"), "{detail}");
    assert!(!h.state().ws.is_dirty(), "the refusal changes nothing");
    // the bite is never run from here: the selection stays on the function
    assert_eq!(h.state().ws.selected(), Some(SETUP));

    // after the bite the notice goes and the final position runs
    h.state_mut().ws.run_leaf("aligners.occlusion.align");
    settle(&mut h);
    h.state_mut().ws.select_item(SETUP);
    h.run();
    assert!(h.query_by_label("Bite first").is_none());
    click_in_view(&mut h, "Run");
    settle(&mut h);
    assert!(!aligners_of(h.state().ws.state()).setup.poses.is_empty());

    // a single arch has nothing to relate
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    let mut h = harness(ws);
    h.state_mut().ws.select_item(SETUP);
    h.run();
    assert!(h.query_by_label("Bite first").is_none());
}

/// The inspector of Aligners: its result, its parameters (one change, one
/// undo step, nothing computed) and "Slow a tooth"; selecting it runs
/// nothing.
#[test]
fn the_aligners_inspector_holds_its_result_parameters_and_slow_a_tooth() {
    const ALIGNERS: &str = "aligners.planning.aligners";
    let mut project = demo_project(&["lower"]);
    let mut al = aligners_of(&project.case_state);
    let f = dc_case::Fdi::new(31).unwrap();
    al.staging_results.insert(
        ScanRole::Lower,
        dca_model::StagingSummary {
            aligners: 12,
            pattern: dca_model::StagingPattern::Structured,
            level: "easy".to_owned(),
            slowed: [(f, 0.5)].into(),
            collisions_left: 1,
            overcorrection: false,
        },
    );
    al.tooth_speed.insert(f, 0.5);
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(al));
    let mut ws = Workspace::new_headless(module(), Some(project), None);
    let before = Arc::clone(ws.state());
    ws.select_item(ALIGNERS);
    let mut h = harness(ws);
    h.run();
    for label in [
        "12 aligners",
        "Pattern: Structured; complexity: easy",
        "Slowed teeth: 31 (0.50)",
        "Collisions left: 1 (see the report)",
        "Parameters",
        "Periodontium",
        "Healthy",
        "Calendar",
        "Fixed interval",
        "Start of treatment",
        "Largest movement per aligner",
        "Manual corrections",
        "Slow a tooth",
        "Select a tooth in the view to slow it down.",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(!h.state().ws.is_busy(), "selecting Aligners ran something");
    assert!(
        Arc::ptr_eq(h.state().ws.state(), &before),
        "the case changed"
    );
    // a parameter: one change of the settings, nothing computed
    click_in_view(&mut h, "Simultaneous, proportional");
    settle(&mut h);
    let after = aligners_of(h.state().ws.state());
    assert_eq!(
        after.settings.staging.pattern,
        dca_model::StagingPattern::Proportional
    );
    assert_eq!(after.stages, aligners_of(&before).stages);
    assert_eq!(after.staging_results, aligners_of(&before).staging_results);
    // the adaptive calendar hides the days
    click_in_view(&mut h, "Adaptive (7/14 days)");
    settle(&mut h);
    assert_eq!(
        aligners_of(h.state().ws.state()).settings.interval_policy,
        dca_model::IntervalPolicy::Adaptive
    );
    h.run();
    assert!(h.query_by_label("Days per aligner").is_none());
    // each change is one undo step
    assert!(h.state_mut().ws.undo());
    assert!(h.state_mut().ws.undo());
    assert!(Arc::ptr_eq(h.state().ws.state(), &before));
}

#[test]
fn the_case_analysis_is_in_the_inspector_with_its_figures() {
    let mut project = demo_project(&["lower"]);
    let mut al = aligners_of(&project.case_state);
    al.diagnosis.arches.insert(
        ScanRole::Lower,
        dca_model::ArchSpace {
            available_mm: 62.4,
            required_mm: 66.1,
            little_index_mm: 4.2,
        },
    );
    al.diagnosis.bolton_anterior = Some(77.9);
    al.diagnosis.bolton_overall = Some(91.3);
    al.diagnosis.complexity_score = Some(35.0);
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(al));
    let mut ws = Workspace::new_headless(module(), Some(project), None);
    ws.select_item("aligners.diagnosis.space-analysis");
    let mut h = harness(ws);
    h.run();
    for label in [
        "Results of the case analysis",
        "Space available",
        "62.4 mm",
        "Space needed",
        "66.1 mm",
        "Difference",
        "+3.7 mm",
        "Crowding",
        "Little index",
        "4.2 mm",
        "Moderate irregularity",
        "Bolton ratio",
        "77.9 %",
        "91.3 %",
        "Complexity of the case",
        "Moderate",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
}

#[test]
fn a_bolton_that_could_not_be_calculated_says_so() {
    let mut project = demo_project(&["lower", "upper"]);
    let mut al = aligners_of(&project.case_state);
    for role in [ScanRole::Lower, ScanRole::Upper] {
        al.diagnosis
            .arches
            .insert(role, dca_model::ArchSpace::default());
    }
    al.diagnosis.bolton_anterior = None;
    al.diagnosis.bolton_overall = None;
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(al));
    let mut ws = Workspace::new_headless(module(), Some(project), None);
    ws.select_item("aligners.diagnosis.space-analysis");
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("Bolton ratio").is_some());
    assert_eq!(h.get_all_by_label("Not calculated").count(), 2);
    assert!(
        h.query_by_label_contains("Bolton needs every tooth")
            .is_some()
    );
}

/// A case with the lower arch analysed: 62.4 mm available, 66.1 needed (3.7 mm
/// of crowding), Little index 4.2 mm, midline 0.5 mm off, largest space 1.5 mm,
/// complexity 3.
fn analysed_project() -> Project {
    let mut project = demo_project(&["lower"]);
    let mut al = aligners_of(&project.case_state);
    al.diagnosis.arches.insert(
        ScanRole::Lower,
        dca_model::ArchSpace {
            available_mm: 62.4,
            required_mm: 66.1,
            little_index_mm: 4.2,
        },
    );
    al.diagnosis.details.insert(
        ScanRole::Lower,
        dca_model::ArchDetails {
            largest_space_mm: 1.5,
            midline_offset_mm: 0.5,
            catcat_score: 3,
        },
    );
    al.diagnosis.complexity_score = Some(3.0);
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(al));
    project
}

const CASE_ANALYSIS_ID: &str = "aligners.diagnosis.space-analysis";

/// Something on the screen has this label (a figure may be shown twice, as a
/// card and as a field).
fn has(h: &Harness<'static, App>, label: &str) -> bool {
    h.query_all_by_label(label).count() > 0
}

#[test]
fn the_case_analysis_inspector_lists_the_values_to_correct_and_runs_nothing() {
    let mut ws = Workspace::new_headless(module(), Some(analysed_project()), None);
    ws.select_item(CASE_ANALYSIS_ID);
    let before = ws.state().clone();
    let mut h = harness(ws);
    h.run();
    for label in [
        "Manual corrections",
        "Your crowding (+) or spacing (−)",
        "Your Little index",
        "Your midline offset",
        "Your largest space",
        // the computed values stay next to the fields
        "Computed: 3.7 mm",
        "Computed: 4.2 mm",
        "Computed: 0.5 mm",
        "Computed: 1.5 mm",
    ] {
        assert!(has(&h, label), "{label}");
    }
    // none set by hand yet; showing the section started nothing
    assert!(!has(&h, "Set by hand") && !has(&h, "Reset"));
    assert!(!h.state().ws.is_busy());
    assert!(Arc::ptr_eq(h.state().ws.state(), &before));
    assert!(h.state().ws.toast_keys().is_empty());

    // a case that has no analysis says what to do first
    let mut ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    ws.select_item(CASE_ANALYSIS_ID);
    let mut h = harness(ws);
    h.run();
    assert!(has(&h, "Manual corrections"));
    assert!(
        h.query_by_label_contains("Run the case analysis first")
            .is_some()
    );
    assert!(!has(&h, "Your Little index"));
}

#[test]
fn a_value_written_by_hand_shows_in_the_cards_undoes_and_resets() {
    use dca_algo::diagnosis::edit::EditField;
    let mut ws = Workspace::new_headless(module(), Some(analysed_project()), None);
    ws.select_item(CASE_ANALYSIS_ID);
    let mut h = harness(ws);
    h.run();
    let start = aligners_of(h.state().ws.state());
    let lower = ScanRole::Lower;

    // 9 mm of crowding by hand (the command's discrepancy is -9)
    h.state_mut()
        .ws
        .edit_diagnosis(lower, EditField::Discrepancy, Some(-9.0));
    settle(&mut h);
    let al = aligners_of(h.state().ws.state());
    assert_eq!(al.diagnosis.edits_of(lower).discrepancy_mm, Some(-9.0));
    // the computed values are as they were; the complexity follows (crowding 9: 4 points)
    assert_eq!(al.diagnosis.arches, start.diagnosis.arches);
    assert_eq!(al.diagnosis.complexity_score, Some(4.0));
    assert!(
        h.state()
            .ws
            .toast_keys()
            .contains(&"ws-toast-diag-edited".to_owned())
    );
    // the cards show the value in force, the section the badge and the reset
    for label in [
        "+9.0 mm",
        "Includes values set by hand",
        "Set by hand",
        "Reset",
        "Computed: 3.7 mm",
    ] {
        assert!(has(&h, label), "{label}");
    }
    assert!(!has(&h, "+3.7 mm"));
    // the complexity card: 4 is still easy
    assert!(has(&h, "Easy"));

    // a Little index by hand, then the card words it
    h.state_mut()
        .ws
        .edit_diagnosis(lower, EditField::Little, Some(12.0));
    settle(&mut h);
    assert!(has(&h, "12.0 mm"));
    assert!(has(&h, "Very severe irregularity"));
    assert_eq!(h.get_all_by_label("Set by hand").count(), 2);

    // one undo step each
    assert!(h.state_mut().ws.undo());
    h.run();
    let al = aligners_of(h.state().ws.state());
    assert_eq!(al.diagnosis.edits_of(lower).little_index_mm, None);
    assert_eq!(al.diagnosis.edits_of(lower).discrepancy_mm, Some(-9.0));
    assert!(h.state_mut().ws.redo());
    h.run();
    assert_eq!(
        aligners_of(h.state().ws.state())
            .diagnosis
            .edits_of(lower)
            .little_index_mm,
        Some(12.0)
    );

    // a reset by the command, then the Reset button of the other value
    h.state_mut()
        .ws
        .edit_diagnosis(lower, EditField::Little, None);
    settle(&mut h);
    assert_eq!(h.get_all_by_label("Set by hand").count(), 1);
    click_in_view(&mut h, "Reset");
    settle(&mut h);
    let al = aligners_of(h.state().ws.state());
    assert_eq!(al.diagnosis, start.diagnosis);
    assert!(!has(&h, "Reset"));
    assert!(has(&h, "+3.7 mm"));
}

#[test]
fn a_value_out_of_range_is_refused_and_leaves_the_case_as_it_was() {
    use dca_algo::diagnosis::edit::EditField;
    let mut ws = Workspace::new_headless(module(), Some(analysed_project()), None);
    ws.select_item(CASE_ANALYSIS_ID);
    let mut h = harness(ws);
    h.run();
    let before = h.state().ws.state().clone();
    h.state_mut()
        .ws
        .edit_diagnosis(ScanRole::Lower, EditField::Little, Some(99.0));
    settle(&mut h);
    assert!(Arc::ptr_eq(h.state().ws.state(), &before));
    assert!(
        h.state()
            .ws
            .toast_keys()
            .contains(&"ws-toast-command-failed".to_owned())
    );
    // nothing to undo from it
    assert!(!h.state_mut().ws.undo());
}

#[test]
fn the_shortcut_list_names_the_leaves_in_key_order() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    h.get_by_label("Help").click();
    h.run();
    for label in ["Separate teeth and gum", "Tooth brush", "Aligners"] {
        assert!(h.get_all_by_label(label).count() >= 1, "{label}");
    }
    // the keys, in their order
    for key in ["A", "B", "S"] {
        assert_eq!(h.get_all_by_label(key).count(), 1, "{key}");
    }
    assert!(h.query_by_label("Keyboard shortcuts").is_some());
}

// ---- the numbering in order (seed tool) ---------------------------------------------------

/// Selects Find the teeth and opens its tool "Mark a tooth", as a user does.
fn open_the_seed_tool(h: &mut Harness<'static, App>) {
    h.state_mut().ws.select_item("aligners.teeth.find");
    h.run();
    click_in_view(h, "Mark a tooth");
    h.run();
    assert_eq!(
        h.state().ws.active_tool(),
        Some("aligners.preparation.numbering.place-seeds")
    );
}

#[test]
fn the_numbering_in_order_is_driven_from_the_panel_and_the_keys() {
    let ws = Workspace::new_headless(module(), Some(demo_project(&["lower"])), None);
    let mut h = harness(ws);
    h.run();
    open_the_seed_tool(&mut h);
    assert!(h.query_by_label("Number the teeth in order").is_some());
    let marks = aligners(&h.state().ws).seeds.len();
    assert!(marks > 0);

    // clear all the tooth marks from the panel: one undoable command
    let colours = aligners(&h.state().ws).labels;
    click_in_view(&mut h, "Clear all the tooth marks");
    settle(&mut h);
    assert!(aligners(&h.state().ws).seeds.is_empty());
    assert!(
        h.state()
            .ws
            .toast_keys()
            .iter()
            .any(|k| k == "ws-manual-cleared")
    );
    // the colours of the teeth stay (as in the plugin), and the notice says
    // they are out of date
    assert_eq!(aligners(&h.state().ws).labels, colours);
    assert!(
        h.state()
            .ws
            .toast_keys()
            .iter()
            .any(|k| k == "ws-toast-marks-outdated")
    );

    // the numbering asks for the right distal tooth of the lower arch first
    click_in_view(&mut h, "Number the teeth in order");
    h.run();
    assert!(h.query_by_label("Click tooth 47").is_some());
    // S skips the tooth: the next one is 46
    h.key_press(Key::S);
    h.run();
    assert!(h.query_by_label("Click tooth 46").is_some());
    h.key_press(Key::S);
    h.run();
    assert!(h.query_by_label("Click tooth 45").is_some());
    // Backspace takes the last one back
    h.key_press(Key::Backspace);
    h.run();
    assert!(h.query_by_label("Click tooth 46").is_some());
    // nothing was sent yet
    assert!(aligners(&h.state().ws).missing.is_empty());
    // Enter: done. Skips alone change nothing (no tooth was clicked), and S
    // never marks a tooth as missing
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.query_by_label("Click tooth 46").is_none());
    assert!(aligners(&h.state().ws).missing.is_empty());
    assert!(aligners(&h.state().ws).seeds.is_empty());
    assert!(
        !h.state()
            .ws
            .toast_keys()
            .iter()
            .any(|k| k == "ws-manual-seq-numbered")
    );

    // the clearing is the only step on the undo stack
    h.state_mut().ws.undo();
    settle(&mut h);
    assert_eq!(aligners(&h.state().ws).seeds.len(), marks);

    // Esc is done too
    click_in_view(&mut h, "Number the teeth in order");
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(h.query_by_label("Click tooth 47").is_none());
    assert!(h.query_by_label("Number the teeth in order").is_some());
}

#[test]
fn the_numbering_in_order_stops_when_the_case_changes_and_follows_the_third_molars_setting() {
    // the case searches the third molars (setting of Find the teeth)
    let mut project = demo_project(&["lower"]);
    let mut data = aligners_of(&project.case_state);
    data.settings.find_teeth.third_molars = true;
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(data));
    let ws = Workspace::new_headless(module(), Some(project), None);
    let mut h = harness(ws);
    h.run();
    open_the_seed_tool(&mut h);
    click_in_view(&mut h, "Clear all the tooth marks");
    settle(&mut h);
    // the box of the panel starts from the setting: the numbering starts at 48
    click_in_view(&mut h, "Number the teeth in order");
    h.run();
    assert!(h.query_by_label("Click tooth 48").is_some());
    h.key_press(Key::S);
    h.run();
    assert!(h.query_by_label("Click tooth 47").is_some());
    // undo changes the case under the numbering: it stops
    assert!(h.state_mut().ws.undo());
    settle(&mut h);
    h.run();
    assert!(h.query_by_label("Click tooth 47").is_none());
    assert!(h.query_by_label("Number the teeth in order").is_some());
    assert!(
        h.state()
            .ws
            .toast_keys()
            .iter()
            .any(|k| k == "ws-manual-seq-stopped")
    );
}

/// Bite, the plugin's MORSO page: the report of the last bite under the
/// status, the bite scans with the button that imports them, Three points
/// and the correction of the interpenetration. Selecting it runs nothing.
#[test]
fn the_bite_inspector_shows_the_report_the_scans_and_the_corrections() {
    let mut project = demo_project(&["lower", "upper"]);
    let state = &mut project.case_state;
    // a bite scan and the report of a registration on it
    let blob = state.blobs.insert(tetra(12.0));
    let scan: Scan = serde_json::from_value(json!({
        "id": "b1700000-0000-4000-8000-000000000009",
        "role": "bite",
        "source_name": "right-bite.stl",
        "source_units": "mm",
        "import_xform": { "t": [0.0, 0.0, 0.0], "q": [0.0, 0.0, 0.0, 1.0] },
        "mesh": blob,
    }))
    .unwrap();
    state.case.scans.push(scan);
    let mut data = state
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .unwrap()
        .clone();
    let fit = |rms: f64, n: usize, area: f64| dca_model::BiteFit {
        rms_mm: rms,
        n_teeth: n,
        area_mm2: area,
        ..dca_model::BiteFit::default()
    };
    data.bite_registered = true;
    data.bite_report = Some(dca_model::BiteReport {
        method: dca_model::BiteMethod::Registration,
        registration: Some(dca_model::BiteRegistration {
            scans: vec![dca_model::BiteScanFit {
                scan: "right-bite.stl".into(),
                side: Some("right".into()),
                lower: fit(0.021, 4, 212.0),
                upper: fit(0.034, 3, 187.0),
            }],
            penetration_mm: 0.18,
            reasons: vec!["one_side_only".into(), "interpenetration".into()],
            verdict: dca_model::BiteVerdict::ToReview,
            ..dca_model::BiteRegistration::default()
        }),
        correction_mm: 0.0,
    });
    state.set_module(dca_model::MODULE_ID, ModuleState::new(data));
    let mut h = harness(Workspace::new_headless(module(), Some(project), None));
    h.run();
    let before = Arc::clone(h.state().ws.state());
    h.state_mut().ws.select_item("aligners.occlusion.align");
    h.run();
    for label in [
        // the report
        "Bite registered, to check",
        "Bite registered from 1 buccal scan(s) (right): largest RMS deviation 0.034 mm, \
         interpenetration 0.18 mm.",
        "To check: Bite of one side only",
        "To check: Interpenetration of the arches 0.18 mm",
        "right-bite.stl (right): RMS 0.021 / 0.034 mm, teeth 4 + 3, 212 + 187 mm²",
        // the bite scans and the button that imports them
        "Side 1: right-bite.stl",
        "Import the bite scans…",
        "Run registers the arches on the bite scans",
        // the manual corrections
        "Manual corrections",
        "Three points",
        "Correct the interpenetration (0.18 mm)",
    ] {
        assert!(h.query_by_label_contains(label).is_some(), "{label}");
    }
    let ws = &h.state().ws;
    assert!(!ws.is_busy());
    assert_eq!(ws.dialog_requests(), 0);
    assert_eq!(ws.active_tool(), None);
    assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
    assert!(
        Arc::ptr_eq(&before, ws.state()),
        "the selection changed the case"
    );
    // the button of the bite scans asks for the files, and only that
    click_in_view(&mut h, "Import the bite scans…");
    h.run();
    let ws = &h.state().ws;
    assert_eq!(ws.dialog_requests(), 1);
    assert!(!ws.is_busy());
    assert!(Arc::ptr_eq(&before, ws.state()));
}

/// "Export for printing": where the aligner ends (the teeth left out, the
/// advice, the final position computed with another end), the quality and
/// the folded groups of the production settings; selecting it runs
/// nothing, the advice button ends the aligner in one undo step.
#[test]
fn the_export_inspector_shows_the_aligner_end_and_the_production_settings() {
    let mut project = demo_project(&["lower"]);
    let mut d = aligners_of(&project.case_state);
    let lower = ScanRole::Lower;
    d.seeds.retain(|s| s.fdi.get() != 35);
    d.crowns.retain(|f, _| f.get() != 35);
    d.aligner_end.insert(
        lower,
        dca_model::AlignerEnd {
            right: Some(Fdi::new(46).unwrap()),
            left: None,
        },
    );
    d.setup
        .results
        .insert(lower, dca_model::SetupSummary::default());
    project
        .case_state
        .set_module(dca_model::MODULE_ID, ModuleState::new(d));
    let mut h = harness(Workspace::new_headless(module(), Some(project), None));
    h.run();
    let before = Arc::clone(h.state().ws.state());
    h.state_mut().ws.select_item("aligners.output.export-all");
    h.run();
    for label in [
        "Where the aligner ends",
        "Lower arch",
        "Right side",
        "Left side",
        "Out of the aligner: 47.",
        "Computed with another end",
        "computed with another end of the aligner",
        "Advice, left side",
        "Tooth 35 is missing",
        "End at 34",
        "Quality of the print files",
        "Standard (production files, 0.1 mm detail)",
        "Aligner edge",
        "Shell",
        "Identification text",
        "Bite with the aligners worn",
        "Bite ramps",
    ] {
        assert!(h.query_by_label_contains(label).is_some(), "{label}");
    }
    let ws = &h.state().ws;
    assert!(!ws.is_busy());
    assert_eq!(ws.dialog_requests(), 0);
    assert!(ws.toast_keys().is_empty(), "{:?}", ws.toast_keys());
    assert!(
        Arc::ptr_eq(&before, ws.state()),
        "the selection changed the case"
    );
    // nothing of the inspector is wider than the window (a long line wraps)
    let button = h.get_by_label("End at 34").rect();
    assert!(button.max.x <= 1920.0, "{button:?}");
    // the advice ends the aligner at 34 on the left
    click_in_view(&mut h, "End at 34");
    settle(&mut h);
    let end = aligners_of(h.state().ws.state()).aligner_end_of(lower);
    assert_eq!(end.left.map(|f| f.get()), Some(34));
    assert_eq!(end.right.map(|f| f.get()), Some(46));
}
