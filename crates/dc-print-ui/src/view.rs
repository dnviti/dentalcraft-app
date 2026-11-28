//! The window of the print popup: on the left what to print with (printer,
//! resin), the advanced settings when they are on, the parts and Prepare;
//! on the right the plate seen from above, the progress, the estimates and
//! the save.

use std::path::PathBuf;
use std::time::Duration;

use dc_print::PrinterProfile;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{fonts, tokens::Palette};
use egui::{
    Align, Align2, Color32, CornerRadius, Id, Key, Layout, Margin, Modal, Modifiers, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::form::{FormAct, ResinForm};
use crate::work::{FittingSource, Outline, Plate, Step};
use crate::{Message, Output, Phase, PrintPopup, caption, t, tr};

/// Width of the left column.
const LEFT_W: f32 = 360.0;

/// What the controls ask for, applied after the frame is drawn.
enum Act {
    Close,
    Prepare,
    Cancel,
    Save,
    SaveModels,
    EditResin,
    Form(FormAct),
    Choose(usize, bool),
    ChooseAll(bool),
    ChooseMissing,
    Plate(usize),
    Changed,
}

/// `secs` as "1 h 20 min" / "35 min".
fn duration(secs: u64) -> String {
    let min = secs.div_ceil(60);
    let (h, m) = (min / 60, min % 60);
    if h == 0 {
        tr("print-time-m", &[("m", &m.to_string())])
    } else {
        tr(
            "print-time-hm",
            &[("h", &h.to_string()), ("m", &m.to_string())],
        )
    }
}

/// One decimal (`12.3`).
fn one(v: f64) -> String {
    format!("{v:.1}")
}

/// A section title of a column.
fn section(ui: &mut Ui, key: &str) {
    let c = *colors(ui);
    ui.add_space(14.0);
    ui.label(
        RichText::new(t(key))
            .font(fonts::semibold(14.0))
            .color(c.text_primary),
    );
    ui.add_space(6.0);
}

/// A label and its value on one line.
fn value_row(ui: &mut Ui, label: &str, value: &str) {
    let c = *colors(ui);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(fonts::ui(13.0))
                .color(c.text_secondary),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
        });
    });
}

/// The folder of `files` (the first one's).
fn folder_of(files: &[PathBuf]) -> String {
    files
        .first()
        .and_then(|f| f.parent())
        .map_or_else(String::new, |d| d.display().to_string())
}

impl PrintPopup {
    /// Draws the window, when open, and runs what its controls ask for.
    /// Call it every frame, under the host's own dialogs; it polls too.
    pub fn show(&mut self, ui: &mut Ui) {
        self.poll();
        let ctx = ui.ctx().clone();
        if self.is_busy() || self.folder.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if !self.open {
            return;
        }
        let adv = dc_module_api::advanced_ui(&ctx);
        let screen = ctx.content_rect();
        let size = vec2(
            (screen.width() * 0.92).min(1040.0),
            (screen.height() * 0.9).min(760.0),
        ) - vec2(32.0, 32.0);
        let mut acts = Vec::new();
        let resp = Modal::new(Id::unique("dc-print-popup"))
            .frame(widgets::card_frame(ui).inner_margin(Margin::same(16)))
            .show(&ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                self.contents(ui, adv, &mut acts);
            });
        if resp.is_top_modal
            && !resp.any_popup_open
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
        {
            acts.push(Act::Close);
        }
        for act in acts {
            self.apply(act);
        }
    }

    fn apply(&mut self, act: Act) {
        match act {
            Act::Close => self.close(),
            Act::Prepare => {
                self.prepare();
            }
            Act::Cancel => self.cancel(),
            Act::Save => self.ask_folder(Output::PrintFiles),
            Act::SaveModels => self.ask_folder(Output::PlateModels),
            Act::EditResin => self.form = Some(ResinForm::from_resin(self.resin.as_ref())),
            Act::Form(a) => self.form_answer(a),
            Act::Choose(i, on) => {
                if let Some(id) = self.rows.get(i).map(|r| r.item.id.clone()) {
                    self.set_chosen(&id, on);
                }
            }
            Act::ChooseAll(on) => {
                for r in &mut self.rows {
                    r.checked = on && r.item.path.is_some();
                }
                self.invalidate();
            }
            Act::ChooseMissing => {
                for r in &mut self.rows {
                    r.checked = r.item.status.preselected() && r.item.path.is_some();
                }
                self.invalidate();
            }
            Act::Plate(i) => self.plate = i,
            Act::Changed => {
                self.resin_changed = true;
                self.invalidate();
            }
        }
    }

    fn contents(&mut self, ui: &mut Ui, adv: bool, acts: &mut Vec<Act>) {
        let c = *colors(ui);
        // title and close
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(ic::PRINTER)
                    .font(fonts::icon(20.0))
                    .color(c.text_primary),
            );
            ui.label(
                RichText::new(t("print-title"))
                    .font(fonts::semibold(18.0))
                    .color(c.text_primary),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, ic::X, t("print-close")).clicked() {
                    acts.push(Act::Close);
                }
            });
        });
        ui.add_space(8.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.allocate_ui_with_layout(vec2(LEFT_W, height), Layout::top_down(Align::Min), |ui| {
                ui.set_width(LEFT_W);
                self.left(ui, adv, height, acts);
            });
            widgets::divider_v(ui, height);
            let w = ui.available_width();
            ui.allocate_ui_with_layout(vec2(w, height), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                self.right(ui, adv, acts);
            });
        });
    }

    // ------------------------------------------------------------ left

    fn left(&mut self, ui: &mut Ui, adv: bool, height: f32, acts: &mut Vec<Act>) {
        let busy = self.is_busy();
        // the Prepare button and its hint stay at the bottom
        let foot = 76.0;
        ScrollArea::vertical()
            .id_salt("dc-print-left")
            .auto_shrink([false, false])
            .max_height((height - foot).max(80.0))
            .show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    self.printer_section(ui);
                    self.resin_section(ui, acts);
                    if adv && self.resin.is_some() && self.form.is_none() {
                        self.advanced_sections(ui, acts);
                    }
                    self.parts_section(ui, acts);
                });
            });
        ui.add_space(8.0);
        let can = self.can_prepare() && self.form.is_none();
        if Button::primary(t("print-prepare"))
            .icon(ic::STACK)
            .enabled(can)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            acts.push(Act::Prepare);
        }
        if !busy {
            let hint = if self.resin.is_none() {
                Some("print-hint-no-resin")
            } else if self.form.is_some() {
                Some("print-hint-form-open")
            } else if !self.rows.iter().any(|r| r.checked) {
                Some("print-hint-no-parts")
            } else {
                None
            };
            if let Some(key) = hint {
                ui.add_space(4.0);
                caption(ui, t(key));
            }
        }
    }

    fn printer_section(&self, ui: &mut Ui) {
        let c = *colors(ui);
        section(ui, "print-printer");
        widgets::sunken_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(&self.profile.machine_name)
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
            // the printer's own parameters: the profile name and the vat
            // temperature it holds
            if let Some(m) = &self.profile.machine {
                caption(
                    ui,
                    tr(
                        "print-printer-profile",
                        &[
                            ("profile", &m.name),
                            ("c", &format!("{:.0}", m.tank_target_c)),
                        ],
                    ),
                );
            }
            caption(ui, t("print-printer-usb"));
        });
    }

    fn resin_section(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let c = *colors(ui);
        section(ui, "print-resin");
        if let Some(form) = &mut self.form {
            let can_cancel = self.resin.is_some();
            if let Some(a) = form.show(ui, &self.profile, can_cancel) {
                acts.push(Act::Form(a));
            }
            return;
        }
        match &self.resin {
            Some(r) => {
                ui.label(
                    RichText::new(&r.name)
                        .font(fonts::semibold(13.0))
                        .color(c.text_primary),
                );
                caption(
                    ui,
                    tr(
                        "print-resin-summary",
                        &[
                            ("layer", &format!("{}", r.layer_um as f32 / 1000.0)),
                            ("exposure", &format!("{}", r.exposure_s)),
                            ("bottom", &r.bottom_layers.to_string()),
                            ("bottom_exposure", &format!("{}", r.bottom_exposure_s)),
                        ],
                    ),
                );
                ui.add_space(2.0);
                if widgets::link(ui, t("print-resin-change")).clicked() {
                    acts.push(Act::EditResin);
                }
            }
            None => {
                widgets::badge(ui, BadgeKind::Warning, t("print-resin-none"));
                ui.add_space(4.0);
                caption(ui, t("print-resin-none-body"));
                ui.add_space(6.0);
                if Button::secondary(t("print-resin-enter"))
                    .icon(ic::DROP)
                    .show(ui)
                    .clicked()
                {
                    acts.push(Act::EditResin);
                }
            }
        }
    }

    /// The advanced settings: the resin's layer values, the supports and
    /// the spacing on the plate.
    fn advanced_sections(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let p = &self.profile;
        let mut changed = false;
        if let Some(r) = &mut self.resin {
            section(ui, "print-adv-settings");
            let mut layer = r.layer_um as f32 / 1000.0;
            let lo = p.layer_min_um as f32 / 1000.0;
            let hi = p.layer_max_um as f32 / 1000.0;
            if widgets::labeled_slider(
                ui,
                &t("print-form-layer"),
                &mut layer,
                lo..=hi,
                0.01,
                2,
                "mm",
                0.05,
            )
            .changed()
            {
                r.layer_um = (layer * 1000.0).round() as u32;
                changed = true;
            }
            let mut bottom = r.bottom_layers as f32;
            if widgets::labeled_slider(
                ui,
                &t("print-form-bottom-layers"),
                &mut bottom,
                0.0..=20.0,
                1.0,
                0,
                "",
                4.0,
            )
            .changed()
            {
                r.bottom_layers = bottom.round() as u32;
                changed = true;
            }
            changed |= widgets::labeled_slider(
                ui,
                &t("print-form-bottom-exposure"),
                &mut r.bottom_exposure_s,
                1.0..=120.0,
                0.1,
                1,
                "s",
                30.0,
            )
            .changed();
            changed |= widgets::labeled_slider(
                ui,
                &t("print-form-rest"),
                &mut r.rest_s,
                0.0..=10.0,
                0.1,
                1,
                "s",
                1.0,
            )
            .changed();
        }
        section(ui, "print-adv-supports");
        let s = &mut self.support;
        changed |=
            widgets::labeled_slider_mm(ui, &t("print-adv-tip"), &mut s.tip_d_mm, 0.1..=1.0, 0.35)
                .changed();
        changed |= widgets::labeled_slider_mm(
            ui,
            &t("print-adv-shaft"),
            &mut s.shaft_d_mm,
            0.4..=3.0,
            1.0,
        )
        .changed();
        changed |=
            widgets::labeled_slider_mm(ui, &t("print-adv-raft"), &mut s.raft_mm, 0.0..=3.0, 1.0)
                .changed();
        section(ui, "print-adv-plate");
        let l = &mut self.layout;
        changed |=
            widgets::labeled_slider_mm(ui, &t("print-adv-gap"), &mut l.gap_mm, 1.0..=10.0, 3.0)
                .changed();
        changed |= widgets::labeled_slider_mm(
            ui,
            &t("print-adv-margin"),
            &mut l.margin_mm,
            1.0..=15.0,
            4.0,
        )
        .changed();
        if changed {
            acts.push(Act::Changed);
        }
    }

    fn parts_section(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let c = *colors(ui);
        section(ui, "print-parts");
        if self.rows.is_empty() {
            caption(ui, t("print-parts-none"));
            return;
        }
        let chosen = self.rows.iter().filter(|r| r.checked).count();
        caption(
            ui,
            tr(
                "print-parts-chosen",
                &[
                    ("n", &chosen.to_string()),
                    ("of", &self.rows.len().to_string()),
                ],
            ),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if widgets::link(ui, t("print-parts-missing")).clicked() {
                acts.push(Act::ChooseMissing);
            }
            if widgets::link(ui, t("print-parts-all")).clicked() {
                acts.push(Act::ChooseAll(true));
            }
            if widgets::link(ui, t("print-parts-none-chosen")).clicked() {
                acts.push(Act::ChooseAll(false));
            }
        });
        ui.add_space(6.0);
        for (i, row) in self.rows.iter().enumerate() {
            ui.horizontal(|ui| {
                let (kind, key) = row.item.status.badge();
                let badge_w = 120.0;
                let label_w = (ui.available_width() - badge_w).max(80.0);
                ui.allocate_ui_with_layout(
                    vec2(label_w, 24.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if row.item.path.is_some() {
                            let mut on = row.checked;
                            if widgets::checkbox(ui, &mut on, &row.item.label).changed() {
                                acts.push(Act::Choose(i, on));
                            }
                        } else {
                            ui.add_space(26.0);
                            ui.label(
                                RichText::new(&row.item.label)
                                    .font(fonts::semibold(13.0))
                                    .color(c.text_disabled),
                            )
                            .on_hover_text(t("print-part-no-file"));
                        }
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::badge(ui, kind, t(key));
                });
            });
            if row.item.path.is_none() {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    caption(ui, t("print-part-no-file"));
                });
            }
        }
    }

    // ------------------------------------------------------------ right

    fn right(&mut self, ui: &mut Ui, adv: bool, acts: &mut Vec<Act>) {
        let prepared = match &self.phase {
            Phase::Ready(p) | Phase::Saving { prepared: p, .. } => Some(std::sync::Arc::clone(p)),
            _ => None,
        };
        let count = prepared.as_ref().map_or(0, |p| p.plates.len());
        let shown = self.plate.min(count.saturating_sub(1));
        // the title and the plate arrows
        ui.horizontal(|ui| {
            let c = *colors(ui);
            let title = if count > 0 {
                tr(
                    "print-plate-n",
                    &[("n", &(shown + 1).to_string()), ("of", &count.to_string())],
                )
            } else {
                t("print-plate")
            };
            ui.label(
                RichText::new(title)
                    .font(fonts::semibold(14.0))
                    .color(c.text_primary),
            );
            if count > 1 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let next = ui.add_enabled_ui(shown + 1 < count, |ui| {
                        widgets::icon_button(ui, ic::CARET_RIGHT, t("print-plate-next"))
                    });
                    if next.inner.clicked() {
                        acts.push(Act::Plate(shown + 1));
                    }
                    let prev = ui.add_enabled_ui(shown > 0, |ui| {
                        widgets::icon_button(ui, ic::CARET_LEFT, t("print-plate-prev"))
                    });
                    if prev.inner.clicked() {
                        acts.push(Act::Plate(shown - 1));
                    }
                });
            }
        });
        ui.add_space(6.0);
        let plate = prepared.as_ref().and_then(|p| p.plates.get(shown));
        let room = (ui.available_height() - 230.0).max(140.0);
        plate_view(ui, &self.profile, plate, room);
        ui.add_space(10.0);
        self.profile_warnings_view(ui);

        match &self.phase {
            Phase::Preparing(w) => self.progress(ui, w.progress(), acts),
            Phase::Saving { worker, .. } => self.progress(ui, worker.progress(), acts),
            _ => {}
        }
        if let Some(p) = &prepared {
            ScrollArea::vertical()
                .id_salt("dc-print-right")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    self.fitting_view(ui, &p.fitting);
                    if let Some(plate) = plate {
                        estimates(ui, plate);
                    }
                    if p.plates.len() > 1 {
                        totals(ui, &p.plates);
                    }
                    if let Some(plate) = plate {
                        ui.add_space(6.0);
                        preheat(ui, plate);
                    }
                    ui.add_space(10.0);
                    let ready = matches!(self.phase, Phase::Ready(_)) && self.folder.is_none();
                    if Button::primary(t("print-save"))
                        .icon(ic::FLOPPY_DISK)
                        .enabled(ready)
                        .full_width(true)
                        .show(ui)
                        .clicked()
                    {
                        acts.push(Act::Save);
                    }
                    ui.add_space(4.0);
                    caption(ui, t("print-save-help"));
                    if adv {
                        ui.add_space(8.0);
                        if Button::secondary(t("print-save-models"))
                            .icon(ic::CUBE)
                            .enabled(ready)
                            .full_width(true)
                            .show(ui)
                            .clicked()
                        {
                            acts.push(Act::SaveModels);
                        }
                    }
                    self.message_view(ui);
                });
        } else {
            self.message_view(ui);
        }
    }

    fn progress(&self, ui: &mut Ui, (step, fraction): (Step, f32), acts: &mut Vec<Act>) {
        let c = *colors(ui);
        let text = match step {
            Step::Reading => t("print-step-reading"),
            Step::Placing => t("print-step-placing"),
            Step::Layers { n, of } => tr(
                "print-step-layers",
                &[("n", &n.to_string()), ("of", &of.to_string())],
            ),
            Step::Saving => t("print-step-saving"),
        };
        ui.label(
            RichText::new(text)
                .font(fonts::ui(13.0))
                .color(c.text_primary),
        );
        ui.add_space(6.0);
        // placing gives no fraction of its own: the bar runs
        let f = (step != Step::Placing).then_some(fraction);
        widgets::progress_bar(ui, f, ui.available_width());
        ui.add_space(8.0);
        if Button::secondary(t("print-cancel"))
            .icon(ic::X)
            .show(ui)
            .clicked()
        {
            acts.push(Act::Cancel);
        }
    }

    /// The warnings of the printer profile, as every `print.prepare` result
    /// carries them: today only that its scale and left/right orientation
    /// are not yet checked against the printer. Printing stays allowed.
    fn profile_warnings_view(&self, ui: &mut Ui) {
        for code in self.profile.warnings() {
            let (title, body) = match code {
                dc_print::profile::PROFILE_NOT_VALIDATED => (
                    "print-profile-not-validated",
                    "print-profile-not-validated-body",
                ),
                _ => continue,
            };
            widgets::badge(ui, BadgeKind::Warning, t(title));
            ui.add_space(4.0);
            ui.label(RichText::new(t(body)).font(fonts::ui(13.0)));
            ui.add_space(8.0);
        }
    }

    /// Where the fitting side of the prepared parts comes from: a line when
    /// every one comes from its stage model, a warning naming the parts
    /// whose side was guessed from their shape.
    fn fitting_view(&self, ui: &mut Ui, fitting: &[(String, FittingSource)]) {
        let guessed: Vec<String> = fitting
            .iter()
            .filter(|(_, s)| *s == FittingSource::Geometry)
            .map(|(id, _)| {
                self.rows
                    .iter()
                    .find(|r| &r.item.id == id)
                    .map_or_else(|| id.clone(), |r| r.item.label.clone())
            })
            .collect();
        if guessed.is_empty() {
            caption(ui, t("print-fitting-model"));
        } else {
            widgets::badge(ui, BadgeKind::Warning, t("print-fitting-guessed"));
            ui.add_space(4.0);
            ui.label(RichText::new(t("print-fitting-guessed-body")).font(fonts::ui(13.0)));
            caption(
                ui,
                tr(
                    "print-fitting-guessed-parts",
                    &[("parts", &guessed.join(", "))],
                ),
            );
        }
        ui.add_space(8.0);
    }

    fn message_view(&self, ui: &mut Ui) {
        let Some(m) = &self.message else { return };
        ui.add_space(12.0);
        match m {
            Message::Failed(f) => {
                widgets::badge(ui, BadgeKind::Danger, t("print-failed"));
                ui.add_space(4.0);
                ui.label(RichText::new(t(&f.key())).font(fonts::ui(13.0)));
                caption(ui, f.detail.clone());
            }
            Message::Cancelled => {
                widgets::badge(ui, BadgeKind::Neutral, t("print-cancelled"));
            }
            Message::Saved { files, what } => {
                widgets::badge(ui, BadgeKind::Success, t("print-saved"));
                ui.add_space(4.0);
                let key = match what {
                    Output::PrintFiles => "print-saved-files",
                    Output::PlateModels => "print-saved-models",
                };
                ui.label(
                    RichText::new(tr(
                        key,
                        &[
                            ("n", &files.len().to_string()),
                            ("folder", &folder_of(files)),
                        ],
                    ))
                    .font(fonts::ui(13.0)),
                );
                for f in files {
                    caption(
                        ui,
                        f.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    );
                }
                if *what == Output::PrintFiles {
                    ui.add_space(6.0);
                    caption(ui, t("print-next-steps"));
                    caption(ui, t("print-after"));
                }
            }
        }
    }
}

/// What one plate needs.
fn estimates(ui: &mut Ui, plate: &Plate) {
    let s = &plate.summary;
    value_row(ui, &t("print-est-parts"), &s.parts.len().to_string());
    value_row(ui, &t("print-est-layers"), &s.layers.to_string());
    value_row(ui, &t("print-est-time"), &duration(u64::from(s.est_time_s)));
    value_row(
        ui,
        &t("print-est-resin"),
        &tr(
            "print-ml-g",
            &[("ml", &one(s.volume_ml)), ("g", &one(s.weight_g))],
        ),
    );
    value_row(
        ui,
        &t("print-est-resin-needed"),
        &tr(
            "print-ml",
            &[("ml", &format!("{:.0}", s.resin_needed_ml.ceil()))],
        ),
    );
}

/// The warm-up before the first layer, as the plate's estimate counts it.
fn preheat(ui: &mut Ui, plate: &Plate) {
    let s = &plate.summary;
    let time = duration(u64::from(s.preheat_s));
    let text = match s.tank_target_c {
        Some(c) => tr(
            "print-preheat",
            &[("c", &format!("{c:.0}")), ("time", &time)],
        ),
        None => tr("print-preheat-no-target", &[("time", &time)]),
    };
    caption(ui, text);
}

/// What every plate needs together.
fn totals(ui: &mut Ui, plates: &[Plate]) {
    let time: u64 = plates.iter().map(|p| u64::from(p.summary.est_time_s)).sum();
    let ml: f64 = plates.iter().map(|p| p.summary.volume_ml).sum();
    let g: f64 = plates.iter().map(|p| p.summary.weight_g).sum();
    ui.add_space(6.0);
    widgets::hairline(ui);
    ui.add_space(6.0);
    value_row(
        ui,
        &tr("print-total", &[("n", &plates.len().to_string())]),
        &tr(
            "print-total-value",
            &[("time", &duration(time)), ("ml", &one(ml)), ("g", &one(g))],
        ),
    );
}

/// The plate seen from above, at most `max_h` tall: the parts (blue) with
/// their supports (grey), each with its name; an empty plate before the
/// parts are placed.
fn plate_view(ui: &mut Ui, profile: &PrinterProfile, plate: Option<&Plate>, max_h: f32) {
    let c = *colors(ui);
    let [w_mm, h_mm, _] = profile.build_mm;
    let width = ui.available_width();
    let height = (width * h_mm / w_mm).min(max_h);
    let plate_w = (height * w_mm / h_mm).min(width);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::accesskit::Role::Image, true, t("print-plate-view"))
    });
    let r = Rect::from_center_size(rect.center(), vec2(plate_w, height));
    let p = ui.painter_at(rect);
    p.rect(
        r,
        CornerRadius::same(8),
        c.bg_sunken,
        Stroke::new(1.0, c.border_subtle),
        StrokeKind::Inside,
    );
    let scale = r.width() / w_mm;
    // a line every centimetre
    let grid = Stroke::new(1.0, c.border_subtle.gamma_multiply(0.5));
    let mut x = 10.0;
    while x < w_mm {
        let sx = r.left() + x * scale;
        p.line_segment([pos2(sx, r.top() + 1.0), pos2(sx, r.bottom() - 1.0)], grid);
        x += 10.0;
    }
    let mut y = 10.0;
    while y < h_mm {
        let sy = r.bottom() - y * scale;
        p.line_segment([pos2(r.left() + 1.0, sy), pos2(r.right() - 1.0, sy)], grid);
        y += 10.0;
    }
    let Some(plate) = plate else {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            t("print-plate-empty"),
            fonts::ui(12.0),
            c.text_secondary,
        );
        return;
    };
    let to_screen = |q: &[f32; 2]| pos2(r.left() + q[0] * scale, r.bottom() - q[1] * scale);
    for o in &plate.outlines {
        polygon(&p, &o.supports, &to_screen, support_fill(&c), Stroke::NONE);
    }
    for o in &plate.outlines {
        polygon(
            &p,
            &o.part,
            &to_screen,
            c.select_soft,
            Stroke::new(1.5, c.select_blue),
        );
        name(&p, o, &to_screen, &c);
    }
}

fn support_fill(c: &Palette) -> Color32 {
    c.text_disabled.gamma_multiply(0.45)
}

fn polygon(
    p: &egui::Painter,
    pts: &[[f32; 2]],
    to_screen: &impl Fn(&[f32; 2]) -> Pos2,
    fill: Color32,
    stroke: Stroke,
) {
    if pts.len() >= 3 {
        p.add(Shape::convex_polygon(
            pts.iter().map(to_screen).collect(),
            fill,
            stroke,
        ));
    }
}

/// The part's name at the centre of its outline.
fn name(p: &egui::Painter, o: &Outline, to_screen: &impl Fn(&[f32; 2]) -> Pos2, c: &Palette) {
    if o.part.is_empty() {
        return;
    }
    let n = o.part.len() as f32;
    let centre = o
        .part
        .iter()
        .fold([0.0, 0.0], |a, q| [a[0] + q[0] / n, a[1] + q[1] / n]);
    p.text(
        to_screen(&centre),
        Align2::CENTER_CENTER,
        &o.id,
        fonts::semibold(11.0),
        c.text_primary,
    );
}
