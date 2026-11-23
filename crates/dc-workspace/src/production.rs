//! The parameters of "Export for printing", as the plugin's page
//! "Produzione" (`ui_phases.py` `page_produzione`) has them:
//!
//! - "Where the aligner ends": for each arch, the last tooth covered on
//!   each side ("Every tooth" or a tooth of that side), the teeth left out,
//!   the ends suggested with a button that applies each, and the warning
//!   when the final position was computed with another end
//!   (`aligners.production.aligner-end`, one undo step each);
//! - the quality of the print files (Draft, Light, Standard, High);
//! - in folded groups, the trim line, the shell, the identification text,
//!   the bite with the aligners worn and the ramps.
//!
//! A change runs `aligners.settings.production` (a slider when it is let
//! go, the case code when the field is left), one undo step each, never the
//! export. The mapping from the case to the controls and to the parameters
//! of the commands is pure and tested without a GPU.

use dc_case::{Fdi, ScanRole};
use dc_ui::fonts;
use dc_ui::widgets::{self, BadgeKind, Button};
use dca_model::{AlignersState, BiteUse, PRODUCTION_NUMBERS, Side, StampSettings};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::function::caption;
use crate::setup::{
    Choice, Number, ResultLine, choice, flag, number, show_result_lines, slider, tr,
};
use crate::t;
use crate::workspace::Workspace;

/// The command that changes the production settings of the case.
pub(crate) const SETTINGS: &str = "aligners.settings.production";
/// The command that sets where the aligner ends.
pub(crate) const ALIGNER_END: &str = "aligners.production.aligner-end";

const QUALITY: Choice = Choice {
    field: "quality",
    key: "ws-prod-quality",
    options: &[
        ("draft", "ws-prod-quality-draft"),
        ("light", "ws-prod-quality-light"),
        ("standard", "ws-prod-quality-standard"),
        ("high", "ws-prod-quality-high"),
    ],
};

const TRIM_MODE: Choice = Choice {
    field: "mode",
    key: "ws-prod-trim-mode",
    options: &[
        ("equator", "ws-prod-trim-mode-equator"),
        ("straight", "ws-prod-trim-mode-straight"),
        ("scalloped", "ws-prod-trim-mode-scalloped"),
        ("smooth", "ws-prod-trim-mode-smooth"),
    ],
};

const INSERTION: Choice = Choice {
    field: "insertion_axis",
    key: "ws-prod-trim-axis",
    options: &[
        ("vertical", "ws-prod-trim-axis-vertical"),
        ("common", "ws-prod-trim-axis-common"),
        ("per_tooth", "ws-prod-trim-axis-per-tooth"),
    ],
};

const STAMP_TOOTH: Choice = Choice {
    field: "tooth",
    key: "ws-prod-stamp-tooth",
    options: &[
        ("M6R", "ws-prod-stamp-tooth-m6r"),
        ("M6L", "ws-prod-stamp-tooth-m6l"),
        ("M7R", "ws-prod-stamp-tooth-m7r"),
        ("M7L", "ws-prod-stamp-tooth-m7l"),
        ("PM5R", "ws-prod-stamp-tooth-pm5r"),
        ("PM5L", "ws-prod-stamp-tooth-pm5l"),
    ],
};

const STAMP_FACE: Choice = Choice {
    field: "face",
    key: "ws-prod-stamp-face",
    options: &[
        ("lingual", "ws-prod-stamp-face-lingual"),
        ("buccal", "ws-prod-stamp-face-buccal"),
    ],
};

const STAMP_READ: Choice = Choice {
    field: "read",
    key: "ws-prod-stamp-read",
    options: &[
        ("inside", "ws-prod-stamp-read-inside"),
        ("outside", "ws-prod-stamp-read-outside"),
    ],
};

const STAMP_DATE: Choice = Choice {
    field: "date",
    key: "ws-prod-stamp-date",
    options: &[
        ("production", "ws-prod-stamp-date-production"),
        ("start", "ws-prod-stamp-date-start"),
    ],
};

const STAMP_DATE_FORMAT: Choice = Choice {
    field: "date_format",
    key: "ws-prod-stamp-date-format",
    options: &[
        ("DDMMYY", "ws-prod-stamp-date-ddmmyy"),
        ("DD.MM.YY", "ws-prod-stamp-date-dotted"),
        ("YYMMDD", "ws-prod-stamp-date-yymmdd"),
    ],
};

const BITE_MODE: Choice = Choice {
    field: "mode",
    key: "ws-prod-bite-mode",
    options: &[
        ("auto", "ws-prod-bite-mode-auto"),
        ("manual", "ws-prod-bite-mode-manual"),
        ("same", "ws-prod-bite-mode-same"),
    ],
};

const BITE_CLOSING: Choice = Choice {
    field: "closing",
    key: "ws-prod-bite-closing",
    options: &[
        ("hinge", "ws-prod-bite-closing-hinge"),
        ("vertical", "ws-prod-bite-closing-vertical"),
    ],
};

/// One control of a folded group.
enum Item {
    Number(Number),
    Flag(&'static str, &'static str),
    Choice(Choice),
    /// Shown only while the choice `field` of the group is `value`.
    When(&'static str, &'static str, &'static Item),
}

const MANUAL_THICKNESS: Item = Item::Number(number(
    "thickness_mm",
    "ws-prod-bite-thickness",
    0.05,
    2,
    "mm",
));
const HINGE_POSTERIOR: Item = Item::Number(number(
    "hinge_posterior_mm",
    "ws-prod-bite-hinge-posterior",
    1.0,
    0,
    "mm",
));
const HINGE_HEIGHT: Item = Item::Number(number(
    "hinge_height_mm",
    "ws-prod-bite-hinge-height",
    1.0,
    0,
    "mm",
));
const EQUILIBRATE_TEETH: Item = Item::Number(number(
    "equilibrate_teeth_per_side",
    "ws-prod-bite-equilibrate-teeth",
    1.0,
    0,
    "",
));
const EQUILIBRATE_MAX: Item = Item::Number(number(
    "equilibrate_max_mm",
    "ws-prod-bite-equilibrate-max",
    0.05,
    2,
    "mm",
));

/// The folded groups: the group of `aligners.settings.production`, the key
/// of its title and its controls, in the order of the plugin's page.
const GROUPS: &[(&str, &str, &[Item])] = &[
    (
        "trim",
        "ws-prod-group-trim",
        &[
            Item::Choice(TRIM_MODE),
            Item::Number(number(
                "margin_gap_mm",
                "ws-prod-trim-margin-gap",
                0.1,
                1,
                "mm",
            )),
            Item::Number(number("scallop_mm", "ws-prod-trim-scallop", 0.1, 1, "mm")),
            Item::Number(number(
                "papilla_relief_mm",
                "ws-prod-trim-papilla",
                0.1,
                1,
                "mm",
            )),
            Item::Number(number("recess_mm", "ws-prod-trim-recess", 0.1, 1, "mm")),
            Item::Number(number(
                "extension_mm",
                "ws-prod-trim-extension",
                0.1,
                1,
                "mm",
            )),
            Item::Number(number(
                "equator_offset_mm",
                "ws-prod-trim-equator-offset",
                0.1,
                1,
                "mm",
            )),
            Item::Choice(INSERTION),
        ],
    ),
    (
        "shell",
        "ws-prod-group-shell",
        &[
            Item::Number(number(
                "shell_thickness_mm",
                "ws-prod-shell-thickness",
                0.05,
                2,
                "mm",
            )),
            Item::Number(number(
                "clearance_mm",
                "ws-prod-shell-clearance",
                0.01,
                2,
                "mm",
            )),
            Item::Number(number(
                "gum_clearance_mm",
                "ws-prod-shell-gum-clearance",
                0.05,
                2,
                "mm",
            )),
        ],
    ),
    (
        "stamp",
        "ws-prod-group-stamp",
        &[
            Item::Flag("enabled", "ws-prod-stamp-enabled"),
            Item::When("enabled", "true", &Item::Choice(STAMP_TOOTH)),
            Item::When("enabled", "true", &Item::Choice(STAMP_FACE)),
            Item::When("enabled", "true", &Item::Choice(STAMP_READ)),
            Item::When("enabled", "true", &Item::Choice(STAMP_DATE)),
            Item::When("enabled", "true", &Item::Choice(STAMP_DATE_FORMAT)),
            Item::When(
                "enabled",
                "true",
                &Item::Number(number("height_mm", "ws-prod-stamp-height", 0.05, 2, "mm")),
            ),
            Item::When(
                "enabled",
                "true",
                &Item::Number(number("depth_mm", "ws-prod-stamp-depth", 0.01, 2, "mm")),
            ),
            Item::When(
                "enabled",
                "true",
                &Item::Number(number(
                    "min_wall_mm",
                    "ws-prod-stamp-min-wall",
                    0.05,
                    2,
                    "mm",
                )),
            ),
        ],
    ),
    (
        "bite",
        "ws-prod-group-bite",
        &[
            Item::Choice(BITE_MODE),
            Item::When("mode", "manual", &MANUAL_THICKNESS),
            Item::Number(number(
                "min_thickness_mm",
                "ws-prod-bite-min-thickness",
                0.05,
                2,
                "mm",
            )),
            Item::Number(number(
                "opening_target_mm",
                "ws-prod-bite-opening",
                0.1,
                1,
                "mm",
            )),
            Item::Choice(BITE_CLOSING),
            Item::When("closing", "hinge", &HINGE_POSTERIOR),
            Item::When("closing", "hinge", &HINGE_HEIGHT),
            Item::Flag("equilibrate", "ws-prod-bite-equilibrate"),
            Item::When("equilibrate", "true", &EQUILIBRATE_TEETH),
            Item::When("equilibrate", "true", &EQUILIBRATE_MAX),
        ],
    ),
    (
        "ramps",
        "ws-prod-group-ramps",
        &[
            Item::Flag("enabled", "ws-prod-ramps-enabled"),
            Item::Number(number("width_mm", "ws-prod-ramps-width", 0.1, 1, "mm")),
            Item::Number(number("depth_mm", "ws-prod-ramps-depth", 0.1, 1, "mm")),
            Item::Number(number("height_mm", "ws-prod-ramps-height", 0.1, 1, "mm")),
            Item::Number(number(
                "overbite_min_mm",
                "ws-prod-ramps-overbite",
                0.1,
                1,
                "mm",
            )),
        ],
    ),
];

/// The groups of settings that work on the bite of the two arches (the bite
/// with the aligners worn, the ramps): ignored with one arch.
fn ignored_with_one_arch(group: &str) -> bool {
    matches!(group, "bite" | "ramps")
}

/// The settings that are whole numbers.
const WHOLE: [&str; 1] = ["equilibrate_teeth_per_side"];

/// What the inspector keeps between frames: the slider being dragged in
/// each group and the case code being typed.
#[derive(Debug, Default)]
pub(crate) struct ProductionUi {
    drag: [Option<(&'static str, f32)>; 5],
    code: Option<String>,
}

/// The range and the default of the numeric setting `field` of `group`.
fn range_of(group: &str, field: &str) -> (f32, f32, f32) {
    let (_, _, lo, hi) = PRODUCTION_NUMBERS
        .iter()
        .find(|n| n.0 == group && n.1 == field)
        .copied()
        .unwrap_or(("", "", 0.0, 1.0));
    let default = AlignersState::default().production_values()[group][field]
        .as_f64()
        .unwrap_or(lo);
    (lo as f32, hi as f32, default as f32)
}

/// The parameters of `aligners.settings.production` for the change
/// `change` (`{field: value}`) of the group `group`; a whole number goes as
/// one.
pub(crate) fn settings_patch(group: &str, change: Value) -> Value {
    if group == "quality" {
        return change;
    }
    let mut change = change;
    if let Some(m) = change.as_object_mut() {
        for (k, v) in m.iter_mut() {
            if WHOLE.contains(&k.as_str())
                && let Some(n) = v.as_f64()
            {
                *v = json!(n.round() as u64);
            }
        }
    }
    json!({ group: change })
}

/// The choices of the end of the aligner of the arch `role` on `side`:
/// every tooth (`None`), then the teeth of that side from the front; and
/// the one in force.
pub(crate) fn end_options(
    data: &AlignersState,
    role: ScanRole,
    side: Side,
) -> (Vec<Option<Fdi>>, usize) {
    let q = side.quadrant(role);
    let mut teeth: Vec<Fdi> = data
        .arch_teeth(role)
        .into_iter()
        .filter(|f| Some(f.quadrant()) == q)
        .collect();
    teeth.sort_by_key(|f| f.position());
    let now = data.aligner_end_of(role).get(side);
    let options: Vec<Option<Fdi>> = std::iter::once(None)
        .chain(teeth.into_iter().map(Some))
        .collect();
    let at = options.iter().position(|o| *o == now).unwrap_or(0);
    (options, at)
}

/// The parameters of `aligners.production.aligner-end` that end the
/// aligner of `role` on `side` at `last` (`None`: every tooth).
pub(crate) fn end_patch(role: ScanRole, side: Side, last: Option<Fdi>) -> Value {
    match last {
        Some(f) => json!({ "action": "set", "fdi": f.get() }),
        None => json!({
            "action": "clear",
            "arch": if role == ScanRole::Upper { "upper" } else { "lower" },
            "side": side,
        }),
    }
}

/// The lines under the end of the aligner of `role`: the teeth left out,
/// and a warning when the final position was computed with another end.
pub(crate) fn end_lines(data: &AlignersState, role: ScanRole) -> Vec<ResultLine> {
    let mut lines = Vec::new();
    let out = data.out_of_aligner(role);
    if !out.is_empty() {
        let list = out
            .iter()
            .map(Fdi::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(ResultLine {
            text: tr("ws-prod-end-out", &[("teeth", &list)]),
            heading: false,
            warning: false,
        });
    }
    if data.setup_end_changed(role) {
        // a short badge, then what to do in a line that wraps
        lines.push(ResultLine {
            text: t("ws-prod-end-setup-outdated"),
            heading: false,
            warning: true,
        });
        lines.push(ResultLine {
            text: t("ws-prod-end-setup-outdated-hint"),
            heading: false,
            warning: false,
        });
    }
    lines
}

/// The case code of the stamp, when the stamp is on: a field written when it
/// is left; "the stamp is off" otherwise.
fn stamp_code(ui: &mut Ui, stamp: &Value, typing: &mut Option<String>, change: &mut Option<Value>) {
    if stamp["enabled"].as_bool() != Some(true) {
        caption(ui, t("ws-prod-stamp-off"));
        return;
    }
    let stored = stamp["code"].as_str().unwrap_or_default().to_owned();
    let mut text = typing.clone().unwrap_or_else(|| stored.clone());
    let ok = StampSettings::code_ok(text.trim());
    let error = (!ok).then(|| t("ws-prod-stamp-code-invalid"));
    let r = widgets::text_input(
        ui,
        &t("ws-prod-stamp-code"),
        &mut text,
        &t("ws-prod-stamp-code-hint"),
        Some(&t("ws-prod-stamp-code-help")),
        error.as_deref(),
    );
    if r.lost_focus() {
        *typing = None;
        if ok && text.trim() != stored {
            *change = Some(json!({ "code": text.trim() }));
        }
    } else if r.has_focus() {
        *typing = Some(text);
    } else {
        *typing = None;
    }
}

/// Whether `item` is shown with the values `values` of its group.
fn shown(item: &Item, values: &Value) -> bool {
    match item {
        Item::When(field, value, _) => match &values[*field] {
            Value::Bool(b) => b.to_string() == *value,
            v => v.as_str() == Some(*value),
        },
        _ => true,
    }
}

impl Workspace {
    /// The parameters of "Export for printing" (see the module
    /// documentation). With the advanced options off: where the aligner
    /// ends, the quality and the stamp on or off with its case code.
    pub(crate) fn production_params(&mut self, ui: &mut Ui) {
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        let state = self.undo.current().clone();
        let data = crate::overlay::aligners(&state)
            .cloned()
            .unwrap_or_default();
        let values = data.production_values();
        let busy = self.is_busy();
        let mut end_send = None;
        let mut commit: Option<(&str, Value)> = None;
        let ui_state = &mut self.production_ui;
        ui.add_enabled_ui(!busy, |ui| {
            // where the aligner ends
            ui.label(
                RichText::new(t("ws-prod-end"))
                    .font(fonts::semibold(12.0))
                    .color(dc_ui::theme::colors(ui).text_primary),
            );
            caption(ui, t("ws-prod-end-hint"));
            let roles: Vec<ScanRole> = [ScanRole::Upper, ScanRole::Lower]
                .into_iter()
                .filter(|r| state.case.scan(*r).is_some() && !data.arch_teeth(*r).is_empty())
                .collect();
            if roles.is_empty() {
                caption(ui, t("ws-prod-end-no-teeth"));
            }
            for role in roles {
                ui.add_space(4.0);
                caption(
                    ui,
                    t(if role == ScanRole::Upper {
                        "ws-prod-arch-upper"
                    } else {
                        "ws-prod-arch-lower"
                    }),
                );
                for side in [Side::Right, Side::Left] {
                    let (options, now) = end_options(&data, role, side);
                    ui.label(
                        RichText::new(t(match side {
                            Side::Right => "ws-prod-end-right",
                            Side::Left => "ws-prod-end-left",
                        }))
                        .font(fonts::ui(12.0)),
                    );
                    let labels: Vec<String> = options
                        .iter()
                        .map(|o| o.map_or_else(|| t("ws-prod-end-all"), |f| f.to_string()))
                        .collect();
                    let mut at = now;
                    if widgets::segmented_fill(ui, &mut at, &labels, 28.0).changed() && at != now {
                        end_send = Some(end_patch(role, side, options[at]));
                    }
                    ui.add_space(2.0);
                }
                show_result_lines(ui, end_lines(&data, role));
                for s in data.aligner_end_suggestions(role) {
                    ui.add_space(4.0);
                    // a short badge, the reason in a line that wraps
                    widgets::badge(
                        ui,
                        BadgeKind::Warning,
                        t(match s.side {
                            Side::Right => "ws-prod-end-suggest-right",
                            Side::Left => "ws-prod-end-suggest-left",
                        }),
                    );
                    caption(
                        ui,
                        tr(
                            "ws-prod-end-suggest-why",
                            &[
                                ("missing", &s.missing.to_string()),
                                ("last", &s.last.to_string()),
                            ],
                        ),
                    );
                    if Button::secondary(tr(
                        "ws-prod-end-suggest-apply",
                        &[("last", &s.last.to_string())],
                    ))
                    .full_width(true)
                    .show(ui)
                    .clicked()
                    {
                        end_send = Some(end_patch(role, s.side, Some(s.last)));
                    }
                }
            }

            // the quality of the print files
            ui.add_space(8.0);
            let mut quality = None;
            choice(ui, &values, &QUALITY, &mut quality);
            if let Some(q) = quality {
                commit = Some(("quality", q));
            }

            if !advanced {
                // the stamp alone, out of its group
                ui.add_space(6.0);
                let stamp = &values["stamp"];
                let mut change = None;
                flag(ui, stamp, "enabled", "ws-prod-stamp-enabled", &mut change);
                stamp_code(ui, stamp, &mut ui_state.code, &mut change);
                if let Some(c) = change {
                    commit = Some(("stamp", c));
                }
                return;
            }

            // the folded groups
            let one_arch = BiteUse::of(&state).one_arch();
            for (gi, (group, title, items)) in GROUPS.iter().enumerate() {
                let group_values = &values[*group];
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new(t(title)).font(fonts::semibold(12.0)))
                    .id_salt(*title)
                    .default_open(false)
                    .show(ui, |ui| {
                        // one arch: there is no bite, so these settings are
                        // ignored; as the Bite inspector says it
                        if one_arch && ignored_with_one_arch(group) {
                            widgets::badge(ui, BadgeKind::Neutral, t("ws-fn-not-applicable"));
                            ui.add_space(6.0);
                            caption(ui, t("ws-prod-one-arch"));
                            return;
                        }
                        let mut change = None;
                        for item in *items {
                            if !shown(item, group_values) {
                                continue;
                            }
                            let item = match item {
                                Item::When(_, _, inner) => inner,
                                other => other,
                            };
                            match item {
                                Item::Number(n) => slider(
                                    ui,
                                    group_values[n.field].as_f64().unwrap_or_default(),
                                    range_of(group, n.field),
                                    n,
                                    &mut ui_state.drag[gi],
                                    &mut change,
                                ),
                                Item::Flag(field, key) => {
                                    flag(ui, group_values, field, key, &mut change)
                                }
                                Item::Choice(c) => choice(ui, group_values, c, &mut change),
                                Item::When(..) => {}
                            }
                        }
                        if *group == "stamp" {
                            stamp_code(ui, group_values, &mut ui_state.code, &mut change);
                        }
                        if let Some(c) = change {
                            commit = Some((group, c));
                        }
                    });
            }
        });
        if advanced {
            ui.add_space(6.0);
            caption(ui, t("ws-prod-params-rerun"));
        }
        if let Some(p) = end_send {
            self.run_module_command(ALIGNER_END, p);
        } else if let Some((group, change)) = commit {
            self.run_module_command(SETTINGS, settings_patch(group, change));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dca_model::{AlignerEnd, PRODUCTION_GROUPS, Seed, SetupSummary};
    use glam::DVec3;

    fn fdi(n: u8) -> Fdi {
        Fdi::new(n).unwrap()
    }

    fn with_seeds(teeth: &[u8]) -> AlignersState {
        AlignersState {
            seeds: teeth
                .iter()
                .map(|&n| Seed {
                    fdi: fdi(n),
                    position: DVec3::ZERO,
                })
                .collect(),
            ..AlignersState::default()
        }
    }

    fn numbers(items: &[Item]) -> Vec<&'static str> {
        items
            .iter()
            .filter_map(|i| match i {
                Item::Number(n) => Some(n.field),
                Item::When(_, _, Item::Number(n)) => Some(n.field),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_numeric_setting_has_one_slider_with_its_range() {
        let mut shown: Vec<(&str, &str)> = GROUPS
            .iter()
            .flat_map(|(g, _, items)| numbers(items).into_iter().map(move |f| (*g, f)))
            .collect();
        shown.sort_unstable();
        let mut all: Vec<(&str, &str)> = PRODUCTION_NUMBERS.iter().map(|n| (n.0, n.1)).collect();
        all.sort_unstable();
        assert_eq!(shown, all);
        for (g, f) in all {
            let (lo, hi, default) = range_of(g, f);
            assert!(lo < hi && (lo..=hi).contains(&default), "{g}.{f}");
        }
        // every group but the quality is folded; the quality is a choice
        let mut groups: Vec<&str> = GROUPS.iter().map(|g| g.0).collect();
        groups.push(QUALITY.field);
        assert_eq!(groups, PRODUCTION_GROUPS);
    }

    #[test]
    fn every_choice_is_a_value_the_case_stores_and_has_its_words() {
        let values = AlignersState::default().production_values();
        let mut choices: Vec<(&str, &Choice)> = vec![("quality", &QUALITY)];
        for (g, _, items) in GROUPS {
            for i in *items {
                match i {
                    Item::Choice(c) | Item::When(_, _, Item::Choice(c)) => choices.push((g, c)),
                    _ => {}
                }
            }
        }
        for (g, c) in choices {
            for (name, key) in c.options {
                let mut s = AlignersState::default();
                let v = if g == "quality" {
                    json!(name)
                } else {
                    let mut v = values[g].clone();
                    v[c.field] = json!(name);
                    v
                };
                s.set_production_group(g, v)
                    .unwrap_or_else(|e| panic!("{g}.{name}: {e}"));
                assert_ne!(t(key), *key, "{key}");
            }
            let now = if g == "quality" {
                &values[g]
            } else {
                &values[g][c.field]
            };
            assert!(c.options.iter().any(|(n, _)| now == n), "{g}.{}", c.field);
            assert_ne!(t(c.key), c.key);
        }
        for (_, title, _) in GROUPS {
            assert_ne!(t(title), *title);
        }
    }

    #[test]
    fn a_change_is_sent_to_its_group() {
        assert_eq!(
            settings_patch("trim", json!({ "margin_gap_mm": 1.2 })),
            json!({ "trim": { "margin_gap_mm": 1.2 } })
        );
        assert_eq!(
            settings_patch("bite", json!({ "equilibrate_teeth_per_side": 3.0 })),
            json!({ "bite": { "equilibrate_teeth_per_side": 3 } })
        );
        assert_eq!(
            settings_patch("quality", json!({ "quality": "high" })),
            json!({ "quality": "high" })
        );
    }

    #[test]
    fn the_end_offers_every_tooth_then_the_teeth_of_the_side() {
        let mut d = with_seeds(&[31, 32, 33, 34, 35, 36, 37, 41, 46, 47, 11]);
        let (opts, at) = end_options(&d, ScanRole::Lower, Side::Right);
        assert_eq!(opts, [None, Some(fdi(41)), Some(fdi(46)), Some(fdi(47))]);
        assert_eq!(at, 0);
        d.aligner_end.insert(
            ScanRole::Lower,
            AlignerEnd {
                right: Some(fdi(46)),
                left: None,
            },
        );
        assert_eq!(end_options(&d, ScanRole::Lower, Side::Right).1, 2);
        assert_eq!(end_options(&d, ScanRole::Lower, Side::Left).0.len(), 8);
        assert_eq!(
            end_patch(ScanRole::Lower, Side::Right, Some(fdi(46))),
            json!({ "action": "set", "fdi": 46 })
        );
        assert_eq!(
            end_patch(ScanRole::Upper, Side::Left, None),
            json!({ "action": "clear", "arch": "upper", "side": "left" })
        );
        // the teeth left out, then the final position computed with another end
        let lines = end_lines(&d, ScanRole::Lower);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].text.contains("47") && !lines[0].warning);
        d.setup
            .results
            .insert(ScanRole::Lower, SetupSummary::default());
        let lines = end_lines(&d, ScanRole::Lower);
        assert!(lines[1].warning && !lines[2].warning);
        assert_eq!(lines[1].text, t("ws-prod-end-setup-outdated"));
        assert_eq!(lines[2].text, t("ws-prod-end-setup-outdated-hint"));
    }

    #[test]
    fn hidden_controls_follow_the_values_of_their_group() {
        let values = AlignersState::default().production_values();
        assert!(!shown(
            &Item::When("mode", "manual", &MANUAL_THICKNESS),
            &values["bite"]
        ));
        assert!(shown(
            &Item::When("closing", "hinge", &HINGE_HEIGHT),
            &values["bite"]
        ));
        assert!(!shown(
            &Item::When("equilibrate", "true", &EQUILIBRATE_MAX),
            &values["bite"]
        ));
        assert!(shown(
            &Item::When("enabled", "true", &MANUAL_THICKNESS),
            &values["stamp"]
        ));
    }
}
