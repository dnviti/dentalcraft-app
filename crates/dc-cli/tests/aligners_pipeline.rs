//! End-to-end test of the Aligners pipeline (the vertical slice): a case
//! from a golden fixture scan (both arches) and from the demo arches, the
//! whole pipeline through `aligners.auto` on a headless `dc_session::Session`
//! with the Aligners module, then the export checked file by file against
//! its manifest. Finally the project round-trips through `.dcprj`.
//!
//! The golden fixtures live in the aligners working tree of the superproject
//! (`dentalcraft-aligners/golden/fixtures`); the fixture test is skipped
//! when they are not there (a standalone checkout of this repository).

use std::path::{Path, PathBuf};

use dc_case::{CaseState, ScanRole};
use dc_session::Session;
use dca_model::{AlignersState, MODULE_ID};
use serde_json::{Value, json};

/// The pipeline steps `aligners.auto` runs on a case with both arches not
/// related and their seeds, with the full export (the check of the
/// segmentation runs once per arch): as the plugin's `case_auto.auto_steps`,
/// the virtual bite comes before the final position.
const STEPS: &[&str] = &[
    "aligners.case.detect-arch",
    "aligners.preparation.orientation.automatic",
    "aligners.preparation.segmentation.automatic",
    "aligners.preparation.segmentation.check",
    "aligners.preparation.segmentation.check",
    "aligners.preparation.reconstruction.automatic",
    "aligners.preparation.gingiva",
    "aligners.diagnosis.space-analysis",
    "aligners.occlusion.virtual-bite",
    "aligners.planning.setup.automatic",
    "aligners.planning.staging.automatic",
    "aligners.planning.schedule",
    "aligners.occlusion.contact-map",
    "aligners.output.report",
];

fn session() -> Session {
    dca_module::loader();
    let mut s = Session::new();
    s.register_module(Box::new(dca_module::AlignersModule::new()))
        .unwrap();
    s
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dentalcraft-aligners/golden/fixtures")
}

fn aligners(state: &CaseState) -> &AlignersState {
    state
        .module_data::<AlignersState>(MODULE_ID)
        .expect("Aligners data")
}

/// Runs `aligners.auto` with the full export into `dir` and checks every
/// step, the state it leaves and the exported files. Returns the output.
fn run_and_check(s: &mut Session, dir: &Path, numbering: usize) -> Value {
    let out = s
        .execute(
            "aligners.auto",
            json!({
                "start_date": "2026-11-02",
                "export_dir": dir.display().to_string(),
                "full_report": true,
            }),
        )
        .unwrap();
    let steps = out["steps"].as_array().unwrap();
    for st in steps {
        eprintln!(
            "{:>2} {:<48} {:>7.2} s",
            st["step"],
            st["id"].as_str().unwrap(),
            st["seconds"].as_f64().unwrap()
        );
    }
    // the steps, in pipeline order, the numbering of the arches without seeds
    // before the orientation, which takes the frame from the new seeds
    let ids: Vec<&str> = steps.iter().map(|s| s["id"].as_str().unwrap()).collect();
    let mut expected = STEPS.to_vec();
    for _ in 0..numbering {
        expected.insert(1, "aligners.preparation.numbering.automatic");
    }
    assert_eq!(ids, expected);
    let by_id = |id: &str| -> &Value { &steps.iter().find(|s| s["id"] == id).unwrap()["output"] };

    // every stage left its results in the case
    let state = s.state().clone();
    let data = aligners(&state);
    for role in [ScanRole::Upper, ScanRole::Lower] {
        assert!(data.labels.contains_key(&role), "{role:?} labels");
        assert!(data.gingiva.contains_key(&role), "{role:?} gingiva");
    }
    assert!(data.seeds.len() >= 24, "{} seeds", data.seeds.len());
    assert!(data.tooth_frames.len() >= 24);
    assert_eq!(data.crowns.len(), data.tooth_frames.len());
    assert_eq!(data.tooth_geometry.len(), data.tooth_frames.len());
    assert_eq!(data.diagnosis.arches.len(), 2);
    assert!(data.diagnosis.bolton_overall.is_some());
    assert!(!data.setup.poses.is_empty());
    assert!(!data.stages.is_empty());
    assert!(state.case.world_frame.from_case != dc_case::Pose::IDENTITY);
    for blob in data.crowns.values().chain(data.gingiva.values()) {
        assert!(state.referenced_blobs().contains(blob));
    }

    // the numbers of the stages are consistent with each other
    let staged = by_id("aligners.planning.staging.automatic");
    let nl = staged["lower"]["aligners"].as_u64().unwrap();
    let nu = staged["upper"]["aligners"].as_u64().unwrap();
    assert!(nl > 0 && nu > 0);
    let cal = by_id("aligners.planning.schedule");
    assert_eq!(cal["lower"]["n_active"], nl);
    assert_eq!(cal["upper"]["n_active"], nu);
    assert_eq!(
        by_id("aligners.occlusion.contact-map")["in_occlusion"],
        true
    );
    // the plugin's warning: the relation is estimated, not registered
    assert!(
        out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("VIRTUAL occlusion")),
        "{}",
        out["warnings"]
    );
    assert!(data.bite_registered);
    let diag = by_id("aligners.diagnosis.space-analysis");
    assert!(diag["lower"]["little"].as_f64().unwrap() >= 0.0);
    assert!(diag["bolton"]["overall"].as_f64().unwrap() > 80.0);

    // the export: models of every stage, tables, calendars, report, manifest
    let report = by_id("aligners.output.report");
    assert_eq!(report["aligners"]["lower"], nl);
    assert_eq!(report["aligners"]["upper"], nu);
    let files = report["files"].as_array().unwrap();
    let stl: Vec<&str> = files
        .iter()
        .filter(|f| f["kind"] == "stl")
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(stl.len() as u64, nl + nu + 2, "{stl:?}");
    for f in files {
        let p = dir.join(f["name"].as_str().unwrap());
        assert!(p.is_file(), "{}", p.display());
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            f["bytes"].as_u64().unwrap()
        );
    }
    for n in [
        "stages.csv",
        "visits.ics",
        "report.html",
        "print_check.json",
        "manifest.json",
    ] {
        assert!(dir.join(n).is_file(), "missing {n}");
    }
    // the models are binary STL in the case coordinates (mm): the size of an arch
    let first = std::fs::read(dir.join(stl[0])).unwrap();
    let tris = u32::from_le_bytes(first[80..84].try_into().unwrap()) as usize;
    assert_eq!(first.len(), 84 + 50 * tris);
    let csv = std::fs::read_to_string(dir.join("stages.csv")).unwrap();
    assert!(csv.starts_with("arch,stage,aligner,tooth,"));
    let ics = std::fs::read_to_string(dir.join("visits.ics")).unwrap();
    assert!(ics.starts_with("BEGIN:VCALENDAR") && ics.contains("20261102"));
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["total_aligners"], nl.max(nu));
    // every hash verifies
    let v = s
        .execute(
            "aligners.output.manifest",
            json!({ "dir": dir.display().to_string() }),
        )
        .unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        v["verification"]["verified"].as_u64().unwrap() as usize,
        files.len()
    );
    out
}

/// Saves the project and opens it again: the Aligners data survives.
fn round_trip(s: &mut Session, dir: &Path) {
    let before = aligners(s.state()).clone();
    let path = dir.join("case.dcprj");
    s.save(Some(&path)).unwrap();
    let mut t = session();
    t.open(&path).unwrap();
    let after = aligners(t.state());
    // the blobs are content addressed: exactly the same
    assert_eq!(after.crowns, before.crowns);
    assert_eq!(after.gingiva, before.gingiva);
    assert_eq!(after.labels, before.labels);
    // the numbers go through JSON text (within a few ulps)
    let close = |a: &dc_case::Pose, b: &dc_case::Pose| {
        (a.t - b.t).length() < 1e-9 && a.q.dot(b.q).abs() > 1.0 - 1e-12
    };
    assert_eq!(after.stages.len(), before.stages.len());
    for (a, b) in after.stages.iter().zip(&before.stages) {
        assert_eq!(a.index, b.index);
        assert_eq!(a.poses.len(), b.poses.len());
        assert!(a.poses.iter().all(|(f, p)| close(p, &b.poses[f])));
    }
    assert_eq!(after.setup.poses.len(), before.setup.poses.len());
    assert!(
        after
            .setup
            .poses
            .iter()
            .all(|(f, p)| close(p, &before.setup.poses[f]))
    );
    assert_eq!(after.setup.ipr.len(), before.setup.ipr.len());
}

#[test]
fn the_pipeline_runs_on_the_golden_fixture_scans_of_both_arches() {
    let root = fixtures();
    let (lower, upper) = (
        root.join("demo_lower_mild/scan.dcm"),
        root.join("demo_upper_mild/scan.dcm"),
    );
    if !lower.is_file() || !upper.is_file() {
        eprintln!("skipped: no golden fixtures at {}", root.display());
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // the scans as the user gets them: STL files, imported into a new case
    let mut s = session();
    for (src, name) in [(&lower, "lower.stl"), (&upper, "upper.stl")] {
        let mesh = dc_mesh::dcm::decode(&std::fs::read(src).unwrap()).unwrap();
        let path = tmp.path().join(name);
        dc_mesh::io::write(&path, &mesh, dc_mesh::io::MeshFormat::Stl).unwrap();
        let out = s
            .execute("case.import", json!({ "path": path.display().to_string() }))
            .unwrap();
        assert_eq!(out["role"], name.trim_end_matches(".stl"), "{out}");
    }
    // the fixture scans come without seeds: the numbering places them
    let out = run_and_check(&mut s, &tmp.path().join("export"), 2);
    eprintln!("golden fixture pipeline: {:.2} s", out["seconds"]);
    round_trip(&mut s, tmp.path());
}

/// The whole pipeline on the golden fixtures of both arches with the print
/// files of the shell stage at the draft quality (`DCA_QUALITY` for
/// another one): a printable model per stage and a trimmed aligner per
/// aligner, each checked by the print check. Prints the timings and the
/// result of every file; fails on the first file that is not printable.
#[test]
#[ignore = "the shells of every stage of both arches: run with --release --ignored --nocapture"]
fn the_pipeline_makes_printable_shells_on_the_golden_fixture_scans() {
    let root = fixtures();
    let (lower, upper) = (
        root.join("demo_lower_mild/scan.dcm"),
        root.join("demo_upper_mild/scan.dcm"),
    );
    if !lower.is_file() || !upper.is_file() {
        eprintln!("skipped: no golden fixtures at {}", root.display());
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut s = session();
    for src in [&lower, &upper] {
        let mesh = dc_mesh::dcm::decode(&std::fs::read(src).unwrap()).unwrap();
        let name = if src == &lower {
            "lower.stl"
        } else {
            "upper.stl"
        };
        let path = tmp.path().join(name);
        dc_mesh::io::write(&path, &mesh, dc_mesh::io::MeshFormat::Stl).unwrap();
        s.execute("case.import", json!({ "path": path.display().to_string() }))
            .unwrap();
    }
    let quality = std::env::var("DCA_QUALITY").unwrap_or_else(|_| "draft".to_owned());
    let dir = tmp.path().join("export");
    let out = s
        .execute(
            "aligners.auto",
            json!({
                "start_date": "2026-11-02",
                "export_dir": dir.display().to_string(),
                "shells": true,
                "quality": quality,
            }),
        )
        .unwrap();
    let steps = out["steps"].as_array().unwrap();
    for st in steps {
        eprintln!(
            "{:>2} {:<48} {:>7.2} s",
            st["step"],
            st["id"].as_str().unwrap(),
            st["seconds"].as_f64().unwrap()
        );
    }
    let report = &steps.last().unwrap()["output"];
    assert_eq!(steps.last().unwrap()["id"], "aligners.output.report");
    assert_eq!(report["meshes"], "shells", "{}", report["notes"]);
    eprintln!("notes: {:#}", report["notes"]);
    let nl = report["aligners"]["lower"].as_u64().unwrap();
    let nu = report["aligners"]["upper"].as_u64().unwrap();
    let files = report["files"].as_array().unwrap();
    let count = |prefix: &str| {
        files
            .iter()
            .filter(|f| f["name"].as_str().unwrap().starts_with(prefix))
            .count() as u64
    };
    // a model per stage (0 the initial one), an aligner per aligner and the
    // passive ones of the shorter arch
    assert_eq!(count("modelli_termoformatura/"), nl + nu + 2);
    assert_eq!(count("stampa_resina/"), 2 * nl.max(nu));
    let check: Value =
        serde_json::from_slice(&std::fs::read(dir.join("print_check.json")).unwrap()).unwrap();
    let mut failed = Vec::new();
    for f in check["files"].as_array().unwrap() {
        let c = &f["check"];
        eprintln!(
            "{:<52} {:<5} faces {:>8} volume {:>6.2} ml wall {:>5} {}",
            f["name"].as_str().unwrap(),
            if f["ok"] == true { "ok" } else { "FAIL" },
            c["triangles"],
            c["volume_mm3"].as_f64().unwrap_or(0.0) / 1000.0,
            c["min_thickness_mm"],
            f["problems_text"]
        );
        if f["ok"] != true {
            failed.push(f["name"].as_str().unwrap().to_owned());
        }
    }
    let v = s
        .execute(
            "aligners.output.manifest",
            json!({ "dir": dir.display().to_string() }),
        )
        .unwrap();
    assert_eq!(v["ok"], true, "{v}");
    eprintln!(
        "golden fixture pipeline with shells ({quality}): {:.2} s",
        out["seconds"]
    );
    assert!(failed.is_empty(), "not printable: {failed:?}");
}

#[test]
fn the_pipeline_runs_on_the_demo_arches() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = session();
    s.execute("aligners.demo", json!({ "arch": "lower" }))
        .unwrap();
    s.execute("aligners.demo", json!({ "arch": "upper" }))
        .unwrap();
    // the demo has its seeds: no numbering
    let out = run_and_check(&mut s, &tmp.path().join("export"), 0);
    eprintln!("demo pipeline: {:.2} s", out["seconds"]);
    // one undoable change (after the manifest check, which changes nothing)
    let mut undone = 0;
    while !aligners(s.state()).stages.is_empty() && undone < 3 {
        assert!(s.undo());
        undone += 1;
    }
    assert!(aligners(s.state()).stages.is_empty(), "{undone} undo steps");
    for _ in 0..undone {
        assert!(s.redo());
    }
    assert!(!aligners(s.state()).stages.is_empty());
    round_trip(&mut s, tmp.path());
}

/// The separation parameters of the case (`aligners.settings.segmentation`)
/// and the separation that takes them: out-of-range values are refused and
/// change nothing, the output carries the teeth, the time and what the new
/// labels left out of date.
#[test]
fn the_segmentation_settings_and_run_work_on_the_demo_case() {
    let mut s = session();
    s.execute("aligners.demo", json!({ "arch": "lower" }))
        .unwrap();
    let out = s
        .execute(
            "aligners.settings.segmentation",
            json!({ "target_faces": 5000 }),
        )
        .unwrap();
    assert_eq!(out["target_faces"], 5000);
    assert_eq!(aligners(s.state()).settings.segmentation.target_faces, 5000);
    for bad in [
        json!({ "target_faces": 4999 }),
        json!({ "target_faces": 200_001 }),
        json!({ "eta_convex": 2.0 }),
    ] {
        assert!(
            s.execute("aligners.settings.segmentation", bad).is_err(),
            "an out-of-range value was accepted"
        );
    }
    assert_eq!(aligners(s.state()).settings.segmentation.target_faces, 5000);
    let out = s
        .execute("aligners.preparation.segmentation.automatic", Value::Null)
        .unwrap();
    assert!(out["teeth"].as_u64().unwrap() > 0, "{out}");
    assert!(out["seconds"].as_f64().unwrap() >= 0.0, "{out}");
    assert!(out["invalidated"].is_array(), "{out}");
    assert!(aligners(s.state()).labels.contains_key(&ScanRole::Lower));
}

/// MCP and the CLI prefer the module's command over `case.import`: everything
/// a client reads from `case.import` must be there, with the same value where
/// the case does not make it differ.
#[test]
fn the_import_of_the_module_is_a_superset_of_case_import() {
    let tmp = tempfile::tempdir().unwrap();
    let mut demo = session();
    demo.execute("aligners.demo", json!({"arch": "lower"}))
        .unwrap();
    let state = demo.state().clone();
    let scan = state.case.scan(ScanRole::Lower).unwrap();
    let mesh = state.blobs.require(&scan.mesh).unwrap();
    let path = tmp.path().join("lower.stl");
    dc_mesh::io::write(&path, mesh, dc_mesh::io::MeshFormat::Stl).unwrap();
    let params = json!({ "path": path.display().to_string() });

    let core = session().execute("case.import", params.clone()).unwrap();
    let module = session()
        .execute("aligners.case.import-scans", params)
        .unwrap();
    let (core, module) = (core.as_object().unwrap(), module.as_object().unwrap());
    for (key, value) in core {
        assert!(module.contains_key(key), "{key} missing");
        if !matches!(key.as_str(), "scan_id" | "imported") {
            assert_eq!(&module[key], value, "{key}");
        }
    }
    assert!(module.contains_key("detections"));
}

/// The demo arches with a virtual bite applied: the upper arch has moved and
/// the module remembers that registration.
fn registered_session() -> Session {
    let mut s = session();
    s.execute("aligners.demo", json!({ "arch": "lower" }))
        .unwrap();
    s.execute("aligners.demo", json!({ "arch": "upper", "seed": 11 }))
        .unwrap();
    s.execute("aligners.occlusion.virtual-bite", Value::Null)
        .unwrap();
    assert!(aligners(s.state()).bite_registered);
    s
}

fn cleared(out: &Value) -> Vec<&str> {
    let ids = out["module_cleared"].as_array().unwrap();
    ids.iter().map(|v| v.as_str().unwrap()).collect()
}

/// An arch changed after a bite registration: every surface says through
/// `module_cleared` that the registration is stale (the other arch stays where
/// it moved it), with an id the workspace translates, and says it once.
#[test]
fn a_stale_bite_registration_is_reported_by_case_set_role_and_case_import() {
    let tmp = tempfile::tempdir().unwrap();

    // case.set-role: the arches swap
    let mut s = registered_session();
    let out = s
        .execute("case.set-role", json!({ "from": "lower", "to": "upper" }))
        .unwrap();
    assert!(cleared(&out).contains(&"bite_registered"), "{out}");
    assert!(!aligners(s.state()).bite_registered);
    assert!(s.undo());
    assert!(aligners(s.state()).bite_registered, "undo brings it back");
    // confirming a role changes no arch: the registration holds
    let out = s
        .execute("case.set-role", json!({ "from": "lower", "to": "lower" }))
        .unwrap();
    assert!(!cleared(&out).contains(&"bite_registered"), "{out}");

    // case.import: a lower arch replaces the lower scan
    let mut s = registered_session();
    let state = s.state().clone();
    let mesh = state
        .blobs
        .require(&state.case.scan(ScanRole::Lower).unwrap().mesh)
        .unwrap();
    let path = tmp.path().join("scan_a.stl");
    dc_mesh::io::write(&path, mesh, dc_mesh::io::MeshFormat::Stl).unwrap();
    let params = json!({ "path": path.display().to_string(), "role": "lower" });
    let out = s.execute("case.import", params.clone()).unwrap();
    assert!(cleared(&out).contains(&"bite_registered"), "{out}");
    assert!(!aligners(s.state()).bite_registered);
    // said once
    let out = s.execute("case.import", params).unwrap();
    assert!(!cleared(&out).contains(&"bite_registered"), "{out}");

    // the module's own import reports it the same way
    let mut s = registered_session();
    let params = json!({ "path": path.display().to_string(), "role": "upper" });
    let out = s.execute("aligners.case.import-scans", params).unwrap();
    assert!(cleared(&out).contains(&"bite_registered"), "{out}");

    // the arch chosen by hand renumbers instead of clearing, and says so too
    let mut s = registered_session();
    let out = s
        .execute(
            "aligners.case.set-arch",
            json!({ "scan": "lower", "arch": "upper" }),
        )
        .unwrap();
    assert!(cleared(&out).contains(&"bite_registered"), "{out}");
    assert!(!aligners(s.state()).seeds.is_empty());

    // the id is a key the module translates, as the workspace reads it
    assert!(dc_i18n::has("aligners-cleared-bite-registered"));
}

/// The scan corrections of "Import scans" are commands of the CLI and the MCP
/// server, described and undoable like the others.
#[test]
fn the_scan_corrections_are_reachable_from_the_session() {
    let mut s = session();
    for id in ["aligners.case.set-arch", "aligners.case.make-base"] {
        let d = s.describe(id).unwrap();
        assert!(!d.doc.is_empty(), "{id}");
    }
    s.execute("aligners.demo", json!({ "arch": "lower" }))
        .unwrap();
    let out = s
        .execute("aligners.case.set-arch", json!({ "flip_occlusal": true }))
        .unwrap();
    assert_eq!(out["occlusal_flipped"], true, "{out}");
    assert!(
        aligners(s.state())
            .occlusal_flipped
            .contains(&ScanRole::Lower)
    );
    // the demo scan is closed: the base leaves it as it is
    let out = s
        .execute("aligners.case.make-base", json!({ "height_mm": 3.0 }))
        .unwrap();
    assert_eq!(out["already_closed"], true, "{out}");
    assert!(s.undo() && s.undo());
    assert!(aligners(s.state()).occlusal_flipped.is_empty());
}

#[test]
fn the_pipeline_stops_at_the_first_failing_step_and_leaves_the_case() {
    let mut s = session();
    let err = s.execute("aligners.auto", Value::Null).unwrap_err();
    assert!(err.to_string().contains("no upper or lower scan"), "{err}");
    s.execute("aligners.demo", json!({ "arch": "lower" }))
        .unwrap();
    let before = s.state().clone();
    // an export directory that cannot be created: every stage runs, the export fails
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    let err = s
        .execute(
            "aligners.auto",
            json!({ "export_dir": file.join("sub").display().to_string() }),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Stopped at Export moved scans and calendar"),
        "{err}"
    );
    assert_eq!(s.state().case, before.case);
    assert!(aligners(s.state()).stages.is_empty());
}
