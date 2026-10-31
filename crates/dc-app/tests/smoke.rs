//! egui_kittest smoke tests of the shell: every screen renders in English
//! and Italian, the language switch relabels the UI, navigation, New
//! project validation, and i18n key coverage.

use std::sync::{Mutex, MutexGuard, PoisonError};

use dc_app::{AppDeps, DentalCraftApp, Screen};
use dc_i18n::Lang;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// The UI language is process-wide: tests that render take this lock.
static LANG_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LANG_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn harness(dir: &std::path::Path, lang: Lang) -> Harness<'static, DentalCraftApp> {
    let app = DentalCraftApp::new(AppDeps::headless(dir));
    dc_i18n::set_language(lang).unwrap();
    Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(|ui, app: &mut DentalCraftApp| app.show(ui), app)
}

#[test]
fn every_screen_renders_in_both_languages() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    for lang in Lang::ALL {
        let mut h = harness(dir.path(), lang);
        for screen in [
            Screen::Home,
            Screen::SignIn,
            Screen::Support,
            Screen::NewProject,
        ] {
            h.state_mut().go(screen);
            h.run_steps(3);
            assert_eq!(h.state().screen(), screen);
        }
        let title = match lang {
            Lang::En => "New project",
            Lang::It => "Nuovo progetto",
        };
        assert!(h.query_by_label(title).is_some(), "{lang}: {title}");
    }
    dc_i18n::set_language(Lang::En).unwrap();
}

#[test]
fn language_switch_relabels_the_ui() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), Lang::En);
    h.run_steps(3);
    assert!(h.query_by_label("Open project…").is_some());
    h.get_by_label("IT").click();
    h.run_steps(3);
    assert_eq!(dc_i18n::current(), Lang::It);
    assert!(h.query_by_label("Apri progetto…").is_some());
    assert!(h.query_by_label("Open project…").is_none());
    h.get_by_label("EN").click();
    h.run_steps(3);
    assert!(h.query_by_label("Open project…").is_some());
    // the choice is persisted
    let settings =
        dc_app::config::Store::new(dc_app::config::Dirs::under(dir.path())).load_settings();
    assert_eq!(settings.language.as_deref(), Some("en"));
}

#[test]
fn navigation_home_support_home() {
    let _g = lock();
    dc_i18n::set_language(Lang::En).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), Lang::En);
    h.run_steps(3);
    assert_eq!(h.state().screen(), Screen::Home);
    h.get_by_label("Support").click();
    h.run_steps(3);
    assert_eq!(h.state().screen(), Screen::Support);
    assert!(h.query_by_label("Help & support").is_some());
    h.get_by_label("Home").click();
    h.run_steps(3);
    assert_eq!(h.state().screen(), Screen::Home);
}

#[test]
fn new_project_requires_case_alias_and_scans() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path(), Lang::En);
    h.get_by_label("New project").click();
    h.run_steps(3);
    assert_eq!(h.state().screen(), Screen::NewProject);
    assert!(h.query_by_label_contains("Required").is_none());
    h.get_by_label("Create project and segment").click();
    h.run_steps(3);
    // still here, with the errors shown
    assert_eq!(h.state().screen(), Screen::NewProject);
    assert_eq!(h.query_all_by_label_contains("Required").count(), 2);
    assert!(h.query_by_label_contains("Add at least one scan").is_some());
    // coming-soon modules cannot be picked
    assert!(h.query_all_by_label("Implant").count() >= 1);
}

#[test]
fn i18n_keys_exist_in_every_language() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
    dc_i18n::parity::check_dir(&dir).unwrap();
    let en = std::fs::read_to_string(dir.join("en").join("dc-app.ftl")).unwrap();
    let ids = dc_i18n::parity::ids(&en).unwrap();
    // runtime keys of this crate; the module name keys live in their own bundles
    let missing: Vec<_> = dc_app::runtime_keys()
        .into_iter()
        .filter(|k| !ids.contains(*k))
        .collect();
    assert!(missing.is_empty(), "missing keys: {missing:?}");
    // the sign-in error keys of dc-auth-client resolve through the registry
    let _g = lock();
    dc_app::register_bundles();
    for lang in Lang::ALL {
        dc_i18n::set_language(lang).unwrap();
        for k in dc_app::runtime_keys() {
            assert_ne!(dc_i18n::tr(k), k, "{lang}: {k}");
        }
    }
    dc_i18n::set_language(Lang::En).unwrap();
}
