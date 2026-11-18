//! `03 Support`: documentation links, ticket form, tickets list and the
//! workstation panel (system info, diagnostics export, check for updates).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{Palette, fonts, icons, tokens};
use egui::{Align, CornerRadius, Key, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};

use crate::app::{DentalCraftApp, Screen};
use crate::dialogs::{Purpose, Spec};
use crate::paint;
use crate::support_client::{self, Ticket, TicketOutcome};

/// A documentation link.
#[derive(Debug, Clone, Copy)]
pub struct DocLink {
    /// Icon.
    pub icon: &'static str,
    /// Title key.
    pub title_key: &'static str,
    /// Description key.
    pub desc_key: &'static str,
    /// URL.
    pub url: &'static str,
}

// TODO(release): point to the published manual.
/// Documentation entries.
pub const DOC_LINKS: [DocLink; 7] = [
    DocLink {
        icon: icons::BOOK_OPEN,
        title_key: "doc-getting-started",
        desc_key: "doc-getting-started-desc",
        url: "https://docs.dentalcraft.app/getting-started",
    },
    DocLink {
        icon: icons::TOOTH,
        title_key: "doc-aligners-guide",
        desc_key: "doc-aligners-guide-desc",
        url: "https://docs.dentalcraft.app/aligners",
    },
    DocLink {
        icon: icons::SLIDERS_HORIZONTAL,
        title_key: "doc-staging",
        desc_key: "doc-staging-desc",
        url: "https://docs.dentalcraft.app/aligners/staging",
    },
    DocLink {
        icon: icons::PRINTER,
        title_key: "doc-printing",
        desc_key: "doc-printing-desc",
        url: "https://docs.dentalcraft.app/printing",
    },
    DocLink {
        icon: icons::KEYBOARD,
        title_key: "doc-shortcuts",
        desc_key: "doc-shortcuts-desc",
        url: "https://docs.dentalcraft.app/shortcuts",
    },
    DocLink {
        icon: icons::NEWSPAPER,
        title_key: "doc-release-notes",
        desc_key: "doc-release-notes-desc",
        url: "https://docs.dentalcraft.app/releases",
    },
    DocLink {
        icon: icons::PLAY_CIRCLE,
        title_key: "doc-videos",
        desc_key: "doc-videos-desc",
        url: "https://docs.dentalcraft.app/videos",
    },
];

/// Ticket categories: id and i18n key.
pub const CATEGORIES: [(&str, &str); 6] = [
    ("aligners-staging", "category-aligners-staging"),
    ("aligners-segmentation", "category-aligners-segmentation"),
    ("aligners-export", "category-aligners-export"),
    ("licence-account", "category-licence"),
    ("installation", "category-installation"),
    ("other", "category-other"),
];

/// Support screen state.
#[derive(Default)]
pub struct SupportState {
    /// Manual search text.
    pub search: String,
    /// Subject.
    pub subject: String,
    /// Index into [`CATEGORIES`].
    pub category: usize,
    /// Description.
    pub description: String,
    /// Attach the diagnostics text.
    pub attach_diagnostics: Option<bool>,
    /// Show the diagnostics preview.
    pub preview: bool,
    /// Files the user wants to attach.
    pub attachments: Vec<PathBuf>,
    /// Submission in progress.
    pub(crate) sending: Option<Receiver<TicketOutcome>>,
    /// Last outcome.
    pub outcome: Option<TicketOutcome>,
    /// Show validation errors.
    pub show_errors: bool,
    /// "Show what is sent": the next anonymous diagnostics report, as it
    /// was when the user opened it.
    pub(crate) diagnostics_preview: Option<String>,
}

impl SupportState {
    fn attach(&self) -> bool {
        self.attach_diagnostics.unwrap_or(true)
    }
}

fn text(ui: &mut Ui, s: impl Into<String>, font: egui::FontId, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).wrap());
}

fn card(ui: &mut Ui, c: &Palette, width: f32, height: f32, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(c.bg_panel)
        .stroke(Stroke::new(1.0, c.border_subtle))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width((width - 40.0).max(80.0));
                ui.set_min_height(height);
                add(ui);
            });
        });
}

impl DentalCraftApp {
    pub(crate) fn poll_support(&mut self) {
        if let Some(rx) = &self.support.sending {
            match rx.try_recv() {
                Ok(o) => {
                    match &o {
                        TicketOutcome::Sent(_) => {
                            self.support.subject.clear();
                            self.support.description.clear();
                            self.support.attachments.clear();
                            self.support.show_errors = false;
                        }
                        TicketOutcome::Failed(e) => tracing::warn!("ticket not sent: {e}"),
                        TicketOutcome::NotAvailable => {}
                    }
                    self.support.outcome = Some(o);
                    self.support.sending = None;
                }
                Err(TryRecvError::Disconnected) => self.support.sending = None,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    fn ticket(&self) -> Ticket {
        let s = &self.support;
        Ticket {
            subject: s.subject.trim().to_owned(),
            category: CATEGORIES[s.category.min(CATEGORIES.len() - 1)]
                .0
                .to_owned(),
            description: s.description.trim().to_owned(),
            diagnostics: s.attach().then(|| self.sysinfo.to_text()),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn send_ticket(&mut self) {
        self.support.show_errors = true;
        if self.support.subject.trim().is_empty() || self.support.description.trim().is_empty() {
            return;
        }
        let ticket = self.ticket();
        let issuer = self.settings.issuer();
        let auth = self.auth.clone().filter(|_| self.user().is_some());
        let (tx, rx) = channel();
        let wake = self.wake();
        let spawned = std::thread::Builder::new()
            .name("dc-support-ticket".into())
            .spawn(move || {
                let token = auth.and_then(|a| a.ensure_access_token().ok());
                let _ = tx.send(support_client::submit(&issuer, token.as_deref(), &ticket));
                wake();
            });
        if spawned.is_ok() {
            self.support.outcome = None;
            self.support.sending = Some(rx);
        }
    }

    fn export_diagnostics(&mut self) {
        let name = format!(
            "dentalcraft-diagnostics-{}.zip",
            chrono::Local::now().format("%Y%m%d-%H%M")
        );
        self.open_dialog(
            Purpose::ExportDiagnostics,
            Spec {
                title: t!("support-export-diagnostics"),
                filter: Some(("Zip".into(), vec!["zip"])),
                file_name: Some(name),
                save: true,
                ..Spec::default()
            },
        );
    }

    pub(crate) fn support_ui(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        let tk = tokens();
        let search_id = egui::Id::unique("dc-support-search");
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
            ui.memory_mut(|m| m.request_focus(search_id));
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin {
                        left: 48,
                        right: 48,
                        top: 32,
                        bottom: 32,
                    })
                    .show(ui, |ui| {
                        // header
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 8.0;
                                if Button::ghost(t!("nav-home"))
                                    .icon(icons::CARET_LEFT)
                                    .size(widgets::ButtonSize::Compact)
                                    .show(ui)
                                    .clicked()
                                {
                                    self.screen = Screen::Home;
                                }
                                text(
                                    ui,
                                    t!("support-title"),
                                    fonts::style(&tk.text.display_l),
                                    c.text_primary,
                                );
                                text(
                                    ui,
                                    t!("support-sub"),
                                    fonts::style(&tk.text.body_l),
                                    c.text_secondary,
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                                let w = 460.0_f32.min(ui.available_width());
                                egui::Frame::new()
                                    .fill(c.bg_sunken)
                                    .stroke(Stroke::new(1.0, c.border_strong))
                                    .corner_radius(CornerRadius::same(4))
                                    .inner_margin(Margin::symmetric(14, 0))
                                    .show(ui, |ui| {
                                        ui.set_width(w - 28.0);
                                        ui.set_height(44.0);
                                        ui.horizontal_centered(|ui| {
                                            ui.label(
                                                RichText::new(icons::MAGNIFYING_GLASS)
                                                    .font(fonts::icon(16.0))
                                                    .color(c.text_secondary),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new("Ctrl K")
                                                            .font(fonts::mono(11.0))
                                                            .color(c.text_secondary),
                                                    );
                                                    ui.add(
                                                        egui::TextEdit::singleline(
                                                            &mut self.support.search,
                                                        )
                                                        .id(search_id)
                                                        .frame(egui::Frame::NONE)
                                                        .hint_text(t!("support-search-hint"))
                                                        .font(fonts::ui(13.0))
                                                        .desired_width(ui.available_width()),
                                                    );
                                                },
                                            );
                                        });
                                    });
                            });
                        });
                        ui.add_space(28.0);
                        let full = ui.available_width();
                        let gap = 24.0;
                        let docs_w = 440.0_f32.min(full * 0.26);
                        let side_w = 460.0_f32.min(full * 0.27);
                        let form_w = (full - docs_w - side_w - 2.0 * gap).max(420.0);
                        let col_h = (ui.available_height() - 8.0).max(720.0);
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            card(ui, &c, docs_w, col_h, |ui| self.docs_column(ui, &c));
                            card(ui, &c, form_w, col_h, |ui| self.ticket_form(ui, &c));
                            ui.vertical(|ui| {
                                ui.set_width(side_w);
                                card(ui, &c, side_w, 120.0, |ui| self.tickets_list(ui, &c));
                                ui.add_space(gap);
                                card(
                                    ui,
                                    &c,
                                    side_w,
                                    (col_h - 120.0 - 40.0 - gap - 20.0).max(200.0),
                                    |ui| self.workstation(ui, &c),
                                );
                            });
                        });
                    });
            });
    }

    fn docs_column(&mut self, ui: &mut Ui, c: &Palette) {
        text(
            ui,
            t!("support-docs-title"),
            fonts::style(&tokens().text.title_s),
            c.text_primary,
        );
        text(
            ui,
            t!("support-docs-sub"),
            fonts::ui(12.0),
            c.text_secondary,
        );
        ui.add_space(12.0);
        let q = self.support.search.trim().to_lowercase();
        let mut shown = 0;
        for d in DOC_LINKS {
            let title = dc_i18n::tr(d.title_key);
            let desc = dc_i18n::tr(d.desc_key);
            if !q.is_empty()
                && !title.to_lowercase().contains(&q)
                && !desc.to_lowercase().contains(&q)
            {
                continue;
            }
            shown += 1;
            let w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 62.0), Sense::click());
            let label = title.clone();
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, &label));
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rect, CornerRadius::same(4), c.bg_hover);
            }
            p.text(
                rect.left_center() + vec2(22.0, 0.0),
                egui::Align2::CENTER_CENTER,
                d.icon,
                fonts::icon(20.0),
                c.text_secondary,
            );
            p.text(
                rect.left_top() + vec2(44.0, 12.0),
                egui::Align2::LEFT_TOP,
                title,
                fonts::semibold(13.0),
                c.text_primary,
            );
            p.text(
                rect.left_top() + vec2(44.0, 32.0),
                egui::Align2::LEFT_TOP,
                desc,
                fonts::ui(12.0),
                c.text_secondary,
            );
            p.text(
                rect.right_center() - vec2(16.0, 0.0),
                egui::Align2::CENTER_CENTER,
                icons::ARROW_SQUARE_OUT,
                fonts::icon(14.0),
                c.text_secondary,
            );
            widgets::focus_ring(ui, &resp, rect, CornerRadius::same(4));
            if resp.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(d.url));
            }
            ui.add_space(4.0);
        }
        if shown == 0 {
            text(
                ui,
                t!("support-search-empty"),
                fonts::ui(13.0),
                c.text_secondary,
            );
        }
    }

    fn ticket_form(&mut self, ui: &mut Ui, c: &Palette) {
        text(
            ui,
            t!("support-ticket-title"),
            fonts::style(&tokens().text.title_s),
            c.text_primary,
        );
        text(
            ui,
            t!("support-ticket-sub"),
            fonts::ui(12.0),
            c.text_secondary,
        );
        ui.add_space(16.0);
        let show_err = self.support.show_errors;
        ui.horizontal_top(|ui| {
            let w = ui.available_width();
            ui.vertical(|ui| {
                ui.set_width((w - 260.0 - 16.0).max(160.0));
                let err = (show_err && self.support.subject.trim().is_empty())
                    .then(|| dc_ui::tr("ui-required"));
                widgets::text_input(
                    ui,
                    &t!("support-subject"),
                    &mut self.support.subject,
                    &t!("support-subject-hint"),
                    Some(&t!("support-subject-help")),
                    err.as_deref(),
                );
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(260.0);
                ui.label(
                    RichText::new(t!("support-category"))
                        .font(fonts::medium(12.0))
                        .color(c.text_secondary),
                );
                let labels: Vec<String> = CATEGORIES.iter().map(|(_, k)| dc_i18n::tr(k)).collect();
                egui::ComboBox::from_id_salt("dc-ticket-category")
                    .width(260.0)
                    .height(300.0)
                    .selected_text(labels[self.support.category].clone())
                    .show_ui(ui, |ui| {
                        for (i, l) in labels.iter().enumerate() {
                            ui.selectable_value(&mut self.support.category, i, l);
                        }
                    });
                ui.label(
                    RichText::new(t!("support-category-help"))
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                );
            });
        });
        ui.add_space(12.0);
        widgets::text_area(
            ui,
            &t!("support-description"),
            &mut self.support.description,
            &t!("support-description-hint"),
            150.0,
        );
        if show_err && self.support.description.trim().is_empty() {
            ui.label(
                RichText::new(format!(
                    "{}  {}",
                    icons::WARNING_CIRCLE,
                    dc_ui::tr("ui-required")
                ))
                .font(fonts::ui(12.0))
                .color(c.status_danger),
            );
        } else {
            ui.label(
                RichText::new(t!("support-description-help"))
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        }
        ui.add_space(12.0);
        // diagnostics box
        let mut attach = self.support.attach();
        let checked = attach;
        egui::Frame::new()
            .fill(if checked { c.select_soft } else { c.bg_sunken })
            .stroke(Stroke::new(
                1.0,
                if checked {
                    c.select_blue
                } else {
                    c.border_subtle
                },
            ))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let size = format!("{:.1} KB", self.sysinfo.to_text().len() as f32 / 1024.0);
                if widgets::checkbox(
                    ui,
                    &mut attach,
                    t!("support-attach-diagnostics", size = size),
                )
                .changed()
                {
                    self.support.attach_diagnostics = Some(attach);
                }
                ui.indent("diag", |ui| {
                    text(
                        ui,
                        t!("support-attach-diagnostics-body"),
                        fonts::ui(12.0),
                        c.text_secondary,
                    );
                    if widgets::link(ui, t!("support-preview")).clicked() {
                        self.support.preview = !self.support.preview;
                    }
                    if self.support.preview {
                        egui::Frame::new()
                            .fill(c.bg_sunken)
                            .inner_margin(Margin::same(8))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(self.sysinfo.to_text()).font(fonts::mono(11.0)),
                                );
                            });
                    }
                });
            });
        ui.add_space(12.0);
        // drop zone
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click());
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let stroke = Stroke::new(
            1.0,
            if hovering {
                c.select_blue
            } else {
                c.border_strong
            },
        );
        paint::dashed_rect(ui.painter(), rect, [4.0; 4], stroke);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}  {}", icons::PAPERCLIP, t!("support-drop")),
            fonts::ui(13.0),
            c.text_secondary,
        );
        let label = t!("support-drop");
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        if resp.clicked() {
            self.open_dialog(
                Purpose::TicketAttachments,
                Spec {
                    title: t!("support-drop"),
                    multiple: true,
                    ..Spec::default()
                },
            );
        }
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.support.attachments.extend(dropped);
        }
        let mut remove = None;
        for (i, a) in self.support.attachments.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icons::PAPERCLIP)
                        .font(fonts::icon(14.0))
                        .color(c.text_secondary),
                );
                ui.label(
                    RichText::new(
                        a.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                    .font(fonts::ui(12.0)),
                );
                if widgets::icon_button_sized(ui, icons::X, t!("remove"), 22.0, true).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.support.attachments.remove(i);
        }
        if !self.support.attachments.is_empty() {
            text(
                ui,
                t!("support-attachments-later"),
                fonts::ui(11.0),
                c.text_secondary,
            );
        }
        ui.add_space(16.0);
        // outcome
        if let Some(o) = self.support.outcome.clone() {
            self.ticket_outcome(ui, c, &o);
            ui.add_space(12.0);
        }
        ui.horizontal(|ui| {
            text(
                ui,
                t!("support-reply-note"),
                fonts::ui(12.0),
                c.text_secondary,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sending = self.support.sending.is_some();
                if Button::primary(if sending {
                    t!("support-sending")
                } else {
                    t!("support-send")
                })
                .icon(icons::UPLOAD_SIMPLE)
                .large()
                .enabled(!sending)
                .show(ui)
                .clicked()
                {
                    self.send_ticket();
                }
                if sending {
                    widgets::spinner(ui, 18.0);
                }
            });
        });
    }

    fn ticket_outcome(&mut self, ui: &mut Ui, c: &Palette, o: &TicketOutcome) {
        let (tone, icon, title, body) = match o {
            TicketOutcome::Sent(id) => (
                c.status_success,
                icons::CHECK_CIRCLE,
                t!("ticket-sent-title"),
                t!(
                    "ticket-sent-body",
                    id = id.clone().unwrap_or_else(|| "-".into())
                ),
            ),
            TicketOutcome::NotAvailable => (
                c.status_warning,
                icons::WARNING,
                t!("ticket-unavailable-title"),
                t!(
                    "ticket-unavailable-body",
                    email = support_client::SUPPORT_EMAIL
                ),
            ),
            TicketOutcome::Failed(_) => (
                c.status_danger,
                icons::X_CIRCLE,
                t!("ticket-failed-title"),
                t!("ticket-failed-body", email = support_client::SUPPORT_EMAIL),
            ),
        };
        egui::Frame::new()
            .fill(c.bg_raised)
            .stroke(Stroke::new(1.0, c.border_subtle))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        r.left_top() - vec2(14.0, 14.0),
                        r.left_bottom() - vec2(14.0, -14.0),
                    ],
                    Stroke::new(3.0, tone),
                );
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(icon).font(fonts::icon(20.0)).color(tone));
                    ui.vertical(|ui| {
                        text(ui, title, fonts::semibold(13.0), c.text_primary);
                        text(ui, body, fonts::ui(12.0), c.text_secondary);
                        if !matches!(o, TicketOutcome::Sent(_)) {
                            ui.horizontal(|ui| {
                                if Button::secondary(t!("ticket-copy-diagnostics"))
                                    .icon(icons::COPY)
                                    .show(ui)
                                    .clicked()
                                {
                                    let t = self.ticket();
                                    ui.ctx().copy_text(format!(
                                        "{}\n\n{}\n\n{}",
                                        t.subject,
                                        t.description,
                                        self.sysinfo.to_text()
                                    ));
                                    self.toast(BadgeKind::Success, t!("copied"));
                                }
                                if Button::ghost(t!("ticket-email-us"))
                                    .icon(icons::ENVELOPE_SIMPLE)
                                    .show(ui)
                                    .clicked()
                                {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(
                                        support_client::mailto(&self.ticket()),
                                    ));
                                }
                            });
                        }
                    });
                });
            });
    }

    fn tickets_list(&mut self, ui: &mut Ui, c: &Palette) {
        text(
            ui,
            t!("support-tickets-title"),
            fonts::style(&tokens().text.title_s),
            c.text_primary,
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(icons::TRAY)
                    .font(fonts::icon(20.0))
                    .color(c.text_secondary),
            );
            ui.vertical(|ui| {
                text(
                    ui,
                    t!("support-tickets-empty"),
                    fonts::semibold(13.0),
                    c.text_primary,
                );
                text(
                    ui,
                    t!("support-tickets-empty-body"),
                    fonts::ui(12.0),
                    c.text_secondary,
                );
            });
        });
    }

    fn workstation(&mut self, ui: &mut Ui, c: &Palette) {
        text(
            ui,
            t!("support-workstation-title"),
            fonts::style(&tokens().text.title_s),
            c.text_primary,
        );
        ui.add_space(12.0);
        egui::Grid::new("dc-sysinfo")
            .num_columns(2)
            .spacing(vec2(24.0, 8.0))
            .show(ui, |ui| {
                for (k, v) in self.sysinfo.rows() {
                    ui.label(
                        RichText::new(dc_i18n::tr(k))
                            .font(fonts::ui(12.0))
                            .color(c.text_secondary),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(v)
                                .font(fonts::mono(12.0))
                                .color(c.text_primary),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(t!("support-copy-system-info"))
                .icon(icons::FILE_TEXT)
                .show(ui)
                .clicked()
            {
                ui.ctx().copy_text(self.sysinfo.to_text());
                self.toast(BadgeKind::Success, t!("copied"));
            }
            if Button::ghost(t!("support-open-log-folder"))
                .icon(icons::FOLDER_OPEN)
                .show(ui)
                .clicked()
            {
                let dir = self.store.dirs().logs();
                let _ = std::fs::create_dir_all(&dir);
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab(format!("file://{}", dir.display())));
            }
        });
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(t!("support-export-diagnostics"))
                .icon(icons::EXPORT)
                .show(ui)
                .clicked()
            {
                self.export_diagnostics();
            }
            if Button::ghost(t!("support-shortcuts"))
                .icon(icons::KEYBOARD)
                .show(ui)
                .clicked()
            {
                self.show_shortcuts = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(t!("update-support-check"))
                .icon(icons::ARROWS_CLOCKWISE)
                .show(ui)
                .clicked()
            {
                self.check_for_updates();
            }
        });
        ui.add_space(12.0);
        let state = match self.settings.diagnostics {
            Some(true) => t!("support-diagnostics-on"),
            Some(false) => t!("support-diagnostics-off"),
            None => t!("support-diagnostics-unset"),
        };
        text(ui, state, fonts::ui(12.0), c.text_secondary);
        if widgets::link(ui, t!("support-diagnostics-show")).clicked() {
            self.support.diagnostics_preview = match self.support.diagnostics_preview.take() {
                Some(_) => None,
                None => Some(self.diagnostics_preview_text()),
            };
        }
        if let Some(preview) = &self.support.diagnostics_preview {
            egui::ScrollArea::vertical()
                .id_salt("dc-diagnostics-preview")
                .max_height(180.0)
                .show(ui, |ui| {
                    egui::Frame::new()
                        .fill(c.bg_sunken)
                        .inner_margin(Margin::same(8))
                        .show(ui, |ui| {
                            ui.label(RichText::new(preview.as_str()).font(fonts::mono(11.0)));
                        });
                });
        }
    }
}
