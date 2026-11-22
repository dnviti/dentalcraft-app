//! The print popup, a window any egui screen opens to print parts on the
//! resin printer (print plan §7): the printer (the Elegoo Saturn 4 Ultra 16K,
//! the only profile), the user's resin (entered once, see [`ResinStore`]),
//! the parts to print with the missing ones preselected, then "Prepare":
//! the parts are turned, given their supports and placed on as few plates
//! as needed, and each plate's print file is prepared, on a thread of the
//! popup with its progress and a Cancel. The window then shows each plate
//! from above, the plate count, the layers, the time and the resin, and
//! "Save print files…" writes `plate_NN.goo` into the folder the user
//! chooses (a USB stick for the printer). There is no network: the user
//! carries the stick to the printer.
//!
//! A shell of an export takes the side that sits on the teeth from its
//! stage model, as `print.prepare` does ([`FittingSource::Model`]); a shell
//! outside an export has it guessed from its shape
//! ([`FittingSource::Geometry`]) and the window warns to export again.
//!
//! The advanced view (Settings, "Advanced options and parameters":
//! [`dc_module_api::advanced_ui`]) adds the layer height, the bottom layers
//! and their exposure, the rest before each exposure, the supports, the
//! spacing on the plate and the plate saved as a 3D model (STL).
//!
//! # Hosting
//!
//! ```ignore
//! // once
//! let mut print = PrintPopup::new(ResinStore::standard()).with_native_dialogs(true);
//! // the button of the host: what to print
//! print.open(PrintRequest { parts, out_dir });
//! // every frame
//! print.show(ui);
//! for e in print.take_events() {
//!     match e {
//!         PrintEvent::Saved { ids, files } => { /* record what went to the printer */ }
//!     }
//! }
//! ```

mod form;
mod resin;
mod view;
mod work;

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use dc_print::PrinterProfile;
use dc_print::job::{PlateSummary, PrintJob};
use dc_print::layout::LayoutParams;
use dc_print::orient::OrientParams;
use dc_print::support::SupportParams;
use dc_ui::theme::colors;
use dc_ui::widgets::BadgeKind;
use egui::{RichText, Ui};
use serde_json::Value;

pub use resin::{RESIN_FILE, Resin, ResinStore};
pub use work::{Failure, FittingSource, plate_file_name, plate_model_name};

use form::{FormAct, ResinForm};
use work::{Prepared, Source, Worker};

dc_i18n::bundle!(fn loader);

/// Translates `key` (this crate's bundle is registered on first use).
fn t(key: &str) -> String {
    loader();
    dc_i18n::tr(key)
}

/// Translates `key` with its arguments.
fn tr(key: &str, args: &[(&str, &str)]) -> String {
    loader();
    dc_i18n::tr_args(key, args)
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

/// Where a part stands in the print tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartStatus {
    /// Never printed.
    Missing,
    /// On a plate being prepared.
    Queued,
    /// Its file went to the printer (or a USB stick).
    Sent,
    /// Printed (confirmed).
    Printed,
    /// The print failed.
    Failed,
    /// To print again.
    Reprint,
}

impl PartStatus {
    /// From the tracking's id (`missing`, `queued`, …).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "missing" => Self::Missing,
            "queued" => Self::Queued,
            "sent" => Self::Sent,
            "printed" => Self::Printed,
            "failed" => Self::Failed,
            "reprint" => Self::Reprint,
            _ => return None,
        })
    }

    /// `true` for the parts still to print: chosen when the popup opens.
    pub fn preselected(self) -> bool {
        matches!(self, Self::Missing | Self::Failed | Self::Reprint)
    }

    /// The badge of the status.
    fn badge(self) -> (BadgeKind, &'static str) {
        match self {
            Self::Missing => (BadgeKind::Neutral, "print-status-missing"),
            Self::Queued => (BadgeKind::Info, "print-status-queued"),
            Self::Sent => (BadgeKind::Info, "print-status-sent"),
            Self::Printed => (BadgeKind::Success, "print-status-printed"),
            Self::Failed => (BadgeKind::Danger, "print-status-failed"),
            Self::Reprint => (BadgeKind::Warning, "print-status-reprint"),
        }
    }
}

/// One part the host offers to print.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintItem {
    /// Stable id (`SUP-03`, `INF-12-P`): the id of the tracking.
    pub id: String,
    /// What the user reads (`SUP 03`).
    pub label: String,
    /// Its shell file; `None` while it is not exported yet (shown, not
    /// printable).
    pub path: Option<PathBuf>,
    /// Its status in the tracking.
    pub status: PartStatus,
}

/// What the popup is opened with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrintRequest {
    /// The parts, in the order shown.
    pub parts: Vec<PrintItem>,
    /// The folder the save dialog starts in (the export folder).
    pub out_dir: Option<PathBuf>,
}

impl PrintRequest {
    /// The request a module command's output carries: `print_parts` (each
    /// `{id, label, path|null, status}`) and `export_dir`; `None` when the
    /// output has no `print_parts`. Parts without an id are left out; an
    /// unknown status reads as missing.
    pub fn from_output(out: &Value) -> Option<Self> {
        let parts = out.get("print_parts")?.as_array()?;
        let parts = parts
            .iter()
            .filter_map(|p| {
                let id = p.get("id")?.as_str()?.to_owned();
                Some(PrintItem {
                    label: p
                        .get("label")
                        .and_then(Value::as_str)
                        .map_or_else(|| id.clone(), str::to_owned),
                    path: p.get("path").and_then(Value::as_str).map(PathBuf::from),
                    status: p
                        .get("status")
                        .and_then(Value::as_str)
                        .and_then(PartStatus::parse)
                        .unwrap_or(PartStatus::Missing),
                    id,
                })
            })
            .collect();
        Some(Self {
            parts,
            out_dir: out
                .get("export_dir")
                .and_then(Value::as_str)
                .map(PathBuf::from),
        })
    }
}

/// What the popup tells its host.
#[derive(Debug, Clone, PartialEq)]
pub enum PrintEvent {
    /// The print files were saved: the parts they carry (ids, plate after
    /// plate) and the files.
    Saved {
        /// Ids of the parts in the files.
        ids: Vec<String>,
        /// The files written.
        files: Vec<PathBuf>,
    },
}

/// What a save writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    /// The print files (`plate_NN.goo`).
    PrintFiles,
    /// The plates as 3D models (`plate_NN.stl`, advanced view).
    PlateModels,
}

/// What the popup is doing.
enum Phase {
    /// Choosing.
    Idle,
    /// Preparing the plates.
    Preparing(Worker<Prepared>),
    /// Plates ready to save.
    Ready(Arc<Prepared>),
    /// Writing files.
    Saving {
        prepared: Arc<Prepared>,
        worker: Worker<Vec<PathBuf>>,
        what: Output,
    },
}

/// The last outcome shown under the plate.
#[derive(Debug, Clone, PartialEq)]
enum Message {
    Failed(Failure),
    Cancelled,
    Saved { files: Vec<PathBuf>, what: Output },
}

/// A part in the list, chosen or not.
struct Row {
    item: PrintItem,
    checked: bool,
}

/// The print popup.
pub struct PrintPopup {
    open: bool,
    rows: Vec<Row>,
    out_dir: Option<PathBuf>,
    profile: PrinterProfile,
    store: ResinStore,
    resin: Option<Resin>,
    /// The advanced view changed the resin values: saved at the next
    /// Prepare.
    resin_changed: bool,
    form: Option<ResinForm>,
    support: SupportParams,
    layout: LayoutParams,
    phase: Phase,
    /// The plate shown (0-based).
    plate: usize,
    message: Option<Message>,
    events: Vec<PrintEvent>,
    native_dialogs: bool,
    folder: Option<(Output, mpsc::Receiver<Option<PathBuf>>)>,
}

impl Default for PrintPopup {
    fn default() -> Self {
        Self::new(ResinStore::memory())
    }
}

impl PrintPopup {
    /// A closed popup whose resin is kept in `store` (read now).
    pub fn new(store: ResinStore) -> Self {
        Self {
            open: false,
            rows: Vec::new(),
            out_dir: None,
            profile: PrinterProfile::saturn_4_ultra_16k(),
            resin: store.load(),
            store,
            resin_changed: false,
            form: None,
            support: SupportParams::default(),
            layout: LayoutParams::default(),
            phase: Phase::Idle,
            plate: 0,
            message: None,
            events: Vec::new(),
            native_dialogs: false,
            folder: None,
        }
    }

    /// Asks for the folders with the system's dialog (`false`, the default:
    /// the save goes to the request's `out_dir`; tests and headless hosts).
    pub fn with_native_dialogs(mut self, on: bool) -> Self {
        self.native_dialogs = on;
        self
    }

    /// Opens the window for `request`: the parts still to print that have a
    /// file are chosen. Nothing is prepared before its Prepare button.
    pub fn open(&mut self, request: PrintRequest) {
        self.phase = Phase::Idle;
        self.rows = request
            .parts
            .into_iter()
            .map(|item| Row {
                checked: item.status.preselected() && item.path.is_some(),
                item,
            })
            .collect();
        self.out_dir = request.out_dir;
        self.plate = 0;
        self.message = None;
        self.folder = None;
        self.open = true;
    }

    /// `true` while the window is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Closes the window; a preparation or a save under way stops.
    pub fn close(&mut self) {
        self.open = false;
        self.phase = Phase::Idle;
        self.folder = None;
        self.form = None;
    }

    /// `true` while plates are prepared or files written.
    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Preparing(_) | Phase::Saving { .. })
    }

    /// The parts offered, each with whether it is chosen.
    pub fn parts(&self) -> impl Iterator<Item = (&PrintItem, bool)> {
        self.rows.iter().map(|r| (&r.item, r.checked))
    }

    /// Chooses part `id` or not (a part without a file cannot be chosen).
    pub fn set_chosen(&mut self, id: &str, on: bool) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.item.id == id) {
            r.checked = on && r.item.path.is_some();
            self.invalidate();
        }
    }

    /// The saved resin.
    pub fn resin(&self) -> Option<&Resin> {
        self.resin.as_ref()
    }

    /// Saves `resin` as the user's resin.
    ///
    /// # Errors
    /// The settings file cannot be written (the resin is used anyway).
    pub fn set_resin(&mut self, resin: Resin) -> std::io::Result<()> {
        let r = self.store.save(&resin);
        self.resin = Some(resin);
        self.resin_changed = false;
        self.form = None;
        self.invalidate();
        r
    }

    /// `true` when Prepare can run: a resin, at least one part chosen,
    /// nothing under way.
    pub fn can_prepare(&self) -> bool {
        self.resin.is_some() && self.rows.iter().any(|r| r.checked) && !self.is_busy()
    }

    /// The prepared plates, once ready.
    pub fn plates(&self) -> Option<Vec<PlateSummary>> {
        match &self.phase {
            Phase::Ready(p) | Phase::Saving { prepared: p, .. } => {
                Some(p.plates.iter().map(|p| p.summary.clone()).collect())
            }
            _ => None,
        }
    }

    /// The last failure (its stable code and detail), if the last run
    /// failed.
    pub fn failure(&self) -> Option<&Failure> {
        match &self.message {
            Some(Message::Failed(f)) => Some(f),
            _ => None,
        }
    }

    /// Where each prepared part's fitting side comes from (`model`: its
    /// stage model in the export; `geometry`: guessed from its shape), by
    /// id in the order chosen, once the plates are ready.
    pub fn fitting_sources(&self) -> Option<Vec<(String, FittingSource)>> {
        match &self.phase {
            Phase::Ready(p) | Phase::Saving { prepared: p, .. } => Some(p.fitting.clone()),
            _ => None,
        }
    }

    /// Warning codes of the printer profile in use, shown in the popup and
    /// carried by every `print.prepare` result (`warnings`):
    /// [`dc_print::profile::PROFILE_NOT_VALIDATED`] until the profile's
    /// scale and orientation are checked against the printer.
    pub fn profile_warnings(&self) -> Vec<&'static str> {
        self.profile.warnings()
    }

    /// The print job of the chosen resin and the settings in force.
    fn print_job(&self, resin: &Resin) -> PrintJob {
        PrintJob {
            profile: self.profile.clone(),
            job: resin.job_settings(),
            density_g_ml: resin.density_g_ml,
            orient: OrientParams::default(),
            support: self.support.clone(),
            layout: self.layout.clone(),
        }
    }

    /// Starts preparing the chosen parts (the Prepare button); `false` when
    /// it cannot ([`PrintPopup::can_prepare`]) or the settings are refused
    /// (the window says why).
    pub fn prepare(&mut self) -> bool {
        if !self.can_prepare() {
            return false;
        }
        let Some(resin) = self.resin.clone() else {
            return false;
        };
        if self.resin_changed {
            if let Err(e) = self.store.save(&resin) {
                tracing::warn!(code = "print-resin-write", "the resin was not saved: {e}");
            }
            self.resin_changed = false;
        }
        let job = self.print_job(&resin);
        self.message = None;
        if let Err(e) = job.validate() {
            self.message = Some(Message::Failed(e.into()));
            return false;
        }
        let sources: Vec<Source> = self
            .rows
            .iter()
            .filter(|r| r.checked)
            .filter_map(|r| {
                Some(Source {
                    id: r.item.id.clone(),
                    path: r.item.path.clone()?,
                })
            })
            .collect();
        self.plate = 0;
        self.phase = Phase::Preparing(Worker::spawn(
            "dc-print-prepare",
            move |cancel, progress| work::prepare(&sources, &job, cancel, progress),
        ));
        true
    }

    /// Writes the print files of the prepared plates into `folder`;
    /// `false` when no plate is ready or a save runs.
    pub fn save_to(&mut self, folder: PathBuf) -> bool {
        self.start_save(Output::PrintFiles, folder)
    }

    /// Writes the prepared plates as 3D models (STL) into `folder`.
    pub fn save_models_to(&mut self, folder: PathBuf) -> bool {
        self.start_save(Output::PlateModels, folder)
    }

    fn start_save(&mut self, what: Output, folder: PathBuf) -> bool {
        let Phase::Ready(prepared) = &self.phase else {
            return false;
        };
        let prepared = Arc::clone(prepared);
        let p = Arc::clone(&prepared);
        self.message = None;
        let worker = Worker::spawn("dc-print-save", move |cancel, progress| match what {
            Output::PrintFiles => work::save_files(&p, &folder, cancel, progress),
            Output::PlateModels => work::save_plate_models(&p, &folder, cancel, progress),
        });
        self.phase = Phase::Saving {
            prepared,
            worker,
            what,
        };
        true
    }

    /// Asks where to save (the system dialog, else the request's folder).
    fn ask_folder(&mut self, what: Output) {
        if !self.native_dialogs {
            match self.out_dir.clone() {
                Some(dir) => {
                    self.start_save(what, dir);
                }
                None => {
                    self.message = Some(Message::Failed(Failure::new(
                        "print-no-folder",
                        "no folder to save in",
                    )));
                }
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        let start = self.out_dir.clone();
        let title = t(match what {
            Output::PrintFiles => "print-dialog-files",
            Output::PlateModels => "print-dialog-models",
        });
        let spawned = std::thread::Builder::new()
            .name("dc-print-folder".into())
            .spawn(move || {
                let mut d = rfd::FileDialog::new().set_title(title);
                if let Some(dir) = start.filter(|d| d.is_dir()) {
                    d = d.set_directory(dir);
                }
                let _ = tx.send(d.pick_folder());
            });
        match spawned {
            Ok(_) => self.folder = Some((what, rx)),
            Err(e) => {
                self.message = Some(Message::Failed(Failure::new(
                    "print-internal",
                    format!("cannot open the folder dialog: {e}"),
                )));
            }
        }
    }

    /// Stops the preparation or the save under way.
    pub fn cancel(&mut self) {
        match &self.phase {
            Phase::Preparing(w) => w.cancel(),
            Phase::Saving { worker, .. } => worker.cancel(),
            _ => {}
        }
    }

    /// The parts or the settings changed: plates prepared before no longer
    /// match them.
    fn invalidate(&mut self) {
        if let Phase::Ready(_) = self.phase {
            self.phase = Phase::Idle;
        }
        if !matches!(self.message, Some(Message::Saved { .. })) {
            self.message = None;
        }
    }

    fn fail(&mut self, f: Failure) {
        if f.is_cancelled() {
            self.message = Some(Message::Cancelled);
        } else {
            tracing::warn!(code = %f.code, "print: {}", f.detail);
            self.message = Some(Message::Failed(f));
        }
    }

    /// Applies what ended: the plates prepared, the files saved, the folder
    /// chosen. [`PrintPopup::show`] calls it every frame; a host that does
    /// not draw the window calls it itself.
    pub fn poll(&mut self) {
        if let Some((what, rx)) = &self.folder {
            match rx.try_recv() {
                Ok(Some(dir)) => {
                    let what = *what;
                    self.folder = None;
                    self.start_save(what, dir);
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => self.folder = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let ended = match &self.phase {
            Phase::Preparing(w) => w.try_take().map(|r| r.map(|p| (Arc::new(p), None))),
            Phase::Saving {
                prepared,
                worker,
                what,
            } => worker
                .try_take()
                .map(|r| r.map(|files| (Arc::clone(prepared), Some((files, *what))))),
            _ => None,
        };
        let Some(ended) = ended else {
            return;
        };
        let saving = match &self.phase {
            Phase::Saving { prepared, .. } => Some(Arc::clone(prepared)),
            _ => None,
        };
        match ended {
            Ok((prepared, saved)) => {
                if let Some((files, what)) = saved {
                    if what == Output::PrintFiles {
                        self.events.push(PrintEvent::Saved {
                            ids: prepared.ids(),
                            files: files.clone(),
                        });
                    }
                    self.message = Some(Message::Saved { files, what });
                }
                self.phase = Phase::Ready(prepared);
            }
            Err(f) => {
                // a failed save keeps the plates; a failed preparation has none
                self.phase = saving.map_or(Phase::Idle, Phase::Ready);
                self.fail(f);
            }
        }
    }

    /// What happened since the last call (the files saved).
    pub fn take_events(&mut self) -> Vec<PrintEvent> {
        std::mem::take(&mut self.events)
    }

    /// The resin form answered.
    fn form_answer(&mut self, act: FormAct) {
        match act {
            FormAct::Save(r) => {
                if let Err(e) = self.set_resin(r) {
                    self.message = Some(Message::Failed(Failure::new(
                        "print-write",
                        format!("the resin values were not saved: {e}"),
                    )));
                }
            }
            FormAct::Cancel => self.form = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn en_it_parity() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }

    #[test]
    fn an_unvalidated_profile_is_shown_with_its_warning() {
        let p = PrintPopup::default();
        assert_eq!(
            p.profile_warnings(),
            [dc_print::profile::PROFILE_NOT_VALIDATED]
        );
        for key in [
            "print-profile-not-validated",
            "print-profile-not-validated-body",
        ] {
            assert_ne!(t(key), key, "{key} has no text");
        }
    }

    #[test]
    fn the_request_of_a_module_output() {
        let out = json!({
            "print_parts": [
                {"id": "SUP-03", "label": "SUP 03", "path": "/x/SUP_03.stl", "status": "missing"},
                {"id": "INF-12-P", "label": "INF 12 P", "path": null, "status": "printed"},
                {"label": "no id"},
                {"id": "SUP-04", "status": "strange"},
            ],
            "export_dir": "/x",
        });
        let r = PrintRequest::from_output(&out).unwrap();
        assert_eq!(r.parts.len(), 3);
        assert_eq!(
            r.parts[0].path.as_deref(),
            Some(std::path::Path::new("/x/SUP_03.stl"))
        );
        assert_eq!(r.parts[1].status, PartStatus::Printed);
        assert_eq!(r.parts[1].path, None);
        assert_eq!(r.parts[2].label, "SUP-04");
        assert_eq!(r.parts[2].status, PartStatus::Missing);
        assert_eq!(r.out_dir, Some(PathBuf::from("/x")));
        assert_eq!(PrintRequest::from_output(&json!({"done": true})), None);
    }

    #[test]
    fn the_parts_still_to_print_are_chosen_when_it_opens() {
        let mut p = PrintPopup::default();
        let item = |id: &str, status, path: bool| PrintItem {
            id: id.into(),
            label: id.into(),
            path: path.then(|| PathBuf::from(format!("/x/{id}.stl"))),
            status,
        };
        p.open(PrintRequest {
            parts: vec![
                item("a", PartStatus::Missing, true),
                item("b", PartStatus::Printed, true),
                item("c", PartStatus::Reprint, true),
                item("d", PartStatus::Missing, false),
                item("e", PartStatus::Failed, true),
                item("f", PartStatus::Sent, true),
            ],
            out_dir: None,
        });
        let chosen: Vec<&str> = p
            .parts()
            .filter(|(_, on)| *on)
            .map(|(i, _)| i.id.as_str())
            .collect();
        assert_eq!(chosen, ["a", "c", "e"]);
        p.set_chosen("d", true);
        assert!(
            !p.parts().any(|(i, on)| i.id == "d" && on),
            "no file, no choice"
        );
    }

    #[test]
    fn prepare_needs_a_resin_and_a_part() {
        let mut p = PrintPopup::default();
        p.open(PrintRequest {
            parts: vec![PrintItem {
                id: "a".into(),
                label: "a".into(),
                path: Some("/x/a.stl".into()),
                status: PartStatus::Missing,
            }],
            out_dir: None,
        });
        assert!(!p.can_prepare());
        assert!(!p.prepare());
        p.set_resin(Resin {
            name: "R".into(),
            layer_um: 50,
            exposure_s: 2.5,
            bottom_exposure_s: 30.0,
            bottom_layers: 4,
            rest_s: 1.0,
            density_g_ml: 1.1,
        })
        .unwrap();
        assert!(p.can_prepare());
        p.set_chosen("a", false);
        assert!(!p.can_prepare());
    }

    #[test]
    fn a_part_that_cannot_be_read_fails_with_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = PrintPopup::default();
        p.open(PrintRequest {
            parts: vec![PrintItem {
                id: "a".into(),
                label: "a".into(),
                path: Some(dir.path().join("missing.stl")),
                status: PartStatus::Missing,
            }],
            out_dir: None,
        });
        p.set_resin(Resin {
            name: "R".into(),
            layer_um: 50,
            exposure_s: 2.5,
            bottom_exposure_s: 30.0,
            bottom_layers: 4,
            rest_s: 1.0,
            density_g_ml: 1.1,
        })
        .unwrap();
        assert!(p.prepare());
        let start = std::time::Instant::now();
        while p.is_busy() {
            assert!(start.elapsed().as_secs() < 30);
            std::thread::sleep(std::time::Duration::from_millis(5));
            p.poll();
        }
        assert_eq!(p.failure().map(|f| f.code.as_str()), Some("print-read"));
        assert!(p.plates().is_none());
    }
}
