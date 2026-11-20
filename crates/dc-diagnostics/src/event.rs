//! The report the server receives (contract v1): the installation, the
//! machine and the events.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the report format.
pub const SCHEMA: u32 = 1;
/// Longest `code`, in bytes.
pub const MAX_CODE: usize = 64;
/// Longest `message`, in bytes.
pub const MAX_MESSAGE: usize = 2000;
/// Longest `backtrace`, in bytes.
pub const MAX_BACKTRACE: usize = 64 * 1024;
/// Longest `log_tail`, in bytes.
pub const MAX_LOG_TAIL: usize = 64 * 1024;
/// Largest `context`, serialized, in bytes.
pub const MAX_CONTEXT: usize = 4 * 1024;
/// Most events in one report.
pub const MAX_EVENTS: usize = 200;
/// Largest report body the server takes, in bytes.
pub const MAX_BODY: usize = 256 * 1024;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The program panicked.
    Crash,
    /// Something failed.
    Error,
    /// Something went wrong but the work went on.
    Warning,
    /// The app started.
    Session,
}

/// Where in DentalCraft it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The application shell.
    App,
    /// The module workspace.
    Workspace,
    /// A command.
    Command,
    /// A background job.
    Job,
    /// Saving a project.
    Save,
    /// Opening or creating a project.
    Project,
    /// The scanner.
    Scan,
    /// The turntable.
    Turntable,
    /// Updates.
    Update,
    /// Sign-in.
    Auth,
    /// Drawing (GPU).
    Render,
    /// Importing scans.
    Import,
    /// Exporting files.
    Export,
    /// Anything else.
    Other,
}

impl Source {
    /// Every source.
    pub const ALL: [Source; 14] = [
        Source::App,
        Source::Workspace,
        Source::Command,
        Source::Job,
        Source::Save,
        Source::Project,
        Source::Scan,
        Source::Turntable,
        Source::Update,
        Source::Auth,
        Source::Render,
        Source::Import,
        Source::Export,
        Source::Other,
    ];

    /// The word of the report (`scan`).
    pub fn name(self) -> &'static str {
        match self {
            Source::App => "app",
            Source::Workspace => "workspace",
            Source::Command => "command",
            Source::Job => "job",
            Source::Save => "save",
            Source::Project => "project",
            Source::Scan => "scan",
            Source::Turntable => "turntable",
            Source::Update => "update",
            Source::Auth => "auth",
            Source::Render => "render",
            Source::Import => "import",
            Source::Export => "export",
            Source::Other => "other",
        }
    }

    /// The source a word names.
    pub fn from_name(word: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|s| s.name() == word.trim())
    }

    /// The source of a log target (`dc_workspace::workspace`) or a crate
    /// name (`dc-scan-ui`).
    pub fn from_target(target: &str) -> Source {
        let krate = target
            .split("::")
            .next()
            .unwrap_or_default()
            .replace('-', "_");
        match krate.as_str() {
            "dc_app" | "dentalcraft" => Source::App,
            "dc_workspace" | "dc_viewport" | "dc_ui" => Source::Workspace,
            "dc_command" | "dc_session" | "dc_module_api" | "dc_mcp" => Source::Command,
            "dc_jobs" => Source::Job,
            "dc_project" => Source::Project,
            "dc_scan_ui" | "dc_scanner" => Source::Scan,
            "dc_update" => Source::Update,
            "dc_auth_client" => Source::Auth,
            "wgpu" | "wgpu_core" | "wgpu_hal" | "egui_wgpu" | "naga" | "eframe" | "dc_raster" => {
                Source::Render
            }
            k if k.starts_with("dca_") => Source::Command,
            _ => Source::Other,
        }
    }
}

/// One event of a report, as it is queued and sent (already cleaned).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Random id (UUID v4): the server drops an event it already has.
    pub id: String,
    /// What happened.
    pub kind: Kind,
    /// When (`2026-11-18T15:59:58Z`).
    pub at: String,
    /// Where.
    pub source: Source,
    /// Stable code (`save-failed`, `scan.device`, `panic`).
    pub code: String,
    /// What the log said.
    pub message: String,
    /// The command id, when a command failed.
    pub command: Option<String>,
    /// The module id (`aligners`).
    pub module: Option<String>,
    /// The call stack of a crash.
    pub backtrace: Option<String>,
    /// The last log lines before the event.
    pub log_tail: Option<String>,
    /// A small object with more facts (`startup_ms`, `thread`).
    pub context: Option<Value>,
}

/// An event as a part of the app reports it, before it is cleaned and
/// stamped ([`crate::Diagnostics::record`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    /// What happened.
    pub kind: Kind,
    /// Where.
    pub source: Source,
    /// Stable code.
    pub code: String,
    /// What happened, in words.
    pub message: String,
    /// The command id.
    pub command: Option<String>,
    /// The module id.
    pub module: Option<String>,
    /// The call stack of a crash.
    pub backtrace: Option<String>,
    /// The last log lines (the recent lines the log layer kept when
    /// `None`, for crashes and errors).
    pub log_tail: Option<String>,
    /// More facts.
    pub context: Option<Value>,
}

impl NewEvent {
    /// An event with no command, module, backtrace, log lines or context.
    pub fn new(kind: Kind, source: Source, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            source,
            code: code.to_owned(),
            message: message.into(),
            command: None,
            module: None,
            backtrace: None,
            log_tail: None,
            context: None,
        }
    }

    /// With the command id.
    pub fn command(mut self, id: impl Into<String>) -> Self {
        self.command = Some(id.into());
        self
    }

    /// With the module id.
    pub fn module(mut self, id: impl Into<String>) -> Self {
        self.module = Some(id.into());
        self
    }

    /// With more facts.
    pub fn context(mut self, context: Value) -> Self {
        self.context = Some(context);
        self
    }
}

/// The running program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Version (`0.2.0-alpha.8`).
    pub version: String,
    /// Commit of the build, when known.
    pub commit: Option<String>,
    /// How it was installed: `deb`, `rpm`, `flatpak`, `appimage`, `setup`,
    /// `msi`, `macos-app`, `standalone` or `dev`.
    pub install_kind: Option<String>,
}

impl AppInfo {
    /// This program and its installation.
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("DC_GIT_COMMIT").map(str::to_owned),
            install_kind: Some(install_kind(dc_update::Installation::detect().kind).to_owned()),
        }
    }
}

/// The word of the report for an installation kind.
pub fn install_kind(kind: dc_update::InstallKind) -> &'static str {
    use dc_update::InstallKind as K;
    match kind {
        K::Standalone => "standalone",
        K::AppImage => "appimage",
        K::Deb => "deb",
        K::Rpm => "rpm",
        K::Flatpak => "flatpak",
        K::Exe => "setup",
        K::Msi => "msi",
        K::MacApp => "macos-app",
        K::Unknown => "dev",
    }
}

/// The graphics adapter.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Gpu {
    /// Backend (`Vulkan`, `Dx12`, `Metal`, `Gl`).
    pub backend: String,
    /// Adapter name.
    pub name: String,
    /// Driver and version.
    pub driver: String,
}

/// The machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// Operating system (`Fedora Linux 45`).
    pub os: String,
    /// CPU architecture (`x86_64`).
    pub arch: String,
    /// Language of the interface (`it`).
    pub locale: String,
    /// The graphics adapter, when one draws.
    pub gpu: Option<Gpu>,
    /// Total memory in MB.
    pub memory_mb: Option<u64>,
    /// Logical processors.
    pub cpus: Option<u32>,
}

impl Platform {
    /// This machine (the language is the interface's at the time of the
    /// report).
    pub fn current(gpu: Option<Gpu>) -> Self {
        Self {
            os: crate::system::os_name(),
            arch: std::env::consts::ARCH.to_owned(),
            locale: dc_i18n::current().code().to_owned(),
            gpu,
            memory_mb: crate::system::memory_gib().map(|(total, _)| (total * 1024.0) as u64),
            cpus: std::thread::available_parallelism()
                .ok()
                .and_then(|n| u32::try_from(n.get()).ok()),
        }
    }
}

/// The body of `POST /api/v1/diagnostics/reports`.
#[derive(Debug, Serialize)]
pub(crate) struct ReportBody<'a> {
    pub schema: u32,
    pub install_id: &'a str,
    pub sent_at: String,
    pub app: &'a AppInfo,
    pub system: &'a Platform,
    pub events: &'a [Event],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_of_the_contract() {
        for s in Source::ALL {
            assert_eq!(Source::from_name(s.name()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.name());
        }
        assert_eq!(serde_json::to_value(Kind::Crash).unwrap(), "crash");
        assert_eq!(serde_json::to_value(Kind::Session).unwrap(), "session");
        assert_eq!(
            Source::from_target("dc_workspace::workspace"),
            Source::Workspace
        );
        assert_eq!(Source::from_target("dc-scan-ui"), Source::Scan);
        assert_eq!(Source::from_target("dca_algo::setup"), Source::Command);
        assert_eq!(Source::from_target("wgpu_core::device"), Source::Render);
        assert_eq!(Source::from_target("hyper"), Source::Other);
        assert_eq!(install_kind(dc_update::InstallKind::Exe), "setup");
        assert_eq!(install_kind(dc_update::InstallKind::MacApp), "macos-app");
        assert_eq!(install_kind(dc_update::InstallKind::Unknown), "dev");
        let app = AppInfo::current();
        assert_eq!(app.version, env!("CARGO_PKG_VERSION"));
        let p = Platform::current(None);
        assert!(!p.os.is_empty() && !p.arch.is_empty());
        assert!(p.cpus.is_some_and(|n| n > 0));
    }

    #[test]
    fn an_event_serializes_every_field() {
        let e = Event {
            id: "x".into(),
            kind: Kind::Error,
            at: "2026-11-18T15:59:58Z".into(),
            source: Source::Save,
            code: "save-failed".into(),
            message: "m".into(),
            command: None,
            module: Some("aligners".into()),
            backtrace: None,
            log_tail: None,
            context: None,
        };
        let v = serde_json::to_value(&e).unwrap();
        for k in [
            "id",
            "kind",
            "at",
            "source",
            "code",
            "message",
            "command",
            "module",
            "backtrace",
            "log_tail",
            "context",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(v["kind"], "error");
        assert_eq!(serde_json::from_value::<Event>(v).unwrap(), e);
    }
}
