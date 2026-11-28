//! Updates in the app: the check at startup offers a newer version (quietly
//! when it fails or the version was skipped), the dialog says what the user
//! can do for each kind of installation, and "Update now" downloads and
//! checks the update, then "Restart now" saves the project and puts the new
//! version in place when the window closes.
//!
//! The two tests that install a standalone folder and close the window run on
//! Linux only, so off Linux their helpers and imports are unused.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use dc_app::config::{Dirs, Settings, Store};
use dc_app::{AppDeps, DentalCraftApp, Updates};
use dc_case::CaseState;
use dc_command::{Command, CommandError, JobCtx};
use dc_i18n::Lang;
use dc_project::{AppInfo, Project};
use dc_update::{Check, InstallKind, Installation, Manifest, Source};
use egui::{ViewportCommand, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use sha2::{Digest, Sha256};

const TEST_KEY: &str = include_str!("../../dc-update/tests/fixtures/test-key.pub");
const NEWER: &[u8] = include_bytes!("../../dc-update/tests/fixtures/newer.json");
const NEWER_SIG: &[u8] = include_bytes!("../../dc-update/tests/fixtures/newer.json.minisig");

/// The UI language is process-wide: tests that render take this lock.
static LANG_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LANG_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A tiny HTTP server on 127.0.0.1 answering `GET /<name>` from `files`
/// (404 otherwise); returns its base URL and the number of requests.
fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::Relaxed);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                header.clear();
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/");
            let mut out = stream;
            let _ = match files.get(path.trim_start_matches('/')) {
                Some(body) => out
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .and_then(|()| out.write_all(body)),
                None => out.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                ),
            };
        }
    });
    (base, hits)
}

/// The signed test manifests (version 999.0.0) on a local server.
fn update_site() -> (String, Arc<AtomicUsize>) {
    serve(HashMap::from([
        ("stable.json".to_owned(), NEWER.to_vec()),
        ("stable.json.minisig".to_owned(), NEWER_SIG.to_vec()),
    ]))
}

fn updates(dir: &Path, manifest_url: String) -> Updates {
    Updates {
        source: Source {
            manifest_url,
            public_key: TEST_KEY.lines().nth(1).unwrap().to_owned(),
        },
        work_dir: dir.join("work"),
        relaunch: false,
    }
}

fn harness(dir: &Path, updates: Option<Updates>) -> Harness<'static, DentalCraftApp> {
    let mut deps = AppDeps::headless(dir);
    deps.updates = updates;
    let app = DentalCraftApp::new(deps);
    dc_i18n::set_language(Lang::En).unwrap();
    Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(|ui, app: &mut DentalCraftApp| app.show(ui), app)
}

fn save_settings(dir: &Path, edit: impl FnOnce(&mut Settings)) {
    let store = Store::new(Dirs::under(dir));
    let mut s = store.load_settings();
    edit(&mut s);
    store.save_settings(&s);
}

/// Draws frames until `done`.
fn step_until(
    h: &mut Harness<'_, DentalCraftApp>,
    what: &str,
    done: impl Fn(&DentalCraftApp) -> bool,
) {
    let start = Instant::now();
    while !done(h.state()) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timed out: {what}"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A few frames, with time for the worker threads.
fn settle(h: &mut Harness<'_, DentalCraftApp>) {
    for _ in 0..20 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_startup_check_offers_a_newer_version() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (base, _) = update_site();
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), format!("{base}/stable.json"))),
    );
    step_until(&mut h, "the dialog", DentalCraftApp::update_dialog_open);
    h.run_steps(2);
    assert_eq!(h.state().update_available().as_deref(), Some("999.0.0"));
    assert!(
        h.query_by_label("A new version of DentalCraft is available")
            .is_some()
    );
    let line = format!(
        "DentalCraft 999.0.0 is available. You have {}.",
        env!("CARGO_PKG_VERSION")
    );
    assert!(h.query_by_label(&line).is_some(), "{line}");
    assert!(h.query_by_label("What's new").is_some());
    // the test program has no installation marker: the website is the way
    assert!(h.query_by_label("Open the download page").is_some());
    assert!(h.query_by_label("Update now").is_none());
    // Later closes it, the update stays known
    h.get_by_label("Later").click();
    h.run_steps(2);
    assert!(!h.state().update_dialog_open());
    assert_eq!(h.state().update_available().as_deref(), Some("999.0.0"));
}

#[test]
fn a_skipped_version_or_a_failure_stays_quiet_at_startup() {
    let _g = lock();
    // skipped: no dialog, "Check now" of the Settings dialog still shows it
    let dir = tempfile::tempdir().unwrap();
    save_settings(dir.path(), |s| s.skipped_update = Some("999.0.0".into()));
    let (base, _) = update_site();
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), format!("{base}/stable.json"))),
    );
    step_until(&mut h, "the check", |a| a.update_available().is_some());
    settle(&mut h);
    assert!(!h.state().update_dialog_open());
    h.get_by_label("Settings").click();
    h.run_steps(2);
    h.get_by_label("Check now").click();
    step_until(&mut h, "the dialog", DentalCraftApp::update_dialog_open);
    h.run_steps(2);
    assert!(
        h.query_by_label("A new version of DentalCraft is available")
            .is_some()
    );

    // offline at startup: nothing on screen; asked by the user: said plainly
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), "http://127.0.0.1:9/stable.json".into())),
    );
    settle(&mut h);
    assert!(!h.state().update_dialog_open());
    h.state_mut().check_for_updates();
    let deadline = Instant::now() + Duration::from_secs(30);
    while h.query_by_label("Could not check for updates").is_none() {
        assert!(Instant::now() < deadline, "no failure shown");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        h.query_by_label_contains("could not reach its website")
            .is_some()
    );
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().update_dialog_open());
}

#[test]
fn the_setting_turns_the_startup_check_off() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    save_settings(dir.path(), |s| s.check_updates = false);
    let (base, hits) = update_site();
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), format!("{base}/stable.json"))),
    );
    settle(&mut h);
    assert_eq!(hits.load(Ordering::Relaxed), 0, "no request at startup");
    assert_eq!(h.state().update_available(), None);
    // the Settings dialog turns it back on (saved at once)
    h.get_by_label("Settings").click();
    h.run_steps(2);
    h.get_by_label("Check for updates at startup").click();
    h.run_steps(2);
    let s = Store::new(Dirs::under(dir.path())).load_settings();
    assert!(s.check_updates);
}

#[test]
fn a_package_manager_installation_gets_the_command() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), None);
    h.run_steps(2);
    let manifest = Manifest::parse(NEWER).unwrap();
    h.state_mut().offer_update(Check::new(
        "0.1.0",
        manifest.clone(),
        Installation::new(InstallKind::Rpm, "/usr/bin"),
    ));
    h.run_steps(2);
    assert!(h.query_by_label("sudo dnf upgrade dentalcraft").is_some());
    assert!(h.query_by_label("Copy the command").is_some());
    assert!(h.query_by_label("Update now").is_none());
    h.get_by_label("Copy the command").click();
    h.run_steps(2);
    // Skip this version: saved, the dialog closes
    h.get_by_label("Skip this version").click();
    h.run_steps(2);
    assert!(!h.state().update_dialog_open());
    let s = Store::new(Dirs::under(dir.path())).load_settings();
    assert_eq!(s.skipped_update.as_deref(), Some("999.0.0"));
    // Flatpak and apt have their own words and command
    h.state_mut().offer_update(Check::new(
        "0.1.0",
        manifest.clone(),
        Installation::new(InstallKind::Flatpak, "/app/bin"),
    ));
    h.run_steps(2);
    assert!(
        h.query_by_label("flatpak update com.dentalcraft.DentalCraft")
            .is_some()
    );
    assert!(
        h.query_by_label_contains("installed with Flatpak")
            .is_some()
    );
    h.state_mut().offer_update(Check::new(
        "0.1.0",
        manifest,
        Installation::new(InstallKind::Deb, "/usr/bin"),
    ));
    h.run_steps(2);
    assert!(
        h.query_by_label("sudo apt update && sudo apt upgrade dentalcraft")
            .is_some()
    );
}

#[test]
fn the_newest_version_says_so() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), None);
    h.run_steps(2);
    h.state_mut().offer_update(Check::new(
        "999.0.0",
        Manifest::parse(NEWER).unwrap(),
        Installation::new(InstallKind::Standalone, dir.path()),
    ));
    h.run_steps(2);
    assert!(h.query_by_label("DentalCraft is up to date").is_some());
    assert!(
        h.query_by_label("You have the newest version, 999.0.0.")
            .is_some()
    );
}

// ---------------------------------------------------------------- install

/// `files` as a `.tar.gz` with the top folder `top/`.
fn tar_gz(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    for (name, body) in files {
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o755);
        h.set_size(body.len() as u64);
        tar.append_data(&mut h, format!("{top}/{name}"), body.as_bytes())
            .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// A standalone installation with the old program, and the check that
/// offers the new one from a local server.
fn standalone_offer(root: &Path) -> (PathBuf, Check) {
    let install = root.join("dentalcraft_0.1.0_linux_x86_64");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join(dc_update::APP_BIN), "old app").unwrap();
    std::fs::write(install.join("install-kind"), "standalone\n").unwrap();
    let archive = tar_gz(
        "dentalcraft_999.0.0_linux_x86_64",
        &[
            (dc_update::APP_BIN, "new app"),
            ("install-kind", "standalone\n"),
            ("README.md", "new readme"),
        ],
    );
    let name = "dentalcraft_999.0.0_linux_x86_64.tar.gz";
    let sha: String = Sha256::digest(&archive)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let size = archive.len();
    let (base, _) = serve(HashMap::from([(name.to_owned(), archive)]));
    let manifest = Manifest::parse(
        json!({
            "version": "999.0.0",
            "notes_url": "https://dentalcraft.app/en/download/",
            "assets": {"linux-standalone": {"url": format!("{base}/{name}"), "sha256": sha, "size": size}}
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
    let check = Check::new(
        "0.1.0",
        manifest,
        Installation::new(InstallKind::Standalone, &install),
    );
    (install, check)
}

/// A project file with the lower demo arch.
fn demo_project_file(dir: &Path) -> PathBuf {
    let state = dca_module::commands::find("aligners.demo")
        .unwrap()
        .execute(
            &CaseState::default(),
            &json!({ "arch": "lower" }),
            &JobCtx::detached(),
        )
        .unwrap()
        .0;
    let project = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    );
    let path = dir.join("update.dcprj");
    dc_project::save(&path, &project).unwrap();
    path
}

/// Renames the patient: a change of the case.
struct Rename(&'static str);

impl Command for Rename {
    fn id(&self) -> &'static str {
        "test.rename"
    }
    fn label_key(&self) -> &str {
        "test-rename"
    }
    fn apply(&self, state: &CaseState, _ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let mut next = state.clone();
        next.case.patient_code = self.0.into();
        Ok(next)
    }
}

fn change(h: &mut Harness<'_, DentalCraftApp>, code: &'static str) {
    h.state_mut()
        .workspace_mut()
        .unwrap()
        .run_command(Box::new(Rename(code)));
    step_until(h, "the change", |a| {
        a.workspace()
            .is_some_and(|w| w.state().case.patient_code == code)
    });
}

fn patient_code(path: &Path) -> String {
    let module = dca_module::AlignersModule::new();
    let loaded = dc_project::load(path, &dc_workspace::project_codecs(&module)).unwrap();
    loaded.project.case_state.case.patient_code
}

/// The user closes the window (its close button): one frame.
fn close_window(h: &mut Harness<'_, DentalCraftApp>) {
    h.input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(ViewportEvent::Close);
    h.step();
}

/// The app asked the window for `cmd` in the last frame.
fn asked(h: &Harness<'_, DentalCraftApp>, cmd: &ViewportCommand) -> bool {
    h.output()
        .viewport_output
        .get(&ViewportId::ROOT)
        .is_some_and(|v| v.commands.contains(cmd))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn update_now_then_restart_saves_the_project_and_installs() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (install, check) = standalone_offer(dir.path());
    let path = demo_project_file(dir.path());
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), "http://127.0.0.1:9/x".into())),
    );
    h.state_mut().open_project(path.clone());
    step_until(&mut h, "the project opens", |a| a.workspace().is_some());
    change(&mut h, "UPDATED");

    h.state_mut().offer_update(check);
    h.run_steps(2);
    assert!(
        h.query_by_label_contains("Your project is saved first")
            .is_some()
    );
    h.get_by_label("Update now").click();
    step_until(
        &mut h,
        "downloaded and checked",
        DentalCraftApp::update_ready,
    );
    h.run_steps(2);
    assert!(
        h.query_by_label("DentalCraft 999.0.0 is ready to install")
            .is_some()
    );
    // nothing changed yet
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "old app");

    h.get_by_label("Restart now").click();
    step_until(
        &mut h,
        "saved, the window may close",
        DentalCraftApp::may_close,
    );
    assert!(asked(&h, &ViewportCommand::Close));
    assert_eq!(
        patient_code(&path),
        "UPDATED",
        "the project was saved first"
    );
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "old app");
    // the window closes: the new version takes the place of the old one
    close_window(&mut h);
    assert!(!asked(&h, &ViewportCommand::CancelClose));
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "new app");
    assert_eq!(read(&install.join("README.md")), "new readme");
    assert!(!h.state().update_ready());
}

#[cfg(target_os = "linux")]
#[test]
fn later_installs_when_the_window_closes_and_staying_brings_the_dialog_back() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (install, check) = standalone_offer(dir.path());
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), "http://127.0.0.1:9/x".into())),
    );
    // a project never saved: leaving asks first
    assert!(h.state_mut().new_workspace("aligners"));
    h.run_steps(2);
    change(&mut h, "NEW");
    h.state_mut().offer_update(check);
    h.run_steps(2);
    h.get_by_label("Update now").click();
    step_until(
        &mut h,
        "downloaded and checked",
        DentalCraftApp::update_ready,
    );
    h.run_steps(2);
    h.get_by_label("Restart now").click();
    h.run_steps(3);
    // the workspace asks; the user stays: the update dialog comes back
    assert!(h.query_by_label("Don't save").is_some());
    h.get_by_label("Cancel").click();
    step_until(
        &mut h,
        "the dialog again",
        DentalCraftApp::update_dialog_open,
    );
    assert!(!h.state().may_close());
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "old app");
    // Later: nothing now, the new version goes in when the window closes
    h.run_steps(2);
    h.get_by_label("Later").click();
    h.run_steps(2);
    assert!(!h.state().update_dialog_open());
    assert!(h.state().update_ready());
    close_window(&mut h);
    assert!(asked(&h, &ViewportCommand::CancelClose), "it asks first");
    h.run_steps(2);
    h.get_by_label("Don't save").click();
    step_until(&mut h, "the window may close", DentalCraftApp::may_close);
    close_window(&mut h);
    assert!(!asked(&h, &ViewportCommand::CancelClose));
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "new app");
}

#[test]
fn a_damaged_download_is_refused_and_said_plainly() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (install, mut check) = standalone_offer(dir.path());
    if let dc_update::Action::Install { asset, .. } = &mut check.action {
        asset.sha256 = "0".repeat(64);
    } else {
        // not a system with standalone downloads
        return;
    }
    let mut h = harness(
        dir.path(),
        Some(updates(dir.path(), "http://127.0.0.1:9/x".into())),
    );
    h.run_steps(2);
    h.state_mut().offer_update(check);
    h.run_steps(2);
    h.get_by_label("Update now").click();
    let deadline = Instant::now() + Duration::from_secs(30);
    while h.query_by_label("The update was not installed").is_none() {
        assert!(Instant::now() < deadline, "no failure shown");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.query_by_label_contains("damaged or incomplete").is_some());
    assert!(h.query_by_label("Try again").is_some());
    assert!(!h.state().update_ready());
    assert_eq!(read(&install.join(dc_update::APP_BIN)), "old app");
}

/// Words the user is never shown: the names of the tools and checks behind
/// the update.
#[test]
fn the_update_words_are_plain_in_both_languages() {
    const JARGON: [&str; 14] = [
        "minisign",
        "sha256",
        "sha-256",
        "hash",
        "checksum",
        "signature",
        "firma",
        "manifest",
        "json",
        "ed25519",
        "msiexec",
        "staging",
        "http",
        "powershell",
    ];
    for lang in Lang::ALL {
        let ftl = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("i18n")
                .join(lang.code())
                .join("dc-app.ftl"),
        )
        .unwrap();
        let lines: Vec<&str> = ftl.lines().filter(|l| l.starts_with("update-")).collect();
        assert!(lines.len() > 20, "{lang}");
        for line in lines {
            let text = line.to_lowercase();
            for word in JARGON {
                assert!(!text.contains(word), "{lang}: {word:?} in {line:?}");
            }
        }
    }
}

// ---------------------------------------------------------------- bridge

/// One JSON-RPC connection to the app bridge.
struct Client {
    r: BufReader<std::net::TcpStream>,
    w: std::net::TcpStream,
    id: u64,
}

impl Client {
    /// Connects and proves the token of the bridge on `port`.
    fn connect(port: u16) -> Self {
        use dc_app::control::auth;
        let mut w = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut r = BufReader::new(w.try_clone().unwrap());
        let token = auth::read_token(&auth::token_file(&auth::token_dir().unwrap(), port)).unwrap();
        auth::client_handshake(&mut r, &mut w, &token, port).unwrap();
        Self { r, w, id: 0 }
    }

    /// The answer of `method`: `Ok(result)` or `Err(error)`.
    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, serde_json::Value> {
        self.id += 1;
        let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.w, "{req}").unwrap();
        let mut line = String::new();
        self.r.read_line(&mut line).unwrap();
        let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
        match resp.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(resp["result"].clone()),
        }
    }
}

#[test]
fn the_bridge_lists_checks_and_applies_updates() {
    use dc_app::control::{ControlServer, ControlledApp};
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    save_settings(dir.path(), |s| s.check_updates = false);
    let (base, _) = update_site();
    let mut deps = AppDeps::headless(dir.path());
    deps.updates = Some(updates(dir.path(), format!("{base}/stable.json")));
    let app = DentalCraftApp::new(deps);
    dc_i18n::set_language(Lang::En).unwrap();
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let list = c.call("list", json!({})).unwrap();
        let entry = list
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "app.update.check")
            .cloned()
            .unwrap();
        let described = c
            .call("describe", json!({"id": "app.update.apply"}))
            .unwrap();
        let check = c
            .call("execute", json!({"id": "app.update.check"}))
            .unwrap();
        let bad = c
            .call(
                "execute",
                json!({"id": "app.update.apply", "params": {"restart": "now"}}),
            )
            .unwrap_err();
        let apply = c
            .call("execute", json!({"id": "app.update.apply"}))
            .unwrap();
        (entry, described, check, bad, apply)
    });
    let start = Instant::now();
    while !client.is_finished() {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    let (entry, described, check, bad, apply) = client.join().unwrap();
    assert_eq!(entry["source"], "app");
    assert_eq!(entry["label"], "Check for updates");
    assert_eq!(
        described["params_schema"]["properties"]["restart"]["type"],
        "boolean"
    );
    assert_eq!(check["latest"], "999.0.0");
    assert_eq!(check["update_available"], true);
    // this test program has no installation marker: the website is the way
    assert_eq!(check["kind"], "unknown");
    assert_eq!(check["action"], "manual");
    assert_eq!(bad["code"], -32602);
    assert_eq!(apply["updated"], false);
    assert_eq!(apply["action"], "manual");
    // an agent's check does not put the dialog on the user's screen
    assert!(!h.state().bridge().app.update_dialog_open());
}
