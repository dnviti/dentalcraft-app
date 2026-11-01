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
/// success line included (the stack draws four).
pub const MAX_REPORT_TOASTS: usize = 4;

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

/// Notification for the results a module cleared because their scan changed.
/// It names what was cleared and where to get it back; it never says the
/// case was reset, because only these results were.
pub(crate) fn cleared_toast(module_id: &str, report: &Value) -> Option<Toast> {
    let ids = cleared_ids(report);
    if ids.is_empty() {
        return None;
    }
    let list = cleared_list(module_id, &ids);
    Some(
        Toast::new(ToastLevel::Warning, "ws-toast-import-cleared")
            .with_detail(tr_args("ws-import-cleared-detail", &[("list", &list)])),
    )
}

/// Notifications for the JSON report of an import (the output of the module's
/// `import-scans` or of `case.import`), oldest first. Order of importance:
/// failed files, the replaced or cleared notice, the warnings, then the
/// success line last, so it stays at the bottom. At most
/// [`MAX_REPORT_TOASTS`]: the surplus becomes one grouped line.
pub(crate) fn report_toasts(report: &Value, module_id: &str) -> Vec<Toast> {
    let mut notes: Vec<Toast> = Vec::new();
    for f in array(report, "failed") {
        let reason = text(f, "reason").unwrap_or("?");
        notes
            .push(Toast::new(ToastLevel::Error, "ws-toast-import-file-failed").with_detail(reason));
    }
    if let Some(t) = cleared_toast(module_id, report) {
        notes.push(t);
    } else if report.get("replaced").and_then(Value::as_bool) == Some(true)
        || !array(report, "replaced_roles").is_empty()
    {
        notes.push(
            Toast::new(ToastLevel::Info, "ws-toast-import-replaced")
                .with_detail(crate::t("ws-import-replaced-detail")),
        );
    }
    for w in array(report, "warnings") {
        let Some(message) = warning_message(w) else {
            continue;
        };
        let key = warning_title_key(text(w, "code").unwrap_or_default());
        notes.push(Toast::new(ToastLevel::Warning, key).with_detail(message));
    }

    let imported = array(report, "imported").len();
    let budget = MAX_REPORT_TOASTS - usize::from(imported > 0);
    if notes.len() > budget {
        let rest = notes.split_off(budget - 1);
        notes.push(group(&rest));
    }
    if imported > 0 {
        let key = if imported == 1 {
            "ws-import-added-one"
        } else {
            "ws-import-added-many"
        };
        notes.push(
            Toast::new(ToastLevel::Success, "ws-toast-imported")
                .with_detail(tr_args(key, &[("count", &imported.to_string())])),
        );
    }
    notes
}

/// One line for the notifications that did not fit.
fn group(rest: &[Toast]) -> Toast {
    let mut list = rest
        .iter()
        .filter_map(|t| t.detail.as_deref())
        .collect::<Vec<_>>()
        .join(" ");
    if list.chars().count() > GROUP_CHARS {
        list = list.chars().take(GROUP_CHARS).collect::<String>() + "…";
    }
    let level = if rest.iter().any(|t| t.level == ToastLevel::Error) {
        ToastLevel::Error
    } else {
        ToastLevel::Warning
    };
    Toast::new(level, "ws-toast-import-more").with_detail(tr_args(
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

    fn warning(n: usize) -> Value {
        json!({ "code": "provisional", "key": "x", "args": {}, "message": format!("w{n}") })
    }

    #[test]
    fn a_clean_import_is_one_success_line() {
        let r = json!({ "imported": [{}, {}], "warnings": [], "failed": [], "module_cleared": [] });
        let t = report_toasts(&r, "aligners");
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
        let t = report_toasts(&r, "aligners");
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
        let warnings: Vec<Value> = (0..9).map(warning).collect();
        let r = json!({ "imported": [{}], "warnings": warnings });
        let t = report_toasts(&r, "aligners");
        assert_eq!(t.len(), MAX_REPORT_TOASTS);
        assert_eq!(t[0].detail.as_deref(), Some("w0"));
        assert_eq!(t[1].detail.as_deref(), Some("w1"));
        assert_eq!(t[2].message_key, "ws-toast-import-more");
        let more = t[2].detail.as_deref().unwrap();
        assert!(more.contains('7') && more.contains("w8"), "{more}");
        assert_eq!(t[3].message_key, "ws-toast-imported");
        // nothing imported: the success slot goes to the notes
        let r = json!({ "warnings": (0..9).map(warning).collect::<Vec<_>>() });
        assert_eq!(report_toasts(&r, "aligners").len(), MAX_REPORT_TOASTS);
    }

    #[test]
    fn the_cleared_notice_lists_only_what_was_cleared() {
        let r = json!({ "imported": [{}], "replaced": true, "module_cleared": ["seeds", "nope"] });
        let t = report_toasts(&r, "no-such-module");
        assert_eq!(t[0].message_key, "ws-toast-import-cleared");
        let d = t[0].detail.as_deref().unwrap();
        assert!(d.contains("seeds, nope"), "{d}");
        assert!(!d.to_lowercase().contains("reset"), "{d}");
        // replaced without anything cleared
        let r = json!({ "imported": [{}], "replaced": true });
        assert_eq!(
            report_toasts(&r, "x")[0].message_key,
            "ws-toast-import-replaced"
        );
    }

    #[test]
    fn a_report_without_fields_shows_nothing() {
        assert!(report_toasts(&json!({}), "x").is_empty());
        assert!(report_toasts(&Value::Null, "x").is_empty());
    }
}
