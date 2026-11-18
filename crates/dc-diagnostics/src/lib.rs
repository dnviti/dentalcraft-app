//! Anonymous diagnostics of DentalCraft: what goes wrong on the user's
//! machine reaches its developer, only when the user agreed.
//!
//! - **Consent** ([`settings`]): `diagnostics` in the app's `settings.json`
//!   (`null`: not asked yet) and `install_id`, a random number not linked to
//!   the account. Nothing is collected or sent while it is not `true`;
//!   turning it off deletes what waits and the number.
//! - **Events** ([`Event`]): crashes (the panic hook, and the crash files
//!   of earlier runs: [`Diagnostics::pick_up_crashes`]), errors and flagged
//!   warnings of the log ([`DiagLayer`]), one session event per start.
//!   Every text is cleaned of personal data first ([`Sanitizer`]): home
//!   folder, user name, project and scan file names, patient codes, e-mail
//!   addresses and access codes.
//! - **Queue** ([`Queue`]): one file per event in `<data>/diagnostics/queue`,
//!   bounded (oldest dropped).
//! - **Sending**: `POST {issuer}/api/v1/diagnostics/reports` ([`endpoint`]),
//!   without any account data, from a worker thread
//!   ([`Diagnostics::start_worker`]): at start, a few seconds after new
//!   events, and every ten minutes; later again after a refusal for load,
//!   a server error or no connection; a report the server rejects as
//!   invalid is dropped.
//! - **Commands** ([`commands`]): `app.diagnostics.status`, `.set`,
//!   `.preview` and `.send` for `dentalcraft-cli`, its MCP server and the
//!   app bridge.

pub mod commands;
mod crash;
mod event;
mod layer;
pub mod logs;
mod queue;
mod sanitize;
mod send;
mod service;
pub mod settings;
pub mod system;

pub use crash::Crash;
pub use event::{
    AppInfo, Event, Gpu, Kind, MAX_BACKTRACE, MAX_BODY, MAX_CODE, MAX_CONTEXT, MAX_EVENTS,
    MAX_LOG_TAIL, MAX_MESSAGE, NewEvent, Platform, SCHEMA, Source, install_kind,
};
pub use layer::DiagLayer;
pub use queue::{MAX_QUEUED, MAX_QUEUED_BYTES, Queue};
pub use sanitize::Sanitizer;
pub use send::{SendOutcome, post};
pub use service::{Config, Diagnostics, Flush};
pub use settings::Consent;

dc_i18n::bundle!(pub fn loader);

/// Path of the reports on the server, after the issuer.
pub const REPORTS_PATH: &str = "/api/v1/diagnostics/reports";

/// The address the reports go to on the server `issuer`.
pub fn endpoint(issuer: &str) -> String {
    format!("{}{REPORTS_PATH}", issuer.trim_end_matches('/'))
}

/// The current time as `2026-11-18T16:00:00Z`.
pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Current Unix time in seconds.
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translations_are_complete() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }

    #[test]
    fn the_endpoint_is_under_the_issuer() {
        assert_eq!(
            endpoint("http://127.0.0.1:8787/"),
            "http://127.0.0.1:8787/api/v1/diagnostics/reports"
        );
        assert_eq!(
            endpoint("https://auth.dentalcraft.app"),
            "https://auth.dentalcraft.app/api/v1/diagnostics/reports"
        );
        let t = now_rfc3339();
        assert!(t.ends_with('Z') && t.len() == 20, "{t}");
    }
}
