//! The whole popup flow on two small cup-shaped shells: Prepare places them
//! on a plate, the plate shows its estimates, "Save print files" writes the
//! plate file and tells the host which parts it carries. A shell of an
//! export takes its fitting side from its stage model, as `print.prepare`
//! does; a loose shell has it guessed and the window warns.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dc_mesh::TriMesh;
use dc_mesh::io::MeshFormat;
use dc_print_ui::{
    FittingSource, PartStatus, PrintEvent, PrintItem, PrintPopup, PrintRequest, Resin,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;

/// A closed cup `w` × `d` × `h` mm with walls `t` thick, open at the top.
fn cup(w: f32, d: f32, h: f32, t: f32) -> TriMesh {
    let o = [[0.0, 0.0], [w, 0.0], [w, d], [0.0, d]];
    let i = [[t, t], [w - t, t], [w - t, d - t], [t, d - t]];
    let mut positions = Vec::new();
    for [x, y] in o {
        positions.push(Vec3::new(x, y, 0.0));
    }
    for [x, y] in o {
        positions.push(Vec3::new(x, y, h));
    }
    for [x, y] in i {
        positions.push(Vec3::new(x, y, t));
    }
    for [x, y] in i {
        positions.push(Vec3::new(x, y, h));
    }
    // outer bottom 0-3, outer top 4-7, inner bottom 8-11, inner top 12-15
    let mut tris: Vec<[u32; 3]> = vec![[0, 2, 1], [0, 3, 2], [8, 9, 10], [8, 10, 11]];
    for k in 0..4u32 {
        let n = (k + 1) % 4;
        // outer wall, facing out
        tris.push([k, n, 4 + n]);
        tris.push([k, 4 + n, 4 + k]);
        // inner wall, facing the cavity
        tris.push([8 + k, 12 + n, 8 + n]);
        tris.push([8 + k, 12 + k, 12 + n]);
        // rim between the outer and the inner top edge
        tris.push([4 + k, 4 + n, 12 + n]);
        tris.push([4 + k, 12 + n, 12 + k]);
    }
    TriMesh {
        positions,
        tris,
        ..TriMesh::default()
    }
}

fn write_cup(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}.stl"));
    let mut f = std::fs::File::create(&path).unwrap();
    dc_mesh::io::write_to(&mut f, &cup(30.0, 20.0, 10.0, 1.0), MeshFormat::Stl).unwrap();
    path
}

/// A closed box from `lo` to `hi`, faces outward.
fn block(lo: Vec3, hi: Vec3) -> TriMesh {
    let positions = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        })
        .collect();
    let tris = vec![
        [0, 4, 6],
        [0, 6, 2],
        [1, 3, 7],
        [1, 7, 5],
        [0, 1, 5],
        [0, 5, 4],
        [2, 6, 7],
        [2, 7, 3],
        [0, 2, 3],
        [0, 3, 1],
        [4, 5, 7],
        [4, 7, 6],
    ];
    TriMesh {
        positions,
        tris,
        ..TriMesh::default()
    }
}

/// An export as the aligners write it: the shell of `SUP` aligner 01 in
/// `stampa_resina/` and the stage model it was built on (the cup's
/// cavity, moved by `model_offset`) in `modelli_termoformatura/`. Answers
/// the shell file.
fn write_export(root: &Path, model_offset: Vec3) -> PathBuf {
    let (shells, models) = (
        root.join("stampa_resina"),
        root.join("modelli_termoformatura"),
    );
    std::fs::create_dir_all(&shells).unwrap();
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(
        root.join("manifest.json"),
        br#"{"settings":{"clearance_mm":0.0}}"#,
    )
    .unwrap();
    let shell = shells.join("SUP_allineatore_01.stl");
    let mut f = std::fs::File::create(&shell).unwrap();
    dc_mesh::io::write_to(&mut f, &cup(30.0, 20.0, 10.0, 1.0), MeshFormat::Stl).unwrap();
    let model = block(
        Vec3::new(1.0, 1.0, 1.0) + model_offset,
        Vec3::new(29.0, 19.0, 10.0) + model_offset,
    );
    let mut f = std::fs::File::create(models.join("SUP_modello_01.stl")).unwrap();
    dc_mesh::io::write_to(&mut f, &model, MeshFormat::Stl).unwrap();
    shell
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

/// A popup with `resin()`, opened on `parts` (id, file), prepared.
fn prepared(parts: &[(&str, PathBuf)]) -> PrintPopup {
    let mut popup = PrintPopup::default();
    popup.set_resin(resin()).unwrap();
    popup.open(PrintRequest {
        parts: parts
            .iter()
            .map(|(id, path)| PrintItem {
                id: (*id).into(),
                label: id.replace('-', " "),
                path: Some(path.clone()),
                status: PartStatus::Missing,
            })
            .collect(),
        out_dir: None,
    });
    assert!(popup.prepare());
    wait(&mut popup);
    popup
}

fn window(popup: PrintPopup) -> Harness<'static, PrintPopup> {
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

const GUESSED: &str = "Fitting side guessed";
const FROM_MODEL: &str = "Fitting side taken from each aligner's model in the export: the supports stay on the outer side.";

/// The shell of an export takes its fitting side from its stage model (as
/// `print.prepare`); a loose shell has it guessed from its shape, and the
/// simple view says so.
#[test]
fn a_shell_of_an_export_takes_its_fitting_side_from_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let shell = write_export(&dir.path().join("export"), Vec3::ZERO);

    let popup = prepared(&[("SUP-01", shell.clone())]);
    assert_eq!(popup.failure(), None);
    assert_eq!(
        popup.fitting_sources(),
        Some(vec![("SUP-01".to_owned(), FittingSource::Model)])
    );
    assert_eq!(FittingSource::Model.as_str(), "model");
    let h = window(popup);
    assert!(h.query_by_label(FROM_MODEL).is_some());
    assert!(h.query_by_label(GUESSED).is_none(), "no warning");
    drop(h);

    let loose = write_cup(dir.path(), "INF-01");
    let popup = prepared(&[("SUP-01", shell), ("INF-01", loose)]);
    assert_eq!(popup.failure(), None);
    assert_eq!(
        popup.fitting_sources(),
        Some(vec![
            ("SUP-01".to_owned(), FittingSource::Model),
            ("INF-01".to_owned(), FittingSource::Geometry),
        ])
    );
    assert_eq!(FittingSource::Geometry.as_str(), "geometry");
    let h = window(popup);
    assert!(h.query_by_label(GUESSED).is_some(), "the warning is shown");
    assert!(
        h.query_by_label_contains("Fitting side guessed from shape; export again.")
            .is_some()
    );
    assert!(h.query_by_label("Guessed for: INF 01").is_some());
    assert!(h.query_by_label(FROM_MODEL).is_none());
}

/// A model from another position is refused, as `print.prepare` refuses
/// it: nothing is guessed.
#[test]
fn a_model_that_does_not_fit_its_shell_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let shell = write_export(dir.path(), Vec3::splat(3.0));
    let popup = prepared(&[("SUP-01", shell)]);
    assert_eq!(
        popup.failure().map(|f| f.code.as_str()),
        Some("print-model-mismatch")
    );
    assert_eq!(popup.fitting_sources(), None);
    let h = window(popup);
    assert!(
        h.query_by_label_contains("does not match its model in the export")
            .is_some()
    );
}

fn wait(popup: &mut PrintPopup) {
    let start = Instant::now();
    while popup.is_busy() {
        assert!(start.elapsed() < Duration::from_secs(300), "it did not end");
        std::thread::sleep(Duration::from_millis(10));
        popup.poll();
    }
}

#[test]
fn two_shells_go_on_one_plate_and_its_file_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let parts = ["SUP-01", "INF-01"]
        .map(|id| PrintItem {
            id: id.into(),
            label: id.replace('-', " "),
            path: Some(write_cup(dir.path(), id)),
            status: PartStatus::Missing,
        })
        .to_vec();
    let usb = dir.path().join("usb");
    let mut popup = PrintPopup::default();
    popup
        .set_resin(Resin {
            name: "Clear aligner".into(),
            layer_um: 100,
            exposure_s: 5.7,
            bottom_exposure_s: 35.0,
            bottom_layers: 4,
            rest_s: 1.0,
            density_g_ml: 1.1,
        })
        .unwrap();
    popup.open(PrintRequest {
        parts,
        out_dir: Some(usb.clone()),
    });
    assert!(popup.prepare());
    wait(&mut popup);
    assert_eq!(popup.failure(), None);
    let plates = popup.plates().expect("the plates are ready");
    assert_eq!(plates.len(), 1);
    assert_eq!(plates[0].parts, ["SUP-01", "INF-01"]);
    assert!(plates[0].layers > 0 && plates[0].volume_ml > 0.0);
    assert!(plates[0].preheat_s > 0, "the warm-up is counted");
    assert!(plates[0].est_time_s > plates[0].preheat_s);
    assert_eq!(plates[0].tank_target_c, Some(45.0));
    assert!(plates[0].resin_needed_ml > plates[0].volume_ml);

    // without the system dialog the files go to the request's folder
    assert!(popup.save_to(usb.clone()));
    wait(&mut popup);
    let file = usb.join("plate_01.goo");
    assert!(std::fs::metadata(&file).unwrap().len() > 195_477);
    assert_eq!(
        popup.take_events(),
        [PrintEvent::Saved {
            ids: vec!["SUP-01".into(), "INF-01".into()],
            files: vec![file],
        }]
    );
    // the plates stay ready for another stick
    assert!(popup.plates().is_some());

    // the window shows the plate, its estimates and what was saved
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 860.0))
        .with_context_setup(|ctx| {
            dc_ui::fonts::install(ctx);
            dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
        })
        .build_ui_state(|ui, p: &mut PrintPopup| p.show(ui), popup);
    h.run_steps(3);
    for label in [
        "Plate 1 of 1",
        "Aligners on this plate",
        "Print time (about)",
        "Resin to pour in the vat (at least)",
        "Saved",
        "plate_01.goo",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label} is not shown");
    }
    assert!(
        !h.get_by_label("Save print files…")
            .accesskit_node()
            .is_disabled()
    );
}
