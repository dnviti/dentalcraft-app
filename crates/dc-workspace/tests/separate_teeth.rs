//! The leaf "Separate teeth and gum" in the workspace, driven the way a user
//! does (egui_kittest, no GPU) on the demo arch: the result line and the
//! toast of a run, what a new run leaves out of date, the separation
//! parameters of the inspector, settings of the case, and its tools.

use std::time::Duration;

use dc_case::{CaseState, Fdi, ModuleState, ScanRole};
use dc_mesh::TriMesh;
use dc_project::{AppInfo, Project};
use dc_workspace::Workspace;
use dca_model::{AlignersState, MODULE_ID, SegmentationSettings};
use dca_module::AlignersModule;
use egui::{Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use serde_json::json;

const WAIT: Duration = Duration::from_secs(300);
const SEGMENT: &str = "aligners.preparation.segmentation.automatic";
const BUILD: &str = "aligners.teeth.build";

fn demo_state(arches: &[&str]) -> CaseState {
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
    state
}

fn project(state: CaseState) -> Project {
    Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    )
}

fn harness(project: Project) -> Harness<'static, Workspace> {
    let ws = Workspace::new_headless(Box::new(AlignersModule::new()), Some(project), None);
    harness_of(ws)
}

fn harness_of(ws: Workspace) -> Harness<'static, Workspace> {
    let mut h = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        // the numbers of the separation are advanced options
        .with_context_setup(|ctx| {
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, true);
        })
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        );
    h.state_mut().select_item(SEGMENT);
    h.run();
    h
}

fn settle(h: &mut Harness<'static, Workspace>) {
    h.step();
    assert!(h.state_mut().wait_for_jobs(WAIT));
    h.run();
}

fn data(h: &Harness<'static, Workspace>) -> AlignersState {
    h.state()
        .state()
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone()
}

fn seg_settings(h: &Harness<'static, Workspace>) -> SegmentationSettings {
    data(h).settings.segmentation
}

fn detail_of(h: &Harness<'static, Workspace>, key: &str) -> Option<String> {
    h.state()
        .toast_details()
        .into_iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, d)| d)
}

#[test]
fn a_run_on_the_demo_shows_its_result_line_and_toast_and_the_scan() {
    // a case opened with its colours but without a stored run (a project saved
    // before the case kept it): no line
    let h = harness(project(demo_state(&["lower"])));
    assert!(h.query_by_label("Run again").is_some());
    assert!(h.query_by_label_contains("teeth separated in").is_none());

    // the demo arch with its tooth marks and no colours yet
    let mut state = demo_state(&["lower"]);
    let mut al = state
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone();
    al.labels.clear();
    state.set_module(MODULE_ID, ModuleState::new(al));
    let mut h = harness(project(state));
    assert!(
        h.query_by_label_contains("It starts from the tooth marks")
            .is_some()
    );
    // the parameters are in sight, then the run button and the tools
    assert!(h.query_by_label("Parameters").is_some());
    assert!(
        h.query_by_role_and_label(Role::Slider, "Scan detail (faces)")
            .is_some()
    );
    assert!(h.query_by_label("Not done yet").is_some());
    assert!(h.query_by_label("Run").is_some());
    for label in ["Manual corrections", "Tooth brush", "Clean borders"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.state().teeth_shown(), "3D teeth by default");
    assert!(!h.state().is_busy(), "the selection runs nothing");

    // the click starts the job; the next frame shows it in the status bar
    h.get_by_label("Run").scroll_to_me();
    h.run();
    h.get_by_label("Run").click();
    h.step();
    assert!(h.state().is_busy());
    h.step();
    // the status bar names the job
    assert!(
        h.query_by_label_contains("Separating teeth and gum")
            .is_some()
    );
    settle(&mut h);
    let detail = detail_of(&h, "ws-toast-seg-done").expect("the success toast");
    assert!(detail.contains("teeth separated in"), "{detail}");
    let teeth: u64 = detail.split(' ').next().unwrap().parse().unwrap();
    assert!(teeth >= 12, "{detail}");
    // the same line in the inspector, and the scan shows the new colours
    assert!(h.query_by_label(&detail).is_some(), "{detail}");
    assert!(!h.state().teeth_shown(), "Scan view after the run");
    assert!(h.query_by_label("Run again").is_some());
    // one undo step: no colours, and the line goes with the run
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert!(data(&h).labels.is_empty());
    assert!(h.query_by_label(&detail).is_none());
    // redo brings the colours of that run and its line back
    assert!(h.state_mut().redo());
    settle(&mut h);
    assert!(h.query_by_label(&detail).is_some());
}

#[test]
fn the_result_line_is_kept_in_the_project_and_goes_with_the_colours_on_undo() {
    let mut state = demo_state(&["lower"]);
    let mut al = state
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone();
    al.labels.clear();
    state.set_module(MODULE_ID, ModuleState::new(al));
    let mut h = harness(project(state));
    h.state_mut().run_leaf(SEGMENT);
    settle(&mut h);
    let detail = detail_of(&h, "ws-toast-seg-done").expect("the success toast");
    assert!(h.query_by_label(&detail).is_some(), "{detail}");
    let kept = data(&h).segmentation_runs[&ScanRole::Lower];
    assert!(kept.teeth >= 12, "{kept:?}");

    // saved and opened again, in a new session: the line is still there
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.dcprj");
    h.state_mut().save_to(path.clone());
    assert!(h.state_mut().wait_for_jobs(WAIT));
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(&path, &codecs).unwrap();
    let reopened = harness(loaded.project);
    assert_eq!(data(&reopened).segmentation_runs[&ScanRole::Lower], kept);
    assert!(reopened.query_by_label(&detail).is_some(), "{detail}");

    // undo takes the colours and the line away
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert!(data(&h).segmentation_runs.is_empty());
    assert!(h.query_by_label(&detail).is_none());
}

#[test]
fn a_new_run_after_the_3d_teeth_drops_their_tick_and_says_they_are_out_of_date() {
    // the demo arch with a tooth solid and a gingiva: 3D teeth done
    let mut state = demo_state(&["lower"]);
    let solid = state.blobs.insert(TriMesh::new(
        vec![
            Vec3::ZERO,
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, 0.0, 5.0),
        ],
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
    ));
    let mut al = state
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .clone();
    al.crowns.insert(Fdi::new(46).unwrap(), solid);
    al.gingiva.insert(ScanRole::Lower, solid);
    state.set_module(MODULE_ID, ModuleState::new(al));
    let mut h = harness(project(state));
    let done =
        |h: &Harness<'static, Workspace>| h.state().module().leaf_done(h.state().state(), BUILD);
    assert!(done(&h));

    h.get_by_label("Run again").scroll_to_me();
    h.run();
    h.get_by_label("Run again").click();
    settle(&mut h);
    assert!(!done(&h), "the tick of 3D teeth is removed");
    let detail = detail_of(&h, "ws-toast-marks-outdated").expect("the out of date notice");
    assert!(
        detail.starts_with("Out of date because the teeth were separated again"),
        "{detail}"
    );
    assert!(!detail.contains("aligners-"), "{detail}");
    // the run itself said what it did
    assert!(detail_of(&h, "ws-toast-seg-done").is_some());
    // undo brings the 3D teeth back
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert!(done(&h));
}

#[test]
fn a_parameter_is_a_setting_of_the_case_one_undo_step_kept_in_the_project() {
    let mut h = harness(project(demo_state(&["lower"])));
    assert_eq!(seg_settings(&h), SegmentationSettings::default());
    for label in [
        "Scan detail (faces)",
        "Tooth edge sensitivity",
        "Border smoothing",
    ] {
        assert!(
            h.query_by_role_and_label(Role::Slider, label).is_some(),
            "{label}"
        );
    }
    // a click on the track sets the value there: one change of the case
    h.get_by_role_and_label(Role::Slider, "Scan detail (faces)")
        .click();
    settle(&mut h);
    let set = seg_settings(&h);
    assert_ne!(set.target_faces, 30_000);
    assert!((5000..=200_000).contains(&set.target_faces));
    assert_eq!(set.target_faces % 1000, 0);
    assert_eq!((set.eta_convex, set.sigma), (0.2, 1.0), "the others stay");

    // kept in the project file
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.dcprj");
    h.state_mut().save_to(path.clone());
    assert!(h.state_mut().wait_for_jobs(WAIT));
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(&path, &codecs).unwrap();
    let reopened = loaded
        .project
        .case_state
        .module_data::<AlignersState>(MODULE_ID)
        .unwrap()
        .settings
        .segmentation;
    assert_eq!(reopened, set);

    // one undo step takes it back
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert_eq!(seg_settings(&h), SegmentationSettings::default());
    assert!(!h.state_mut().undo(), "nothing else on the undo stack");
}
