//! What the plan warns about, in the inspector of the function that made it
//! (menu law: the inspector of a function shows everything about it).
//!
//! The commands report these in their output only, the case does not keep
//! them: the aligners (`aligners.planning.staging.automatic`) say which steps
//! go over a clinical limit (`over_limit`) and which IPR no aligner carries
//! (`ipr_unscheduled`), and a mark placed on the teeth says which marks it
//! is too close to (`warnings`). The workspace keeps what the last run said,
//! together with the part of the case it is true of, and shows it under the
//! status of the function only while that part is unchanged (an undo, or
//! another run, never leaves a warning of a plan that is gone). It is not
//! saved with the project: a reopened project shows it again with the next
//! run of the function.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use dc_case::{CaseState, Fdi, ScanRole};
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind};
use dca_model::{AlignersState, Seed, StagingSummary};
use egui::Ui;
use serde_json::Value;

use crate::setup::{num, tr};
use crate::t;
use crate::workspace::Workspace;

/// The command that places, moves and deletes a mark on a tooth.
const SEEDS: &str = crate::manual::SEEDS;
/// The command that slows a tooth and computes the aligners again.
const TOOTH_SPEED: &str = crate::staging::TOOTH_SPEED_CMD;
/// The automatic staging, a step of the commands that run it.
pub(crate) const STAGING_STEP: &str = "aligners.planning.staging.automatic";

/// A step over a clinical limit.
#[derive(Debug, Clone, PartialEq)]
struct OverStep {
    fdi: Fdi,
    /// A component of the movement, or `crown`.
    what: String,
    /// The largest step (mm, or radians for torque, tip and rotation); `None`
    /// when it is not a number.
    step: Option<f64>,
    /// Its limit, in the same unit.
    limit: f64,
}

/// What the last staging of an arch warned about.
#[derive(Debug, Clone, PartialEq)]
struct ArchNotes {
    /// The result the notes go with: they hold while the case still has it.
    summary: StagingSummary,
    /// The aligners of the case when it was made (both arches).
    stages: usize,
    over_limit: Vec<OverStep>,
    /// The contacts whose IPR is in no aligner.
    ipr_unscheduled: Vec<(Fdi, Fdi)>,
}

#[derive(Debug, Default)]
struct Notes {
    staging: BTreeMap<ScanRole, ArchNotes>,
    /// The marks of the case the warnings of the last mark go with, and
    /// the warnings (already in the words of the module).
    seeds: Option<(Vec<Seed>, Vec<String>)>,
}

/// The warnings of the last runs, shared with the commands that run off the
/// UI thread (they record the warnings with the case they produce).
#[derive(Debug, Clone, Default)]
pub(crate) struct PlanWarnings(Arc<Mutex<Notes>>);

/// A block of the inspector: a title and the lines under it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Block {
    pub title: String,
    pub lines: Vec<String>,
}

fn role_of(name: &str) -> Option<ScanRole> {
    match name {
        "upper" => Some(ScanRole::Upper),
        "lower" => Some(ScanRole::Lower),
        _ => None,
    }
}

fn fdi_of(v: &Value) -> Option<Fdi> {
    Fdi::new(u8::try_from(v.as_u64()?).ok()?).ok()
}

/// The over-limit steps of the report of an arch.
fn over_limit_of(arch: &Value) -> Vec<OverStep> {
    arch["over_limit"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(OverStep {
                fdi: fdi_of(&o["fdi"])?,
                what: o["what"].as_str()?.to_owned(),
                step: o["step"].as_f64(),
                limit: o["limit"].as_f64().unwrap_or_default(),
            })
        })
        .collect()
}

/// The unscheduled IPR of the report of an arch, as pairs of teeth.
fn ipr_of(arch: &Value) -> Vec<(Fdi, Fdi)> {
    arch["ipr_unscheduled"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some((fdi_of(&r["between"][0])?, fdi_of(&r["between"][1])?)))
        .collect()
}

impl PlanWarnings {
    fn lock(&self) -> std::sync::MutexGuard<'_, Notes> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps what a staging report says (`upper` and `lower` hold the
    /// arches it computed; `state` is the case it produced). An arch the
    /// report does not hold keeps what it had.
    pub(crate) fn record_staging(&self, report: &Value, state: &CaseState) {
        let Some(data) = crate::overlay::aligners(state) else {
            return;
        };
        let mut notes = self.lock();
        for name in ["upper", "lower"] {
            let (Some(role), arch) = (role_of(name), &report[name]) else {
                continue;
            };
            if !arch.is_object() {
                continue;
            }
            let (over_limit, ipr_unscheduled) = (over_limit_of(arch), ipr_of(arch));
            let Some(summary) = data.staging_results.get(&role).cloned() else {
                notes.staging.remove(&role);
                continue;
            };
            if over_limit.is_empty() && ipr_unscheduled.is_empty() {
                notes.staging.remove(&role);
            } else {
                notes.staging.insert(
                    role,
                    ArchNotes {
                        summary,
                        stages: data.stages.len(),
                        over_limit,
                        ipr_unscheduled,
                    },
                );
            }
        }
    }

    /// Keeps the warnings of the output of command `id`, which produced
    /// `state`: the marks too close, or the staging that slowing a tooth
    /// computed again. Other commands say nothing of this kind.
    pub(crate) fn record(&self, id: &str, out: &Value, state: &CaseState) {
        match id {
            SEEDS => {
                let Some(data) = crate::overlay::aligners(state) else {
                    return;
                };
                let texts: Vec<String> = out["warnings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w.as_str().map(str::to_owned))
                    .collect();
                self.lock().seeds = Some((data.seeds.clone(), texts));
            }
            TOOTH_SPEED => self.record_staging(&out["staging"], state),
            _ => {}
        }
    }

    /// The warnings of the aligners of the case `data`, for the inspector of
    /// "Aligners": the steps over a limit, then the IPR no aligner carries,
    /// each with its arch when both arches have a result. Nothing when the
    /// last run warned of nothing, or its result is gone.
    pub(crate) fn staging_blocks(&self, data: &AlignersState) -> Vec<Block> {
        let notes = self.lock();
        let both = data.staging_results.len() > 1;
        let mut over = Vec::new();
        let mut ipr = Vec::new();
        for (role, n) in &notes.staging {
            let holds =
                data.staging_results.get(role) == Some(&n.summary) && data.stages.len() == n.stages;
            if !holds {
                continue;
            }
            let arch = |text: String| {
                if both {
                    let key = match role {
                        ScanRole::Upper => "ws-setup-result-upper",
                        _ => "ws-setup-result-lower",
                    };
                    tr("ws-warn-arch-line", &[("arch", &t(key)), ("text", &text)])
                } else {
                    text
                }
            };
            over.extend(n.over_limit.iter().map(|o| arch(over_line(o))));
            ipr.extend(n.ipr_unscheduled.iter().map(|(a, b)| {
                arch(tr(
                    "ws-warn-ipr-unscheduled",
                    &[("a", &a.to_string()), ("b", &b.to_string())],
                ))
            }));
        }
        let mut blocks = Vec::new();
        if !over.is_empty() {
            blocks.push(Block {
                title: tr(
                    "ws-warn-over-limit-title",
                    &[("n", &over.len().to_string())],
                ),
                lines: over,
            });
        }
        if !ipr.is_empty() {
            blocks.push(Block {
                title: t("ws-warn-ipr-unscheduled-title"),
                lines: ipr,
            });
        }
        blocks
    }

    /// The warning of the last mark placed, for the inspector of the tooth
    /// marks: nothing when it warned of nothing, or the marks changed since.
    pub(crate) fn seeds_blocks(&self, data: &AlignersState) -> Vec<Block> {
        let notes = self.lock();
        match &notes.seeds {
            Some((seeds, lines)) if !lines.is_empty() && *seeds == data.seeds => vec![Block {
                title: t("ws-warn-seeds-title"),
                lines: lines.clone(),
            }],
            _ => Vec::new(),
        }
    }
}

/// The words of a step over its limit: the tooth, the component, the step
/// and by how much it is over the limit, in mm or degrees.
fn over_line(o: &OverStep) -> String {
    let what = match o.what.as_str() {
        "mesiodistal" | "buccolingual" | "vertical" | "torque" | "tip" | "rotation" | "crown" => {
            t(&format!("ws-warn-comp-{}", o.what))
        }
        other => other.to_owned(),
    };
    let Some(step) = o.step.filter(|s| s.is_finite()) else {
        return tr(
            "ws-warn-over-limit-nan",
            &[("fdi", &o.fdi.to_string()), ("what", &what)],
        );
    };
    // translations and the cap of a crown point are in mm, the angles in radians
    let (k, unit) = match o.what.as_str() {
        "crown" | "mesiodistal" | "buccolingual" | "vertical" => (1.0, "mm"),
        _ => (180.0 / std::f64::consts::PI, "°"),
    };
    let (step, limit) = (step * k, o.limit * k);
    tr(
        "ws-warn-over-limit",
        &[
            ("fdi", &o.fdi.to_string()),
            ("what", &what),
            ("step", &num(step, 4)),
            ("excess", &num(step - limit, 4)),
            ("limit", &num(limit, 4)),
            ("unit", unit),
        ],
    )
}

impl Workspace {
    /// Under the status of "Find the teeth": the warning of the last tooth
    /// mark placed (two marks too close), while the marks are as it left them.
    pub(crate) fn marks_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let Some(data) = crate::overlay::aligners(&state) else {
            return;
        };
        show_blocks(ui, self.plan_warnings.seeds_blocks(data));
    }
}

/// Draws the blocks: the title as a warning badge, the lines under it.
pub(crate) fn show_blocks(ui: &mut Ui, blocks: Vec<Block>) {
    let c = *colors(ui);
    for b in blocks {
        ui.add_space(6.0);
        widgets::badge(ui, BadgeKind::Warning, b.title);
        ui.add_space(2.0);
        for line in b.lines {
            ui.label(
                egui::RichText::new(line)
                    .font(dc_ui::fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fdi(n: u8) -> Fdi {
        Fdi::new(n).unwrap()
    }

    fn state_with(summary: Option<StagingSummary>, stages: usize) -> CaseState {
        let mut data = AlignersState::default();
        if let Some(s) = summary {
            data.staging_results.insert(ScanRole::Lower, s);
        }
        data.stages = (0..stages)
            .map(|i| dca_model::Stage::new(i as u32))
            .collect();
        let mut s = CaseState::default();
        s.set_module(dca_model::MODULE_ID, dc_case::ModuleState::new(data));
        s
    }

    fn summary(aligners: u32) -> StagingSummary {
        StagingSummary {
            aligners,
            ..Default::default()
        }
    }

    fn report() -> Value {
        json!({ "lower": {
            "over_limit": [
                { "fdi": 31, "what": "mesiodistal", "step": 0.31, "limit": 0.25 },
                { "fdi": 33, "what": "rotation", "step": 0.0872664626, "limit": 0.0436332313 },
                { "fdi": 41, "what": "crown", "step": null, "limit": 0.25 },
            ],
            "ipr_unscheduled": [{ "between": [41, 31], "stage": 0 }],
        }})
    }

    #[test]
    fn the_steps_over_a_limit_are_worded_with_their_units() {
        let state = state_with(Some(summary(8)), 8);
        let w = PlanWarnings::default();
        w.record_staging(&report(), &state);
        let data = crate::overlay::aligners(&state).unwrap();
        let blocks = w.staging_blocks(data);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        let over = &blocks[0].lines;
        assert_eq!(over.len(), 3);
        assert!(
            over[0].contains("31") && over[0].contains("0.3100 mm") && over[0].contains("0.0600"),
            "{}",
            over[0]
        );
        // an angle is told in degrees, the excess is what is over the limit
        assert!(
            over[1].contains("5.0000 °") && over[1].contains("2.5000 °"),
            "{}",
            over[1]
        );
        assert!(
            over[2].contains("41") && !over[2].contains("null"),
            "{}",
            over[2]
        );
        assert!(blocks[0].title.contains('3'), "{}", blocks[0].title);
        assert!(blocks[1].lines[0].contains("41") && blocks[1].lines[0].contains("31"));
    }

    #[test]
    fn nothing_is_shown_without_warnings_or_once_the_result_is_gone() {
        let state = state_with(Some(summary(8)), 8);
        let data = crate::overlay::aligners(&state).unwrap();
        let w = PlanWarnings::default();
        // lists empty: no block
        w.record_staging(
            &json!({ "lower": { "over_limit": [], "ipr_unscheduled": [] } }),
            &state,
        );
        assert!(w.staging_blocks(data).is_empty());
        w.record_staging(&report(), &state);
        assert_eq!(w.staging_blocks(data).len(), 2);
        // the aligners were taken back (undo), or are another result: hidden
        let gone = state_with(None, 0);
        assert!(
            w.staging_blocks(crate::overlay::aligners(&gone).unwrap())
                .is_empty()
        );
        let other = state_with(Some(summary(9)), 9);
        assert!(
            w.staging_blocks(crate::overlay::aligners(&other).unwrap())
                .is_empty()
        );
        // a new run without warnings clears them
        w.record_staging(
            &json!({ "lower": { "over_limit": [], "ipr_unscheduled": [] } }),
            &state,
        );
        assert!(w.staging_blocks(data).is_empty());
    }

    #[test]
    fn slowing_a_tooth_keeps_the_warnings_of_the_staging_it_computed() {
        let state = state_with(Some(summary(8)), 8);
        let data = crate::overlay::aligners(&state).unwrap();
        let w = PlanWarnings::default();
        w.record(
            TOOTH_SPEED,
            &json!({ "fdi": 31, "speed": 0.5, "restaged": true, "staging": report() }),
            &state,
        );
        assert_eq!(w.staging_blocks(data).len(), 2);
        // not restaged: nothing to say
        w.record(
            TOOTH_SPEED,
            &json!({ "fdi": 31, "restaged": false, "staging": null }),
            &state,
        );
        assert_eq!(w.staging_blocks(data).len(), 2);
    }

    #[test]
    fn the_close_marks_are_shown_while_the_marks_are_the_same() {
        let mut data = AlignersState::default();
        data.seeds.push(Seed {
            fdi: fdi(11),
            position: glam::DVec3::new(1.0, 2.0, 3.0),
        });
        let mut state = CaseState::default();
        state.set_module(
            dca_model::MODULE_ID,
            dc_case::ModuleState::new(data.clone()),
        );
        let w = PlanWarnings::default();
        let out = json!({ "warnings": ["the marks of teeth 11 and 21 are only 2.0 mm apart"] });
        w.record(SEEDS, &out, &state);
        let blocks = w.seeds_blocks(&data);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].lines[0].contains("only 2.0 mm"));
        // another mark was placed (or the last one undone): the warning is old
        let mut moved = data.clone();
        moved.seeds[0].position.x += 1.0;
        assert!(w.seeds_blocks(&moved).is_empty());
        // a placement without warnings clears them
        w.record(SEEDS, &json!({ "warnings": [] }), &state);
        assert!(w.seeds_blocks(&data).is_empty());
    }
}
