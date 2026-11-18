//! The `tracing` layer that turns the log's errors into events.
//!
//! While diagnostics are on it keeps the last log lines in memory (the
//! `log_tail` of the next error or crash) and records:
//!
//! - every `ERROR` event as an `error`;
//! - a `WARN` event only when it carries `diag = true` (a failure worth a
//!   report that the app survives: an import with no scan, a sign-in
//!   error).
//!
//! The structured fields `code`, `command`, `module` and `source` fill the
//! event; without `code` the log target names it
//! (`dc_workspace::workspace` → `dc-workspace.workspace`). An event with
//! `crash = true` is skipped (the panic hook reports it with its call
//! stack), and so are the crate's own logs. Off, it does nothing.

use std::fmt::Write as _;

use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use crate::{Diagnostics, Kind, NewEvent, Source};

/// The layer; add it to the app's subscriber ([`Diagnostics::layer`]).
pub struct DiagLayer {
    pub(crate) diag: Diagnostics,
}

#[derive(Default)]
struct Fields {
    message: String,
    code: Option<String>,
    command: Option<String>,
    module: Option<String>,
    source: Option<String>,
    diag: bool,
    crash: bool,
    other: String,
}

impl Fields {
    fn set(&mut self, field: &Field, value: String) {
        match field.name() {
            "code" | "command" | "module" | "source" if value.is_empty() => {}
            "message" => self.message = value,
            "code" => self.code = Some(value),
            "command" => self.command = Some(value),
            "module" => self.module = Some(value),
            "source" => self.source = Some(value),
            "diag" => self.diag = value == "true",
            "crash" => self.crash = value == "true",
            name => {
                let _ = write!(self.other, " {name}={value}");
            }
        }
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.set(field, value.to_owned());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.set(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        let text = match field.name() {
            "message" => text,
            _ => text.trim_matches('"').to_owned(),
        };
        self.set(field, text);
    }
}

/// `dc_workspace::workspace` → `dc-workspace.workspace`.
fn code_of_target(target: &str) -> String {
    target.replace("::", ".").replace('_', "-")
}

impl<S: Subscriber> Layer<S> for DiagLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if !self.diag.enabled() {
            return;
        }
        let meta = event.metadata();
        let target = meta.target();
        if target.starts_with(env!("CARGO_CRATE_NAME")) {
            return;
        }
        let mut f = Fields::default();
        event.record(&mut f);
        let level = *meta.level();
        self.diag.push_log_line(&format!(
            "{} {level:>5} {target}: {}{}",
            chrono::Utc::now().format("%H:%M:%S%.3f"),
            f.message,
            f.other
        ));
        let kind = if level == Level::ERROR {
            Kind::Error
        } else if level == Level::WARN && f.diag {
            Kind::Warning
        } else {
            return;
        };
        if f.crash {
            return;
        }
        let source = f
            .source
            .as_deref()
            .and_then(Source::from_name)
            .unwrap_or_else(|| Source::from_target(target));
        let code = f.code.unwrap_or_else(|| code_of_target(target));
        let module = f
            .module
            .or_else(|| target.starts_with("dca_").then(|| "aligners".to_owned()));
        let mut e = NewEvent::new(kind, source, &code, f.message);
        e.command = f.command;
        e.module = module;
        self.diag.record(e);
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use crate::service::tests::diagnostics;
    use crate::{Kind, Source};

    #[test]
    fn the_layer_records_only_with_consent() {
        let dir = tempfile::tempdir().unwrap();
        let diag = diagnostics(dir.path(), false);
        let subscriber = tracing_subscriber::registry().with(diag.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "dc_app::app", code = "save-failed", source = "save", "cannot save");
            assert_eq!(diag.pending(), 0);
            diag.set_enabled(true);
            tracing::info!(target: "dc_app::app", "opening a project");
            tracing::warn!(target: "dc_app::app", "a plain warning is not a report");
            tracing::warn!(
                target: "dc_app::app",
                diag = true,
                code = "import-nothing",
                source = "import",
                "no scan could be imported"
            );
            tracing::error!(
                target: "dc_app::app",
                code = "command-failed",
                command = "aligners.planning.setup",
                module = "aligners",
                source = "command",
                "the setup failed for mario.rossi@studio.it"
            );
            tracing::error!(target: "dc_app::logging", crash = true, "panic: reported by the hook");
            // the crate's own logs are never reports
            tracing::error!("diagnostics: not sent");
            tracing::error!(target: "dc_workspace::workspace", "an uncoded error");
        });
        let events = diag.queued();
        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(
            (events[0].kind, events[0].source),
            (Kind::Warning, Source::Import)
        );
        let e = &events[1];
        assert_eq!(
            (e.kind, e.source, e.code.as_str()),
            (Kind::Error, Source::Command, "command-failed")
        );
        assert_eq!(e.command.as_deref(), Some("aligners.planning.setup"));
        assert_eq!(e.module.as_deref(), Some("aligners"));
        assert_eq!(e.message, "the setup failed for <email>");
        let tail = e.log_tail.as_deref().unwrap();
        assert!(tail.contains("opening a project"), "{tail}");
        assert!(!tail.contains("mario.rossi"), "{tail}");
        // no code: the target names it
        assert_eq!(events[2].code, "dc-workspace.workspace");
        assert_eq!(events[2].source, Source::Workspace);
    }
}
