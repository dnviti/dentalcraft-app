//! The Settings dialog, opened by the gear of the top bars (Home and the
//! workspace) and by the account menu: language and theme, the advanced
//! options and parameters of the inspectors, the updates and the anonymous
//! diagnostics. Every change is saved at once.
//!
//! "Advanced options and parameters" is off by default: the inspectors and
//! the scan window then show only the automatic runs and the most useful
//! manual corrections. The app hands the setting to every screen each frame
//! ([`dc_module_api::set_advanced_ui`]); the agent commands keep all their
//! parameters either way (`app.settings.*` reads and changes it).

use dc_i18n::Lang;
use dc_ui::widgets::{self, Button};
use dc_ui::{fonts, icons, theme, tokens};
use egui::{Align, Layout, RichText, Ui};
use serde_json::{Map, Value};

use crate::app::DentalCraftApp;
use crate::config::ThemePref;

/// The themes in the order of the dialog, with their labels (dc-ui keys).
const THEMES: [(ThemePref, &str); 3] = [
    (ThemePref::Dark, "ui-theme-dark"),
    (ThemePref::Light, "ui-theme-light"),
    (ThemePref::System, "ui-theme-system"),
];

/// What the user changed in a frame of the dialog.
enum Change {
    Language(Lang),
    Theme(ThemePref),
    Advanced(bool),
    CheckAtStartup(bool),
    CheckNow,
    Diagnostics(bool),
    Close,
}

impl DentalCraftApp {
    /// Opens the Settings dialog.
    pub fn open_settings(&mut self) {
        self.settings_open = true;
    }

    /// `true` while the Settings dialog is on screen.
    pub fn settings_dialog_open(&self) -> bool {
        self.settings_open
    }

    /// "Advanced options and parameters" is on.
    pub fn advanced(&self) -> bool {
        self.settings.advanced
    }

    /// Shows or hides the advanced options and parameters (saved at once;
    /// the screens follow from this frame on).
    pub fn set_advanced(&mut self, on: bool) {
        self.settings.advanced = on;
        if let Some(ctx) = &self.ctx {
            dc_module_api::set_advanced_ui(ctx, on);
            ctx.request_repaint();
        }
        self.store.save_settings(&self.settings);
        tracing::info!(
            "advanced options and parameters {}",
            if on { "shown" } else { "hidden" }
        );
    }

    /// The output of `app.settings.get` (and `.set`).
    pub(crate) fn settings_summary(&self) -> Value {
        let saved = serde_json::to_value(&self.settings).unwrap_or(Value::Null);
        let empty = Map::new();
        dc_diagnostics::settings::summary(saved.as_object().unwrap_or(&empty))
    }

    /// Draws the dialog when it is open. Esc or a click outside closes it.
    pub(crate) fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let c = *theme::ctx_colors(ctx);
        let width = 460.0_f32.min(ctx.content_rect().width() - 80.0).max(320.0);
        let mut changes = Vec::new();
        let resp = egui::Modal::new(egui::Id::unique("dc-settings")).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icons::GEAR_SIX)
                        .font(fonts::icon(22.0))
                        .color(c.text_secondary),
                );
                ui.label(
                    RichText::new(t!("settings-title"))
                        .font(fonts::style(&tokens().text.title_s))
                        .color(c.text_primary),
                );
            });
            let heading = |ui: &mut Ui, text: String| {
                ui.add_space(4.0);
                widgets::hairline(ui);
                ui.label(
                    RichText::new(text)
                        .font(fonts::semibold(13.0))
                        .color(c.text_secondary),
                );
            };
            // a label on the left, its control on the right
            let row = |ui: &mut Ui, label: String, control: &mut dyn FnMut(&mut Ui)| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(label)
                            .font(fonts::ui(13.0))
                            .color(c.text_primary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| control(ui));
                });
            };

            heading(ui, t!("settings-general"));
            row(ui, t!("language-switch"), &mut |ui| {
                let mut idx = usize::from(dc_i18n::current() == Lang::It);
                if widgets::segmented(ui, &mut idx, &["EN", "IT"], 28.0).changed() {
                    changes.push(Change::Language(if idx == 1 { Lang::It } else { Lang::En }));
                }
            });
            row(ui, t!("settings-theme"), &mut |ui| {
                let mut idx = THEMES
                    .iter()
                    .position(|(p, _)| *p == self.settings.theme)
                    .unwrap_or(0);
                let labels = THEMES.map(|(_, key)| dc_ui::tr(key));
                if widgets::segmented(ui, &mut idx, &labels, 28.0).changed() {
                    changes.push(Change::Theme(THEMES[idx].0));
                }
            });

            heading(ui, t!("settings-workspace"));
            let mut advanced = self.settings.advanced;
            if widgets::toggle(ui, &mut advanced, t!("settings-advanced")).changed() {
                changes.push(Change::Advanced(advanced));
            }
            ui.add(
                egui::Label::new(
                    RichText::new(t!("settings-advanced-desc"))
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                )
                .wrap(),
            );

            heading(ui, t!("settings-updates"));
            ui.horizontal(|ui| {
                let mut at_startup = self.settings.check_updates;
                if widgets::toggle(ui, &mut at_startup, t!("update-setting-startup")).changed() {
                    changes.push(Change::CheckAtStartup(at_startup));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::secondary(t!("settings-check-now"))
                        .icon(icons::ARROWS_CLOCKWISE)
                        .show(ui)
                        .clicked()
                    {
                        changes.push(Change::CheckNow);
                    }
                });
            });

            heading(ui, t!("settings-privacy"));
            let mut diagnostics = self.settings.diagnostics == Some(true);
            if widgets::toggle(ui, &mut diagnostics, t!("diagnostics-setting")).changed() {
                changes.push(Change::Diagnostics(diagnostics));
            }

            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Button::ghost(t!("settings-close"))
                    .icon(icons::X)
                    .show(ui)
                    .clicked()
                {
                    changes.push(Change::Close);
                }
            });
        });
        if resp.should_close() {
            self.settings_open = false;
        }
        for change in changes {
            match change {
                Change::Language(lang) => self.set_language(lang),
                Change::Theme(pref) => self.set_theme(ctx, pref),
                Change::Advanced(on) => self.set_advanced(on),
                Change::CheckAtStartup(on) => self.set_check_updates(on),
                Change::CheckNow => {
                    // the update dialog takes the screen
                    self.settings_open = false;
                    self.check_for_updates();
                }
                Change::Diagnostics(on) => self.set_diagnostics(on),
                Change::Close => self.settings_open = false,
            }
        }
    }
}
