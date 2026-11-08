//! The inspector of "Find the teeth": the third-molars box, the field of the
//! missing teeth and the grid of the teeth found, driven the way a user does
//! (egui_kittest, no GPU), and the parameters the leaf passes to the command.

use std::time::Duration;

use dc_case::{CaseState, Fdi};
use dc_project::{AppInfo, Project};
use dc_workspace::Workspace;
use dca_model::{AlignersState, FindTeethSettings, MODULE_ID};
use dca_module::AlignersModule;
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(60);
const FIND: &str = "aligners.teeth.find";

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

fn harness(arches: &[&str]) -> Harness<'static, Workspace> {
    let ws = Workspace::new_headless(
        Box::new(AlignersModule::new()),
        Some(demo_project(arches)),
        None,
    );
    let mut h = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        );
    h.state_mut().select_item(FIND);
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

fn find_settings(h: &Harness<'static, Workspace>) -> FindTeethSettings {
    data(h).settings.find_teeth
}

/// The field of the missing teeth, found by its label.
fn type_in_field(h: &mut Harness<'static, Workspace>, text: &str) {
    h.get_by_label("Missing teeth").click();
    h.run();
    h.get_by_label("Missing teeth").type_text(text);
    h.run();
}

#[test]
fn the_inspector_has_the_two_options_and_the_grid_of_the_arch() {
    let h = harness(&["lower"]);
    assert!(h.query_by_label("Search also the third molars").is_some());
    // the field is named by its label (screen readers)
    assert!(
        h.get_all_by_role(egui::Role::TextInput)
            .any(|n| n.accesskit_node().label().as_deref() == Some("Missing teeth"))
    );
    assert!(h.query_by_label("Missing teeth").is_some());
    assert!(h.query_by_label("Lower arch").is_some());
    assert!(h.query_by_label("Upper arch").is_none(), "no upper scan");
    // the demo arch has its marks: a cell per tooth, without the third molars
    let marked = data(&h).seeds.len();
    assert!(marked >= 12);
    assert!(h.query_by_label("36: has a mark").is_some());
    assert!(h.query_by_label_contains("48:").is_none());
    assert!(h.query_by_label_contains("38:").is_none());
    assert!(
        h.query_by_label(&format!("{marked} teeth numbered"))
            .is_some()
    );
    // the plugin's defaults
    assert_eq!(find_settings(&h), FindTeethSettings::default());
}

#[test]
fn the_third_molars_box_is_a_setting_of_the_case_and_adds_the_cells() {
    let mut h = harness(&["lower"]);
    h.get_by_label("Search also the third molars").click();
    settle(&mut h);
    assert!(find_settings(&h).third_molars);
    assert!(h.query_by_label_contains("48:").is_some());
    assert!(h.query_by_label_contains("38:").is_some());
    // one undo step, and the box follows it
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert!(!find_settings(&h).third_molars);
    assert!(h.query_by_label_contains("48:").is_none());
}

#[test]
fn a_valid_list_is_applied_when_the_field_is_left_and_shown_in_the_grid() {
    let mut h = harness(&["lower"]);
    type_in_field(&mut h, "45; 35 45");
    // nothing is applied while typing
    assert!(find_settings(&h).missing.is_empty());
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(find_settings(&h).missing, [35, 45]);
    assert!(h.query_by_label("45: missing").is_some());
    assert!(h.query_by_label("35: missing").is_some());
    assert!(h.query_by_label("36: has a mark").is_some());
    // the text is written back in the normal form
    assert!(h.query_by_label_contains("Not a tooth number").is_none());
    // undo takes the list back, field included
    assert!(h.state_mut().undo());
    settle(&mut h);
    assert!(find_settings(&h).missing.is_empty());
    assert!(h.query_by_label("45: missing").is_none());
}

#[test]
fn an_invalid_entry_is_a_field_error_and_changes_nothing() {
    let mut h = harness(&["lower"]);
    type_in_field(&mut h, "35, 99, abc");
    assert!(
        h.query_by_label_contains("Not a tooth number: 99, abc")
            .is_some()
    );
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(find_settings(&h).missing.is_empty(), "not applied");
    assert!(h.query_by_label_contains("Not a tooth number").is_some());
    assert!(!h.state_mut().undo(), "no step on the undo stack");
}

#[test]
fn the_leaf_runs_with_the_options_and_leaves_the_missing_teeth_without_a_mark() {
    let mut h = harness(&["lower"]);
    assert!(
        data(&h)
            .seeds
            .iter()
            .any(|s| s.fdi == Fdi::new(45).unwrap())
    );
    type_in_field(&mut h, "45");
    h.key_press(Key::Enter);
    settle(&mut h);
    h.get_by_label("Search also the third molars").click();
    settle(&mut h);
    assert_eq!(
        find_settings(&h),
        FindTeethSettings {
            third_molars: true,
            missing: vec![45]
        }
    );
    // the click on the leaf: the numbering with the options, then the orientation
    h.state_mut().activate(FIND);
    settle(&mut h);
    // the new marks dropped the colours computed from the old ones: a notice
    // in the words of the scan import says so
    assert!(data(&h).labels.is_empty());
    assert!(
        h.state()
            .toast_keys()
            .iter()
            .any(|k| k == "ws-toast-marks-dropped")
    );
    let seeds = data(&h).seeds;
    assert!(seeds.len() >= 10);
    assert!(seeds.iter().all(|s| s.fdi.get() != 45), "45 is missing");
    assert!(h.query_by_label("45: missing").is_some());
    // the options are kept by the run
    assert_eq!(find_settings(&h).missing, [45]);
    assert!(find_settings(&h).third_molars);
}

#[test]
fn with_both_arches_each_has_its_grid_and_an_empty_case_says_what_to_do() {
    let h = harness(&["lower", "upper"]);
    assert!(h.query_by_label("Upper arch").is_some());
    assert!(h.query_by_label("Lower arch").is_some());
    assert!(h.query_by_label("16: has a mark").is_some());
    assert!(h.query_by_label("46: has a mark").is_some());

    let ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    let mut h = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        );
    h.state_mut().select_item(FIND);
    h.run();
    assert!(
        h.query_by_label("Import the scan of an arch to see its teeth.")
            .is_some()
    );
}
