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
    fn connect(port: u16) -> Self {
        let w = TcpStream::connect(("127.0.0.1", port)).unwrap();
        Self {
            r: BufReader::new(w.try_clone().unwrap()),
            w,
            id: 0,
        }
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
    });
    drive(&mut h, client);
    assert_eq!(h.state().bridge().app.screen(), Screen::Workspace);
    assert!(project.is_file());
}
