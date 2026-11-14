//! The inspector of "Final position", as the plugin's page (`ui_phases.py`
//! `page_finale`) has it:
//!
//! - under the status, the result of the last computation of each arch
//!   (expansion, incisors, IPR, the lines of its log the plugin shows, the
//!   crowding not resolved), read from the case (`Setup::results`);
//! - "Parameters": the setup parameters stored in the case, in the groups
//!   of the plugin's page; a change runs `aligners.settings.setup` (one undo
//!   step), never the computation;
//! - in "Manual corrections", "Back to the initial position"
//!   (`aligners.planning.setup.reset`), and the numbers of "Move a tooth":
//!   the movement of the tooth in its clinical frame and the notes on how
//!   predictable it is (`movement_flags`).
//!
//! The mapping from the case to the lines and to the parameters of the
//! commands is pure and tested without a GPU.

use dc_case::{Fdi, ScanRole};
use dc_i18n::Lang;
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dca_algo::anatomy::Arch;
use dca_algo::diagnosis::classify::{self, MovementDesc, MovementFlag};
use dca_algo::setup::Note;
use dca_algo::staging;
use dca_model::{AlignersState, SetupSettings};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::t;
use crate::workspace::Workspace;

/// The command that changes the parameters of the case.
pub(crate) const SETTINGS: &str = "aligners.settings.setup";
/// "Back to the initial position".
pub(crate) const RESET: &str = "aligners.planning.setup.reset";

/// A numeric parameter: its field, the key of its label, the step of its
/// slider, the decimals shown and its unit (its range and default are the
/// case settings', [`SetupSettings::NUMBERS`]).
struct Number {
    field: &'static str,
    key: &'static str,
    step: f32,
    decimals: usize,
    unit: &'static str,
}

const fn number(
    field: &'static str,
    key: &'static str,
    step: f32,
    decimals: usize,
    unit: &'static str,
) -> Number {
    Number {
        field,
        key,
        step,
        decimals,
        unit,
    }
}

/// A parameter chosen among a few: its field, the key of its label and its
/// values (the name the case stores, the key of its label).
struct Choice {
    field: &'static str,
    key: &'static str,
    options: &'static [(&'static str, &'static str)],
}

/// One parameter of a group.
enum Item {
    Number(Number),
    Flag(&'static str, &'static str),
    Choice(Choice),
    /// The expansions of the arches of the case (the plugin shows those of
    /// its arch).
    Expansion,
}

const SCOPE: Choice = Choice {
    field: "align_scope",
    key: "ws-setup-scope",
    options: &[
        ("anterior", "ws-setup-scope-anterior"),
        ("premolars", "ws-setup-scope-premolars"),
        ("all", "ws-setup-scope-all"),
    ],
};

/// The groups of the parameters, in the order and with the titles of the
/// plugin's page (`Parametri della posizione finale`).
const GROUPS: &[(&str, &[Item])] = &[
    (
        "ws-setup-group-line",
        &[
            Item::Number(number("line_sigma_mm", "ws-setup-line-sigma", 0.5, 1, "mm")),
            Item::Number(number(
                "normal_tol_mm",
                "ws-setup-normal-tol",
                0.01,
                2,
                "mm",
            )),
            Item::Number(number("rot_tol_deg", "ws-setup-rot-tol", 0.5, 1, "°")),
            Item::Number(number(
                "post_normal_tol_mm",
                "ws-setup-post-normal-tol",
                0.01,
                2,
                "mm",
            )),
            Item::Number(number(
                "post_rot_tol_deg",
                "ws-setup-post-rot-tol",
                0.5,
                1,
                "°",
            )),
            Item::Number(number("pos_tol_mm", "ws-setup-pos-tol", 0.01, 2, "mm")),
            Item::Number(number(
                "contact_tol_mm",
                "ws-setup-contact-tol",
                0.01,
                2,
                "mm",
            )),
        ],
    ),
    (
        "ws-setup-group-held",
        &[
            Item::Flag("fix_molars", "ws-setup-fix-molars"),
            Item::Flag("align_fixed", "ws-setup-align-fixed"),
            Item::Flag("fix_third", "ws-setup-fix-third"),
            Item::Choice(Choice {
                field: "midline",
                key: "ws-setup-midline",
                options: &[
                    ("keep", "ws-setup-midline-keep"),
                    ("center", "ws-setup-midline-center"),
                    ("balance", "ws-setup-midline-balance"),
                ],
            }),
            Item::Number(number(
                "incisor_weight",
                "ws-setup-incisor-weight",
                0.5,
                1,
                "",
            )),
        ],
    ),
    (
        "ws-setup-group-space",
        &[
            Item::Choice(Choice {
                field: "priority",
                key: "ws-setup-priority",
                options: &[
                    ("exp_ipr_pro", "ws-setup-priority-exp-ipr-pro"),
                    ("ipr_exp_pro", "ws-setup-priority-ipr-exp-pro"),
                    ("exp_pro_ipr", "ws-setup-priority-exp-pro-ipr"),
                ],
            }),
            Item::Expansion,
            Item::Number(number(
                "advance_max_mm",
                "ws-setup-advance-max",
                0.1,
                1,
                "mm",
            )),
            Item::Number(number(
                "retract_max_mm",
                "ws-setup-retract-max",
                0.1,
                1,
                "mm",
            )),
            Item::Flag("close_spaces", "ws-setup-close-spaces"),
        ],
    ),
    (
        "ws-setup-group-ipr",
        &[
            Item::Choice(Choice {
                field: "ipr_zone",
                key: "ws-setup-ipr-zone",
                options: &[
                    ("none", "ws-setup-ipr-zone-none"),
                    ("anterior", "ws-setup-ipr-zone-anterior"),
                    ("anterior_premolars", "ws-setup-ipr-zone-anterior-premolars"),
                    ("all", "ws-setup-ipr-zone-all"),
                ],
            }),
            Item::Number(number(
                "ipr_surface_cap_mm",
                "ws-setup-ipr-surface-cap",
                0.01,
                2,
                "mm",
            )),
            Item::Number(number(
                "enamel_fraction",
                "ws-setup-enamel-fraction",
                0.01,
                2,
                "",
            )),
        ],
    ),
    (
        "ws-setup-group-vertical",
        &[
            Item::Choice(Choice {
                field: "leveling",
                key: "ws-setup-leveling",
                options: &[
                    ("smooth", "ws-setup-leveling-smooth"),
                    ("flat", "ws-setup-leveling-flat"),
                    ("none", "ws-setup-leveling-none"),
                ],
            }),
            Item::Number(number(
                "vertical_max_mm",
                "ws-setup-vertical-max",
                0.05,
                2,
                "mm",
            )),
            Item::Number(number("level_tol_mm", "ws-setup-level-tol", 0.05, 2, "mm")),
            Item::Flag("level_canines", "ws-setup-level-canines"),
        ],
    ),
];

const EXPANSION_UPPER: Number = number(
    "expansion_upper_mm",
    "ws-setup-expansion-upper",
    0.1,
    1,
    "mm",
);
const EXPANSION_LOWER: [Number; 2] = [
    number(
        "expansion_lower_posterior_mm",
        "ws-setup-expansion-lower-posterior",
        0.1,
        1,
        "mm",
    ),
    number(
        "expansion_lower_canine_mm",
        "ws-setup-expansion-lower-canine",
        0.1,
        1,
        "mm",
    ),
];

/// The steps of the − and + buttons of "Move a tooth".
const STEP_MM: Number = number("adjust_step_mm", "ws-manual-step-mm", 0.01, 2, "mm");
const STEP_DEG: Number = number("adjust_step_deg", "ws-manual-step-deg", 0.1, 1, "°");

/// What the parameters keep between frames: the slider being dragged.
#[derive(Debug, Default)]
pub(crate) struct SetupUi {
    drag: Option<(&'static str, f32)>,
}

/// The parameters of `aligners.settings.setup` that set the numeric
/// setting `field` to `v`, rounded to the decimals shown; `None` when that
/// is its value already.
fn number_patch(s: &SetupSettings, field: &'static str, v: f32, decimals: usize) -> Option<Value> {
    let scale = 10f64.powi(decimals as i32);
    let v = (f64::from(v) * scale).round() / scale;
    let now = s.number(field)?;
    ((v - now).abs() >= 0.5 / scale).then(|| json!({ field: v }))
}

/// The range and the default of the numeric setting `field`.
fn range_of(field: &str) -> (f32, f32, f32) {
    let (_, lo, hi) = SetupSettings::NUMBERS
        .iter()
        .find(|(f, _, _)| *f == field)
        .copied()
        .unwrap_or(("", 0.0, 1.0));
    let default = SetupSettings::default().number(field).unwrap_or(lo);
    (lo as f32, hi as f32, default as f32)
}

/// A slider of a numeric setting; a change is kept in `commit` when it is
/// let go.
fn slider(
    ui: &mut Ui,
    s: &SetupSettings,
    n: &Number,
    drag: &mut Option<(&'static str, f32)>,
    commit: &mut Option<Value>,
) {
    let (lo, hi, default) = range_of(n.field);
    let mut v = match *drag {
        Some((f, v)) if f == n.field => v,
        _ => s.number(n.field).unwrap_or(f64::from(default)) as f32,
    };
    let r = widgets::labeled_slider(
        ui,
        &t(n.key),
        &mut v,
        lo..=hi,
        n.step,
        n.decimals,
        n.unit,
        default,
    );
    if r.dragged() {
        *drag = Some((n.field, v));
    } else if r.drag_stopped() || r.changed() {
        *drag = None;
        if let Some(p) = number_patch(s, n.field, v, n.decimals) {
            *commit = Some(p);
        }
    }
    ui.add_space(4.0);
}

/// A choice among a few values, one row each.
fn choice(ui: &mut Ui, values: &Value, c: &Choice, commit: &mut Option<Value>) {
    let col = *colors(ui);
    ui.label(
        RichText::new(t(c.key))
            .font(fonts::medium(12.0))
            .color(col.text_secondary),
    );
    let now = values[c.field].as_str().unwrap_or_default();
    for (name, key) in c.options {
        let selected = *name == now;
        let r = ui
            .horizontal(|ui| {
                let dot = widgets::radio_indicator(ui, selected, ui.is_enabled());
                let text = ui.add(
                    egui::Label::new(
                        RichText::new(t(key))
                            .font(fonts::ui(12.0))
                            .color(col.text_primary),
                    )
                    .sense(egui::Sense::click()),
                );
                dot.interact(egui::Sense::click()).clicked() || text.clicked()
            })
            .inner;
        if r && !selected {
            *commit = Some(json!({ c.field: name }));
        }
    }
    ui.add_space(4.0);
}

/// A setting that is on or off.
fn flag(ui: &mut Ui, values: &Value, field: &'static str, key: &str, commit: &mut Option<Value>) {
    let mut on = values[field].as_bool().unwrap_or_default();
    if widgets::checkbox(ui, &mut on, t(key)).changed() {
        *commit = Some(json!({ field: on }));
    }
    ui.add_space(2.0);
}

/// A number with `decimals` decimals and the decimal mark of the language.
fn num(v: f64, decimals: usize) -> String {
    let text = format!("{v:.decimals$}");
    match dc_i18n::current() {
        Lang::It => text.replace('.', ","),
        Lang::En => text,
    }
}

/// [`num`] with a sign.
pub(crate) fn signed(v: f64, decimals: usize) -> String {
    let text = num(v, decimals);
    if v >= 0.5 * 10f64.powi(-(decimals as i32)) {
        format!("+{text}")
    } else {
        text
    }
}

fn tr(key: &str, args: &[(&str, &str)]) -> String {
    crate::loader();
    dc_i18n::tr_args(key, args)
}

fn teeth_list(teeth: &[Fdi]) -> String {
    teeth
        .iter()
        .map(Fdi::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The words of a line of the log of the final position, for the lines
/// the plugin's page shows (`Denti allineati`, `Valutazione`, `Dente
/// bloccato`, `Denti fermi`, `Denti che si muovono`, `Linea mediana`,
/// `Spazio insufficiente`, `Spazio che resta`); `None` for the others.
fn note_text(note: &Note) -> Option<String> {
    let deviation = |d: &dca_algo::setup::Deviation| {
        format!(
            "{} ({} mm, {}°)",
            d.fdi,
            num(d.off_mm, 1),
            num(d.rot_deg, 0)
        )
    };
    let left = |l: &[dca_algo::setup::LeftRotation]| {
        l.iter()
            .map(|r| format!("{} {}°", r.fdi, num(r.deg, 0)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Some(match note {
        Note::Scope { scope } => t(match scope {
            dca_algo::setup::AlignScope::Anterior => "ws-setup-note-scope-anterior",
            dca_algo::setup::AlignScope::Premolars => "ws-setup-note-scope-premolars",
            dca_algo::setup::AlignScope::All => "ws-setup-note-scope-all",
        }),
        Note::Evaluation { to_align, in_place } => {
            let list = if to_align.is_empty() {
                t("ws-setup-note-no-tooth")
            } else {
                to_align
                    .iter()
                    .map(deviation)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut text = tr("ws-setup-note-evaluation", &[("teeth", &list)]);
            if !in_place.is_empty() {
                text.push_str(&tr(
                    "ws-setup-note-in-place",
                    &[("teeth", &teeth_list(in_place))],
                ));
            }
            text
        }
        Note::LockedOffLine { tooth } => tr(
            "ws-setup-note-locked-off-line",
            &[
                ("fdi", &tooth.fdi.to_string()),
                ("mm", &num(tooth.off_mm, 1)),
                ("deg", &num(tooth.rot_deg, 0)),
            ],
        ),
        Note::Anchors {
            teeth,
            aligned_in_place,
        } => {
            let list = if teeth.is_empty() {
                t("ws-setup-note-none")
            } else {
                teeth_list(teeth)
            };
            let mut text = tr("ws-setup-note-anchors", &[("teeth", &list)]);
            if !aligned_in_place.is_empty() {
                text.push_str(&tr(
                    "ws-setup-note-aligned-in-place",
                    &[("teeth", &teeth_list(aligned_in_place))],
                ));
            }
            text
        }
        Note::AnchorsNotDerotated { teeth } => tr(
            "ws-setup-note-anchors-not-derotated",
            &[("teeth", &left(teeth))],
        ),
        Note::Moved {
            count,
            of,
            max_mm,
            mean_mm,
        } => tr(
            "ws-setup-note-moved",
            &[
                ("count", &count.to_string()),
                ("of", &of.to_string()),
                ("max", &num(*max_mm, 2)),
                ("mean", &num(*mean_mm, 2)),
            ],
        ),
        Note::Midline { shift_mm } => tr("ws-setup-note-midline", &[("mm", &signed(*shift_mm, 2))]),
        Note::PartialDerotation { teeth } => tr(
            "ws-setup-note-partial-derotation",
            &[("teeth", &left(teeth))],
        ),
        Note::Spacing { mm } => tr("ws-setup-note-spacing", &[("mm", &num(*mm, 2))]),
        _ => return None,
    })
}

/// A line of the result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResultLine {
    pub text: String,
    /// The arch it is about starts here.
    pub heading: bool,
    /// Something to look at (the plugin's `ERROR` icon).
    pub warning: bool,
}

/// The result of the last final position of each arch, as the plugin's
/// page words `setup_json`: the expansion, the incisors, the IPR of the
/// arch (`Setup::ipr`), the lines of the log it shows and the crowding not
/// resolved. The arch is named when both have a result. Nothing when no
/// arch has one.
pub(crate) fn result_lines(data: &AlignersState) -> Vec<ResultLine> {
    let results = &data.setup.results;
    let both = results.len() > 1;
    let mut out = Vec::new();
    let line = |text: String, warning: bool| ResultLine {
        text,
        heading: false,
        warning,
    };
    for (role, r) in results {
        let arch = match role {
            ScanRole::Upper => Arch::Upper,
            ScanRole::Lower => Arch::Lower,
            ScanRole::Bite => continue,
        };
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
            tr(
                "ws-setup-result-expansion",
                &[
                    ("posterior", &num(r.expansion_posterior_mm, 2)),
                    ("canine", &num(r.expansion_canine_mm, 2)),
                ],
            ),
            false,
        ));
        out.push(line(
            tr(
                "ws-setup-result-incisors",
                &[("mm", &signed(r.advance_mm, 2))],
            ),
            false,
        ));
        let ipr: Vec<f64> = data
            .setup
            .ipr
            .iter()
            .filter(|s| Arch::of(s.between[0]) == arch)
            .map(|s| s.amount_mm)
            .collect();
        out.push(line(
            tr(
                "ws-setup-result-ipr",
                &[
                    ("mm", &num(ipr.iter().sum(), 2)),
                    ("contacts", &ipr.len().to_string()),
                ],
            ),
            false,
        ));
        for n in &r.notes {
            if let Some(text) = serde_json::from_value::<Note>(n.clone())
                .ok()
                .as_ref()
                .and_then(note_text)
            {
                out.push(line(text, false));
            }
        }
        // `residual["affollamento"]`: rounded to 0.01 mm, shown when not zero
        let crowding = (r.residual_crowding_mm * 100.0).round() / 100.0;
        if crowding > 0.0 {
            out.push(line(
                tr("ws-setup-result-crowding", &[("mm", &num(crowding, 2))]),
                true,
            ));
        }
    }
    out
}

/// The movement of tooth `f` in its clinical frame (mesial, buccal,
/// extrusion in mm; torque, tip, rotation in degrees: `staging::describe_q`),
/// in the final position, or at stage `stage`.
pub(crate) fn movement(data: &AlignersState, f: Fdi, stage: Option<u32>) -> [f64; 6] {
    let pose = match stage {
        None => data.setup.poses.get(&f).copied(),
        Some(k) => data
            .stages
            .iter()
            .find(|s| s.index == k)
            .and_then(|s| s.poses.get(&f).copied()),
    }
    .unwrap_or(dc_case::Pose::IDENTITY);
    let md_sign = data.tooth_geometry.get(&f).map_or(1, |g| g.md_sign);
    staging::describe_q(&staging::q_from_pose(&pose), md_sign)
}

/// The notes on how predictable the movement `mv` ([`movement`]) of tooth
/// `f` is (the plugin shows them under the tooth).
pub(crate) fn flags(f: Fdi, mv: &[f64; 6]) -> Vec<MovementFlag> {
    classify::movement_flags(
        f,
        &MovementDesc {
            mesial_mm: mv[0],
            buccal_mm: mv[1],
            extrusion_mm: mv[2],
            torque_deg: mv[3],
            rotation_deg: mv[5],
        },
    )
}

/// The key of the words of a movement note.
pub(crate) fn flag_key(flag: &MovementFlag) -> String {
    format!("ws-flag-{}", flag.id)
}

impl Workspace {
    /// The result of the last final position, under the status.
    pub(crate) fn setup_result_lines(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let Some(data) = crate::overlay::aligners(&state) else {
            return;
        };
        let c = *colors(ui);
        for l in result_lines(data) {
            if l.heading {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(l.text)
                        .font(fonts::semibold(12.0))
                        .color(c.text_primary),
                );
            } else if l.warning {
                widgets::badge(ui, BadgeKind::Warning, l.text);
            } else {
                ui.label(
                    RichText::new(l.text)
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                );
            }
        }
    }

    /// The parameters of the final position, read from the case settings:
    /// a change is sent when it is made (a slider when it is let go), one
    /// undo step each; the final position is computed again only by its
    /// run button.
    pub(crate) fn setup_params(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let s = crate::overlay::aligners(&state)
            .map(|d| d.settings.setup)
            .unwrap_or_default();
        let values = serde_json::to_value(s).unwrap_or_default();
        let upper = state.case.scan(ScanRole::Upper).is_some();
        let lower = state.case.scan(ScanRole::Lower).is_some();
        let busy = self.is_busy();
        let mut commit = None;
        let drag = &mut self.setup_ui.drag;
        ui.add_enabled_ui(!busy, |ui| {
            choice(ui, &values, &SCOPE, &mut commit);
            crate::function::caption(ui, t("ws-setup-scope-hint"));
            for (title, items) in GROUPS {
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new(t(title)).font(fonts::semibold(12.0)))
                    .id_salt(*title)
                    .default_open(false)
                    .show(ui, |ui| {
                        for item in *items {
                            match item {
                                Item::Number(n) => slider(ui, &s, n, drag, &mut commit),
                                Item::Flag(field, key) => {
                                    flag(ui, &values, field, key, &mut commit)
                                }
                                Item::Choice(c) => choice(ui, &values, c, &mut commit),
                                Item::Expansion => {
                                    if upper || !lower {
                                        slider(ui, &s, &EXPANSION_UPPER, drag, &mut commit);
                                    }
                                    if lower || !upper {
                                        for n in &EXPANSION_LOWER {
                                            slider(ui, &s, n, drag, &mut commit);
                                        }
                                    }
                                }
                            }
                        }
                    });
            }
        });
        ui.add_space(6.0);
        crate::function::caption(ui, t("ws-setup-params-rerun"));
        crate::function::caption(ui, t("ws-setup-params-check"));
        if let Some(p) = commit {
            self.run_setup_settings(p);
        }
    }

    /// Runs `aligners.settings.setup` with `params`.
    pub(crate) fn run_setup_settings(&mut self, params: Value) {
        if let Some(spec) = self.specs.iter().find(|s| s.id == SETTINGS) {
            let cmd = spec.command(params);
            self.run_command(Box::new(cmd));
        }
    }

    /// The steps of the − and + buttons of "Move a tooth" (case settings).
    pub(crate) fn adjust_steps(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let s = crate::overlay::aligners(&state)
            .map(|d| d.settings.setup)
            .unwrap_or_default();
        let mut commit = None;
        let drag = &mut self.setup_ui.drag;
        slider(ui, &s, &STEP_MM, drag, &mut commit);
        slider(ui, &s, &STEP_DEG, drag, &mut commit);
        if let Some(p) = commit {
            self.run_setup_settings(p);
        }
    }

    /// "Back to the initial position": a correction of the final position
    /// that is a button (it never computes it again).
    pub(crate) fn setup_corrections(&mut self, ui: &mut Ui) {
        crate::function::caption(ui, t("ws-setup-reset-hint"));
        ui.add_space(4.0);
        let has = crate::overlay::aligners(self.undo.current())
            .is_some_and(|d| !d.setup.poses.is_empty() || !d.setup.results.is_empty());
        if Button::secondary(t("ws-setup-reset"))
            .enabled(has && !self.is_busy())
            .full_width(true)
            .show(ui)
            .clicked()
            && let Some(spec) = self.specs.iter().find(|s| s.id == RESET)
        {
            let cmd = spec.command(Value::Null);
            self.run_command(Box::new(cmd));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dca_model::{IprStep, SetupSummary};

    fn fdi(n: u8) -> Fdi {
        Fdi::new(n).unwrap()
    }

    #[test]
    fn every_numeric_parameter_has_a_range_and_a_slider() {
        let mut fields: Vec<&str> = GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .filter_map(|i| match i {
                Item::Number(n) => Some(n.field),
                _ => None,
            })
            .chain([EXPANSION_UPPER.field, STEP_MM.field, STEP_DEG.field])
            .chain(EXPANSION_LOWER.iter().map(|n| n.field))
            .collect();
        fields.sort_unstable();
        let mut all: Vec<&str> = SetupSettings::NUMBERS.iter().map(|n| n.0).collect();
        all.sort_unstable();
        assert_eq!(fields, all);
        // every value of a choice is one the case stores
        let values = serde_json::to_value(SetupSettings::default()).unwrap();
        for c in GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .filter_map(|i| match i {
                Item::Choice(c) => Some(c),
                _ => None,
            })
            .chain([&SCOPE])
        {
            for (name, _) in c.options {
                let mut v = values.clone();
                v[c.field] = json!(name);
                serde_json::from_value::<SetupSettings>(v).unwrap();
            }
            assert!(c.options.iter().any(|(n, _)| values[c.field] == *n));
        }
    }

    #[test]
    fn a_slider_value_is_sent_rounded_and_only_when_it_changes() {
        let s = SetupSettings::default();
        assert_eq!(number_patch(&s, "advance_max_mm", 1.0, 1), None);
        assert_eq!(
            number_patch(&s, "advance_max_mm", 1.5004, 1),
            Some(json!({ "advance_max_mm": 1.5 }))
        );
        assert_eq!(
            number_patch(&s, "contact_tol_mm", 0.104, 2),
            Some(json!({ "contact_tol_mm": 0.1 }))
        );
        assert_eq!(number_patch(&s, "nothing", 1.0, 1), None);
        let (lo, hi, default) = range_of("enamel_fraction");
        assert_eq!((lo, hi, default), (0.1, 0.5, 0.5));
    }

    #[test]
    fn the_result_reads_like_the_plugin_page() {
        // the language of the tests is English (the Italian words have their
        // own parity test)
        assert_eq!(dc_i18n::current(), Lang::En);
        let mut data = AlignersState::default();
        assert!(result_lines(&data).is_empty());
        data.setup.results.insert(
            ScanRole::Lower,
            SetupSummary {
                expansion_posterior_mm: 0.5,
                expansion_canine_mm: 0.25,
                advance_mm: 0.4,
                residual_crowding_mm: 1.234,
                spacing_mm: 0.0,
                notes: vec![
                    json!({ "kind": "arch_line", "drawn": false }),
                    json!({ "kind": "scope", "scope": "anterior" }),
                    json!({ "kind": "moved", "count": 4, "of": 12, "max_mm": 0.63, "mean_mm": 0.3 }),
                    json!({ "kind": "nothing_we_know" }),
                ],
            },
        );
        data.setup.ipr = vec![
            IprStep {
                between: [fdi(31), fdi(32)],
                amount_mm: 0.2,
            },
            IprStep {
                between: [fdi(41), fdi(31)],
                amount_mm: 0.3,
            },
            IprStep {
                between: [fdi(11), fdi(21)],
                amount_mm: 0.5,
            },
        ];
        let lines = result_lines(&data);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        // one arch: not named; the IPR of the arch only; the log lines the plugin shows
        assert!(lines.iter().all(|l| !l.heading));
        assert!(
            texts[0].contains("0.50") && texts[0].contains("0.25"),
            "{texts:?}"
        );
        assert!(texts[1].contains("+0.40"), "{texts:?}");
        assert!(
            texts[2].contains("0.50") && texts[2].contains('2'),
            "{texts:?}"
        );
        assert_eq!(texts.len(), 6, "{texts:?}");
        assert!(texts[4].contains("4") && texts[4].contains("12") && texts[4].contains("0.63"));
        let last = lines.last().unwrap();
        assert!(last.warning && last.text.contains("1.23"), "{last:?}");
        // both arches: each named, upper first
        data.setup
            .results
            .insert(ScanRole::Upper, SetupSummary::default());
        let lines = result_lines(&data);
        assert!(lines[0].heading);
        assert_eq!(lines.iter().filter(|l| l.heading).count(), 2);
        assert!(!lines.iter().take(5).any(|l| l.warning));
    }

    #[test]
    fn the_movement_and_its_notes_come_from_the_stored_pose() {
        crate::loader();
        let mut data = AlignersState::default();
        let f = fdi(13);
        assert_eq!(movement(&data, f, None), [0.0; 6]);
        // 20 degrees of rotation about the long axis of a canine
        let q = [0.0, 0.0, 0.0, 0.0, 0.0, 20f64.to_radians()];
        data.setup.poses.insert(f, staging::pose_from_q(&q));
        let mv = movement(&data, f, None);
        assert!((mv[5] - 20.0).abs() < 1e-9, "{mv:?}");
        let ids: Vec<&str> = flags(f, &mv).iter().map(|f| f.id).collect();
        assert_eq!(ids, ["round-rotation", "canine-rotation"]);
        // every note has its words (both languages: the parity test)
        for flag in [
            "round-rotation",
            "canine-rotation",
            "extrusion",
            "lower-incisor-intrusion",
            "torque",
            "expansion-tipping",
            "distalisation",
            "distalisation-limit",
        ] {
            let key = format!("ws-flag-{flag}");
            assert_ne!(t(&key), key, "{key}");
        }
        // at a stage without the tooth: at rest
        assert_eq!(movement(&data, f, Some(3)), [0.0; 6]);
    }
}
