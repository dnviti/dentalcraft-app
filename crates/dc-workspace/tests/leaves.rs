//! The leaves that ask for a folder or a file, what the workspace shows of
//! the output of a command, and the run button of the inspector (the only
//! way to run a function: a click on the menu selects it): tested with a
//! module of its own whose commands record their parameters, so no dialog
//! and no real pipeline is needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use dc_case::CaseState;
use dc_command::{CommandError, JobCtx};
use dc_module_api::{
    CommandDesc, CommandOutput, CommandSpec, LeafInput, MenuItem, MenuTree, Module, ModuleCodec,
    ModuleCtx, ModuleError,
};
use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui::{Role, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

const EXPORT: &str = "aligners.output.export-all";
const VISIT: &str = "aligners.followup.visits";
const BITE: &str = "aligners.occlusion.align";
const OTHER: &str = "aligners.teeth.build";

/// The parameters the commands were run with, and whether the export waits
/// for a cancel. The tests share them: they run one at a time.
static CALLS: Mutex<Vec<(&'static str, Value)>> = Mutex::new(Vec::new());
static SLOW: AtomicBool = AtomicBool::new(false);
/// The export does not look at the cancel flag until a test clears this (a
/// long step), so the test does not race the clock on a slow machine.
static STUBBORN: AtomicBool = AtomicBool::new(false);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
static METHOD: Mutex<&'static str> = Mutex::new("virtual_bite");
/// The case holds the result of `OTHER`.
static OTHER_DONE: AtomicBool = AtomicBool::new(false);

fn exclusive() -> MutexGuard<'static, ()> {
    let g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    CALLS.lock().unwrap().clear();
    SLOW.store(false, Ordering::SeqCst);
    STUBBORN.store(false, Ordering::SeqCst);
    OTHER_DONE.store(false, Ordering::SeqCst);
    g
}

fn calls() -> Vec<(&'static str, Value)> {
    CALLS.lock().unwrap().clone()
}

fn run_export(
    state: &CaseState,
    params: &Value,
    ctx: &JobCtx,
) -> Result<CommandOutput, CommandError> {
    CALLS.lock().unwrap().push((EXPORT, params.clone()));
    let held = Instant::now();
    while STUBBORN.load(Ordering::SeqCst) && held.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(5));
    }
    if SLOW.load(Ordering::SeqCst) {
        // a long job: it stops at the first look at the cancel flag
        for _ in 0..4000 {
            ctx.check_cancelled()?;
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let out = json!({ "dir": params["dir"], "files": [{}, {}, {}] });
    Ok((state.clone(), out))
}

fn run_visit(
    state: &CaseState,
    params: &Value,
    _ctx: &JobCtx,
) -> Result<CommandOutput, CommandError> {
    CALLS.lock().unwrap().push((VISIT, params.clone()));
    Ok((state.clone(), json!({ "teeth": 14, "on_track": 11 })))
}

fn run_bite(
    state: &CaseState,
    params: &Value,
    _ctx: &JobCtx,
) -> Result<CommandOutput, CommandError> {
    CALLS.lock().unwrap().push((BITE, params.clone()));
    Ok((state.clone(), json!({ "method": *METHOD.lock().unwrap() })))
}

fn run_other(
    state: &CaseState,
    params: &Value,
    _ctx: &JobCtx,
) -> Result<CommandOutput, CommandError> {
    CALLS.lock().unwrap().push((OTHER, params.clone()));
    Ok((state.clone(), json!({})))
}

fn schema() -> Value {
    json!({ "type": "object" })
}

fn spec(
    id: &'static str,
    run: fn(&CaseState, &Value, &JobCtx) -> Result<CommandOutput, CommandError>,
) -> CommandSpec {
    CommandSpec {
        id,
        label_key: "aligners-menu-auto",
        doc_key: "aligners-cmd-auto-doc",
        params_schema: schema,
        run,
    }
}

/// A module with four leaves: the export (asks for a folder), the control
/// visit (asks for a scan), the bite and one that runs on a click.
struct Fake {
    inner: AlignersModule,
}

impl Module for Fake {
    fn id(&self) -> &'static str {
        self.inner.id()
    }
    fn name_key(&self) -> &'static str {
        self.inner.name_key()
    }
    fn version(&self) -> &'static str {
        self.inner.version()
    }
    fn menu(&self) -> MenuTree {
        MenuTree::new(vec![MenuItem::submenu(
            "fake.group",
            "aligners-menu-planning",
            vec![
                MenuItem::command(EXPORT, "aligners-menu-output-export-all", EXPORT)
                    .with_input(LeafInput::Folder { param: "dir" })
                    .not_a_step(),
                MenuItem::command(VISIT, "aligners-menu-followup-visits", VISIT)
                    .with_input(LeafInput::ScanFile { param: "scan" })
                    .not_a_step(),
                MenuItem::command(BITE, "aligners-menu-occlusion-align", BITE),
                MenuItem::command(OTHER, "aligners-menu-teeth-build", OTHER),
            ],
        )])
    }
    fn commands(&self) -> Vec<CommandDesc> {
        [EXPORT, VISIT, BITE, OTHER]
            .into_iter()
            .map(|id| CommandDesc::new(id, "aligners-menu-auto"))
            .collect()
    }
    fn command_specs(&self) -> Vec<CommandSpec> {
        vec![
            spec(EXPORT, run_export),
            spec(VISIT, run_visit),
            spec(BITE, run_bite),
            spec(OTHER, run_other),
        ]
    }
    fn invoke(&mut self, id: &str, ctx: &mut ModuleCtx<'_>) -> Result<(), ModuleError> {
        // the commands without an input run with their defaults
        if id == EXPORT || id == VISIT {
            return Ok(());
        }
        let spec = self
            .command_specs()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| ModuleError::UnknownCommand(id.to_owned()))?;
        ctx.queue(Box::new(spec.command(Value::Null)));
        Ok(())
    }
    fn leaf_done(&self, _state: &CaseState, id: &str) -> bool {
        id == OTHER && OTHER_DONE.load(Ordering::SeqCst)
    }
    fn inspector(&mut self, ui: &mut egui::Ui, ctx: &mut ModuleCtx<'_>) {
        self.inner.inspector(ui, ctx);
    }
    fn scene(&self, state: &CaseState) -> Vec<dc_scene::SceneItem> {
        self.inner.scene(state)
    }
    fn data_codec(&self) -> ModuleCodec {
        self.inner.data_codec()
    }
}

fn fake() -> Workspace {
    Workspace::new_headless(
        Box::new(Fake {
            inner: AlignersModule::new(),
        }),
        None,
        None,
    )
}

fn harness(ws: Workspace) -> Harness<'static, Workspace> {
    Harness::builder()
        .with_size(vec2(1600.0, 900.0))
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        )
}

fn settle(h: &mut Harness<'static, Workspace>) {
    h.step();
    assert!(h.state_mut().wait_for_jobs(WAIT));
    h.run();
}

#[test]
fn a_folder_answer_runs_the_command_with_the_dir_parameter() {
    let _g = exclusive();
    let mut ws = fake();
    ws.run_leaf(EXPORT);
    assert_eq!(ws.dialog_requests(), 1);
    assert!(calls().is_empty(), "nothing runs before the answer");
    ws.answer_leaf_input(EXPORT, Some(PathBuf::from("/tmp/dc-print")));
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(calls(), [(EXPORT, json!({ "dir": "/tmp/dc-print" }))]);
    // the sentence says where the files are and how many there are, and it stays
    let keys = ws.sticky_toast_keys();
    assert_eq!(keys, ["ws-toast-export-done"], "{keys:?}");
    let detail = ws.toast_details().pop().unwrap().1.unwrap();
    assert!(detail.contains("/tmp/dc-print"), "{detail}");
    assert!(
        detail.contains("4 files"),
        "three listed and the manifest: {detail}"
    );
    // an export writes files and leaves the case as it is: no undo step
    assert!(!ws.undo());
    assert!(!ws.is_dirty());
}

#[test]
fn a_scan_answer_runs_the_visit_with_the_scan_parameter_in_one_undo_step() {
    let _g = exclusive();
    let mut ws = fake();
    ws.run_leaf(VISIT);
    ws.answer_leaf_input(VISIT, Some(PathBuf::from("/tmp/visit.stl")));
    assert!(ws.wait_for_jobs(WAIT));
    assert_eq!(calls(), [(VISIT, json!({ "scan": "/tmp/visit.stl" }))]);
    let details = ws.toast_details();
    assert_eq!(details[0].0, "ws-toast-visit-added");
    assert!(details[0].1.as_deref().unwrap().contains("11 of 14"));
    assert!(ws.is_dirty());
    assert!(ws.undo(), "one undo step");
    assert!(!ws.is_dirty());
    assert!(!ws.undo(), "and only one");
}

#[test]
fn a_cancelled_dialog_does_nothing() {
    let _g = exclusive();
    let mut ws = fake();
    ws.run_leaf(EXPORT);
    ws.answer_leaf_input(EXPORT, None);
    assert!(!ws.is_busy());
    assert!(calls().is_empty());
    assert!(ws.toast_keys().is_empty());
    assert!(!ws.is_dirty());
}

#[test]
fn while_a_command_runs_the_leaf_gets_the_busy_toast_and_no_dialog() {
    let _g = exclusive();
    let mut ws = fake();
    SLOW.store(true, Ordering::SeqCst);
    ws.answer_leaf_input(EXPORT, Some(PathBuf::from("/tmp/a")));
    assert!(ws.is_busy());
    let asked = ws.dialog_requests();
    // the button of another input leaf, and a late answer
    ws.run_leaf(VISIT);
    assert_eq!(ws.dialog_requests(), asked, "no dialog while busy");
    ws.answer_leaf_input(VISIT, Some(PathBuf::from("/tmp/v.stl")));
    ws.run_leaf(OTHER);
    let busy = ws
        .toast_keys()
        .iter()
        .filter(|k| *k == "ws-toast-busy")
        .count();
    assert_eq!(busy, 3, "{:?}", ws.toast_keys());
    assert!(
        calls().iter().all(|(id, _)| *id == EXPORT),
        "only the export ran"
    );
    // stop it with the status bar cross
    let mut h = harness(ws);
    // the spinner keeps the window repainting: step, do not run
    h.step();
    h.step();
    h.get_by_label("Cancel the background job").click();
    SLOW.store(false, Ordering::SeqCst);
    settle(&mut h);
}

#[test]
fn the_export_says_it_takes_long_shows_it_running_and_the_cross_stops_it() {
    let _g = exclusive();
    let mut ws = fake();
    SLOW.store(true, Ordering::SeqCst);
    ws.answer_leaf_input(EXPORT, Some(PathBuf::from("/tmp/dc-slow")));
    assert!(ws.toast_keys().contains(&"ws-toast-export-long".to_owned()));
    let long = ws
        .toast_details()
        .into_iter()
        .find(|(k, _)| k == "ws-toast-export-long");
    assert!(
        long.unwrap().1.unwrap().contains("can take a long time"),
        "the line says it"
    );
    let mut h = harness(ws);
    h.step();
    h.step();
    // a visible running state with a way to stop it
    assert!(h.query_by_label("Exporting the print files").is_some());
    let cross = h.get_by_label("Cancel the background job");
    cross.click();
    let t0 = std::time::Instant::now();
    h.step();
    assert!(h.state_mut().wait_for_jobs(WAIT));
    h.run();
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "the job stopped at its next look at the flag: {:?}",
        t0.elapsed()
    );
    assert!(!h.state().is_busy());
    let keys = h.state().toast_keys();
    assert!(keys.contains(&"job-cancelled".to_owned()), "{keys:?}");
    assert!(
        !keys.contains(&"ws-toast-export-done".to_owned()),
        "a stopped export reports no success: {keys:?}"
    );
    assert!(!h.state().is_dirty(), "and changes nothing");
}

#[test]
fn the_bite_says_which_method_ran() {
    let _g = exclusive();
    for (method, key, sticky) in [
        ("bite_scan", "ws-toast-bite-scan", false),
        ("virtual_bite", "ws-toast-bite-estimated", true),
    ] {
        *METHOD.lock().unwrap() = method;
        let mut ws = fake();
        ws.run_leaf(BITE);
        assert!(ws.wait_for_jobs(WAIT));
        assert_eq!(ws.toast_keys(), [key], "{method}");
        assert_eq!(
            ws.sticky_toast_keys().len(),
            usize::from(sticky),
            "{method}"
        );
        // a bite is a change of the case: one undo step
        assert!(ws.undo());
    }
}

#[test]
fn the_selected_leaf_has_a_primary_button_that_runs_it() {
    let _g = exclusive();
    let mut ws = fake();
    ws.select_item(OTHER);
    let mut h = harness(ws);
    h.run();
    assert!(calls().is_empty(), "the selection runs nothing");
    assert!(h.query_by_label("Not done yet").is_some());
    h.get_by_label("Run").click();
    settle(&mut h);
    assert_eq!(calls(), [(OTHER, Value::Null)]);

    // a leaf that asks for a folder: the selection asks nothing, the button
    // asks again
    h.state_mut().select_item(EXPORT);
    h.run();
    assert_eq!(h.state().dialog_requests(), 0);
    h.get_by_label("Choose the folder and export…").click();
    h.run();
    assert_eq!(h.state().dialog_requests(), 1);
    assert!(
        h.query_by_label_contains("can take a long time").is_some(),
        "the line is in the inspector before the click"
    );
}

#[test]
fn a_step_already_done_offers_run_again_and_runs_only_from_it() {
    let _g = exclusive();
    OTHER_DONE.store(true, Ordering::SeqCst);
    let mut h = harness(fake());
    h.run();
    // the click on its row selects it: nothing runs again by itself (the
    // toolbar has a "3D teeth" toggle too)
    h.get_by_role_and_label(Role::TreeItem, "3D teeth").click();
    h.run();
    assert_eq!(h.state().selected(), Some(OTHER));
    assert!(!h.state().is_busy());
    assert!(calls().is_empty(), "{:?}", calls());
    assert!(h.query_by_label("Done").is_some());
    assert!(h.query_by_label("Run").is_none());
    // the secondary button runs it again
    h.get_by_label("Run again").click();
    settle(&mut h);
    assert_eq!(calls(), [(OTHER, Value::Null)]);
}

#[test]
fn after_the_cross_the_status_bar_says_it_is_stopping_until_the_job_ends() {
    let _g = exclusive();
    let mut ws = fake();
    SLOW.store(true, Ordering::SeqCst);
    STUBBORN.store(true, Ordering::SeqCst);
    ws.answer_leaf_input(EXPORT, Some(PathBuf::from("/tmp/dc-stubborn")));
    let mut h = harness(ws);
    h.step();
    h.step();
    assert!(h.query_by_label_contains("Stopping at the end").is_none());
    h.get_by_label("Cancel the background job").click();
    h.step();
    h.step();
    // the job is in its long step: the bar says so and offers no second cross
    assert!(h.state().is_busy());
    assert!(h.query_by_label_contains("Stopping at the end").is_some());
    assert!(h.query_by_label("Cancel the background job").is_none());
    STUBBORN.store(false, Ordering::SeqCst);
    assert!(h.state_mut().wait_for_jobs(WAIT));
    h.run();
    assert!(h.state().toast_keys().contains(&"job-cancelled".to_owned()));
    assert!(h.query_by_label_contains("Stopping at the end").is_none());
}
