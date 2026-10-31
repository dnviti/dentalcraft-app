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

/// The pipeline steps `aligners.auto` runs on a case with both arches and
/// their seeds, with the full export.
const STEPS: &[&str] = &[
    "aligners.preparation.orientation.automatic",
    "aligners.preparation.segmentation.automatic",
    "aligners.preparation.reconstruction.automatic",
    "aligners.preparation.gingiva",
    "aligners.diagnosis.space-analysis",
    "aligners.planning.setup.automatic",
    "aligners.planning.staging.automatic",
    "aligners.planning.schedule",
    "aligners.occlusion.virtual-bite",
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
    // the steps, in pipeline order, the numbering of the arches without seeds second
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
    assert_eq!(cal["lower"]["rows"][0]["start"], "2026-11-02");
    assert_eq!(
        by_id("aligners.occlusion.contact-map")["in_occlusion"],
        true
    );
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
        err.contains("aligners.auto stopped at step") && err.contains("aligners.output.export-stl"),
        "{err}"
    );
    assert_eq!(s.state().case, before.case);
    assert!(aligners(s.state()).stages.is_empty());
}
