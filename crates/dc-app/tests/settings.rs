//! The Settings dialog: the gear of both top bars opens it, and its switch
//! "Advanced options and parameters" (off on a fresh installation) shows
//! the technical readouts, is saved, and reaches every screen in the frame
//! it changes (`dc_module_api::advanced_ui`).

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use dc_app::config::{Dirs, Store};
use dc_app::{AppDeps, DentalCraftApp, Screen};
use dc_i18n::Lang;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// The UI language is process-wide: tests that render take this lock.
static LANG_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LANG_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

const ADVANCED: &str = "Advanced options and parameters";

fn harness(dir: &Path) -> Harness<'static, DentalCraftApp> {
    let app = DentalCraftApp::new(AppDeps::headless(dir));
    dc_i18n::set_language(Lang::En).unwrap();
    Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(|ui, app: &mut DentalCraftApp| app.show(ui), app)
}

fn saved_advanced(dir: &Path) -> bool {
    Store::new(Dirs::under(dir)).load_settings().advanced
}

#[test]
fn home_hides_the_renderer_until_the_advanced_switch_is_on_and_saves_it() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path());
    h.run_steps(3);
    // a fresh installation: off, and the status bar has no renderer name
    assert!(!h.state().advanced());
    assert!(!dc_module_api::advanced_ui(&h.ctx));
    assert!(h.query_by_label("renderer not available").is_none());
    h.get_by_label("Settings").click();
    h.run_steps(2);
    assert!(h.state().settings_dialog_open());
    h.get_by_label(ADVANCED).click();
    h.run_steps(1);
    assert!(h.state().advanced());
    assert!(dc_module_api::advanced_ui(&h.ctx));
    assert!(saved_advanced(dir.path()));
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(!h.state().settings_dialog_open());
    assert!(h.query_by_label("renderer not available").is_some());
    // the next start keeps it
    let mut h = harness(dir.path());
    h.run_steps(2);
    assert!(h.state().advanced());
    assert!(dc_module_api::advanced_ui(&h.ctx));
}

#[test]
fn the_workspace_gear_opens_the_dialog_which_takes_the_keys() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path());
    h.run_steps(2);
    assert!(h.state_mut().new_workspace("aligners"));
    h.run_steps(3);
    assert_eq!(h.state().screen(), Screen::Workspace);
    // the technical readouts of the status bar are hidden
    assert!(h.query_by_label("FDI").is_none());
    h.get_by_label("Settings").click();
    h.run_steps(3);
    assert!(h.state().settings_dialog_open());
    // the keys are the dialog's: F1 does not open the workspace's help
    h.key_press(egui::Key::F1);
    h.run_steps(2);
    assert!(h.query_by_label("Keyboard shortcuts").is_none());
    h.get_by_label(ADVANCED).click();
    h.run_steps(1);
    assert!(dc_module_api::advanced_ui(&h.ctx));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(!h.state().settings_dialog_open());
    assert_eq!(h.state().screen(), Screen::Workspace);
    assert!(h.query_by_label("FDI").is_some());
}
