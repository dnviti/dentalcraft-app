//! The 3D teeth of an arch are out of date after any change of its tooth
//! colours or tooth marks made once they were built (a mark placed, moved or
//! deleted, a number changed, a tooth marked missing, the marks cleared and
//! numbered in order, Find the teeth or the separation run again, the brush):
//! the arch shows its scan and the inspector of 3D teeth says so until the
//! teeth are built again, also after the project is saved and opened again.

use std::path::Path;

use dc_case::{CaseState, ScanRole};
use dc_command::JobCtx;
use dc_project::{AppInfo, Project};
use dc_workspace::Workspace;
use dca_model::{AlignersState, MODULE_ID};
use dca_module::AlignersModule;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

fn exec(state: &CaseState, id: &str, params: Value) -> CaseState {
    dca_module::commands::find(id)
        .unwrap()
        .execute(state, &params, &JobCtx::detached())
        .unwrap_or_else(|e| panic!("{id} {params}: {e}"))
        .0
}

fn data(state: &CaseState) -> &AlignersState {
    state.module_data::<AlignersState>(MODULE_ID).unwrap()
}

/// The 3D teeth of the lower arch show stale teeth: built, and out of date.
fn stale(state: &CaseState) -> bool {
    data(state).teeth_out_of_date(ScanRole::Lower)
}

fn built(state: &CaseState) -> bool {
    data(state).crowns.keys().any(|f| !f.is_upper())
}

/// The lower demo arch with its 3D teeth built.
fn built_lower() -> CaseState {
    let s = exec(
        &CaseState::default(),
        "aligners.demo",
        json!({ "arch": "lower" }),
    );
    let s = exec(&s, "aligners.teeth.build", json!({ "arch": "lower" }));
    assert!(built(&s) && !stale(&s));
    s
}

/// A built tooth and its tooth mark.
fn mark(state: &CaseState, skip: usize) -> (u8, [f64; 3]) {
    let d = data(state);
    let f = *d.crowns.keys().nth(skip).unwrap();
    let s = d.seeds.iter().find(|s| s.fdi == f).unwrap();
    (f.get(), s.position.to_array())
}

/// Saved and opened again as Home opens it.
fn reopened(state: &CaseState, dir: &Path) -> CaseState {
    let path = dir.join("outdated.dcprj");
    let project = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state.clone(),
    );
    dc_project::save(&path, &project).unwrap();
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    dc_project::load(&path, &codecs).unwrap().project.case_state
}

type Change = fn(&CaseState) -> CaseState;

#[test]
fn every_change_of_the_marks_or_colours_after_the_build_leaves_the_3d_teeth_out_of_date() {
    let dir = tempfile::tempdir().unwrap();
    let base = built_lower();
    let changes: [(&str, Change); 10] = [
        ("mark moved", |s| {
            let (f, [x, y, z]) = mark(s, 1);
            let p = json!({ "op": "move", "fdi": f, "position": [x + 0.25, y, z] });
            exec(s, "aligners.preparation.numbering.place-seeds", p)
        }),
        ("mark deleted", |s| {
            let p = json!({ "fdi": mark(s, 1).0 });
            exec(s, "aligners.preparation.numbering.delete-seed", p)
        }),
        ("mark added", |s| {
            let (f, p) = mark(s, 1);
            let s = exec(
                s,
                "aligners.preparation.numbering.delete-seed",
                json!({ "fdi": f }),
            );
            let base = exec(&s, "aligners.teeth.build", json!({ "arch": "lower" }));
            let p = json!({ "op": "add", "fdi": f, "position": p });
            exec(&base, "aligners.preparation.numbering.place-seeds", p)
        }),
        ("number changed", |s| {
            let p = json!({ "src": mark(s, 0).0, "dst": 38 });
            exec(s, "aligners.preparation.numbering.renumber", p)
        }),
        ("tooth missing", |s| {
            let p = json!({ "fdi": mark(s, 2).0 });
            exec(s, "aligners.preparation.numbering.mark-missing", p)
        }),
        ("marks cleared", |s| {
            exec(
                s,
                "aligners.preparation.numbering.clear-seeds",
                json!({ "arch": "lower" }),
            )
        }),
        ("numbered in order", |s| {
            let (_, p) = mark(s, 0);
            let s = exec(
                s,
                "aligners.preparation.numbering.clear-seeds",
                json!({ "arch": "lower" }),
            );
            let steps = json!({ "arch": "lower", "steps": [{ "kind": "point", "position": p }] });
            exec(&s, "aligners.preparation.numbering.sequence", steps)
        }),
        ("brush", |s| {
            let (f, _) = mark(s, 0);
            let labels = s.mesh(&data(s).labels[&ScanRole::Lower]).unwrap();
            let vertices: Vec<u32> = labels
                .vertex_labels
                .as_ref()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, l)| **l == u16::from(f))
                .take(30)
                .map(|(i, _)| i as u32)
                .collect();
            let p = json!({ "arch": "lower", "label": 0, "vertices": vertices });
            exec(s, "aligners.preparation.segmentation.manual-brush", p)
        }),
        ("Find the teeth again", |s| {
            exec(s, "aligners.teeth.find", json!({}))
        }),
        ("separated again", |s| {
            let p = json!({ "arch": "lower", "sigma": 2.0 });
            exec(s, "aligners.preparation.segmentation.automatic", p)
        }),
    ];
    for (what, change) in changes {
        let s = change(&base);
        // stale 3D teeth are never shown: the arch is out of date, or the
        // change took its 3D teeth away
        assert!(!built(&s) || stale(&s), "{what}: stale teeth shown");
        if !built(&s) {
            eprintln!("{what}: the 3D teeth of the arch were cleared");
            continue;
        }
        // the project file keeps it
        assert!(stale(&reopened(&s, dir.path())), "{what}: reopened");
        // built again: up to date (what the change left of the arch)
        let rebuilt = exec(&s, "aligners.teeth.build", json!({ "arch": "lower" }));
        assert!(!stale(&rebuilt), "{what}: built again");
        assert!(
            !stale(&reopened(&rebuilt, dir.path())),
            "{what}: built again, reopened"
        );
    }
}

#[test]
fn the_inspector_and_the_view_say_the_3d_teeth_are_out_of_date_after_a_mark_moved() {
    let base = built_lower();
    let (f, [x, y, z]) = mark(&base, 1);
    let moved = exec(
        &base,
        "aligners.preparation.numbering.place-seeds",
        json!({ "op": "move", "fdi": f, "position": [x + 0.25, y, z] }),
    );
    let project = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        moved,
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
    assert!(h.query_by_label("Out of date").is_some());
    assert!(
        h.query_by_label_contains("changed after the 3D teeth were built (Lower)")
            .is_some()
    );
}
