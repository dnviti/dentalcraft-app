//! The print popup drawn in a headless egui loop: it lists the parts it was
//! opened with, refuses to prepare without the user's resin, shows the
//! advanced settings only with the advanced options of the Settings on, and
//! keeps the resin for the next time.

use std::path::PathBuf;

use dc_print_ui::{PartStatus, PrintItem, PrintPopup, PrintRequest, Resin, ResinStore};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

fn request() -> PrintRequest {
    let item = |id: &str, label: &str, status, path: bool| PrintItem {
        id: id.into(),
        label: label.into(),
        path: path.then(|| PathBuf::from(format!("/nowhere/{id}.stl"))),
        status,
    };
    PrintRequest {
        parts: vec![
            item("SUP-03", "SUP 03", PartStatus::Missing, true),
            item("INF-03", "INF 03", PartStatus::Printed, true),
            item("SUP-04-P", "SUP 04 P", PartStatus::Missing, false),
        ],
        out_dir: None,
    }
}

fn resin() -> Resin {
    Resin {
        name: "Clear aligner".into(),
        layer_um: 100,
        exposure_s: 5.7,
        bottom_exposure_s: 35.0,
        bottom_layers: 4,
        rest_s: 1.0,
        density_g_ml: 1.1,
    }
}

fn harness(popup: PrintPopup) -> Harness<'static, PrintPopup> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .with_context_setup(|ctx| {
            dc_ui::fonts::install(ctx);
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
        })
        .build_ui_state(|ui, p: &mut PrintPopup| p.show(ui), popup);
    h.run_steps(3);
    h
}

#[test]
fn the_popup_opens_with_its_parts_listed() {
    let mut popup = PrintPopup::default();
    popup.open(request());
    let h = harness(popup);
    assert!(h.query_by_label("Print aligners").is_some());
    assert!(h.query_by_label("Saturn 4 Ultra 16K").is_some());
    assert!(
        h.query_by_label_contains("DENTAL 45: the resin tank warms to 45 °C")
            .is_some()
    );
    for label in ["SUP 03", "INF 03", "SUP 04 P"] {
        assert!(h.query_by_label(label).is_some(), "{label} is not listed");
    }
    // the missing one with a file is chosen, the printed one is not
    assert!(h.query_by_label("1 of 3 chosen").is_some());
    assert_eq!(h.query_all_by_label("To print").count(), 2);
    assert!(h.query_by_label("Printed").is_some());
}

#[test]
fn prepare_is_disabled_without_a_resin() {
    let mut popup = PrintPopup::default();
    popup.open(request());
    let h = harness(popup);
    assert!(
        h.get_by_label("Prepare the print")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        h.query_by_label("Enter your resin's values first: the print needs them.")
            .is_some()
    );
    assert!(h.query_by_label("Enter your resin's values").is_some());

    let mut popup = PrintPopup::default();
    popup.set_resin(resin()).unwrap();
    popup.open(request());
    let h = harness(popup);
    assert!(
        !h.get_by_label("Prepare the print")
            .accesskit_node()
            .is_disabled()
    );
    assert!(h.query_by_label("Clear aligner").is_some());
}

#[test]
fn the_resin_form_opens_from_its_button() {
    let mut popup = PrintPopup::default();
    popup.open(request());
    let mut h = harness(popup);
    h.get_by_label("Enter your resin's values").click();
    h.run_steps(2);
    assert!(h.query_by_label("Save resin values").is_some());
    assert!(h.query_by_label("Resin name").is_some());
    // nothing typed: nothing is saved, the fields say why
    h.get_by_label("Save resin values").click();
    h.run_steps(2);
    assert!(h.state().resin().is_none());
    assert!(
        h.query_all_by_label_contains("Enter a number above 0")
            .count()
            > 1
    );
}

#[test]
fn the_advanced_settings_are_hidden_by_default() {
    let mut popup = PrintPopup::default();
    popup.set_resin(resin()).unwrap();
    popup.open(request());
    let h = harness(popup);
    for label in [
        "Print settings",
        "Supports",
        "Spacing on the plate",
        "Save the plate with its supports as STL…",
    ] {
        assert!(h.query_by_label(label).is_none(), "{label} is shown");
    }
}

#[test]
fn the_advanced_settings_show_with_the_advanced_options_on() {
    let mut popup = PrintPopup::default();
    popup.set_resin(resin()).unwrap();
    popup.open(request());
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .with_context_setup(|ctx| {
            dc_ui::fonts::install(ctx);
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, true);
        })
        .build_ui_state(|ui, p: &mut PrintPopup| p.show(ui), popup);
    h.run_steps(3);
    for label in ["Print settings", "Supports", "Spacing on the plate"] {
        assert!(h.query_by_label(label).is_some(), "{label} is not shown");
    }
}

#[test]
fn the_resin_is_kept_for_the_next_time() {
    let dir = tempfile::tempdir().unwrap();
    let store = ResinStore::file(dir.path().join(dc_print_ui::RESIN_FILE));
    let mut popup = PrintPopup::new(store.clone());
    assert!(popup.resin().is_none());
    popup.set_resin(resin()).unwrap();
    let mut again = PrintPopup::new(store);
    assert_eq!(again.resin(), Some(&resin()));
    again.open(request());
    assert!(again.can_prepare());
}

#[test]
fn escape_and_the_close_button_close_it() {
    let mut popup = PrintPopup::default();
    popup.open(request());
    let mut h = harness(popup);
    h.get_by_label("Close the print window").click();
    h.run_steps(2);
    assert!(!h.state().is_open());
    h.state_mut().open(request());
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().is_open());
}
