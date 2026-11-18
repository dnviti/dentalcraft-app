//! The DentalCraft desktop application shell: launcher (Home), sign-in,
//! support, new project and the module workspace (`dc-workspace`).
//!
//! The binary (`src/main.rs`) only sets up logging and eframe; everything
//! else lives here so `egui_kittest` can drive it ([`DentalCraftApp::show`]).

/// Translates a key of this crate's bundle, checked at compile time against
/// `i18n/en/dc-app.ftl`.
macro_rules! t {
    ($key:literal) => {
        ::dc_i18n::fl!(crate::loader(), $key)
    };
    ($key:literal, $($arg:ident = $val:expr),+ $(,)?) => {
        ::dc_i18n::fl!(crate::loader(), $key, $($arg = $val),+)
    };
}

pub mod about;
mod app;
pub mod config;
mod consent;
pub mod control;
pub mod create_job;
pub mod diagnostics;
pub mod dialogs;
pub mod logging;
pub mod modules;
mod paint;
mod screens;
pub mod support_client;
mod update;

pub use about::AboutView;
pub use app::{AppDeps, DentalCraftApp, Screen};
pub use update::Updates;

dc_i18n::bundle!(pub fn loader);

/// Registers every Fluent bundle the shell shows (so a language switch
/// reaches all of them immediately).
pub fn register_bundles() {
    loader();
    dc_i18n::loader();
    dc_ui::loader();
    dca_module::loader();
    dc_update::loader();
    dc_diagnostics::loader();
}

/// Runtime i18n keys (looked up with `dc_i18n::tr`), listed so a test can
/// check that each one exists in every language.
pub fn runtime_keys() -> Vec<&'static str> {
    use dc_auth_client::AuthErrorCode as E;
    let mut keys: Vec<&'static str> = [
        E::InvalidIssuer,
        E::Network,
        E::Timeout,
        E::DiscoveryFailed,
        E::ListenerFailed,
        E::CallbackInvalid,
        E::AccessDenied,
        E::ServerRejected,
        E::InvalidGrant,
        E::TokenInvalid,
        E::Cancelled,
        E::StorageUnavailable,
        E::NotSignedIn,
        E::Internal,
    ]
    .into_iter()
    .map(E::i18n_key)
    .collect();
    for m in modules::COMING_SOON {
        keys.push(m.name_key);
        keys.push(m.desc_key);
        keys.extend(m.eta_key);
    }
    keys.extend(["module-aligners-desc", "module-generic-desc"]);
    keys.extend(
        screens::support::DOC_LINKS
            .iter()
            .flat_map(|d| [d.title_key, d.desc_key]),
    );
    keys.extend(screens::support::CATEGORIES.iter().map(|c| c.1));
    keys.extend(
        screens::home::NEWS
            .iter()
            .flat_map(|n| [n.title_key, n.body_key]),
    );
    keys.extend(screens::home::NEWS.iter().filter_map(|n| n.link_key));
    keys.extend(app::WEEKDAY_KEYS);
    keys.extend(consent::runtime_keys());
    keys.extend(app::MONTH_KEYS);
    keys
}
