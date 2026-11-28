//! The inspector of "Aligners", as the plugin's page (`ui_phases.py`
//! `page_allineatori`) has it:
//!
//! - under the status, the result of the last aligners of each arch: how
//!   many, the days and months of the calendar with its first and last day,
//!   the pattern used and the complexity, the slowed teeth and the
//!   overcorrection (`AlignersState::staging_results`, the calendar built
//!   from the stored aligners with the calendar settings of the case, as the
//!   plugin's `key_numbers` builds it);
//! - the collisions the aligners leave, one line each (aligner, teeth,
//!   penetration and what is over the allowance) in a warning block
//!   (`AlignersState::staging_collisions`, kept in the case: shown after a
//!   reopen too);
//! - "Parameters": the periodontal condition, the pattern and the calendar
//!   (how the aligner changes, the days of each, the start date), then the
//!   largest movement per aligner in a group of its own, as on the plugin's
//!   page; a change runs `aligners.settings.staging` (one undo step), never
//!   the computation;
//! - "Manual corrections": "Slow a tooth" on the tooth selected in the view
//!   (`aligners.planning.tooth-speed`: the aligners of its arch are computed
//!   again in the same undo step, as the plugin's `oa.tooth_speed` does).
//!
//! The mapping from the case to the lines and to the parameters of the
//! commands is pure and tested without a GPU.

use dc_case::{Fdi, ScanRole};
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, Button};
use dca_algo::anatomy::Arch;
use dca_algo::schedule::{self, Date};
use dca_model::{AlignersState, INTERVAL_DAYS, IntervalPolicy, StagingSettings, TOOTH_SPEED};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::function::caption;
use crate::plan_warnings::{collision_blocks, show_blocks};
use crate::select::Component;
use crate::setup::{
    Choice, Number, ResultLine, choice, flag, num, number, show_result_lines, slider, tr,
};
use crate::t;
use crate::workspace::Workspace;

/// The command that changes the parameters of the case.
pub(crate) const SETTINGS: &str = "aligners.settings.staging";
/// "Slow a tooth".
pub(crate) const TOOTH_SPEED_CMD: &str = "aligners.planning.tooth-speed";

const PERIO: Choice = Choice {
    field: "perio",
    key: "ws-stg-perio",
    options: &[
        ("healthy", "ws-stg-perio-healthy"),
        ("stage1", "ws-stg-perio-stage1"),
        ("stage2", "ws-stg-perio-stage2"),
        ("stage3", "ws-stg-perio-stage3"),
    ],
};

const PATTERN: Choice = Choice {
    field: "pattern",
    key: "ws-stg-pattern",
    options: &[
        ("auto", "ws-stg-pattern-auto"),
        ("proportional", "ws-stg-pattern-proportional"),
        ("max_velocity", "ws-stg-pattern-max-velocity"),
        ("sequential_distal", "ws-stg-pattern-sequential-distal"),
        ("structured", "ws-stg-pattern-structured"),
    ],
};

const POLICY: Choice = Choice {
    field: "interval_policy",
    key: "ws-stg-policy",
    options: &[
        ("fixed", "ws-stg-policy-fixed"),
        ("adaptive", "ws-stg-policy-adaptive"),
    ],
};

/// Days each aligner is worn with the fixed calendar.
const DAYS: Number = number("interval_days", "ws-stg-days", 1.0, 0, "");

/// The largest movement per aligner, in the order of the plugin's group
/// ("Movimento massimo per allineatore").
const LIMITS: [Number; 6] = [
    number("step_linear_mm", "ws-stg-step-linear", 0.01, 2, "mm"),
    number("step_vertical_mm", "ws-stg-step-vertical", 0.01, 2, "mm"),
    number("step_rotation_deg", "ws-stg-step-rotation", 0.05, 2, "°"),
    number(
        "step_rotation_round_deg",
        "ws-stg-step-rotation-round",
        0.05,
        2,
        "°",
    ),
    number("step_angular_deg", "ws-stg-step-angular", 0.05, 2, "°"),
    number("max_crown_step_mm", "ws-stg-max-crown-step", 0.01, 2, "mm"),
];
const COLLISION_TOL: Number = number("collision_tol_mm", "ws-stg-collision-tol", 0.01, 2, "mm");

/// The speed of a slowed tooth, as the plugin's dialog shows it.
const SPEED_DEFAULT: f32 = 0.5;

/// What the inspector keeps between frames: the slider being dragged, the
/// start date being typed and the speed chosen for the selected tooth.
#[derive(Debug, Default)]
pub(crate) struct StagingUi {
    drag: Option<(&'static str, f32)>,
    date: Option<String>,
    speed: Option<(Fdi, f32)>,
}

/// The range and the default of the numeric setting `field`.
fn range_of(field: &str) -> (f32, f32, f32) {
    if field == DAYS.field {
        let (lo, hi) = INTERVAL_DAYS;
        return (lo as f32, hi as f32, 14.0);
    }
    let (_, lo, hi) = StagingSettings::NUMBERS
        .iter()
        .find(|(f, _, _)| *f == field)
        .copied()
        .unwrap_or(("", 0.0, 1.0));
    let default = StagingSettings::default().number(field).unwrap_or(lo);
    (lo as f32, hi as f32, default as f32)
}

/// The words of the arch.
fn arch_of(role: ScanRole) -> Option<Arch> {
    match role {
        ScanRole::Upper => Some(Arch::Upper),
        ScanRole::Lower => Some(Arch::Lower),
        ScanRole::Bite => None,
    }
}

/// The key of a value of a choice (the name the case stores).
fn option_key(c: &Choice, name: &str) -> String {
    c.options
        .iter()
        .find(|(n, _)| *n == name)
        .map_or_else(|| name.to_owned(), |(_, key)| t(key))
}

/// The calendar of arch `arch` from the stored aligners and the calendar
/// settings of the case (no start date: `today`), with the attachments of
/// the clinical review bonded before the first aligner, as the plugin's
/// page builds it (`schedule.build` of the arch alone).
pub(crate) fn calendar(
    data: &AlignersState,
    arch: Arch,
    today: Date,
) -> Option<schedule::Calendar> {
    let input = schedule::input_from_stages(arch, &data.stages, data.attachment_teeth(arch.role()));
    if input.n_stages == 0 {
        return None;
    }
    let s = &data.settings;
    schedule::run(
        &input,
        &schedule::Params {
            start_date: Some(s.start_date.clone().unwrap_or_else(|| today.to_iso())),
            interval_days: s.interval_days,
            policy: s.interval_policy.into(),
            wear_hours: s.wear_hours,
            ..schedule::Params::default()
        },
    )
    .ok()
}

/// The slowed teeth and their speed, `31 (0.50), 41 (0.30)`.
fn slowed_list(teeth: impl Iterator<Item = (Fdi, f64)>) -> String {
    teeth
        .map(|(f, s)| format!("{f} ({})", num(s, 2)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The result of the last aligners of each arch, as the plugin's page words
/// it: the aligners, the days and months, the first and last day with the
/// hours of wear, the pattern and the complexity, the slowed teeth, the
/// overcorrection. The collisions left are a warning: a count line only for
/// an arch whose case holds no list of them (a case saved before the list
/// was kept), otherwise the list is shown by [`collision_blocks`]. The arch
/// is named when both have a result. Nothing when no arch has one. Without
/// `advanced` (the advanced options off) the pattern, the slowed teeth and
/// the overcorrection are left out.
pub(crate) fn result_lines(data: &AlignersState, today: Date, advanced: bool) -> Vec<ResultLine> {
    let results = &data.staging_results;
    let both = results.len() > 1;
    let mut out = Vec::new();
    let line = |text: String, warning: bool| ResultLine {
        text,
        heading: false,
        warning,
    };
    for (role, r) in results {
        let Some(arch) = arch_of(*role) else { continue };
        if both {
            out.push(ResultLine {
                text: t(if arch.is_upper() {
                    "ws-setup-result-upper"
                } else {
                    "ws-setup-result-lower"
                }),
                heading: true,
                warning: false,
            });
        }
        out.push(line(
            tr("ws-stg-result-aligners", &[("n", &r.aligners.to_string())]),
            false,
        ));
        if let Some(cal) = calendar(data, arch, today) {
            out.push(line(
                tr(
                    "ws-stg-result-days",
                    &[
                        ("days", &cal.total_days.to_string()),
                        ("months", &num(f64::from(cal.total_days) / 30.44, 1)),
                    ],
                ),
                false,
            ));
            out.push(line(
                tr(
                    "ws-stg-result-dates",
                    &[
                        ("start", &cal.start.dmy()),
                        ("end", &cal.end.dmy()),
                        ("hours", &num(cal.wear_hours, 0)),
                    ],
                ),
                false,
            ));
        }
        let pattern = json!(r.pattern);
        let level = format!("ws-stg-level-{}", r.level.replace('_', "-"));
        if advanced {
            out.push(line(
                tr(
                    "ws-stg-result-pattern",
                    &[
                        (
                            "pattern",
                            &option_key(&PATTERN, pattern.as_str().unwrap_or_default()),
                        ),
                        ("level", &t(&level)),
                    ],
                ),
                false,
            ));
        }
        if advanced && !r.slowed.is_empty() {
            out.push(line(
                tr(
                    "ws-stg-result-slowed",
                    &[(
                        "teeth",
                        &slowed_list(r.slowed.iter().map(|(f, s)| (*f, *s))),
                    )],
                ),
                false,
            ));
        }
        if r.collisions_left > 0 && data.staging_collisions.get(role).is_none_or(Vec::is_empty) {
            out.push(line(
                tr(
                    "ws-stg-result-collisions",
                    &[("n", &r.collisions_left.to_string())],
                ),
                true,
            ));
        }
        if advanced && r.overcorrection {
            out.push(line(t("ws-stg-result-overcorrection"), false));
        }
    }
    out
}

/// The parameters of `aligners.planning.tooth-speed` that slow tooth `f` to
/// `speed`, rounded to two decimals as the case keeps it; the normal speed
/// (1) gives `null`.
pub(crate) fn speed_patch(f: Fdi, speed: f32) -> Value {
    let v = (f64::from(speed) * 100.0).round() / 100.0;
    if v >= 0.999 {
        json!({ "fdi": f.get(), "speed": null })
    } else {
        json!({ "fdi": f.get(), "speed": v })
    }
}

/// A title of a group of parameters.
fn group_title(ui: &mut Ui, key: &str) {
    let c = *colors(ui);
    ui.add_space(6.0);
    ui.label(
        RichText::new(t(key))
            .font(fonts::semibold(12.0))
            .color(c.text_primary),
    );
    ui.add_space(2.0);
}

impl Workspace {
    /// The result of the last aligners, under the status.
    pub(crate) fn staging_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let Some(data) = crate::overlay::aligners(&state) else {
            return;
        };
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        show_result_lines(ui, result_lines(data, Date::today(), advanced));
        show_blocks(ui, collision_blocks(data));
        show_blocks(ui, self.plan_warnings.staging_blocks(data));
    }

    /// The parameters of the aligners and of their calendar, read from the
    /// case settings: a change is sent when it is made (a slider when it is
    /// let go, the date when the field is left), one undo step each; the
    /// aligners are computed again only by the run button. With the advanced
    /// options off only the calendar shows.
    pub(crate) fn staging_params(&mut self, ui: &mut Ui) {
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        let state = self.undo.current().clone();
        let settings = crate::overlay::aligners(&state)
            .map(|d| d.settings.clone())
            .unwrap_or_default();
        let s = settings.staging;
        let mut values = serde_json::to_value(s).unwrap_or_default();
        values["interval_policy"] = json!(settings.interval_policy);
        let busy = self.is_busy();
        let mut commit = None;
        let ui_state = &mut self.staging_ui;
        ui.add_enabled_ui(!busy, |ui| {
            if advanced {
                choice(ui, &values, &PERIO, &mut commit);
                choice(ui, &values, &PATTERN, &mut commit);
            }
            group_title(ui, "ws-stg-group-calendar");
            choice(ui, &values, &POLICY, &mut commit);
            if settings.interval_policy == IntervalPolicy::Fixed {
                slider(
                    ui,
                    f64::from(settings.interval_days),
                    range_of(DAYS.field),
                    &DAYS,
                    &mut ui_state.drag,
                    &mut commit,
                );
            } else {
                caption(ui, t("ws-stg-policy-adaptive-hint"));
            }
            let stored = settings.start_date.clone().unwrap_or_default();
            let mut text = ui_state.date.clone().unwrap_or_else(|| stored.clone());
            let r = widgets::text_input(
                ui,
                &t("ws-stg-start"),
                &mut text,
                &t("ws-stg-start-hint"),
                Some(&t("ws-stg-start-help")),
                None,
            );
            if r.lost_focus() {
                ui_state.date = None;
                if text.trim() != stored {
                    commit = Some(json!({ "start_date": text.trim() }));
                }
            } else if r.has_focus() {
                ui_state.date = Some(text);
            } else {
                ui_state.date = None;
            }
            if !advanced {
                return;
            }
            ui.add_space(6.0);
            egui::CollapsingHeader::new(
                RichText::new(t("ws-stg-group-limits")).font(fonts::semibold(12.0)),
            )
            .id_salt("ws-stg-group-limits")
            .default_open(false)
            .show(ui, |ui| {
                for n in &LIMITS {
                    let now = s.number(n.field).unwrap_or_default();
                    slider(
                        ui,
                        now,
                        range_of(n.field),
                        n,
                        &mut ui_state.drag,
                        &mut commit,
                    );
                }
                flag(
                    ui,
                    &values,
                    "collision_check",
                    "ws-stg-collision",
                    &mut commit,
                );
                slider(
                    ui,
                    s.collision_tol_mm,
                    range_of(COLLISION_TOL.field),
                    &COLLISION_TOL,
                    &mut ui_state.drag,
                    &mut commit,
                );
                flag(
                    ui,
                    &values,
                    "overcorrection",
                    "ws-stg-overcorrection",
                    &mut commit,
                );
            });
        });
        if advanced {
            ui.add_space(6.0);
            caption(ui, t("ws-stg-params-rerun"));
        }
        if let Some(p) = commit {
            self.run_module_command(SETTINGS, p);
        }
    }

    /// Runs the registered command `id` with `params`.
    pub(crate) fn run_module_command(&mut self, id: &str, params: Value) {
        if let Some(spec) = self.specs.iter().find(|s| s.id == id) {
            let cmd = spec.command(params);
            self.run_command(Box::new(cmd));
        }
    }

    /// "Slow a tooth" on the tooth selected in the view: its speed, the
    /// button that slows it (the aligners of its arch are computed again)
    /// and the one that gives it back its normal speed; the teeth slowed.
    pub(crate) fn staging_corrections(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let data = crate::overlay::aligners(&state);
        let c = *colors(ui);
        ui.label(
            RichText::new(t("ws-stg-slow"))
                .font(fonts::semibold(13.0))
                .color(c.text_primary),
        );
        caption(ui, t("ws-stg-slow-hint"));
        ui.add_space(6.0);
        let selected = match self.selection.component {
            Some(Component::Tooth(n)) => Fdi::new(n)
                .ok()
                .filter(|f| data.is_some_and(|d| d.tooth_frames.contains_key(f))),
            _ => None,
        };
        let busy = self.is_busy();
        let mut send = None;
        match selected {
            None => caption(ui, t("ws-stg-slow-pick")),
            Some(f) => {
                let stored = data.and_then(|d| d.tooth_speed.get(&f).copied());
                ui.label(
                    RichText::new(tr("ws-sel-tooth", &[("fdi", &f.to_string())]))
                        .font(fonts::medium(12.0))
                        .color(c.text_primary),
                );
                caption(
                    ui,
                    match stored {
                        Some(s) => tr("ws-stg-slow-now", &[("speed", &num(s, 2))]),
                        None => t("ws-stg-slow-normal"),
                    },
                );
                let mut v = match self.staging_ui.speed {
                    Some((g, v)) if g == f => v,
                    _ => stored.map_or(SPEED_DEFAULT, |s| s as f32),
                };
                let (lo, hi) = TOOTH_SPEED;
                if widgets::labeled_slider(
                    ui,
                    &t("ws-stg-speed"),
                    &mut v,
                    lo as f32..=hi as f32,
                    0.05,
                    2,
                    "",
                    SPEED_DEFAULT,
                )
                .changed()
                {
                    self.staging_ui.speed = Some((f, v));
                }
                ui.add_space(4.0);
                let patch = speed_patch(f, v);
                let changes = patch["speed"].as_f64() != stored;
                if Button::primary(t("ws-stg-slow-apply"))
                    .enabled(!busy && changes && !patch["speed"].is_null())
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    send = Some(patch);
                }
                if stored.is_some() {
                    ui.add_space(4.0);
                    if Button::secondary(t("ws-stg-slow-reset"))
                        .enabled(!busy)
                        .full_width(true)
                        .show(ui)
                        .clicked()
                    {
                        send = Some(speed_patch(f, 1.0));
                    }
                }
            }
        }
        if let Some(d) = data.filter(|d| !d.tooth_speed.is_empty()) {
            ui.add_space(6.0);
            caption(
                ui,
                tr(
                    "ws-stg-slowed-list",
                    &[(
                        "teeth",
                        &slowed_list(d.tooth_speed.iter().map(|(f, s)| (*f, *s))),
                    )],
                ),
            );
        }
        if let Some(p) = send {
            self.staging_ui.speed = None;
            // its report holds the warnings of the aligners it computed again
            self.run_manual(TOOTH_SPEED_CMD, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::Pose;
    use dca_model::{Stage, StagingPattern, StagingSummary};
    use glam::DVec3;

    fn fdi(n: u8) -> Fdi {
        Fdi::new(n).unwrap()
    }

    #[test]
    fn every_numeric_parameter_has_a_range_and_a_slider() {
        let mut fields: Vec<&str> = LIMITS
            .iter()
            .map(|n| n.field)
            .chain([COLLISION_TOL.field])
            .collect();
        fields.sort_unstable();
        let mut all: Vec<&str> = StagingSettings::NUMBERS.iter().map(|n| n.0).collect();
        all.sort_unstable();
        assert_eq!(fields, all);
        for n in LIMITS.iter().chain([&COLLISION_TOL, &DAYS]) {
            let (lo, hi, default) = range_of(n.field);
            assert!(lo < hi && (lo..=hi).contains(&default), "{}", n.field);
        }
        assert_eq!(range_of("interval_days"), (5.0, 21.0, 14.0));
        // every value of a choice is one the case stores
        let mut values = serde_json::to_value(StagingSettings::default()).unwrap();
        values["interval_policy"] = json!(IntervalPolicy::default());
        for c in [&PERIO, &PATTERN] {
            for (name, _) in c.options {
                let mut v = values.clone();
                v[c.field] = json!(name);
                v.as_object_mut().unwrap().remove("interval_policy");
                serde_json::from_value::<StagingSettings>(v).unwrap();
            }
            assert!(c.options.iter().any(|(n, _)| values[c.field] == *n));
        }
        for (name, _) in POLICY.options {
            serde_json::from_value::<IntervalPolicy>(json!(name)).unwrap();
        }
        // every key has its words (both languages: the parity test)
        for c in [&PERIO, &PATTERN, &POLICY] {
            for (_, key) in c.options {
                assert_ne!(t(key), *key, "{key}");
            }
        }
    }

    #[test]
    fn the_speed_is_sent_rounded_and_the_normal_speed_clears_it() {
        assert_eq!(
            speed_patch(fdi(31), 0.504),
            json!({ "fdi": 31, "speed": 0.5 })
        );
        assert_eq!(
            speed_patch(fdi(13), 0.2),
            json!({ "fdi": 13, "speed": 0.2 })
        );
        assert_eq!(
            speed_patch(fdi(13), 1.0),
            json!({ "fdi": 13, "speed": null })
        );
        assert_eq!(
            speed_patch(fdi(13), 0.9996),
            json!({ "fdi": 13, "speed": null })
        );
    }

    /// Stages where tooth 31 moves 0.25 mm per aligner over `n` aligners.
    fn stages(n: u32) -> Vec<Stage> {
        (1..=n)
            .map(|k| {
                let mut s = Stage::new(k);
                s.poses.insert(
                    fdi(31),
                    Pose::new(
                        DVec3::new(0.25 * f64::from(k), 0.0, 0.0),
                        Default::default(),
                    ),
                );
                s
            })
            .collect()
    }

    #[test]
    fn the_result_reads_like_the_plugin_page() {
        // the language of the tests is English (the Italian words have their
        // own parity test)
        assert_eq!(dc_i18n::current(), dc_i18n::Lang::En);
        let today = Date::parse("2026-11-16").unwrap();
        let mut data = AlignersState::default();
        assert!(result_lines(&data, today, true).is_empty());
        data.stages = stages(6);
        data.staging_results.insert(
            ScanRole::Lower,
            StagingSummary {
                aligners: 6,
                pattern: StagingPattern::Structured,
                level: "very_complex".to_owned(),
                slowed: [(fdi(31), 0.5)].into(),
                collisions_left: 2,
                overcorrection: true,
            },
        );
        data.settings.start_date = Some("2026-01-05".to_owned());
        data.settings.interval_days = 10;
        let lines = result_lines(&data, today, true);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        // one arch: not named
        assert!(lines.iter().all(|l| !l.heading), "{texts:?}");
        assert!(texts[0].contains('6'), "{texts:?}");
        // 6 aligners of 10 days from 5 January 2026
        assert!(
            texts[1].contains("60") && texts[1].contains("2.0"),
            "{texts:?}"
        );
        assert!(
            texts[2].contains("05/01/2026")
                && texts[2].contains("06/03/2026")
                && texts[2].contains("22"),
            "{texts:?}"
        );
        assert!(
            texts[3].contains(&t("ws-stg-pattern-structured"))
                && texts[3].contains(&t("ws-stg-level-very-complex")),
            "{texts:?}"
        );
        assert!(texts[4].contains("31 (0.50)"), "{texts:?}");
        assert!(lines[5].warning && texts[5].contains('2'), "{texts:?}");
        assert_eq!(texts[6], t("ws-stg-result-overcorrection"));
        assert_eq!(texts.len(), 7);
        // the advanced options off: no pattern, slowed teeth, overcorrection
        let simple = result_lines(&data, today, false);
        assert_eq!(simple.len(), 4);
        assert!(simple[3].warning, "{simple:?}");
        // the adaptive calendar: 7 days for an anterior translation
        data.settings.interval_policy = IntervalPolicy::Adaptive;
        let lines = result_lines(&data, today, true);
        assert!(lines[1].text.contains("42"), "{:?}", lines[1]);
        // no start date: today
        data.settings.start_date = None;
        let lines = result_lines(&data, today, true);
        assert!(lines[2].text.contains("16/11/2026"), "{:?}", lines[2]);
        // both arches: each named
        data.staging_results
            .insert(ScanRole::Upper, StagingSummary::default());
        let lines = result_lines(&data, today, true);
        assert!(lines[0].heading);
        assert_eq!(lines.iter().filter(|l| l.heading).count(), 2);
    }

    #[test]
    fn the_collision_count_line_gives_way_to_the_list() {
        assert_eq!(dc_i18n::current(), dc_i18n::Lang::En);
        let today = Date::parse("2026-11-16").unwrap();
        let mut data = AlignersState {
            stages: stages(3),
            ..Default::default()
        };
        data.staging_results.insert(
            ScanRole::Lower,
            StagingSummary {
                aligners: 3,
                collisions_left: 1,
                ..Default::default()
            },
        );
        let counts = |data: &AlignersState| {
            result_lines(data, today, false)
                .iter()
                .filter(|l| l.warning)
                .count()
        };
        // a case saved before the list was kept: the count says it
        assert_eq!(counts(&data), 1);
        // with the list, the inspector shows the list and the count is not repeated
        data.staging_collisions.insert(
            ScanRole::Lower,
            vec![dca_model::CollisionLeft {
                stage: 2,
                a: fdi(41),
                b: fdi(31),
                penetration_mm: 0.4,
                allowed_mm: 0.2,
            }],
        );
        assert_eq!(counts(&data), 0);
    }
}
