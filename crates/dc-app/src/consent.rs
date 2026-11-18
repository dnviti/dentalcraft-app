//! The consent to anonymous diagnostics (`dc-diagnostics`) in the app: the
//! question at the first start (and at every start while unanswered), the
//! checkbox of the account menu, the line of the support page, and the
//! `app.diagnostics.*` commands of the bridge.
//!
//! Nothing is collected or sent before the user ticks the box. Turning it
//! off deletes the reports not sent yet and the random installation number.

use std::sync::mpsc::{Receiver, channel};

use dc_ui::widgets::{self, Button};
use dc_ui::{fonts, icons, theme, tokens};
use egui::{Align, Layout, RichText, Ui};
use serde_json::{Value, json};

use crate::app::DentalCraftApp;

/// The first-start question.
#[derive(Debug, Default)]
pub(crate) struct ConsentDialog {
    /// On screen.
    pub open: bool,
    /// The box of the dialog (unticked until the user ticks it).
    pub checked: bool,
    /// The details are shown.
    pub details: bool,
}

/// What a report holds and what it never holds (i18n keys).
const SENT_KEYS: [&str; 3] = [
    "diagnostics-sent-problems",
    "diagnostics-sent-system",
    "diagnostics-sent-id",
];
const NEVER_KEYS: [&str; 3] = [
    "diagnostics-never-data",
    "diagnostics-never-account",
    "diagnostics-never-files",
];

impl DentalCraftApp {
    /// At the first frame: the start of this run, the crash files of the
    /// earlier runs, and the thread that sends (each does nothing without
    /// the consent); the question when it was never answered.
    pub(crate) fn start_diagnostics(&mut self) {
        let Some(d) = &self.diagnostics else {
            return;
        };
        d.launched();
        let picked = d.pick_up_crashes();
        if picked > 0 {
            tracing::info!("{picked} crash reports of earlier runs queued");
        }
        d.start_worker();
        self.consent.open = self.settings.diagnostics.is_none();
    }

    /// The consent: `None` until the user answers.
    pub fn diagnostics_enabled(&self) -> Option<bool> {
        self.settings.diagnostics
    }

    /// `true` while the first-start question is on screen.
    pub fn consent_dialog_open(&self) -> bool {
        self.consent.open
    }

    /// Turns anonymous diagnostics on or off (saved at once): on makes a
    /// new random installation number, off deletes it and the reports not
    /// sent yet. Answers the first-start question.
    pub fn set_diagnostics(&mut self, on: bool) {
        let install_id = match &self.diagnostics {
            Some(d) => d.set_enabled(on),
            None if on => self
                .settings
                .install_id
                .clone()
                .or_else(|| Some(uuid::Uuid::new_v4().to_string())),
            None => None,
        };
        self.settings.diagnostics = Some(on);
        self.settings.install_id = install_id;
        self.store.save_settings(&self.settings);
        self.consent.open = false;
        self.support.diagnostics_preview = None;
        tracing::info!(
            "anonymous diagnostics turned {}",
            if on { "on" } else { "off" }
        );
    }

    /// The output of `app.diagnostics.status`.
    pub(crate) fn diagnostics_status(&self) -> Value {
        match &self.diagnostics {
            Some(d) => dc_diagnostics::commands::status_json(d, self.settings.diagnostics),
            None => json!({
                "enabled": self.settings.diagnostics,
                "install_id": self.settings.install_id,
                "pending": 0,
                "endpoint": Value::Null,
            }),
        }
    }

    /// The output of `app.diagnostics.preview`.
    pub(crate) fn diagnostics_preview(&self) -> Value {
        match &self.diagnostics {
            Some(d) => d.preview(),
            None => json!({ "enabled": false, "pending": 0, "report": Value::Null }),
        }
    }

    /// `app.diagnostics.send`: sends the waiting reports now on a worker
    /// thread; the answer comes on the channel.
    pub(crate) fn agent_diagnostics_send(&self) -> Result<Receiver<Result<Value, String>>, String> {
        let Some(d) = self.diagnostics.clone() else {
            return Err("diagnostics are off in this app".into());
        };
        let (tx, rx) = channel();
        let wake = self.wake();
        std::thread::Builder::new()
            .name("dc-diagnostics-send".into())
            .spawn(move || {
                let _ = tx.send(dc_diagnostics::commands::send_result(d.flush()));
                wake();
            })
            .map_err(|e| e.to_string())?;
        Ok(rx)
    }

    /// The text of the support page's "Show what is sent".
    pub(crate) fn diagnostics_preview_text(&self) -> String {
        let p = self.diagnostics_preview();
        if p["report"].is_null() {
            return t!("diagnostics-preview-empty");
        }
        serde_json::to_string_pretty(&p["report"]).unwrap_or_default()
    }

    /// The first-start question, when it is open: what is sent, what never
    /// is, an unticked box and Continue. Esc or a click outside closes it
    /// unanswered: it comes back at the next start.
    pub(crate) fn consent_dialog(&mut self, ctx: &egui::Context) {
        if !self.consent.open {
            return;
        }
        let c = *theme::ctx_colors(ctx);
        let width = 520.0_f32.min(ctx.content_rect().width() - 80.0).max(320.0);
        let mut answer = None;
        let resp = egui::Modal::new(egui::Id::unique("dc-diagnostics-consent")).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icons::SHIELD_CHECK)
                        .font(fonts::icon(22.0))
                        .color(c.select_blue),
                );
                ui.label(
                    RichText::new(t!("diagnostics-consent-title"))
                        .font(fonts::style(&tokens().text.title_s))
                        .color(c.text_primary),
                );
            });
            let body = |ui: &mut Ui, text: String| {
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(fonts::ui(13.0))
                            .color(c.text_secondary),
                    )
                    .wrap(),
                );
            };
            body(ui, t!("diagnostics-consent-body"));
            let list = |ui: &mut Ui, title: String, icon: &str, color, keys: &[&str]| {
                ui.label(
                    RichText::new(title)
                        .font(fonts::semibold(13.0))
                        .color(c.text_primary),
                );
                for key in keys {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(icon).font(fonts::icon(14.0)).color(color));
                        ui.add(
                            egui::Label::new(
                                RichText::new(dc_i18n::tr(key))
                                    .font(fonts::ui(13.0))
                                    .color(c.text_secondary),
                            )
                            .wrap(),
                        );
                    });
                }
            };
            list(
                ui,
                t!("diagnostics-sent-title"),
                icons::CHECK,
                c.status_success,
                &SENT_KEYS,
            );
            list(
                ui,
                t!("diagnostics-never-title"),
                icons::PROHIBIT,
                c.status_danger,
                &NEVER_KEYS,
            );
            if widgets::link(ui, t!("diagnostics-details-link")).clicked() {
                self.consent.details = !self.consent.details;
            }
            if self.consent.details {
                body(ui, t!("diagnostics-details-body"));
            }
            ui.add_space(4.0);
            widgets::checkbox(
                ui,
                &mut self.consent.checked,
                t!("diagnostics-consent-check"),
            );
            body(ui, t!("diagnostics-consent-later"));
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Button::primary(t!("diagnostics-consent-continue"))
                    .icon(icons::ARROW_RIGHT)
                    .show(ui)
                    .clicked()
                {
                    answer = Some(self.consent.checked);
                }
            });
        });
        match answer {
            Some(on) => self.set_diagnostics(on),
            None if resp.should_close() => self.consent.open = false,
            None => {}
        }
    }
}

/// The i18n keys of the dialog that are looked up at run time.
pub(crate) fn runtime_keys() -> impl Iterator<Item = &'static str> {
    SENT_KEYS.into_iter().chain(NEVER_KEYS)
}
