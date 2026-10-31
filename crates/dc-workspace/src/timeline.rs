//! Stage timeline under the viewport: transport, stage readout, jaw
//! toggles and the row of stage cells with event markers.

use dc_case::CaseState;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets;
use egui::{
    Align2, CornerRadius, Key, Rect, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo, pos2, vec2,
};

use crate::t;

/// One stage cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageCell {
    /// Stage number (0 = initial position).
    pub index: u32,
    /// Interproximal reduction happens at this stage.
    pub ipr: bool,
    /// An attachment is placed or removed at this stage.
    pub attachment: bool,
    /// A movement exceeds the per-stage limits.
    pub warning: bool,
}

/// Stages of the case, read from the Aligners module data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineModel {
    /// Cells 0..=last, in order.
    pub cells: Vec<StageCell>,
    /// Days each aligner is worn.
    pub interval_days: u32,
}

impl TimelineModel {
    /// Reads the stages of `state` (empty without Aligners data or stages).
    pub fn from_state(state: &CaseState) -> Self {
        let Some(al) = state.module_data::<dca_model::AlignersState>(dca_model::MODULE_ID) else {
            return Self::default();
        };
        let Some(last) = al.stages.iter().map(|s| s.index).max() else {
            return Self::default();
        };
        let cells = (0..=last)
            .map(|i| {
                let st = al.stages.iter().find(|s| s.index == i);
                StageCell {
                    index: i,
                    ipr: st.is_some_and(|s| !s.ipr.is_empty()),
                    attachment: false,
                    warning: false,
                }
            })
            .collect();
        Self {
            cells,
            interval_days: al.settings.interval_days,
        }
    }

    /// Last stage number (0 when empty).
    pub fn last(&self) -> u32 {
        self.cells.last().map_or(0, |c| c.index)
    }

    /// `true` without stages.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Week of treatment at which `stage` starts.
    pub fn week(&self, stage: u32) -> u32 {
        stage * self.interval_days / 7
    }
}

/// Timeline UI state.
#[derive(Debug, Clone)]
pub(crate) struct TimelineState {
    pub current: u32,
    pub playing: bool,
    last_step: f64,
    pub upper: bool,
    pub lower: bool,
    pub overlay0: bool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            current: 0,
            playing: false,
            last_step: 0.0,
            upper: true,
            lower: true,
            overlay0: false,
        }
    }
}

impl TimelineState {
    /// Moves by `delta` stages, clamped to `0..=last`.
    pub fn step(&mut self, delta: i64, last: u32) {
        let next = (i64::from(self.current) + delta).clamp(0, i64::from(last));
        self.current = u32::try_from(next).unwrap_or(0);
    }

    /// Advances while playing (one stage every 0.6 s, stops at the end).
    pub fn tick(&mut self, now: f64, last: u32) -> bool {
        if !self.playing {
            return false;
        }
        if now - self.last_step >= 0.6 {
            self.last_step = now;
            if self.current >= last {
                self.playing = false;
            } else {
                self.current += 1;
            }
        }
        true
    }

    /// Starts or stops playing (from the start when at the end).
    pub fn toggle_play(&mut self, now: f64, last: u32) {
        self.playing = !self.playing;
        if self.playing {
            if self.current >= last {
                self.current = 0;
            }
            self.last_step = now;
        }
    }

    /// Keyboard: `,` / `.` previous / next (Shift = 5), Space play,
    /// Home / End first / last.
    pub fn keys(&mut self, ui: &Ui, last: u32) {
        let now = ui.input(|i| i.time);
        let (prev, next, play, home, end, shift) = ui.input(|i| {
            (
                i.key_pressed(Key::Comma),
                i.key_pressed(Key::Period),
                i.key_pressed(Key::Space),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.modifiers.shift,
            )
        });
        let n = if shift { 5 } else { 1 };
        if prev {
            self.step(-n, last);
        }
        if next {
            self.step(n, last);
        }
        if home {
            self.current = 0;
        }
        if end {
            self.current = last;
        }
        if play && last > 0 {
            self.toggle_play(now, last);
        }
    }
}

/// Draws the timeline (112 pt tall area).
pub(crate) fn show(ui: &mut Ui, model: &TimelineModel, st: &mut TimelineState) {
    let c = *colors(ui);
    let last = model.last();
    if st.current > last {
        st.current = last;
    }
    let now = ui.input(|i| i.time);
    if st.tick(now, last) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    let has = !model.is_empty();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let b = |ui: &mut Ui, icon: &str, key: &str| {
            widgets::icon_button_sized(ui, icon, t(key), 36.0, has)
        };
        if b(ui, ic::SKIP_BACK, "ws-tl-first").clicked() {
            st.current = 0;
        }
        if b(ui, ic::CARET_LEFT, "ws-tl-prev").clicked() {
            st.step(-1, last);
        }
        let play_icon = if st.playing { ic::PAUSE } else { ic::PLAY };
        if b(ui, play_icon, "ws-tl-play").clicked() {
            st.toggle_play(now, last);
        }
        if b(ui, ic::CARET_RIGHT, "ws-tl-next").clicked() {
            st.step(1, last);
        }
        if b(ui, ic::SKIP_FORWARD, "ws-tl-last").clicked() {
            st.current = last;
        }
        ui.add_space(16.0);
        if has {
            ui.label(
                egui::RichText::new(t("ws-tl-stage"))
                    .font(fonts::medium(13.0))
                    .color(c.text_secondary),
            );
            ui.label(
                egui::RichText::new(format!("{:02}", st.current))
                    .font(fonts::mono_bold(20.0))
                    .color(c.text_primary),
            );
            ui.label(
                egui::RichText::new(dc_i18n::tr_args(
                    "ws-tl-of-week",
                    &[
                        ("last", &last.to_string()),
                        ("week", &model.week(st.current).to_string()),
                    ],
                ))
                .font(fonts::mono(13.0))
                .color(c.text_secondary),
            );
        } else {
            ui.label(
                egui::RichText::new(t("ws-tl-empty"))
                    .font(fonts::ui(13.0))
                    .color(c.text_secondary),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            widgets::toggle(ui, &mut st.overlay0, t("ws-tl-overlay0"));
            widgets::toggle(ui, &mut st.lower, t("ws-tl-lower"));
            widgets::toggle(ui, &mut st.upper, t("ws-tl-upper"));
        });
    });
    ui.add_space(6.0);
    if has {
        cells(ui, model, st);
    } else {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        ui.painter().rect(
            rect,
            CornerRadius::same(3),
            c.bg_sunken,
            Stroke::new(1.0, c.border_subtle),
            StrokeKind::Inside,
        );
    }
}

fn cells(ui: &mut Ui, model: &TimelineModel, st: &mut TimelineState) {
    let c = *colors(ui);
    let n = model.cells.len().max(1) as f32;
    let gap = 4.0;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 52.0), Sense::click_and_drag());
    let cell_w = ((w - gap * (n - 1.0)) / n).max(2.0);
    let label = format!("{} {:02}", t("ws-tl-stage"), st.current);
    resp.widget_info(|| WidgetInfo::labeled(Role::Slider, true, &label));
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let i = ((pos.x - rect.left()) / (cell_w + gap))
            .floor()
            .clamp(0.0, n - 1.0);
        st.current = model.cells[i as usize].index;
        st.playing = false;
    }
    let p = ui.painter();
    let bar_top = rect.top() + 16.0;
    for (k, cell) in model.cells.iter().enumerate() {
        let x = rect.left() + k as f32 * (cell_w + gap);
        let r = Rect::from_min_size(pos2(x, bar_top), vec2(cell_w, 20.0));
        let current = cell.index == st.current;
        let (fill, stroke) = if current {
            (c.select_blue, Stroke::NONE)
        } else if cell.index < st.current {
            (c.bg_hover, Stroke::new(1.0, c.border_strong))
        } else {
            (c.bg_sunken, Stroke::new(1.0, c.border_subtle))
        };
        let stroke = if cell.warning {
            Stroke::new(2.0, c.status_warning)
        } else {
            stroke
        };
        p.rect(r, CornerRadius::same(3), fill, stroke, StrokeKind::Inside);
        // markers above the cell
        let my = rect.top() + 6.0;
        let mut mx = r.center().x;
        let markers = [
            (cell.attachment, ic::SQUARE, c.model_attachment),
            (cell.ipr, ic::DIAMOND, c.model_ipr),
            (cell.warning, ic::WARNING, c.status_warning),
        ];
        let count = markers.iter().filter(|m| m.0).count() as f32;
        mx -= (count - 1.0).max(0.0) * 6.0;
        for (on, glyph, color) in markers {
            if on {
                p.text(
                    pos2(mx, my),
                    Align2::CENTER_CENTER,
                    glyph,
                    fonts::icon(11.0),
                    color,
                );
                mx += 12.0;
            }
        }
        let font = if current {
            fonts::mono_bold(11.0)
        } else {
            fonts::mono(11.0)
        };
        p.text(
            pos2(r.center().x, r.bottom() + 8.0),
            Align2::CENTER_CENTER,
            format!("{:02}", cell.index),
            font,
            if current {
                c.text_primary
            } else {
                c.text_secondary
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::{CaseState, Fdi, ModuleState};
    use dca_model::{AlignersState, IprStep, Stage};

    #[test]
    fn model_from_aligners_stages() {
        assert!(TimelineModel::from_state(&CaseState::default()).is_empty());
        let mut al = AlignersState::default();
        al.stages.push(Stage::new(0));
        al.stages.push(Stage::new(2));
        al.stages[1].ipr.push(IprStep {
            between: [Fdi::new(11).unwrap(), Fdi::new(21).unwrap()],
            amount_mm: 0.2,
        });
        let mut st = CaseState::default();
        st.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let m = TimelineModel::from_state(&st);
        assert_eq!(m.last(), 2);
        assert_eq!(m.cells.len(), 3);
        assert!(m.cells[2].ipr && !m.cells[1].ipr);
        assert_eq!(m.week(2), 2 * m.interval_days / 7);
    }

    #[test]
    fn stepping_and_playing() {
        let mut s = TimelineState::default();
        s.step(3, 5);
        assert_eq!(s.current, 3);
        s.step(10, 5);
        assert_eq!(s.current, 5);
        s.step(-50, 5);
        assert_eq!(s.current, 0);
        s.toggle_play(0.0, 2);
        assert!(s.tick(1.0, 2));
        assert_eq!(s.current, 1);
        s.tick(2.0, 2);
        s.tick(3.0, 2);
        assert!(!s.playing);
        assert_eq!(s.current, 2);
    }
}
