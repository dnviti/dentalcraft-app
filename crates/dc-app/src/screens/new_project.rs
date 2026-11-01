//! `04 New project`: 1 Module, 2 Case, 3 Scans, footer with the save path.
//! Each scan row is probed on a worker thread (triangles, units, or why the
//! file is bad). "Create" imports the scans with the shared pipeline on
//! another worker ([`crate::create_job`]), saves the `.dcprj` and opens the
//! workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use dc_case::ScanRole;
use dc_case::import::{ImportError, PrepareOptions, WarningCode, prepare_with};
use dc_command::{CancelToken, JobCtx};
use dc_mesh::Units;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{Palette, fonts, icons, tokens};
use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::app::{DentalCraftApp, Screen};
use crate::create_job::{NewProjectRequest, file_stem};
use crate::dialogs::{Purpose, Spec};
use crate::paint;

/// Extensions the file dialog offers. The portal matches them case-sensitively,
/// so each one is listed in lower and upper case (`SCAN.STL` from a scanner).
pub const DIALOG_EXTENSIONS: [&str; 6] = ["stl", "ply", "obj", "STL", "PLY", "OBJ"];

/// How often the screen repaints while a file is still being probed.
const PROBE_POLL: Duration = Duration::from_millis(50);

/// What the probe of a file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFacts {
    /// Size of the file in bytes.
    pub bytes: u64,
    /// Triangles after the cleanup.
    pub triangles: usize,
    /// Unit the file was in (converted to millimetres on import). A file read
    /// with [`PrepareOptions::KEEP_UNITS`] (a bite) stays in millimetres.
    pub units: Units,
    /// Symbol of the unit a kept-as-it-is file looks to be in (`cm`) when its
    /// size does not fit millimetres: the import's `units_suspect` warning.
    pub units_suspect: Option<String>,
}

/// State of the probe of one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Being read on a worker thread.
    Pending,
    /// Readable.
    Ready(ScanFacts),
    /// Not usable: empty, corrupt, unreadable.
    Failed(ImportError),
}

/// The probe: reads a file like the import does, with the options the import
/// will use for it, and keeps the facts.
pub type ProbeFn =
    dyn Fn(&Path, &PrepareOptions, &JobCtx) -> Result<ScanFacts, ImportError> + Send + Sync;

/// The probe of the application: the import's own `prepare_with`.
pub fn probe_file(
    path: &Path,
    options: &PrepareOptions,
    ctx: &JobCtx,
) -> Result<ScanFacts, ImportError> {
    let scan = prepare_with(path, options, ctx)?;
    let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
    let units_suspect = scan
        .warnings
        .iter()
        .find(|w| w.code == WarningCode::UnitsSuspect)
        .and_then(|w| w.args.get("units").cloned());
    Ok(ScanFacts {
        bytes,
        triangles: scan.mesh.face_count(),
        units: scan.source_units,
        units_suspect,
    })
}

/// How the import reads a file of this role: a bite scan keeps its units
/// (the plugin never rescales those), every other scan is converted.
fn options_for(role: Option<ScanRole>) -> PrepareOptions {
    if role == Some(ScanRole::Bite) {
        PrepareOptions::KEEP_UNITS
    } else {
        PrepareOptions::default()
    }
}

type ProbeResult = (u64, Result<ScanFacts, ImportError>);

/// Worker threads of the probes and the channel their answers come back on.
struct Prober {
    run: Arc<ProbeFn>,
    tx: Sender<ProbeResult>,
    rx: Receiver<ProbeResult>,
    next_id: u64,
}

impl std::fmt::Debug for Prober {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prober")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Prober {
    fn new(run: Arc<ProbeFn>) -> Self {
        let (tx, rx) = channel();
        Self {
            run,
            tx,
            rx,
            next_id: 0,
        }
    }

    /// Starts the probe of `path`, read with `options`, on its own thread;
    /// returns the row id, the token that stops it and the state to show
    /// until the answer comes.
    fn start(&mut self, path: &Path, options: PrepareOptions) -> (u64, CancelToken, Probe) {
        let id = self.next_id;
        self.next_id += 1;
        let ctx = JobCtx::detached();
        let token = ctx.cancel_token().clone();
        let (run, tx, file) = (self.run.clone(), self.tx.clone(), path.to_owned());
        let spawned = std::thread::Builder::new()
            .name("dc-scan-probe".into())
            .spawn(move || {
                let _ = tx.send((id, run(&file, &options, &ctx)));
            });
        let probe = match spawned {
            Ok(_) => Probe::Pending,
            Err(e) => Probe::Failed(ImportError::Unreadable {
                name: file_name(path),
                detail: e.to_string(),
            }),
        };
        (id, token, probe)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A scan chosen for the new project.
#[derive(Debug)]
pub struct ScanEntry {
    id: u64,
    /// File.
    pub path: PathBuf,
    /// Role read from the file name (never from the folders).
    pub hint: Option<ScanRole>,
    /// Role the user chose by clicking the badge; `None` leaves it to the
    /// file name and then to the shape of the scan.
    pub choice: Option<ScanRole>,
    /// What the probe found.
    pub probe: Probe,
    /// `true` when the probe reads the file the way a bite is read (units
    /// kept): what the import will do when the row's role is bite.
    probed_as_bite: bool,
    cancel: CancelToken,
}

impl Drop for ScanEntry {
    /// A row that goes away stops its probe.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl ScanEntry {
    /// The role the badge shows: the user's choice, else the name's hint,
    /// else none (auto).
    pub fn shown(&self) -> Option<ScanRole> {
        self.choice.or(self.hint)
    }

    /// `true` when the import will read this row as a bite scan (the user's
    /// choice, else the name's hint), and so keep its units.
    pub fn is_bite(&self) -> bool {
        self.shown() == Some(ScanRole::Bite)
    }

    /// Next value of the badge: Auto, Upper, Lower, Bite, Auto. A row whose
    /// name says a role cannot go back to Auto (the name still counts); it
    /// goes back to the name's role, which is no choice at all.
    pub fn cycle(&mut self) {
        let order = [
            None,
            Some(ScanRole::Upper),
            Some(ScanRole::Lower),
            Some(ScanRole::Bite),
        ];
        let at = order.iter().position(|r| *r == self.shown()).unwrap_or(0);
        let next = (1..=order.len())
            .map(|k| order[(at + k) % order.len()])
            .find(|r| r.is_some() || self.hint.is_none())
            .flatten();
        self.choice = next.filter(|r| Some(*r) != self.hint);
    }
}

/// Jaw choice (stored in the case).
pub use dc_case::Jaws as Jaw;

/// New project form state.
#[derive(Debug)]
pub struct NewProjectState {
    /// Selected module id.
    pub module: String,
    /// Case ID.
    pub case_id: String,
    /// Patient alias.
    pub alias: String,
    /// Jaws being treated. Stored in the case; it picks no scan's role.
    pub jaw: Jaw,
    /// Notes for the technician.
    pub notes: String,
    /// Scans.
    pub scans: Vec<ScanEntry>,
    /// Chosen save path (default: projects folder / case_alias.dcprj).
    pub location: Option<PathBuf>,
    /// Show validation errors.
    pub show_errors: bool,
    /// Last creation error.
    pub error: Option<String>,
    /// Why no scan could be imported (one reason per file) when creation
    /// found nothing to import.
    pub failures: Vec<String>,
    projects_dir: PathBuf,
    prober: Prober,
}

/// Validation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Problems {
    /// Case ID missing.
    pub case_id: bool,
    /// Alias missing.
    pub alias: bool,
    /// No scans.
    pub scans: bool,
}

impl Problems {
    /// Nothing wrong.
    pub fn ok(&self) -> bool {
        !(self.case_id || self.alias || self.scans)
    }
}

impl NewProjectState {
    /// Empty form saving under `projects_dir`.
    pub fn new(projects_dir: &Path) -> Self {
        Self::with_probe(projects_dir, Arc::new(probe_file))
    }

    /// Like [`NewProjectState::new`] with another probe (tests).
    pub fn with_probe(projects_dir: &Path, probe: Arc<ProbeFn>) -> Self {
        Self {
            module: "aligners".into(),
            case_id: String::new(),
            alias: String::new(),
            jaw: Jaw::Both,
            notes: String::new(),
            scans: Vec::new(),
            location: None,
            show_errors: false,
            error: None,
            failures: Vec::new(),
            projects_dir: projects_dir.to_path_buf(),
            prober: Prober::new(probe),
        }
    }

    /// Adds scan files (other extensions, in any letter case, and duplicates
    /// are ignored). The role hint comes from the file name only; each file
    /// is probed on a worker thread, so this returns at once.
    pub fn add_scans(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if !dc_workspace::import::is_scan_file(&p) || self.scans.iter().any(|s| s.path == p) {
                continue;
            }
            let hint = p
                .file_name()
                .and_then(|n| dc_case::role_from_name(&n.to_string_lossy()));
            let as_bite = hint == Some(ScanRole::Bite);
            let (id, cancel, probe) = self.prober.start(&p, options_for(hint));
            self.scans.push(ScanEntry {
                id,
                path: p,
                hint,
                choice: None,
                probe,
                probed_as_bite: as_bite,
                cancel,
            });
        }
    }

    /// Gives row `index` the next value of its badge. When that makes it a
    /// bite row, or stops it being one, the file is probed again the way the
    /// import will read it (a bite keeps its units); the answer of the probe
    /// it replaces is dropped.
    pub fn cycle_scan(&mut self, index: usize) {
        let Some(row) = self.scans.get_mut(index) else {
            return;
        };
        row.cycle();
        if row.is_bite() == row.probed_as_bite {
            return;
        }
        row.cancel.cancel();
        let as_bite = row.is_bite();
        let (id, cancel, probe) = self.prober.start(&row.path, options_for(row.shown()));
        row.id = id;
        row.cancel = cancel;
        row.probe = probe;
        row.probed_as_bite = as_bite;
    }

    /// Removes row `index`; its probe is cancelled and a late answer is
    /// ignored.
    pub fn remove_scan(&mut self, index: usize) {
        if index < self.scans.len() {
            self.scans.remove(index);
        }
    }

    /// Takes the answers of the probes that finished; `true` when a row
    /// changed. Answers of removed rows are dropped.
    pub fn poll_probes(&mut self) -> bool {
        let mut changed = false;
        while let Ok((id, result)) = self.prober.rx.try_recv() {
            if let Some(row) = self.scans.iter_mut().find(|s| s.id == id) {
                row.probe = match result {
                    Ok(facts) => Probe::Ready(facts),
                    Err(e) => Probe::Failed(e),
                };
                changed = true;
            }
        }
        changed
    }

    /// `true` while a file is still being probed.
    pub fn probing(&self) -> bool {
        self.scans.iter().any(|s| s.probe == Probe::Pending)
    }

    /// Where the project will be saved.
    pub fn path(&self) -> PathBuf {
        self.location.clone().unwrap_or_else(|| {
            self.projects_dir.join(format!(
                "{}.{}",
                file_stem(&self.case_id, &self.alias),
                dc_project::FILE_EXTENSION
            ))
        })
    }

    /// Checks the form.
    pub fn problems(&self) -> Problems {
        Problems {
            case_id: self.case_id.trim().is_empty(),
            alias: self.alias.trim().is_empty(),
            scans: self.scans.is_empty(),
        }
    }

    /// The creation request when the form is valid. Every row goes in, a bad
    /// one too: the import reports it. A row's role is the user's choice, or
    /// auto (the import reads the name, then the shape).
    pub fn request(&self) -> Option<NewProjectRequest> {
        self.problems().ok().then(|| NewProjectRequest {
            module_id: self.module.clone(),
            case_id: self.case_id.trim().to_owned(),
            alias: self.alias.trim().to_owned(),
            jaws: self.jaw,
            notes: self.notes.trim().to_owned(),
            scans: self
                .scans
                .iter()
                .map(|s| (s.path.clone(), s.choice))
                .collect(),
            path: self.path(),
        })
    }
}

fn text(ui: &mut Ui, s: impl Into<String>, font: egui::FontId, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).wrap());
}

fn role_key(r: ScanRole) -> String {
    match r {
        ScanRole::Upper => t!("jaw-upper-badge"),
        ScanRole::Lower => t!("jaw-lower-badge"),
        ScanRole::Bite => t!("jaw-bite-badge"),
    }
}

fn role_icon(r: ScanRole) -> &'static str {
    match r {
        ScanRole::Upper => icons::CARET_UP,
        ScanRole::Lower => icons::CARET_DOWN,
        ScanRole::Bite => icons::ARROWS_LEFT_RIGHT,
    }
}

/// The badge of a row: Auto, a role read from the name (neutral, a hint) or
/// a role the user chose (blue). Clicking it asks for the next value.
fn role_badge(ui: &mut Ui, s: &ScanEntry) -> egui::Response {
    let (kind, icon, label, tip) = match (s.choice, s.hint) {
        (Some(r), _) => (
            BadgeKind::Info,
            role_icon(r),
            role_key(r),
            t!("np-role-change"),
        ),
        (None, Some(r)) => (
            BadgeKind::Neutral,
            role_icon(r),
            role_key(r),
            t!("np-role-hint-tip"),
        ),
        (None, None) => (
            BadgeKind::Neutral,
            icons::MAGIC_WAND,
            t!("np-role-auto"),
            t!("np-role-auto-tip"),
        ),
    };
    let b = widgets::badge_icon(ui, kind, Some(icon), label.clone());
    let resp = ui
        .interact(b.rect, b.id.with("cycle"), Sense::click())
        .on_hover_text(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp
}

/// `1234567` as `1,234,567` (`1.234.567` in Italian).
fn grouped(n: usize) -> String {
    let sep = if dc_i18n::current() == dc_i18n::Lang::It {
        '.'
    } else {
        ','
    };
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

/// The unit of a file and what the import does with it: `mm`, or `cm → mm`.
fn units_text(u: Units) -> String {
    let symbol = |u: Units| {
        if u == Units::Micrometers {
            "\u{b5}m"
        } else {
            u.symbol()
        }
    };
    if u == Units::Millimeters {
        symbol(u).to_owned()
    } else {
        format!("{} \u{2192} mm", symbol(u))
    }
}

/// The second line of a row: reading, size / triangles / units, or why the
/// file is bad.
fn probe_line(ui: &mut Ui, c: &Palette, probe: &Probe) {
    match probe {
        Probe::Pending => {
            ui.horizontal(|ui| {
                widgets::spinner(ui, 12.0);
                ui.label(
                    RichText::new(t!("np-scan-reading"))
                        .font(fonts::mono(11.0))
                        .color(c.text_secondary),
                );
            });
        }
        Probe::Ready(f) => {
            ui.label(
                RichText::new(t!(
                    "np-scan-facts",
                    size = format!("{:.1}", f.bytes as f64 / 1_048_576.0),
                    triangles = grouped(f.triangles),
                    units = units_text(f.units)
                ))
                .font(fonts::mono(11.0))
                .color(c.text_secondary),
            );
            if let Some(looks) = &f.units_suspect {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "{}  {}",
                            icons::WARNING,
                            t!("np-scan-units-suspect", units = looks.clone())
                        ))
                        .font(fonts::ui(12.0))
                        .color(c.status_warning),
                    )
                    .wrap(),
                );
            }
        }
        Probe::Failed(e) => {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{}  {}", icons::WARNING_CIRCLE, e.message()))
                        .font(fonts::ui(12.0))
                        .color(c.status_danger),
                )
                .wrap(),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn column(
    ui: &mut Ui,
    c: &Palette,
    n: u32,
    title: String,
    right: Option<&str>,
    width: f32,
    height: f32,
    add: impl FnOnce(&mut Ui),
) {
    egui::Frame::new()
        .fill(c.bg_panel)
        .stroke(Stroke::new(1.0, c.border_subtle))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.set_width((width - 40.0).max(80.0));
                ui.set_min_height(height);
                ui.horizontal(|ui| {
                    paint::step_number(ui, c, n);
                    ui.label(
                        RichText::new(title)
                            .font(fonts::style(&tokens().text.title_s))
                            .color(c.text_primary),
                    );
                    if let Some(r) = right {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(r)
                                    .font(fonts::mono(11.0))
                                    .color(c.text_secondary),
                            );
                        });
                    }
                });
                ui.add_space(16.0);
                add(ui);
            });
        });
}

impl DentalCraftApp {
    fn pick_scans(&mut self) {
        self.open_dialog(
            Purpose::PickScans,
            Spec {
                title: t!("np-browse"),
                filter: Some((t!("dialog-filter-scans"), DIALOG_EXTENSIONS.to_vec())),
                multiple: true,
                ..Spec::default()
            },
        );
    }

    /// The text of the red banner: the creation error, or the reasons no scan
    /// could be imported.
    fn np_error_text(&self) -> Option<String> {
        let np = &self.new_project;
        if !np.failures.is_empty() {
            let mut lines = vec![t!("np-create-none")];
            lines.extend(
                np.failures
                    .iter()
                    .map(|r| format!("{}  {r}", icons::DOT_OUTLINE)),
            );
            return Some(lines.join("\n"));
        }
        np.error
            .as_ref()
            .map(|e| t!("np-create-failed", reason = e.clone()))
    }

    pub(crate) fn new_project_ui(&mut self, ui: &mut Ui) {
        self.new_project.poll_probes();
        if self.new_project.probing() {
            ui.ctx().request_repaint_after(PROBE_POLL);
        }
        let c = *dc_ui::colors(ui);
        let tk = tokens();
        // footer bar
        egui::Panel::bottom("dc-np-footer")
            .exact_size(84.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(c.bg_canvas)
                    .inner_margin(Margin::symmetric(48, 0)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [r.left_top(), r.right_top()],
                    Stroke::new(1.0, c.border_subtle),
                );
                ui.horizontal_centered(|ui| self.np_footer(ui, &c));
            });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin {
                        left: 48,
                        right: 48,
                        top: 32,
                        bottom: 24,
                    })
                    .show(ui, |ui| {
                        text(
                            ui,
                            t!("np-title"),
                            fonts::style(&tk.text.display_l),
                            c.text_primary,
                        );
                        ui.add_space(6.0);
                        text(
                            ui,
                            t!("np-sub"),
                            fonts::style(&tk.text.body_l),
                            c.text_secondary,
                        );
                        ui.add_space(24.0);
                        if let Some(reason) = self.np_error_text() {
                            egui::Frame::new()
                                .fill(c.bg_raised)
                                .stroke(Stroke::new(1.0, c.status_danger))
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(Margin::same(12))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    text(
                                        ui,
                                        format!("{}  {reason}", icons::X_CIRCLE),
                                        fonts::ui(13.0),
                                        c.status_danger,
                                    );
                                });
                            ui.add_space(16.0);
                        }
                        let full = ui.available_width();
                        let gap = 24.0;
                        let w1 = 400.0_f32.min(full * 0.24);
                        let w3 = 640.0_f32.min(full * 0.34);
                        let w2 = (full - w1 - w3 - 2.0 * gap).max(360.0);
                        let h = (ui.available_height() - 8.0).max(640.0);
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            column(ui, &c, 1, t!("np-module"), None, w1, h, |ui| {
                                self.np_modules(ui, &c)
                            });
                            column(ui, &c, 2, t!("np-case"), None, w2, h, |ui| {
                                self.np_case(ui, &c)
                            });
                            column(
                                ui,
                                &c,
                                3,
                                t!("np-scans"),
                                Some("STL, PLY, OBJ"),
                                w3,
                                h,
                                |ui| self.np_scans(ui, &c),
                            );
                        });
                    });
            });
    }

    fn np_modules(&mut self, ui: &mut Ui, c: &Palette) {
        let cards = self.module_cards.clone();
        for m in &cards {
            let installed = m.installed();
            let selected = self.new_project.module == m.id;
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(
                vec2(w, 78.0),
                if installed {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            let name = dc_i18n::tr(m.name_key);
            let label = name.clone();
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::RadioButton, installed, selected, &label)
            });
            if resp.clicked() {
                self.new_project.module = m.id.to_owned();
            }
            let p = ui.painter().clone();
            let (fill, stroke) = if selected {
                (c.select_soft, Stroke::new(2.0, c.select_blue))
            } else {
                (c.bg_panel, Stroke::new(1.0, c.border_subtle))
            };
            p.rect(
                rect,
                CornerRadius::same(6),
                fill,
                stroke,
                StrokeKind::Inside,
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(vec2(14.0, 15.0)))
                    .layout(Layout::left_to_right(Align::Center)),
            );
            paint::code_tile(&mut child, c, m.code, vec2(44.0, 48.0), installed);
            child.add_space(8.0);
            child.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(name)
                        .font(fonts::semibold(15.0))
                        .color(if installed {
                            c.text_primary
                        } else {
                            c.text_secondary
                        }),
                );
                let sub = match m.version {
                    Some(v) if m.id == "aligners" => {
                        format!("{}, v{v}", t!("module-aligners-short"))
                    }
                    Some(v) => format!("v{v}"),
                    None => t!("np-not-installed"),
                };
                ui.label(
                    RichText::new(sub)
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                );
            });
            child.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if installed {
                    widgets::radio_indicator(ui, selected, true);
                } else {
                    widgets::badge_icon(
                        ui,
                        BadgeKind::Neutral,
                        Some(icons::HOURGLASS_MEDIUM),
                        t!("module-coming-soon"),
                    );
                }
            });
            widgets::focus_ring(ui, &resp, rect, CornerRadius::same(6));
            ui.add_space(10.0);
        }
        text(ui, t!("np-modules-note"), fonts::ui(12.0), c.text_secondary);
    }

    fn np_case(&mut self, ui: &mut Ui, c: &Palette) {
        let probs = self.new_project.problems();
        let show = self.new_project.show_errors;
        let req = dc_ui::tr("ui-required");
        widgets::text_input(
            ui,
            &t!("np-case-id"),
            &mut self.new_project.case_id,
            "AL-2026-0001",
            Some(&t!("np-case-id-help")),
            (show && probs.case_id).then_some(req.as_str()),
        );
        ui.add_space(14.0);
        widgets::text_input(
            ui,
            &t!("np-alias"),
            &mut self.new_project.alias,
            "BIA-07",
            Some(&t!("np-alias-help")),
            (show && probs.alias).then_some(req.as_str()),
        );
        ui.add_space(14.0);
        ui.label(
            RichText::new(t!("np-jaw"))
                .font(fonts::medium(12.0))
                .color(c.text_secondary),
        );
        let mut idx = match self.new_project.jaw {
            Jaw::Upper => 0,
            Jaw::Lower => 1,
            Jaw::Both => 2,
        };
        if widgets::segmented_fill(
            ui,
            &mut idx,
            &[t!("jaw-upper"), t!("jaw-lower"), t!("jaw-both")],
            40.0,
        )
        .changed()
        {
            self.new_project.jaw = [Jaw::Upper, Jaw::Lower, Jaw::Both][idx];
        }
        ui.add_space(14.0);
        widgets::text_area(ui, &t!("np-notes"), &mut self.new_project.notes, "", 150.0);
        ui.add_space(14.0);
        widgets::sunken_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(
                    RichText::new(icons::RULER)
                        .font(fonts::icon(16.0))
                        .color(c.text_secondary),
                );
                text(ui, t!("np-units-note"), fonts::ui(12.0), c.text_secondary);
            });
        });
    }

    fn np_scans(&mut self, ui: &mut Ui, c: &Palette) {
        // drop zone
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 180.0), Sense::hover());
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        paint::dashed_rect(
            ui.painter(),
            rect,
            [6.0; 4],
            Stroke::new(
                1.0,
                if hovering {
                    c.select_blue
                } else {
                    c.border_strong
                },
            ),
        );
        let mut z = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(20.0))
                .layout(Layout::top_down(Align::Center)),
        );
        z.label(
            RichText::new(icons::UPLOAD_SIMPLE)
                .font(fonts::icon(24.0))
                .color(c.text_secondary),
        );
        z.label(
            RichText::new(t!("np-drop-title"))
                .font(fonts::semibold(15.0))
                .color(c.text_primary),
        );
        z.label(
            RichText::new(t!("np-drop-body"))
                .font(fonts::ui(12.0))
                .color(c.text_secondary),
        );
        z.add_space(4.0);
        if Button::secondary(t!("np-browse"))
            .icon(icons::FOLDER_OPEN)
            .show(&mut z)
            .clicked()
        {
            self.pick_scans();
        }
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.new_project.add_scans(dropped);
        }
        ui.add_space(14.0);
        if self.new_project.show_errors && self.new_project.problems().scans {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    icons::WARNING_CIRCLE,
                    t!("np-scans-required")
                ))
                .font(fonts::ui(12.0))
                .color(c.status_danger),
            );
        }
        let mut remove = None;
        let mut cycle = None;
        for (i, s) in self.new_project.scans.iter().enumerate() {
            let failed = matches!(s.probe, Probe::Failed(_));
            egui::Frame::new()
                .fill(c.bg_raised)
                .stroke(if failed {
                    Stroke::new(1.0, c.status_danger)
                } else {
                    Stroke::NONE
                })
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(icons::CUBE)
                                .font(fonts::icon(18.0))
                                .color(c.text_secondary),
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(
                                RichText::new(file_name(&s.path))
                                    .font(fonts::semibold(13.0))
                                    .color(c.text_primary),
                            );
                            probe_line(ui, c, &s.probe);
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button_sized(ui, icons::X, t!("remove"), 26.0, true)
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            if role_badge(ui, s).clicked() {
                                cycle = Some(i);
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            self.new_project.remove_scan(i);
        }
        if let Some(i) = cycle {
            self.new_project.cycle_scan(i);
        }
        ui.add_space(8.0);
        // preview
        let w = ui.available_width();
        let h = ui.available_height().clamp(180.0, 280.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            c.bg_viewport,
            Stroke::new(1.0, c.border_subtle),
            StrokeKind::Inside,
        );
        if let Some(first) = self.new_project.scans.first() {
            paint::arch(ui.painter(), c, rect.shrink2(vec2(40.0, 40.0)));
            let name = file_name(&first.path);
            ui.painter().text(
                rect.center_bottom() - vec2(0.0, 14.0),
                egui::Align2::CENTER_BOTTOM,
                t!("np-preview", name = name),
                fonts::ui(12.0),
                c.text_secondary,
            );
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                t!("np-preview-empty"),
                fonts::ui(12.0),
                c.text_secondary,
            );
        }
    }

    fn np_footer(&mut self, ui: &mut Ui, c: &Palette) {
        ui.label(
            RichText::new(icons::FLOPPY_DISK)
                .font(fonts::icon(16.0))
                .color(c.text_secondary),
        );
        ui.label(
            RichText::new(crate::diagnostics::tilde(&self.new_project.path()))
                .font(fonts::mono(12.0))
                .color(c.text_secondary),
        );
        if widgets::link(ui, t!("np-change-location")).clicked() {
            let p = self.new_project.path();
            self.open_dialog(
                Purpose::NewProjectLocation,
                Spec {
                    title: t!("np-change-location"),
                    filter: Some((
                        t!("dialog-filter-project"),
                        vec![dc_project::FILE_EXTENSION],
                    )),
                    directory: p.parent().map(Path::to_path_buf),
                    file_name: p.file_name().map(|n| n.to_string_lossy().into_owned()),
                    save: true,
                    ..Spec::default()
                },
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if Button::primary(t!("np-create"))
                .icon(icons::CHECK)
                .large()
                .show(ui)
                .clicked()
            {
                self.new_project.show_errors = true;
                self.new_project.error = None;
                self.new_project.failures.clear();
                if let Some(req) = self.new_project.request() {
                    self.start_create(req);
                }
            }
            if Button::ghost(t!("cancel"))
                .icon(icons::X)
                .show(ui)
                .clicked()
            {
                self.screen = Screen::Home;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::sync_channel;
    use std::time::Instant;

    const TRI_STL: &str = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid t\n";

    /// A probe that answers at once without touching the disk.
    fn instant() -> Arc<ProbeFn> {
        Arc::new(|_, _, _| {
            Ok(ScanFacts {
                bytes: 1_048_576,
                triangles: 12,
                units: Units::Millimeters,
                units_suspect: None,
            })
        })
    }

    fn state() -> NewProjectState {
        NewProjectState::with_probe(Path::new("/projects"), instant())
    }

    fn wait_probes(s: &mut NewProjectState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.probing() {
            assert!(Instant::now() < deadline, "probe did not finish");
            s.poll_probes();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn form_validation_and_roles() {
        let mut s = state();
        assert!(!s.problems().ok());
        assert!(s.request().is_none());
        s.case_id = "AL-1".into();
        s.alias = "BIA-07".into();
        s.add_scans(vec![
            PathBuf::from("/x/BIA_lower.stl"),
            PathBuf::from("/x/a.obj"),
            PathBuf::from("/x/notes.txt"),
            PathBuf::from("/x/a.obj"),
        ]);
        assert_eq!(s.scans.len(), 2);
        // the name says lower; the other file says nothing: auto, not a guess
        assert_eq!(s.scans[0].hint, Some(ScanRole::Lower));
        assert_eq!(s.scans[0].shown(), Some(ScanRole::Lower));
        assert_eq!(s.scans[1].hint, None);
        assert_eq!(s.scans[1].shown(), None);
        s.jaw = Jaw::Lower;
        s.notes = " mind the 36\n".into();
        let r = s.request().unwrap();
        assert_eq!(r.path, Path::new("/projects/AL-1_BIA-07.dcprj"));
        assert_eq!(r.jaws, Jaw::Lower);
        assert_eq!(r.notes, "mind the 36");
        // no row has a role of the user's: both go in as auto, the hinted one
        // is read from its name by the import
        assert!(r.scans.iter().all(|(_, role)| role.is_none()));
        // the jaw field does not change any row
        assert_eq!(s.scans[1].shown(), None);
    }

    #[test]
    fn the_folder_name_is_never_a_hint() {
        let mut s = state();
        s.add_scans(vec![
            PathBuf::from("/cases/upper/scan.stl"),
            PathBuf::from("/cases/lower_jaw/mandibola/scan2.stl"),
            PathBuf::from("/cases/x/upper.stl"),
        ]);
        assert_eq!(s.scans[0].hint, None);
        assert_eq!(s.scans[0].shown(), None);
        assert_eq!(s.scans[1].hint, None);
        assert_eq!(s.scans[2].hint, Some(ScanRole::Upper));
    }

    #[test]
    fn extensions_in_any_letter_case_are_accepted() {
        let mut s = state();
        s.add_scans(vec![
            PathBuf::from("/x/A.STL"),
            PathBuf::from("/x/b.Ply"),
            PathBuf::from("/x/c.OBJ"),
            PathBuf::from("/x/d.txt"),
            PathBuf::from("/x/noext"),
        ]);
        assert_eq!(s.scans.len(), 3);
        // the dialog filter lists both cases of every extension
        for e in ["stl", "ply", "obj"] {
            assert!(DIALOG_EXTENSIONS.contains(&e));
            assert!(DIALOG_EXTENSIONS.contains(&e.to_uppercase().as_str()));
        }
    }

    fn roles(s: &mut ScanEntry, clicks: usize) -> Vec<(Option<ScanRole>, bool)> {
        (0..clicks)
            .map(|_| {
                s.cycle();
                (s.shown(), s.choice.is_some())
            })
            .collect()
    }

    #[test]
    fn badge_cycle_without_a_hint_is_auto_upper_lower_bite() {
        let mut s = state();
        s.add_scans(vec![PathBuf::from("/x/scan.stl")]);
        let row = &mut s.scans[0];
        assert_eq!((row.shown(), row.choice), (None, None), "starts as Auto");
        assert_eq!(
            roles(row, 5),
            [
                (Some(ScanRole::Upper), true),
                (Some(ScanRole::Lower), true),
                (Some(ScanRole::Bite), true),
                (None, false), // back to Auto
                (Some(ScanRole::Upper), true),
            ]
        );
    }

    #[test]
    fn a_hinted_row_shows_the_hint_and_a_click_makes_it_explicit() {
        let mut s = state();
        s.add_scans(vec![PathBuf::from("/x/caso_upper.stl")]);
        let row = &mut s.scans[0];
        assert_eq!((row.shown(), row.choice), (Some(ScanRole::Upper), None));
        // a click leaves the hint for an explicit role; Auto is not on the way
        // (the name still counts), the last click gives the hint back
        assert_eq!(
            roles(row, 4),
            [
                (Some(ScanRole::Lower), true),
                (Some(ScanRole::Bite), true),
                (Some(ScanRole::Upper), false),
                (Some(ScanRole::Lower), true),
            ]
        );
        // a bite hint: the same, through Upper and Lower
        s.add_scans(vec![PathBuf::from("/x/bite.stl")]);
        let row = &mut s.scans[1];
        assert_eq!(row.hint, Some(ScanRole::Bite));
        assert_eq!(
            roles(row, 3),
            [
                (Some(ScanRole::Upper), true),
                (Some(ScanRole::Lower), true),
                (Some(ScanRole::Bite), false),
            ]
        );
    }

    #[test]
    fn only_an_explicit_choice_goes_into_the_request_as_a_role() {
        let mut s = state();
        s.case_id = "AL-1".into();
        s.alias = "B".into();
        s.add_scans(vec![
            PathBuf::from("/x/a.stl"),
            PathBuf::from("/x/b_lower.stl"),
            PathBuf::from("/x/c.stl"),
        ]);
        s.scans[0].cycle(); // Auto -> Upper
        s.scans[1].cycle(); // hint Lower -> explicit Bite
        s.scans[1].cycle();
        let r = s.request().unwrap();
        let roles: Vec<_> = r.scans.iter().map(|(_, r)| *r).collect();
        assert_eq!(roles, [Some(ScanRole::Upper), Some(ScanRole::Upper), None]);
    }

    #[test]
    fn probes_run_off_the_ui_thread() {
        // a probe that waits for the test: if add_scans ran it inline, this
        // would deadlock
        let (release, gate) = sync_channel::<()>(0);
        let gate = Mutex::new(gate);
        let ui_thread = std::thread::current().id();
        let seen = Arc::new(Mutex::new(None));
        let seen_in = seen.clone();
        let probe: Arc<ProbeFn> = Arc::new(move |_, _, _| {
            *seen_in.lock().unwrap() = Some(std::thread::current().id());
            gate.lock().unwrap().recv().ok();
            Ok(ScanFacts {
                bytes: 10,
                triangles: 3,
                units: Units::Centimeters,
                units_suspect: None,
            })
        });
        let mut s = NewProjectState::with_probe(Path::new("/p"), probe);
        s.add_scans(vec![PathBuf::from("/x/a.stl")]);
        assert_eq!(s.scans[0].probe, Probe::Pending);
        assert!(s.probing());
        assert!(!s.poll_probes(), "nothing came back yet");
        // a second file is added and shown while the first still reads
        s.add_scans(vec![PathBuf::from("/x/b.stl")]);
        assert_eq!(s.scans.len(), 2);
        release.send(()).unwrap();
        release.send(()).unwrap();
        wait_probes(&mut s);
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.triangles == 3));
        let worker = seen.lock().unwrap().expect("the probe ran");
        assert_ne!(worker, ui_thread);
    }

    #[test]
    fn a_removed_rows_late_result_is_ignored() {
        let (release, gate) = sync_channel::<()>(0);
        let gate = Mutex::new(gate);
        let cancelled_seen = Arc::new(Mutex::new(false));
        let flag = cancelled_seen.clone();
        let probe: Arc<ProbeFn> = Arc::new(move |path, _, ctx| {
            if path.ends_with("slow.stl") {
                gate.lock().unwrap().recv().ok();
                *flag.lock().unwrap() = ctx.is_cancelled();
            }
            Ok(ScanFacts {
                bytes: 1,
                triangles: 1,
                units: Units::Millimeters,
                units_suspect: None,
            })
        });
        let mut s = NewProjectState::with_probe(Path::new("/p"), probe);
        s.add_scans(vec![
            PathBuf::from("/x/slow.stl"),
            PathBuf::from("/x/fast.stl"),
        ]);
        // the fast one finishes, the slow one is removed, then its answer comes
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(s.scans[1].probe, Probe::Ready(_)) {
            assert!(Instant::now() < deadline);
            s.poll_probes();
            std::thread::sleep(Duration::from_millis(5));
        }
        s.remove_scan(0);
        release.send(()).unwrap();
        // the worker saw the cancellation of the removed row
        let deadline = Instant::now() + Duration::from_secs(10);
        while !*cancelled_seen.lock().unwrap() {
            assert!(Instant::now() < deadline, "the probe was not cancelled");
            std::thread::sleep(Duration::from_millis(5));
        }
        // the late answer reaches the channel and is dropped: it does not
        // touch the row that is left, and does not make a new one
        std::thread::sleep(Duration::from_millis(50));
        assert!(!s.poll_probes());
        assert_eq!(s.scans.len(), 1);
        assert!(s.scans[0].path.ends_with("fast.stl"));
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.triangles == 1));
    }

    #[test]
    fn a_bad_file_shows_in_its_row_and_blocks_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (bad, empty, good) = (
            dir.join("bad.stl"),
            dir.join("empty.stl"),
            dir.join("ok.stl"),
        );
        std::fs::write(&bad, "not a mesh at all").unwrap();
        std::fs::write(&empty, "").unwrap();
        std::fs::write(&good, TRI_STL).unwrap();
        let mut s = NewProjectState::new(Path::new("/p"));
        s.case_id = "A".into();
        s.alias = "B".into();
        s.add_scans(vec![bad, empty, good]);
        wait_probes(&mut s);
        assert!(matches!(s.scans[0].probe, Probe::Failed(_)));
        assert!(matches!(s.scans[1].probe, Probe::Failed(_)));
        match &s.scans[2].probe {
            Probe::Ready(f) => assert_eq!(f.triangles, 1),
            other => panic!("{other:?}"),
        }
        // the form still creates: the import reports the bad rows
        assert_eq!(s.request().unwrap().scans.len(), 3);
    }

    /// A probe that records the options and the name of every read, and
    /// answers like the real one: a file read with the units kept is in mm.
    type Reads = Arc<Mutex<Vec<(String, bool)>>>;

    fn recording() -> (Arc<ProbeFn>, Reads) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let probe: Arc<ProbeFn> = Arc::new(move |path, options, _| {
            log.lock()
                .unwrap()
                .push((file_name(path), options.normalize_units));
            Ok(ScanFacts {
                bytes: 1,
                triangles: 1,
                units: if options.normalize_units {
                    Units::Centimeters
                } else {
                    Units::Millimeters
                },
                units_suspect: (!options.normalize_units).then(|| "cm".to_owned()),
            })
        });
        (probe, seen)
    }

    fn reads(seen: &Mutex<Vec<(String, bool)>>) -> Vec<(String, bool)> {
        let mut v = seen.lock().unwrap().clone();
        v.sort();
        v
    }

    #[test]
    fn a_bite_row_is_probed_with_the_units_kept() {
        let (probe, seen) = recording();
        let mut s = NewProjectState::with_probe(Path::new("/p"), probe);
        s.add_scans(vec![
            PathBuf::from("/x/bite.stl"),
            PathBuf::from("/x/lower.stl"),
            PathBuf::from("/x/scan.stl"),
        ]);
        wait_probes(&mut s);
        // the file name sets the bite role: the import keeps its units, so
        // does the probe; the others are converted
        assert_eq!(
            reads(&seen),
            [
                ("bite.stl".to_owned(), false),
                ("lower.stl".to_owned(), true),
                ("scan.stl".to_owned(), true)
            ]
        );
        let Probe::Ready(f) = &s.scans[0].probe else {
            panic!("{:?}", s.scans[0].probe)
        };
        assert_eq!(units_text(f.units), "mm");
        assert_eq!(f.units_suspect.as_deref(), Some("cm"));
        let Probe::Ready(f) = &s.scans[1].probe else {
            panic!()
        };
        assert_eq!(f.units, Units::Centimeters);
        assert_eq!(f.units_suspect, None);
    }

    #[test]
    fn the_badge_going_to_or_from_bite_probes_the_row_again() {
        let (probe, seen) = recording();
        let mut s = NewProjectState::with_probe(Path::new("/p"), probe);
        s.add_scans(vec![PathBuf::from("/x/scan.stl")]);
        wait_probes(&mut s);
        let count = || seen.lock().unwrap().len();
        assert_eq!(count(), 1);
        // Auto -> Upper -> Lower: still converted, no new read
        s.cycle_scan(0);
        s.cycle_scan(0);
        wait_probes(&mut s);
        assert_eq!(count(), 1);
        // Lower -> Bite: read again with the units kept
        s.cycle_scan(0);
        assert_eq!(s.scans[0].probe, Probe::Pending);
        wait_probes(&mut s);
        assert_eq!(count(), 2);
        assert_eq!(seen.lock().unwrap()[1], ("scan.stl".to_owned(), false));
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.units == Units::Millimeters));
        // Bite -> Auto: back to the conversion
        s.cycle_scan(0);
        wait_probes(&mut s);
        assert_eq!(seen.lock().unwrap()[2], ("scan.stl".to_owned(), true));
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.units == Units::Centimeters));
        // a name that says bite: leaving it for Upper reads it converted
        s.add_scans(vec![PathBuf::from("/x/bite.stl")]);
        wait_probes(&mut s);
        let before = count();
        s.cycle_scan(1); // bite hint -> Upper
        wait_probes(&mut s);
        assert_eq!(count(), before + 1);
        assert_eq!(
            seen.lock().unwrap().last().unwrap(),
            &("bite.stl".to_owned(), true)
        );
        assert!(!s.scans[1].is_bite());
    }

    #[test]
    fn a_stale_probe_never_overwrites_the_new_one() {
        // the first read of the row is slow and answers last
        let (release, gate) = sync_channel::<()>(0);
        let gate = Mutex::new(gate);
        let probe: Arc<ProbeFn> = Arc::new(move |_, options, ctx| {
            if options.normalize_units {
                gate.lock().unwrap().recv().ok();
                assert!(ctx.is_cancelled(), "the replaced probe is cancelled");
            }
            Ok(ScanFacts {
                bytes: 1,
                triangles: 1,
                units: if options.normalize_units {
                    Units::Centimeters
                } else {
                    Units::Millimeters
                },
                units_suspect: None,
            })
        });
        let mut s = NewProjectState::with_probe(Path::new("/p"), probe);
        s.add_scans(vec![PathBuf::from("/x/scan.stl")]);
        s.cycle_scan(0);
        s.cycle_scan(0);
        s.cycle_scan(0); // Bite: the converted read is stale
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.scans[0].probe == Probe::Pending {
            assert!(Instant::now() < deadline);
            s.poll_probes();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.units == Units::Millimeters));
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!s.poll_probes(), "the stale answer is dropped");
        assert!(matches!(&s.scans[0].probe, Probe::Ready(f) if f.units == Units::Millimeters));
    }

    #[test]
    fn the_real_probe_keeps_the_units_of_a_bite_and_says_when_they_look_wrong() {
        let tmp = tempfile::tempdir().unwrap();
        let bite = tmp.path().join("bite.stl");
        let other = tmp.path().join("scan.stl");
        // 10 units wide: centimetres for an arch, a suspect size for a bite
        std::fs::write(&bite, TRI_STL).unwrap();
        std::fs::write(&other, TRI_STL).unwrap();
        let mut s = NewProjectState::new(Path::new("/p"));
        s.add_scans(vec![bite, other]);
        wait_probes(&mut s);
        let Probe::Ready(b) = &s.scans[0].probe else {
            panic!("{:?}", s.scans[0].probe)
        };
        assert_eq!(b.units, Units::Millimeters);
        assert_eq!(b.units_suspect.as_deref(), Some("cm"));
        let Probe::Ready(o) = &s.scans[1].probe else {
            panic!("{:?}", s.scans[1].probe)
        };
        assert_eq!(o.units, Units::Centimeters);
        assert_eq!(o.units_suspect, None);
        // the same file as an upper arch: converted
        s.cycle_scan(0); // bite hint -> Upper
        wait_probes(&mut s);
        let Probe::Ready(b) = &s.scans[0].probe else {
            panic!()
        };
        assert_eq!(
            (b.units, b.units_suspect.clone()),
            (Units::Centimeters, None)
        );
    }

    #[test]
    fn thousands_are_grouped_by_language() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(855_210), "855,210");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(units_text(Units::Millimeters), "mm");
        assert_eq!(units_text(Units::Centimeters), "cm \u{2192} mm");
        assert_eq!(units_text(Units::Micrometers), "\u{b5}m \u{2192} mm");
    }
}
