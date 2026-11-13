//! Stage timeline under the viewport: transport, stage readout, the overlay
//! toggle and the stage slider.

use dc_case::CaseState;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets;
use egui::{Key, Rect, Ui, vec2};

use crate::t;

/// Stages of the case, read from the Aligners module data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineModel {
    /// Last stage number; `None` without stages.
    last: Option<u32>,
    /// Days each aligner is worn.
    pub interval_days: u32,
}

impl TimelineModel {
    /// Reads the stages of `state` (empty without Aligners data or stages).
    pub fn from_state(state: &CaseState) -> Self {
        let Some(al) = state.module_data::<dca_model::AlignersState>(dca_model::MODULE_ID) else {
            return Self::default();
        };
        Self {
            last: al.stages.iter().map(|s| s.index).max(),
            interval_days: al.settings.interval_days,
        }
    }

    /// Last stage number (0 when empty).
    pub fn last(&self) -> u32 {
        self.last.unwrap_or(0)
    }

    /// `true` without stages.
    pub fn is_empty(&self) -> bool {
        self.last.is_none()
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
    pub overlay0: bool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            current: 0,
            playing: false,
            last_step: 0.0,
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

/// Width of the transport buttons and the space after them.
const TRANSPORT_WIDTH: f32 = 5.0 * 36.0 + 4.0 * 4.0 + 16.0;
/// The least room beside the transport that is worth a hint.
const HINT_MIN_WIDTH: f32 = 160.0;

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

    // The overlay toggle keeps its own room at the right (the widest label
    // is the Italian one); the transport and the readout or hint share what
    // is left, and the hint is cut with an ellipsis rather than run under it.
    // (The arch toggles live in the viewport toolbar.)
    let toggle_gap = 16.0;
    let toggles_w = widgets::toggle_width(ui, &t("ws-tl-overlay0"));
    let row_h = 36.0;
    let mut hint_below = false;
    ui.horizontal(|ui| {
        let left_w = (ui.available_width() - toggles_w - toggle_gap).max(0.0);
        // too little room beside the transport for the hint: it goes in the
        // empty bar under the row instead
        hint_below = !has && left_w - TRANSPORT_WIDTH < HINT_MIN_WIDTH;
        ui.allocate_ui_with_layout(
            vec2(left_w, row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                transport_and_readout(ui, model, st, has, hint_below, last, now);
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = toggle_gap;
            widgets::toggle(ui, &mut st.overlay0, t("ws-tl-overlay0"));
        });
    });
    ui.add_space(6.0);
    if has {
        track(ui, model, st);
    } else {
        // no stages: the slider is there but off, with the hint over its
        // empty number row when the header had no room for it
        let resp = widgets::stage_slider(ui, &mut 0, 0, &t("ws-tl-stage"));
        if hint_below {
            let hint = t("ws-tl-empty");
            let row = Rect::from_min_size(
                resp.rect.left_top() - vec2(0.0, widgets::STAGE_SLIDER_LABEL_ROW),
                vec2(resp.rect.width(), widgets::STAGE_SLIDER_LABEL_ROW),
            );
            ui.put(
                row,
                egui::Label::new(
                    egui::RichText::new(hint.clone())
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                )
                .truncate(),
            )
            .on_hover_text(hint);
        }
    }
}

/// The left side of the timeline header: transport buttons, then the stage
/// readout or, without stages, the hint (which gives way when short of room).
fn transport_and_readout(
    ui: &mut Ui,
    model: &TimelineModel,
    st: &mut TimelineState,
    has: bool,
    hint_below: bool,
    last: u32,
    now: f64,
) {
    let c = *colors(ui);
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
    } else if !hint_below {
        let hint = t("ws-tl-empty");
        ui.add(
            egui::Label::new(
                egui::RichText::new(hint.clone())
                    .font(fonts::ui(13.0))
                    .color(c.text_secondary),
            )
            .truncate(),
        )
        .on_hover_text(hint);
    }
}

/// The stage slider: a plain track over `0..=last`.
fn track(ui: &mut Ui, model: &TimelineModel, st: &mut TimelineState) {
    let label = format!("{} {:02}", t("ws-tl-stage"), st.current);
    let mut stage = st.current;
    if widgets::stage_slider(ui, &mut stage, model.last(), &label).changed() {
        st.current = stage;
        st.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::{CaseState, ModuleState};
    use dca_model::{AlignersState, Stage};

    #[test]
    fn model_from_aligners_stages() {
        assert!(TimelineModel::from_state(&CaseState::default()).is_empty());
        let mut al = AlignersState::default();
        al.stages.push(Stage::new(0));
        al.stages.push(Stage::new(2));
        let mut st = CaseState::default();
        st.set_module(dca_model::MODULE_ID, ModuleState::new(al));
        let m = TimelineModel::from_state(&st);
        assert!(!m.is_empty());
        assert_eq!(m.last(), 2);
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
