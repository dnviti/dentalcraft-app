//! "Licence and notices" dialog: copyright, the DentalCraft licence
//! (`LICENSE.md`, with `NOTICE.md`) and the third-party notices
//! (`THIRD-PARTY-NOTICES.md`), all embedded in the binary from the
//! repository root.

use std::sync::LazyLock;

use dc_ui::widgets::{self, Button};
use dc_ui::{fonts, icons, theme, tokens};
use egui::{Align, Layout, RichText, Ui};

use crate::app::DentalCraftApp;

/// The DentalCraft licence: the PolyForm Noncommercial License 1.0.0 with
/// the required notice.
pub const LICENCE: &str = include_str!("../../../LICENSE.md");

/// The project notice: commercial licensing, medical disclaimer, origin of
/// the code and the main third-party components.
pub const NOTICE: &str = include_str!("../../../NOTICE.md");

/// Copyright and licence notices of the bundled open source components.
pub const THIRD_PARTY_NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

/// What the open dialog shows below the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AboutView {
    /// Copyright and summary only.
    #[default]
    Summary,
    /// The DentalCraft licence.
    Licence,
    /// The third-party notices.
    ThirdParty,
}

/// The licence texts in reading order: the licence, then the notice.
pub fn licence_parts() -> [&'static str; 2] {
    [LICENCE, NOTICE]
}

/// Lines of the third-party notices, split once (the list is long and is
/// drawn a screenful at a time).
static NOTICE_LINES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| THIRD_PARTY_NOTICES.lines().collect());

/// One Markdown line of the licence: headings in a heavier face, the rest
/// as wrapped body text.
fn licence_line(ui: &mut Ui, line: &str, c: &dc_ui::Palette) {
    let (s, font, color) = if let Some(h) = line.strip_prefix("# ") {
        (h, fonts::bold(17.0), c.text_primary)
    } else if let Some(h) = line.strip_prefix("## ") {
        (h, fonts::semibold(15.0), c.text_primary)
    } else if let Some(h) = line.strip_prefix("### ") {
        (h, fonts::semibold(13.0), c.text_primary)
    } else if line.trim().is_empty() {
        ui.add_space(4.0);
        return;
    } else {
        (line, fonts::ui(13.0), c.text_secondary)
    };
    ui.add(
        egui::Label::new(RichText::new(s).font(font).color(color))
            .wrap()
            .selectable(true),
    );
}

impl DentalCraftApp {
    /// Opens the "Licence and notices" dialog.
    pub fn open_about(&mut self) {
        self.about = Some(AboutView::Summary);
    }

    /// What the "Licence and notices" dialog shows; `None` when closed.
    pub fn about(&self) -> Option<AboutView> {
        self.about
    }

    /// Draws the dialog when it is open.
    pub(crate) fn about_dialog(&mut self, ctx: &egui::Context) {
        let Some(view) = self.about else {
            return;
        };
        let c = *theme::ctx_colors(ctx);
        let screen = ctx.content_rect().size();
        let wide: f32 = if view == AboutView::Summary {
            560.0
        } else {
            820.0
        };
        let width = wide.min(screen.x - 80.0).max(320.0);
        let text_h = (screen.y - 320.0).clamp(160.0, 560.0);
        let mut next = Some(view);
        let resp = egui::Modal::new(egui::Id::unique("dc-about")).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icons::SEAL)
                        .font(fonts::icon(22.0))
                        .color(c.accent_amber_text),
                );
                ui.label(
                    RichText::new(t!("about-licence-title"))
                        .font(fonts::style(&tokens().text.title_s))
                        .color(c.text_primary),
                );
            });
            ui.label(
                RichText::new(format!("{} {}", t!("app-name"), env!("CARGO_PKG_VERSION")))
                    .font(fonts::medium(12.0))
                    .color(c.text_secondary),
            );
            ui.label(
                RichText::new(t!("about-copyright"))
                    .font(fonts::ui(13.0))
                    .color(c.text_primary),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(t!("about-licence-summary"))
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                )
                .wrap(),
            );
            match view {
                AboutView::Summary => {}
                AboutView::Licence => {
                    ui.add_space(4.0);
                    widgets::sunken_frame(ui).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("dc-about-licence")
                            .max_height(text_h)
                            .min_scrolled_height(text_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                for (i, part) in licence_parts().into_iter().enumerate() {
                                    if i > 0 {
                                        ui.add_space(8.0);
                                        widgets::hairline(ui);
                                        ui.add_space(8.0);
                                    }
                                    for line in part.lines() {
                                        licence_line(ui, line, &c);
                                    }
                                }
                            });
                    });
                }
                AboutView::ThirdParty => {
                    ui.add_space(4.0);
                    widgets::sunken_frame(ui).show(ui, |ui| {
                        // show_rows places rows with the spacing of this ui
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let font = fonts::ui(12.0);
                        let row_h = ui.fonts_mut(|f| f.row_height(&font));
                        let lines: &[&str] = &NOTICE_LINES;
                        egui::ScrollArea::both()
                            .id_salt("dc-about-notices")
                            .max_height(text_h)
                            .min_scrolled_height(text_h)
                            .auto_shrink([false, false])
                            .show_rows(ui, row_h, lines.len(), |ui, rows| {
                                for line in &lines[rows] {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(*line)
                                                .font(font.clone())
                                                .color(c.text_secondary),
                                        )
                                        .extend()
                                        .selectable(true),
                                    );
                                }
                            });
                    });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (target, label, icon) in [
                    (
                        AboutView::Licence,
                        t!("about-button-licence"),
                        icons::SCROLL,
                    ),
                    (
                        AboutView::ThirdParty,
                        t!("about-button-third-party"),
                        icons::LIST,
                    ),
                ] {
                    let button = if view == target {
                        Button::primary(label)
                    } else {
                        Button::secondary(label)
                    };
                    if button.icon(icon).show(ui).clicked() {
                        next = Some(if view == target {
                            AboutView::Summary
                        } else {
                            target
                        });
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::ghost(t!("about-button-close"))
                        .icon(icons::X)
                        .show(ui)
                        .clicked()
                    {
                        next = None;
                    }
                });
            });
        });
        if resp.should_close() {
            next = None;
        }
        self.about = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_licence_is_polyform_noncommercial_with_the_required_notice() {
        let [licence, notice] = licence_parts();
        assert!(licence.starts_with("Required Notice: Copyright (c) Daniele Viti"));
        assert!(licence.contains("# PolyForm Noncommercial License 1.0.0"));
        assert!(notice.contains("Commercial licensing"));
    }

    #[test]
    fn the_notices_name_the_ui_toolkit() {
        assert!(NOTICE_LINES.iter().any(|l| l.starts_with("| egui |")));
    }
}
