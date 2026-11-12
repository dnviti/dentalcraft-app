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

// ---- scan import: repeated --in, --role, the import result ----

/// Writes an ASCII STL box of `size` mm at x = `x` named `name` in `dir`
/// (a neutral scan: no arch in the name, no dental geometry).
fn box_stl(dir: &Path, name: &str, x: f32, size: f32) -> String {
    let c = |dx: f32, dy: f32, dz: f32| [x + dx * size, dy * size, dz * size];
    let corners = [
        c(0., 0., 0.),
        c(1., 0., 0.),
        c(1., 1., 0.),
        c(0., 1., 0.),
        c(0., 0., 1.),
        c(1., 0., 1.),
        c(1., 1., 1.),
        c(0., 1., 1.),
    ];
    let tris = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    let mut text = String::from("solid box\n");
    for t in tris {
        text.push_str("facet normal 0 0 0\nouter loop\n");
        for i in t {
            let [vx, vy, vz] = corners[i];
            text.push_str(&format!("vertex {vx} {vy} {vz}\n"));
        }
        text.push_str("endloop\nendfacet\n");
    }
    text.push_str("endsolid box\n");
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// Writes the demo arch (`upper` or `lower`, synthetic data) as an STL
/// file with a neutral name.
fn demo_arch_stl(dir: &Path, arch: &str, name: &str) -> String {
    dca_module::loader();
    let mut s = dc_session::Session::new();
    s.register_module(Box::new(dca_module::AlignersModule::new()))
        .unwrap();
    s.execute("aligners.demo", json!({ "arch": arch })).unwrap();
    let role = if arch == "upper" {
        dc_case::ScanRole::Upper
    } else {
        dc_case::ScanRole::Lower
    };
    let state = s.state().clone();
    let scan = state.case.scan(role).unwrap();
    let mesh = state.blobs.require(&scan.mesh).unwrap();
    let path = dir.join(name);
    dc_mesh::io::write(&path, mesh, dc_mesh::io::MeshFormat::Stl).unwrap();
    path.to_string_lossy().into_owned()
}

fn roles_of(import: &Value) -> Vec<(String, String)> {
    import["imported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["source_name"].as_str().unwrap().to_owned(),
                s["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn two_neutral_files_in_one_import_get_their_arch_from_the_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let a = demo_arch_stl(dir.path(), "upper", "scan_a.stl");
    let b = demo_arch_stl(dir.path(), "lower", "scan_b.stl");
    // listed lower first: the geometry decides, not the order
    let v = ok_json(&["run", "--in", &b, "--in", &a, "--print"]);
    let import = &v["import"];
    assert_eq!(
        roles_of(import),
        [pair("scan_b.stl", "lower"), pair("scan_a.stl", "upper")]
    );
    assert!(import["failed"].as_array().unwrap().is_empty(), "{import}");
    assert!(
        import["warnings"].as_array().unwrap().is_empty(),
        "{import}"
    );
    let detections = import["detections"].as_array().unwrap();
    assert_eq!(detections.len(), 2, "{import}");
    assert_eq!(detections[0]["arch"], "lower");
    assert_eq!(detections[1]["arch"], "upper");
    for s in import["imported"].as_array().unwrap() {
        assert_eq!(s["role_source"], "detected", "{s}");
    }
    // both scans are in the case
    assert_eq!(v["case"]["scans"].as_array().unwrap().len(), 2);
}

#[test]
fn neutral_boxes_are_provisional_and_the_output_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "part_1.stl", 0.0, 10.0);
    let b = box_stl(dir.path(), "part_2.stl", 30.0, 10.0);
    let v = ok_json(&["run", "--in", &a, "--in", &b]);
    let import = &v["import"];
    let imported = import["imported"].as_array().unwrap();
    assert_eq!(imported.len(), 2, "{import}");
    // not an arch by its geometry: each takes a free arch, marked provisional
    for s in imported {
        assert_eq!(s["role_source"], "provisional", "{s}");
    }
    let roles: Vec<&str> = imported
        .iter()
        .map(|s| s["role"].as_str().unwrap())
        .collect();
    assert_ne!(roles[0], roles[1], "{roles:?}");
    // and the provisional arch is not hidden: a warning names the file
    let warnings = import["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "{import}");
    assert!(
        warnings
            .iter()
            .any(|w| w.to_string().contains("part_1.stl")),
        "{warnings:?}"
    );
}

#[test]
fn a_single_in_is_still_one_scan_with_the_import_in_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "upper.stl", 0.0, 10.0);
    let v = ok_json(&["run", "--in", &a, "--cmd", "edit.undo"]);
    assert_eq!(roles_of(&v["import"]), [pair("upper.stl", "upper")]);
    assert_eq!(v["import"]["role_source"], "file_name");
    // the command steps still run after the import
    assert_eq!(v["results"][0]["id"], "edit.undo");
}

#[test]
fn role_once_applies_to_every_scan_and_per_file_maps_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "part_1.stl", 0.0, 10.0);
    let b = box_stl(dir.path(), "part_2.stl", 30.0, 10.0);
    // once, for all: two bite scans (a case holds up to 2)
    let v = ok_json(&["run", "--in", &a, "--in", &b, "--role", "bite"]);
    assert_eq!(
        roles_of(&v["import"]),
        [pair("part_1.stl", "bite"), pair("part_2.stl", "bite")]
    );
    // once per file, in --in order
    let v = ok_json(&[
        "run", "--in", &a, "--in", &b, "--role", "lower", "--role", "upper",
    ]);
    assert_eq!(
        roles_of(&v["import"]),
        [pair("part_1.stl", "lower"), pair("part_2.stl", "upper")]
    );
    for s in v["import"]["imported"].as_array().unwrap() {
        assert_eq!(s["role_source"], "explicit", "{s}");
    }
    // `auto` per file lets the import decide for that file
    let v = ok_json(&[
        "run", "--in", &a, "--in", &b, "--role", "upper", "--role", "auto",
    ]);
    let imported = v["import"]["imported"].as_array().unwrap();
    assert_eq!(imported[0]["role"], "upper");
    assert_eq!(imported[0]["role_source"], "explicit");
    assert_eq!(imported[1]["role"], "lower");
    assert_eq!(imported[1]["role_source"], "provisional");
}

#[test]
fn role_works_with_info_and_render() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "part_1.stl", 0.0, 10.0);
    let b = box_stl(dir.path(), "part_2.stl", 30.0, 10.0);
    let info = ok_json(&[
        "info", "--in", &a, "--in", &b, "--role", "lower", "--role", "upper",
    ]);
    assert_eq!(
        roles_of(&info["import"]),
        [pair("part_1.stl", "lower"), pair("part_2.stl", "upper")]
    );
    assert_eq!(info["scans"].as_array().unwrap().len(), 2);
    let png = dir.path().join("out.png");
    let png_s = png.to_string_lossy().into_owned();
    let v = ok_json(&[
        "render", "--in", &a, "--in", &b, "--size", "32x24", "--out", &png_s,
    ]);
    assert_eq!(png_size(&png), (32, 24));
    assert_eq!(v["import"]["imported"].as_array().unwrap().len(), 2);
}

#[test]
fn a_wrong_number_of_roles_or_a_bad_role_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "part_1.stl", 0.0, 10.0);
    let b = box_stl(dir.path(), "part_2.stl", 30.0, 10.0);
    let c = box_stl(dir.path(), "part_3.stl", 60.0, 10.0);
    // 2 roles for 3 files
    let out = cli(&[
        "run", "--in", &a, "--in", &b, "--in", &c, "--role", "upper", "--role", "lower",
    ]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(
        err.contains("--role was given 2 times for 3 scan file(s)"),
        "{err}"
    );
    // 3 roles for 2 files
    let out = cli(&[
        "info", "--in", &a, "--in", &b, "--role", "upper", "--role", "lower", "--role", "bite",
    ]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("--role was given 3 times"));
    // a role without a scan
    let out = cli(&["run", "--role", "upper"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("--role needs a scan file"));
    // not a role
    let out = cli(&["run", "--in", &a, "--role", "molar"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("molar"));
}

#[test]
fn a_project_is_only_allowed_as_the_first_in() {
    let dir = tempfile::tempdir().unwrap();
    let a = box_stl(dir.path(), "upper.stl", 0.0, 10.0);
    let project = dir.path().join("case.dcprj");
    let project_s = project.to_string_lossy().into_owned();
    ok_json(&["run", "--in", &a, "--save", &project_s]);
    // after a scan: refused, with the file named
    let out = cli(&["run", "--in", &a, "--in", &project_s]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(
        err.contains("case.dcprj") && err.contains("only allowed as the first --in"),
        "{err}"
    );
    // two projects: the second is refused
    let out = cli(&["info", "--in", &project_s, "--in", &project_s]);
    assert!(!out.status.success());
    // first: opened, and the scans after it are imported into the case
    let lower = box_stl(dir.path(), "lower.stl", 30.0, 10.0);
    let v = ok_json(&["run", "--in", &project_s, "--in", &lower, "--print"]);
    assert_eq!(roles_of(&v["import"]), [pair("lower.stl", "lower")]);
    assert_eq!(v["case"]["scans"].as_array().unwrap().len(), 2);
    // a project alone has no import result
    let v = ok_json(&["run", "--in", &project_s]);
    assert!(v.get("import").is_none(), "{v}");
}

#[test]
fn a_failed_file_is_listed_and_the_exit_stays_zero() {
    let dir = tempfile::tempdir().unwrap();
    let good = box_stl(dir.path(), "part_1.stl", 0.0, 10.0);
    let bad = dir.path().join("broken.stl");
    std::fs::write(&bad, b"this is not a mesh").unwrap();
    let missing = dir.path().join("missing.stl");
    let v = ok_json(&[
        "run",
        "--in",
        &good,
        "--in",
        &bad.to_string_lossy(),
        "--in",
        &missing.to_string_lossy(),
    ]);
    let import = &v["import"];
    assert_eq!(roles_of(import).len(), 1, "{import}");
    let failed = import["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 2, "{import}");
    let names: Vec<String> = failed
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.iter().any(|p| p.ends_with("broken.stl")), "{names:?}");
    assert!(
        names.iter().any(|p| p.ends_with("missing.stl")),
        "{names:?}"
    );
    for f in failed {
        assert!(f["code"].is_string() && f["reason"].is_string(), "{f}");
    }
}

#[test]
fn nothing_imported_is_an_error_that_names_the_files() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("broken.stl");
    std::fs::write(&bad, b"this is not a mesh").unwrap();
    let missing = dir.path().join("missing.stl");
    let out = cli(&[
        "run",
        "--in",
        &bad.to_string_lossy(),
        "--in",
        &missing.to_string_lossy(),
    ]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.starts_with("error: --in: "), "{err}");
    assert!(
        err.contains("broken.stl") && err.contains("missing.stl"),
        "{err}"
    );
}

#[test]
fn help_explains_in_role_and_the_import_output() {
    for sub in ["run", "render", "info"] {
        let out = cli(&[sub, "--help"]);
        let help = String::from_utf8(out.stdout).unwrap();
        for needle in [
            "repeatable",
            "--role auto|upper|lower|bite",
            "once per scan file",
            "\"import\"",
            "\"failed\"",
            "non-zero only when no scan could be imported",
        ] {
            assert!(help.contains(needle), "{sub}: {needle}\n{help}");
        }
    }
}

#[test]
fn licence_and_third_party_notices() {
    let out = cli(&["licence"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("Copyright (c) 2026 Daniele Viti. All rights reserved."),
        "{text}"
    );
    assert!(text.contains("End-User Licence Agreement"));
    let out = cli(&["licence", "--third-party"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().any(|l| l.starts_with("| egui |")));
    let out = cli(&["--version"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Copyright (c) 2026 Daniele Viti."), "{text}");
}
