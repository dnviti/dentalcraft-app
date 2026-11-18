//! Italian texts of the scan popup (own test binary: the language is
//! process-wide).

use std::path::PathBuf;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

#[test]
fn no_key_of_the_scan_popup_is_shown_raw_in_italian() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    // drawing the button loads the bundle
    let mut h = Harness::builder()
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui(|ui| {
            let _ = dc_scan_ui::scan_button(ui, true);
        });
    h.run();
    assert!(h.query_by_label("Scansiona").is_some());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("i18n/en/dc-scan-ui.ftl");
    let ids = dc_i18n::parity::ids(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(ids.len() > 150);
    for id in &ids {
        let text = dc_i18n::tr(id);
        assert_ne!(&text, id, "{id} has no Italian text");
    }
}
