//! `04 New project`: 1 Module, 2 Case, 3 Scans, footer with the save path.
//! "Create" reads the scans on a worker thread ([`crate::create_job`]),
//! saves the `.dcprj` and opens the workspace.

use std::path::{Path, PathBuf};

use dc_case::ScanRole;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{Palette, fonts, icons, tokens};
use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::app::{DentalCraftApp, Screen};
use crate::create_job::{NewProjectRequest, file_stem};
use crate::dialogs::{Purpose, Spec};
use crate::paint;

/// Accepted scan extensions.
pub const SCAN_EXTENSIONS: [&str; 3] = ["stl", "ply", "obj"];

/// A scan chosen for the new project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    /// File.
    pub path: PathBuf,
    /// Size in bytes.
    pub bytes: u64,
    /// Role (from the file name, changeable).
    pub role: ScanRole,
}

/// Jaw choice (stored in the case).
pub use dc_case::Jaws as Jaw;

/// New project form state.
#[derive(Debug, Clone)]
pub struct NewProjectState {
    /// Selected module id.
    pub module: String,
    /// Case ID.
    pub case_id: String,
    /// Patient alias.
    pub alias: String,
    /// Jaw.
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
    projects_dir: PathBuf,
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
            projects_dir: projects_dir.to_path_buf(),
        }
    }

    /// Adds scan files (other extensions and duplicates are ignored); roles
    /// come from the file names.
    pub fn add_scans(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let ok = p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SCAN_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if !ok || self.scans.iter().any(|s| s.path == p) {
                continue;
            }
            let existing: Vec<ScanRole> = self.scans.iter().map(|s| s.role).collect();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let hint = dc_case::role_from_name(&name);
            // the name's role, else the first of upper, lower, bite not taken
            let role = hint.unwrap_or_else(|| {
                [ScanRole::Upper, ScanRole::Lower, ScanRole::Bite]
                    .into_iter()
                    .find(|r| !existing.contains(r))
                    .unwrap_or(ScanRole::Upper)
            });
            let role = match (self.jaw, hint) {
                (Jaw::Lower, None) if role == ScanRole::Upper => ScanRole::Lower,
                _ => role,
            };
            let bytes = std::fs::metadata(&p).map_or(0, |m| m.len());
            self.scans.push(ScanEntry {
                path: p,
                bytes,
                role,
            });
        }
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

    /// The creation request when the form is valid.
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
                .map(|s| (s.path.clone(), s.role))
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
                filter: Some((t!("dialog-filter-scans"), SCAN_EXTENSIONS.to_vec())),
                multiple: true,
                ..Spec::default()
            },
        );
    }

    pub(crate) fn new_project_ui(&mut self, ui: &mut Ui) {
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
                        if let Some(e) = self.new_project.error.clone() {
                            egui::Frame::new()
                                .fill(c.bg_raised)
                                .stroke(Stroke::new(1.0, c.status_danger))
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(Margin::same(12))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    text(
                                        ui,
                                        format!(
                                            "{}  {}",
                                            icons::X_CIRCLE,
                                            t!("np-create-failed", reason = e)
                                        ),
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
            egui::Frame::new()
                .fill(c.bg_raised)
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
                            let name = s
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(name)
                                    .font(fonts::semibold(13.0))
                                    .color(c.text_primary),
                            );
                            ui.label(
                                RichText::new(format!("{:.1} MB", s.bytes as f64 / 1_048_576.0))
                                    .font(fonts::mono(11.0))
                                    .color(c.text_secondary),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button_sized(ui, icons::X, t!("remove"), 26.0, true)
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            let icon = match s.role {
                                ScanRole::Upper => icons::CARET_UP,
                                ScanRole::Lower => icons::CARET_DOWN,
                                ScanRole::Bite => icons::ARROWS_LEFT_RIGHT,
                            };
                            let b = widgets::badge_icon(
                                ui,
                                BadgeKind::Info,
                                Some(icon),
                                role_key(s.role),
                            );
                            if ui
                                .interact(b.rect, b.id.with("cycle"), Sense::click())
                                .on_hover_text(t!("np-role-change"))
                                .clicked()
                            {
                                cycle = Some(i);
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }
        if let Some(i) = remove {
            self.new_project.scans.remove(i);
        }
        if let Some(i) = cycle {
            let s = &mut self.new_project.scans[i];
            s.role = match s.role {
                ScanRole::Upper => ScanRole::Lower,
                ScanRole::Lower => ScanRole::Bite,
                ScanRole::Bite => ScanRole::Upper,
            };
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
            let name = first
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
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

    #[test]
    fn form_validation_and_roles() {
        let mut s = NewProjectState::new(Path::new("/projects"));
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
        assert_eq!(s.scans[0].role, ScanRole::Lower);
        assert_eq!(s.scans[1].role, ScanRole::Upper);
        s.jaw = Jaw::Lower;
        s.notes = " mind the 36\n".into();
        let r = s.request().unwrap();
        assert_eq!(r.path, Path::new("/projects/AL-1_BIA-07.dcprj"));
        assert_eq!(r.jaws, Jaw::Lower);
        assert_eq!(r.notes, "mind the 36");
    }
}
