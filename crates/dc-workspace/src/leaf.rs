//! What the workspace shows around the menu leaves: the notice before a long
//! job, the sentence after a command whose output matters, the label of the
//! run button and the parts of the inspector of some functions (the
//! separation parameters and result, the results of the case analysis; see
//! `function` for the layout).
//!
//! The commands run through the same registered specs as the CLI and the MCP
//! server; the workspace only reads their JSON output.

use std::sync::{Arc, Weak};

use dc_case::{CaseState, ScanRole};
use dc_i18n::Lang;
use dc_module_api::{LeafInput, Toast, ToastLevel};
use dc_ui::fonts;
use dc_ui::theme::colors;
use dc_ui::widgets::{self, BadgeKind};
use dca_algo::diagnosis::edit::{self, EditField};
use dca_algo::diagnosis::{Level, LittleCategory};
use dca_model::{AlignersState, ArchSpace, Diagnosis, MODULE_ID, SegmentationSettings};
use egui::{RichText, Ui};
use serde_json::Value;

use crate::workspace::Workspace;
use crate::{import, t};

/// "Run everything automatically".
pub(crate) const AUTO: &str = "aligners.auto";
/// "Aligners".
pub(crate) const ALIGNERS: &str = "aligners.planning.aligners";
/// "Clinical review".
pub(crate) const REVIEW: &str = "aligners.review.classify";
/// "Bite".
pub(crate) const BITE: &str = "aligners.occlusion.align";
/// "Case analysis".
pub(crate) const CASE_ANALYSIS: &str = "aligners.diagnosis.space-analysis";
/// "Export for printing".
pub(crate) const EXPORT_ALL: &str = "aligners.output.export-all";
/// "Print": lists the aligners to print; its output opens the print popup.
pub(crate) const PRINT: &str = "aligners.production.print";
/// Records aligners as sent to the printer (after the popup saved their
/// files).
pub(crate) const PRINT_MARK: &str = "aligners.print.mark";
/// "Control visit".
pub(crate) const VISITS: &str = "aligners.followup.visits";
/// "3D teeth".
pub(crate) const BUILD_TEETH: &str = "aligners.teeth.build";
/// "Find the teeth".
pub(crate) const FIND_TEETH: &str = "aligners.teeth.find";
/// "Separate teeth and gum".
pub(crate) const SEGMENT: &str = "aligners.preparation.segmentation.automatic";
/// The check of the separated teeth (run by the pipeline after the
/// separation): its output lists what it left out of date too.
pub(crate) const SEGMENT_CHECK: &str = "aligners.preparation.segmentation.check";
/// "Final position".
pub(crate) const SETUP: &str = "aligners.planning.setup.automatic";
/// "Study models".
pub(crate) const STUDY_MODELS: &str = "aligners.output.study-models";
/// The arch of a scan chosen by hand, and its occlusal side turned over
/// (the manual corrections of "Import scans").
pub(crate) const SET_ARCH: &str = "aligners.case.set-arch";
/// "Close with a base" (the manual corrections of "Import scans").
pub(crate) const MAKE_BASE: &str = "aligners.case.make-base";
/// The values of the case analysis written by hand (the manual corrections
/// of "Case analysis").
pub(crate) const DIAGNOSIS_EDIT: &str = "aligners.diagnosis.edit-values";
/// The separation parameters stored in the case.
const SEGMENT_SETTINGS: &str = "aligners.settings.segmentation";

/// The parameters leaf `id` runs with: none for most leaves; "Find the
/// teeth" takes the third-molars box and the missing teeth of its inspector,
/// which are settings of the case.
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
        SEGMENT => "ws-job-segment",
        BUILD_TEETH => "ws-job-teeth",
        _ => "ws-job-command",
    }
}

/// `false` for a leaf whose command leaves the case as it is (it lists or
/// shows something): it makes no undo step. The export records its folder
/// in the case, so the print finds the shells.
pub(crate) fn changes_case(id: &str) -> bool {
    id != PRINT
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
            // not applicable (one arch: there is no bite): nothing was put
            // in occlusion, nothing was estimated, nothing to warn about;
            // the inspector says why
            if out["applies"] == false {
                return None;
            }
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
        BUILD_TEETH => Some(teeth_report(out)),
        SEGMENT => Some(Reported {
            toast: Toast::new(ToastLevel::Success, "ws-toast-seg-done")
                .with_detail(segment_lines(out).join("\n")),
            sticky: false,
        }),
        SET_ARCH => arch_report(out),
        MAKE_BASE => Some(base_report(out)),
        CASE_ANALYSIS => diagnosis_reset_report(out),
        DIAGNOSIS_EDIT => Some(diagnosis_edit_report(out)),
        // `oa.classify`: "N classificazioni preimpostate da rivedere", and the
        // aligners made again with the pattern of the review
        REVIEW => {
            let pending = out["counts"]["pending"].as_u64().unwrap_or_default();
            let mut detail = tr_args("ws-rev-done-detail", &[("n", &pending.to_string())]);
            if out["restaged"].as_array().is_some_and(|a| !a.is_empty()) {
                detail = format!("{detail} {}", t("ws-rev-done-restaged"));
            }
            Some(Reported {
                toast: Toast::new(ToastLevel::Success, "ws-toast-review-done").with_detail(detail),
                sticky: false,
            })
        }
        // the aligner of the view: the exact one built, the notes of a build
        crate::aligner::SHOW | crate::aligner::EXACT => crate::aligner::report(id, out),
        _ => None,
    }
}

/// The text key of the level of a complexity score.
fn level_key(level: Level) -> &'static str {
    match level {
        Level::Easy => "ws-ca-level-easy",
        Level::Moderate => "ws-ca-level-moderate",
        Level::Difficult => "ws-ca-level-difficult",
        Level::VeryComplex => "ws-ca-level-very-complex",
    }
}

/// What the correction of the case analysis by hand gave: the complexity
/// now, and that the aligners planned with the old values are to be planned
/// again.
fn diagnosis_edit_report(out: &Value) -> Reported {
    let score = out["catcat_score"].as_u64().unwrap_or(0);
    let level = t(level_key(Level::of(u32::try_from(score).unwrap_or(0))));
    let mut lines = vec![tr_args(
        "ws-diag-edited-detail",
        &[("score", &score.to_string()), ("level", &level)],
    )];
    if out["staging_outdated"] == true {
        lines.push(t("ws-diag-edited-stale"));
    }
    Reported {
        toast: Toast::new(ToastLevel::Success, "ws-toast-diag-edited")
            .with_detail(lines.join("\n")),
        sticky: false,
    }
}

/// A new case analysis writes the diagnosis anew: the values the clinician
/// had set by hand in the arches it analysed are gone, and the toast says
/// which. Nothing when there were none.
fn diagnosis_reset_report(out: &Value) -> Option<Reported> {
    let mut names: Vec<String> = Vec::new();
    for arch in ["upper", "lower"] {
        for key in out[arch]["hand_values_reset"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(field) = key.as_str().and_then(EditField::from_key) else {
                continue;
            };
            let name = t(edit_label_key(field));
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return None;
    }
    Some(Reported {
        toast: Toast::new(ToastLevel::Info, "ws-toast-diag-reset").with_detail(tr_args(
            "ws-diag-reset-detail",
            &[("values", &names.join(", "))],
        )),
        sticky: false,
    })
}

/// What "Choose the arch" and "Flip the occlusal side" did: the arch the
/// scan took (its marks and colours renumbered, its 3D teeth built again),
/// the occlusal side turned over (the separation to redo). Nothing for a
/// role only confirmed.
fn arch_report(out: &Value) -> Option<Reported> {
    let mut lines = Vec::new();
    let mut title = None;
    if out["renumbered"] == true {
        let arch = out["to"].as_str().unwrap_or_default();
        lines.push(tr_args("ws-arch-set-detail", &[("arch", arch)]));
        let rebuilt = out["rebuilt"].as_u64().unwrap_or(0);
        if rebuilt > 0 {
            let count = rebuilt.to_string();
            lines.push(tr_args("ws-arch-rebuilt-detail", &[("count", &count)]));
        }
        title = Some("ws-toast-arch-set");
    }
    if out["flipped"] == true {
        lines.push(t(if out["occlusal_flipped"] == true {
            "ws-flipped-detail"
        } else {
            "ws-unflipped-detail"
        }));
        title = title.or(Some("ws-toast-flipped"));
    }
    let title = title?;
    Some(Reported {
        toast: Toast::new(ToastLevel::Success, title).with_detail(lines.join("\n")),
        sticky: false,
    })
}

/// What "Close with a base" did, or that the scan was already closed.
fn base_report(out: &Value) -> Reported {
    let toast = if out["already_closed"] == true {
        Toast::new(ToastLevel::Info, "ws-toast-base-closed")
    } else {
        let edges = out["closed_edges"].as_u64().unwrap_or(0).to_string();
        Toast::new(ToastLevel::Success, "ws-toast-base-done")
            .with_detail(tr_args("ws-base-detail", &[("edges", &edges)]))
    };
    Reported {
        toast,
        sticky: false,
    }
}

/// The output of the step `id` in the `steps` of a wrapper command.
pub(crate) fn step_output<'a>(out: &'a Value, id: &str) -> Option<&'a Value> {
    out["steps"]
        .as_array()?
        .iter()
        .find(|s| s["id"] == id)
        .map(|s| &s["output"])
}

/// What one arch of a run of "Separate teeth and gum" gave: its teeth and
/// how long it took.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SegRun {
    pub role: ScanRole,
    pub teeth: u64,
    pub seconds: f64,
}

/// The arches of the output `out` of "Separate teeth and gum", upper first.
pub(crate) fn seg_runs(out: &Value) -> Vec<SegRun> {
    [("upper", ScanRole::Upper), ("lower", ScanRole::Lower)]
        .into_iter()
        .filter_map(|(name, role)| {
            let a = out.get(name)?;
            Some(SegRun {
                role,
                teeth: a["teeth"].as_u64()?,
                seconds: a["seconds"].as_f64().unwrap_or(0.0),
            })
        })
        .collect()
}

/// The arches of the last runs the case holds, upper first.
pub(crate) fn case_seg_runs(data: Option<&AlignersState>) -> Vec<SegRun> {
    let Some(data) = data else {
        return Vec::new();
    };
    [ScanRole::Upper, ScanRole::Lower]
        .into_iter()
        .filter_map(|role| {
            let r = data.segmentation_runs.get(&role)?;
            Some(SegRun {
                role,
                teeth: u64::from(r.teeth),
                seconds: r.seconds,
            })
        })
        .collect()
}

/// The result lines of `runs`: "<n> teeth separated in <s> s", with the
/// name of the arch in front when more than one arch ran.
pub(crate) fn seg_lines(runs: &[SegRun]) -> Vec<String> {
    let both = runs.len() > 1;
    runs.iter()
        .map(|r| {
            let count = r.teeth.to_string();
            let time = duration_text(r.seconds);
            if both {
                let arch = t(match r.role {
                    ScanRole::Upper => "ws-arch-upper",
                    _ => "ws-arch-lower",
                });
                tr_args(
                    "ws-seg-result-arch",
                    &[("arch", &arch), ("count", &count), ("time", &time)],
                )
            } else {
                tr_args("ws-seg-result", &[("count", &count), ("time", &time)])
            }
        })
        .collect()
}

/// The lines of the success toast of "Separate teeth and gum": one per
/// arch, or the totals when the output has no arch.
fn segment_lines(out: &Value) -> Vec<String> {
    let runs = seg_runs(out);
    if !runs.is_empty() {
        return seg_lines(&runs);
    }
    vec![tr_args(
        "ws-seg-result",
        &[
            ("count", &out["teeth"].as_u64().unwrap_or(0).to_string()),
            (
                "time",
                &duration_text(out["seconds"].as_f64().unwrap_or(0.0)),
            ),
        ],
    )]
}

/// The notice "Results out of date" for the results a new separation of the
/// teeth left to redo (`invalidated` of the separation and of its check),
/// listed in the words of the scan import. `None` when nothing is listed.
pub(crate) fn outdated_toast(out: &Value) -> Option<Toast> {
    let ids: Vec<&str> = out["invalidated"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let ordered: Vec<&str> = dca_model::CLEARED_IDS
        .into_iter()
        .filter(|id| ids.contains(id))
        .collect();
    if ordered.is_empty() {
        return None;
    }
    let list = import::cleared_list(MODULE_ID, &ordered);
    Some(
        Toast::new(ToastLevel::Info, "ws-toast-marks-outdated")
            .with_detail(tr_args("ws-seg-outdated-detail", &[("list", &list)])),
    )
}

/// One notice per entry of `warnings` (sentences in the language of the
/// app) of the whole pipeline or of the export: what to check before the
/// aligners are delivered.
pub(crate) fn warning_toasts(out: &Value) -> Vec<Toast> {
    out["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|w| !w.is_empty())
        .map(|w| Toast::new(ToastLevel::Warning, "ws-toast-to-check").with_detail(w))
        .collect()
}

/// [`warning_toasts`] of every step of a leaf that runs several commands
/// (`steps`, each with its `output`): what "Find the teeth" left in doubt on
/// the teeth it found (the base of a model, a stretch with no crown, a low
/// stretch), said as soon as it ends.
pub(crate) fn step_warning_toasts(out: &Value) -> Vec<Toast> {
    out["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| warning_toasts(&s["output"]))
        .collect()
}

/// One notice per doubt the automatic numbering left on the tooth marks the
/// case holds (`numbering_doubts`, by arch, in words): the notices "to
/// check" that Find the teeth said when it ended, said again when a project
/// is opened with its marks still in doubt (they go when the marks change).
pub(crate) fn numbering_doubt_toasts(state: &CaseState) -> Vec<Toast> {
    state
        .module_data::<AlignersState>(MODULE_ID)
        .into_iter()
        .flat_map(|d| d.numbering_doubts.values().flatten())
        .filter(|w| !w.is_empty())
        .map(|w| Toast::new(ToastLevel::Warning, "ws-toast-to-check").with_detail(w.as_str()))
        .collect()
}

/// What "3D teeth" built: the teeth built per arch and the gingivas. Read
/// from `arches` (per arch: `rebuilt` the tooth numbers built this time,
/// else `teeth` their count, and `gingiva`), so a partial rebuild counts the
/// teeth it rebuilt; the old output (the crowns and gingiva steps) is the
/// fallback. A field the output lacks counts as nothing. An arch the build
/// could not build (`built: false`) makes the toast a warning that names it
/// with its reason.
fn teeth_report(out: &Value) -> Reported {
    let (upper, lower, gum) = match out["arches"].as_object() {
        Some(arches) => {
            let of = |arch: &str| arches.get(arch).unwrap_or(&Value::Null);
            let count = |arch: &str| {
                let a = of(arch);
                a["rebuilt"]
                    .as_array()
                    .map(Vec::len)
                    .or_else(|| a["teeth"].as_u64().map(|n| n as usize))
                    .unwrap_or(0)
            };
            let gum = |arch: &str| of(arch)["gingiva"].as_bool().unwrap_or(false);
            (count("upper"), count("lower"), [gum("upper"), gum("lower")])
        }
        None => {
            let crowns = step_output(out, "aligners.preparation.reconstruction.automatic");
            let gums = step_output(out, "aligners.preparation.gingiva");
            let count = |arch: &str| {
                crowns
                    .and_then(|c| c[arch]["teeth"].as_object())
                    .map_or(0, |m| m.len())
            };
            let gum = |arch: &str| gums.is_some_and(|g| g.get(arch).is_some_and(|v| !v.is_null()));
            (count("upper"), count("lower"), [gum("upper"), gum("lower")])
        }
    };
    let key = match (gum[0], gum[1]) {
        (true, true) => "ws-teeth-detail-both",
        (true, false) => "ws-teeth-detail-upper",
        (false, true) => "ws-teeth-detail-lower",
        (false, false) => "ws-teeth-detail-none",
    };
    let mut detail = tr_args(
        key,
        &[
            ("count", &(upper + lower).to_string()),
            ("upper", &upper.to_string()),
            ("lower", &lower.to_string()),
        ],
    );
    let not_built: Vec<String> = out["arches"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, a)| a["built"] == false)
        .map(|(_, a)| {
            let reason = a["reason"].as_str().unwrap_or_default();
            tr_args("ws-teeth-arch-not-built", &[("reason", reason)])
        })
        .collect();
    let level = if not_built.is_empty() {
        ToastLevel::Success
    } else {
        for line in &not_built {
            detail.push('\n');
            detail.push_str(line);
        }
        ToastLevel::Warning
    };
    Reported {
        toast: Toast::new(level, "ws-toast-teeth-built").with_detail(detail),
        sticky: !not_built.is_empty(),
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

/// Label of the run button of leaf `id`.
pub(crate) fn button_key(id: &str, input: Option<LeafInput>, done: bool) -> &'static str {
    if dc_module_api::is_import_scans_command(id) {
        return "ws-btn-choose-scans";
    }
    match (id, input) {
        (EXPORT_ALL, _) => "ws-btn-export",
        (PRINT, _) => "ws-btn-print",
        (VISITS, _) => "ws-btn-visit",
        (_, Some(LeafInput::Folder { .. })) => "ws-btn-choose-folder",
        (_, Some(LeafInput::ScanFile { .. })) => "ws-btn-choose-scan",
        _ if done => "ws-btn-run-again",
        _ => "ws-btn-run",
    }
}

/// A separation parameter of the inspector: its label, its range, the step
/// of the slider, the decimals shown and its default (the plugin's).
struct SegParam {
    key: &'static str,
    lo: f32,
    hi: f32,
    step: f32,
    decimals: usize,
    default: f32,
}

/// The three separation parameters, in the order of the inspector and of
/// [`seg_value`].
const SEG_PARAMS: [SegParam; 3] = [
    SegParam {
        key: "ws-seg-target-faces",
        lo: 5000.0,
        hi: 200_000.0,
        step: 1000.0,
        decimals: 0,
        default: 30_000.0,
    },
    SegParam {
        key: "ws-seg-eta-convex",
        lo: 0.01,
        hi: 1.0,
        step: 0.01,
        decimals: 2,
        default: 0.2,
    },
    SegParam {
        key: "ws-seg-sigma",
        lo: 0.1,
        hi: 10.0,
        step: 0.1,
        decimals: 1,
        default: 1.0,
    },
];

/// Parameter `i` of [`SEG_PARAMS`] in `s`.
fn seg_value(s: &SegmentationSettings, i: usize) -> f64 {
    match i {
        0 => s.target_faces as f64,
        1 => s.eta_convex,
        _ => s.sigma,
    }
}

/// The parameters of `aligners.settings.segmentation` that set parameter
/// `i` to `v`, rounded to the decimals the slider shows; `None` when that is
/// the value of `s` already.
fn seg_patch(s: &SegmentationSettings, i: usize, v: f32) -> Option<Value> {
    let p = &SEG_PARAMS[i];
    let scale = 10f64.powi(p.decimals as i32);
    let v = (f64::from(v) * scale).round() / scale;
    if (v - seg_value(s, i)).abs() < 0.5 / scale {
        return None;
    }
    Some(match i {
        0 => serde_json::json!({ "target_faces": v as u64 }),
        1 => serde_json::json!({ "eta_convex": v }),
        _ => serde_json::json!({ "sigma": v }),
    })
}

/// What the inspector of "Separate teeth and gum" keeps between frames.
#[derive(Debug, Default)]
pub(crate) struct SegmentUi {
    /// The slider being dragged and its value, sent when it is let go.
    drag: Option<(usize, f32)>,
}

/// What the "Manual corrections" of the case analysis keep between frames.
#[derive(Debug, Default)]
pub(crate) struct DiagnosisUi {
    /// The field being typed in or dragged, with its value, written when it
    /// is let go.
    pub(crate) field: Option<(ScanRole, EditField, f64)>,
}

/// Whether the case needs the bite before the final position
/// ([`dca_algo::occlusion::bite_needed`]), worked out once per state of the
/// case: it reads the arch frames from the scans.
#[derive(Debug, Default)]
pub(crate) struct BiteCheck {
    /// The state the answer is for (kept weak: the allocation stays, so a
    /// new state never takes its address).
    state: Weak<CaseState>,
    needed: bool,
}

impl BiteCheck {
    /// `true` when `state` needs the bite before the final position.
    pub(crate) fn needed(&mut self, state: &Arc<CaseState>) -> bool {
        if !std::ptr::eq(self.state.as_ptr(), Arc::as_ptr(state)) {
            self.state = Arc::downgrade(state);
            self.needed = dca_algo::occlusion::bite_needed(state);
        }
        self.needed
    }
}

impl Workspace {
    /// The separation parameters, read from the case settings: a change is
    /// sent when the slider is let go, one undo step each.
    pub(crate) fn segment_params(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let settings = crate::overlay::aligners(&state)
            .map(|d| d.settings.segmentation)
            .unwrap_or_default();
        let busy = self.is_busy();
        let mut commit = None;
        let drag = &mut self.segment.drag;
        ui.add_enabled_ui(!busy, |ui| {
            for (i, p) in SEG_PARAMS.iter().enumerate() {
                let mut v = match *drag {
                    Some((d, v)) if d == i => v,
                    _ => seg_value(&settings, i) as f32,
                };
                let r = widgets::labeled_slider(
                    ui,
                    &t(p.key),
                    &mut v,
                    p.lo..=p.hi,
                    p.step,
                    p.decimals,
                    "",
                    p.default,
                );
                if r.dragged() {
                    *drag = Some((i, v));
                } else if r.drag_stopped() || r.changed() {
                    *drag = None;
                    commit = Some((i, v));
                }
                ui.add_space(4.0);
            }
        });
        if let Some((i, v)) = commit
            && let Some(params) = seg_patch(&settings, i, v)
            && let Some(spec) = self.specs.iter().find(|s| s.id == SEGMENT_SETTINGS)
        {
            let cmd = spec.command(params);
            self.run_command(Box::new(cmd));
        }
    }

    /// The result of the last run of each arch, read from the case (it goes
    /// with the colours it describes).
    pub(crate) fn segment_results(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let state = self.undo.current().clone();
        for line in seg_lines(&case_seg_runs(crate::overlay::aligners(&state))) {
            ui.label(
                RichText::new(line)
                    .font(fonts::ui(13.0))
                    .color(c.text_primary),
            );
        }
    }

    /// The status of the 3D teeth: the arches with a scan and no 3D teeth
    /// while the other arch has them, with the reason, and the arches whose
    /// built teeth are out of date, each with what changed since the teeth
    /// were built, and what to do (run again builds both arches again; an
    /// out of date arch shows the scan until then). Nothing while every
    /// arch has teeth that match the case.
    pub(crate) fn teeth_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current();
        let unbuilt = crate::overlay::unbuilt_lines(state);
        let outdated = crate::overlay::outdated_reasons(state);
        let c = *colors(ui);
        let line = |ui: &mut Ui, text: String| {
            ui.label(
                RichText::new(text)
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        };
        if !unbuilt.is_empty() {
            widgets::badge(ui, BadgeKind::Warning, t("ws-teeth-unbuilt"));
            ui.add_space(4.0);
            for text in unbuilt {
                line(ui, text);
            }
            ui.add_space(6.0);
        }
        if let Some(arches) = outdated {
            widgets::badge(ui, BadgeKind::Warning, t("ws-teeth-outdated"));
            ui.add_space(4.0);
            line(
                ui,
                dc_i18n::tr_args("ws-teeth-outdated-detail", &[("arches", &arches)]),
            );
        }
    }

    /// The results of the case analysis, or the line that says it has not
    /// run.
    pub(crate) fn case_analysis_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        case_analysis(ui, &state, dc_module_api::advanced_ui(ui.ctx()));
    }

    /// "Manual corrections" of the case analysis: the values the clinician
    /// measured themselves, per arch. Each shows the computed value, a field
    /// for the clinician's own value (written when the field is let go or
    /// confirmed, `aligners.diagnosis.edit-values`, one undo step), a badge
    /// when it is set by hand and a button that brings the computed one
    /// back. The complexity follows them. Nothing runs by being shown.
    pub(crate) fn case_analysis_corrections(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        let Some(data) = state.module_data::<AlignersState>(MODULE_ID) else {
            crate::function::caption(ui, t("ws-ca-edit-none"));
            return;
        };
        let d = &data.diagnosis;
        if d.arches.is_empty() {
            crate::function::caption(ui, t("ws-ca-edit-none"));
            return;
        }
        let c = *colors(ui);
        crate::function::caption(ui, t("ws-ca-edit-hint"));
        ui.add_space(6.0);
        let enabled = !self.is_busy();
        let mut write = None;
        for role in d.arches.keys().copied() {
            let name = match role {
                ScanRole::Upper => "ws-role-upper",
                ScanRole::Lower => "ws-role-lower",
                ScanRole::Bite => continue,
            };
            let Some(computed) = edit::computed_values(data, role) else {
                continue;
            };
            let hand = d.edits_of(role);
            let typing = &mut self.diagnosis_ui;
            card(ui, |ui| {
                ui.label(
                    RichText::new(t(name))
                        .font(fonts::semibold(13.0))
                        .color(c.text_primary),
                );
                for field in EditField::ALL {
                    let in_force = match field {
                        EditField::Discrepancy => d.discrepancy_mm(role),
                        EditField::Little => d.little_index_mm(role),
                        EditField::MidlineOffset => Some(d.midline_offset_mm(role)),
                        EditField::LargestSpace => Some(d.largest_space_mm(role)),
                    }
                    .unwrap_or_else(|| computed.get(field));
                    let done = correction_row(
                        ui,
                        typing,
                        role,
                        field,
                        shown_value(field, computed.get(field)),
                        shown_value(field, in_force),
                        field.get(&hand).is_some(),
                        enabled,
                    );
                    if let Some(v) = done {
                        // the field shows crowding where the command has discrepancy
                        write = Some((role, field, v.map(|v| shown_value(field, v))));
                    }
                }
            });
            ui.add_space(8.0);
        }
        if let Some((role, field, value)) = write {
            self.edit_diagnosis(role, field, value);
        }
    }

    /// Under the status of "Final position": the notice while the case
    /// needs the bite first (both arches separated and not related,
    /// [`dca_algo::occlusion::bite_needed`]), which points to Bite (the run
    /// button stays; a run is refused with the reason, an error toast, and
    /// the bite is never run from here: menu law), then the result of the
    /// last computation of each arch (`setup::result_lines`).
    pub(crate) fn setup_results(&mut self, ui: &mut Ui) {
        let state = self.undo.current().clone();
        if self.bite_check.needed(&state) {
            let c = *colors(ui);
            widgets::badge(ui, BadgeKind::Warning, t("ws-setup-bite-first"));
            ui.add_space(4.0);
            ui.label(
                RichText::new(t("ws-setup-bite-first-detail"))
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
            ui.add_space(6.0);
        }
        self.setup_result_lines(ui);
    }
}

/// The main figures of the case analysis, one card per arch and the ratios
/// and the complexity of the case. With the advanced options off: per arch
/// the space and its verdict, and the complexity.
fn case_analysis(ui: &mut Ui, state: &CaseState, advanced: bool) {
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
        arch_card(ui, &t(name), d, *role, space, advanced);
        ui.add_space(8.0);
    }
    // the ratios need both arches; when a tooth of the segment is missing in
    // one of them they are not calculated, and the card says so
    if advanced && (d.arches.len() > 1 || d.bolton_anterior.is_some() || d.bolton_overall.is_some())
    {
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

fn arch_card(
    ui: &mut Ui,
    name: &str,
    d: &Diagnosis,
    role: ScanRole,
    space: &ArchSpace,
    advanced: bool,
) {
    let c = *colors(ui);
    let by_hand = !d.edits_of(role).is_empty();
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
        // the values in force: the ones written by hand over the computed
        let gap = d.crowding_mm(role).unwrap_or_else(|| space.crowding_mm());
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
        let little = d.little_index_mm(role).unwrap_or(space.little_index_mm);
        if advanced {
            figure(ui, &t("ws-ca-little"), format!("{} mm", mm(little)));
        }
        let (kind, key) = match LittleCategory::of(little) {
            LittleCategory::Perfect => (BadgeKind::Success, "ws-ca-little-perfect"),
            LittleCategory::Minimal => (BadgeKind::Success, "ws-ca-little-minimal"),
            LittleCategory::Moderate => (BadgeKind::Info, "ws-ca-little-moderate"),
            LittleCategory::Severe => (BadgeKind::Warning, "ws-ca-little-severe"),
            LittleCategory::VerySevere => (BadgeKind::Danger, "ws-ca-little-very-severe"),
        };
        widgets::badge(ui, kind, t(key));
        if advanced && d.details.contains_key(&role) {
            figure(
                ui,
                &t("ws-ca-midline"),
                format!("{} mm", signed(d.midline_offset_mm(role))),
            );
            figure(
                ui,
                &t("ws-ca-largest"),
                format!("{} mm", mm(d.largest_space_mm(role))),
            );
        }
        if by_hand {
            widgets::badge(ui, BadgeKind::Info, t("ws-ca-has-hand"));
        }
    });
}

/// The label of a value of "Manual corrections" of the case analysis.
fn edit_label_key(field: EditField) -> &'static str {
    match field {
        EditField::Discrepancy => "ws-ca-edit-crowding",
        EditField::Little => "ws-ca-edit-little",
        EditField::MidlineOffset => "ws-ca-edit-midline",
        EditField::LargestSpace => "ws-ca-edit-largest",
    }
}

/// The value of a field as the inspector shows and edits it: the
/// discrepancy (negative is crowding) as crowding, positive when the teeth
/// need more room, as the cards show it. The other fields as they are. The
/// conversion is its own inverse.
fn shown_value(field: EditField, v: f64) -> f64 {
    if field == EditField::Discrepancy {
        -v
    } else {
        v
    }
}

/// One value of "Manual corrections": its name, the field (a click or a
/// drag-and-let-go writes it), the computed value, and when the value is
/// by hand its badge and the button that brings the computed one back.
/// Returns what to write: `Some(Some(v))` a value (as the field shows it),
/// `Some(None)` a reset.
#[allow(clippy::too_many_arguments)]
fn correction_row(
    ui: &mut Ui,
    typing: &mut DiagnosisUi,
    role: ScanRole,
    field: EditField,
    computed: f64,
    in_force: f64,
    by_hand: bool,
    enabled: bool,
) -> Option<Option<f64>> {
    let c = *colors(ui);
    let (lo, hi) = field.range();
    let (lo, hi) = (shown_value(field, lo), shown_value(field, hi));
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(t(edit_label_key(field)))
                .font(fonts::medium(12.0))
                .color(c.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut v = match typing.field {
                Some((r, f, v)) if r == role && f == field => v,
                _ => in_force,
            };
            let r = ui.add_enabled(
                enabled,
                egui::DragValue::new(&mut v)
                    .speed(0.1)
                    .range(lo..=hi)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
            if (r.dragged() || r.has_focus()) && (v - in_force).abs() > 1e-9 {
                typing.field = Some((role, field, v));
            }
            if (r.drag_stopped() || r.lost_focus())
                && let Some((rr, ff, vv)) = typing.field.take()
                && rr == role
                && ff == field
            {
                out = Some(Some(vv));
            }
        });
    });
    // the computed value, the badge and the reset wrap in a narrow inspector
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(dc_i18n::tr_args(
                "ws-ca-edit-computed",
                &[("value", &mm(computed))],
            ))
            .font(fonts::ui(12.0))
            .color(c.text_secondary),
        );
        if by_hand {
            widgets::badge(ui, BadgeKind::Info, t("ws-ca-by-hand"));
            if widgets::Button::ghost(t("ws-ca-reset"))
                .size(widgets::ButtonSize::Compact)
                .enabled(enabled)
                .show(ui)
                .clicked()
            {
                out = Some(None);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_print_list_is_the_one_leaf_that_changes_no_state() {
        assert!(!changes_case(PRINT));
        assert!(
            changes_case(EXPORT_ALL)
                && changes_case(AUTO)
                && changes_case(BITE)
                && changes_case(VISITS)
        );
    }

    #[test]
    fn the_doubts_of_the_numbering_the_case_holds_are_notices_to_check() {
        let mut state = CaseState::default();
        assert!(numbering_doubt_toasts(&state).is_empty());
        let mut data = AlignersState::default();
        data.numbering_doubts.insert(
            ScanRole::Upper,
            vec!["Check the teeth of the upper arch: no crown between 11 and 22".into()],
        );
        state.set_module(MODULE_ID, dc_case::ModuleState::new(data));
        let toasts = numbering_doubt_toasts(&state);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].message_key, "ws-toast-to-check");
        assert_eq!(toasts[0].level, ToastLevel::Warning);
        assert!(toasts[0].detail.as_deref().unwrap().contains("11 and 22"));
    }

    /// A workspace on a case whose upper arch is in doubt, as opened.
    fn opened_in_doubt() -> Workspace {
        let mut state = CaseState::default();
        let mut data = AlignersState::default();
        data.numbering_doubts
            .insert(ScanRole::Upper, vec!["no crown between 11 and 22".into()]);
        state.set_module(MODULE_ID, dc_case::ModuleState::new(data));
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

    fn to_check(ws: &Workspace) -> Vec<String> {
        ws.sticky_toast_keys()
            .into_iter()
            .filter(|k| k == "ws-toast-to-check")
            .collect()
    }

    #[test]
    fn the_doubts_of_find_the_teeth_stay_on_screen_from_the_leaf_the_pipeline_and_a_reopened_project()
     {
        // the inspector's button: the warnings of the numbering step
        let mut ws =
            Workspace::new_headless(Box::new(dca_module::AlignersModule::new()), None, None);
        let numbering = json!({ "steps": [
            { "id": "aligners.preparation.numbering.automatic",
              "output": { "warnings": ["no crown between 11 and 22"] } },
        ] });
        ws.show_leaf_report(FIND_TEETH, &numbering);
        assert_eq!(to_check(&ws).len(), 1);
        // the whole pipeline: the doubts are in its warnings, once
        let mut ws =
            Workspace::new_headless(Box::new(dca_module::AlignersModule::new()), None, None);
        let auto = json!({ "steps": [], "seconds": 1.0,
                           "warnings": ["no crown between 11 and 22"] });
        ws.show_leaf_report(AUTO, &auto);
        assert_eq!(to_check(&ws).len(), 1);
        // the project opened again with the marks still in doubt: said again
        assert_eq!(to_check(&opened_in_doubt()).len(), 1);
        // a new case, or marks without doubt: nothing
        let none = Workspace::new_headless(Box::new(dca_module::AlignersModule::new()), None, None);
        assert!(to_check(&none).is_empty());
    }

    #[test]
    fn a_bite_that_does_not_apply_has_no_notification() {
        let none = json!({ "applies": false, "reason": "one_arch" });
        assert!(report(BITE, &none).is_none());
        // an estimate still warns, a bite scan does not
        assert!(report(BITE, &json!({ "method": "virtual_bite" })).is_some_and(|r| r.sticky));
        assert!(report(BITE, &json!({ "method": "bite_scan" })).is_some_and(|r| !r.sticky));
    }

    #[test]
    fn the_print_leaf_button_opens_the_popup() {
        assert_eq!(button_key(PRINT, None, false), "ws-btn-print");
        assert_eq!(button_key(PRINT, None, true), "ws-btn-print");
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
    }

    #[test]
    fn the_teeth_report_counts_the_crowns_and_names_the_gingivas() {
        crate::loader();
        let teeth = |n: usize| {
            let m: serde_json::Map<String, Value> =
                (0..n).map(|k| (k.to_string(), json!({}))).collect();
            json!({ "teeth": m, "seconds": 1.0 })
        };
        let out = json!({ "steps": [
            { "id": "aligners.preparation.reconstruction.automatic",
              "output": { "upper": teeth(14), "lower": teeth(13) } },
            { "id": "aligners.preparation.gingiva",
              "output": { "upper": { "faces": 1 }, "lower": { "faces": 1 } } },
        ] });
        let r = report(BUILD_TEETH, &out).unwrap();
        assert_eq!(r.toast.message_key, "ws-toast-teeth-built");
        let detail = r.toast.detail.unwrap();
        assert!(
            detail.contains("27") && detail.contains("14") && detail.contains("13"),
            "{detail}"
        );
        // a missing field is nothing, not a failure
        let r = report(BUILD_TEETH, &json!({})).unwrap();
        assert!(r.toast.detail.unwrap().contains('0'));
    }

    #[test]
    fn the_teeth_report_reads_the_arches_and_counts_a_partial_rebuild() {
        crate::loader();
        // every tooth of the upper arch, two teeth of the lower one
        let out = json!({ "arches": {
            "upper": { "mode": "all", "teeth": 14, "rebuilt": (11..=24).collect::<Vec<u8>>(),
                       "gingiva": true },
            "lower": { "mode": "some", "teeth": 13, "rebuilt": [41, 42], "gingiva": true },
        } });
        let r = report(BUILD_TEETH, &out).unwrap();
        assert_eq!(r.toast.message_key, "ws-toast-teeth-built");
        let both = tr_args(
            "ws-teeth-detail-both",
            &[("count", "16"), ("upper", "14"), ("lower", "2")],
        );
        assert_eq!(r.toast.detail.unwrap(), both);
        // one arch, no gingiva; without `rebuilt` the count of teeth
        let out = json!({ "arches": { "lower": { "teeth": 13, "gingiva": false } } });
        let r = report(BUILD_TEETH, &out).unwrap();
        let none = tr_args(
            "ws-teeth-detail-none",
            &[("count", "13"), ("upper", "0"), ("lower", "13")],
        );
        assert_eq!(r.toast.detail.unwrap(), none);
    }

    #[test]
    fn the_teeth_report_warns_of_an_arch_it_could_not_build() {
        crate::loader();
        let out = json!({ "arches": {
            "lower": { "built": true, "mode": "all", "teeth": 14, "gingiva": true },
            "upper": { "built": false, "reason": "the upper arch has no tooth colours" },
        } });
        let r = report(BUILD_TEETH, &out).unwrap();
        assert_eq!(r.toast.level, ToastLevel::Warning);
        assert!(r.sticky);
        let detail = r.toast.detail.unwrap();
        let line = tr_args(
            "ws-teeth-arch-not-built",
            &[("reason", "the upper arch has no tooth colours")],
        );
        assert!(detail.ends_with(&format!("\n{line}")), "{detail}");
        // both arches built: a plain success
        let out = json!({ "arches": { "lower": { "built": true, "teeth": 14, "gingiva": true } } });
        let r = report(BUILD_TEETH, &out).unwrap();
        assert_eq!(r.toast.level, ToastLevel::Success);
        assert!(!r.sticky);
    }

    #[test]
    fn the_separation_says_its_teeth_and_time_per_arch() {
        crate::loader();
        let out = json!({
            "lower": { "teeth": 14, "seconds": 9.6, "labels_blob": "ab" },
            "teeth": 14, "seconds": 10.0, "invalidated": [],
        });
        let r = report(SEGMENT, &out).unwrap();
        assert_eq!(r.toast.message_key, "ws-toast-seg-done");
        assert_eq!(r.toast.level, ToastLevel::Success);
        assert_eq!(
            r.toast.detail.as_deref(),
            Some("14 teeth separated in 10 s")
        );
        assert!(!r.sticky);
        // nothing went out of date: no notice
        assert!(outdated_toast(&out).is_none());
        // two arches: one line each, named
        let out = json!({
            "upper": { "teeth": 13, "seconds": 4.0, "labels_blob": "cd" },
            "lower": { "teeth": 14, "seconds": 5.0, "labels_blob": "ab" },
        });
        let runs = seg_runs(&out);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].role, ScanRole::Upper);
        assert_eq!(runs[1].teeth, 14);
        assert_eq!(
            seg_lines(&runs),
            [
                "Upper: 13 teeth separated in 4 s",
                "Lower: 14 teeth separated in 5 s"
            ]
        );
        // an output without arches falls back to the totals
        let r = report(SEGMENT, &json!({ "teeth": 3, "seconds": 2.0 })).unwrap();
        assert_eq!(r.toast.detail.as_deref(), Some("3 teeth separated in 2 s"));
    }

    #[test]
    fn the_inspector_line_is_read_from_the_case_upper_arch_first() {
        crate::loader();
        let mut data = AlignersState::default();
        assert!(case_seg_runs(Some(&data)).is_empty() && case_seg_runs(None).is_empty());
        let run = |teeth, seconds| dca_model::SegmentationRun { teeth, seconds };
        data.segmentation_runs.insert(ScanRole::Lower, run(14, 5.0));
        let runs = case_seg_runs(Some(&data));
        assert_eq!(seg_lines(&runs), ["14 teeth separated in 5 s"]);
        data.segmentation_runs.insert(ScanRole::Upper, run(13, 4.0));
        assert_eq!(
            seg_lines(&case_seg_runs(Some(&data))),
            [
                "Upper: 13 teeth separated in 4 s",
                "Lower: 14 teeth separated in 5 s"
            ]
        );
    }

    #[test]
    fn what_a_separation_left_to_redo_is_listed_in_the_order_of_the_module() {
        crate::loader();
        dca_module::loader();
        let toast = outdated_toast(&json!({ "invalidated": ["gingiva", "crowns"] })).unwrap();
        assert_eq!(toast.message_key, "ws-toast-marks-outdated");
        assert_eq!(toast.level, ToastLevel::Info);
        let detail = toast.detail.unwrap();
        assert!(
            detail.starts_with("Out of date because the teeth were separated again"),
            "{detail}"
        );
        assert!(
            !detail.contains("aligners-") && !detail.contains("crowns"),
            "{detail}"
        );
        assert!(outdated_toast(&json!({})).is_none());
    }

    #[test]
    fn every_warning_of_the_pipeline_or_the_export_is_a_notice() {
        let out = json!({ "warnings": ["Check the teeth of the lower arch", ""] });
        let toasts = warning_toasts(&out);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].message_key, "ws-toast-to-check");
        assert_eq!(toasts[0].level, ToastLevel::Warning);
        assert_eq!(
            toasts[0].detail.as_deref(),
            Some("Check the teeth of the lower arch")
        );
        assert!(warning_toasts(&json!({ "warnings": [] })).is_empty());
        assert!(warning_toasts(&json!({})).is_empty());
    }

    #[test]
    fn what_find_the_teeth_left_in_doubt_is_a_notice() {
        let out = json!({ "steps": [
            { "id": "aligners.preparation.numbering.automatic",
              "output": { "arches": [], "failed": [],
                          "warnings": ["Check the teeth of the upper arch: no crown between 11 and 22"] } },
            { "id": "aligners.preparation.orientation.automatic", "output": {} },
        ] });
        let toasts = step_warning_toasts(&out);
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].message_key, "ws-toast-to-check");
        assert_eq!(toasts[0].level, ToastLevel::Warning);
        assert!(toasts[0].detail.as_deref().unwrap().contains("11 and 22"));
        // an older module, or a numbering with no doubt: nothing
        assert!(step_warning_toasts(&json!({ "steps": [{ "output": {} }] })).is_empty());
        assert!(step_warning_toasts(&json!({})).is_empty());
    }

    #[test]
    fn a_slider_value_is_sent_rounded_and_only_when_it_changes() {
        let s = SegmentationSettings::default();
        assert_eq!(
            seg_patch(&s, 0, 52_000.4),
            Some(json!({ "target_faces": 52000 }))
        );
        assert_eq!(seg_patch(&s, 1, 0.35), Some(json!({ "eta_convex": 0.35 })));
        assert_eq!(seg_patch(&s, 2, 2.5), Some(json!({ "sigma": 2.5 })));
        assert_eq!(seg_patch(&s, 0, 30_000.0), None);
        assert_eq!(seg_patch(&s, 1, 0.2), None);
        assert_eq!(seg_patch(&s, 2, 1.0), None);
        // the defaults of the sliders are the ones of the case settings
        for (i, p) in SEG_PARAMS.iter().enumerate() {
            assert!((f64::from(p.default) - seg_value(&s, i)).abs() < 1e-6);
            assert!(p.lo <= p.default && p.default <= p.hi);
        }
    }
}
