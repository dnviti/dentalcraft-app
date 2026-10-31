//! End to end: `dentalcraft-cli mcp --connect PORT` drives the app bridge
//! of a headless app (egui_kittest loop), so MCP tool calls run in the app.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dc_app::control::{ControlServer, ControlledApp};
use dc_app::{AppDeps, DentalCraftApp};
use egui_kittest::Harness;
use serde_json::{Value, json};

#[test]
fn mcp_connect_drives_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let app = DentalCraftApp::new(AppDeps::headless(dir.path()));
    let server = ControlServer::bind(0).unwrap();
    let port = server.local_addr().port().to_string();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(
            |ui, s: &mut ControlledApp| s.show(ui),
            ControlledApp::new(app, server),
        );
    h.run_steps(2);

    let mut child = Command::new(env!("CARGO_BIN_EXE_dentalcraft-cli"))
        .args(["mcp", "--connect", &port])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let agent = std::thread::spawn(move || {
        let mut call = |id: u64, name: &str, args: Value| -> Value {
            let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                             "params": {"name": name, "arguments": args}});
            writeln!(stdin, "{msg}").unwrap();
            stdin.flush().unwrap();
            let mut line = String::new();
            stdout.read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            v["result"].clone()
        };
        let r = call(
            1,
            "execute",
            json!({"id": "aligners.demo", "params": {"arch": "upper"}}),
        );
        assert_ne!(r["isError"], true, "{r}");
        let r = call(2, "inspect", json!({"path": "scans"}));
        assert_eq!(r["content"][0]["type"], "text");
        assert!(
            r["content"][0]["text"].as_str().unwrap().contains("upper"),
            "{r}"
        );
        let r = call(3, "undo", json!({}));
        assert_eq!(r["structuredContent"]["done"], true, "{r}");
        let r = call(4, "render", json!({"width": 48, "height": 32}));
        assert_eq!(r["content"][0]["type"], "image", "{r}");
        let r = call(5, "execute", json!({"id": "no.such"}));
        assert_eq!(r["isError"], true, "{r}");
        drop(stdin);
    });
    let start = Instant::now();
    while !agent.is_finished() {
        assert!(start.elapsed() < Duration::from_secs(120), "timed out");
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
    agent.join().unwrap();
    assert!(child.wait().unwrap().success());
    // the undo ran in the app: its workspace has no scan left
    let ws = h.state().bridge().app.workspace().unwrap();
    assert!(ws.state().case.scans.is_empty());
}
