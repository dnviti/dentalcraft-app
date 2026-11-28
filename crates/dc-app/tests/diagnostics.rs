//! Anonymous diagnostics in the app: the question at the first start (an
//! unticked box; Continue stores the answer and it is not asked again),
//! the switch of the Settings dialog, and the `app.diagnostics.*` commands
//! of the bridge, up to a report the server takes.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use dc_app::config::{Dirs, Settings, Store};
use dc_app::{AppDeps, DentalCraftApp};
use dc_diagnostics::Diagnostics;
use dc_diagnostics::settings::Paths;
use dc_i18n::Lang;
use egui::accesskit::Toggled;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

/// The UI language is process-wide: tests that render take this lock.
static LANG_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LANG_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

const CHECK: &str = "Send anonymous diagnostic data to help fix bugs";
const SETTING: &str = "Send anonymous diagnostic data";

fn save_settings(dir: &Path, edit: impl FnOnce(&mut Settings)) {
    let store = Store::new(Dirs::under(dir));
    let mut s = store.load_settings();
    edit(&mut s);
    store.save_settings(&s);
}

fn settings(dir: &Path) -> Settings {
    Store::new(Dirs::under(dir)).load_settings()
}

/// Headless dependencies with diagnostics under `dir`, reporting to
/// `issuer`.
fn deps(dir: &Path, issuer: &str) -> (AppDeps, Diagnostics) {
    let mut deps = AppDeps::headless(dir);
    let paths = Paths {
        config: dir.join("config"),
        data: dir.join("data"),
    };
    let d = Diagnostics::new(
        dc_diagnostics::Config::standard(&paths, issuer),
        &deps.store.load_settings().consent(),
    );
    deps.diagnostics = Some(d.clone());
    (deps, d)
}

fn harness(deps: AppDeps) -> Harness<'static, DentalCraftApp> {
    let app = DentalCraftApp::new(deps);
    dc_i18n::set_language(Lang::En).unwrap();
    Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(|ui, app: &mut DentalCraftApp| app.show(ui), app)
}

#[test]
fn the_first_start_asks_with_an_unticked_box_and_continue_stores_no() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (deps, d) = deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    assert!(h.state().consent_dialog_open());
    assert_eq!(h.state().diagnostics_enabled(), None);
    let check = h.get_by_label(CHECK);
    assert_eq!(check.accesskit_node().toggled(), Some(Toggled::False));
    // what is sent and what never is
    assert!(h.query_by_label("What is never sent").is_some());
    assert!(
        h.query_by_label("Scans, projects or patient data")
            .is_some()
    );
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
    assert!(h.query_by_label(CHECK).is_none());
    assert_eq!(h.state().diagnostics_enabled(), Some(false));
    let s = settings(dir.path());
    assert_eq!((s.diagnostics, s.install_id), (Some(false), None));
    // nothing was collected
    assert_eq!(d.pending(), 0);
    // the next start does not ask again
    let (deps, _) = self::deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
}

#[test]
fn ticking_the_box_turns_diagnostics_on_and_records_the_start() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (deps, d) = deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    h.get_by_label(CHECK).click();
    h.run_steps(2);
    assert_eq!(
        h.get_by_label(CHECK).accesskit_node().toggled(),
        Some(Toggled::True)
    );
    h.get_by_label("Continue").click();
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
    let s = settings(dir.path());
    assert_eq!(s.diagnostics, Some(true));
    assert_eq!(s.install_id, d.install_id());
    assert!(s.install_id.is_some());
    // the session event of this start
    let q = d.queued();
    assert!(q.iter().any(|e| e.code == "start"), "{q:?}");
}

#[test]
fn closing_the_question_unanswered_asks_again_next_time() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let (deps, _) = deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
    assert_eq!(settings(dir.path()).diagnostics, None);
    // the shell is usable meanwhile
    assert_eq!(h.state().screen(), dc_app::Screen::Home);
    let (deps, _) = self::deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    assert!(h.state().consent_dialog_open());
}

#[test]
fn without_diagnostics_in_the_app_nothing_is_asked() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(AppDeps::headless(dir.path()));
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
    assert!(h.query_by_label(CHECK).is_none());
}

#[test]
fn the_settings_switch_toggles_diagnostics() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    save_settings(dir.path(), |s| s.diagnostics = Some(false));
    let (deps, d) = deps(dir.path(), "http://127.0.0.1:9");
    let mut h = harness(deps);
    h.run_steps(3);
    assert!(!h.state().consent_dialog_open());
    // the gear of the top bar opens the Settings dialog
    assert!(h.query_by_label(SETTING).is_none());
    h.get_by_label("Settings").click();
    h.run_steps(3);
    assert!(h.state().settings_dialog_open());
    assert_eq!(
        h.get_by_label(SETTING).accesskit_node().toggled(),
        Some(Toggled::False)
    );
    h.get_by_label(SETTING).click();
    h.run_steps(3);
    assert_eq!(h.state().diagnostics_enabled(), Some(true));
    let id = settings(dir.path()).install_id.unwrap();
    assert_eq!(d.install_id().as_deref(), Some(id.as_str()));
    assert!(d.pending() > 0);
    // the dialog stays open
    assert_eq!(
        h.get_by_label(SETTING).accesskit_node().toggled(),
        Some(Toggled::True)
    );
    h.get_by_label(SETTING).click();
    h.run_steps(3);
    assert_eq!(h.state().diagnostics_enabled(), Some(false));
    let s = settings(dir.path());
    assert_eq!((s.diagnostics, s.install_id), (Some(false), None));
    // turning off deletes what waited
    assert_eq!(d.pending(), 0);
    // Esc closes it
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().settings_dialog_open());
    assert_eq!(h.state().screen(), dc_app::Screen::Home);
}

/// The requests a test server received: head and body.
type Received = Arc<Mutex<Vec<(String, String)>>>;

/// A loopback server answering every `POST` with `202`; returns its base
/// URL and the requests it received.
fn reports_server() -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let got = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&got);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut head, mut length) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let n = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v["events"].as_array().map(Vec::len))
                .unwrap_or(0);
            keep.lock()
                .unwrap()
                .push((head, String::from_utf8_lossy(&body).into_owned()));
            let reply = format!(r#"{{"accepted":{n},"duplicates":0}}"#);
            let _ = write!(
                stream,
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (base, got)
}

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

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.id += 1;
        let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.w, "{req}").unwrap();
        let mut line = String::new();
        self.r.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        match resp.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(resp["result"].clone()),
        }
    }
}

#[test]
fn the_bridge_cannot_turn_diagnostics_on_but_shows_sends_and_turns_them_off() {
    use dc_app::control::{ControlServer, ControlledApp};
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    save_settings(dir.path(), |s| s.diagnostics = Some(false));
    let (base, got) = reports_server();
    let (deps, _) = deps(dir.path(), &base);
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
    fn run<T>(h: &mut Harness<'_, ControlledApp>, client: std::thread::JoinHandle<T>) -> T {
        let start = Instant::now();
        while !client.is_finished() {
            assert!(start.elapsed() < Duration::from_secs(60), "timed out");
            h.step();
            std::thread::sleep(Duration::from_millis(2));
        }
        client.join().unwrap()
    }
    let status = json!({"id": "app.diagnostics.status"});
    let set = |on: Value| json!({"id": "app.diagnostics.set", "params": {"enabled": on}});
    let (s, set_) = (status.clone(), set);
    let (ids, before, bad, refused, after) = run(
        &mut h,
        std::thread::spawn(move || {
            let mut c = Client::connect(port);
            let list = c.call("list", json!({})).unwrap();
            let ids: Vec<String> = list
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["source"] == "app")
                .map(|e| e["id"].as_str().unwrap().to_owned())
                .collect();
            let before = c.call("execute", s.clone()).unwrap();
            let bad = c.call("execute", set_(json!("yes"))).unwrap_err();
            // only the user gives consent, in the app
            let refused = c.call("execute", set_(json!(true))).unwrap_err();
            let after = c.call("execute", s).unwrap();
            (ids, before, bad, refused, after)
        }),
    );
    for id in [
        "app.diagnostics.status",
        "app.diagnostics.set",
        "app.diagnostics.preview",
        "app.diagnostics.send",
    ] {
        assert!(ids.iter().any(|i| i == id), "{id} not listed: {ids:?}");
    }
    assert_eq!(before["enabled"], false);
    assert_eq!(bad["code"], -32602);
    // the bridge names the command first, as for every parameter error
    assert_eq!(refused["code"], -32602);
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .starts_with("app.diagnostics.set: consent-only-in-app:"),
        "{refused}"
    );
    assert_eq!(after["enabled"], false);
    assert_eq!(h.state().bridge().app.diagnostics_enabled(), Some(false));
    // the user ticks the box in Settings
    h.state_mut().bridge_mut().app.set_diagnostics(true);
    let (on, preview, sent, off) = run(
        &mut h,
        std::thread::spawn(move || {
            let mut c = Client::connect(port);
            let on = c.call("execute", status).unwrap();
            let preview = c
                .call("execute", json!({"id": "app.diagnostics.preview"}))
                .unwrap();
            let sent = c
                .call("execute", json!({"id": "app.diagnostics.send"}))
                .unwrap();
            // an agent may turn it off
            let off = c.call("execute", set(json!(false))).unwrap();
            (on, preview, sent, off)
        }),
    );
    assert_eq!(on["enabled"], true);
    assert!(on["install_id"].is_string());
    assert_eq!(
        (off["enabled"].clone(), off["install_id"].clone()),
        (json!(false), Value::Null)
    );
    // the account menu follows
    assert_eq!(h.state().bridge().app.diagnostics_enabled(), Some(false));
    // the start of this run waited to be sent, with the machine
    let report = &preview["report"];
    assert_eq!(report["install_id"], on["install_id"]);
    assert_eq!(report["events"][0]["kind"], "session");
    assert!(report["system"]["os"].is_string());
    assert!(sent["sent"].as_u64().unwrap() >= 1, "{sent}");
    assert_eq!(sent["remaining"], 0);
    let got = got.lock().unwrap();
    assert!(!got.is_empty());
    let (head, body) = &got[0];
    assert!(
        head.to_ascii_lowercase()
            .starts_with("post /api/v1/diagnostics/reports"),
        "{head}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("authorization"),
        "{head}"
    );
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["schema"], 1);
}
