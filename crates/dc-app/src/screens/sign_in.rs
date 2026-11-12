//! `02 Sign in`, `02b waiting for browser`, `02c signed in`: story panel
//! with the shade guide on the left, the sign-in card on the right. The
//! browser flow runs in `dc-auth-client` worker threads; this screen only
//! polls its state.

use dc_auth_client::{AuthState, SIGN_IN_TIMEOUT};
use dc_ui::widgets::{self, BadgeKind, Button, ButtonSize};
use dc_ui::{Palette, fonts, icons, theme, tokens};
use egui::{Align, CornerRadius, Layout, Margin, Panel, Rect, RichText, Stroke, Ui, vec2};

use crate::app::{DentalCraftApp, Screen};
use crate::paint;

/// Sign-in screen state.
#[derive(Debug, Clone, Default)]
pub struct SignInState {
    /// egui time when the browser flow started (for the expiry countdown).
    pub started_at: Option<f64>,
    /// Local clock time when the browser was opened.
    pub opened_clock: Option<String>,
}

fn text(ui: &mut Ui, s: impl Into<String>, font: egui::FontId, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).wrap());
}

impl DentalCraftApp {
    pub(crate) fn sign_in_escape(&mut self) {
        match &self.auth_state {
            AuthState::WaitingForBrowser { .. } | AuthState::Working => {
                if let Some(a) = &self.auth {
                    a.cancel_sign_in();
                    self.auth_state = a.state();
                }
            }
            _ => self.screen = Screen::Home,
        }
    }

    pub(crate) fn sign_in_ui(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        let width = ui.available_width();
        Panel::left("dc-signin-story")
            .exact_size((width * 0.427).clamp(420.0, 820.0))
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(c.bg_sunken)
                    .inner_margin(Margin::symmetric(72, 56)),
            )
            .show(ui, |ui| story(ui, &c));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c.bg_canvas)
                    .inner_margin(Margin::same(24)),
            )
            .show(ui, |ui| {
                let full = ui.max_rect();
                // top right: language + support + back to Home
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if Button::ghost(t!("nav-support"))
                            .icon(icons::LIFEBUOY)
                            .show(ui)
                            .clicked()
                        {
                            self.screen = Screen::Support;
                        }
                        self.language_switch(ui);
                        if Button::ghost(t!("nav-home"))
                            .icon(icons::CARET_LEFT)
                            .show(ui)
                            .clicked()
                        {
                            self.screen = Screen::Home;
                        }
                    });
                });
                // card, centred
                let card_w = 480.0_f32.min(full.width() - 48.0);
                let card_rect = Rect::from_center_size(full.center(), vec2(card_w, 10.0));
                let mut card_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_size(
                            egui::pos2(
                                card_rect.left(),
                                full.top() + (full.height() * 0.24).max(60.0),
                            ),
                            vec2(card_w, full.height()),
                        ))
                        .layout(Layout::top_down(Align::Min)),
                );
                egui::Frame::new()
                    .fill(c.bg_panel)
                    .stroke(Stroke::new(1.0, c.border_subtle))
                    .corner_radius(CornerRadius::same(12))
                    .shadow(theme::shadow(&tokens().shadow.e2_dialog))
                    .inner_margin(Margin::same(36))
                    .show(&mut card_ui, |ui| {
                        ui.set_width(card_w - 72.0);
                        ui.spacing_mut().item_spacing.y = 12.0;
                        self.sign_in_card(ui, &c);
                    });
                // footer links
                let footer = Rect::from_min_size(
                    egui::pos2(full.left(), full.bottom() - 24.0),
                    vec2(full.width(), 20.0),
                );
                let mut fui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                fui.horizontal(|ui| {
                    ui.add_space(((footer.width() - 400.0) / 2.0).max(0.0));
                    let note = |ui: &mut Ui, k: String| {
                        ui.label(
                            RichText::new(k)
                                .font(fonts::ui(11.0))
                                .color(c.text_secondary),
                        );
                        ui.add_space(8.0);
                    };
                    note(ui, t!("footer-privacy"));
                    if widgets::link(ui, t!("footer-terms")).clicked() {
                        self.open_about();
                    }
                    ui.add_space(8.0);
                    note(ui, t!("footer-gdpr"));
                });
            });
    }

    fn sign_in_card(&mut self, ui: &mut Ui, c: &Palette) {
        let t = tokens();
        let state = self.auth_state.clone();
        if self.auth.is_none() {
            hero_icon(ui, icons::WARNING_CIRCLE, c.status_danger);
            text(
                ui,
                t!("signin-error-title"),
                fonts::style(&t.text.display_l),
                c.text_primary,
            );
            text(
                ui,
                dc_i18n::tr("auth-error-invalid-issuer"),
                fonts::style(&t.text.body_l),
                c.text_secondary,
            );
            return;
        }
        match state {
            AuthState::SignedOut => self.card_signed_out(ui, c),
            AuthState::Working => {
                widgets::spinner(ui, 32.0);
                text(
                    ui,
                    t!("signin-working-title"),
                    fonts::style(&t.text.display_l),
                    c.text_primary,
                );
                text(
                    ui,
                    t!("signin-working-body"),
                    fonts::style(&t.text.body_l),
                    c.text_secondary,
                );
                widgets::progress_bar(ui, None, ui.available_width());
                if Button::ghost(t!("cancel"))
                    .icon(icons::X)
                    .show(ui)
                    .clicked()
                {
                    self.sign_in_escape();
                }
            }
            AuthState::WaitingForBrowser { url } => self.card_waiting(ui, c, &url),
            AuthState::SignedIn(u) => {
                hero_icon(ui, icons::CHECK_CIRCLE, c.status_success);
                let first = u.name.split_whitespace().next().unwrap_or("").to_owned();
                let title = if first.is_empty() {
                    t!("signin-done-title-anon")
                } else {
                    t!("signin-done-title", name = first)
                };
                text(ui, title, fonts::style(&t.text.display_l), c.text_primary);
                text(
                    ui,
                    t!("signin-done-body"),
                    fonts::style(&t.text.body_l),
                    c.text_secondary,
                );
                let host = hostname();
                egui::Frame::new()
                    .fill(c.bg_sunken)
                    .corner_radius(CornerRadius::same(4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let rows: Vec<(String, FactValue)> = vec![
                            (t!("signin-fact-account"), {
                                let e = u.email.clone();
                                Box::new(move |ui: &mut Ui| {
                                    ui.label(RichText::new(e.clone()).font(fonts::medium(13.0)));
                                })
                            }),
                            (t!("signin-fact-workstation"), {
                                let h = host.clone();
                                Box::new(move |ui: &mut Ui| {
                                    ui.label(RichText::new(h.clone()).font(fonts::medium(13.0)));
                                })
                            }),
                            (dc_i18n::tr(dca_module::NAME_KEY), {
                                let ok = u.entitlements.has_module("aligners");
                                Box::new(move |ui: &mut Ui| {
                                    if ok {
                                        widgets::badge(
                                            ui,
                                            BadgeKind::Success,
                                            t!("account-licence-active"),
                                        );
                                    } else {
                                        widgets::badge(
                                            ui,
                                            BadgeKind::Warning,
                                            t!("account-licence-missing"),
                                        );
                                    }
                                })
                            }),
                            (
                                t!("signin-fact-other-modules"),
                                Box::new(|ui: &mut Ui| {
                                    widgets::badge_icon(
                                        ui,
                                        BadgeKind::Neutral,
                                        Some(icons::HOURGLASS_MEDIUM),
                                        t!(
                                            "signin-fact-coming-soon",
                                            count = crate::modules::COMING_SOON.len().to_string()
                                        ),
                                    );
                                }),
                            ),
                        ];
                        for (i, (k, v)) in rows.into_iter().enumerate() {
                            if i > 0 {
                                widgets::hairline(ui);
                            }
                            egui::Frame::new()
                                .inner_margin(Margin::symmetric(14, 10))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(k)
                                                .font(fonts::ui(12.0))
                                                .color(c.text_secondary),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| v(ui),
                                        );
                                    });
                                });
                        }
                    });
                if Button::primary(t!("signin-continue"))
                    .icon(icons::HOUSE)
                    .size(ButtonSize::Cta)
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    self.screen = Screen::Home;
                }
                ui.vertical_centered(|ui| {
                    if Button::ghost(t!("signin-switch-account"))
                        .icon(icons::USER_SWITCH)
                        .show(ui)
                        .clicked()
                    {
                        self.sign_out();
                        self.begin_sign_in();
                    }
                });
            }
            AuthState::Error(e) => {
                hero_icon(ui, icons::WARNING_CIRCLE, c.status_danger);
                text(
                    ui,
                    t!("signin-error-title"),
                    fonts::style(&t.text.display_l),
                    c.text_primary,
                );
                text(
                    ui,
                    dc_i18n::tr(e.code.i18n_key()),
                    fonts::style(&t.text.body_l),
                    c.text_secondary,
                );
                if let Some(a) = &self.auth {
                    text(
                        ui,
                        t!("signin-error-server", issuer = a.issuer()),
                        fonts::ui(12.0),
                        c.text_secondary,
                    );
                }
                if Button::primary(t!("signin-try-again"))
                    .icon(icons::ARROWS_CLOCKWISE)
                    .size(ButtonSize::Cta)
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    self.begin_sign_in();
                }
                if Button::ghost(t!("nav-home"))
                    .icon(icons::CARET_LEFT)
                    .show(ui)
                    .clicked()
                {
                    self.screen = Screen::Home;
                }
            }
        }
    }

    fn card_signed_out(&mut self, ui: &mut Ui, c: &Palette) {
        let t = tokens();
        text(
            ui,
            t!("signin-title"),
            fonts::style(&t.text.display_l),
            c.text_primary,
        );
        text(
            ui,
            t!("signin-body"),
            fonts::style(&t.text.body_l),
            c.text_secondary,
        );
        let resp = Button::primary(t!("signin-with-account"))
            .icon(icons::ARROW_SQUARE_OUT)
            .size(ButtonSize::Cta)
            .full_width(true)
            .show(ui);
        // keyboard default: focused when nothing else is
        if ui.memory(|m| m.focused().is_none()) {
            resp.request_focus();
        }
        if resp.clicked() {
            self.begin_sign_in();
        }
        let host = self
            .auth
            .as_ref()
            .and_then(|a| url_host(a.issuer()))
            .unwrap_or_default();
        text(
            ui,
            t!("signin-hint", host = host),
            fonts::ui(12.0),
            c.text_secondary,
        );
        // divider "no account yet"
        ui.horizontal(|ui| {
            let label = t!("signin-no-account");
            let g = ui
                .painter()
                .layout_no_wrap(label, fonts::ui(12.0), c.text_secondary);
            let (r, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 20.0), egui::Sense::hover());
            let y = r.center().y;
            let half = g.size().x / 2.0 + 12.0;
            ui.painter().line_segment(
                [egui::pos2(r.left(), y), egui::pos2(r.center().x - half, y)],
                Stroke::new(1.0, c.border_subtle),
            );
            ui.painter().line_segment(
                [egui::pos2(r.center().x + half, y), egui::pos2(r.right(), y)],
                Stroke::new(1.0, c.border_subtle),
            );
            ui.painter()
                .galley(r.center() - g.size() / 2.0, g, c.text_secondary);
        });
        ui.horizontal(|ui| {
            if Button::secondary(t!("signin-evaluation"))
                .icon(icons::HOURGLASS_MEDIUM)
                .large()
                .min_width(240.0)
                .show(ui)
                .clicked()
            {
                // TODO(server): evaluation licences; for now local work without an account.
                self.toast(BadgeKind::Info, t!("signin-evaluation-local"));
                self.screen = Screen::Home;
            }
            if Button::ghost(t!("signin-create-account"))
                .icon(icons::USER)
                .large()
                .show(ui)
                .clicked()
                && let Some(a) = &self.auth
            {
                // The server's sign-up page: an administrator approves the request.
                let signup = format!("{}/signup", a.issuer().trim_end_matches('/'));
                ui.ctx().open_url(egui::OpenUrl::new_tab(signup));
            }
        });
        crate::screens::home::offline_note(ui, c);
    }

    fn card_waiting(&mut self, ui: &mut Ui, c: &Palette, url: &str) {
        let t = tokens();
        widgets::spinner(ui, 32.0);
        text(
            ui,
            t!("signin-waiting-title"),
            fonts::style(&t.text.display_l),
            c.text_primary,
        );
        let host = url_host(url).unwrap_or_default();
        text(
            ui,
            t!("signin-waiting-body", host = host, workstation = hostname()),
            fonts::style(&t.text.body_l),
            c.text_secondary,
        );
        widgets::progress_bar(ui, None, ui.available_width());
        let opened = self.sign_in.opened_clock.clone().unwrap_or_default();
        for (state, label) in [
            (2, t!("signin-step-opened", time = opened)),
            (1, t!("signin-step-approve")),
            (0, t!("signin-step-licences")),
        ] {
            ui.horizontal(|ui| {
                let (icon, color, font) = match state {
                    2 => (icons::CHECK_CIRCLE, c.status_success, fonts::ui(12.0)),
                    1 => (icons::CIRCLE_HALF, c.select_blue, fonts::semibold(12.0)),
                    _ => (icons::CIRCLE, c.text_secondary, fonts::ui(12.0)),
                };
                ui.label(RichText::new(icon).font(fonts::icon(16.0)).color(color));
                ui.label(RichText::new(label).font(font).color(if state == 1 {
                    c.text_primary
                } else {
                    c.text_secondary
                }));
            });
        }
        ui.horizontal(|ui| {
            if Button::secondary(t!("signin-reopen"))
                .icon(icons::ARROW_SQUARE_OUT)
                .show(ui)
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            if Button::ghost(t!("signin-copy-link"))
                .icon(icons::LINK)
                .show(ui)
                .clicked()
            {
                ui.ctx().copy_text(url.to_owned());
                self.toast(BadgeKind::Success, t!("signin-link-copied"));
            }
            if Button::ghost(t!("cancel"))
                .icon(icons::X)
                .show(ui)
                .clicked()
            {
                self.sign_in_escape();
            }
        });
        let now = ui.input(|i| i.time);
        let started = *self.sign_in.started_at.get_or_insert(now);
        let left = (SIGN_IN_TIMEOUT.as_secs_f64() - (now - started)).max(0.0) as u64;
        egui::Frame::new()
            .fill(c.bg_sunken)
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{:02}:{:02}", left / 60, left % 60))
                            .font(fonts::mono_bold(13.0))
                            .color(c.text_primary),
                    );
                    text(ui, t!("signin-expiry"), fonts::ui(11.0), c.text_secondary);
                });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
}

/// Right-hand cell of a fact row.
type FactValue = Box<dyn Fn(&mut Ui)>;

fn hero_icon(ui: &mut Ui, icon: &str, color: egui::Color32) {
    ui.label(RichText::new(icon).font(fonts::icon(40.0)).color(color));
}

fn story(ui: &mut Ui, c: &Palette) {
    let t = tokens();
    let r = ui.max_rect();
    ui.horizontal(|ui| {
        paint::logo_tile(ui, c, vec2(28.0, 32.0));
        ui.label(
            RichText::new(t!("app-name"))
                .font(fonts::extrabold(18.0))
                .color(c.text_primary),
        );
    });
    // pitch at the bottom
    let pitch_h = 170.0;
    let mut pitch = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                egui::pos2(r.left(), r.bottom() - pitch_h),
                r.max,
            ))
            .layout(Layout::bottom_up(Align::Min)),
    );
    pitch.add(
        egui::Label::new(
            RichText::new(t!("story-body"))
                .font(fonts::style(&t.text.body_l))
                .color(c.text_secondary),
        )
        .wrap(),
    );
    pitch.add_space(12.0);
    pitch.add(
        egui::Label::new(
            RichText::new(t!("story-title"))
                .font(fonts::style(&t.text.display_hero))
                .color(c.text_primary),
        )
        .wrap(),
    );
    // shade guide in between
    let top = r.top() + 140.0;
    let bottom = (r.bottom() - pitch_h - 120.0).max(top + 120.0);
    paint::shade_guide(
        ui.painter(),
        c,
        Rect::from_min_max(egui::pos2(r.left(), top), egui::pos2(r.right(), bottom)),
    );
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    Some(rest.split(['/', '?']).next()?.to_owned())
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "-".to_owned())
}
