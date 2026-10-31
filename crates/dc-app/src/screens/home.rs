//! `01 Home`: greeting, Open / New project, module cards, recent projects,
//! account card and "From the bench".

use dc_auth_client::AuthState;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{Palette, fonts, icons, tokens};
use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};

use crate::app::{DentalCraftApp, Screen};
use crate::config::RecentProject;
use crate::modules::ModuleInfo;
use crate::paint;

/// A "From the bench" entry (static for now).
#[derive(Debug, Clone, Copy)]
pub struct NewsItem {
    /// Phosphor icon.
    pub icon: &'static str,
    /// Title key.
    pub title_key: &'static str,
    /// Body key.
    pub body_key: &'static str,
    /// Link label key and URL.
    pub link_key: Option<&'static str>,
    /// Link URL.
    pub url: Option<&'static str>,
}

/// The news list.
// TODO(server): fetch from the server once there is a news endpoint.
pub const NEWS: [NewsItem; 3] = [
    NewsItem {
        icon: icons::SPARKLE,
        title_key: "news-1-title",
        body_key: "news-1-body",
        link_key: Some("news-1-link"),
        url: Some("https://dentalcraft.app/releases"),
    },
    NewsItem {
        icon: icons::LIGHTBULB,
        title_key: "news-2-title",
        body_key: "news-2-body",
        link_key: None,
        url: None,
    },
    NewsItem {
        icon: icons::CALENDAR_CHECK,
        title_key: "news-3-title",
        body_key: "news-3-body",
        link_key: None,
        url: None,
    },
];

const RAIL_W: f32 = 380.0;

fn text(
    ui: &mut Ui,
    s: impl Into<String>,
    font: egui::FontId,
    color: egui::Color32,
) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(s).font(font).color(color)).wrap())
}

/// Relative "edited" time.
fn edited(touched: u64) -> String {
    let now = crate::config::now_unix();
    let d = now.saturating_sub(touched);
    if d < 3600 {
        t!("time-minutes-ago", n = (d / 60).max(1).to_string())
    } else if d < 86_400 {
        t!("time-hours-ago", n = (d / 3600).to_string())
    } else if d < 2 * 86_400 {
        t!("time-yesterday")
    } else {
        let dt = chrono::DateTime::from_timestamp(touched as i64, 0)
            .map(|d| d.with_timezone(&chrono::Local));
        dt.map_or_else(String::new, |d| {
            use chrono::Datelike;
            format!(
                "{} {}",
                d.day(),
                dc_i18n::tr(crate::app::MONTH_KEYS[d.month0() as usize])
                    .chars()
                    .take(3)
                    .collect::<String>()
            )
        })
    }
}

impl DentalCraftApp {
    pub(crate) fn home_ui(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 48,
                        right: 48,
                        top: 40,
                        bottom: 32,
                    })
                    .show(ui, |ui| {
                        let full = ui.available_width();
                        let main_w = (full - RAIL_W - 40.0).max(600.0);
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 40.0;
                            ui.vertical(|ui| {
                                ui.set_width(main_w);
                                self.hero(ui, &c);
                                ui.add_space(40.0);
                                self.modules_row(ui, &c);
                                ui.add_space(40.0);
                                self.recent_projects(ui, &c);
                            });
                            ui.vertical(|ui| {
                                ui.set_width(RAIL_W.min(full - main_w - 40.0).max(280.0));
                                self.account_card(ui, &c);
                                ui.add_space(20.0);
                                news_card(ui, &c);
                            });
                        });
                    });
            });
    }

    fn hero(&mut self, ui: &mut Ui, c: &Palette) {
        let (greeting, date) = self.greeting();
        let t = tokens();
        ui.spacing_mut().item_spacing.y = 10.0;
        text(ui, date, fonts::medium(13.0), c.text_secondary);
        text(
            ui,
            greeting,
            fonts::style(&t.text.display_hero),
            c.text_primary,
        );
        ui.horizontal(|ui| {
            let w = ui.available_width();
            ui.vertical(|ui| {
                ui.set_max_width(640.0_f32.min(w - 320.0).max(300.0));
                let sub = if self.user().is_some() {
                    t!("home-sub")
                } else {
                    t!("home-sub-signed-out")
                };
                text(ui, sub, fonts::style(&t.text.body_l), c.text_secondary);
            });
            ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if Button::primary(t!("home-new-project"))
                    .icon(icons::PLUS)
                    .large()
                    .show(ui)
                    .clicked()
                {
                    self.screen = Screen::NewProject;
                }
                if Button::secondary(t!("home-open-project"))
                    .icon(icons::FOLDER_OPEN)
                    .large()
                    .show(ui)
                    .clicked()
                {
                    self.pick_project_to_open();
                }
            });
        });
    }

    fn modules_row(&mut self, ui: &mut Ui, c: &Palette) {
        let installed = self.module_cards.iter().filter(|m| m.installed()).count();
        let soon = self.module_cards.len() - installed;
        widgets::section_header(
            ui,
            t!("home-modules"),
            Some(&t!(
                "home-modules-count",
                installed = installed.to_string(),
                soon = soon.to_string()
            )),
        );
        ui.add_space(16.0);
        let cards = self.module_cards.clone();
        let gap = 20.0;
        let w = ui.available_width();
        let others = cards.iter().filter(|m| !m.installed()).count() as f32;
        let first_w = 340.0_f32.min(w * 0.3);
        let other_w = ((w - first_w * installed as f32 - gap * (cards.len() as f32 - 1.0))
            / others.max(1.0))
        .max(150.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for m in &cards {
                let cw = if m.installed() { first_w } else { other_w };
                if module_card(ui, c, m, cw) {
                    if m.installed() {
                        self.new_project.module = m.id.to_owned();
                        self.screen = Screen::NewProject;
                    } else {
                        self.toast(BadgeKind::Info, t!("module-notify-ok"));
                    }
                }
            }
        });
    }

    fn recent_projects(&mut self, ui: &mut Ui, c: &Palette) {
        let n = self.recent.items.len();
        ui.horizontal(|ui| {
            widgets::section_header(ui, t!("home-recent-projects"), None);
            ui.label(
                RichText::new(crate::diagnostics::tilde(&self.store.dirs().projects))
                    .font(fonts::mono(12.0))
                    .color(c.text_secondary),
            );
            if n > 4 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if self.show_all_recent {
                        t!("home-show-fewer")
                    } else {
                        t!("home-show-all", count = n.to_string())
                    };
                    if Button::ghost(label).icon(icons::LIST).show(ui).clicked() {
                        self.show_all_recent = !self.show_all_recent;
                    }
                });
            }
        });
        ui.add_space(16.0);
        if n == 0 {
            widgets::sunken_frame(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(icons::FOLDER_OPEN)
                            .font(fonts::icon(20.0))
                            .color(c.text_secondary),
                    );
                    text(
                        ui,
                        t!("home-recent-empty"),
                        fonts::ui(13.0),
                        c.text_secondary,
                    );
                });
            });
            return;
        }
        let items: Vec<RecentProject> = if self.show_all_recent {
            self.recent.items.clone()
        } else {
            self.recent.items.iter().take(4).cloned().collect()
        };
        let gap = 20.0;
        let w = ((ui.available_width() - 3.0 * gap) / 4.0).clamp(200.0, 330.0);
        let ctx = ui.ctx().clone();
        let mut open = None;
        let mut forget = None;
        for row in items.chunks(4) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for item in row {
                    let thumb = self.thumbnail(&ctx, &item.path);
                    let resp = project_card(ui, c, item, w, thumb.as_ref());
                    if resp.clicked() {
                        open = Some(item.path.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button(t!("home-recent-forget")).clicked() {
                            forget = Some(item.path.clone());
                            ui.close();
                        }
                    });
                }
            });
            ui.add_space(gap);
        }
        if let Some(p) = forget {
            self.recent.remove(&p);
            self.store.save_recent(&self.recent);
        }
        if let Some(p) = open {
            if p.exists() {
                self.open_project(p);
            } else {
                self.toast(
                    BadgeKind::Warning,
                    t!("home-recent-missing", path = p.display().to_string()),
                );
            }
        }
    }

    fn account_card(&mut self, ui: &mut Ui, c: &Palette) {
        let state = self.auth_state.clone();
        widgets::card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 12.0;
            ui.horizontal(|ui| {
                text(
                    ui,
                    t!("account-title"),
                    fonts::semibold(15.0),
                    c.text_primary,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &state {
                    AuthState::SignedIn(_) => {
                        widgets::badge(ui, BadgeKind::Success, t!("account-badge-signed-in"));
                    }
                    AuthState::Working | AuthState::WaitingForBrowser { .. } => {
                        widgets::badge(ui, BadgeKind::Info, t!("account-badge-signing-in"));
                    }
                    _ => {
                        widgets::badge_icon(
                            ui,
                            BadgeKind::Neutral,
                            Some(icons::CLOUD_SLASH),
                            t!("account-badge-signed-out"),
                        );
                    }
                });
            });
            match &state {
                AuthState::SignedIn(u) => {
                    text(ui, u.email.clone(), fonts::ui(13.0), c.text_secondary);
                    widgets::sunken_frame(ui).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for m in self.module_cards.iter().filter(|m| m.installed()) {
                            let licensed = u.entitlements.has_module(m.id);
                            ui.horizontal(|ui| {
                                text(
                                    ui,
                                    t!("account-licence", module = dc_i18n::tr(m.name_key)),
                                    fonts::medium(12.0),
                                    c.text_primary,
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if licensed {
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
                                });
                            });
                        }
                        text(
                            ui,
                            t!(
                                "account-modules-granted",
                                count = u.entitlements.modules.len().to_string()
                            ),
                            fonts::ui(11.0),
                            c.text_secondary,
                        );
                    });
                    ui.horizontal(|ui| {
                        if Button::secondary(t!("account-manage"))
                            .icon(icons::ARROW_SQUARE_OUT)
                            .show(ui)
                            .clicked()
                        {
                            self.open_account_page(ui.ctx());
                        }
                        if Button::ghost(t!("account-sign-out"))
                            .icon(icons::SIGN_OUT)
                            .show(ui)
                            .clicked()
                        {
                            self.sign_out();
                        }
                    });
                }
                _ => {
                    text(
                        ui,
                        t!("account-signed-out-body"),
                        fonts::ui(13.0),
                        c.text_secondary,
                    );
                    if Button::primary(t!("account-sign-in"))
                        .icon(icons::SIGN_IN)
                        .show(ui)
                        .clicked()
                    {
                        self.begin_sign_in();
                    }
                    offline_note(ui, c);
                }
            }
        });
    }
}

/// The offline / evaluation note box (shared with the sign-in card).
pub(crate) fn offline_note(ui: &mut Ui, c: &Palette) {
    egui::Frame::new()
        .fill(c.bg_sunken)
        .stroke(Stroke::new(1.0, c.border_subtle))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(
                    RichText::new(icons::CLOUD_SLASH)
                        .font(fonts::icon(18.0))
                        .color(c.text_secondary),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    text(
                        ui,
                        t!("offline-note-title"),
                        fonts::semibold(13.0),
                        c.text_primary,
                    );
                    text(
                        ui,
                        t!("offline-note-body"),
                        fonts::ui(12.0),
                        c.text_secondary,
                    );
                });
            });
        });
}

fn news_card(ui: &mut Ui, c: &Palette) {
    widgets::card_frame(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(ui.available_height().max(300.0));
        text(ui, t!("news-title"), fonts::semibold(15.0), c.text_primary);
        text(ui, t!("news-sub"), fonts::ui(12.0), c.text_secondary);
        ui.add_space(12.0);
        for (i, n) in NEWS.iter().enumerate() {
            if i > 0 {
                ui.add_space(4.0);
                widgets::hairline(ui);
                ui.add_space(4.0);
            }
            ui.horizontal_top(|ui| {
                let color = if i == 0 {
                    c.accent_amber_text
                } else {
                    c.text_secondary
                };
                ui.label(RichText::new(n.icon).font(fonts::icon(20.0)).color(color));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    text(
                        ui,
                        dc_i18n::tr(n.title_key),
                        fonts::semibold(13.0),
                        c.text_primary,
                    );
                    text(
                        ui,
                        dc_i18n::tr(n.body_key),
                        fonts::ui(12.0),
                        c.text_secondary,
                    );
                    if let (Some(k), Some(url)) = (n.link_key, n.url)
                        && widgets::link(ui, dc_i18n::tr(k)).clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                });
            });
        }
    });
}

/// A module card (shade-tab shape). Returns `true` when its action was clicked.
fn module_card(ui: &mut Ui, c: &Palette, m: &ModuleInfo, width: f32) -> bool {
    let height = 270.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter().clone();
    let radius = [16.0, 16.0, 4.0, 4.0];
    let corner = widgets::shade_tab_radius(16.0, 4.0);
    let installed = m.installed();
    // body
    if installed {
        p.rect(
            rect,
            corner,
            c.bg_panel,
            Stroke::new(1.0, c.border_subtle),
            StrokeKind::Inside,
        );
    } else {
        paint::dashed_rect(
            &p,
            rect.shrink(0.5),
            radius,
            Stroke::new(1.0, c.border_strong),
        );
    }
    // handle band
    let band = Rect::from_min_size(rect.min, vec2(width, 44.0));
    if installed {
        p.rect_filled(
            band.shrink(1.0),
            widgets::shade_tab_radius(15.0, 0.0),
            c.bg_sunken,
        );
    }
    p.text(
        band.left_center() + vec2(20.0, 0.0),
        Align2::LEFT_CENTER,
        m.code,
        fonts::mono_bold(20.0),
        if installed {
            c.accent_amber_text
        } else {
            c.text_secondary
        },
    );
    let meta = match (m.version, m.eta_key) {
        (Some(v), _) => format!("v{v}"),
        (None, Some(k)) => dc_i18n::tr(k),
        _ => String::new(),
    };
    p.text(
        band.right_center() - vec2(20.0, 0.0),
        Align2::RIGHT_CENTER,
        meta,
        if installed {
            fonts::mono(11.0)
        } else {
            fonts::medium(11.0)
        },
        c.text_secondary,
    );
    // body content
    let body = Rect::from_min_max(
        pos2(rect.left() + 20.0, band.bottom() + 20.0),
        pos2(rect.right() - 20.0, rect.bottom() - 48.0),
    );
    let fg = if installed {
        c.text_primary
    } else {
        c.text_secondary
    };
    p.text(
        body.left_top(),
        Align2::LEFT_TOP,
        m.icon,
        fonts::icon(32.0),
        fg,
    );
    p.text(
        body.left_top() + vec2(0.0, 50.0),
        Align2::LEFT_TOP,
        dc_i18n::tr(m.name_key),
        fonts::semibold(18.0),
        fg,
    );
    let desc = p.layout(
        dc_i18n::tr(m.desc_key),
        fonts::ui(13.0),
        c.text_secondary,
        body.width(),
    );
    p.galley(body.left_top() + vec2(0.0, 82.0), desc, c.text_secondary);
    // footer
    let footer_y = rect.bottom() - 48.0;
    p.line_segment(
        [
            pos2(rect.left() + 1.0, footer_y),
            pos2(rect.right() - 1.0, footer_y),
        ],
        Stroke::new(1.0, c.border_subtle),
    );
    let fy = footer_y + 24.0;
    let (status_icon, status, status_color) = if installed {
        (
            icons::CHECK_CIRCLE,
            t!("module-installed"),
            c.status_success,
        )
    } else {
        (
            icons::HOURGLASS_MEDIUM,
            t!("module-coming-soon"),
            c.text_secondary,
        )
    };
    p.text(
        pos2(rect.left() + 20.0, fy),
        Align2::LEFT_CENTER,
        status_icon,
        fonts::icon(14.0),
        status_color,
    );
    // action
    let action = if installed {
        t!("module-open")
    } else {
        t!("module-notify")
    };
    let galley = p.layout_no_wrap(action.clone(), fonts::semibold(13.0), Color32::PLACEHOLDER);
    // the status word only when it fits next to the action (narrow cards keep the icon)
    let status_g = p.layout_no_wrap(status, fonts::medium(12.0), c.text_primary);
    if 40.0 + status_g.size().x + 16.0 + galley.size().x + 20.0 <= width {
        p.galley(
            pos2(rect.left() + 40.0, fy - status_g.size().y / 2.0),
            status_g,
            c.text_primary,
        );
    }
    let ar = Rect::from_center_size(
        pos2(rect.right() - 20.0 - galley.size().x / 2.0, fy),
        galley.size() + vec2(12.0, 10.0),
    );
    let a = ui.interact(ar, resp.id.with("action"), Sense::click());
    let name = dc_i18n::tr(m.name_key);
    a.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{action} {name}"))
    });
    if a.hovered() {
        p.rect_filled(ar, CornerRadius::same(4), c.bg_hover);
    }
    let color = if installed {
        c.accent_amber_text
    } else {
        c.text_primary
    };
    p.galley(ar.center() - galley.size() / 2.0, galley, color);
    widgets::focus_ring(ui, &a, ar, CornerRadius::same(4));
    a.clicked()
}

fn project_card(
    ui: &mut Ui,
    c: &Palette,
    item: &RecentProject,
    width: f32,
    thumb: Option<&egui::TextureHandle>,
) -> egui::Response {
    let height = 168.0 + 74.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let name = item.name();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let p = ui.painter().clone();
    let corner = CornerRadius::same(8);
    let border = if resp.hovered() {
        c.border_strong
    } else {
        c.border_subtle
    };
    p.rect(
        rect,
        corner,
        c.bg_panel,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let thumb_rect = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(width - 2.0, 168.0));
    p.rect_filled(
        thumb_rect,
        CornerRadius {
            nw: 7,
            ne: 7,
            sw: 0,
            se: 0,
        },
        c.bg_viewport,
    );
    match thumb {
        Some(t) => {
            let size = t.size_vec2();
            let scale = (thumb_rect.width() / size.x).min(thumb_rect.height() / size.y);
            let r = Rect::from_center_size(thumb_rect.center(), size * scale);
            p.image(
                t.id(),
                r,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => paint::arch(&p, c, thumb_rect.shrink2(vec2(0.0, 8.0))),
    }
    let x = rect.left() + 16.0;
    let name_g = p.layout(name, fonts::semibold(13.0), c.text_primary, width - 32.0);
    p.galley(pos2(x, thumb_rect.bottom() + 14.0), name_g, c.text_primary);
    // status pill + edited
    let pill_y = thumb_rect.bottom() + 46.0;
    let missing = !item.path.exists();
    let (tone, label, icon) = if missing {
        (
            c.status_danger,
            t!("home-recent-file-missing"),
            icons::WARNING,
        )
    } else {
        (
            c.select_blue,
            dc_i18n::tr(if item.module == "aligners" {
                dca_module::NAME_KEY
            } else {
                "module-generic-desc"
            }),
            icons::TOOTH,
        )
    };
    let lg = p.layout_no_wrap(label, fonts::medium(12.0), tone);
    let pill = Rect::from_min_size(pos2(x, pill_y - 11.0), vec2(lg.size().x + 32.0, 22.0));
    p.rect(
        pill,
        CornerRadius::same(11),
        tone.gamma_multiply(0.12),
        Stroke::new(1.0, tone),
        StrokeKind::Inside,
    );
    p.text(
        pill.left_center() + vec2(12.0, 0.0),
        Align2::CENTER_CENTER,
        icon,
        fonts::icon(12.0),
        tone,
    );
    p.galley(
        pos2(pill.left() + 22.0, pill_y - lg.size().y / 2.0),
        lg,
        tone,
    );
    p.text(
        pos2(rect.right() - 16.0, pill_y),
        Align2::RIGHT_CENTER,
        edited(item.touched),
        fonts::ui(12.0),
        c.text_secondary,
    );
    widgets::focus_ring(ui, &resp, rect, corner);
    resp.on_hover_text(item.path.display().to_string())
}
