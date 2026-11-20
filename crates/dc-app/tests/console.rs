//! The `dentalcraft` binary when its standard output or error cannot be
//! written (a wrapper that redirected them and went away, no console): it
//! must never panic over it (seen on Windows: "failed printing to stdout:
//! pipe closing (os error 232)" at the bridge's listening line).

use std::process::Command;

/// A pipe whose reader is gone: every write to it fails (`EPIPE`;
/// `ERROR_NO_DATA`, os error 232, on Windows).
fn closed_pipe() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer
}

#[test]
fn a_bad_argument_with_a_closed_stderr_is_a_plain_failure() {
    let status = Command::new(env!("CARGO_BIN_EXE_dentalcraft"))
        .args(["--control", "not-a-port"])
        .stdout(closed_pipe())
        .stderr(closed_pipe())
        .status()
        .unwrap();
    // a panic exits with 101
    assert_eq!(status.code(), Some(2));
}

/// The app up to its window, with the bridge's listening line and the log
/// lines written to closed pipes. Without a display the window cannot
/// open, so the run ends there with an error of its own, not a panic.
#[cfg(target_os = "linux")]
#[test]
fn the_listening_line_to_a_closed_stdout_does_not_stop_the_app() {
    use std::process::Stdio;
    let home = tempfile::tempdir().unwrap();
    let run = |stderr: Stdio| {
        Command::new(env!("CARGO_BIN_EXE_dentalcraft"))
            .args(["--control", "0"])
            // a private home, no display, no session bus
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(closed_pipe())
            .stderr(stderr)
            .output()
            .unwrap()
    };
    let out = run(Stdio::piped());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.contains("control bridge listening on"), "{stderr}");
    assert_ne!(out.status.code(), Some(101), "{stderr}");
    // stderr closed too: the log lines are lost, nothing else
    let out = run(Stdio::from(closed_pipe()));
    assert_ne!(out.status.code(), Some(101));
    assert!(out.status.code().is_some());
}
