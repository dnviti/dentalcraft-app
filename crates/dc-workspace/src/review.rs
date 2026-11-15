//! The inspector of "Clinical review", as the plugin's page has it
//! (`ui_phases.py` `page_revisione`):
//!
//! - under the status, the counts: to review, confirmed, with a note to
//!   check (`review.counts`);
//! - "Automatic": the run button makes the classifications again
//!   (`aligners.review.classify`; the decisions already taken stay where the
//!   computed value did not change);
//! - "Manual corrections": the filter of the list (all, to review, with a
//!   note, the tooth selected in the view: the plugin's `review_filter`),
//!   the records, and the one chosen with its method, its proposal, its
//!   note and its decision; "Confirm" (`aligners.review.decide`), the other
//!   values (`aligners.review.set-value`) and "Back to the proposal"; then
//!   the two actions on every record (`aligners.review.bulk`). Choosing a
//!   record of a tooth selects the tooth in the view, as the plugin's
//!   "Collega alla vista 3D" does. Every decision is one undo step.
//!
//! The words of the records and the filter are pure functions of the case,
//! tested without a GPU.

use dc_case::{Fdi, ScanRole};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dca_algo::review;
use dca_model::{AlignersState, ReviewKind, ReviewRecord, ReviewStatus};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::function::caption;
use crate::select::Component;
use crate::setup::{ResultLine, num, show_result_lines, signed, tr};
use crate::t;
use crate::workspace::Workspace;

/// The decision on one record.
pub(crate) const DECIDE: &str = "aligners.review.decide";
/// Another value for one record.
pub(crate) const SET_VALUE: &str = "aligners.review.set-value";
/// The decision on every record.
pub(crate) const BULK: &str = "aligners.review.bulk";

/// Which records the list shows (the plugin's `review_filter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Filter {
    /// Every record.
    #[default]
    All,
    /// The proposals still to review.
    Pending,
    /// The records with a note asking for a check.
    Notes,
    /// The records of the tooth selected in the view.
    Tooth,
}

impl Filter {
    const ALL: [Filter; 4] = [Filter::All, Filter::Pending, Filter::Notes, Filter::Tooth];

    fn key(self) -> &'static str {
        match self {
            Filter::All => "ws-rev-filter-all",
            Filter::Pending => "ws-rev-filter-pending",
            Filter::Notes => "ws-rev-filter-notes",
            Filter::Tooth => "ws-rev-filter-tooth",
        }
    }
}

/// What the inspector keeps between frames: the filter and the record
/// chosen (by key).
#[derive(Debug, Default)]
pub(crate) struct ReviewUi {
    pub filter: Filter,
    pub chosen: Option<String>,
}

/// The records `filter` shows, by index; `tooth` is the tooth selected in
/// the view (`Filter::Tooth` shows nothing without one).
pub(crate) fn filtered(records: &[ReviewRecord], filter: Filter, tooth: Option<Fdi>) -> Vec<usize> {
    records
        .iter()
        .enumerate()
        .filter(|(_, r)| match filter {
            Filter::All => true,
            Filter::Pending => r.status == ReviewStatus::Pending,
            Filter::Notes => r.needs_check(),
            Filter::Tooth => tooth.is_some() && r.fdi == tooth,
        })
        .map(|(i, _)| i)
        .collect()
}

/// The words of a value of a record (`structured`, `root_control`).
pub(crate) fn value_words(id: &str) -> String {
    let key = format!("ws-rev-value-{}", id.replace('_', "-"));
    let words = t(&key);
    if words == key { id.to_owned() } else { words }
}

fn kind_key(kind: ReviewKind) -> String {
    format!("ws-rev-kind-{}", kind.id().replace('_', "-"))
}

fn status_key(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Pending => "ws-rev-status-pending",
        ReviewStatus::Accepted => "ws-rev-status-accepted",
        ReviewStatus::Rejected => "ws-rev-status-rejected",
        ReviewStatus::Modified => "ws-rev-status-modified",
    }
}

/// The name of a record: its kind, then the tooth or the arch (the arch
/// only when the case has both).
pub(crate) fn title(r: &ReviewRecord, both: bool) -> String {
    let kind = t(&kind_key(r.kind));
    match r.fdi {
        Some(f) => tr(
            "ws-rev-title-tooth",
            &[("kind", &kind), ("fdi", &f.to_string())],
        ),
        None if both => tr(
            "ws-rev-title-arch",
            &[
                ("kind", &kind),
                (
                    "arch",
                    &t(if r.arch == ScanRole::Upper {
                        "ws-setup-result-upper"
                    } else {
                        "ws-setup-result-lower"
                    }),
                ),
            ],
        ),
        None => kind,
    }
}

/// One row of the list: the name and the value in force.
pub(crate) fn row_text(r: &ReviewRecord, both: bool) -> String {
    format!("{}: {}", title(r, both), value_words(r.in_force()))
}

fn f64_at(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or_default()
}

/// How the computed value was worked out, as the plugin's `math_detail`
/// says it: the measures and the thresholds of the rule.
pub(crate) fn method_lines(r: &ReviewRecord) -> Vec<String> {
    let d = &r.detail;
    let n = |key: &str, decimals: usize| num(f64_at(d, key), decimals);
    match r.kind {
        ReviewKind::Complexity => {
            let items = &d["items"];
            let item = |k: &str| items[k].as_u64().unwrap_or_default().to_string();
            vec![tr(
                "ws-rev-method-complexity",
                &[
                    (
                        "score",
                        &d["score"].as_u64().unwrap_or_default().to_string(),
                    ),
                    ("crowding", &item("crowding")),
                    ("rotations", &item("rotations")),
                    ("molar", &item("molar_movement")),
                    ("midline", &item("midline")),
                    ("diastema", &item("diastema")),
                ],
            )]
        }
        ReviewKind::StagingPattern => vec![tr(
            "ws-rev-method-pattern",
            &[
                ("distal", &n("distalization_mm", 2)),
                ("crowding", &n("crowding_mm", 1)),
                ("little", &n("little_mm", 1)),
                (
                    "level",
                    &value_words(d["level"].as_str().unwrap_or_default()),
                ),
            ],
        )],
        ReviewKind::AlignerSuitability => {
            let mut out = vec![tr(
                "ws-rev-method-suitability",
                &[
                    ("discrepancy", &signed(f64_at(d, "discrepancy_mm"), 2)),
                    ("distal", &n("distal_mm", 2)),
                ],
            )];
            out.extend(problems(d, "ws-rev-suitability-"));
            out
        }
        ReviewKind::MovementRisk => {
            let moves = movement_words(d);
            let mut out = vec![match d["aligners"].as_u64() {
                Some(a) => tr(
                    "ws-rev-method-risk",
                    &[("moves", &moves), ("aligners", &a.to_string())],
                ),
                None => tr("ws-rev-method-risk-unplanned", &[("moves", &moves)]),
            }];
            let flags: Vec<&str> = d["flags"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if flags.is_empty() {
                out.push(t("ws-rev-risk-none"));
            } else {
                out.extend(flags.iter().map(|f| t(&format!("ws-flag-{f}"))));
            }
            out
        }
        ReviewKind::Attachment => vec![
            t("ws-rev-method-attachment"),
            t(&format!(
                "ws-rev-attachment-{}",
                r.computed.replace('_', "-")
            )),
        ],
        ReviewKind::Segmentation => {
            let mut out = vec![tr(
                "ws-rev-method-segmentation",
                &[
                    ("md", &n("width_md", 2)),
                    ("typical_md", &n("typical_md", 1)),
                    ("bl", &n("width_bl", 2)),
                    ("typical_bl", &n("typical_bl", 1)),
                    ("height", &n("height", 2)),
                ],
            )];
            out.extend(problems(d, "ws-rev-segmentation-"));
            out
        }
        ReviewKind::ToothType => vec![tr(
            "ws-rev-method-type",
            &[
                ("assigned", &value_words(&r.computed)),
                (
                    "nearest",
                    &value_words(d["nearest"].as_str().unwrap_or_default()),
                ),
                ("distance", &n("distance_nearest", 2)),
                ("assigned_distance", &n("distance_assigned", 2)),
            ],
        )],
    }
}

/// The words of the problems listed under `problems` in the detail.
fn problems(d: &Value, prefix: &str) -> Vec<String> {
    d["problems"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|p| t(&format!("{prefix}{p}")))
                .collect()
        })
        .unwrap_or_default()
}

/// The planned movement, the components of at least 0.05: `Mesial / distal
/// +0.20 mm, Rotation +12.0°`; "none" when nothing moves.
fn movement_words(d: &Value) -> String {
    const PARTS: [(&str, &str, usize); 6] = [
        ("ws-manual-mesial", "mm", 2),
        ("ws-manual-buccal", "mm", 2),
        ("ws-manual-extrusion", "mm", 2),
        ("ws-manual-torque", "°", 1),
        ("ws-manual-tip", "°", 1),
        ("ws-manual-rotation", "°", 1),
    ];
    let mv = d["movement"].as_array();
    let parts: Vec<String> = PARTS
        .iter()
        .enumerate()
        .filter_map(|(i, (key, unit, decimals))| {
            let v = mv?.get(i)?.as_f64()?;
            (v.abs() >= 0.05).then(|| format!("{} {} {unit}", t(key), signed(v, *decimals)))
        })
        .collect();
    if parts.is_empty() {
        t("ws-rev-moves-none")
    } else {
        parts.join(", ")
    }
}

/// The note of a record, in words.
pub(crate) fn note_words(r: &ReviewRecord) -> Option<String> {
    match r.note.as_deref()? {
        review::NOTE_CHECK_NUMBERING => Some(tr(
            "ws-rev-note-check-numbering",
            &[(
                "type",
                &value_words(r.detail["nearest"].as_str().unwrap_or_default()),
            )],
        )),
        other => Some(other.to_owned()),
    }
}

/// `dd/mm/yyyy hh:mm UTC` of a Unix time.
fn when(secs: u64) -> String {
    let day = dca_algo::schedule::Date::from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("{} {:02}:{:02} UTC", day.dmy(), s / 3600, (s % 3600) / 60)
}

/// The decision on a record in words: preset and to review, or the
/// decision with the value and when.
pub(crate) fn decision_words(r: &ReviewRecord) -> String {
    match r.status {
        ReviewStatus::Pending => tr(
            "ws-rev-decision-pending",
            &[("value", &value_words(r.in_force()))],
        ),
        status => tr(
            "ws-rev-decision",
            &[
                ("status", &t(status_key(status))),
                ("value", &value_words(&r.decided)),
                ("when", &r.decided_at.map(when).unwrap_or_default()),
            ],
        ),
    }
}

/// The counts under the status: to review, decided, with a note.
pub(crate) fn result_lines(records: &[ReviewRecord]) -> Vec<ResultLine> {
    if records.is_empty() {
        return Vec::new();
    }
    let c = review::counts(records);
    let mut out = vec![ResultLine {
        text: tr(
            "ws-rev-result",
            &[
                ("pending", &c.pending.to_string()),
                ("decided", &c.decided().to_string()),
            ],
        ),
        heading: false,
        warning: false,
    }];
    if c.notes > 0 {
        out.push(ResultLine {
            text: tr("ws-rev-result-notes", &[("n", &c.notes.to_string())]),
            heading: false,
            warning: true,
        });
    }
    out
}

/// The parameters of the decision on record `key`.
pub(crate) fn decide_params(key: &str, action: &str) -> Value {
    json!({ "key": key, "action": action })
}

/// The parameters of another value for record `key`.
pub(crate) fn value_params(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value })
}

impl Workspace {
    /// The counts of the review, under the status.
    pub(crate) fn review_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let Some(data) = crate::overlay::aligners(&state) else {
            return;
        };
        show_result_lines(ui, result_lines(&data.review));
    }

    /// The records, the one chosen with its decision controls, and the
    /// actions on every record.
    pub(crate) fn review_corrections(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let data = crate::overlay::aligners(&state)
            .cloned()
            .unwrap_or_else(AlignersState::default);
        if data.review.is_empty() {
            caption(ui, t("ws-rev-empty"));
            return;
        }
        let records = &data.review;
        let both = records.iter().any(|r| r.arch == ScanRole::Upper)
            && records.iter().any(|r| r.arch == ScanRole::Lower);
        let busy = self.is_busy();
        let c = *colors(ui);
        let mut send: Option<(&'static str, Value)> = None;

        // the filter
        let labels: Vec<String> = Filter::ALL.iter().map(|f| t(f.key())).collect();
        let mut i = Filter::ALL
            .iter()
            .position(|f| *f == self.review_ui.filter)
            .unwrap_or(0);
        if widgets::segmented_fill(ui, &mut i, &labels, 24.0).changed() {
            self.review_ui.filter = Filter::ALL[i];
        }
        ui.add_space(6.0);
        let tooth = match self.selection.component {
            Some(Component::Tooth(n)) => Fdi::new(n).ok(),
            _ => None,
        };
        let shown = filtered(records, self.review_ui.filter, tooth);
        if shown.is_empty() {
            caption(
                ui,
                t(
                    if self.review_ui.filter == Filter::Tooth && tooth.is_none() {
                        "ws-rev-pick-tooth"
                    } else {
                        "ws-rev-none-shown"
                    },
                ),
            );
        }

        // the list
        let chosen_key = self.review_ui.chosen.clone();
        let mut choose: Option<(String, Option<Fdi>)> = None;
        egui::ScrollArea::vertical()
            .id_salt("ws-rev-list")
            .max_height(240.0)
            .show(ui, |ui| {
                for &k in &shown {
                    let r = &records[k];
                    let key = r.key();
                    // the status as the menu shows a step: done, to do, a warning
                    let (icon, color) = if r.needs_check() && r.status == ReviewStatus::Pending {
                        (ic::WARNING, c.status_warning)
                    } else if r.status == ReviewStatus::Pending {
                        (ic::CIRCLE, c.text_secondary)
                    } else {
                        (ic::CHECK_CIRCLE, c.status_success)
                    };
                    let selected = chosen_key.as_deref() == Some(key.as_str());
                    let clicked = ui
                        .horizontal(|ui| {
                            ui.label(RichText::new(icon).font(fonts::icon(13.0)).color(color));
                            Button::secondary(row_text(r, both))
                                .selected(selected)
                                .full_width(true)
                                .show(ui)
                                .clicked()
                        })
                        .inner;
                    if clicked {
                        choose = Some((key, r.fdi));
                    }
                    ui.add_space(2.0);
                }
            });
        if let Some((key, fdi)) = choose {
            self.review_ui.chosen = Some(key);
            if let Some(f) = fdi {
                self.select_component(Some(Component::Tooth(f.get())));
            }
        }

        // the record chosen
        let chosen = self
            .review_ui
            .chosen
            .as_deref()
            .and_then(|k| records.iter().find(|r| r.key() == k));
        if let Some(r) = chosen {
            ui.add_space(8.0);
            widgets::hairline(ui);
            ui.add_space(6.0);
            ui.label(
                RichText::new(title(r, both))
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
            caption(
                ui,
                tr("ws-rev-computed", &[("value", &value_words(&r.computed))]),
            );
            for line in method_lines(r) {
                caption(ui, line);
            }
            caption(
                ui,
                tr("ws-rev-proposed", &[("value", &value_words(&r.proposed))]),
            );
            if let Some(note) = note_words(r) {
                widgets::badge(ui, BadgeKind::Warning, note);
            }
            ui.label(
                RichText::new(decision_words(r))
                    .font(fonts::medium(12.0))
                    .color(c.text_primary),
            );
            ui.add_space(6.0);
            let key = r.key();
            if Button::primary(t("ws-rev-confirm"))
                .enabled(!busy)
                .full_width(true)
                .show(ui)
                .clicked()
            {
                send = Some((DECIDE, decide_params(&key, "confirm")));
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(t("ws-rev-choose"))
                    .font(fonts::medium(12.0))
                    .color(c.text_secondary),
            );
            ui.add_enabled_ui(!busy, |ui| {
                for option in &r.options {
                    let on = option == r.in_force();
                    let clicked = ui
                        .horizontal(|ui| {
                            let dot = widgets::radio_indicator(ui, on, ui.is_enabled());
                            let text = ui.add(
                                egui::Label::new(
                                    RichText::new(value_words(option))
                                        .font(fonts::ui(12.0))
                                        .color(c.text_primary),
                                )
                                .sense(egui::Sense::click()),
                            );
                            dot.interact(egui::Sense::click()).clicked() || text.clicked()
                        })
                        .inner;
                    if clicked && !(on && r.status != ReviewStatus::Pending) {
                        send = Some((SET_VALUE, value_params(&key, option)));
                    }
                }
            });
            if r.status != ReviewStatus::Pending {
                ui.add_space(4.0);
                if Button::secondary(t("ws-rev-reset"))
                    .enabled(!busy)
                    .full_width(true)
                    .show(ui)
                    .clicked()
                {
                    send = Some((DECIDE, decide_params(&key, "reset")));
                }
            }
        }

        // every record at once
        ui.add_space(10.0);
        widgets::hairline(ui);
        ui.add_space(6.0);
        ui.label(
            RichText::new(t("ws-rev-all"))
                .font(fonts::semibold(13.0))
                .color(c.text_primary),
        );
        caption(ui, t("ws-rev-all-hint"));
        ui.add_space(4.0);
        if Button::secondary(t("ws-rev-confirm-unflagged"))
            .enabled(!busy)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            send = Some((BULK, json!({ "mode": "confirm_unflagged" })));
        }
        ui.add_space(4.0);
        if Button::ghost(t("ws-rev-reset-all"))
            .enabled(!busy)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            send = Some((BULK, json!({ "mode": "reset" })));
        }

        if let Some((id, params)) = send {
            self.run_module_command(id, params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dca_algo::anatomy::{self, Arch};
    use dca_algo::diagnosis::classify::PatternInput;
    use dca_algo::review::{Action, ArchInput, Movement, ToothInput};

    fn fdi(n: u8) -> Fdi {
        Fdi::new(n).unwrap()
    }

    fn records() -> Vec<ReviewRecord> {
        let (md, bl) = anatomy::typical_diameters(fdi(36));
        let odd = ToothInput {
            fdi: fdi(33),
            width_md: md,
            width_bl: bl,
            height: 7.0,
            movement: Some(Movement {
                desc: [0.3, 0.0, 1.0, 0.0, 0.0, 12.0],
                aligners: Some(9),
            }),
        };
        let (md, bl) = anatomy::typical_diameters(fdi(41));
        let fine = ToothInput {
            fdi: fdi(41),
            width_md: md,
            width_bl: bl,
            height: 8.0,
            movement: Some(Movement {
                desc: [0.0; 6],
                aligners: None,
            }),
        };
        review::build_records(&ArchInput {
            arch: Arch::Lower,
            catcat_score: 24,
            catcat_items: Default::default(),
            discrepancy_mm: -9.0,
            distal_mm: 0.0,
            pattern: PatternInput {
                level_above_easy: true,
                crowding_mm: 9.0,
                little_mm: 4.0,
                distalization_mm: 0.0,
            },
            teeth: vec![odd, fine],
        })
    }

    #[test]
    fn the_filter_follows_the_plugin() {
        let mut recs = records();
        assert_eq!(filtered(&recs, Filter::All, None).len(), recs.len());
        review::decide(&mut recs[0], &Action::Confirm, 1);
        assert_eq!(filtered(&recs, Filter::Pending, None).len(), recs.len() - 1);
        let notes = filtered(&recs, Filter::Notes, None);
        assert_eq!(notes.len(), 1);
        assert_eq!(recs[notes[0]].kind, ReviewKind::ToothType);
        assert!(filtered(&recs, Filter::Tooth, None).is_empty());
        let tooth = filtered(&recs, Filter::Tooth, Some(fdi(41)));
        assert_eq!(tooth.len(), 4);
        assert!(tooth.iter().all(|&i| recs[i].fdi == Some(fdi(41))));
    }

    #[test]
    fn every_record_reads_in_words() {
        assert_eq!(dc_i18n::current(), dc_i18n::Lang::En);
        let mut recs = records();
        for r in &recs {
            // a kind, a value and a method in words, never an id or a key
            let row = row_text(r, false);
            assert!(!row.contains("ws-") && !row.contains('_'), "{row}");
            for line in method_lines(r) {
                assert!(!line.contains("ws-") && !line.is_empty(), "{line}");
            }
            for o in &r.options {
                assert!(!value_words(o).contains('_'), "{o}");
            }
        }
        let suit = recs
            .iter()
            .find(|r| r.kind == ReviewKind::AlignerSuitability)
            .unwrap();
        assert_eq!(value_words(&suit.proposed), t("ws-rev-value-no"));
        assert_eq!(method_lines(suit).len(), 2, "the reason is given");
        let risk = recs
            .iter()
            .find(|r| r.kind == ReviewKind::MovementRisk && r.fdi == Some(fdi(33)))
            .unwrap();
        let lines = method_lines(risk);
        assert!(
            lines[0].contains("+0.30") && lines[0].contains('9'),
            "{lines:?}"
        );
        assert!(lines.contains(&t("ws-flag-extrusion")), "{lines:?}");
        let quiet = recs
            .iter()
            .find(|r| r.kind == ReviewKind::MovementRisk && r.fdi == Some(fdi(41)))
            .unwrap();
        assert_eq!(method_lines(quiet)[1], t("ws-rev-risk-none"));
        let ty = recs
            .iter()
            .find(|r| r.kind == ReviewKind::ToothType && r.fdi == Some(fdi(33)))
            .unwrap();
        assert!(
            note_words(ty)
                .unwrap()
                .contains(&t("ws-rev-value-first-molar")),
            "{:?}",
            note_words(ty)
        );
        // the decision and the counts
        assert!(decision_words(&recs[0]).contains(&t("ws-rev-value-moderate")));
        review::decide(&mut recs[0], &Action::Value("easy".into()), 86_400);
        let d = decision_words(&recs[0]);
        assert!(
            d.contains(&t("ws-rev-status-modified")) && d.contains("02/01/1970 00:00 UTC"),
            "{d}"
        );
        let lines = result_lines(&recs);
        assert!(lines[0].text.contains(&(recs.len() - 1).to_string()));
        assert!(lines[1].warning);
        assert!(result_lines(&[]).is_empty());
        // both arches: the arch is named
        let mut upper = recs[0].clone();
        upper.arch = ScanRole::Upper;
        assert!(title(&upper, true).contains(&t("ws-setup-result-upper")));
        assert_eq!(title(&upper, false), t("ws-rev-kind-complexity"));
    }

    #[test]
    fn the_decisions_are_sent_to_the_registered_commands() {
        assert_eq!(
            decide_params("lower:attachment:33", "confirm"),
            json!({ "key": "lower:attachment:33", "action": "confirm" })
        );
        assert_eq!(
            value_params("lower:attachment:33", "rotation"),
            json!({ "key": "lower:attachment:33", "value": "rotation" })
        );
        for id in [DECIDE, SET_VALUE, BULK] {
            assert!(dca_module::commands::find(id).is_some(), "{id}");
        }
    }
}
