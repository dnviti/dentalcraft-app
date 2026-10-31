//! Logging (`tracing`, daily rolling file in the data directory plus
//! stderr) and the panic hook that writes a crash report.

use std::io::Write;
use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// File name prefix of the rolling log files.
pub const LOG_PREFIX: &str = "dentalcraft.log";

/// Starts logging to `<log_dir>/dentalcraft.log.<date>` and stderr. Keep the
/// guard alive for the whole run (it flushes the file on drop). `RUST_LOG`
/// overrides the default `info` filter.
pub fn init(log_dir: &Path) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));
    let (file_layer, guard) = match std::fs::create_dir_all(log_dir) {
        Ok(()) => {
            let appender = tracing_appender::rolling::daily(log_dir, LOG_PREFIX);
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (
                Some(fmt::layer().with_ansi(false).with_writer(writer)),
                Some(guard),
            )
        }
        Err(e) => {
            eprintln!("dentalcraft: cannot create {}: {e}", log_dir.display());
            (None, None)
        }
    };
    let res = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(fmt::layer().with_writer(std::io::stderr))
        .try_init();
    if res.is_err() {
        return None;
    }
    guard
}

/// Installs a panic hook that writes `<log_dir>/crash-<unix time>.log`
/// (message, location, backtrace) and logs the panic, then runs the
/// previous hook.
pub fn install_panic_hook(log_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "DentalCraft {} crashed\nthread: {}\n{info}\n\nbacktrace:\n{backtrace}\n",
            env!("CARGO_PKG_VERSION"),
            std::thread::current().name().unwrap_or("?"),
        );
        tracing::error!("panic: {info}");
        let path = log_dir.join(format!("crash-{}.log", crate::config::now_unix()));
        if std::fs::create_dir_all(&log_dir).is_ok()
            && let Ok(mut f) = std::fs::File::create(&path)
        {
            let _ = f.write_all(report.as_bytes());
            eprintln!("dentalcraft: crash report written to {}", path.display());
        }
        previous(info);
    }));
}

/// The newest log file in `log_dir`, if any.
pub fn newest_log(log_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(log_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(LOG_PREFIX))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// The last `n` lines of the newest log file (empty when there is none).
pub fn last_lines(log_dir: &Path, n: usize) -> Vec<String> {
    let Some(path) = newest_log(log_dir) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_of_newest_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(last_lines(dir.path(), 5).is_empty());
        let text: String = (0..10).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join(format!("{LOG_PREFIX}.2026-10-31")), text).unwrap();
        std::fs::write(dir.path().join("other.txt"), "x").unwrap();
        assert_eq!(last_lines(dir.path(), 2), ["line 8", "line 9"]);
    }
}
