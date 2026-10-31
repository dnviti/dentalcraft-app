//! Smoke tests of the `dentalcraft-cli` binary.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dentalcraft-cli"))
        .args(args)
        .output()
        .expect("run dentalcraft-cli")
}

fn ok_json(args: &[&str]) -> Value {
    let out = cli(args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("JSON on stdout")
}

/// Width and height from the IHDR chunk of a PNG file.
fn png_size(path: &Path) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n");
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (be(16), be(20))
}

#[test]
fn commands_lists_builtins_and_module_commands() {
    let out = cli(&["commands"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().any(|l| l.starts_with("project.new")), "{text}");
    assert!(text.lines().any(|l| l.starts_with("aligners.")), "{text}");
    let list = ok_json(&["commands", "--json"]);
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"case.import"));
}

#[test]
fn describe_and_info() {
    let d = ok_json(&["describe", "case.import", "--json"]);
    assert_eq!(d["id"], "case.import");
    assert_eq!(d["params_schema"]["type"], "object");
    let info = ok_json(&["info"]);
    assert_eq!(info["modules"][0]["id"], "aligners");
    let bad = cli(&["describe", "no.such"]);
    assert!(!bad.status.success());
}

#[test]
fn run_prints_results_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("case.dcprj");
    let out_s = out_path.to_string_lossy().into_owned();
    let v = ok_json(&[
        "run",
        "--cmd",
        "project.new={}",
        "--cmd",
        r#"project.new={"patient_code":"P-7"}"#,
        "--save",
        &out_s,
        "--print",
    ]);
    assert_eq!(v["results"][0]["id"], "project.new");
    assert_eq!(v["results"][1]["output"]["patient_code"], "P-7");
    assert_eq!(v["case"]["case"]["patient_code"], "P-7");
    assert!(v["saved"]["bytes"].as_u64().unwrap() > 0);
    // the saved project opens again
    let info = ok_json(&["info", "--in", &out_s]);
    assert_eq!(info["case"]["patient_code"], "P-7");
}

#[test]
fn run_reads_command_files_and_names_failures() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("cmds.jsonl");
    std::fs::write(
        &f,
        "# a demo case\n{\"id\":\"project.new\",\"params\":{\"patient_code\":\"F\"}}\nview.set={\"view\":\"iso\"}\n",
    )
    .unwrap();
    let f = f.to_string_lossy().into_owned();
    let v = ok_json(&["run", "--cmd-file", &f, "--cmd", "edit.undo", "--print"]);
    let ids: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["project.new", "view.set", "edit.undo"]);
    let bad = cli(&["run", "--cmd", "project.new", "--cmd", "no.such={}"]);
    assert!(!bad.status.success());
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("--cmd 2 (no.such)"), "{err}");
}

#[test]
fn render_writes_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("demo.png");
    let png_s = png.to_string_lossy().into_owned();
    let v = ok_json(&[
        "render",
        "--cmd",
        r#"aligners.demo={"arch":"upper"}"#,
        "--view",
        "iso",
        "--size",
        "96x64",
        "--out",
        &png_s,
    ]);
    assert_eq!(v["width"], 96);
    assert_eq!(png_size(&png), (96, 64));
    // an empty case renders the background; a bad view is refused
    ok_json(&["render", "--size", "8x8", "--out", &png_s]);
    assert_eq!(png_size(&png), (8, 8));
    assert!(
        !cli(&["render", "--view", "sideways", "--out", &png_s])
            .status
            .success()
    );
}

#[test]
fn mcp_stdio_round_trip() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dentalcraft-cli"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: Value| {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = || -> Value {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    }));
    let init = recv();
    assert_eq!(init["result"]["serverInfo"]["name"], "dentalcraft");
    // a notification gets no answer: the next line answers tools/list
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = recv();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"execute") && names.contains(&"render"),
        "{names:?}"
    );
    send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "execute", "arguments": {"id": "aligners.demo", "params": {"arch": "lower"}}}
    }));
    let demo = recv();
    assert_ne!(demo["result"]["isError"], true, "{demo}");
    send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "render", "arguments": {"width": 64, "height": 48, "preset": "front"}}
    }));
    let img = recv();
    let content = &img["result"]["content"][0];
    assert_eq!(content["type"], "image", "{img}");
    assert_eq!(content["mimeType"], "image/png");
    drop(send);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
