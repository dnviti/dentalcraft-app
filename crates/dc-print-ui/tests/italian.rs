//! Italian texts of the print popup (own test binary: the language is
//! process-wide).

use std::path::PathBuf;

use dc_print_ui::{PrintPopup, PrintRequest};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

#[test]
fn no_key_of_the_print_popup_is_shown_raw_in_italian() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    let mut popup = PrintPopup::default();
    popup.open(PrintRequest::default());
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(|ui, p: &mut PrintPopup| p.show(ui), popup);
    h.run_steps(2);
    assert!(h.query_by_label("Stampa allineatori").is_some());
    assert!(
        h.get_by_label("Prepara la stampa")
            .accesskit_node()
            .is_disabled()
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("i18n/en/dc-print-ui.ftl");
    let ids = dc_i18n::parity::ids(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(ids.len() > 80);
    for id in &ids {
        let text = dc_i18n::tr(id);
        assert_ne!(&text, id, "{id} has no Italian text");
    }
}
