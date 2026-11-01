//! Shared widgets styled per the "06 Design system" page: buttons, badges,
//! inputs, toggles, sliders, tabs, segmented controls, cards.
//!
//! Every interactive widget is keyboard focusable, draws the 2 pt
//! `focus_ring` outside itself when focused and publishes accessibility info
//! (so `egui_kittest` can find it by label).

use std::ops::RangeInclusive;

use egui::{
    Align2, Color32, CornerRadius, EventFilter, Frame, Key, Margin, Pos2, Rect, Response, Role,
    Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, pos2, vec2,
};

use crate::fonts;
use crate::theme::{colors, mix};
use crate::tokens::{Palette, tokens};

fn radius(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Shade-tab corner radius (16/16/4/4) scaled to `top`/`bottom`.
pub fn shade_tab_radius(top: f32, bottom: f32) -> CornerRadius {
    CornerRadius {
        nw: top as u8,
        ne: top as u8,
        sw: bottom as u8,
        se: bottom as u8,
    }
}

/// Paints the 2 pt focus ring outside `rect` when `response` has keyboard focus.
pub fn focus_ring(ui: &Ui, response: &Response, rect: Rect, corner: CornerRadius) {
    if response.has_focus() {
        let c = colors(ui);
        ui.painter().rect_stroke(
            rect.expand(1.0),
            corner,
            Stroke::new(tokens().stroke.focus_ring, c.focus_ring),
            StrokeKind::Outside,
        );
    }
}

// ---------------------------------------------------------------- buttons

/// Button variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Amber fill: the one main action of a view.
    Primary,
    /// Raised fill with a strong border.
    Secondary,
    /// No fill until hovered.
    Ghost,
}

/// Button height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ButtonSize {
    /// 28 pt (dense rows, segmented items).
    Compact,
    /// 36 pt (panels).
    Panel,
    /// 44 pt (Home, dialogs).
    Large,
    /// 48 pt (single call to action, sign-in).
    Cta,
}

impl ButtonSize {
    /// Height in points.
    pub fn height(self) -> f32 {
        let s = &tokens().size;
        match self {
            ButtonSize::Compact => 28.0,
            ButtonSize::Panel => s.button_h,
            ButtonSize::Large => s.button_h_large,
            ButtonSize::Cta => s.button_h_cta,
        }
    }
}

/// A design-system button. Use [`primary_button`] and friends for the
/// common cases, this builder for icons, sizes and disabled state.
#[must_use = "call .show(ui)"]
pub struct Button {
    kind: ButtonKind,
    label: String,
    icon: Option<&'static str>,
    size: ButtonSize,
    enabled: bool,
    full_width: bool,
    min_width: f32,
}

impl Button {
    /// A button of `kind`.
    pub fn new(kind: ButtonKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            icon: None,
            size: ButtonSize::Panel,
            enabled: true,
            full_width: false,
            min_width: 0.0,
        }
    }

    /// Primary (amber) button.
    pub fn primary(label: impl Into<String>) -> Self {
        Self::new(ButtonKind::Primary, label)
    }

    /// Secondary button.
    pub fn secondary(label: impl Into<String>) -> Self {
        Self::new(ButtonKind::Secondary, label)
    }

    /// Ghost button.
    pub fn ghost(label: impl Into<String>) -> Self {
        Self::new(ButtonKind::Ghost, label)
    }

    /// Leading Phosphor icon (16 pt).
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Height.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// 44 pt tall.
    pub fn large(self) -> Self {
        self.size(ButtonSize::Large)
    }

    /// 48 pt tall.
    pub fn cta(self) -> Self {
        self.size(ButtonSize::Cta)
    }

    /// Disabled buttons do not react and use `text_disabled`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Takes the whole available width.
    pub fn full_width(mut self, full: bool) -> Self {
        self.full_width = full;
        self
    }

    /// Minimum width.
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// Adds the button.
    pub fn show(self, ui: &mut Ui) -> Response {
        let c = *colors(ui);
        let pad = if self.kind == ButtonKind::Ghost {
            12.0
        } else {
            16.0
        };
        let font = fonts::semibold(13.0);
        let text_color = |hover: bool, pressed: bool| -> Color32 {
            if !self.enabled {
                c.text_disabled
            } else {
                match self.kind {
                    ButtonKind::Primary => c.accent_amber_on,
                    _ if hover || pressed => c.text_primary,
                    _ => c.text_primary,
                }
            }
        };
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.clone(), font, Color32::PLACEHOLDER);
        let icon_galley = self.icon.map(|i| {
            ui.painter()
                .layout_no_wrap(i.into(), fonts::icon(16.0), Color32::PLACEHOLDER)
        });
        let icon_w = icon_galley
            .as_ref()
            .map(|g| g.size().x + if self.label.is_empty() { 0.0 } else { 8.0 })
            .unwrap_or(0.0);
        let content_w = icon_w + galley.size().x;
        let mut width = (content_w + 2.0 * pad).max(self.min_width);
        if self.full_width {
            width = width.max(ui.available_width());
        }
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, self.size.height()), sense);
        let label = self.label.clone();
        let enabled = self.enabled;
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));
        if ui.is_rect_visible(rect) {
            let hovered = self.enabled && response.hovered();
            let pressed = self.enabled && response.is_pointer_button_down_on();
            let r = radius(tokens().radius.sm);
            let (fill, stroke) = match (self.kind, self.enabled) {
                (_, false) => (c.bg_hover, Stroke::NONE),
                (ButtonKind::Primary, _) => (
                    if pressed {
                        c.accent_amber_pressed
                    } else if hovered {
                        c.accent_amber_hover
                    } else {
                        c.accent_amber
                    },
                    Stroke::NONE,
                ),
                (ButtonKind::Secondary, _) => (
                    if hovered || pressed {
                        c.bg_hover
                    } else {
                        c.bg_raised
                    },
                    Stroke::new(1.0, c.border_strong),
                ),
                (ButtonKind::Ghost, _) => (
                    if hovered || pressed {
                        c.bg_hover
                    } else {
                        Color32::TRANSPARENT
                    },
                    Stroke::NONE,
                ),
            };
            let p = ui.painter();
            p.rect(rect, r, fill, stroke, StrokeKind::Inside);
            focus_ring(ui, &response, rect, r);
            let color = text_color(hovered, pressed);
            let mut x = rect.center().x - content_w / 2.0;
            if let Some(g) = icon_galley {
                let pos = pos2(x, rect.center().y - g.size().y / 2.0);
                x += icon_w;
                p.galley(pos, g, color);
            }
            p.galley(
                pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
        if self.enabled {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            response
        }
    }
}

/// Primary (amber) button, 36 pt. At most one per view.
pub fn primary_button(ui: &mut Ui, label: impl Into<String>) -> Response {
    Button::primary(label).show(ui)
}

/// Secondary button, 36 pt.
pub fn secondary_button(ui: &mut Ui, label: impl Into<String>) -> Response {
    Button::secondary(label).show(ui)
}

/// Ghost button, 36 pt.
pub fn ghost_button(ui: &mut Ui, label: impl Into<String>) -> Response {
    Button::ghost(label).show(ui)
}

/// Square ghost icon button (32 pt, icon 18) with a tooltip, which is also
/// its accessible label.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: impl Into<String>) -> Response {
    icon_button_sized(ui, icon, tooltip, tokens().size.icon_button, true)
}

/// Icon button with an explicit size and enabled state.
pub fn icon_button_sized(
    ui: &mut Ui,
    icon: &str,
    tooltip: impl Into<String>,
    size: f32,
    enabled: bool,
) -> Response {
    let c = *colors(ui);
    let tooltip = tooltip.into();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let label = tooltip.clone();
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));
    if ui.is_rect_visible(rect) {
        let r = radius(tokens().radius.sm);
        if enabled && (response.hovered() || response.is_pointer_button_down_on()) {
            ui.painter().rect_filled(rect, r, c.bg_hover);
        }
        focus_ring(ui, &response, rect, r);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon,
            fonts::icon((size * 0.56).round()),
            if enabled {
                c.text_primary
            } else {
                c.text_disabled
            },
        );
    }
    response.on_hover_text(tooltip)
}

/// Underlined amber text link.
pub fn link(ui: &mut Ui, text: impl Into<String>) -> Response {
    let c = *colors(ui);
    let text = text.into();
    let galley =
        ui.painter()
            .layout_no_wrap(text.clone(), fonts::semibold(13.0), c.accent_amber_text);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Link, true, &text));
    let p = ui.painter();
    let color = if response.hovered() {
        c.accent_amber_hover
    } else {
        c.accent_amber_text
    };
    p.galley(rect.min, galley, color);
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, color),
    );
    focus_ring(ui, &response, rect.expand(2.0), radius(2.0));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------- badges

/// Badge tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// Grey, e.g. "Not signed in", "Coming soon".
    Neutral,
    /// Curing blue, e.g. "In progress", project stage.
    Info,
    /// Green, e.g. "Signed in", "Resolved".
    Success,
    /// Yellow, e.g. "Waiting for you".
    Warning,
    /// Red, e.g. "Collision".
    Danger,
    /// Amber, e.g. "New".
    Amber,
}

impl BadgeKind {
    /// Text and stroke colour of the tone.
    pub fn color(self, c: &Palette) -> Color32 {
        match self {
            BadgeKind::Neutral => c.text_secondary,
            BadgeKind::Info => c.select_blue,
            BadgeKind::Success => c.status_success,
            BadgeKind::Warning => c.status_warning,
            BadgeKind::Danger => c.status_danger,
            BadgeKind::Amber => c.accent_amber_text,
        }
    }

    /// Default icon of the tone (status is never colour alone).
    pub fn default_icon(self) -> &'static str {
        use crate::icons;
        match self {
            BadgeKind::Neutral => icons::CIRCLE,
            BadgeKind::Info => icons::CIRCLE_HALF,
            BadgeKind::Success => icons::CHECK,
            BadgeKind::Warning => icons::WARNING,
            BadgeKind::Danger => icons::X_CIRCLE,
            BadgeKind::Amber => icons::SPARKLE,
        }
    }
}

/// Pill badge (22 pt) with the tone's default icon.
pub fn badge(ui: &mut Ui, kind: BadgeKind, label: impl Into<String>) -> Response {
    badge_icon(ui, kind, Some(kind.default_icon()), label)
}

/// Pill badge with an explicit icon (or none).
pub fn badge_icon(
    ui: &mut Ui,
    kind: BadgeKind,
    icon: Option<&str>,
    label: impl Into<String>,
) -> Response {
    let c = *colors(ui);
    let tone = kind.color(&c);
    let label = label.into();
    let galley = ui
        .painter()
        .layout_no_wrap(label.clone(), fonts::medium(12.0), tone);
    let icon_w = if icon.is_some() { 12.0 + 4.0 } else { 0.0 };
    let left = if icon.is_some() { 6.0 } else { 8.0 };
    let size = vec2(left + icon_w + galley.size().x + 8.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect(
            rect,
            radius(11.0),
            tone.gamma_multiply(0.12),
            Stroke::new(1.0, tone),
            StrokeKind::Inside,
        );
        let mut x = rect.left() + left;
        if let Some(i) = icon {
            p.text(
                pos2(x + 6.0, rect.center().y),
                Align2::CENTER_CENTER,
                i,
                fonts::icon(12.0),
                tone,
            );
            x += icon_w;
        }
        p.galley(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            tone,
        );
    }
    response
}

/// Small filled status dot (8 pt). Pair it with a word: colour never
/// carries meaning alone.
pub fn status_dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    response
}

// ---------------------------------------------------------------- layout

/// Section title (Title/M) with an optional caption next to it (Body/M,
/// secondary), as "Modules  1 installed, 4 on the way".
pub fn section_header(ui: &mut Ui, title: impl Into<String>, caption: Option<&str>) -> Response {
    let c = *colors(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let t = ui.label(
            egui::RichText::new(title.into())
                .font(fonts::style(&tokens().text.title_m))
                .color(c.text_primary),
        );
        if let Some(cap) = caption {
            ui.label(
                egui::RichText::new(cap)
                    .font(fonts::ui(13.0))
                    .color(c.text_secondary),
            );
        }
        t
    })
    .inner
}

/// Card frame: `bg_panel`, 1 pt `border_subtle`, radius 12, padding 20.
pub fn card_frame(ui: &Ui) -> Frame {
    let c = colors(ui);
    Frame::new()
        .fill(c.bg_panel)
        .stroke(Stroke::new(tokens().stroke.hairline, c.border_subtle))
        .corner_radius(radius(tokens().radius.lg))
        .inner_margin(Margin::same(20))
}

/// Sunken block (licence block, info boxes): `bg_sunken`, radius 8, padding 12.
pub fn sunken_frame(ui: &Ui) -> Frame {
    let c = colors(ui);
    Frame::new()
        .fill(c.bg_sunken)
        .corner_radius(radius(tokens().radius.md))
        .inner_margin(Margin::same(12))
}

/// Vertical 1 pt divider `height` tall.
pub fn divider_v(ui: &mut Ui, height: f32) -> Response {
    let c = *colors(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0, c.border_subtle),
    );
    response
}

/// Horizontal 1 pt hairline over the available width.
pub fn hairline(ui: &mut Ui) -> Response {
    let c = *colors(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0, c.border_subtle),
    );
    response
}

// ---------------------------------------------------------------- toggles

/// Width of [`toggle`] with this label, to reserve room for it.
pub fn toggle_width(ui: &Ui, label: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        fonts::ui(13.0),
        egui::Color32::PLACEHOLDER,
    );
    let gap = if label.is_empty() { 0.0 } else { 8.0 };
    36.0 + gap + galley.size().x
}

/// Toggle switch (track 36 x 20, knob 14) with a label on its right.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: impl Into<String>) -> Response {
    let c = *colors(ui);
    let label = label.into();
    let galley = ui
        .painter()
        .layout_no_wrap(label.clone(), fonts::ui(13.0), c.text_primary);
    let gap = if label.is_empty() { 0.0 } else { 8.0 };
    let size = vec2(36.0 + gap + galley.size().x, 24.0_f32.max(galley.size().y));
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let value = *on;
    response.widget_info(|| WidgetInfo::selected(Role::Switch, true, value, &label));
    if ui.is_rect_visible(rect) {
        let track =
            Rect::from_min_size(pos2(rect.left(), rect.center().y - 10.0), vec2(36.0, 20.0));
        let t = ui.ctx().animate_bool(response.id, *on);
        let p = ui.painter();
        let (fill, stroke) = if *on {
            (c.select_blue, Stroke::NONE)
        } else {
            (c.bg_sunken, Stroke::new(1.0, c.border_strong))
        };
        p.rect(track, radius(10.0), fill, stroke, StrokeKind::Inside);
        let x = egui::lerp(track.left() + 10.0..=track.right() - 10.0, t);
        let knob = if *on { c.bg_raised } else { c.text_secondary };
        p.circle_filled(pos2(x, track.center().y), 7.0, knob);
        focus_ring(ui, &response, track, radius(10.0));
        p.galley(
            pos2(track.right() + gap, rect.center().y - galley.size().y / 2.0),
            galley,
            c.text_primary,
        );
    }
    response
}

/// Checkbox (18 pt box, check icon) with a label.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, label: impl Into<String>) -> Response {
    let c = *colors(ui);
    let label = label.into();
    let galley = ui.painter().layout(
        label.clone(),
        fonts::semibold(13.0),
        c.text_primary,
        (ui.available_width() - 26.0).max(40.0),
    );
    let size = vec2(26.0 + galley.size().x, 20.0_f32.max(galley.size().y));
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let value = *checked;
    response.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, value, &label));
    let bx = Rect::from_min_size(pos2(rect.left(), rect.top() + 1.0), Vec2::splat(18.0));
    let p = ui.painter();
    if *checked {
        p.rect_filled(bx, radius(4.0), c.select_blue);
        p.text(
            bx.center(),
            Align2::CENTER_CENTER,
            crate::icons::CHECK,
            fonts::icon(13.0),
            c.bg_raised,
        );
    } else {
        p.rect(
            bx,
            radius(4.0),
            c.bg_sunken,
            Stroke::new(1.0, c.border_strong),
            StrokeKind::Inside,
        );
    }
    focus_ring(ui, &response, bx, radius(4.0));
    p.galley(pos2(bx.right() + 8.0, rect.top()), galley, c.text_primary);
    response
}

/// Radio indicator (16 pt ring, blue dot when selected); returns the
/// response of the indicator only, for use inside larger clickable rows.
pub fn radio_indicator(ui: &mut Ui, selected: bool, enabled: bool) -> Response {
    let c = *colors(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
    let p = ui.painter();
    let ring = if !enabled {
        c.text_disabled
    } else if selected {
        c.select_blue
    } else {
        c.border_strong
    };
    p.circle_stroke(rect.center(), 8.0, Stroke::new(1.5, ring));
    if selected {
        p.circle_filled(rect.center(), 4.5, c.select_blue);
    }
    response
}

// ---------------------------------------------------------------- tabs, segmented

/// Tab bar (36 pt tabs, padding 0/14). Selected = 2 pt `select_blue`
/// underline + SemiBold primary text. Returns the bar's response, `changed()`
/// when the selection changed.
pub fn tab_bar<S: AsRef<str>>(ui: &mut Ui, selected: &mut usize, labels: &[S]) -> Response {
    let c = *colors(ui);
    let mut changed = false;
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, l) in labels.iter().enumerate() {
            let l = l.as_ref();
            let is_sel = *selected == i;
            let font = if is_sel {
                fonts::semibold(13.0)
            } else {
                fonts::ui(13.0)
            };
            let galley = ui
                .painter()
                .layout_no_wrap(l.to_owned(), font, Color32::PLACEHOLDER);
            let (rect, resp) =
                ui.allocate_exact_size(vec2(galley.size().x + 28.0, 36.0), Sense::click());
            resp.widget_info(|| WidgetInfo::selected(Role::Tab, true, is_sel, l));
            if resp.clicked() && !is_sel {
                *selected = i;
                changed = true;
            }
            let p = ui.painter();
            if resp.hovered() && !is_sel {
                p.rect_filled(rect, radius(4.0), c.bg_hover);
            }
            let color = if is_sel {
                c.text_primary
            } else {
                c.text_secondary
            };
            p.galley(
                pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            if is_sel {
                p.line_segment(
                    [
                        rect.left_bottom() - vec2(0.0, 1.0),
                        rect.right_bottom() - vec2(0.0, 1.0),
                    ],
                    Stroke::new(2.0, c.select_blue),
                );
            }
            focus_ring(ui, &resp, rect.shrink(2.0), radius(4.0));
        }
    });
    let bar = inner.response.rect;
    ui.painter().line_segment(
        [bar.left_bottom(), pos2(ui.max_rect().right(), bar.bottom())],
        Stroke::new(1.0, c.border_subtle),
    );
    let mut response = inner.response;
    if changed {
        response.mark_changed();
    }
    response
}

/// Segmented control (e.g. EN | IT, Upper | Lower | Both): a sunken track
/// with the active item on `bg_hover` + Bold. `height` 28 in the top bar,
/// 36 in forms.
pub fn segmented<S: AsRef<str>>(
    ui: &mut Ui,
    selected: &mut usize,
    labels: &[S],
    height: f32,
) -> Response {
    segmented_impl(ui, selected, labels, height, None)
}

/// [`segmented`] spanning the available width with equal items (form
/// fields such as Upper | Lower | Both jaws).
pub fn segmented_fill<S: AsRef<str>>(
    ui: &mut Ui,
    selected: &mut usize,
    labels: &[S],
    height: f32,
) -> Response {
    let w = ui.available_width();
    segmented_impl(ui, selected, labels, height, Some(w))
}

fn segmented_impl<S: AsRef<str>>(
    ui: &mut Ui,
    selected: &mut usize,
    labels: &[S],
    height: f32,
    fill_width: Option<f32>,
) -> Response {
    let c = *colors(ui);
    let mut changed = false;
    let pad = 2.0;
    let galleys: Vec<_> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let font = if i == *selected {
                fonts::bold(12.0)
            } else {
                fonts::medium(12.0)
            };
            ui.painter()
                .layout_no_wrap(l.as_ref().to_owned(), font, Color32::PLACEHOLDER)
        })
        .collect();
    let item_w: Vec<f32> = match fill_width {
        Some(w) if !labels.is_empty() => {
            vec![((w - 2.0 * pad) / labels.len() as f32).max(1.0); labels.len()]
        }
        _ => galleys.iter().map(|g| g.size().x + 20.0).collect(),
    };
    let total = item_w.iter().sum::<f32>() + 2.0 * pad;
    let (rect, mut response) = ui.allocate_exact_size(vec2(total, height), Sense::hover());
    ui.painter().rect(
        rect,
        radius(4.0),
        c.bg_sunken,
        Stroke::new(1.0, c.border_subtle),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + pad;
    for (i, (g, w)) in galleys.into_iter().zip(item_w).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + pad), vec2(w, height - 2.0 * pad));
        x += w;
        let id = response.id.with(i);
        let resp = ui.interact(r, id, Sense::click());
        let is_sel = i == *selected;
        let label = labels[i].as_ref().to_owned();
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, is_sel, &label));
        if resp.clicked() && !is_sel {
            *selected = i;
            changed = true;
        }
        let p = ui.painter();
        if is_sel {
            p.rect_filled(r, radius(3.0), c.bg_hover);
        } else if resp.hovered() {
            p.rect_filled(r, radius(3.0), mix(c.bg_sunken, c.bg_hover, 0.5));
        }
        let color = if is_sel {
            c.text_primary
        } else {
            c.text_secondary
        };
        p.galley(r.center() - g.size() / 2.0, g, color);
        focus_ring(ui, &resp, r, radius(3.0));
    }
    if changed {
        response.mark_changed();
    }
    response
}

// ---------------------------------------------------------------- inputs

/// Labelled single-line input (label 12 Medium, field 36 tall on
/// `bg_sunken`, helper below; `error` replaces the helper in danger colour
/// with a warning-circle icon and a 2 pt danger border).
pub fn text_input(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    helper: Option<&str>,
    error: Option<&str>,
) -> Response {
    let hint = placeholder(ui, hint);
    labeled_field(ui, label, helper, error, |ui| {
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .font(fonts::ui(13.0))
            .margin(Margin::symmetric(10, 9))
            .desired_width(f32::INFINITY)
            .show(ui)
            .response
            .response
    })
}

/// Labelled multi-line input `height` tall.
pub fn text_area(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    height: f32,
) -> Response {
    let hint = placeholder(ui, hint);
    labeled_field(ui, label, None, None, |ui| {
        let rows = ((height - 18.0) / 19.0).max(1.0) as usize;
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .font(fonts::ui(13.0))
            .margin(Margin::symmetric(10, 9))
            .desired_width(f32::INFINITY)
            .desired_rows(rows)
            .show(ui)
            .response
            .response
    })
}

/// Placeholder text of an input: the disabled colour, so it never reads as a
/// value that was typed.
fn placeholder(ui: &Ui, hint: &str) -> egui::RichText {
    egui::RichText::new(hint)
        .font(fonts::ui(13.0))
        .color(colors(ui).text_disabled)
}

fn labeled_field(
    ui: &mut Ui,
    label: &str,
    helper: Option<&str>,
    error: Option<&str>,
    field: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    let c = *colors(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        if !label.is_empty() {
            ui.label(
                egui::RichText::new(label)
                    .font(fonts::medium(12.0))
                    .color(c.text_secondary),
            );
        }
        let resp = ui
            .scope(|ui| {
                if error.is_some() {
                    let v = ui.visuals_mut();
                    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered] {
                        w.bg_stroke = Stroke::new(2.0, c.status_danger);
                    }
                    v.selection.stroke = Stroke::new(2.0, c.status_danger);
                }
                field(ui)
            })
            .inner;
        if let Some(e) = error {
            ui.label(
                egui::RichText::new(format!("{}  {e}", crate::icons::WARNING_CIRCLE))
                    .font(fonts::ui(12.0))
                    .color(c.status_danger),
            );
        } else if let Some(h) = helper {
            ui.label(
                egui::RichText::new(h)
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        }
        resp
    })
    .inner
}

// ---------------------------------------------------------------- sliders, progress

/// Labelled slider for a length in millimetres (step 0.01 mm, Shift x10,
/// double-click resets to `default`).
pub fn labeled_slider_mm(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
) -> Response {
    labeled_slider(ui, label, value, range, 0.01, 2, "mm", default)
}

/// Labelled slider for an angle in degrees (step 0.1°, Shift x10).
pub fn labeled_slider_deg(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
) -> Response {
    labeled_slider(ui, label, value, range, 0.1, 1, "°", default)
}

/// Generic labelled slider: label row with a mono value box (84 x 26), a
/// 4 pt track filled in `select_blue` and a 14 pt thumb. Drag or click to
/// set, arrow keys step by `step` (Shift x10), double-click resets.
#[allow(clippy::too_many_arguments)]
pub fn labeled_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: f32,
    decimals: usize,
    unit: &str,
    default: f32,
) -> Response {
    let c = *colors(ui);
    let (lo, hi) = (*range.start(), *range.end());
    let width = ui.available_width();
    // label row
    let row = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover()).0;
    let p = ui.painter();
    p.text(
        row.left_center(),
        Align2::LEFT_CENTER,
        label,
        fonts::medium(12.0),
        c.text_secondary,
    );
    let vbox = Rect::from_min_size(pos2(row.right() - 84.0, row.top()), vec2(84.0, 26.0));
    // track
    let (track_rect, mut response) =
        ui.allocate_exact_size(vec2(width, 20.0), Sense::click_and_drag());
    let id = response.id;
    response.widget_info(|| WidgetInfo::labeled(Role::Slider, true, label));
    let old = *value;
    let x0 = track_rect.left() + 7.0;
    let x1 = track_rect.right() - 7.0;
    if let Some(pos) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked())
    {
        let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
        let raw = lo + t * (hi - lo);
        *value = (raw / step).round() * step;
    }
    if response.double_clicked() {
        *value = default;
    }
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    horizontal_arrows: true,
                    ..Default::default()
                },
            )
        });
        let (left, right, shift) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        let s = if shift { step * 10.0 } else { step };
        if left {
            *value -= s;
        }
        if right {
            *value += s;
        }
    }
    *value = value.clamp(lo, hi);
    if (*value - old).abs() > f32::EPSILON {
        response.mark_changed();
    }
    let p = ui.painter();
    p.rect(
        vbox,
        radius(4.0),
        c.bg_sunken,
        Stroke::new(
            if response.has_focus() { 2.0 } else { 1.0 },
            if response.has_focus() {
                c.focus_ring
            } else {
                c.border_strong
            },
        ),
        StrokeKind::Inside,
    );
    p.text(
        vbox.right_center() - vec2(10.0, 0.0),
        Align2::RIGHT_CENTER,
        format!("{:.*} {unit}", decimals, *value),
        fonts::mono(12.0),
        c.text_primary,
    );
    let cy = track_rect.center().y;
    let t = if hi > lo {
        (*value - lo) / (hi - lo)
    } else {
        0.0
    };
    let xv = egui::lerp(x0..=x1, t);
    let rail = Rect::from_min_max(pos2(x0, cy - 2.0), pos2(x1, cy + 2.0));
    p.rect_filled(rail, radius(2.0), c.bg_sunken);
    p.rect_filled(
        Rect::from_min_max(rail.min, pos2(xv, rail.max.y)),
        radius(2.0),
        c.select_blue,
    );
    let thumb = Pos2::new(xv, cy);
    if response.has_focus() {
        p.circle_stroke(thumb, 9.0, Stroke::new(4.0, c.focus_ring));
    }
    p.circle(thumb, 7.0, c.text_primary, Stroke::new(2.0, c.select_blue));
    response
}

/// Determinate (`Some(0..=1)`) or indeterminate (`None`, 120 pt run
/// sliding) 4 pt progress bar over `width`.
pub fn progress_bar(ui: &mut Ui, fraction: Option<f32>, width: f32) -> Response {
    let c = *colors(ui);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, radius(2.0), c.bg_sunken);
    match fraction {
        Some(f) => {
            let w = rect.width() * f.clamp(0.0, 1.0);
            p.rect_filled(
                Rect::from_min_size(rect.min, vec2(w, 4.0)),
                radius(2.0),
                c.select_blue,
            );
        }
        None => {
            let run = 120.0_f32.min(rect.width() / 2.0);
            let time = ui.input(|i| i.time) as f32;
            let span = rect.width() + run;
            let x = rect.left() - run + (time * 160.0) % span;
            let bar = Rect::from_min_max(
                pos2(x.max(rect.left()), rect.top()),
                pos2((x + run).min(rect.right()), rect.bottom()),
            );
            if bar.width() > 0.0 {
                p.rect_filled(bar, radius(2.0), c.select_blue);
            }
            ui.ctx().request_repaint();
        }
    }
    response
}

/// Spinner (circle-notch style) in `select_blue`.
pub fn spinner(ui: &mut Ui, size: f32) -> Response {
    let c = *colors(ui);
    ui.add(egui::Spinner::new().size(size).color(c.select_blue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn widgets_render_and_react() {
        let mut on = false;
        let mut tab = 0usize;
        let mut seg = 0usize;
        let mut mm = 0.25f32;
        let mut text = String::new();
        let mut clicks = 0;
        {
            let mut harness = Harness::builder()
                .with_size(vec2(800.0, 900.0))
                .build_ui(|ui| {
                    crate::theme::install(ui.ctx(), crate::theme::Mode::Dark);
                    if Button::primary("New project")
                        .icon(crate::icons::PLUS)
                        .large()
                        .show(ui)
                        .clicked()
                    {
                        clicks += 1;
                    }
                    secondary_button(ui, "Open project…");
                    ghost_button(ui, "Support");
                    Button::primary("Disabled").enabled(false).show(ui);
                    icon_button(ui, crate::icons::GEAR, "Settings");
                    for k in [
                        BadgeKind::Neutral,
                        BadgeKind::Info,
                        BadgeKind::Success,
                        BadgeKind::Warning,
                        BadgeKind::Danger,
                        BadgeKind::Amber,
                    ] {
                        badge(ui, k, format!("{k:?}"));
                    }
                    status_dot(ui, Color32::GREEN);
                    section_header(ui, "Modules", Some("1 installed"));
                    card_frame(ui).show(ui, |ui| ui.label("card"));
                    toggle(ui, &mut on, "Keep in step");
                    tab_bar(ui, &mut tab, &["Stages", "Per tooth", "Report"]);
                    segmented(ui, &mut seg, &["EN", "IT"], 28.0);
                    labeled_slider_mm(ui, "Translation", &mut mm, 0.0..=1.0, 0.25);
                    text_input(
                        ui,
                        "Case ID",
                        &mut text,
                        "AL-2026-0001",
                        None,
                        Some("Taken"),
                    );
                    progress_bar(ui, Some(0.4), 200.0);
                    link(ui, "Read the release notes");
                });
            harness.run();
            harness.get_by_label("New project").click();
            harness.get_by_label("Keep in step").click();
            harness.get_by_label("Report").click();
            harness.get_by_label("IT").click();
            harness.run();
        }
        assert_eq!(clicks, 1);
        assert!(on);
        assert_eq!(tab, 2);
        assert_eq!(seg, 1);
    }
}
