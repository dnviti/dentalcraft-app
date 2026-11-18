//! Logging (`tracing`, daily rolling file in the data directory plus
//! stderr, plus the layer of the anonymous diagnostics) and the panic hook
//! that writes a crash report.

use std::io::Write;
use std::path::{Path, PathBuf};

use dc_diagnostics::logs::LOG_PREFIX;
use dc_diagnostics::{Crash, DiagLayer, Diagnostics};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Starts logging to `<log_dir>/dentalcraft.log.<date>` and stderr, and to
/// `diagnostics` (it records nothing without the user's consent). Keep the
/// guard alive for the whole run (it flushes the file on drop). `RUST_LOG`
/// overrides the default `info` filter.
pub fn init(log_dir: &Path, diagnostics: Option<DiagLayer>) -> Option<WorkerGuard> {
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
        .with(diagnostics)
        .try_init();
    if res.is_err() {
        return None;
    }
    guard
}

/// Installs a panic hook that writes `<log_dir>/crash-<unix time>.log`
/// (message, location, backtrace) and logs the panic, then runs the
/// previous hook. With the consent, `diagnostics` queues the crash at once
/// and the file is named `crash-<unix time>.reported.log` (else the next
/// start picks it up).
pub fn install_panic_hook(log_dir: PathBuf, diagnostics: Option<Diagnostics>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        let report = format!(
            "DentalCraft {} crashed\nthread: {thread}\n{info}\n\nbacktrace:\n{backtrace}\n",
            env!("CARGO_PKG_VERSION"),
        );
        // the crash event comes from the hook, with its call stack
        tracing::error!(crash = true, "panic: {info}");
        let reported = diagnostics
            .as_ref()
            .is_some_and(|d| Crash::parse(&report).is_some_and(|c| d.record_crash(&c)));
        let name = if reported {
            format!("crash-{}.reported.log", crate::config::now_unix())
        } else {
            format!("crash-{}.log", crate::config::now_unix())
        };
        let path = log_dir.join(name);
        if std::fs::create_dir_all(&log_dir).is_ok()
            && let Ok(mut f) = std::fs::File::create(&path)
        {
            let _ = f.write_all(report.as_bytes());
            eprintln!("dentalcraft: crash report written to {}", path.display());
        }
        previous(info);
    }));
}
