//! The in-app scanner (alpha): the "Scan" button of "Import scans" (and of
//! the empty case) opens a popup that drives a 3D scanner and a turntable,
//! cleans the point cloud, builds the surface, repairs it and imports it
//! into the case.
//!
//! Every action of the popup is a `scan.*` or `turntable.*` command of
//! [`dc_scanner::control`], run on the one [`ScanController`] of the
//! workspace, so an agent connected to the app (`dentalcraft --control`,
//! [`Workspace::start_scan_command`]) drives the same scanner and turntable
//! the window shows. Commands run one at a time, in order, on the job
//! threads (the UI thread only reads the controller with `try_lock`); the
//! automatic scan and the calibration return at once and go on in the
//! background of the controller.
//!
//! `scan.import` is the host's: the job writes the surface to a temporary
//! file ([`ScanController::export_mesh_for_import`]) and the workspace then
//! imports that file with its normal import command, as one undoable change
//! with the import report, like a file chosen by hand.

mod popup;
mod select;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use dc_case::ScanRole;
use dc_jobs::JobHandle;
use dc_mesh::TriMesh;
use dc_scanner::ScanError;
use dc_scanner::control::{self, SCAN_IMPORT, ScanController};
use serde_json::{Value, json};

pub(crate) use popup::{ScanUi, scan_button, scan_button_width};

use crate::workspace::Workspace;

/// A scan command started by an agent ([`Workspace::start_scan_command`]):
/// its answer comes from [`Workspace::scan_result`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScanTicket(u64);

/// A scan command that failed: the stable code of the error
/// ([`ScanError::code`], or `busy` / `import`) and its technical detail.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScanFailure {
    pub code: String,
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

/// The error codes that have a sentence of their own (`ws-scan-err-<code>`).
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
    "ble-platform",
    "rig-uncalibrated",
];

impl ScanFailure {
    fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
        }
    }

    /// The i18n key of the sentence shown to the user.
    pub fn key(&self) -> String {
        if KNOWN_CODES.contains(&self.code.as_str()) {
            format!("ws-scan-err-{}", self.code)
        } else {
            "ws-scan-err-other".to_owned()
        }
    }

    /// What an agent reads: the code, then the detail.
    fn text(&self) -> String {
        format!("{}: {}", self.code, self.detail)
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

/// The command that runs.
struct Active {
    ticket: u64,
    id: String,
    origin: Origin,
    handle: JobHandle<Result<Value, ScanFailure>>,
}

/// The import into the case of a `scan.import`, which runs as a workspace
/// job after the export.
struct Importing {
    ticket: u64,
    origin: Origin,
    /// The surface written for the import (the popup knows it is in the case).
    mesh: Option<Arc<TriMesh>>,
}

/// The scanner of a workspace: the controller, the queue of its commands
/// and the popup.
pub(crate) struct ScanHost {
    controller: Arc<Mutex<ScanController>>,
    queue: VecDeque<Pending>,
    active: Option<Active>,
    importing: Option<Importing>,
    next_ticket: u64,
    /// Answers of the agents' commands, by ticket.
    results: HashMap<u64, Result<Value, ScanFailure>>,
    /// Where `scan.import` writes the surface it imports (created on first
    /// use, removed with the workspace).
    dir: PathBuf,
    pub ui: ScanUi,
}

impl Default for ScanHost {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ScanHost {
    /// The scanner of a workspace whose popup draws its 3D view on the GPU of
    /// `render_state` (`None`: headless, the view is a placeholder).
    pub fn new(render_state: Option<egui_wgpu::RenderState>) -> Self {
        Self {
            controller: Arc::new(Mutex::new(ScanController::new())),
            queue: VecDeque::new(),
            active: None,
            importing: None,
            next_ticket: 1,
            results: HashMap::new(),
            dir: std::env::temp_dir().join(format!("dentalcraft-scan-{}", uuid::Uuid::new_v4())),
            ui: ScanUi::new(render_state),
        }
    }

    /// `true` while a command runs or waits.
    pub fn busy(&self) -> bool {
        self.active.is_some() || self.importing.is_some() || !self.queue.is_empty()
    }

    /// Asks the command that runs to stop (the long computations look at
    /// the flag).
    pub fn cancel(&self) {
        if let Some(a) = &self.active {
            a.handle.cancel();
        }
    }

    /// Runs `f` on the controller when no command holds it (the UI thread
    /// never waits for a command).
    pub fn try_with<T>(&self, f: impl FnOnce(&ScanController) -> T) -> Option<T> {
        match self.controller.try_lock() {
            Ok(c) => Some(f(&c)),
            Err(TryLockError::Poisoned(p)) => Some(f(&p.into_inner())),
            Err(TryLockError::WouldBlock) => None,
        }
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
}

impl Drop for ScanHost {
    /// The workspace goes (its project closes, the app quits): the scanner
    /// and the turntable are let go, a running `scan.auto` or move included,
    /// even while a command still holds the controller (it is asked to stop
    /// and ends first). Otherwise that command's thread could outlive the
    /// process's last destructor and leave a Bluetooth turntable linked to
    /// this computer.
    fn drop(&mut self) {
        self.cancel();
        self.controller
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close();
        // the files written for the imports (already read into the case)
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The role named by `scan.import`'s export result.
fn role_of(name: Option<&str>) -> Option<ScanRole> {
    match name? {
        "upper" => Some(ScanRole::Upper),
        "lower" => Some(ScanRole::Lower),
        "bite" => Some(ScanRole::Bite),
        _ => None,
    }
}

/// The scan and turntable commands that only read: an agent asking them
/// does not open the popup.
fn reads_only(id: &str) -> bool {
    matches!(
        id,
        control::SCAN_DEVICES
            | control::SCAN_STATUS
            | control::TURNTABLE_DEVICES
            | control::TURNTABLE_STATUS
            | control::TURNTABLE_CALIBRATION
    )
}

impl Workspace {
    /// Opens the scan popup (the "Scan" button): it lists the scanners and
    /// the turntables and nothing else; the acquisition starts with its
    /// Start button.
    pub fn open_scan(&mut self) {
        self.scan.ui.open = true;
        self.scan_command(control::SCAN_DEVICES, Value::Null);
        self.scan_command(
            control::TURNTABLE_DEVICES,
            json!({ "timeout_s": popup::OPEN_SEARCH_S }),
        );
    }

    /// `true` while the scan popup is open.
    pub fn is_scan_open(&self) -> bool {
        self.scan.ui.open
    }

    /// `true` while a scan command runs or waits.
    pub fn is_scan_busy(&self) -> bool {
        self.scan.busy()
    }

    /// Starts the scan command `id` (a `scan.*` id of
    /// [`dc_scanner::control::command_infos`]) for an agent, after the scan
    /// commands already asked for. A command that acts on the scanner opens
    /// the popup, so the user sees what the agent does. The answer comes
    /// from [`Workspace::scan_result`].
    ///
    /// # Errors
    /// An unknown id.
    pub fn start_scan_command(&mut self, id: &str, params: Value) -> Result<ScanTicket, String> {
        if !control::command_infos().iter().any(|i| i.id == id) {
            return Err(format!("unknown scan command {id}"));
        }
        if !reads_only(id) {
            self.scan.ui.open = true;
        }
        let ticket = self.scan.enqueue(id, params, Origin::Agent);
        self.pump_scan();
        Ok(ScanTicket(ticket))
    }

    /// The answer of the agent's command `ticket` once it is done (the
    /// result of the command, or its error as `code: detail`); `None` while
    /// it runs or waits. `scan.import` answers with the import report.
    pub fn scan_result(&mut self, ticket: ScanTicket) -> Option<Result<Value, String>> {
        self.poll();
        self.scan
            .results
            .remove(&ticket.0)
            .map(|r| r.map_err(|f| f.text()))
    }

    /// Runs the scan command `id` for a control of the popup.
    pub(crate) fn scan_command(&mut self, id: &str, params: Value) {
        self.scan.enqueue(id, params, Origin::Ui);
        self.pump_scan();
    }

    /// Applies the scan command that ended and starts the next one. Called
    /// by [`Workspace::poll`].
    pub(crate) fn poll_scan(&mut self) {
        if self
            .scan
            .active
            .as_ref()
            .is_some_and(|a| a.handle.is_finished())
        {
            let Some(a) = self.scan.active.take() else {
                return;
            };
            let r = match a.handle.try_take() {
                Some(Ok(r)) => r,
                Some(Err(e)) => Err(ScanFailure::new("device", e.to_string())),
                None => Err(ScanFailure::new("device", "the command did not finish")),
            };
            self.scan_ended(a.ticket, &a.id, a.origin, r);
        }
        self.pump_scan();
    }

    /// Starts the next command when none runs.
    fn pump_scan(&mut self) {
        if self.scan.active.is_some() || self.scan.importing.is_some() {
            return;
        }
        let Some(p) = self.scan.queue.pop_front() else {
            return;
        };
        let controller = Arc::clone(&self.scan.controller);
        let dir = self.scan.dir.clone();
        let (id, params) = (p.id.clone(), p.params);
        let handle = self
            .jobs
            .spawn("ws-scan-job", move |ctx| -> Result<Value, ScanFailure> {
                // looking for turntables takes seconds and needs no
                // controller: the live view keeps reading it meanwhile
                if id == control::TURNTABLE_DEVICES {
                    return dc_scanner::rig::turntable_devices(&params).map_err(ScanFailure::from);
                }
                let mut c = controller.lock().unwrap_or_else(PoisonError::into_inner);
                if id == SCAN_IMPORT {
                    let role = control::import_role(&params)?;
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| ScanFailure::new("import", e.to_string()))?;
                    let path = c.export_mesh_for_import(&dir, role)?;
                    Ok(json!({ "path": path.to_string_lossy(), "role": role }))
                } else {
                    c.execute(&id, &params, &|| ctx.is_cancelled())
                        .map_err(ScanFailure::from)
                }
            });
        self.scan.active = Some(Active {
            ticket: p.ticket,
            id: p.id,
            origin: p.origin,
            handle,
        });
    }

    /// A command ended: the popup follows it, the export of `scan.import`
    /// goes on with the import into the case, an agent gets its answer.
    fn scan_ended(&mut self, ticket: u64, id: &str, origin: Origin, r: Result<Value, ScanFailure>) {
        if id == SCAN_IMPORT
            && let Ok(out) = &r
        {
            let path = PathBuf::from(out["path"].as_str().unwrap_or_default());
            let role = role_of(out["role"].as_str());
            let mesh = self.scan.ui.shown_mesh();
            match self.import_scan_file(path, role, ticket) {
                Ok(()) => {
                    self.scan.importing = Some(Importing {
                        ticket,
                        origin,
                        mesh,
                    });
                }
                Err(f) => {
                    self.scan.ui.failed(f.clone());
                    self.scan.finish(ticket, origin, Err(f));
                }
            }
            return;
        }
        self.scan.ui.after(id, &r);
        self.scan.finish(ticket, origin, r);
    }

    /// The import of a `scan.import` ended (the workspace job, see
    /// `SpecKind::ScanImport`): `out` is the import report.
    pub(crate) fn scan_import_finished(&mut self, ticket: u64, r: Result<Value, String>) {
        let Some(imp) = self.scan.importing.take_if(|i| i.ticket == ticket) else {
            return;
        };
        // the case holds the scan now (or the import failed): the file goes
        let _ = std::fs::remove_dir_all(&self.scan.dir);
        let r = r.map_err(|e| ScanFailure::new("import", e));
        match &r {
            Ok(_) => self.scan.ui.imported(imp.mesh),
            Err(f) => self.scan.ui.failed(f.clone()),
        }
        self.scan.finish(ticket, imp.origin, r);
        self.pump_scan();
    }

    /// Closes the popup: the scan is discarded, the scanner and the
    /// turntable disconnected.
    pub(crate) fn close_scan(&mut self) {
        self.scan.ui.close();
        self.scan_command(control::SCAN_RESET, Value::Null);
        self.scan_command(control::SCAN_DISCONNECT, Value::Null);
        self.scan_command(control::TURNTABLE_DISCONNECT, Value::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_code_has_a_sentence() {
        let en = include_str!("../../i18n/en/dc-workspace.ftl");
        for code in KNOWN_CODES {
            let key = ScanFailure::new(code, "").key();
            assert!(en.contains(&format!("{key} = ")), "{key}");
        }
        assert_eq!(ScanFailure::new("strange", "").key(), "ws-scan-err-other");
        assert!(en.contains("ws-scan-err-other = "));
    }

    #[test]
    fn roles_of_the_export() {
        assert_eq!(role_of(Some("lower")), Some(ScanRole::Lower));
        assert_eq!(role_of(None), None);
        assert_eq!(role_of(Some("x")), None);
    }

    #[test]
    fn dropping_the_host_lets_go_of_the_turntable_even_while_held_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let host = ScanHost::new(None);
        // stands for the thread of a command that still holds the controller
        let held = Arc::clone(&host.controller);
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
        drop(host);
        assert!(!rig.attached(), "the turntable stayed open");
        assert!(!held.lock().unwrap().turntable_status().connected);
    }
}
