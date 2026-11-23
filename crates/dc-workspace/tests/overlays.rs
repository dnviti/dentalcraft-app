//! The four overlay toggles of the 3D view (tooth colours, tooth marks,
//! tooth numbers, contact map): always in the toolbar, whatever the project,
//! the view, the step, the model shown or the advanced options. Their state
//! (off when a project opens; on while the tooth brush is open) is tested
//! with the layers in `view.rs`.

use dc_case::{Case, CaseState, ModuleState, Pose, RoleSource, Scan, ScanRole};
use dc_project::{AppInfo, Project};
use dc_workspace::{WindowOverlays, Workspace};
use dca_module::AlignersModule;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

const LABELS: [&str; 4] = [
    "Tooth colours",
    "Tooth marks",
    "Tooth numbers",
    "Contact map",
];

/// A case with one closed scan (a tetrahedron) per role of `roles`.
fn case_with(roles: &[ScanRole]) -> Project {
    let mut state = CaseState::new(Case::new("AL-OVERLAYS-01"));
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

fn harness(ws: Workspace, advanced: bool) -> Harness<'static, Workspace> {
    Harness::builder()
        .with_size(vec2(1600.0, 900.0))
        .with_context_setup(move |ctx| {
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, advanced);
        })
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        )
}

fn assert_four(h: &mut Harness<'static, Workspace>, what: &str) {
    h.run();
    for label in LABELS {
        assert!(h.query_by_label(label).is_some(), "{what}: no {label}");
    }
}

#[test]
fn the_four_toggles_are_in_the_toolbar_in_every_state() {
    let projects: [(&str, Option<Project>); 4] = [
        ("no project", None),
        ("one arch", Some(case_with(&[ScanRole::Lower]))),
        (
            "two arches",
            Some(case_with(&[ScanRole::Upper, ScanRole::Lower])),
        ),
        (
            "two arches and a bite",
            Some(case_with(&[
                ScanRole::Upper,
                ScanRole::Lower,
                ScanRole::Bite,
            ])),
        ),
    ];
    let steps = [
        "aligners.case.import-scans",
        "aligners.teeth.find",
        "aligners.preparation.segmentation.automatic",
        "aligners.planning.aligners",
        "aligners.output.export-all",
    ];
    for advanced in [false, true] {
        for (name, project) in &projects {
            let ws =
                Workspace::new_headless(Box::new(AlignersModule::new()), project.clone(), None);
            let mut h = harness(ws, advanced);
            for step in steps {
                h.state_mut().select_item(step);
                assert_four(&mut h, &format!("{name}, {step}, advanced {advanced}"));
            }
            for view in ["occlusal", "front", "left", "right", "back"] {
                for model in ["scan", "teeth"] {
                    h.state_mut()
                        .set_window_view(
                            Some(view),
                            None,
                            None,
                            None,
                            Some(model),
                            WindowOverlays::default(),
                        )
                        .unwrap();
                    assert_four(
                        &mut h,
                        &format!("{name}, {view}, {model}, advanced {advanced}"),
                    );
                }
            }
        }
    }
}

#[test]
fn the_four_toggles_start_off_and_a_click_turns_one_on_and_off() {
    let ws = Workspace::new_headless(
        Box::new(AlignersModule::new()),
        Some(case_with(&[ScanRole::Lower])),
        None,
    );
    let mut h = harness(ws, false);
    h.run();
    let shown = |h: &mut Harness<'static, Workspace>| {
        h.state_mut()
            .set_window_view(None, None, None, None, None, WindowOverlays::default())
            .unwrap()
    };
    let all_off = |v: &serde_json::Value| {
        ["colours", "marks", "numbers", "contacts"]
            .iter()
            .all(|k| v[*k] == false)
    };
    assert!(all_off(&shown(&mut h)));
    // the marks: a click on the toggle turns them on, another off
    h.get_by_label("Tooth marks").click();
    h.run();
    assert_eq!(shown(&mut h)["marks"], true);
    h.get_by_label("Tooth marks").click();
    h.run();
    assert!(all_off(&shown(&mut h)));
}
