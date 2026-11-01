//! Italian labels (own test binary: the language is process-wide).

use dc_mesh::{MeshFormat, TriMesh};
use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;

#[test]
fn italian_labels() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1920.0, 1080.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui(move |ui| {
            let _ = ws.show(ui);
        });
    h.run();
    assert!(h.query_by_label("Nessuna scansione").is_some());
    assert!(h.query_by_label("Caso").is_some());
    assert_eq!(h.get_all_by_label("Importa scansioni").count(), 2);
}

/// Closed tetrahedron: too few faces to recognise an arch.
fn tetra() -> TriMesh {
    TriMesh::new(
        vec![
            Vec3::ZERO,
            Vec3::new(40.0, 0.0, 0.0),
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(0.0, 0.0, 40.0),
        ],
        vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
    )
}

#[test]
fn italian_import_report_and_scan_rows() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scansione.stl");
    dc_mesh::io::write(&path, &tetra(), MeshFormat::Stl).unwrap();
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    ws.activate("aligners.case.import-scans");
    ws.import_files(vec![path]);
    assert!(ws.wait_for_jobs(std::time::Duration::from_secs(20)));
    // the sentences of the import and of the workspace are in Italian
    let details = ws.toast_details();
    let all: String = details
        .iter()
        .filter_map(|(_, d)| d.as_deref())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(all.contains("1 scansione aggiunta al caso."), "{all}");
    assert!(
        all.contains("non è stato possibile riconoscere l'arcata"),
        "{all}"
    );
    assert!(!all.contains("scan added"), "{all}");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1920.0, 1080.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui(move |ui| {
            let _ = ws.show(ui);
        });
    h.run();
    assert!(h.query_by_label("scansione.stl").is_some());
    assert!(h.query_by_label("Provvisoria").is_some());
    assert!(h.query_by_label("Ruolo delle prossime scansioni").is_some());
    assert!(h.query_by_label("Conferma").is_some());
}
