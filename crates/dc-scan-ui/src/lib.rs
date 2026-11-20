//! The scan popup (alpha), a component any egui screen opens: it drives a 3D
//! scanner and a turntable, cleans the point cloud, builds the surface,
//! repairs it and hands the surface back to the screen that opened it as a
//! file with its role ([`ScanOutput`]). What the file becomes is the host's
//! business: the workspace imports it into the case, the new-project screen
//! adds it to the scans of the project.
//!
//! Every action of the popup is a `scan.*` or `turntable.*` command of
//! [`dc_scanner::control`], run on the one [`ScanController`] of the popup, so
//! an agent connected to the app ([`ScanPopup::start_command`]) drives the
//! same scanner and turntable the window shows. Commands run one at a time,
//! in order, on the popup's own job thread (the UI thread only reads the
//! controller with `try_lock`); the automatic scan and the calibration return
//! at once and go on in the background of the controller.
//!
//! # Hosting
//!
//! ```ignore
//! // once (`None`: no GPU, the 3D view is a placeholder)
//! let mut scan = ScanPopup::new(Some(render_state.clone()));
//! // the button only opens the window: the acquisition starts from its Start
//! if dc_scan_ui::scan_button(ui, true) {
//!     scan.open(ScanTarget::new(roles));
//! }
//! // every frame
//! scan.poll();
//! for out in scan.take_outputs() {
//!     // read or import `out.path()` with `out.role()`, then answer
//!     scan.output_done(out.id(), Ok(report));
//!     // keep `out` until the file is read: dropping it deletes the file
//! }
//! scan.show(ui);
//! ```
//!
//! # `scan.import` and the file it writes
//!
//! The step "Import" of the popup and an agent's `scan.import` write the
//! surface to a file in a temporary folder of its own
//! ([`ScanController::export_mesh_for_import`]); the popup hands it over as a
//! [`ScanOutput`], which owns that folder: the file exists while the host
//! keeps the output and is deleted when the output is dropped. The popup
//! waits (busy, later commands queued) until the host answers every output
//! with [`ScanPopup::output_done`]: that answer is what an agent's
//! `scan.import` returns and what the popup shows (the "done" badge or the
//! error).

mod popup;
mod select;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use dc_case::ScanRole;
use dc_jobs::{JobHandle, JobSystem};
use dc_mesh::TriMesh;
use dc_scanner::ScanError;
use dc_scanner::control::{self, SCAN_IMPORT, ScanController};
use dc_ui::theme::colors;
use egui::{RichText, Ui};
use serde_json::{Value, json};
use tempfile::TempDir;

pub use popup::{restore_steps_width, scan_button, scan_button_width, take_resized_steps_width};

use popup::ScanUi;

dc_i18n::bundle!(fn loader);

/// Translates `key` (any registered bundle; this crate's is registered on
/// first use).
fn t(key: &str) -> String {
    loader();
    dc_i18n::tr(key)
}

/// Translates `key` with its number `$n`.
fn tr_n(key: &str, n: String) -> String {
    loader();
    dc_i18n::tr_args(key, &[("n", &n)])
}

/// A small secondary line of text.
fn caption(ui: &mut Ui, text: impl Into<String>) {
    let c = *colors(ui);
    ui.label(
        RichText::new(text.into())
            .font(dc_ui::fonts::ui(12.0))
            .color(c.text_secondary),
    );
}

/// A token colour as the 3D view takes it.
fn scene_color(c: egui::Color32) -> dc_scene::Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    dc_scene::Color::rgba(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    )
}

/// A scan command started by an agent ([`ScanPopup::start_command`]): its
/// answer comes from [`ScanPopup::take_result`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScanTicket(u64);

/// Which [`ScanOutput`] the host answers with [`ScanPopup::output_done`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScanOutputId(u64);

/// A scan command that failed: the stable code of the error
/// ([`ScanError::code`], or `busy` / `import` from the host) and its
/// technical detail.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanFailure {
    /// Stable code (`device`, `no-mesh`, `busy`, `import`, …).
    pub code: String,
    /// Technical detail, untranslated.
    pub detail: String,
}

impl From<ScanError> for ScanFailure {
    fn from(e: ScanError) -> Self {
        Self {
            code: e.code().to_owned(),
            detail: e.to_string(),
        }
    }
}

/// The error codes that have a sentence of their own (`scan-err-<code>`).
const KNOWN_CODES: &[&str] = &[
    "unknown-backend",
    "device-not-found",
    "device",
    "not-connected",
    "invalid-state",
    "invalid-params",
    "empty",
    "cancelled",
    "geometry",
    "mesh",
    "no-mesh",
    "busy",
    "import",
    "sdk-missing",
    "sdk-unauthorized",
    "platform-unsupported",
    "platform-capture-denied",
    "device-lost",
    "turntable-not-connected",
    "turntable-out-of-range",
    "turntable-busy",
    "turntable-protocol",
    "ble-no-adapter",
    "ble-adapter-off",
    "ble-unsupported",
    "ble-not-found",
    "ble-timeout",
    "ble-disconnected",
    "ble-unauthorized",
    "ble-app-bundle-required",
    "ble-platform",
    "rig-uncalibrated",
    "calibration",
    "calibration-no-object",
    "calibration-tracking",
    "calibration-tilt",
    "calibration-inaccurate",
    "calibration-implausible",
    "scan-quality-low",
    "scan-no-stops",
];

impl ScanFailure {
    /// The failure `code` with its `detail`.
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
        }
    }

    /// The i18n key of the sentence shown to the user.
    pub fn key(&self) -> String {
        if KNOWN_CODES.contains(&self.code.as_str()) {
            format!("scan-err-{}", self.code)
        } else {
            "scan-err-other".to_owned()
        }
    }

    /// What an agent reads: the code, then the detail.
    fn text(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

/// The roles offered when the opener names none, in this order.
const ALL_ROLES: [ScanRole; 3] = [ScanRole::Upper, ScanRole::Lower, ScanRole::Bite];

/// What the popup is opened for: the roles the scan can be handed over with
/// and, optionally, the words of the host.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanTarget {
    /// The roles the scan can take, in the order of the selector of the step
    /// "Import" (the first is preselected; one role: no selector). Usually
    /// the roles a module declares (`dc_module_api::ScannerInput`); empty:
    /// upper, lower and bite.
    pub roles: Vec<ScanRole>,
    /// What the scan is for, shown beside the title (translated by the host).
    pub context: Option<String>,
    /// The words of the step "Import" (translated by the host); `None`: the
    /// popup's own ("Import into the case").
    pub words: Option<ImportWords>,
}

impl Default for ScanTarget {
    fn default() -> Self {
        Self::new(ALL_ROLES)
    }
}

impl ScanTarget {
    /// A scan handed over with one of `roles`.
    pub fn new(roles: impl Into<Vec<ScanRole>>) -> Self {
        Self {
            roles: roles.into(),
            context: None,
            words: None,
        }
    }

    /// The same target with `context` beside the title.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// The same target with the host's words for the step "Import".
    #[must_use]
    pub fn with_words(mut self, words: ImportWords) -> Self {
        self.words = Some(words);
        self
    }

    /// The roles offered (all three when none is named).
    fn offered(&self) -> &[ScanRole] {
        if self.roles.is_empty() {
            &ALL_ROLES
        } else {
            &self.roles
        }
    }
}

/// The words of the step "Import" when the host does something else than
/// importing into the case (e.g. adding the scan to a new project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWords {
    /// The button that hands the scan over.
    pub button: String,
    /// The line under it: what happens to the scan.
    pub hint: String,
    /// The badge once the host took the scan.
    pub done: String,
    /// The question when the window closes on a scan not handed over yet.
    pub unsaved: String,
}

/// A surface the popup wrote for the host (`scan.import`): the file, its role
/// and the temporary folder that holds it. The file lives as long as this
/// value: drop it once the file is read (see the crate documentation).
#[derive(Debug)]
pub struct ScanOutput {
    id: ScanOutputId,
    path: PathBuf,
    role: Option<ScanRole>,
    _dir: TempDir,
}

impl ScanOutput {
    /// What [`ScanPopup::output_done`] answers.
    pub fn id(&self) -> ScanOutputId {
        self.id
    }

    /// The mesh file (PLY).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The role chosen in the popup; `None` when an agent's `scan.import`
    /// named none (the host's import decides).
    pub fn role(&self) -> Option<ScanRole> {
        self.role
    }
}

/// Who asked for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// A button of the popup.
    Ui,
    /// An agent over the control bridge: the answer is kept for its ticket.
    Agent,
}

/// A command waiting for its turn.
struct Pending {
    ticket: u64,
    id: String,
    params: Value,
    origin: Origin,
}

/// The popup's own step on its command thread, not a scan command: it
/// finalises a rig job that ended ([`ScanController::finish_rig_job`]).
const FINISH_RIG_JOB: &str = "popup.finish-rig-job";

/// What a command's job returns: its answer and, for `scan.import`, the
/// folder of the file it wrote.
type JobOut = Result<(Value, Option<TempDir>), ScanFailure>;

/// The command that runs.
struct Active {
    ticket: u64,
    id: String,
    origin: Origin,
    handle: JobHandle<JobOut>,
}

/// A `scan.import` whose file the host has, until it answers.
struct Awaiting {
    ticket: u64,
    origin: Origin,
    /// The surface written (the popup shows it handed over).
    mesh: Option<Arc<TriMesh>>,
}

/// The scan popup and its scanner: the controller, the queue of its commands,
/// the files waiting for the host and the window.
pub struct ScanPopup {
    controller: Arc<Mutex<ScanController>>,
    /// The thread of the commands (made on first use).
    jobs: Option<JobSystem>,
    queue: VecDeque<Pending>,
    active: Option<Active>,
    awaiting: Option<Awaiting>,
    outputs: Vec<ScanOutput>,
    next_ticket: u64,
    /// Answers of the agents' commands, by ticket.
    results: HashMap<u64, Result<Value, ScanFailure>>,
    ui: ScanUi,
}

impl Default for ScanPopup {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ScanPopup {
    /// A closed popup whose 3D view draws on the GPU of `render_state`
    /// (`None`: headless, the view is a placeholder).
    pub fn new(render_state: Option<egui_wgpu::RenderState>) -> Self {
        Self {
            controller: Arc::new(Mutex::new(ScanController::new())),
            jobs: None,
            queue: VecDeque::new(),
            active: None,
            awaiting: None,
            outputs: Vec::new(),
            next_ticket: 1,
            results: HashMap::new(),
            ui: ScanUi::new(render_state),
        }
    }

    /// Opens the window (the "Scan" button) for `target`: it lists the
    /// scanners and the turntables and nothing else; the acquisition starts
    /// with its Start button.
    pub fn open(&mut self, target: ScanTarget) {
        self.set_target(target);
        self.ui.open = true;
        self.command(control::SCAN_DEVICES, Value::Null);
        self.command(
            control::TURNTABLE_DEVICES,
            json!({ "timeout_s": popup::OPEN_SEARCH_S }),
        );
    }

    /// Sets what the popup is for without opening it (what an agent's
    /// command opens it with).
    pub fn set_target(&mut self, target: ScanTarget) {
        self.ui.set_target(target);
    }

    /// `true` while the window is open.
    pub fn is_open(&self) -> bool {
        self.ui.open
    }

    /// `true` while a command runs or waits, or a file waits for the host.
    pub fn is_busy(&self) -> bool {
        self.active.is_some() || self.awaiting.is_some() || !self.queue.is_empty()
    }

    /// Closes the window: the scan is discarded, the scanner and the
    /// turntable disconnected (the files already handed over stay with the
    /// host).
    pub fn close(&mut self) {
        self.ui.close();
        self.command(control::SCAN_RESET, Value::Null);
        self.command(control::SCAN_DISCONNECT, Value::Null);
        self.command(control::TURNTABLE_DISCONNECT, Value::Null);
    }

    /// Starts the scan command `id` (a `scan.*` or `turntable.*` id of
    /// [`dc_scanner::control::command_infos`]) for an agent, after the
    /// commands already asked for. A command that acts on the scanner opens
    /// the window, so the user sees what the agent does. The answer comes
    /// from [`ScanPopup::take_result`]; `scan.import` answers with what the
    /// host answered for its file ([`ScanPopup::output_done`]).
    ///
    /// # Errors
    /// An unknown id.
    pub fn start_command(&mut self, id: &str, params: Value) -> Result<ScanTicket, String> {
        if !control::command_infos().iter().any(|i| i.id == id) {
            return Err(format!("unknown scan command {id}"));
        }
        if !reads_only(id) {
            self.ui.open = true;
        }
        let ticket = self.enqueue(id, params, Origin::Agent);
        self.pump();
        Ok(ScanTicket(ticket))
    }

    /// The answer of the agent's command `ticket` once it is done (the result
    /// of the command, or its error as `code: detail`); `None` while it runs
    /// or waits. It does not poll: call [`ScanPopup::poll`] (and hand over
    /// the outputs) first.
    pub fn take_result(&mut self, ticket: ScanTicket) -> Option<Result<Value, String>> {
        self.results
            .remove(&ticket.0)
            .map(|r| r.map_err(|f| f.text()))
    }

    /// Applies the command that ended and starts the next one. Call it every
    /// frame (and while waiting for an agent's answer), then
    /// [`ScanPopup::take_outputs`].
    pub fn poll(&mut self) {
        if self.active.as_ref().is_some_and(|a| a.handle.is_finished()) {
            let Some(a) = self.active.take() else {
                return;
            };
            let r = match a.handle.try_take() {
                Some(Ok(r)) => r,
                Some(Err(e)) => Err(ScanFailure::new("device", e.to_string())),
                None => Err(ScanFailure::new("device", "the command did not finish")),
            };
            self.ended(a.ticket, &a.id, a.origin, r);
        }
        // a rig job that ended leaves the acquisition held, as it ran, until
        // a command finalises it: without one the window would go on showing
        // a scan under way that fuses nothing
        if !self.is_busy() && self.try_with(ScanController::rig_job_ended) == Some(true) {
            self.enqueue(FINISH_RIG_JOB, Value::Null, Origin::Ui);
        }
        self.pump();
    }

    /// The surfaces written for the host since the last call. Each one must
    /// be answered with [`ScanPopup::output_done`]; the popup runs nothing
    /// else until then.
    pub fn take_outputs(&mut self) -> Vec<ScanOutput> {
        std::mem::take(&mut self.outputs)
    }

    /// The host is done with the output `id`: `Ok` with what it did (the
    /// import report: an agent's `scan.import` answers it), or why it could
    /// not take it (`busy`, `import`). The popup shows it and goes on with
    /// the next command.
    pub fn output_done(&mut self, id: ScanOutputId, r: Result<Value, ScanFailure>) {
        let Some(a) = self.awaiting.take_if(|a| a.ticket == id.0) else {
            return;
        };
        match &r {
            Ok(_) => self.ui.imported(a.mesh),
            Err(f) => {
                log_failure(SCAN_IMPORT, f);
                self.ui.failed(f.clone());
            }
        }
        self.finish(a.ticket, a.origin, r);
        self.pump();
    }

    /// Asks the command that runs to stop (the long computations look at
    /// the flag).
    fn cancel(&self) {
        if let Some(a) = &self.active {
            a.handle.cancel();
        }
    }

    /// Runs `f` on the controller when no command holds it (the UI thread
    /// never waits for a command).
    fn try_with<T>(&self, f: impl FnOnce(&ScanController) -> T) -> Option<T> {
        match self.controller.try_lock() {
            Ok(c) => Some(f(&c)),
            Err(TryLockError::Poisoned(p)) => Some(f(&p.into_inner())),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Runs the command `id` for a control of the window.
    fn command(&mut self, id: &str, params: Value) {
        self.enqueue(id, params, Origin::Ui);
        self.pump();
    }

    fn enqueue(&mut self, id: &str, params: Value, origin: Origin) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.queue.push_back(Pending {
            ticket,
            id: id.to_owned(),
            params,
            origin,
        });
        ticket
    }

    fn finish(&mut self, ticket: u64, origin: Origin, r: Result<Value, ScanFailure>) {
        if origin == Origin::Agent {
            self.results.insert(ticket, r);
        }
    }

    /// Starts the next command when none runs and no file waits for the
    /// host.
    fn pump(&mut self) {
        if self.active.is_some() || self.awaiting.is_some() {
            return;
        }
        let Some(p) = self.queue.pop_front() else {
            return;
        };
        if self.jobs.is_none() {
            match JobSystem::new(1) {
                Ok(jobs) => self.jobs = Some(jobs),
                Err(e) => {
                    let f = ScanFailure::new("device", e.to_string());
                    log_failure(&p.id, &f);
                    let r = Err(f);
                    self.ui.after(&p.id, &r);
                    self.finish(p.ticket, p.origin, r);
                    return;
                }
            }
        }
        let Some(jobs) = &self.jobs else {
            return;
        };
        let controller = Arc::clone(&self.controller);
        let (id, params) = (p.id.clone(), p.params);
        let handle = jobs.spawn("scan-job", move |ctx| -> JobOut {
            // looking for turntables takes seconds and needs no controller:
            // the live view keeps reading it meanwhile
            if id == control::TURNTABLE_DEVICES {
                return dc_scanner::rig::turntable_devices(&params)
                    .map(|v| (v, None))
                    .map_err(ScanFailure::from);
            }
            let mut c = controller.lock().unwrap_or_else(PoisonError::into_inner);
            if id == FINISH_RIG_JOB {
                c.finish_rig_job();
                return Ok((Value::Null, None));
            }
            if id == SCAN_IMPORT {
                let role = control::import_role(&params)?;
                let dir = tempfile::Builder::new()
                    .prefix("dentalcraft-scan-")
                    .tempdir()
                    .map_err(|e| ScanFailure::new("import", e.to_string()))?;
                let path = c.export_mesh_for_import(dir.path(), role)?;
                Ok((
                    json!({ "path": path.to_string_lossy(), "role": role }),
                    Some(dir),
                ))
            } else {
                c.execute(&id, &params, &|| ctx.is_cancelled())
                    .map(|v| (v, None))
                    .map_err(ScanFailure::from)
            }
        });
        self.active = Some(Active {
            ticket: p.ticket,
            id: p.id,
            origin: p.origin,
            handle,
        });
    }

    /// A command ended: the window follows it, the file of `scan.import`
    /// goes to the host, an agent gets its answer.
    fn ended(&mut self, ticket: u64, id: &str, origin: Origin, r: JobOut) {
        if id == FINISH_RIG_JOB {
            // the window reads what the job left from the controller
            return;
        }
        let r = match r {
            Ok((out, Some(dir))) => {
                self.outputs.push(ScanOutput {
                    id: ScanOutputId(ticket),
                    path: PathBuf::from(out["path"].as_str().unwrap_or_default()),
                    role: role_of(out["role"].as_str()),
                    _dir: dir,
                });
                self.awaiting = Some(Awaiting {
                    ticket,
                    origin,
                    mesh: self.ui.shown_mesh(),
                });
                return;
            }
            r => r.map(|(out, _)| out),
        };
        if let Err(f) = &r {
            log_failure(id, f);
        }
        self.ui.after(id, &r);
        // `scan.close` ended the session on the controller: the window goes
        // with it (refused while a scan or a job runs: it stays)
        if id == control::SCAN_CLOSE && r.is_ok() {
            self.ui.close();
        }
        self.finish(ticket, origin, r);
    }
}

/// Logs a failed scanner or turntable command for the log file and the
/// anonymous diagnostics: a fault as an error, a missing or switched-off
/// device as a flagged warning; a cancel or a busy device is no failure.
fn log_failure(command: &str, f: &ScanFailure) {
    const SETUP: [&str; 13] = [
        "device-not-found",
        "not-connected",
        "turntable-not-connected",
        "ble-no-adapter",
        "ble-adapter-off",
        "ble-not-found",
        "sdk-missing",
        "platform-unsupported",
        "platform-capture-denied",
        "rig-uncalibrated",
        "calibration-no-object",
        "invalid-params",
        "empty",
    ];
    let code = f.code.as_str();
    // a refused `scan.close` (a scan or a job runs) is an answer, no fault
    if matches!(code, "cancelled" | "busy" | "turntable-busy")
        || (command == control::SCAN_CLOSE && code == "invalid-state")
    {
        return;
    }
    let source = if ["turntable", "ble", "rig"]
        .iter()
        .any(|p| code.starts_with(p))
        || command.starts_with("turntable.")
    {
        "turntable"
    } else {
        "scan"
    };
    if SETUP.contains(&code) {
        tracing::warn!(
            diag = true,
            code = %format!("scan.{code}"),
            source,
            command,
            "{command}: {}",
            f.detail
        );
    } else {
        tracing::error!(
            code = %format!("scan.{code}"),
            source,
            command,
            "{command}: {}",
            f.detail
        );
    }
}

impl Drop for ScanPopup {
    /// The popup goes (its screen closes, the app quits): the scanner and the
    /// turntable are let go, a running `scan.auto` or move included, even
    /// while a command still holds the controller (it is asked to stop and
    /// ends first). Otherwise that command's thread could outlive the
    /// process's last destructor and leave a Bluetooth turntable linked to
    /// this computer. The files not taken by the host go with it.
    fn drop(&mut self) {
        self.cancel();
        self.controller
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close();
    }
}

/// The name of `role` in `scan.import`'s parameters.
fn role_id(role: ScanRole) -> &'static str {
    match role {
        ScanRole::Upper => "upper",
        ScanRole::Lower => "lower",
        ScanRole::Bite => "bite",
    }
}

/// The role named by `scan.import`'s export result.
fn role_of(name: Option<&str>) -> Option<ScanRole> {
    let name = name?;
    ALL_ROLES.into_iter().find(|&r| role_id(r) == name)
}

/// The scan and turntable commands that only read, or that close the
/// window: an agent asking them does not open it.
fn reads_only(id: &str) -> bool {
    matches!(
        id,
        control::SCAN_DEVICES
            | control::SCAN_STATUS
            | control::TURNTABLE_DEVICES
            | control::TURNTABLE_STATUS
            | control::TURNTABLE_CALIBRATION
            | control::SCAN_CLOSE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_it_parity() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }

    #[test]
    fn every_error_code_has_a_sentence() {
        let en = include_str!("../i18n/en/dc-scan-ui.ftl");
        for code in KNOWN_CODES {
            let key = ScanFailure::new(code, "").key();
            assert!(en.contains(&format!("{key} = ")), "{key}");
        }
        assert_eq!(ScanFailure::new("strange", "").key(), "scan-err-other");
        assert!(en.contains("scan-err-other = "));
    }

    #[test]
    fn roles_of_the_export() {
        assert_eq!(role_of(Some("lower")), Some(ScanRole::Lower));
        assert_eq!(role_of(None), None);
        assert_eq!(role_of(Some("x")), None);
        for r in ALL_ROLES {
            assert_eq!(role_of(Some(role_id(r))), Some(r));
        }
    }

    #[test]
    fn a_target_without_roles_offers_all_three() {
        assert_eq!(ScanTarget::new([]).offered(), &ALL_ROLES);
        assert_eq!(ScanTarget::default().offered(), &ALL_ROLES);
        let only = ScanTarget::new([ScanRole::Bite]).with_context("PT-1");
        assert_eq!(only.offered(), &[ScanRole::Bite]);
        assert_eq!(only.context.as_deref(), Some("PT-1"));
    }

    #[test]
    fn dropping_the_popup_lets_go_of_the_turntable_even_while_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let popup = ScanPopup::new(None);
        // stands for the thread of a command that still holds the controller
        let held = Arc::clone(&popup.controller);
        let rig = {
            let mut c = held.lock().unwrap();
            c.set_rig_dir(dir.path());
            c.execute(
                control::TURNTABLE_CONNECT,
                &json!({"backend": "sim"}),
                &|| false,
            )
            .unwrap();
            c.execute(
                control::TURNTABLE_MOVE,
                &json!({"axis": "rotate", "by_deg": 90.0}),
                &|| false,
            )
            .unwrap();
            Arc::clone(c.sim_rig())
        };
        assert!(rig.attached());
        drop(popup);
        assert!(!rig.attached(), "the turntable stayed open");
        assert!(!held.lock().unwrap().turntable_status().connected);
    }

    /// Runs `id` on `popup` as an agent does and waits for its answer.
    fn run(popup: &mut ScanPopup, id: &str, params: Value) -> Value {
        let ticket = popup.start_command(id, params).unwrap();
        let start = std::time::Instant::now();
        loop {
            popup.poll();
            if let Some(r) = popup.take_result(ticket) {
                return r.unwrap_or_else(|e| panic!("{id}: {e}"));
            }
            assert!(start.elapsed().as_secs() < 300, "{id} did not end");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_rig_job_that_ended_is_finalised_while_the_window_only_polls() {
        use dc_scanner::{RigJobState, ScanState};
        let dir = tempfile::tempdir().unwrap();
        let mut popup = ScanPopup::new(None);
        popup.controller.lock().unwrap().set_rig_dir(dir.path());
        let sim =
            json!({"backend": "sim", "options": {"realtime": false, "width": 160, "height": 100}});
        run(&mut popup, control::SCAN_CONNECT, sim);
        run(
            &mut popup,
            control::TURNTABLE_CONNECT,
            json!({"backend": "sim", "options": {"fast": true}}),
        );
        run(
            &mut popup,
            control::TURNTABLE_CALIBRATE,
            json!({"steps": 6, "step_deg": 30, "frames_per_stop": 1, "settle_s": 0}),
        );
        // from here on nothing but the window's polling: once the job ends
        // the acquisition it held stops, without waiting for a command
        let start = std::time::Instant::now();
        loop {
            popup.poll();
            let seen = popup.try_with(|c| {
                (
                    c.calibration_job().map(|j| j.state),
                    c.session().status().state,
                )
            });
            if let Some((Some(job), ScanState::Connected)) = seen
                && !matches!(job, RigJobState::Running | RigJobState::Paused)
            {
                break;
            }
            assert!(
                start.elapsed().as_secs() < 120,
                "the calibration ended, the acquisition did not: {seen:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(popup.try_with(ScanController::rig_job_ended) == Some(false));
    }
}
