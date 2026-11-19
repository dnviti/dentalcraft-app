//! The scan popup hosted by a screen that is not a workspace: its button
//! opens it for the roles of the host, a simulated scan goes back to the
//! host as a file it owns, and an agent's `scan.import` answers with what
//! the host did.

use std::time::{Duration, Instant};

use dc_case::ScanRole;
use dc_scan_ui::{ImportWords, ScanOutput, ScanPopup, ScanTarget, scan_button};
use egui::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(600);

/// A host: the popup and the files it took.
#[derive(Default)]
struct Host {
    scan: ScanPopup,
    taken: Vec<ScanOutput>,
}

impl Host {
    /// What a host does every frame: polls, takes the files, answers them.
    fn poll(&mut self) {
        self.scan.poll();
        for out in self.scan.take_outputs() {
            let answer = json!({ "added": out.path().to_string_lossy() });
            self.scan.output_done(out.id(), Ok(answer));
            self.taken.push(out);
        }
    }

    /// Runs the scan command `id` as an agent does and waits for its answer.
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let ticket = self.scan.start_command(id, params).unwrap();
        let start = Instant::now();
        loop {
            self.poll();
            if let Some(r) = self.scan.take_result(ticket) {
                return r;
            }
            assert!(start.elapsed() < WAIT, "{id} did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ok(&mut self, id: &str, params: Value) -> Value {
        self.run(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }
}

#[test]
fn a_simulated_scan_goes_to_the_host_as_a_file_it_owns() {
    let mut host = Host::default();
    host.scan.set_target(ScanTarget::new([ScanRole::Lower]));
    let frames = 30;
    host.ok(
        "scan.connect",
        json!({
            "backend": "sim",
            "device": "sim-typodont-lower",
            "options": { "realtime": false, "frames": frames, "width": 160, "height": 100 },
        }),
    );
    assert!(
        host.scan.is_open(),
        "acting on the scanner opens the window"
    );
    host.ok("scan.start", Value::Null);
    let start = Instant::now();
    while host.ok("scan.status", Value::Null)["frames"].as_u64() != Some(frames) {
        assert!(start.elapsed() < WAIT, "the frames did not come");
        std::thread::sleep(Duration::from_millis(20));
    }
    host.ok("scan.stop", Value::Null);
    // nothing to hand over before the surface exists
    let early = host
        .run("scan.import", json!({ "role": "lower" }))
        .unwrap_err();
    assert!(early.starts_with("no-mesh"), "{early}");
    host.ok("scan.mesh-build", json!({ "resolution_mm": 0.6 }));

    let answer = host.ok("scan.import", json!({ "role": "lower" }));
    let [out] = host.taken.as_slice() else {
        panic!("{} files handed over", host.taken.len());
    };
    assert_eq!(out.role(), Some(ScanRole::Lower));
    let path = out.path().to_path_buf();
    assert!(path.is_file(), "{}", path.display());
    let mesh = dc_mesh::io::read(&path).unwrap();
    assert!(mesh.face_count() > 100);
    // the agent reads what the host answered
    assert_eq!(answer["added"], json!(path.to_string_lossy()));
    assert!(!host.scan.is_busy());

    // the file lives as long as the host keeps it
    host.taken.clear();
    assert!(!path.exists(), "{} stayed", path.display());
}

#[test]
fn the_host_refusing_a_file_is_the_agents_answer() {
    let mut host = Host::default();
    host.ok(
        "scan.connect",
        json!({
            "backend": "sim",
            "device": "sim-typodont-lower",
            "options": { "realtime": false, "frames": 10, "width": 160, "height": 100 },
        }),
    );
    host.ok("scan.start", Value::Null);
    let start = Instant::now();
    while host.ok("scan.status", Value::Null)["frames"].as_u64() != Some(10) {
        assert!(start.elapsed() < WAIT, "the frames did not come");
        std::thread::sleep(Duration::from_millis(20));
    }
    host.ok("scan.stop", Value::Null);
    host.ok("scan.mesh-build", json!({ "resolution_mm": 0.6 }));
    let ticket = host
        .scan
        .start_command("scan.import", json!({ "role": "bite" }))
        .unwrap();
    let start = Instant::now();
    let refused = loop {
        host.scan.poll();
        let outs = host.scan.take_outputs();
        if let Some(out) = outs.first() {
            assert_eq!(out.role(), Some(ScanRole::Bite));
            // busy until the host answers
            assert!(host.scan.is_busy());
            host.scan.output_done(
                out.id(),
                Err(dc_scan_ui::ScanFailure::new("busy", "the host is busy")),
            );
        }
        if let Some(r) = host.scan.take_result(ticket) {
            break r.unwrap_err();
        }
        assert!(start.elapsed() < WAIT, "scan.import did not end");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(refused, "busy: the host is busy");
    assert!(!host.scan.is_busy());
}

/// A screen with the button and the popup, opened for one role with the
/// host's words.
fn harness() -> Harness<'static, Host> {
    Harness::builder()
        .with_size(egui::vec2(1600.0, 1000.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, host: &mut Host| {
                if scan_button(ui, true) {
                    let words = ImportWords {
                        button: "Add to the project".into(),
                        hint: "The scan joins the new project.".into(),
                        done: "In the project".into(),
                        unsaved: "This scan is not in the project.".into(),
                    };
                    host.scan.open(
                        ScanTarget::new([ScanRole::Bite])
                            .with_context("New project PT-7")
                            .with_words(words),
                    );
                }
                host.poll();
                host.scan.show(ui);
            },
            Host::default(),
        )
}

#[test]
fn the_button_of_any_screen_opens_the_window_and_starts_nothing() {
    let mut h = harness();
    h.run();
    assert!(h.query_by_label("ALPHA").is_some());
    h.get_by_role_and_label(Role::Button, "Scan").click();
    h.step();
    assert!(h.state().scan.is_open());
    let start = Instant::now();
    while h.state().scan.is_busy() {
        assert!(start.elapsed() < WAIT, "the scanners are not listed");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
    assert!(h.query_by_label("Scan (alpha)").is_some());
    assert!(h.query_by_label("New project PT-7").is_some());
    assert!(h.query_by_label("Not connected").is_some());
    h.get_by_label("Close the scan window").click();
    h.step();
    assert!(!h.state().scan.is_open());
}

#[test]
fn an_agent_closes_the_window_once_the_scan_is_done() {
    let mut host = Host::default();
    host.scan.set_target(ScanTarget::new([ScanRole::Lower]));
    host.ok(
        "scan.connect",
        json!({
            "backend": "sim",
            "device": "sim-typodont-lower",
            "options": { "realtime": false, "frames": 20, "width": 160, "height": 100 },
        }),
    );
    assert!(host.scan.is_open());
    host.ok("scan.start", Value::Null);
    // a scan under way is not closed under the agent
    let refused = host.run("scan.close", Value::Null).unwrap_err();
    assert!(refused.starts_with("invalid-state"), "{refused}");
    assert!(
        host.scan.is_open(),
        "the refused close left the window open"
    );
    let start = Instant::now();
    while host.ok("scan.status", Value::Null)["frames"].as_u64() != Some(20) {
        assert!(start.elapsed() < WAIT, "the frames did not come");
        std::thread::sleep(Duration::from_millis(20));
    }
    host.ok("scan.stop", Value::Null);
    host.ok("scan.mesh-build", json!({ "resolution_mm": 0.6 }));
    host.ok("scan.import", json!({ "role": "lower" }));
    assert_eq!(host.taken.len(), 1);
    // imported: the agent closes the window, which disconnects the scanner
    assert!(host.scan.is_open());
    host.ok("scan.close", Value::Null);
    assert!(!host.scan.is_open(), "scan.close closes the window");
    assert!(!host.scan.is_busy());
    // the imported file stays the host's
    assert!(host.taken[0].path().is_file());
    // closed twice is fine, and does not open it again
    host.ok("scan.close", Value::Null);
    assert!(!host.scan.is_open());
    let status = host.ok("scan.status", Value::Null);
    assert_eq!(status["state"], json!("idle"), "{status}");
}
