//! The advanced options and parameters: off by default, the inspectors show
//! what is needed to reach the aligners and the next step is marked; on,
//! everything shows (`dc_module_api::set_advanced_ui`, set by the shell).

use dc_case::{Case, CaseState, ModuleState, Pose, RoleSource, Scan, ScanRole};
use dc_project::{AppInfo, Project};
use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui::{Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

const SETUP: &str = "aligners.planning.setup.automatic";

/// The workspace of a new case with `item` selected, with the advanced
/// options on or off.
fn harness(item: &'static str, advanced: bool) -> Harness<'static> {
    harness_with(None, item, advanced)
}

/// The workspace of `project` (a new case without one) with `item`
/// selected, with the advanced options on or off.
fn harness_with(project: Option<Project>, item: &'static str, advanced: bool) -> Harness<'static> {
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), project, None);
    ws.select_item(item);
    let mut h = Harness::builder()
        .with_size(vec2(1920.0, 1080.0))
        .with_context_setup(move |ctx| {
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, advanced);
        })
        .build_ui(move |ui| {
            let _ = ws.show(ui);
        });
    h.run();
    h
}

/// A case with one closed scan (a tetrahedron) per role of `roles`.
fn case_with(roles: &[ScanRole]) -> Project {
    let mut state = CaseState::new(Case::new("AL-SIMPLE-01"));
    let blob = state.blobs.insert(dc_mesh::TriMesh::new(
        vec![
            glam::Vec3::ZERO,
            glam::Vec3::new(50.0, 0.0, 0.0),
            glam::Vec3::new(0.0, 50.0, 0.0),
            glam::Vec3::new(0.0, 0.0, 50.0),
        ],
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
    ));
    for (k, role) in roles.iter().enumerate() {
        state.case.scans.push(Scan {
            id: uuid::Uuid::new_v4(),
            role: *role,
            role_source: RoleSource::Explicit,
            source_name: format!("{role:?}-{k}.stl").to_lowercase(),
            source_units: dc_mesh::Units::Millimeters,
            import_xform: Pose::default(),
            mesh: blob,
        });
    }
    state.set_module(
        dca_model::MODULE_ID,
        ModuleState::new(dca_model::AlignersState::default()),
    );
    Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    )
}

#[test]
fn the_final_position_hides_its_parameter_groups_until_the_advanced_options_are_on() {
    let h = harness(SETUP, false);
    for group in [
        "Arch line and tolerances",
        "Teeth held and midline",
        "Space (therapy, in the chosen order)",
        "Interproximal reduction (IPR)",
        "Vertical movements",
    ] {
        assert!(h.query_by_label(group).is_none(), "{group} shown");
    }
    let h = harness(SETUP, true);
    for group in ["Arch line and tolerances", "Vertical movements"] {
        assert!(h.query_by_label(group).is_some(), "{group} not shown");
    }
}

#[test]
fn the_study_models_have_no_empty_parameters_section_by_default() {
    let h = harness("aligners.output.study-models", false);
    assert!(h.query_by_label("Parameters").is_none());
    let h = harness("aligners.output.study-models", true);
    assert!(h.query_by_label("Parameters").is_some());
}

#[test]
fn print_shows_the_tracking_of_the_aligners_with_the_advanced_options_off() {
    // a new case has no aligners yet: the tracking of the module says so,
    // next to the button that opens the print window
    for advanced in [false, true] {
        let h = harness("aligners.production.print", advanced);
        assert!(
            h.query_by_label_contains("There are no aligners yet")
                .is_some(),
            "advanced {advanced}: no tracking"
        );
        assert!(h.query_by_label("Prepare print…").is_some());
    }
}

#[test]
fn the_next_step_carries_the_next_marker() {
    // a new case: the next step is to import the scans
    let h = harness("aligners.case.import-scans", false);
    let row = h
        .get_by_role_and_label(Role::TreeItem, "Import scans")
        .rect();
    let pill = h.get_by_label("Next").rect();
    assert!(
        row.contains_rect(pill),
        "the pill {pill:?} is not on the row {row:?}"
    );
    // and its inspector says it
    assert!(h.query_by_label("Next step").is_some());
    // the same with the advanced options on
    let h = harness("aligners.case.import-scans", true);
    assert!(h.query_by_label("Next").is_some());
}

#[test]
fn removing_a_scan_and_registering_the_bite_need_no_advanced_options() {
    let roles = [
        ScanRole::Upper,
        ScanRole::Lower,
        ScanRole::Bite,
        ScanRole::Bite,
    ];
    let count = |h: &Harness<'static>, label: &str| h.get_all_by_label(label).count();
    for advanced in [false, true] {
        // "Scans in the case": a bin on every row
        let h = harness_with(
            Some(case_with(&roles)),
            "aligners.case.import-scans",
            advanced,
        );
        assert_eq!(count(&h, "Remove the scan"), 4, "advanced {advanced}");
        // Bite: the bite scans with their bins, their import, the run and
        // the tool Three points
        let h = harness_with(
            Some(case_with(&roles)),
            "aligners.occlusion.align",
            advanced,
        );
        assert_eq!(count(&h, "Remove the scan"), 2, "advanced {advanced}");
        for label in ["Import the bite scans…", "Run", "Three points"] {
            assert!(count(&h, label) > 0, "advanced {advanced}: no {label}");
        }
    }
}
