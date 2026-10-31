//! egui theme from the design tokens: fonts, visuals and spacing for both
//! modes (dark is the default).

use egui::{
    Color32, CornerRadius, Margin, Shadow, Stroke, Style, TextStyle, Visuals, style::Selection,
};

use crate::fonts;
use crate::tokens::{Palette, ShadowToken, tokens};

/// Theme mode (the Color collection modes of the design file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Graphite (default).
    #[default]
    Dark,
    /// Light.
    Light,
}

impl Mode {
    /// The egui theme of this mode.
    pub fn egui_theme(self) -> egui::Theme {
        match self {
            Mode::Dark => egui::Theme::Dark,
            Mode::Light => egui::Theme::Light,
        }
    }

    /// The mode of an egui theme.
    pub fn from_egui(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Dark => Mode::Dark,
            egui::Theme::Light => Mode::Light,
        }
    }
}

/// Installs fonts and the styles of both modes, and selects `mode`.
/// Call once at startup; call [`set_mode`] to switch later.
pub fn install(ctx: &egui::Context, mode: Mode) {
    fonts::install(ctx);
    for m in [Mode::Dark, Mode::Light] {
        ctx.style_mut_of(m.egui_theme(), |s| style(m, s));
    }
    set_mode(ctx, mode);
}

/// Switches between the already installed dark and light styles.
pub fn set_mode(ctx: &egui::Context, mode: Mode) {
    ctx.set_theme(mode.egui_theme());
}

/// The mode the context currently paints with.
pub fn mode(ctx: &egui::Context) -> Mode {
    Mode::from_egui(ctx.theme())
}

/// The palette of the mode `ui` paints with.
pub fn colors(ui: &egui::Ui) -> &'static Palette {
    palette_for(ui.visuals().dark_mode)
}

/// The palette of the mode the context paints with.
pub fn ctx_colors(ctx: &egui::Context) -> &'static Palette {
    tokens().colors(mode(ctx))
}

fn palette_for(dark: bool) -> &'static Palette {
    tokens().colors(if dark { Mode::Dark } else { Mode::Light })
}

fn cr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Shadow from a token.
pub fn shadow(t: &ShadowToken) -> Shadow {
    Shadow {
        offset: [t.offset[0] as i8, t.offset[1] as i8],
        blur: t.blur as u8,
        spread: t.spread as u8,
        color: t.color,
    }
}

/// Visuals of `mode`, following `egui_visuals_mapping` of tokens.json.
pub fn visuals(mode: Mode) -> Visuals {
    let t = tokens();
    let c = t.colors(mode);
    let mut v = match mode {
        Mode::Dark => Visuals::dark(),
        Mode::Light => Visuals::light(),
    };
    let hair = t.stroke.hairline;
    let radius = cr(t.radius.sm);

    v.override_text_color = Some(c.text_primary);
    v.weak_text_color = Some(c.text_secondary);
    v.hyperlink_color = c.accent_amber_text;
    v.faint_bg_color = c.bg_hover;
    v.extreme_bg_color = c.bg_sunken;
    v.text_edit_bg_color = Some(c.bg_sunken);
    v.code_bg_color = c.bg_sunken;
    v.warn_fg_color = c.status_warning;
    v.error_fg_color = c.status_danger;
    v.panel_fill = c.bg_panel;
    v.window_fill = c.bg_raised;
    v.window_stroke = Stroke::new(hair, c.border_subtle);
    v.window_corner_radius = cr(t.radius.lg);
    v.window_shadow = shadow(&t.shadow.e2_dialog);
    v.popup_shadow = shadow(&t.shadow.e1_popover);
    v.menu_corner_radius = cr(t.radius.md);
    v.selection = Selection {
        bg_fill: c.select_soft,
        stroke: Stroke::new(t.stroke.focus_ring, c.select_blue),
    };
    v.text_cursor.stroke = Stroke::new(2.0, c.select_blue);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.indent_has_left_vline = false;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = c.bg_panel;
    w.noninteractive.weak_bg_fill = c.bg_panel;
    w.noninteractive.bg_stroke = Stroke::new(hair, c.border_subtle);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.text_primary);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = c.bg_raised;
    w.inactive.weak_bg_fill = c.bg_raised;
    w.inactive.bg_stroke = Stroke::new(hair, c.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, c.text_primary);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = c.bg_hover;
    w.hovered.weak_bg_fill = c.bg_hover;
    w.hovered.bg_stroke = Stroke::new(hair, c.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.5, c.text_primary);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = c.accent_amber_pressed;
    w.active.weak_bg_fill = c.bg_hover;
    w.active.bg_stroke = Stroke::new(t.stroke.focus_ring, c.focus_ring);
    w.active.fg_stroke = Stroke::new(2.0, c.text_primary);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = c.bg_hover;
    w.open.weak_bg_fill = c.bg_hover;
    w.open.bg_stroke = Stroke::new(hair, c.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, c.text_primary);
    w.open.corner_radius = radius;
    v
}

/// Applies the DentalCraft visuals, text styles and spacing of `mode` to `style`.
pub fn style(mode: Mode, style: &mut Style) {
    let t = tokens();
    style.visuals = visuals(mode);
    style.text_styles = [
        (TextStyle::Small, fonts::style(&t.text.caption)),
        (TextStyle::Body, fonts::style(&t.text.body_m)),
        (TextStyle::Button, fonts::style(&t.text.body_m_strong)),
        (TextStyle::Heading, fonts::style(&t.text.title_m)),
        (TextStyle::Monospace, fonts::style(&t.text.mono_readout)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(t.space.s2, t.space.s2);
    s.button_padding = egui::vec2(t.space.s3, t.space.s1 + 2.0);
    s.interact_size = egui::vec2(t.size.min_hit_target, 24.0);
    s.window_margin = Margin::same(t.space.s6 as i8);
    s.menu_margin = Margin::same(t.space.s1 as i8);
    s.indent = t.space.s6;
    s.icon_width = 16.0;
    s.icon_width_inner = 10.0;
    s.icon_spacing = t.space.s2;
    s.slider_rail_height = 4.0;
    s.combo_height = 300.0;
    style.interaction.tooltip_delay = 0.4;
    style.url_in_tooltip = true;
}

/// Linear blend of two colours (`t` = 0 gives `a`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_sets_both_modes() {
        let ctx = egui::Context::default();
        install(&ctx, Mode::Dark);
        assert_eq!(mode(&ctx), Mode::Dark);
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).visuals.panel_fill,
            tokens().dark.bg_panel
        );
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.panel_fill,
            tokens().light.bg_panel
        );
        set_mode(&ctx, Mode::Light);
        assert_eq!(mode(&ctx), Mode::Light);
        assert_eq!(ctx_colors(&ctx), &tokens().light);
    }
}
