//! The DentalCraft module workspace (screen `05 Aligners workspace` of the
//! design): top bar, left menu tree from [`dc_module_api::Module::menu`],
//! 3D viewport ([`dc_viewport`]) with its overlays, inspector of the selected
//! function (laid out by `function`, with the module's part from
//! [`dc_module_api::Module::inspector`]), stage timeline and status bar.
//!
//! The case lives in a [`dc_command::UndoStack`]; module commands and the
//! scan import run as background jobs ([`dc_jobs::JobSystem`]) and land on
//! the undo stack. Saving writes a `.dcprj` atomically on a job thread; a
//! project that has a file is also saved by itself shortly after each change
//! ([`autosave`], driven by [`Workspace::tick`], which [`Workspace::show`]
//! calls every frame).
//!
//! The "Scan" button of "Import scans" opens the scan window (alpha, `scan`):
//! a 3D scanner driven by the `scan.*` commands of [`dc_scanner::control`],
//! the same ones an agent runs with [`Workspace::start_scan_command`]; its
//! surface joins the case through the import, as one undoable change.
//!
//! ```ignore
//! let mut ws = Workspace::new(Box::new(AlignersModule::new()), None, None, render_state);
//! // every frame
//! match ws.ui(ctx) {
//!     WorkspaceAction::GoHome | WorkspaceAction::Closed => { /* back to Home */ }
//!     WorkspaceAction::Saved(path) => { /* add to recent projects */ }
//!     WorkspaceAction::Quit => { /* close the window: nothing is left to save */ }
//!     WorkspaceAction::None => {}
//! }
//! // the window is asked to close
//! if !ws.request_quit() { /* keep it open: Quit comes once it is saved */ }
//! ```
//!
//! Tests and headless tools use [`Workspace::new_headless`] (no GPU, no
//! native file dialogs).

mod aligner;
mod archline;
pub mod autosave;
mod bite;
mod function;
mod icons;
pub mod import;
mod keys;
mod labels;
mod leaf;
mod manual;
mod menu;
mod overlay;
mod production;
mod project;
mod region;
mod review;
mod scan;
mod select;
mod setup;
mod staging;
pub mod timeline;
mod ui;
mod view;
mod workspace;

pub use menu::filter_ids;
pub use project::{now_rfc3339, project_codecs};
pub use scan::ScanTicket;
pub use workspace::{Workspace, WorkspaceAction};

dc_i18n::bundle!(pub(crate) fn loader);

/// Translates `key` (any registered bundle: this crate, the common keys,
/// the module's).
pub(crate) fn t(key: &str) -> String {
    loader();
    dc_i18n::tr(key)
}

#[cfg(test)]
mod i18n_tests {
    #[test]
    fn en_it_parity() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }
}
