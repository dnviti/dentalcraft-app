//! The inspector of Bite, the plugin's MORSO page (`ui_phases.page_morso`):
//!
//! - under the status, the report of the last bite kept in the case
//!   (`AlignersState::bite_report`): the verdict of a registration on the
//!   bite scans with its reasons to review it, the fit of every bite scan
//!   (deviation, teeth and area in common on each arch), the agreement of two
//!   scans, the interpenetration and its correction; or that the bite is an
//!   estimate;
//! - "Parameters": the bite scans of the case (side 1 and side 2) and the
//!   button that opens the scan dialog for them (`oa.import_bite`);
//! - "Manual corrections": the tool "Three points" (`oa.bite_points`) and
//!   the correction of the interpenetration (`oa.bite_correct`).
//!
//! The picking of "Three points" follows the plugin: 3 points far apart on
//! the lower arch, the same 3 in the same order on the (first) bite scan,
//! then, if the user likes, the same for the upper arch. Only the scan the
//! next point goes on is shown; Backspace takes the last point back, Enter
//! (or Apply) registers once the lower pairs are given (the upper arch is
//! then found automatically) and the twelfth point registers at once. The
//! registration is one command,
//! `aligners.occlusion.bite-registration.three-points`, one undo step.

use dc_case::{CaseState, Scan, ScanRole};
use dc_module_api::{Toast, ToastLevel};
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_viewport::Pick;
use dca_model::{AlignersState, BiteMethod, BiteRegistration, BiteVerdict, MODULE_ID};
use egui::{RichText, Ui};
use glam::{DVec3, Vec3};
use serde_json::{Value, json};

use crate::manual::{self, Action};
use crate::t;
use crate::workspace::Workspace;

/// The registration from points picked by hand.
pub(crate) const THREE_POINTS: &str = "aligners.occlusion.bite-registration.three-points";
/// The correction of the interpenetration.
pub(crate) const CORRECT: &str = "aligners.occlusion.bite-correct";

/// Where the points of a step of the picking go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickOn {
    /// On the arch (lower or upper).
    Arch(ScanRole),
    /// On the bite scan, matching the points of that arch.
    Bite(ScanRole),
}

/// The steps of the picking, in order (`OA_OT_bite_points.STEPS`).
const STEPS: [PickOn; 4] = [
    PickOn::Arch(ScanRole::Lower),
    PickOn::Bite(ScanRole::Lower),
    PickOn::Arch(ScanRole::Upper),
    PickOn::Bite(ScanRole::Upper),
];

/// Points per step.
const POINTS: usize = 3;

/// The bite scan the points are picked on: the first of the case (the
/// plugin's "Side 1"), the one the command pairs.
pub(crate) fn first_bite(state: &CaseState) -> Option<&Scan> {
    state.case.scans.iter().find(|s| s.role == ScanRole::Bite)
}

/// The points picked so far: case coordinates (for the command) and world
/// position (for the marker), per step.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BitePicks {
    pts: [Vec<(DVec3, Vec3)>; 4],
    step: usize,
}

impl BitePicks {
    /// The step the next point is for (`None`: all given).
    pub(crate) fn on(&self) -> Option<PickOn> {
        STEPS.get(self.step).copied()
    }

    /// The number of the next point of its step (1 to 3).
    pub(crate) fn next_number(&self) -> usize {
        self.pts.get(self.step).map_or(POINTS, Vec::len) + 1
    }

    /// The scan the next point goes on, the only one shown while picking.
    pub(crate) fn focus<'a>(&self, state: &'a CaseState) -> Option<&'a Scan> {
        match self.on()? {
            PickOn::Arch(role) => state.case.scan(role),
            PickOn::Bite(_) => first_bite(state),
        }
    }

    /// The lower pairs are given: the registration can run (the upper arch
    /// found automatically when its pairs are not).
    pub(crate) fn can_apply(&self) -> bool {
        self.pts[1].len() == POINTS
    }

    /// No point given.
    pub(crate) fn is_empty(&self) -> bool {
        self.pts.iter().all(Vec::is_empty)
    }

    /// A click: the point goes to the step when it is on the scan of the
    /// step; the twelfth point runs the registration.
    pub(crate) fn click(&mut self, state: &CaseState, pick: &Pick, eye: Vec3) -> Action {
        let Some(scan) = self.focus(state) else {
            return Action::Hint("ws-bite-points-no-scan");
        };
        if manual::scan_mesh_id(scan) != pick.mesh_id {
            return Action::Hint("ws-bite-points-wrong-scan");
        }
        let Some(target) = manual::target_of(state, pick, eye) else {
            return Action::None;
        };
        self.pts[self.step].push((target.point, pick.point));
        if self.pts[self.step].len() == POINTS {
            self.step += 1;
            if self.step == STEPS.len() {
                return self.finish();
            }
        }
        Action::None
    }

    /// Backspace: the last point goes, back to the step before when this
    /// one has none.
    pub(crate) fn undo(&mut self) {
        if self.pts.get(self.step).is_none_or(Vec::is_empty) && self.step > 0 {
            self.step -= 1;
        }
        if let Some(p) = self.pts.get_mut(self.step) {
            p.pop();
        }
    }

    /// The command with the points given, or a notice when the lower pairs
    /// are not complete. The points are forgotten once sent.
    pub(crate) fn finish(&mut self) -> Action {
        if !self.can_apply() {
            return Action::Hint("ws-bite-points-incomplete");
        }
        let params = self.params();
        *self = Self::default();
        Action::Run(THREE_POINTS, params)
    }

    /// The parameters of the command.
    pub(crate) fn params(&self) -> Value {
        let list = |k: usize| -> Vec<[f64; 3]> {
            self.pts[k]
                .iter()
                .map(|(p, _)| [round3(p.x), round3(p.y), round3(p.z)])
                .collect()
        };
        let mut p = json!({ "lower_points": list(0), "bite_points": list(1) });
        if self.pts[3].len() == POINTS {
            p["upper_points"] = json!(list(2));
            p["bite_points_upper"] = json!(list(3));
        }
        p
    }

    /// The markers to draw on the scan shown: world position, number, and
    /// whether it pairs the upper arch.
    pub(crate) fn markers(&self) -> Vec<(Vec3, usize, bool)> {
        let Some(on) = self.on() else {
            return Vec::new();
        };
        STEPS
            .iter()
            .enumerate()
            .filter(|(_, s)| match (on, **s) {
                (PickOn::Arch(a), PickOn::Arch(b)) => a == b,
                (PickOn::Bite(_), PickOn::Bite(_)) => true,
                _ => false,
            })
            .flat_map(|(k, s)| {
                let upper = matches!(
                    s,
                    PickOn::Arch(ScanRole::Upper) | PickOn::Bite(ScanRole::Upper)
                );
                self.pts[k]
                    .iter()
                    .enumerate()
                    .map(move |(i, (_, w))| (*w, i + 1, upper))
            })
            .collect()
    }

    /// What to do now, in words.
    pub(crate) fn hint(&self) -> String {
        let n = self.next_number().to_string();
        match self.on() {
            Some(PickOn::Arch(role)) => dc_i18n::tr_args(
                "ws-bite-points-on-arch",
                &[("n", &n), ("arch", &t(arch_key(role)))],
            ),
            Some(PickOn::Bite(role)) => dc_i18n::tr_args(
                "ws-bite-points-on-bite",
                &[("n", &n), ("arch", &t(arch_key(role)))],
            ),
            None => t("ws-bite-points-done"),
        }
    }
}

fn arch_key(role: ScanRole) -> &'static str {
    if role == ScanRole::Upper {
        "ws-bite-arch-upper"
    } else {
        "ws-bite-arch-lower"
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn mm(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

/// The words of a reason to review a registration (its code).
pub(crate) fn reason_text(code: &str, r: &BiteRegistration) -> String {
    let fit = |scan: &str, arch: &str| {
        r.scans.iter().find(|s| s.scan == scan).map(|s| {
            if arch == "upper" {
                s.upper.clone()
            } else {
                s.lower.clone()
            }
        })
    };
    let mut parts = code.splitn(3, ':');
    let kind = parts.next().unwrap_or_default();
    let scan = parts.next().unwrap_or_default();
    let arch = parts.next().unwrap_or_default();
    let arch_word = t(if arch == "upper" {
        "ws-bite-arch-upper"
    } else {
        "ws-bite-arch-lower"
    });
    match kind {
        "rms_above_good" => dc_i18n::tr_args(
            "ws-bite-why-rms",
            &[
                ("scan", scan),
                ("arch", &arch_word),
                ("rms", &mm(fit(scan, arch).map_or(0.0, |f| f.rms_mm), 2)),
            ],
        ),
        "few_teeth" => dc_i18n::tr_args(
            "ws-bite-why-teeth",
            &[
                ("scan", scan),
                ("arch", &arch_word),
                ("n", &fit(scan, arch).map_or(0, |f| f.n_teeth).to_string()),
            ],
        ),
        "full_arch_scan" => t("ws-bite-why-full-arch"),
        "one_side_only" => t("ws-bite-why-one-side"),
        "scans_disagree" => dc_i18n::tr_args(
            "ws-bite-why-disagree",
            &[("mm", &mm(r.agreement_mm.unwrap_or(0.0), 2))],
        ),
        "interpenetration" => dc_i18n::tr_args(
            "ws-bite-why-penetration",
            &[("mm", &mm(r.penetration_mm, 2))],
        ),
        "arches_not_in_contact" => dc_i18n::tr_args(
            "ws-bite-why-open",
            &[("mm", &mm(-r.opening_at_contact_mm, 2))],
        ),
        other => other.to_owned(),
    }
}

fn side_word(side: Option<&str>) -> String {
    t(match side {
        Some("right") => "ws-bite-side-right",
        Some("left") => "ws-bite-side-left",
        Some("both") => "ws-bite-side-both",
        _ => "ws-bite-side-unknown",
    })
}

/// The sides of the bite scans of a registration, in words.
fn scan_sides(r: &BiteRegistration) -> String {
    let sides: std::collections::BTreeSet<String> = r
        .scans
        .iter()
        .filter(|s| s.side.is_some())
        .map(|s| side_word(s.side.as_deref()))
        .collect();
    sides.into_iter().collect::<Vec<_>>().join(", ")
}

/// The one line of the report with the advanced options off: how many bite
/// scans, on which sides.
fn registration_line(r: &BiteRegistration) -> String {
    let sides = scan_sides(r);
    dc_i18n::tr_args(
        "ws-bite-simple-summary",
        &[
            ("n", &r.scans.len().to_string()),
            (
                "sides",
                &if sides.is_empty() {
                    side_word(None)
                } else {
                    sides
                },
            ),
        ],
    )
}

/// The lines of the report of a registration, as the plugin's page shows
/// them: the summary, the reasons to review it, every bite scan and the
/// agreement of two.
pub(crate) fn registration_lines(
    r: &BiteRegistration,
    method: BiteMethod,
    correction: f64,
) -> Vec<String> {
    let n = r.scans.len();
    let sides = scan_sides(r);
    let mut out = vec![dc_i18n::tr_args(
        if method == BiteMethod::ThreePoints {
            "ws-bite-summary-points"
        } else {
            "ws-bite-summary"
        },
        &[
            ("n", &n.to_string()),
            ("sides", &sides),
            ("rms", &mm(r.max_rms_mm(), 3)),
            ("penetration", &mm(r.penetration_mm, 2)),
        ],
    )];
    if correction > 0.0 {
        out.push(dc_i18n::tr_args(
            "ws-bite-corrected",
            &[("mm", &mm(correction, 2))],
        ));
    }
    for code in &r.reasons {
        out.push(dc_i18n::tr_args(
            "ws-bite-to-check",
            &[("why", &reason_text(code, r))],
        ));
    }
    for s in &r.scans {
        out.push(dc_i18n::tr_args(
            "ws-bite-scan-line",
            &[
                ("scan", &s.scan),
                ("side", &side_word(s.side.as_deref())),
                ("rms_lower", &mm(s.lower.rms_mm, 3)),
                ("rms_upper", &mm(s.upper.rms_mm, 3)),
                ("teeth_lower", &s.lower.n_teeth.to_string()),
                ("teeth_upper", &s.upper.n_teeth.to_string()),
                ("area_lower", &mm(s.lower.area_mm2, 0)),
                ("area_upper", &mm(s.upper.area_mm2, 0)),
            ],
        ));
    }
    if let Some(a) = r.agreement_mm {
        out.push(dc_i18n::tr_args("ws-bite-agreement", &[("mm", &mm(a, 2))]));
    }
    out
}

/// The toast for the report of a bite command.
pub(crate) fn feedback_toast(id: &str, out: &Value) -> Option<Toast> {
    match id {
        THREE_POINTS => {
            let good = out["verdict"] == "good";
            Some(Toast::new(
                if good {
                    ToastLevel::Success
                } else {
                    ToastLevel::Warning
                },
                if good {
                    "ws-toast-bite-points-good"
                } else {
                    "ws-toast-bite-points-review"
                },
            ))
        }
        CORRECT => Some(
            Toast::new(ToastLevel::Success, "ws-toast-bite-corrected").with_detail(
                dc_i18n::tr_args(
                    "ws-bite-corrected",
                    &[("mm", &mm(out["opening_mm"].as_f64().unwrap_or(0.0), 2))],
                ),
            ),
        ),
        _ => None,
    }
}

impl Workspace {
    /// The report of the last bite (the plugin's "Situazione" box).
    pub(crate) fn bite_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let c = *colors(ui);
        let line = |ui: &mut Ui, text: String| {
            ui.label(
                RichText::new(text)
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        };
        let data = state.module_data::<AlignersState>(MODULE_ID);
        match data.and_then(|d| d.bite_report.as_ref()) {
            Some(rep) => match (&rep.registration, rep.method) {
                (Some(r), method) => {
                    let good = r.verdict == BiteVerdict::Good;
                    widgets::badge(
                        ui,
                        if good {
                            BadgeKind::Success
                        } else {
                            BadgeKind::Warning
                        },
                        t(if good {
                            "ws-bite-verdict-good"
                        } else {
                            "ws-bite-verdict-review"
                        }),
                    );
                    ui.add_space(4.0);
                    if dc_module_api::advanced_ui(ui.ctx()) {
                        for text in registration_lines(r, method, rep.correction_mm) {
                            line(ui, text);
                        }
                    } else {
                        line(ui, registration_line(r));
                    }
                }
                (None, _) => {
                    widgets::badge(ui, BadgeKind::Warning, t("ws-bite-virtual"));
                    ui.add_space(4.0);
                    line(ui, t("ws-bite-virtual-detail"));
                    if rep.corrected() {
                        line(
                            ui,
                            dc_i18n::tr_args(
                                "ws-bite-corrected",
                                &[("mm", &mm(rep.correction_mm, 2))],
                            ),
                        );
                    }
                }
            },
            None if data.is_some_and(|d| d.bite_registered) => line(ui, t("ws-bite-unrecorded")),
            None => line(ui, t("ws-bite-none")),
        }
        let arches = [ScanRole::Lower, ScanRole::Upper]
            .iter()
            .all(|r| state.case.scan(*r).is_some());
        if !arches {
            ui.add_space(4.0);
            line(ui, t("ws-bite-both-arches"));
        }
    }

    /// The bite scans of the case and the button that imports them.
    pub(crate) fn bite_params(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let c = *colors(ui);
        crate::function::caption(ui, t("ws-bite-scans-hint"));
        ui.add_space(6.0);
        let bites: Vec<&Scan> = state
            .case
            .scans
            .iter()
            .filter(|s| s.role == ScanRole::Bite)
            .collect();
        if bites.is_empty() {
            ui.label(
                RichText::new(t("ws-bite-scans-none"))
                    .font(fonts::ui(13.0))
                    .color(c.text_primary),
            );
        }
        for (k, s) in bites.iter().enumerate() {
            ui.label(
                RichText::new(dc_i18n::tr_args(
                    "ws-bite-scan-slot",
                    &[("n", &(k + 1).to_string()), ("name", &s.source_name)],
                ))
                .font(fonts::ui(13.0))
                .color(c.text_primary),
            );
        }
        ui.add_space(6.0);
        if Button::secondary(t("ws-bite-import"))
            .enabled(!self.is_busy())
            .full_width(true)
            .show(ui)
            .clicked()
        {
            self.import_bite_scans();
        }
        ui.add_space(4.0);
        // what a run does with or without bite scans: an advanced hint
        if dc_module_api::advanced_ui(ui.ctx()) {
            crate::function::caption(
                ui,
                t(if bites.is_empty() {
                    "ws-bite-run-virtual"
                } else {
                    "ws-bite-run-scans"
                }),
            );
        }
    }

    /// The correction of the interpenetration, after the tools.
    pub(crate) fn bite_corrections(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let rep = state
            .module_data::<AlignersState>(MODULE_ID)
            .and_then(|d| d.bite_report.clone());
        crate::function::caption(ui, t("ws-bite-correct-hint"));
        ui.add_space(4.0);
        let label = match rep.as_ref().and_then(|r| r.registration.as_ref()) {
            Some(r) if r.penetration_mm > 0.0 => {
                dc_i18n::tr_args("ws-bite-correct-mm", &[("mm", &mm(r.penetration_mm, 2))])
            }
            _ => t("ws-bite-correct"),
        };
        let arches = [ScanRole::Lower, ScanRole::Upper]
            .iter()
            .all(|r| state.case.scan(*r).is_some());
        if Button::secondary(label)
            .enabled(!self.is_busy() && arches)
            .full_width(true)
            .show(ui)
            .clicked()
        {
            self.run_manual(CORRECT, json!({}));
        }
    }

    /// The controls of "Three points" while it is open.
    pub(crate) fn bite_points_controls(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let state = self.undo.current().clone();
        let ready = first_bite(&state).is_some()
            && state.case.scan(ScanRole::Lower).is_some()
            && state.case.scan(ScanRole::Upper).is_some();
        if !ready {
            ui.label(
                RichText::new(t("ws-bite-points-no-scan"))
                    .color(c.text_secondary)
                    .small(),
            );
        } else {
            ui.label(RichText::new(self.manual.bite.hint()).strong());
        }
        let mut undo = false;
        let mut apply = false;
        ui.horizontal_wrapped(|ui| {
            if Button::secondary(t("ws-bite-points-undo"))
                .enabled(!self.manual.bite.is_empty())
                .show(ui)
                .clicked()
            {
                undo = true;
            }
            if Button::secondary(t("ws-bite-points-apply"))
                .enabled(self.manual.bite.can_apply() && !self.is_busy())
                .show(ui)
                .clicked()
            {
                apply = true;
            }
        });
        if undo {
            self.manual.bite.undo();
        }
        if apply {
            self.bite_points_finish();
        }
    }

    /// Registers from the points given (Enter or Apply).
    pub(crate) fn bite_points_finish(&mut self) {
        let action = self.manual.bite.finish();
        if matches!(action, Action::Run(..)) {
            self.close_tool();
        }
        self.manual_action(action);
    }

    /// The keys of "Three points": Backspace takes the last point back,
    /// Enter registers, Esc closes the tool. Left alone while a text field
    /// has the keyboard.
    pub(crate) fn bite_points_keys(&mut self, ui: &Ui) {
        if self.manual.tool != Some(manual::Tool::ThreePoints)
            || ui.ctx().egui_wants_keyboard_input()
        {
            return;
        }
        let consume = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        if consume(egui::Key::Backspace) {
            self.manual.bite.undo();
        }
        if consume(egui::Key::Enter) {
            self.bite_points_finish();
        }
        if consume(egui::Key::Escape) {
            self.close_tool();
        }
    }

    /// The scan shown alone while "Three points" is open: the one the next
    /// point goes on.
    pub(crate) fn bite_points_focus(&self) -> Option<uuid::Uuid> {
        (self.manual.tool == Some(manual::Tool::ThreePoints))
            .then(|| self.manual.bite.focus(self.undo.current()).map(|s| s.id))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Event, Mods, Tool};
    use dc_command::JobCtx;
    use dca_algo::anatomy::Arch;
    use dca_algo::occlusion::ArchModel;
    use dca_model::{BiteFit, BiteReport, BiteScanFit};
    use std::sync::Arc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(120);

    fn exec(id: &str, state: &CaseState, params: Value) -> CaseState {
        dca_module::commands::find(id)
            .unwrap()
            .execute(state, &params, &JobCtx::detached())
            .unwrap_or_else(|e| panic!("{id}: {e}"))
            .0
    }

    fn workspace(state: CaseState) -> Workspace {
        let project = dc_project::Project::new(
            dc_project::AppInfo {
                version: "0.1.0".into(),
                commit: "test".into(),
            },
            crate::now_rfc3339(),
            state,
        );
        Workspace::new_headless(
            Box::new(dca_module::AlignersModule::new()),
            Some(project),
            None,
        )
    }

    fn model(state: &CaseState, arch: Arch) -> ArchModel {
        let data = state.module_data::<AlignersState>(MODULE_ID).unwrap();
        let mesh = state
            .mesh(&state.case.scan(arch.role()).unwrap().mesh)
            .unwrap();
        let v: Vec<DVec3> = mesh.positions.iter().map(|p| p.as_dvec3()).collect();
        let labels = state.mesh(&data.labels[&arch.role()]).unwrap();
        let labels = labels.vertex_labels.clone().unwrap();
        ArchModel::new(arch, &v, &mesh.tris, &labels, &data.seeds, false).unwrap()
    }

    fn report(state: &CaseState) -> Option<BiteReport> {
        state
            .module_data::<AlignersState>(MODULE_ID)
            .unwrap()
            .bite_report
            .clone()
    }

    /// The demo arches in virtual occlusion with a synthetic buccal bite
    /// scan of the right side (where the arches were), then the upper arch
    /// moved away by hand: (the bite, the case to register).
    fn bite_case() -> (CaseState, CaseState) {
        let s = exec(
            "aligners.demo",
            &CaseState::default(),
            json!({ "arch": "lower" }),
        );
        let s = exec("aligners.demo", &s, json!({ "arch": "upper", "seed": 11 }));
        let closed = exec("aligners.occlusion.virtual-bite", &s, json!({}));
        let (v, t) = dca_algo::bite::demo_bite_scan(
            &model(&closed, Arch::Lower),
            &model(&closed, Arch::Upper),
            true,
        );
        let mut with_bite = closed.clone();
        let mesh = dc_mesh::TriMesh::new(v.iter().map(|p| p.as_vec3()).collect(), t);
        let blob = with_bite.blobs.insert(mesh);
        let scan: Scan = serde_json::from_value(json!({
            "id": "b1700000-0000-4000-8000-000000000001",
            "role": "bite",
            "source_name": "bite-right.stl",
            "source_units": "mm",
            "import_xform": { "t": [0.0, 0.0, 0.0], "q": [0.0, 0.0, 0.0, 1.0] },
            "mesh": blob,
        }))
        .unwrap();
        with_bite.case.scans.push(scan);
        let moved = exec(
            "aligners.components.transform",
            &with_bite,
            json!({ "component": "scan", "arch": "upper",
                    "move": { "t": [1.5, -1.0, 2.5], "q": [0.0, 0.0, 0.0436, 0.99905] } }),
        );
        (with_bite, moved)
    }

    /// The pick of vertex `v` of `scan`.
    fn pick_on(state: &CaseState, scan: &Scan, v: usize) -> Pick {
        let mesh = state.mesh(&scan.mesh).unwrap();
        let tri = mesh
            .tris
            .iter()
            .position(|t| t.contains(&(v as u32)))
            .unwrap();
        let k = mesh.tris[tri].iter().position(|&i| i == v as u32).unwrap();
        let mut barycentric = Vec3::ZERO;
        barycentric[k] = 1.0;
        let world = state.case.world_frame.from_case.to_mat4().as_mat4()
            * scan.import_xform.to_mat4().as_mat4();
        Pick {
            mesh_id: manual::scan_mesh_id(scan),
            tri: tri as u32,
            point: world.transform_point3(mesh.positions[v]),
            normal: Vec3::Z,
            barycentric,
            distance: 50.0,
        }
    }

    /// Three points of the bite scan on the lower arch, far apart, with the
    /// vertex of the lower scan at the same place: (bite, lower) indices.
    fn three_pairs(state: &CaseState) -> Vec<(usize, usize)> {
        let bite = state.mesh(&first_bite(state).unwrap().mesh).unwrap();
        let lower = state
            .mesh(&state.case.scan(ScanRole::Lower).unwrap().mesh)
            .unwrap();
        let on_lower: Vec<(usize, usize)> = bite
            .positions
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                lower
                    .positions
                    .iter()
                    .position(|q| q.distance(*p) < 1e-3)
                    .map(|j| (i, j))
            })
            .collect();
        let at = |k: usize| bite.positions[on_lower[k].0];
        let a = (0..on_lower.len())
            .min_by(|x, y| at(*x).y.total_cmp(&at(*y).y))
            .unwrap();
        let b = (0..on_lower.len())
            .max_by(|x, y| at(*x).distance(at(a)).total_cmp(&at(*y).distance(at(a))))
            .unwrap();
        let (pa, pb) = (at(a), at(b));
        let off_line = |k: usize| {
            let d = (pb - pa).normalize();
            let w = at(k) - pa;
            (w - d * w.dot(d)).length()
        };
        let c = (0..on_lower.len())
            .max_by(|x, y| off_line(*x).total_cmp(&off_line(*y)))
            .unwrap();
        vec![on_lower[a], on_lower[b], on_lower[c]]
    }

    fn upper_positions(state: &CaseState) -> Vec<Vec3> {
        state
            .mesh(&state.case.scan(ScanRole::Upper).unwrap().mesh)
            .unwrap()
            .positions
            .clone()
    }

    fn max_gap(a: &[Vec3], b: &[Vec3]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(p, q)| p.distance(*q))
            .fold(0.0, f32::max)
    }

    fn visible(ws: &mut Workspace) -> Vec<u64> {
        ws.sync_scene_with(dc_scene::Color::rgba(0.9, 0.9, 0.9, 1.0));
        ws.view
            .meshes
            .iter()
            .filter(|m| m.visible)
            .map(|m| m.id)
            .collect()
    }

    #[test]
    fn three_points_register_the_bite_in_one_undo_step() {
        let (truth, moved) = bite_case();
        assert!(max_gap(&upper_positions(&truth), &upper_positions(&moved)) > 2.0);
        let pairs = three_pairs(&moved);
        let mut ws = workspace(moved);
        // the tool opens in the inspector of Bite and runs nothing
        assert!(ws.open_tool(Tool::ThreePoints));
        assert_eq!(ws.selected(), Some(crate::leaf::BITE));
        let state = Arc::clone(ws.state());
        let lower = state.case.scan(ScanRole::Lower).unwrap().clone();
        let bite = first_bite(&state).unwrap().clone();
        // only the lower scan is shown, at rest
        assert_eq!(visible(&mut ws), [manual::scan_mesh_id(&lower)]);
        // a click on the bite scan now is refused
        ws.manual_events(vec![Event::Click(
            pick_on(&state, &bite, pairs[0].0),
            Mods::default(),
        )]);
        assert_eq!(ws.toast_keys(), ["ws-bite-points-wrong-scan"]);
        // three points on the lower arch, one taken back and given again
        for (k, (_, l)) in pairs.iter().enumerate() {
            let click = Event::Click(pick_on(&state, &lower, *l), Mods::default());
            ws.manual_events(vec![click.clone()]);
            if k == 1 {
                ws.manual.bite.undo();
                assert_eq!(ws.manual.bite.markers().len(), 1);
                ws.manual_events(vec![click]);
            }
        }
        // now the bite scan alone, with no marker of the lower arch on it
        assert_eq!(ws.manual.bite.on(), Some(PickOn::Bite(ScanRole::Lower)));
        assert_eq!(visible(&mut ws), [manual::scan_mesh_id(&bite)]);
        assert!(ws.manual.bite.markers().is_empty());
        assert!(!ws.manual.bite.can_apply());
        for (b, _) in &pairs {
            ws.manual_events(vec![Event::Click(
                pick_on(&state, &bite, *b),
                Mods::default(),
            )]);
        }
        assert!(ws.manual.bite.can_apply());
        assert_eq!(ws.manual.bite.on(), Some(PickOn::Arch(ScanRole::Upper)));
        assert!(Arc::ptr_eq(ws.state(), &state), "nothing ran before Apply");
        // Apply (Enter): the upper arch found automatically, one command
        ws.bite_points_finish();
        assert_eq!(ws.manual.tool, None, "the tool closes");
        assert!(ws.wait_for_jobs(WAIT));
        ws.manual_feedback();
        let after = Arc::clone(ws.state());
        let gap = max_gap(&upper_positions(&truth), &upper_positions(&after));
        assert!(gap < 0.05, "{gap}");
        let rep = report(&after).unwrap();
        assert_eq!(rep.method, BiteMethod::ThreePoints);
        assert!(
            ws.toast_keys()
                .iter()
                .any(|k| k.starts_with("ws-toast-bite-points")),
            "{:?}",
            ws.toast_keys()
        );
        // the scene shows everything again
        assert!(visible(&mut ws).len() > 1);
        // one undo step
        assert!(ws.undo());
        assert!(Arc::ptr_eq(ws.state(), &state));
        assert!(ws.redo());
        assert!(Arc::ptr_eq(ws.state(), &after));
    }

    #[test]
    fn the_correction_button_opens_the_bite_in_one_undo_step() {
        let s = exec(
            "aligners.demo",
            &CaseState::default(),
            json!({ "arch": "lower" }),
        );
        let s = exec("aligners.demo", &s, json!({ "arch": "upper", "seed": 11 }));
        let closed = exec("aligners.occlusion.virtual-bite", &s, json!({}));
        // the upper arch 0.4 mm into the lower one
        let axis = model(&closed, Arch::Lower).to_case.z_axis.truncate() * -0.4;
        let pushed = exec(
            "aligners.components.transform",
            &closed,
            json!({ "component": "scan", "arch": "upper",
                    "move": { "t": [axis.x, axis.y, axis.z], "q": [0.0, 0.0, 0.0, 1.0] } }),
        );
        let mut ws = workspace(pushed);
        let before = Arc::clone(ws.state());
        ws.select_item(crate::leaf::BITE);
        assert!(
            Arc::ptr_eq(ws.state(), &before),
            "the selection runs nothing"
        );
        // the button of the manual corrections
        ws.run_manual(CORRECT, json!({}));
        assert!(ws.wait_for_jobs(WAIT));
        ws.manual_feedback();
        assert_eq!(ws.toast_keys(), ["ws-toast-bite-corrected"]);
        let after = Arc::clone(ws.state());
        let rep = report(&after).unwrap();
        assert_eq!(rep.method, BiteMethod::Virtual);
        assert!(rep.correction_mm > 0.3, "{rep:?}");
        let first = dca_algo::occlusion::first_contact(
            &model(&after, Arch::Lower),
            &model(&after, Arch::Upper),
        )
        .unwrap();
        assert!(
            first.dz.abs() <= dca_algo::bite::CORRECT_MIN_MM,
            "{first:?}"
        );
        assert!(ws.undo());
        assert!(Arc::ptr_eq(ws.state(), &before));
    }

    #[test]
    fn the_bite_scans_are_imported_from_their_button_only() {
        let mut ws = workspace(CaseState::default());
        ws.select_item(crate::leaf::BITE);
        assert_eq!(ws.dialog_requests(), 0);
        ws.import_bite_scans();
        assert_eq!(ws.dialog_requests(), 1);
        assert!(!ws.is_busy());
    }

    fn pick(mesh_id: u64, p: Vec3) -> Pick {
        Pick {
            mesh_id,
            point: p,
            tri: 0,
            barycentric: Vec3::new(1.0, 0.0, 0.0),
            distance: 1.0,
            normal: Vec3::Z,
        }
    }

    #[test]
    fn the_steps_take_three_points_each_and_backspace_goes_back() {
        let mut b = BitePicks::default();
        assert_eq!(b.on(), Some(PickOn::Arch(ScanRole::Lower)));
        assert!(b.markers().is_empty() && !b.can_apply());
        for k in 0..3 {
            b.pts[b.step].push((DVec3::splat(k as f64), Vec3::splat(k as f32)));
        }
        b.step = 1;
        assert_eq!(b.on(), Some(PickOn::Bite(ScanRole::Lower)));
        assert_eq!(b.next_number(), 1);
        // back from an empty step goes to the last point of the one before
        b.undo();
        assert_eq!(b.step, 0);
        assert_eq!(b.pts[0].len(), 2);
        assert_eq!(b.next_number(), 3);
        assert_eq!(b.markers().len(), 2);
        // too soon to register
        assert_eq!(b.finish(), Action::Hint("ws-bite-points-incomplete"));
    }

    #[test]
    fn the_parameters_carry_the_upper_pairs_only_when_complete() {
        let mut b = BitePicks::default();
        for k in 0..2 {
            for i in 0..3 {
                b.pts[k].push((DVec3::new(i as f64, k as f64, 0.123_456), Vec3::ZERO));
            }
        }
        b.step = 2;
        b.pts[2].push((DVec3::ONE, Vec3::ZERO));
        let p = b.params();
        assert_eq!(p["lower_points"][2], json!([2.0, 0.0, 0.123]));
        assert_eq!(p["bite_points"].as_array().unwrap().len(), 3);
        assert!(p.get("upper_points").is_none());
        let Action::Run(id, sent) = b.finish() else {
            panic!("not run")
        };
        assert_eq!(id, THREE_POINTS);
        assert_eq!(sent, p);
        assert_eq!(
            b,
            BitePicks::default(),
            "the points are forgotten once sent"
        );
    }

    #[test]
    fn a_click_off_the_scan_of_the_step_is_refused() {
        let state = CaseState::default();
        let mut b = BitePicks::default();
        assert_eq!(
            b.click(&state, &pick(1, Vec3::ZERO), Vec3::Z),
            Action::Hint("ws-bite-points-no-scan")
        );
    }

    #[test]
    fn the_reasons_read_as_sentences() {
        crate::loader();
        let r = BiteRegistration {
            scans: vec![BiteScanFit {
                scan: "right.stl".into(),
                side: Some("right".into()),
                lower: BiteFit {
                    rms_mm: 0.123,
                    n_teeth: 1,
                    ..BiteFit::default()
                },
                upper: BiteFit::default(),
            }],
            agreement_mm: Some(0.25),
            penetration_mm: 0.4,
            opening_at_contact_mm: -0.3,
            reasons: vec![
                "rms_above_good:right.stl:lower".into(),
                "few_teeth:right.stl:lower".into(),
                "one_side_only".into(),
                "scans_disagree".into(),
                "interpenetration".into(),
                "arches_not_in_contact".into(),
                "full_arch_scan".into(),
            ],
            ..BiteRegistration::default()
        };
        let texts: Vec<String> = r.reasons.iter().map(|c| reason_text(c, &r)).collect();
        assert!(
            texts[0].contains("0.12") && texts[0].contains("right.stl"),
            "{texts:?}"
        );
        assert!(texts[1].contains('1'), "{texts:?}");
        assert!(texts[3].contains("0.25"), "{texts:?}");
        assert!(texts[4].contains("0.40"), "{texts:?}");
        assert!(texts[5].contains("0.30"), "{texts:?}");
        for (code, text) in r.reasons.iter().zip(&texts) {
            assert_ne!(code, text, "no words for {code}");
        }
        let lines = registration_lines(&r, BiteMethod::Registration, 0.25);
        // summary, correction, 7 reasons, 1 scan, agreement
        assert_eq!(lines.len(), 1 + 1 + 7 + 1 + 1, "{lines:?}");
        let rep = BiteReport::virtual_bite();
        assert!(rep.registration.is_none());
    }
}
