//! The app bridge (`--control`) driven over TCP while a headless egui loop
//! (egui_kittest) draws the app: commands run through the workspace and
//! its undo stack, as from the window.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dc_app::control::{ControlServer, ControlledApp, PROTOCOL};
use dc_app::{AppDeps, DentalCraftApp, Screen};
use egui_kittest::Harness;
use serde_json::{Value, json};

struct Client {
    r: BufReader<TcpStream>,
    w: TcpStream,
    id: u64,
}

impl Client {
    /// Connects and proves the token of the bridge on `port` (its file is
    /// where the app writes it).
    fn connect(port: u16) -> Self {
        use dc_app::control::auth;
        let mut w = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut r = BufReader::new(w.try_clone().unwrap());
        let token = auth::read_token(&auth::token_file(&auth::token_dir().unwrap(), port)).unwrap();
        auth::client_handshake(&mut r, &mut w, &token, port).unwrap();
        Self { r, w, id: 0 }
    }

    fn raw(&mut self, line: &str) -> Value {
        writeln!(self.w, "{line}").unwrap();
        let mut resp = String::new();
        self.r.read_line(&mut resp).unwrap();
        serde_json::from_str(&resp).unwrap()
    }

    /// The answer of `method`: `Ok(result)` or `Err(error)`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.id += 1;
        let req = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let resp = self.raw(&req.to_string());
        assert_eq!(resp["id"], json!(self.id), "{resp}");
        match resp.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(resp["result"].clone()),
        }
    }

    fn scans(&mut self) -> usize {
        self.call("inspect", json!({"path": "scans"}))
            .unwrap()
            .as_array()
            .unwrap()
            .len()
    }
}

/// Draws frames until the client thread ends; returns its result.
fn drive<T>(h: &mut Harness<'_, ControlledApp>, client: JoinHandle<T>) -> T {
    let start = Instant::now();
    while !client.is_finished() {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "bridge test timed out"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    client.join().expect("client thread")
}

#[test]
fn bridge_runs_commands_through_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);
    let project = dir.path().join("bridge.dcprj");
    let project_s = project.to_string_lossy().into_owned();
    let dir_s = dir.path().to_string_lossy().into_owned();

    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let hello = c.call("hello", json!({})).unwrap();
        assert_eq!(hello["protocol"], PROTOCOL);

        let list = c.call("list", json!({})).unwrap();
        let ids: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"case.import") && ids.contains(&"aligners.demo"));
        let d = c.call("describe", json!({"id": "aligners.demo"})).unwrap();
        assert_eq!(d["source"], "aligners");

        // a case command opens a workspace and lands on its undo stack
        let out = c
            .call(
                "execute",
                json!({"id": "aligners.demo", "params": {"arch": "upper"}}),
            )
            .unwrap();
        assert!(out.is_object(), "{out}");
        assert_eq!(c.scans(), 1);
        assert_eq!(c.call("undo", json!({})).unwrap()["done"], true);
        assert_eq!(c.scans(), 0);
        assert_eq!(c.call("redo", json!({})).unwrap()["done"], true);
        assert_eq!(c.scans(), 1);

        // a click on the menu, by id or by the command of its inspector:
        // it selects, nothing runs
        assert!(ids.contains(&"app.menu.select"));
        let by_command = c
            .call(
                "execute",
                json!({"id": "app.menu.select", "params": {"id": "aligners.teeth.find"}}),
            )
            .unwrap();
        let leaf = by_command["selected"].as_str().unwrap().to_owned();
        let by_id = c
            .call(
                "execute",
                json!({"id": "app.menu.select", "params": {"id": leaf}}),
            )
            .unwrap();
        assert_eq!(by_id["selected"], by_command["selected"]);
        assert_eq!(c.scans(), 1);
        let none = c
            .call(
                "execute",
                json!({"id": "app.menu.select", "params": {"id": "no.such"}}),
            )
            .unwrap_err();
        assert_eq!(none["code"], -32602, "{none}");
        // the window as the user sees it: this harness draws no pixels
        assert!(ids.contains(&"app.window.screenshot"));
        let shot = format!("{dir_s}/window.png");
        let e = c
            .call(
                "execute",
                json!({"id": "app.window.screenshot", "params": {"path": shot}}),
            )
            .unwrap_err();
        assert!(e["message"].as_str().unwrap().contains("no image"), "{e}");
        let e = c
            .call("execute", json!({"id": "app.window.screenshot"}))
            .unwrap_err();
        assert_eq!(e["code"], -32602, "{e}");

        let outs = c
            .call(
                "batch",
                json!({"commands": [
                    {"id": "view.set", "params": {"view": "iso"}},
                    {"id": "aligners.demo", "params": {"arch": "lower"}}
                ]}),
            )
            .unwrap();
        assert_eq!(outs.as_array().unwrap().len(), 2);
        assert_eq!(c.scans(), 2);
        let failed = c
            .call(
                "batch",
                json!({"commands": [{"id": "edit.undo"}, {"id": "no.such"}]}),
            )
            .unwrap_err();
        assert!(
            failed["message"]
                .as_str()
                .unwrap()
                .contains("command 1 (no.such)"),
            "{failed}"
        );

        let img = c
            .call("render", json!({"width": 64, "height": 48}))
            .unwrap();
        assert_eq!(
            (img["width"].as_u64(), img["height"].as_u64()),
            (Some(64), Some(48))
        );
        // the render takes the model and the stage; a bad one is refused
        let img = c
            .call(
                "render",
                json!({"width": 16, "height": 12, "model": "scan", "stage": 2}),
            )
            .unwrap();
        assert_eq!(img["width"].as_u64(), Some(16));
        let bad = c.call("render", json!({"model": "gums"})).unwrap_err();
        assert!(bad["message"].as_str().unwrap().contains("model"), "{bad}");
        let shot = c.call("screenshot", json!({"size": "32x24"})).unwrap();
        assert!(shot["png"].as_str().unwrap().len() > 20);

        let saved = c.call("save", json!({"path": project_s})).unwrap();
        assert!(saved["bytes"].as_u64().unwrap() > 0, "{saved}");
        let summary = c.call("inspect", json!({})).unwrap();
        assert_eq!(summary["project"]["dirty"], false, "{summary}");

        let opened = c.call("open", json!({"path": project_s})).unwrap();
        assert_eq!(opened["scans"], 1, "{opened}");

        let e = c.call("execute", json!({"id": "no.such"})).unwrap_err();
        assert_eq!(e["code"], -32000);
        let e = c.call("execute", json!({"id": "steps.list"})).unwrap_err();
        assert!(e["message"].as_str().unwrap().contains("not available"));
        assert_eq!(c.call("nope", json!({})).unwrap_err()["code"], -32601);
        assert_eq!(c.raw("{not json")["error"]["code"], -32700);
        // a line that is not JSON-RPC also ends the connection
        let mut rest = String::new();
        assert_eq!(c.r.read_line(&mut rest).unwrap(), 0, "{rest}");
    });
    drive(&mut h, client);
    assert_eq!(h.state().bridge().app.screen(), Screen::Workspace);
    assert!(project.is_file());
}

/// A project opened read-only (its step history cannot be read): the bridge
/// never saves over its file, however the path is written, and saves it to
/// another file.
#[test]
fn bridge_never_saves_over_a_file_opened_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("read-only.dcprj");
    let mut p = dc_project::Project::new(
        dc_project::AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        dc_case::CaseState::new(dc_case::Case::new("RO")),
    );
    p.history = json!("not a step history");
    dc_project::save(&project, &p).unwrap();
    let before = std::fs::read(&project).unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);
    let project_s = project.to_string_lossy().into_owned();
    let spelled = dir.path().join("sub").join("..").join("read-only.dcprj");
    let spelled = spelled.to_string_lossy().into_owned();
    let other = dir.path().join("other.dcprj");
    let other_s = other.to_string_lossy().into_owned();

    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        c.call("open", json!({"path": project_s})).unwrap();
        let summary = c.call("inspect", json!({})).unwrap();
        assert_eq!(summary["project"]["read_only"], true, "{summary}");
        for path in [
            json!({}),
            json!({"path": project_s}),
            json!({"path": spelled}),
        ] {
            let e = c.call("save", path.clone()).unwrap_err();
            let m = e["message"].as_str().unwrap();
            assert!(m.contains("opened read-only"), "{path}: {e}");
        }
        let saved = c.call("save", json!({"path": other_s})).unwrap();
        assert!(saved["bytes"].as_u64().unwrap() > 0, "{saved}");
    });
    drive(&mut h, client);
    assert_eq!(std::fs::read(&project).unwrap(), before);
    assert!(other.is_file());
}

#[test]
fn bridge_runs_a_menu_leaf_as_the_button_of_its_inspector() {
    let dir = tempfile::tempdir().unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
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
        let demo = |c: &mut Client, arch: &str| {
            c.call(
                "execute",
                json!({"id": "aligners.demo", "params": {"arch": arch}}),
            )
            .unwrap();
        };
        demo(&mut c, "lower");
        // a leaf that fails answers with its error, as before (the case
        // analysis needs the 3D teeth, which the demo arch has not yet)
        let analysis = "aligners.diagnosis.space-analysis";
        let e = c.call("execute", json!({ "id": analysis })).unwrap_err();
        assert!(
            e["message"]
                .as_str()
                .unwrap()
                .starts_with(&format!("{analysis}: ")),
            "{e}"
        );
        // Bite with one arch is no failure: it does not apply, and says so
        let out = c
            .call("execute", json!({"id": "aligners.occlusion.align"}))
            .unwrap();
        assert_eq!(out["applies"], false, "{out}");
        demo(&mut c, "upper");
        let out = c
            .call(
                "execute",
                json!({"id": "aligners.preparation.segmentation.automatic",
                       "params": {"arch": "upper"}}),
            )
            .unwrap();
        assert!(out["teeth"].as_u64().is_some_and(|n| n > 0), "{out}");
        // the models are built at a coarser voxel than the default 0.25 mm:
        // the solid of one arch takes about 205 s at the default in a debug
        // build (about 20 s in release), 22 s at 0.5 mm, so the test does not
        // depend on the speed of heavy geometry in the profile CI uses; the
        // test checks the leaf and the bridge, not the detail of the solid
        let out = c
            .call(
                "execute",
                json!({"id": "aligners.output.study-models", "params": {"voxel_mm": 0.5}}),
            )
            .unwrap();
        assert!(
            out["kept"].as_array().is_some_and(|k| !k.is_empty()),
            "{out}"
        );
        assert_eq!(c.scans(), 2);

        // the view of the window: as after a click on their buttons, the new
        // tooth colours show the scan and the study models just built are
        // shown; the toolbar and the timeline change them
        let view = |c: &mut Client, p: Value| {
            c.call("execute", json!({"id": "app.window.view", "params": p}))
        };
        let now = view(&mut c, json!({})).unwrap();
        assert_eq!(
            (&now["teeth_shown"], &now["study_shown"], &now["view"]),
            (&json!(false), &json!(true), &json!("occlusal")),
            "{now}"
        );
        let set = view(&mut c, json!({"view": "front", "stage": 4, "study": false})).unwrap();
        assert_eq!(set["view"], "front", "{set}");
        assert_eq!(set["stage"], set["last_stage"], "{set}");
        assert_eq!(set["study_shown"], false, "{set}");
        // the Scan / 3D teeth choice of the toolbar
        for (model, teeth) in [("teeth", true), ("scan", false)] {
            let set = view(&mut c, json!({ "model": model })).unwrap();
            assert_eq!(
                (&set["model"], &set["teeth_shown"]),
                (&json!(model), &json!(teeth)),
                "{set}"
            );
        }
        // the four toggles of the toolbar: off when the project opens, set
        // one by one (the others stay), and the answer says them
        let four = |v: &Value| {
            [&v["colours"], &v["marks"], &v["numbers"], &v["contacts"]].map(Value::as_bool)
        };
        assert_eq!(four(&now), [Some(false); 4], "{now}");
        let set = view(&mut c, json!({"colours": true, "numbers": true})).unwrap();
        assert_eq!(
            four(&set),
            [Some(true), Some(false), Some(true), Some(false)],
            "{set}"
        );
        let set = view(&mut c, json!({"marks": true})).unwrap();
        assert_eq!(
            four(&set),
            [Some(true), Some(true), Some(true), Some(false)],
            "{set}"
        );
        let set = view(
            &mut c,
            json!({"colours": false, "marks": false, "numbers": false}),
        )
        .unwrap();
        assert_eq!(four(&set), [Some(false); 4], "{set}");
        for bad in [
            json!({"colours": "yes"}),
            json!({"view": "top"}),
            json!({"stage": -1}),
            json!({"zoom": 2}),
            json!({"model": "gums"}),
        ] {
            let e = view(&mut c, bad).unwrap_err();
            assert_eq!(e["code"], -32602, "{e}");
        }
    });
    drive(&mut h, client);
    let ws = h.state().bridge().app.workspace().unwrap();
    assert!(!ws.teeth_shown() && !ws.study_shown());
}

#[test]
fn bridge_reads_and_changes_the_settings_of_the_live_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);
    assert!(!dc_module_api::advanced_ui(&h.ctx));
    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let d = c
            .call("describe", json!({"id": "app.settings.set"}))
            .unwrap();
        assert_eq!(d["source"], "app");
        let s = c
            .call("execute", json!({"id": "app.settings.get"}))
            .unwrap();
        assert_eq!(
            (s["advanced"].clone(), s["theme"].clone()),
            (json!(false), json!("dark"))
        );
        let on = c
            .call(
                "execute",
                json!({"id": "app.settings.set", "params": {"advanced": true}}),
            )
            .unwrap();
        assert_eq!(on["advanced"], true);
        for bad in [
            json!({}),
            json!({"advanced": "on"}),
            json!({"theme": "light"}),
        ] {
            let e = c
                .call("execute", json!({"id": "app.settings.set", "params": bad}))
                .unwrap_err();
            assert_eq!(e["code"], -32602, "{e}");
        }
        let e = c
            .call(
                "execute",
                json!({"id": "app.settings.get", "params": {"x": 1}}),
            )
            .unwrap_err();
        assert_eq!(e["code"], -32602, "{e}");
    });
    drive(&mut h, client);
    h.run_steps(1);
    // the window follows at once, and the next start too
    assert!(h.state().bridge().app.advanced());
    assert!(dc_module_api::advanced_ui(&h.ctx));
    let saved = dc_app::config::Store::new(dc_app::config::Dirs::under(dir.path())).load_settings();
    assert!(saved.advanced);
}

#[test]
fn bridge_lists_and_runs_the_print_commands() {
    let dir = tempfile::tempdir().unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);
    let shells = dir.path().join("shells");
    let plates = dir.path().join("plates");
    std::fs::create_dir_all(&shells).unwrap();

    let client = std::thread::spawn(move || {
        let mut c = Client::connect(port);
        let list = c.call("list", json!({})).unwrap();
        for id in ["print.profiles", "print.prepare"] {
            let entry = list
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == id)
                .unwrap_or_else(|| panic!("{id} not listed"));
            assert_eq!(entry["source"], "session", "{entry}");
        }
        let d = c.call("describe", json!({"id": "print.prepare"})).unwrap();
        assert!(
            d["params_schema"]["properties"]["out_dir"].is_object(),
            "{d}"
        );
        let out = c.call("execute", json!({"id": "print.profiles"})).unwrap();
        assert_eq!(out["profiles"][0]["id"], "saturn-4-ultra-16k", "{out}");
        // the resin values are required: refused as a bad call, nothing written
        let e = c
            .call(
                "execute",
                json!({"id": "print.prepare", "params": {"dir": shells, "out_dir": plates}}),
            )
            .unwrap_err();
        assert_eq!(e["code"], -32602, "{e}");
        assert!(
            e["message"]
                .as_str()
                .unwrap()
                .contains("print-no-resin-preset"),
            "{e}"
        );
        assert!(!plates.exists());
    });
    drive(&mut h, client);
}
