//! The scan window (alpha) with the simulated scanner: its buttons and an
//! agent's `scan.*` commands drive the same scanner; a scan goes from the
//! acquisition to the case (one undoable import) and is saved with it.

use std::path::Path;
use std::time::{Duration, Instant};

use dc_case::ScanRole;
use dc_module_api::Module;
use dc_workspace::{Workspace, WorkspaceAction};
use dca_module::AlignersModule;
use egui::{Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(600);

fn module() -> Box<dyn Module> {
    Box::new(AlignersModule::new())
}

/// Runs the scan command `id` as an agent does and waits for its answer.
fn scan(ws: &mut Workspace, id: &str, params: Value) -> Result<Value, String> {
    let ticket = ws.start_scan_command(id, params).unwrap();
    let start = Instant::now();
    loop {
        if let Some(r) = ws.scan_result(ticket) {
            return r;
        }
        assert!(start.elapsed() < WAIT, "{id} did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn ok(ws: &mut Workspace, id: &str, params: Value) -> Value {
    scan(ws, id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Connects the simulated lower typodont, delivering `frames` small frames
/// as fast as they are computed, and scans until they are all in.
fn scan_lower(ws: &mut Workspace, frames: u64) {
    ok(
        ws,
        "scan.connect",
        json!({
            "backend": "sim",
            "device": "sim-typodont-lower",
            "options": { "realtime": false, "frames": frames, "width": 160, "height": 100 },
        }),
    );
    ok(ws, "scan.start", Value::Null);
    let start = Instant::now();
    loop {
        let st = ok(ws, "scan.status", Value::Null);
        if st["frames"].as_u64() == Some(frames) {
            assert!(st["fused_frames"].as_u64().unwrap() > 0, "{st}");
            break;
        }
        assert!(start.elapsed() < WAIT, "the frames did not come: {st}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_simulated_scan_goes_into_the_case_and_comes_back_after_save() {
    let mut ws = Workspace::new_headless(module(), None, None);
    let devices = ok(&mut ws, "scan.devices", Value::Null);
    assert!(
        devices["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["id"] == "sim-typodont-lower"),
        "{devices}"
    );
    // reading does not open the window, acting on the scanner does
    assert!(!ws.is_scan_open());
    scan_lower(&mut ws, 30);
    assert!(ws.is_scan_open());

    let stopped = ok(&mut ws, "scan.stop", Value::Null);
    assert!(stopped["points"].as_u64().unwrap() > 1000, "{stopped}");
    let cleaned = ok(
        &mut ws,
        "scan.cloud",
        json!({ "op": { "op": "remove-statistical", "k": 16, "std_ratio": 2.0 } }),
    );
    assert!(cleaned["editor"]["can_undo"] == json!(true), "{cleaned}");
    // nothing to import before the surface exists
    let early = scan(&mut ws, "scan.import", json!({ "role": "lower" })).unwrap_err();
    assert!(early.starts_with("no-mesh"), "{early}");
    let built = ok(&mut ws, "scan.mesh-build", json!({ "resolution_mm": 0.6 }));
    assert!(built["faces"].as_u64().unwrap() > 100, "{built}");
    ok(
        &mut ws,
        "scan.mesh",
        json!({ "op": { "op": "remove-islands", "min_fraction": 0.1 } }),
    );

    // the import: the case's own import, one undoable change, its report
    assert!(ws.state().case.scans.is_empty());
    let report = ok(&mut ws, "scan.import", json!({ "role": "lower" }));
    assert_eq!(
        report["imported"].as_array().map(Vec::len),
        Some(1),
        "{report}"
    );
    let lower = ws
        .state()
        .case
        .scan(ScanRole::Lower)
        .expect("the lower scan");
    assert!(ws.state().mesh(&lower.mesh).unwrap().face_count() > 100);
    assert!(ws.undo());
    assert!(ws.state().case.scans.is_empty());
    assert!(ws.redo());
    assert_eq!(ws.state().case.scans.len(), 1);

    // saved and opened again as Home opens it
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scanned.dcprj");
    ws.save_to(path.clone());
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(ws.take_action(), WorkspaceAction::Saved(path.clone()));
    let p = reopen(&path);
    let back = p.case_state.case.scan(ScanRole::Lower).unwrap();
    let mesh = p.case_state.mesh(&back.mesh).unwrap();
    let now = ws.state().mesh(&ws.state().case.scans[0].mesh).unwrap();
    assert_eq!(mesh.face_count(), now.face_count());
}

fn reopen(path: &Path) -> dc_project::Project {
    let codecs = dc_workspace::project_codecs(&AlignersModule::new());
    let loaded = dc_project::load(path, &codecs).unwrap();
    assert!(!loaded.read_only, "{:?}", loaded.issues);
    loaded.project
}

#[test]
fn an_agent_gets_the_scanner_errors_by_code() {
    let mut ws = Workspace::new_headless(module(), None, None);
    let e = scan(&mut ws, "scan.start", Value::Null).unwrap_err();
    assert!(e.starts_with("not-connected"), "{e}");
    let e = scan(&mut ws, "scan.connect", json!({ "backend": "nope" })).unwrap_err();
    assert!(e.starts_with("unknown-backend"), "{e}");
    assert!(ws.start_scan_command("scan.nothing", Value::Null).is_err());
}

/// `scan.status` until its automatic scan ended (`done`, `failed`,
/// `cancelled`); returns that status.
fn wait_auto(ws: &mut Workspace) -> Value {
    let start = Instant::now();
    loop {
        let st = ok(ws, "scan.status", Value::Null);
        let state = st["auto"]["state"].as_str().unwrap_or_default().to_owned();
        if matches!(state.as_str(), "done" | "failed" | "cancelled") {
            return st;
        }
        assert!(
            start.elapsed() < WAIT,
            "the automatic scan did not end: {st}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_agent_runs_an_automatic_scan_on_the_simulated_turntable() {
    let mut ws = Workspace::new_headless(module(), None, None);
    // the turntable commands need a turntable
    let e = scan(
        &mut ws,
        "turntable.move",
        json!({ "axis": "rotate", "by_deg": 10.0 }),
    )
    .unwrap_err();
    assert!(e.starts_with("turntable-not-connected"), "{e}");
    // without discovery: the simulated turntable, and why the others are
    // not usable here (no Bluetooth on a test machine is fine)
    let listed = ok(&mut ws, "turntable.devices", json!({ "timeout_s": 0 }));
    let sim = listed["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["backend"] == "sim")
        .cloned()
        .unwrap_or_else(|| panic!("no simulated turntable: {listed}"));
    for b in listed["backends"].as_array().unwrap() {
        if b["available"] == json!(false) {
            assert!(b["error_code"].is_string(), "{b}");
        }
    }
    ok(
        &mut ws,
        "turntable.connect",
        json!({ "backend": "sim", "device": sim["id"] }),
    );
    let st = ok(&mut ws, "turntable.status", Value::Null);
    assert_eq!(st["connected"], json!(true), "{st}");

    // a short automatic scan: one pass, flat, a quarter turn
    ok(
        &mut ws,
        "scan.connect",
        json!({
            "backend": "sim",
            "device": "sim-typodont-lower",
            "options": { "realtime": false, "width": 160, "height": 100 },
        }),
    );
    // without a calibration of this rig the automatic scan asks for the
    // consent of `allow_uncalibrated` (a calibration of this computer may
    // exist: then it starts at once)
    let auto =
        json!({ "tilts_deg": [0.0], "rotation_deg": 90.0, "deg_per_s": 90.0, "settle_s": 0.1 });
    if let Err(e) = scan(&mut ws, "scan.auto", auto.clone()) {
        assert!(e.starts_with("rig-uncalibrated"), "{e}");
        let mut anyway = auto;
        anyway["allow_uncalibrated"] = json!(true);
        ok(&mut ws, "scan.auto", anyway);
    }
    assert!(
        ws.is_scan_open(),
        "an agent's automatic scan shows the window"
    );
    let st = wait_auto(&mut ws);
    assert_eq!(st["auto"]["state"], json!("done"), "{st}");
    assert!(st["fused_frames"].as_u64().unwrap() > 0, "{st}");
    let stopped = ok(&mut ws, "scan.stop", Value::Null);
    assert!(stopped["points"].as_u64().unwrap() > 0, "{stopped}");
    let st = ok(&mut ws, "turntable.status", Value::Null);
    let turned = st["rotate_deg"].as_f64().unwrap();
    assert!((turned.abs() - 90.0).abs() < 1.0, "{st}");
    ok(&mut ws, "turntable.disconnect", Value::Null);
}

struct App {
    ws: Workspace,
}

fn harness(ws: Workspace) -> Harness<'static, App> {
    Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, app: &mut App| {
                let _ = app.ws.show(ui);
            },
            App { ws },
        )
}

/// Frames until `done` holds (the window repaints while a command runs).
fn frames_until(
    h: &mut Harness<'static, App>,
    what: &str,
    done: impl Fn(&Harness<'static, App>) -> bool,
) {
    let start = Instant::now();
    while !done(h) {
        assert!(start.elapsed() < WAIT, "{what}");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
}

#[test]
fn the_scan_button_opens_the_window_and_starts_nothing() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    // the empty case: Import scans and, beside it, Scan with its alpha badge
    assert!(h.query_by_label("ALPHA").is_some());
    h.get_by_role_and_label(Role::Button, "Scan").click();
    h.step();
    assert!(h.state().ws.is_scan_open());
    frames_until(&mut h, "the scanners are listed", |h| {
        !h.state().ws.is_scan_busy()
    });
    assert!(h.query_by_label("Scan (alpha)").is_some());
    // nothing was started: the scanner is not even connected
    assert!(h.query_by_label("Not connected").is_some());

    // its Connect button runs `scan.connect`, as an agent would
    h.get_by_role_and_label(Role::Button, "Connect").click();
    h.step();
    frames_until(&mut h, "connected", |h| {
        !h.state().ws.is_scan_busy() && h.query_by_label("Connected").is_some()
    });
    assert!(
        h.query_by_role_and_label(Role::Button, "Disconnect")
            .is_some()
    );

    // closing with nothing scanned asks nothing, and disconnects
    h.get_by_label("Close the scan window").click();
    h.step();
    assert!(!h.state().ws.is_scan_open());
    frames_until(&mut h, "disconnected", |h| !h.state().ws.is_scan_busy());
}

#[test]
fn the_turntable_block_connects_the_simulated_turntable() {
    let mut h = harness(Workspace::new_headless(module(), None, None));
    h.run();
    h.get_by_role_and_label(Role::Button, "Scan").click();
    h.step();
    frames_until(&mut h, "the scanners and turntables are listed", |h| {
        !h.state().ws.is_scan_busy()
    });
    assert!(h.query_by_label("Turntable").is_some());
    // without a turntable the automatic scan says what it needs
    assert!(
        h.query_by_label("Connect a turntable for an automatic scan.")
            .is_some()
    );
    h.get_by_role_and_label(Role::Button, "Connect the turntable")
        .click();
    h.step();
    frames_until(&mut h, "the turntable is connected", |h| {
        !h.state().ws.is_scan_busy() && h.query_by_label("Ready").is_some()
    });
    assert!(
        h.query_by_role_and_label(Role::Button, "Disconnect the turntable")
            .is_some()
    );
    // without a calibration the automatic scan asks first; "Calibrate
    // first" goes back to the calibration
    if h.query_by_label("Not calibrated yet").is_some() {
        let ticket = h
            .state_mut()
            .ws
            .start_scan_command(
                "scan.connect",
                json!({ "backend": "sim", "device": "sim-typodont-lower" }),
            )
            .unwrap();
        frames_until(&mut h, "the simulated scanner is connected", |h| {
            !h.state().ws.is_scan_busy()
        });
        assert!(h.state_mut().ws.scan_result(ticket).unwrap().is_ok());
        // the folded turntable controls bring Acquisition into view
        h.get_by_label("Hide the turntable controls").click();
        h.step();
        h.get_by_role_and_label(Role::Button, "Start the automatic scan")
            .click();
        h.step();
        h.step();
        assert!(h.query_by_label("Scan without calibrating?").is_some());
        h.get_by_role_and_label(Role::Button, "Calibrate first")
            .click();
        h.step();
        h.step();
        assert!(h.query_by_label("Scan without calibrating?").is_none());
        assert!(!h.state().ws.is_scan_busy(), "nothing was started");
    }
    assert!(
        h.query_by_label("Not calibrated yet").is_some()
            || h.query_by_label_contains("Calibrated").is_some()
    );
    // the automatic scan waits for the scanner
    assert!(
        h.query_by_role_and_label(Role::Button, "Start the automatic scan")
            .is_some()
    );
    h.get_by_label("Close the scan window").click();
    h.step();
    frames_until(&mut h, "disconnected", |h| !h.state().ws.is_scan_busy());
}
