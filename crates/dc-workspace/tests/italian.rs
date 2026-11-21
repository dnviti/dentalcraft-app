//! Italian labels (own test binary: the language is process-wide).

mod common;

use std::path::PathBuf;
use std::time::Duration;

use dc_case::CaseState;
use dc_mesh::{MeshFormat, TriMesh};
use dc_project::{AppInfo, Project};
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
    ws.select_item("aligners.case.import-scans");
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

#[test]
fn italian_role_change_notice() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    // an import that only moved a scan to the other arch, with a bite registration to redo
    ws.show_import_report(&serde_json::json!({
        "moved": [{ "from": "upper", "to": "lower" }],
        "module_cleared": ["seeds", "bite_registered"],
    }));
    assert_eq!(ws.toast_keys(), ["ws-toast-role-changed"]);
    let text = ws.toast_details()[0].1.clone().unwrap();
    assert!(text.contains("punti dente"), "{text}");
    assert!(
        text.contains("registrazione del morso (da rifare"),
        "{text}"
    );
    assert!(text.contains("Ctrl+Z riporta i ruoli precedenti"), "{text}");
    assert!(!text.contains("sostituit"), "{text}");
    assert_eq!(
        dc_i18n::tr("ws-toast-role-changed"),
        "Ruolo della scansione cambiato"
    );
}

#[test]
fn italian_toolbar_and_timeline_hold_at_every_width() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    common::assert_layout(&common::Labels {
        views: ["Occlusale", "Frontale", "Sinistra", "Destra", "Posteriore"],
        fit: "Inquadra tutto",
        hint: "Nessuno stadio",
        arches: ["Superiore", "Inferiore"],
        buttons: [
            "Solido",
            "Trasparente",
            "Reticolo",
            "Misura",
            "Angolo",
            "Piano di sezione",
            "Colori dei denti",
            "Punti dente",
            "Numeri dei denti",
            "Mappa dei contatti",
            "Attacchi",
            "IPR",
            "Modelli da studio",
            "Posizione finale",
            "Allineatore",
        ],
        solids: "Modelli nella vista",
        overlay: "Sovrapponi stadio 0",
    });
}

// ---------------------------------------------------------------- simple menu

const WAIT: Duration = Duration::from_secs(60);

fn italian() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
}

fn demo_project(arches: &[&str]) -> Project {
    let mut state = CaseState::default();
    for arch in arches {
        state = dca_module::commands::find("aligners.demo")
            .unwrap()
            .execute(
                &state,
                &serde_json::json!({ "arch": arch }),
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

fn harness(ws: Workspace) -> Harness<'static, Workspace> {
    Harness::builder()
        .with_size(egui::vec2(1600.0, 900.0))
        // every control of the inspectors: the advanced options on
        .with_context_setup(|ctx| {
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, true);
        })
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        )
}

#[test]
fn no_key_of_the_workspace_is_shown_raw_in_italian() {
    italian();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("i18n/en/dc-workspace.ftl");
    let ids = dc_i18n::parity::ids(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(ids.len() > 150);
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    ws.select_item("aligners.case.import-scans"); // loads the bundles
    for id in &ids {
        let text = dc_i18n::tr(id);
        assert_ne!(&text, id, "{id} has no Italian text");
    }
}

#[test]
fn the_italian_menu_counts_steps_and_the_inspector_speaks_italian() {
    italian();
    let ws = Workspace::new_headless(
        Box::new(AlignersModule::new()),
        Some(demo_project(&["lower"])),
        None,
    );
    let mut h = harness(ws);
    h.run();
    assert!(h.query_by_label("Flusso di trattamento 3/9").is_some());
    h.state_mut()
        .select_item("aligners.diagnosis.space-analysis");
    h.run();
    // not run yet on this case: the status, the button and the line say so
    assert!(h.query_by_label("Non ancora fatto").is_some());
    assert!(h.query_by_label("Automatico").is_some());
    assert!(h.query_by_label("Esegui").is_some());
    assert!(
        h.query_by_label_contains("non è ancora stata eseguita")
            .is_some()
    );
    // a function with tools: its parameters and its corrections, in Italian
    h.state_mut()
        .select_item("aligners.preparation.segmentation.automatic");
    h.run();
    for label in [
        "Fatto",
        "Parametri",
        "Esegui di nuovo",
        "Correzioni a mano",
        "Pulisci i bordi",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    // the 3D teeth: the view of the region of a tooth, in Italian
    h.state_mut().select_item("aligners.teeth.build");
    h.run();
    for label in ["Correzioni a mano", "Mostra la zona", "Fine"] {
        assert!(h.get_all_by_label(label).count() >= 1, "{label}");
    }
    assert!(
        h.query_by_label_contains("Clicca un dente nella vista")
            .is_some()
    );
    // the keys of the tools, with the new labels
    h.get_by_label("Guida").click();
    h.run();
    for label in [
        "Separa denti e gengiva",
        "Pennello dei denti",
        "Allineatori",
    ] {
        assert!(h.get_all_by_label(label).count() >= 1, "{label}");
    }
}

#[test]
fn the_italian_bite_and_export_sentences_have_their_accents() {
    italian();
    let mut ws = Workspace::new_headless(
        Box::new(AlignersModule::new()),
        Some(demo_project(&["lower", "upper"])),
        None,
    );
    ws.run_leaf("aligners.occlusion.align");
    assert!(ws.wait_for_jobs(WAIT));
    let details = ws.toast_details();
    let (key, text) = details.last().unwrap();
    assert_eq!(key, "ws-toast-bite-estimated");
    let text = text.as_deref().unwrap();
    assert!(text.contains("con una stima"), "{text}");
    assert_eq!(dc_i18n::tr("ws-toast-bite-estimated"), "Morso stimato");
    assert!(
        text.contains("Il caso non ha una scansione del morso"),
        "{text}"
    );
    assert_eq!(
        dc_i18n::tr("ws-export-long"),
        "L'esportazione per la stampa può richiedere molto tempo. \
         Puoi fermarla con la croce nella barra di stato."
    );
    assert!(dc_i18n::tr("ws-ca-little-minimal").contains("Irregolarità"));
}

#[test]
fn the_periodontal_choices_wrap_in_the_narrowest_inspector() {
    italian();
    let mut ws = Workspace::new_headless(
        Box::new(AlignersModule::new()),
        Some(demo_project(&["lower"])),
        None,
    );
    let size = dc_ui::tokens::tokens().size;
    ws.set_panel_widths(dc_workspace::PanelWidths {
        menu: size.left_menu_width,
        inspector: size.inspector_min_width,
    });
    ws.select_item("aligners.planning.aligners");
    // tall enough to show the whole inspector without scrolling
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 3000.0))
        // the periodontal choice is an advanced option
        .with_context_setup(|ctx| {
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
            dc_module_api::set_advanced_ui(ctx, true);
        })
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        );
    h.run();
    let l = h.state().layout();
    assert_eq!(l.inspector.width(), size.inspector_min_width);
    // the long choices are there, wrapped: nothing of the inspector clipped
    for label in [
        "Parodontite stadio I (0,18 mm per allineatore)",
        "Parodontite stadio II (0,15 mm per allineatore)",
        "Parodontite stadio III (0,10 mm per allineatore)",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert_eq!(l.inspector_overflow, 0.0, "{l:?}");
}
