//! Italian labels (own test binary: the language is process-wide).

use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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
