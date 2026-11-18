//! Crash reports: the panic hook's text, and the crash files earlier runs
//! left in the log folder.
//!
//! The app's panic hook writes `crash-<unix time>.log`:
//!
//! ```text
//! DentalCraft 0.2.0 crashed
//! thread: main
//! panicked at crates/dc-x/src/y.rs:1:2:
//! the message
//!
//! backtrace:
//! …
//! ```
//!
//! A report that went into the queue is renamed `crash-<unix
//! time>.reported.log` (kept for the user, never deleted), so it is not
//! picked up twice. Files older than the consent are left alone.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::{Kind, NewEvent, Source};

/// Code of every crash event.
pub const CRASH_CODE: &str = "panic";

/// What the panic hook knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    /// Version that crashed.
    pub version: String,
    /// Name of the thread that panicked.
    pub thread: String,
    /// The panic: location and message.
    pub message: String,
    /// The call stack.
    pub backtrace: String,
}

impl Crash {
    /// Reads a crash file's text.
    pub fn parse(text: &str) -> Option<Crash> {
        let mut lines = text.lines();
        let first = lines.next()?;
        let version = first
            .strip_prefix("DentalCraft ")?
            .strip_suffix(" crashed")?
            .to_owned();
        let thread = lines.next()?.strip_prefix("thread: ")?.to_owned();
        let rest: String = lines.collect::<Vec<_>>().join("\n");
        let (message, backtrace) = match rest.split_once("\n\nbacktrace:\n") {
            Some((m, b)) => (m.to_owned(), b.to_owned()),
            None => (rest, String::new()),
        };
        Some(Crash {
            version,
            thread,
            message: message.trim().to_owned(),
            backtrace,
        })
    }

    /// The crash event (the caller adds the log lines).
    pub fn event(&self, picked_up: bool) -> NewEvent {
        let mut e = NewEvent::new(
            Kind::Crash,
            source_of(&self.message),
            CRASH_CODE,
            self.message.clone(),
        )
        .context(json!({
            "thread": self.thread,
            "crashed_version": self.version,
            "picked_up": picked_up,
        }));
        e.backtrace = (!self.backtrace.trim().is_empty()).then(|| self.backtrace.clone());
        if e.source == Source::Command || self.message.contains("dca-") {
            e.module = Some("aligners".into());
        }
        e
    }
}

/// The source of a panic by the crate of its location (`panicked at
/// crates/dc-workspace/src/…`), else the app.
fn source_of(message: &str) -> Source {
    let loc = message
        .split_once("panicked at ")
        .map_or("", |(_, l)| l)
        .replace('\\', "/");
    let krate = loc
        .split('/')
        .skip_while(|p| *p != "crates")
        .nth(1)
        .or_else(|| {
            loc.split('/')
                .find(|p| p.starts_with("dc") && p.contains('-'))
        })
        .unwrap_or_default();
    match Source::from_target(krate) {
        Source::Other => Source::App,
        s => s,
    }
}

/// The unix time of a crash file not reported yet (`crash-<unix>.log`).
pub(crate) fn crash_time(name: &str) -> Option<u64> {
    name.strip_prefix("crash-")?
        .strip_suffix(".log")?
        .parse()
        .ok()
}

/// The crash files of `log_dir` from `since` on, oldest first.
pub(crate) fn unreported(log_dir: &Path, since: u64) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let t = crash_time(&e.file_name().to_string_lossy())?;
            (t >= since).then(|| (t, e.path()))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// The name a crash file gets once its report is queued.
pub(crate) fn reported_name(path: &Path) -> PathBuf {
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().trim_end_matches(".log").to_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.reported.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "DentalCraft 0.2.0-alpha.7 crashed\nthread: dc-open-project\npanicked at crates/dc-workspace/src/view.rs:10:5:\nindex out of bounds\n\nbacktrace:\n   0: std::panicking\n   1: dc_workspace::view\n";

    #[test]
    fn a_crash_file_is_read() {
        let c = Crash::parse(FILE).unwrap();
        assert_eq!(c.version, "0.2.0-alpha.7");
        assert_eq!(c.thread, "dc-open-project");
        assert_eq!(
            c.message,
            "panicked at crates/dc-workspace/src/view.rs:10:5:\nindex out of bounds"
        );
        assert!(c.backtrace.contains("dc_workspace::view"));
        let e = c.event(true);
        assert_eq!(
            (e.kind, e.source, e.code.as_str()),
            (Kind::Crash, Source::Workspace, "panic")
        );
        assert_eq!(e.context.as_ref().unwrap()["thread"], "dc-open-project");
        assert!(Crash::parse("not a crash").is_none());
        let other = Crash::parse("DentalCraft 1 crashed\nthread: main\nboom").unwrap();
        assert_eq!(other.event(false).source, Source::App);
        assert_eq!(other.event(false).backtrace, None);
    }

    #[test]
    fn crash_files_by_time() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "crash-100.log",
            "crash-300.log",
            "crash-200.reported.log",
            "crash-50.log",
            "dentalcraft.log.2026-11-18",
        ] {
            std::fs::write(dir.path().join(name), FILE).unwrap();
        }
        let found = unreported(dir.path(), 100);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["crash-100.log", "crash-300.log"]);
        assert_eq!(
            reported_name(&found[0]).file_name().unwrap(),
            "crash-100.reported.log"
        );
    }
}
