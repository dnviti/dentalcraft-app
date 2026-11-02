//! What the workspace shows around the menu leaves: the notice before a long
//! job, the sentence after a command whose output matters, the action button
//! and the results of the case analysis in the inspector.
//!
//! The commands run through the same registered specs as the CLI and the MCP
//! server; the workspace only reads their JSON output.

use dc_case::{CaseState, ScanRole};
use dc_i18n::Lang;
use dc_module_api::{LeafInput, MenuItem, Toast, ToastLevel};
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind, Button};
use dca_algo::diagnosis::{Level, LittleCategory};
use dca_model::{AlignersState, ArchSpace, MODULE_ID};
use egui::{RichText, Ui};
use serde_json::Value;

use crate::workspace::Workspace;
use crate::{import, t};

/// "Run everything automatically".
pub(crate) const AUTO: &str = "aligners.auto";
/// "Bite".
pub(crate) const BITE: &str = "aligners.occlusion.align";
/// "Case analysis".
pub(crate) const CASE_ANALYSIS: &str = "aligners.diagnosis.space-analysis";
/// "Export for printing".
pub(crate) const EXPORT_ALL: &str = "aligners.output.export-all";
/// "Control visit".
pub(crate) const VISITS: &str = "aligners.followup.visits";
/// "Find the teeth".
pub(crate) const FIND_TEETH: &str = "aligners.teeth.find";

/// The parameters leaf `id` runs with when it is clicked: none for most
/// leaves; "Find the teeth" takes the third-molars box and the missing teeth
/// of its inspector, which are settings of the case.
pub(crate) fn run_params(id: &str, state: &CaseState) -> Value {
    match id {
        FIND_TEETH => state
            .module_data::<AlignersState>(MODULE_ID)
            .map(|d| d.settings.find_teeth.clone())
            .unwrap_or_default()
            .leaf_params(),
        _ => Value::Null,
    }
}

/// i18n key of the name of the job that runs leaf `id`, shown in the status
/// bar.
pub(crate) fn job_name_key(id: &str) -> &'static str {
    match id {
        EXPORT_ALL => "ws-job-export",
        AUTO => "ws-job-auto",
        _ => "ws-job-command",
    }
}

/// `false` for a leaf whose command writes files and leaves the case as it
/// is: it makes no undo step.
pub(crate) fn changes_case(id: &str) -> bool {
    id != EXPORT_ALL
}

/// The notice that warns, when leaf `id` starts, that it can take long: the
/// keys of its title and of the line that says so.
pub(crate) fn long_notice(id: &str) -> Option<(&'static str, &'static str)> {
    match id {
        EXPORT_ALL => Some(("ws-toast-export-long", "ws-export-long")),
        AUTO => Some(("ws-toast-auto-long", "ws-auto-long")),
        _ => None,
    }
}

fn tr_args(key: &str, args: &[(&str, &str)]) -> String {
    crate::loader();
    dc_i18n::tr_args(key, args)
}

/// A notification built from the output of a command.
pub(crate) struct Reported {
    pub toast: Toast,
    /// Stays until the user closes it.
    pub sticky: bool,
}

/// The notification the output `out` of leaf `id` calls for, if any (the
/// pipeline has its own, see `Workspace::show_auto_report`).
pub(crate) fn report(id: &str, out: &Value) -> Option<Reported> {
    match id {
        BITE => {
            let scan = out["method"] == "bite_scan";
            let (level, title, detail) = if scan {
                (
                    ToastLevel::Success,
                    "ws-toast-bite-scan",
                    "ws-bite-detail-scan",
                )
            } else {
                (
                    ToastLevel::Warning,
                    "ws-toast-bite-estimated",
                    "ws-bite-detail-estimated",
                )
            };
            Some(Reported {
                toast: Toast::new(level, title).with_detail(t(detail)),
                sticky: !scan,
            })
        }
        EXPORT_ALL => {
            let dir = out["dir"].as_str().unwrap_or_default();
            // the files listed, and the manifest that closes the folder
            let count = out["files"].as_array().map_or(0, Vec::len) + 1;
            Some(Reported {
                toast: Toast::new(ToastLevel::Success, "ws-toast-export-done").with_detail(
                    tr_args(
                        "ws-export-done-detail",
                        &[("count", &count.to_string()), ("dir", dir)],
                    ),
                ),
                sticky: true,
            })
        }
        VISITS => {
            let on = out["on_track"].as_u64();
            let teeth = out["teeth"].as_u64();
            let detail = match (on, teeth) {
                (Some(on), Some(teeth)) => tr_args(
                    "ws-visit-detail",
                    &[("on", &on.to_string()), ("teeth", &teeth.to_string())],
                ),
                _ => String::new(),
            };
            let mut toast = Toast::new(ToastLevel::Success, "ws-toast-visit-added");
            if !detail.is_empty() {
                toast = toast.with_detail(detail);
            }
            Some(Reported {
                toast,
                sticky: false,
            })
        }
        _ => None,
    }
}

/// The text of a duration in seconds: `45 s`, `3 min 20 s`.
pub(crate) fn duration_text(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// A number with one decimal and the decimal mark of the language.
fn mm(v: f64) -> String {
    let text = format!("{v:.1}");
    match dc_i18n::current() {
        Lang::It => text.replace('.', ","),
        Lang::En => text,
    }
}

/// A number with a sign and one decimal.
fn signed(v: f64) -> String {
    let m = mm(v);
    if v > 0.05 { format!("+{m}") } else { m }
}

/// What the dialog of an input asks for.
fn dialog_title_key(input: LeafInput) -> &'static str {
    match input {
        LeafInput::Folder { .. } => "ws-dialog-choose-folder",
        LeafInput::ScanFile { .. } => "ws-dialog-choose-scan",
    }
}

/// The folder or the file the user picks for `input` (blocks: run it on its
/// own thread). `None` when the dialog is cancelled.
pub(crate) fn pick(input: LeafInput) -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(t(dialog_title_key(input)));
    match input {
        LeafInput::Folder { .. } => dialog.pick_folder(),
        LeafInput::ScanFile { .. } => dialog
            .add_filter(t("ws-filter-meshes"), &import::DIALOG_EXTENSIONS)
            .pick_file(),
    }
}

/// Label of the action button of leaf `id`.
fn button_key(id: &str, input: Option<LeafInput>, done: bool) -> &'static str {
    if dc_module_api::is_import_scans_command(id) {
        return "ws-btn-choose-scans";
    }
    match (id, input) {
        (EXPORT_ALL, _) => "ws-btn-export",
        (VISITS, _) => "ws-btn-visit",
        (_, Some(LeafInput::Folder { .. })) => "ws-btn-choose-folder",
        (_, Some(LeafInput::ScanFile { .. })) => "ws-btn-choose-scan",
        _ if done => "ws-btn-run-again",
        _ => "ws-btn-run",
    }
}

impl Workspace {
    /// What the inspector adds under the module's text for the selected menu
    /// entry: the action button of a leaf and, for the case analysis, its
    /// results.
    pub(crate) fn leaf_inspector(&mut self, ui: &mut Ui, item: Option<&MenuItem>) {
        let Some(item) = item else { return };
        let Some(id) = item.command_id() else { return };
        let ready = self
            .commands
            .iter()
            .any(|c| c.id == id && c.status == dc_module_api::MenuStatus::Ready);
        // a manual tool opens its own panel
        if !ready || crate::manual::Tool::from_command(id).is_some() {
            return;
        }
        let c = *colors(ui);
        ui.add_space(12.0);
        let done = self.is_done(id);
        let label = t(button_key(id, item.input, done));
        let enabled = !self.is_busy();
        let button = if done && item.input.is_none() && !dc_module_api::is_import_scans_command(id)
        {
            Button::secondary(label)
        } else {
            Button::primary(label)
        };
        if button.enabled(enabled).full_width(true).show(ui).clicked() {
            self.activate(id);
        }
        if let Some((_, line)) = long_notice(id) {
            ui.add_space(6.0);
            ui.label(
                RichText::new(t(line))
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        }
        if id == CASE_ANALYSIS {
            let state = self.undo.current().clone();
            ui.add_space(16.0);
            case_analysis(ui, &state);
        }
    }
}

/// The main figures of the case analysis, one card per arch and the ratios
/// and the complexity of the case.
fn case_analysis(ui: &mut Ui, state: &CaseState) {
    let c = *colors(ui);
    let data = state.module_data::<AlignersState>(MODULE_ID);
    let diagnosis = data.map(|d| &d.diagnosis).filter(|d| !d.arches.is_empty());
    let Some(d) = diagnosis else {
        ui.label(
            RichText::new(t("ws-ca-not-run"))
                .font(fonts::ui(12.0))
                .color(c.text_secondary),
        );
        return;
    };
    ui.label(
        RichText::new(t("ws-ca-title"))
            .font(fonts::semibold(13.0))
            .color(c.text_primary),
    );
    ui.add_space(6.0);
    for (role, space) in &d.arches {
        let name = match role {
            ScanRole::Upper => "ws-role-upper",
            ScanRole::Lower => "ws-role-lower",
            ScanRole::Bite => continue,
        };
        arch_card(ui, &t(name), space);
        ui.add_space(8.0);
    }
    // the ratios need both arches; when a tooth of the segment is missing in
    // one of them they are not calculated, and the card says so
    if d.arches.len() > 1 || d.bolton_anterior.is_some() || d.bolton_overall.is_some() {
        card(ui, |ui| {
            ui.label(
                RichText::new(t("ws-ca-bolton"))
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
            let ratio = |v: Option<f64>| {
                v.map_or_else(|| t("ws-ca-bolton-none"), |v| format!("{} %", mm(v)))
            };
            figure(ui, &t("ws-ca-bolton-anterior"), ratio(d.bolton_anterior));
            figure(ui, &t("ws-ca-bolton-overall"), ratio(d.bolton_overall));
            if d.bolton_anterior.is_none() || d.bolton_overall.is_none() {
                ui.label(
                    RichText::new(t("ws-ca-bolton-none-why"))
                        .font(fonts::ui(12.0))
                        .color(c.text_secondary),
                );
            }
        });
        ui.add_space(8.0);
    }
    if let Some(score) = d.complexity_score {
        card(ui, |ui| {
            ui.label(
                RichText::new(t("ws-ca-complexity"))
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{score:.0}"))
                        .font(fonts::medium(13.0))
                        .color(c.text_primary),
                );
                let (kind, key) = match Level::of(score.max(0.0).round() as u32) {
                    Level::Easy => (BadgeKind::Success, "ws-ca-level-easy"),
                    Level::Moderate => (BadgeKind::Info, "ws-ca-level-moderate"),
                    Level::Difficult => (BadgeKind::Warning, "ws-ca-level-difficult"),
                    Level::VeryComplex => (BadgeKind::Danger, "ws-ca-level-very-complex"),
                };
                widgets::badge(ui, kind, t(key));
            });
        });
    }
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let c = *colors(ui);
    egui::Frame::new()
        .fill(c.bg_raised)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
}

fn figure(ui: &mut Ui, name: &str, value: String) {
    let c = *colors(ui);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(name)
                .font(fonts::ui(12.0))
                .color(c.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(fonts::medium(12.0))
                    .color(c.text_primary),
            );
        });
    });
}

fn arch_card(ui: &mut Ui, name: &str, space: &ArchSpace) {
    let c = *colors(ui);
    card(ui, |ui| {
        ui.label(
            RichText::new(name)
                .font(fonts::semibold(13.0))
                .color(c.text_primary),
        );
        figure(
            ui,
            &t("ws-ca-available"),
            format!("{} mm", mm(space.available_mm)),
        );
        figure(
            ui,
            &t("ws-ca-required"),
            format!("{} mm", mm(space.required_mm)),
        );
        let gap = space.required_mm - space.available_mm;
        figure(ui, &t("ws-ca-difference"), format!("{} mm", signed(gap)));
        ui.horizontal(|ui| {
            let (kind, key) = if gap > 0.5 {
                (BadgeKind::Warning, "ws-ca-crowding")
            } else if gap < -0.5 {
                (BadgeKind::Info, "ws-ca-spacing")
            } else {
                (BadgeKind::Success, "ws-ca-balanced")
            };
            widgets::badge(ui, kind, t(key));
        });
        figure(
            ui,
            &t("ws-ca-little"),
            format!("{} mm", mm(space.little_index_mm)),
        );
        let (kind, key) = match LittleCategory::of(space.little_index_mm) {
            LittleCategory::Perfect => (BadgeKind::Success, "ws-ca-little-perfect"),
            LittleCategory::Minimal => (BadgeKind::Success, "ws-ca-little-minimal"),
            LittleCategory::Moderate => (BadgeKind::Info, "ws-ca-little-moderate"),
            LittleCategory::Severe => (BadgeKind::Warning, "ws-ca-little-severe"),
            LittleCategory::VerySevere => (BadgeKind::Danger, "ws-ca-little-very-severe"),
        };
        widgets::badge(ui, kind, t(key));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_export_is_the_one_leaf_that_changes_no_state() {
        assert!(!changes_case(EXPORT_ALL));
        assert!(changes_case(AUTO) && changes_case(BITE) && changes_case(VISITS));
    }

    #[test]
    fn durations_read_in_seconds_and_minutes() {
        assert_eq!(duration_text(0.4), "0 s");
        assert_eq!(duration_text(44.6), "45 s");
        assert_eq!(duration_text(200.0), "3 min 20 s");
    }

    #[test]
    fn the_bite_says_what_it_rests_on_and_the_estimate_stays() {
        let scan = report(BITE, &json!({ "method": "bite_scan" })).unwrap();
        assert_eq!(scan.toast.message_key, "ws-toast-bite-scan");
        assert!(!scan.sticky);
        let est = report(BITE, &json!({ "method": "virtual_bite" })).unwrap();
        assert_eq!(est.toast.message_key, "ws-toast-bite-estimated");
        assert_eq!(est.toast.level, ToastLevel::Warning);
        assert!(est.sticky);
    }

    #[test]
    fn the_export_names_the_folder_and_counts_the_manifest() {
        let r = report(
            EXPORT_ALL,
            &json!({ "dir": "/tmp/out", "files": [{}, {}, {}] }),
        )
        .unwrap();
        let detail = r.toast.detail.unwrap();
        assert!(
            detail.contains("/tmp/out") && detail.contains('4'),
            "{detail}"
        );
        assert!(r.sticky);
        assert!(report("aligners.teeth.build", &json!({})).is_none());
    }
}
