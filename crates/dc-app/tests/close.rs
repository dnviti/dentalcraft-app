//! Closing the window keeps the work: an open project with unsaved changes
//! is saved before the window closes (the close is cancelled until then),
//! a project never saved asks first, and with nothing to save the window
//! closes at once.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dc_app::{AppDeps, DentalCraftApp};
use dc_case::CaseState;
use dc_command::{Command, CommandError, JobCtx};
use dc_i18n::Lang;
use dc_project::{AppInfo, Project};
use egui::{ViewportCommand, ViewportEvent, ViewportId};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(dir: &Path) -> Harness<'static, DentalCraftApp> {
    let app = DentalCraftApp::new(AppDeps::headless(dir));
    dc_i18n::set_language(Lang::En).unwrap();
    Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .build_ui_state(|ui, app: &mut DentalCraftApp| app.show(ui), app)
}

/// A project file with the lower demo arch.
fn demo_project_file(dir: &Path) -> PathBuf {
    let state = dca_module::commands::find("aligners.demo")
        .unwrap()
        .execute(
            &CaseState::default(),
            &json!({ "arch": "lower" }),
            &JobCtx::detached(),
        )
        .unwrap()
        .0;
    let project = Project::new(
        AppInfo {
            version: "0.1.0".into(),
            commit: "test".into(),
        },
        dc_workspace::now_rfc3339(),
        state,
    );
    let path = dir.join("close.dcprj");
    dc_project::save(&path, &project).unwrap();
    path
}

fn patient_code(path: &Path) -> String {
    let module = dca_module::AlignersModule::new();
    let loaded = dc_project::load(path, &dc_workspace::project_codecs(&module)).unwrap();
    loaded.project.case_state.case.patient_code
}

/// Renames the patient: a change of the case.
struct Rename(&'static str);

impl Command for Rename {
    fn id(&self) -> &'static str {
        "test.rename"
    }
    fn label_key(&self) -> &str {
        "test-rename"
    }
    fn apply(&self, state: &CaseState, _ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let mut next = state.clone();
        next.case.patient_code = self.0.into();
        Ok(next)
    }
}

/// Draws frames until `done`.
fn step_until(
    h: &mut Harness<'_, DentalCraftApp>,
    what: &str,
    done: impl Fn(&DentalCraftApp) -> bool,
) {
    let start = Instant::now();
    while !done(h.state()) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "timed out: {what}"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The user closes the window (its close button): one frame.
fn close_window(h: &mut Harness<'_, DentalCraftApp>) {
    h.input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(ViewportEvent::Close);
    h.step();
}

/// The app asked the window for `cmd` in the last frame.
fn asked(h: &Harness<'_, DentalCraftApp>, cmd: &ViewportCommand) -> bool {
    h.output()
        .viewport_output
        .get(&ViewportId::ROOT)
        .is_some_and(|v| v.commands.contains(cmd))
}

/// Changes the case of the open workspace and waits for the change.
fn change(h: &mut Harness<'_, DentalCraftApp>, code: &'static str) {
    h.state_mut()
        .workspace_mut()
        .unwrap()
        .run_command(Box::new(Rename(code)));
    step_until(h, "the change", |a| {
        a.workspace()
            .is_some_and(|w| w.state().case.patient_code == code)
    });
}

#[test]
fn closing_the_window_saves_the_open_project_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = demo_project_file(dir.path());
    let mut h = harness(dir.path());
    h.state_mut().open_project(path.clone());
    step_until(&mut h, "the project opens", |a| a.workspace().is_some());
    change(&mut h, "CLOSED");
    assert!(h.state().workspace().unwrap().is_dirty());

    close_window(&mut h);
    assert!(
        asked(&h, &ViewportCommand::CancelClose),
        "the window stays open while it saves"
    );
    assert!(!h.state().may_close());
    step_until(
        &mut h,
        "saved, the window may close",
        DentalCraftApp::may_close,
    );
    assert!(asked(&h, &ViewportCommand::Close));
    assert_eq!(patient_code(&path), "CLOSED");
    // the close that follows goes through
    close_window(&mut h);
    assert!(!asked(&h, &ViewportCommand::CancelClose));
}

#[test]
fn with_nothing_to_save_the_window_closes_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path());
    h.run_steps(2);
    close_window(&mut h);
    assert!(!asked(&h, &ViewportCommand::CancelClose));
    // a project open and saved: the same
    let path = demo_project_file(dir.path());
    h.state_mut().open_project(path);
    step_until(&mut h, "the project opens", |a| a.workspace().is_some());
    close_window(&mut h);
    assert!(!asked(&h, &ViewportCommand::CancelClose));
}

#[test]
fn closing_a_project_never_saved_asks_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(dir.path());
    assert!(h.state_mut().new_workspace("aligners"));
    h.run_steps(2);
    change(&mut h, "NEW");
    close_window(&mut h);
    assert!(asked(&h, &ViewportCommand::CancelClose));
    h.run_steps(2);
    assert!(h.query_by_label("Don't save").is_some());
    h.get_by_label("Don't save").click();
    step_until(&mut h, "the window may close", DentalCraftApp::may_close);
    assert!(asked(&h, &ViewportCommand::Close));
}
