//! "Import scans": the files the workspace accepts, the parameters it gives
//! the shared import command and the notifications it builds from the
//! command's JSON report.
//!
//! The import itself is not here: the workspace runs the module's
//! `*.import-scans` command (or the core `case.import`) in a background job,
//! the same code the CLI and the MCP server run.

use std::path::{Path, PathBuf};

use dc_case::ScanRole;
use dc_module_api::{Toast, ToastLevel};
use serde_json::{Value, json};

/// File extensions accepted by a drop (compared without regard to letter case).
pub const EXTENSIONS: [&str; 3] = ["stl", "obj", "ply"];

/// Extensions the file dialog filter lists: the portal matches them case
/// sensitively, so each one is there in lower and upper case (as in the New
/// project dialog).
pub const DIALOG_EXTENSIONS: [&str; 6] = ["stl", "ply", "obj", "STL", "PLY", "OBJ"];

/// Most notifications one report puts on screen, the one-line group and the
/// success line included (the stack draws four). Failed files and arch
/// warnings do not count against it past the first slots: they are never
/// grouped or dropped, so a report with many of them goes over the bound
/// by them alone, and the other notes shrink to [`MIN_OTHER_TOASTS`].
pub const MAX_REPORT_TOASTS: usize = 4;

/// Fewest slots the other notes keep, the success line included, when failed
/// files and arch warnings fill the bound: the cleared notice, one group
/// line and the success line.
const MIN_OTHER_TOASTS: usize = 3;

/// Longest list of grouped messages in the "more" notification.
const GROUP_CHARS: usize = 220;

/// `true` when `path` has the extension of a mesh the import reads, in any
/// letter case.
pub fn is_scan_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Splits `paths` into the files the import reads and the ones it ignores.
pub fn split_scan_files(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    paths.into_iter().partition(|p| is_scan_file(p))
}

/// Id of `role` in the command parameters.
pub(crate) fn role_id(role: ScanRole) -> &'static str {
    match role {
        ScanRole::Upper => "upper",
        ScanRole::Lower => "lower",
        ScanRole::Bite => "bite",
    }
}

/// Parameters of the import command: one entry per file, with the role the
/// user chose (`None` = auto, left out so the import decides).
pub(crate) fn params(paths: &[PathBuf], role: Option<ScanRole>) -> Value {
    let files: Vec<Value> = paths
        .iter()
        .map(|p| {
            let mut f = json!({ "path": p.to_string_lossy() });
            if let Some(r) = role {
                f["role"] = json!(role_id(r));
            }
            f
        })
        .collect();
    json!({ "files": files })
}

/// `key` of this crate (or any registered bundle) with Fluent arguments.
fn tr_args(key: &str, args: &[(&str, &str)]) -> String {
    crate::loader();
    dc_i18n::tr_args(key, args)
}

fn array<'a>(report: &'a Value, key: &str) -> &'a [Value] {
    report
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Title key of the notification of a warning with this `code`.
fn warning_title_key(code: &str) -> &'static str {
    match code {
        "rescaled" => "ws-toast-warn-rescaled",
        "units_suspect" => "ws-toast-warn-units-suspect",
        "tiny" => "ws-toast-warn-tiny",
        "low_polygons" => "ws-toast-warn-low-polygons",
        "obj_objects_merged" => "ws-toast-warn-obj-merged",
        "bad_faces_removed" => "ws-toast-warn-bad-faces",
        "replaced" => "ws-toast-warn-replaced",
        "arch_conflict" => "ws-toast-warn-arch-conflict",
        "provisional" => "ws-toast-warn-provisional",
        "bite_limit" => "ws-toast-warn-bite-limit",
        _ => "ws-toast-import-warning",
    }
}

/// The message of a warning: its translated `message`, or its key looked up
/// with its arguments.
fn warning_message(w: &Value) -> Option<String> {
    if let Some(m) = text(w, "message") {
        return Some(m.to_owned());
    }
    let key = text(w, "key")?;
    let args: Vec<(String, String)> = w
        .get("args")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    Some(tr_args(key, &args))
}

/// The ids of `module_cleared` as a translated list. The module `module_id`
/// names what it cleared with the keys `<module_id>-cleared-<id>` (`_` as `-`);
/// an id without a key stays as it is.
pub(crate) fn cleared_list(module_id: &str, ids: &[&str]) -> String {
    ids.iter()
        .map(|id| {
            let key = format!("{module_id}-cleared-{}", id.replace('_', "-"));
            if dc_i18n::has(&key) {
                dc_i18n::tr(&key)
            } else {
                (*id).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The ids in the `module_cleared` array of `report`.
fn cleared_ids(report: &Value) -> Vec<&str> {
    array(report, "module_cleared")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

/// What made a module clear results: it decides the wording of the notice,
/// which says only what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearCause {
    /// A new scan took the place of one on the same role.
    Replaced,
    /// A scan changed role (`case.set-role`, or an import that moved a
    /// provisional scan to the other arch) and nothing was replaced.
    RoleChanged,
    /// Scans were added and nothing was replaced or moved.
    Added,
}

impl ClearCause {
    /// The cause behind an import report: a replacement wins over a move.
    fn of_import(report: &Value) -> Self {
        let replaced = report.get("replaced").and_then(Value::as_bool) == Some(true)
            || !array(report, "replaced_roles").is_empty();
        if replaced {
            Self::Replaced
        } else if !array(report, "moved").is_empty() {
            Self::RoleChanged
        } else {
            Self::Added
        }
    }

    /// Title key and detail key of the notice.
    fn keys(self) -> (&'static str, &'static str) {
        match self {
            Self::Replaced => ("ws-toast-import-cleared", "ws-import-cleared-detail"),
            Self::RoleChanged => ("ws-toast-role-changed", "ws-role-changed-detail"),
            Self::Added => ("ws-toast-import-affected", "ws-import-affected-detail"),
        }
    }
}

/// Notification for the results a module cleared, or marked to be redone
/// (the bite registration), because the scans changed. It names what was
/// affected and where to get it back; it never says the case was reset,
/// because only these results were.
pub(crate) fn cleared_toast(module_id: &str, report: &Value, cause: ClearCause) -> Option<Toast> {
    let ids = cleared_ids(report);
    if ids.is_empty() {
        return None;
    }
    let list = cleared_list(module_id, &ids);
    let (title, detail) = cause.keys();
    Some(Toast::new(ToastLevel::Warning, title).with_detail(tr_args(detail, &[("list", &list)])))
}

/// Notification of the report of a role change (`case.set-role`).
pub(crate) fn role_changed_toast(module_id: &str, report: &Value) -> Option<Toast> {
    cleared_toast(module_id, report, ClearCause::RoleChanged)
}

/// A notification of an import report and whether it stays until the user
/// closes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportToast {
    pub toast: Toast,
    /// Failed files and arch warnings: never time out, never grouped.
    pub sticky: bool,
}

impl ReportToast {
    fn new(toast: Toast, sticky: bool) -> Self {
        Self { toast, sticky }
    }
}

/// Position of a warning in the report: the lower, the more serious. Arch
/// warnings come first (a wrong arch spoils everything computed after it),
/// then the units ones (the model may be in another unit), then the rest.
/// The Aligners arch notes share the code `provisional` with the generic one,
/// so their keys tell them apart.
fn warning_rank(w: &Value) -> u8 {
    let code = text(w, "code").unwrap_or_default();
    match text(w, "key").unwrap_or_default() {
        "aligners-arch-warn-to-be-checked" => return ARCH_TO_BE_CHECKED,
        "aligners-arch-warn-not-recognised" => return ARCH_NOT_RECOGNISED,
        _ => {}
    }
    match code {
        "arch_conflict" => 0,
        "provisional" => 1,
        "replaced" => 4,
        "units_suspect" => FIRST_UNITS,
        "rescaled" => FIRST_UNITS + 1,
        "tiny" => FIRST_UNITS + 2,
        "low_polygons" => FIRST_OTHER,
        "bad_faces_removed" => FIRST_OTHER + 1,
        "obj_objects_merged" => FIRST_OTHER + 2,
        "bite_limit" => FIRST_OTHER + 3,
        _ => u8::MAX,
    }
}

const ARCH_TO_BE_CHECKED: u8 = 2;
const ARCH_NOT_RECOGNISED: u8 = 3;
const FIRST_UNITS: u8 = 10;
const FIRST_OTHER: u8 = 20;

/// Notifications for the JSON report of an import (the output of the module's
/// `import-scans` or of `case.import`), oldest first. Order of importance:
/// failed files; the arch warnings; the cleared (or replaced) notice; the
/// units warnings; every other warning; then the success line, so it stays at
/// the bottom.
///
/// Failed files and arch warnings are `sticky` and always listed in full,
/// each as its own notification. The rest keeps its time out and, when it
/// does not fit in [`MAX_REPORT_TOASTS`] together with the sticky ones, the
/// surplus becomes one grouped line.
pub(crate) fn report_toasts(report: &Value, module_id: &str) -> Vec<ReportToast> {
    let mut sticky: Vec<ReportToast> = Vec::new();
    for f in array(report, "failed") {
        let reason = text(f, "reason").unwrap_or("?");
        sticky.push(ReportToast::new(
            Toast::new(ToastLevel::Error, "ws-toast-import-file-failed").with_detail(reason),
            true,
        ));
    }

    let cause = ClearCause::of_import(report);
    let cleared = cleared_toast(module_id, report, cause);
    // A replacement is told once: by the module's own warning (which scan took
    // whose place) when the report has it, with the way back appended unless
    // the notice of the cleared results already gives it.
    let replaced_warned = array(report, "warnings")
        .iter()
        .any(|w| text(w, "code") == Some("replaced"));

    // most serious first; the sort is stable, so equals keep the report order
    let mut warnings: Vec<(u8, Toast)> = Vec::new();
    for w in array(report, "warnings") {
        let Some(mut message) = warning_message(w) else {
            continue;
        };
        let code = text(w, "code").unwrap_or_default();
        if code == "replaced" && cause == ClearCause::Replaced && cleared.is_none() {
            message.push(' ');
            message.push_str(&crate::t("ws-import-replaced-detail"));
        }
        let key = warning_title_key(code);
        let toast = Toast::new(ToastLevel::Warning, key).with_detail(message);
        warnings.push((warning_rank(w), toast));
    }
    warnings.sort_by_key(|(rank, _)| *rank);
    let (arch, minor): (Vec<_>, Vec<_>) = warnings.into_iter().partition(|(r, _)| *r < FIRST_UNITS);
    sticky.extend(arch.into_iter().map(|(_, t)| ReportToast::new(t, true)));

    let mut notes: Vec<Toast> = Vec::new();
    if let Some(t) = cleared {
        notes.push(t);
    } else if cause == ClearCause::Replaced && !replaced_warned {
        notes.push(
            Toast::new(ToastLevel::Info, "ws-toast-import-replaced")
                .with_detail(crate::t("ws-import-replaced-detail")),
        );
    }
    // the units warnings, then the others (ranks are already in that order)
    notes.extend(minor.into_iter().map(|(_, t)| t));

    let imported = array(report, "imported").len();
    let success = usize::from(imported > 0);
    let budget = MAX_REPORT_TOASTS
        .saturating_sub(sticky.len())
        .max(MIN_OTHER_TOASTS);
    let slots = budget - success;
    if notes.len() > slots {
        let rest = notes.split_off(slots - 1);
        notes.push(group(&rest));
    }

    let mut out = sticky;
    out.extend(notes.into_iter().map(|t| ReportToast::new(t, false)));
    if imported > 0 {
        let key = if imported == 1 {
            "ws-import-added-one"
        } else {
            "ws-import-added-many"
        };
        out.push(ReportToast::new(
            Toast::new(ToastLevel::Success, "ws-toast-imported")
                .with_detail(tr_args(key, &[("count", &imported.to_string())])),
            false,
        ));
    }
    out
}

/// One line for the notes that did not fit (never failed files or arch
/// warnings: those are not grouped).
fn group(rest: &[Toast]) -> Toast {
    let mut list = rest
        .iter()
        .filter_map(|t| t.detail.as_deref())
        .collect::<Vec<_>>()
        .join(" ");
    if list.chars().count() > GROUP_CHARS {
        list = list.chars().take(GROUP_CHARS).collect::<String>() + "…";
    }
    Toast::new(ToastLevel::Warning, "ws-toast-import-more").with_detail(tr_args(
        "ws-import-more-detail",
        &[("count", &rest.len().to_string()), ("list", &list)],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_files_by_extension_in_any_case() {
        for ok in ["a.stl", "a.STL", "dir/b.Obj", "c.ply", "d.PLY"] {
            assert!(is_scan_file(Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "a", "a.stl.bak", "stl", ".stl", "a.3mf"] {
            assert!(!is_scan_file(Path::new(no)), "{no}");
        }
        let (ok, ignored) = split_scan_files(vec!["x.STL".into(), "notes.txt".into()]);
        assert_eq!(ok, vec![PathBuf::from("x.STL")]);
        assert_eq!(ignored, vec![PathBuf::from("notes.txt")]);
    }

    #[test]
    fn the_dialog_filter_lists_every_extension_in_both_cases() {
        for e in EXTENSIONS {
            assert!(DIALOG_EXTENSIONS.contains(&e), "{e}");
            assert!(
                DIALOG_EXTENSIONS.contains(&e.to_uppercase().as_str()),
                "{e}"
            );
        }
        assert_eq!(DIALOG_EXTENSIONS.len(), 2 * EXTENSIONS.len());
    }

    #[test]
    fn params_carry_the_role_only_when_chosen() {
        let paths = vec![PathBuf::from("a.stl"), PathBuf::from("b.stl")];
        let auto = params(&paths, None);
        assert_eq!(auto["files"][0], json!({ "path": "a.stl" }));
        let bite = params(&paths, Some(ScanRole::Bite));
        assert_eq!(bite["files"][1], json!({ "path": "b.stl", "role": "bite" }));
    }

    /// The notifications of `report` without their stickiness.
    fn plain(report: &Value, module_id: &str) -> Vec<Toast> {
        report_toasts(report, module_id)
            .into_iter()
            .map(|t| t.toast)
            .collect()
    }

    fn warning_of(code: &str, key: &str, message: &str) -> Value {
        json!({ "code": code, "key": key, "args": {}, "message": message })
    }

    /// An arch warning (the sticky kind).
    fn warning(n: usize) -> Value {
        warning_of("provisional", "x", &format!("w{n}"))
    }

    /// A warning that times out and can be grouped.
    fn minor(n: usize) -> Value {
        warning_of("low_polygons", "x", &format!("m{n}"))
    }

    #[test]
    fn a_clean_import_is_one_success_line() {
        let r = json!({ "imported": [{}, {}], "warnings": [], "failed": [], "module_cleared": [] });
        let t = plain(&r, "aligners");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].level, ToastLevel::Success);
        assert_eq!(t[0].message_key, "ws-toast-imported");
    }

    #[test]
    fn failed_files_and_warnings_come_before_the_success_line() {
        let r = json!({
            "imported": [{}],
            "failed": [{ "reason": "broken.stl: the file cannot be read." }],
            "warnings": [warning(1)],
        });
        let t = plain(&r, "aligners");
        let keys: Vec<_> = t.iter().map(|t| t.message_key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "ws-toast-import-file-failed",
                "ws-toast-warn-provisional",
                "ws-toast-imported"
            ]
        );
        assert_eq!(
            t[0].detail.as_deref(),
            Some("broken.stl: the file cannot be read.")
        );
        assert_eq!(t[1].detail.as_deref(), Some("w1"));
    }

    #[test]
    fn many_notes_are_grouped_in_one_line() {
        let warnings: Vec<Value> = (0..9).map(minor).collect();
        let r = json!({ "imported": [{}], "warnings": warnings });
        let t = plain(&r, "aligners");
        assert_eq!(t.len(), MAX_REPORT_TOASTS);
        assert_eq!(t[0].detail.as_deref(), Some("m0"));
        assert_eq!(t[1].detail.as_deref(), Some("m1"));
        assert_eq!(t[2].message_key, "ws-toast-import-more");
        let more = t[2].detail.as_deref().unwrap();
        assert!(more.contains('7') && more.contains("m8"), "{more}");
        assert_eq!(t[3].message_key, "ws-toast-imported");
        // nothing imported: the success slot goes to the notes
        let r = json!({ "warnings": (0..9).map(minor).collect::<Vec<_>>() });
        assert_eq!(plain(&r, "aligners").len(), MAX_REPORT_TOASTS);
    }

    #[test]
    fn the_cleared_notice_lists_only_what_was_cleared() {
        let r = json!({ "imported": [{}], "replaced": true, "module_cleared": ["seeds", "nope"] });
        let t = plain(&r, "no-such-module");
        assert_eq!(t[0].message_key, "ws-toast-import-cleared");
        let d = t[0].detail.as_deref().unwrap();
        assert!(d.contains("seeds, nope"), "{d}");
        assert!(!d.to_lowercase().contains("reset"), "{d}");
        // replaced without anything cleared
        let r = json!({ "imported": [{}], "replaced": true });
        assert_eq!(plain(&r, "x")[0].message_key, "ws-toast-import-replaced");
    }

    #[test]
    fn a_replacement_is_told_once() {
        // the module's own warning says which scan took whose place; the way
        // back goes into it, and there is no second notice with the same title
        let r = json!({
            "imported": [{}],
            "replaced": true,
            "warnings": [warning_of("replaced", "x", "b.stl replaces a.stl.")],
        });
        let t = plain(&r, "x");
        let titles: Vec<&str> = t.iter().map(|t| t.message_key.as_str()).collect();
        assert_eq!(titles, ["ws-toast-warn-replaced", "ws-toast-imported"]);
        let d = t[0].detail.as_deref().unwrap();
        assert!(d.starts_with("b.stl replaces a.stl. "), "{d}");
        assert!(d.contains("Ctrl+Z"), "{d}");
        // with results cleared, that notice carries the way back and has its own title
        let r = json!({
            "imported": [{}],
            "replaced": true,
            "module_cleared": ["seeds"],
            "warnings": [warning_of("replaced", "x", "b.stl replaces a.stl.")],
        });
        let t = plain(&r, "x");
        assert_eq!(t[0].detail.as_deref(), Some("b.stl replaces a.stl."));
        assert_eq!(t[1].message_key, "ws-toast-import-cleared");
        let mut titles: Vec<String> = t.iter().map(|t| dc_i18n::tr(&t.message_key)).collect();
        titles.sort();
        titles.dedup();
        assert_eq!(
            titles.len(),
            t.len(),
            "two notices share a title: {titles:?}"
        );
    }

    /// A report with every kind of warning, listed in the reverse of the
    /// order they must come out in.
    fn full_report() -> Value {
        json!({
            "imported": [{}, {}],
            "replaced": true,
            "module_cleared": ["seeds"],
            "failed": [{ "reason": "broken.stl: the file cannot be read." }],
            "warnings": [
                warning_of("bite_limit", "x", "bite limit"),
                warning_of("obj_objects_merged", "x", "objects merged"),
                warning_of("bad_faces_removed", "x", "faces removed"),
                warning_of("low_polygons", "x", "few polygons"),
                warning_of("tiny", "x", "very small"),
                warning_of("rescaled", "x", "converted"),
                warning_of("units_suspect", "x", "units?"),
                warning_of("replaced", "x", "replaced scan"),
                warning_of("provisional", "aligners-arch-warn-not-recognised", "not recognised"),
                warning_of("provisional", "aligners-arch-warn-to-be-checked", "to be checked"),
                warning_of("arch_conflict", "x", "arch taken"),
            ],
        })
    }

    #[test]
    fn warnings_come_by_severity_and_the_arch_ones_stay() {
        let t = report_toasts(&full_report(), "aligners");
        let order: Vec<(&str, bool)> = t
            .iter()
            .map(|t| (t.toast.detail.as_deref().unwrap_or_default(), t.sticky))
            .collect();
        // failed files, then the arch warnings (conflict, check, not
        // recognised, replaced): never grouped
        assert_eq!(order[0], ("broken.stl: the file cannot be read.", true));
        assert_eq!(
            order[1..5],
            [
                ("arch taken", true),
                ("to be checked", true),
                ("not recognised", true),
                ("replaced scan", true)
            ]
        );
        assert_eq!(t[0].toast.level, ToastLevel::Error);
        // the cleared notice, the units warnings, the rest grouped, success last
        assert_eq!(t[5].toast.message_key, "ws-toast-import-cleared");
        assert!(!t[5].sticky);
        let rest: Vec<_> = t[6..]
            .iter()
            .map(|t| t.toast.message_key.as_str())
            .collect();
        assert_eq!(rest, ["ws-toast-import-more", "ws-toast-imported"]);
        assert!(t[6..].iter().all(|t| !t.sticky));
        // the group keeps the units warnings first, then the others
        let more = t[6].toast.detail.as_deref().unwrap();
        let at = |needle: &str| {
            more.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {more}"))
        };
        assert!(at("units?") < at("converted"), "{more}");
        assert!(at("converted") < at("very small"), "{more}");
        assert!(at("very small") < at("few polygons"), "{more}");
        assert!(at("few polygons") < at("faces removed"), "{more}");
        assert!(at("faces removed") < at("objects merged"), "{more}");
        assert!(at("objects merged") < at("bite limit"), "{more}");
    }

    #[test]
    fn only_the_arch_warnings_and_failed_files_are_sticky() {
        let r = json!({
            "warnings": [
                warning_of("units_suspect", "x", "u"),
                warning_of("provisional", "x", "p"),
                warning_of("low_polygons", "x", "l"),
            ],
            "failed": [{ "reason": "a" }, { "reason": "b" }],
        });
        let sticky: Vec<_> = report_toasts(&r, "aligners")
            .into_iter()
            .filter(|t| t.sticky)
            .map(|t| t.toast.detail.unwrap())
            .collect();
        assert_eq!(sticky, ["a", "b", "p"]);
        // the arch warning is recognised by its key too, whatever its code
        let r =
            json!({ "warnings": [warning_of("other", "aligners-arch-warn-to-be-checked", "k")] });
        assert!(report_toasts(&r, "aligners")[0].sticky);
    }

    #[test]
    fn a_crowd_of_sticky_notes_is_never_grouped_or_cut() {
        let long = "x".repeat(GROUP_CHARS * 3);
        let mut warnings: Vec<Value> = (0..6)
            .map(|n| warning_of("arch_conflict", "x", &format!("{long}{n}")))
            .collect();
        warnings.extend((0..5).map(minor));
        let r = json!({
            "imported": [{}],
            "failed": [{ "reason": "a" }, { "reason": "b" }],
            "warnings": warnings,
        });
        let t = report_toasts(&r, "aligners");
        let sticky: Vec<_> = t.iter().filter(|t| t.sticky).collect();
        assert_eq!(sticky.len(), 8);
        for (n, s) in sticky[2..].iter().enumerate() {
            assert_eq!(
                s.toast.detail.as_deref(),
                Some(format!("{long}{n}").as_str())
            );
            assert!(!s.toast.detail.as_deref().unwrap().ends_with('…'));
        }
        // the others shrink to one note, one group line and the success line
        let rest: Vec<_> = t.iter().filter(|t| !t.sticky).collect();
        assert_eq!(rest.len(), MIN_OTHER_TOASTS);
        assert_eq!(rest[1].toast.message_key, "ws-toast-import-more");
        assert_eq!(rest[2].toast.message_key, "ws-toast-imported");
    }

    #[test]
    fn a_long_arch_warning_is_not_cut_but_a_grouped_one_is() {
        let long = format!("{} end-of-text", "word ".repeat(150));
        let r = json!({ "warnings": [warning_of("provisional", "x", &long)] });
        let t = report_toasts(&r, "aligners");
        assert_eq!(t[0].toast.detail.as_deref(), Some(long.as_str()));
        assert!(t[0].sticky);
        let minor_long = |n: usize| warning_of("low_polygons", "x", &format!("{long}{n}"));
        let r = json!({ "warnings": (0..6).map(minor_long).collect::<Vec<_>>() });
        let t = report_toasts(&r, "aligners");
        let more = t.last().unwrap().toast.detail.as_deref().unwrap();
        assert!(more.ends_with('…'), "{more}");
    }

    #[test]
    fn what_a_clear_says_depends_on_what_happened() {
        let cleared = json!({ "module_cleared": ["seeds", "bite_registered"] });
        let with = |extra: Value| {
            let mut r = cleared.clone();
            r.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            plain(&r, "no-such-module")
        };
        let replaced = with(json!({ "replaced": true }));
        assert_eq!(replaced[0].message_key, "ws-toast-import-cleared");
        let moved = with(json!({ "moved": [{ "from": "upper", "to": "lower" }] }));
        assert_eq!(moved[0].message_key, "ws-toast-role-changed");
        let added = with(json!({}));
        assert_eq!(added[0].message_key, "ws-toast-import-affected");
        // a replacement and a move at once: the replacement is what happened
        let both = with(json!({ "replaced": true, "moved": [{}] }));
        assert_eq!(both[0].message_key, "ws-toast-import-cleared");
        // only the replaced wording talks about a scan being replaced
        for t in moved.iter().chain(&added) {
            let text = format!("{} {}", t.message_key, t.detail.as_deref().unwrap());
            assert!(!text.contains("replace"), "{text}");
        }
    }

    #[test]
    fn a_role_change_says_only_what_happened() {
        let report = json!({
            "module_cleared": ["seeds", "bite_registered"],
            "world_frame_reset": true,
        });
        let t = role_changed_toast("no-such-module", &report).expect("something was cleared");
        assert_eq!(t.message_key, "ws-toast-role-changed");
        assert_eq!(t.level, ToastLevel::Warning);
        let d = t.detail.unwrap();
        assert!(d.contains("seeds, bite_registered"), "{d}");
        for word in ["replaced", "reset"] {
            assert!(!d.to_lowercase().contains(word), "{d}");
        }
        // nothing cleared: no notice at all
        assert!(role_changed_toast("no-such-module", &json!({ "module_cleared": [] })).is_none());
    }

    #[test]
    fn a_report_without_fields_shows_nothing() {
        assert!(plain(&json!({}), "x").is_empty());
        assert!(plain(&Value::Null, "x").is_empty());
    }
}
