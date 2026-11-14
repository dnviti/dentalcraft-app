//! The inspector of the 3D teeth holds the view of the region of a tooth on
//! the scan ("Show the region" and "End"), among its manual corrections:
//! without a tooth selected it says to click one, and the buttons do not
//! start anything (the menu law: nothing is run, the case stays as it is).

use dc_case::CaseState;
use dc_command::JobCtx;
use dc_project::{AppInfo, Project};
use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::Value;

fn lower_demo() -> CaseState {
    dca_module::commands::find("aligners.demo")
        .unwrap()
        .execute(&CaseState::default(), &Value::Null, &JobCtx::detached())
        .unwrap()
        .0
}

fn inspector() -> Harness<'static, Workspace> {
    let project = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        lower_demo(),
    );
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), Some(project), None);
    ws.select_item("aligners.teeth.build");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                ws.show(ui);
            },
            ws,
        );
    h.run();
    h
}

#[test]
fn the_3d_teeth_inspector_offers_the_region_view_and_says_to_click_a_tooth() {
    let mut h = inspector();
    for label in [
        "Manual corrections",
        "Show the region",
        "End",
        "Rebuild a tooth",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(
        h.query_by_label_contains("Click a tooth in the view")
            .is_some()
    );
    // with no tooth selected both buttons are off: a click starts nothing
    for label in ["Show the region", "End"] {
        assert!(
            h.get_by_label(label).accesskit_node().is_disabled(),
            "{label}"
        );
    }
    h.get_by_label("Show the region").click();
    h.run();
    assert!(!h.state().is_busy());
    assert_eq!(h.state().selected_component(), None);
    assert!(
        h.query_by_label_contains("Click a tooth in the view")
            .is_some()
    );
    assert!(!h.state().state().case.scans.is_empty());
}
