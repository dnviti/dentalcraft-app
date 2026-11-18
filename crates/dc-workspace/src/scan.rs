//! The in-app scanner (alpha) of the workspace: the "Scan" button of the
//! leaf whose scans can come from a scanner (the module declares it,
//! [`dc_module_api::ScannerInput`]) and of the empty case opens the generic
//! scan popup ([`dc_scan_ui::ScanPopup`]) with the roles the module accepts.
//!
//! The popup hands each surface back as a file
//! ([`dc_scan_ui::ScanOutput`]); the workspace imports it with its normal
//! import command, as one undoable change with the import report, like a
//! file chosen by hand, keeps the file until the import job has read it,
//! and tells the popup how the import went. An agent connected to the app
//! (`dentalcraft --control`, [`Workspace::start_scan_command`]) drives the
//! same popup.

use dc_scan_ui::{ScanFailure, ScanOutput, ScanOutputId, ScanTarget, ScanTicket};
use serde_json::Value;

use crate::workspace::Workspace;

impl Workspace {
    /// What the popup is for: the roles the module accepts for a scan from
    /// the scanner (all three when it declares none).
    fn scan_target(&self) -> ScanTarget {
        let roles = self
            .import_command_id()
            .and_then(|id| self.menu.find(id))
            .and_then(|item| item.scanner)
            .map_or_else(Vec::new, |s| s.roles.to_vec());
        ScanTarget::new(roles)
    }

    /// `true` when the module lets the scans of the case come from a scanner
    /// (the "Scan" button of the empty case).
    pub(crate) fn offers_scanner(&self) -> bool {
        self.import_command_id()
            .and_then(|id| self.menu.find(id))
            .is_some_and(|item| item.scanner.is_some())
    }

    /// Opens the scan popup (the "Scan" button): it lists the scanners and
    /// the turntables and nothing else; the acquisition starts with its
    /// Start button.
    pub fn open_scan(&mut self) {
        let target = self.scan_target();
        self.scan.open(target);
    }

    /// `true` while the scan popup is open.
    pub fn is_scan_open(&self) -> bool {
        self.scan.is_open()
    }

    /// `true` while a scan command runs or waits.
    pub fn is_scan_busy(&self) -> bool {
        self.scan.is_busy()
    }

    /// Starts the scan command `id` (a `scan.*` id of
    /// `dc_scanner::control::command_infos`) for an agent, after the scan
    /// commands already asked for. A command that acts on the scanner opens
    /// the popup, so the user sees what the agent does. The answer comes
    /// from [`Workspace::scan_result`].
    ///
    /// # Errors
    /// An unknown id.
    pub fn start_scan_command(&mut self, id: &str, params: Value) -> Result<ScanTicket, String> {
        if !self.scan.is_open() {
            let target = self.scan_target();
            self.scan.set_target(target);
        }
        self.scan.start_command(id, params)
    }

    /// The answer of the agent's command `ticket` once it is done (the
    /// result of the command, or its error as `code: detail`); `None` while
    /// it runs or waits. `scan.import` answers with the import report.
    pub fn scan_result(&mut self, ticket: ScanTicket) -> Option<Result<Value, String>> {
        self.poll();
        self.scan.take_result(ticket)
    }

    /// Moves the popup on and imports the surfaces it handed over. Called by
    /// [`Workspace::poll`].
    pub(crate) fn poll_scan(&mut self) {
        self.scan.poll();
        for out in self.scan.take_outputs() {
            self.import_scan_output(out);
        }
    }

    /// The import of the surface `id` ended (the workspace job, see
    /// `SpecKind::ScanImport`): `r` is the import report. The file goes, the
    /// popup shows how it went.
    pub(crate) fn scan_import_finished(&mut self, id: ScanOutputId, r: Result<Value, String>) {
        // the case holds the scan now (or the import failed): the file goes
        if self.scan_output.as_ref().is_some_and(|o| o.id() == id) {
            self.scan_output = None;
        }
        self.scan
            .output_done(id, r.map_err(|e| ScanFailure::new("import", e)));
    }

    /// Imports the surface the popup wrote (`scan.import`) with the import of
    /// a file chosen by hand: the same command, one undoable change, the
    /// import report. Refused while another import or command runs.
    fn import_scan_output(&mut self, out: ScanOutput) {
        let id = out.id();
        match self.import_scan_file(out.path().to_path_buf(), out.role(), id) {
            // the import job reads the file: it stays until the job ends
            Ok(()) => self.scan_output = Some(out),
            Err(f) => self.scan.output_done(id, Err(f)),
        }
    }
}
