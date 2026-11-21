//! The whole popup flow on two small cup-shaped shells: Prepare places them
//! on a plate, the plate shows its estimates, "Save print files" writes the
//! plate file and tells the host which parts it carries.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dc_mesh::TriMesh;
use dc_mesh::io::MeshFormat;
use dc_print_ui::{PartStatus, PrintEvent, PrintItem, PrintPopup, PrintRequest, Resin};
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
