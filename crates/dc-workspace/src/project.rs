//! Project file helpers: building the [`Project`] to save, timestamps and
//! the `.dcprj` codecs of a module.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dc_case::CaseState;
use dc_command::StepHistory;
use dc_module_api::Module;
use dc_project::{AppInfo, Manifest, ModuleCodecs, Project};

/// Writer information stored in new manifests.
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        commit: option_env!("DC_GIT_COMMIT").unwrap_or("dev").to_owned(),
    }
}

/// Current UTC time as RFC 3339 (`2026-10-31T11:20:05Z`).
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Formats Unix seconds as RFC 3339 UTC.
pub fn rfc3339(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    // civil-from-days (Howard Hinnant), proleptic Gregorian calendar
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `path` with the `.dcprj` extension added when it has another one or none.
pub fn with_extension(path: PathBuf) -> PathBuf {
    let ext_ok = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(dc_project::FILE_EXTENSION));
    if ext_ok {
        path
    } else {
        let mut s = path.into_os_string();
        s.push(".");
        s.push(dc_project::FILE_EXTENSION);
        PathBuf::from(s)
    }
}

/// The project to write: `manifest` (or a new one) with the modification
/// time set to now, `state`, its step history and the thumbnail. `Err`: the
/// history could not be written as JSON (then nothing is saved).
pub fn build(
    manifest: Option<&Manifest>,
    state: &Arc<CaseState>,
    history: &StepHistory,
    thumbnail: Option<&Vec<u8>>,
) -> Result<Project, String> {
    let history =
        serde_json::to_value(history).map_err(|e| format!("{}: {e}", dc_project::HISTORY_PATH))?;
    let now = now_rfc3339();
    let manifest = match manifest {
        Some(m) => {
            let mut m = m.clone();
            m.modified = now;
            m
        }
        None => Manifest::new(app_info(), now),
    };
    Ok(Project {
        manifest,
        case_state: CaseState::clone(state),
        history,
        thumbnail: thumbnail.cloned(),
    })
}

/// Saves `project` to `path` (atomic, see [`dc_project::save`]). `Ok` holds
/// the warnings of a save that succeeded (old backup kept, folder not synced).
pub fn save(path: &Path, project: &Project) -> Result<Vec<dc_project::SaveWarning>, String> {
    dc_project::save(path, project).map_err(|e| error_text(&e))
}

/// The words of a failure to save or open a project, in the language of the
/// app. The errors this release added or changed have their own message
/// (the entry and the path stay in it, as the technical detail); the others
/// keep the text of the error itself.
pub fn error_text(error: &dc_project::DcprjError) -> String {
    use dc_project::DcprjError;
    match error {
        DcprjError::NonFinite { entry, path } => {
            crate::setup::tr("ws-project-nonfinite", &[("entry", entry), ("path", path)])
        }
        DcprjError::Corrupt(why) if why == "duplicate archive entry names" => {
            crate::t("ws-project-duplicate-entries")
        }
        e => e.to_string(),
    }
}

/// The words of a problem found while opening a project, in the language of
/// the app (the technical detail stays in it). Problems without their own
/// message keep the text of the issue itself.
pub fn issue_text(issue: &dc_project::LoadIssue) -> String {
    use dc_project::LoadIssue;
    match issue {
        LoadIssue::MissingBlob(r) => {
            let id = r.to_hex();
            crate::setup::tr("ws-project-missing-model", &[("id", &id[..12])])
        }
        LoadIssue::UnreadableEntry { path, reason } if path == dc_project::HISTORY_PATH => {
            crate::setup::tr("ws-project-history-unreadable", &[("reason", reason)])
        }
        LoadIssue::UnreadableEntry { path, reason } => match size_mismatch(reason) {
            Some((found, expected)) => crate::setup::tr(
                "ws-project-entry-size",
                &[("path", path), ("found", found), ("expected", expected)],
            ),
            None => crate::setup::tr(
                "ws-project-unreadable",
                &[("path", path), ("reason", reason)],
            ),
        },
        i => i.to_string(),
    }
}

/// The problems of `issues` that stop the file from being written over,
/// worded for the user and joined (at most the first three).
pub fn blocking_issues_text(issues: &[dc_project::LoadIssue]) -> String {
    const SHOWN: usize = 3;
    let blocking: Vec<String> = issues
        .iter()
        .filter(|i| i.blocks_writing())
        .map(issue_text)
        .collect();
    let mut text = blocking
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if blocking.len() > SHOWN {
        text.push(' ');
        text.push_str(&crate::setup::tr(
            "ws-project-more-issues",
            &[("n", &(blocking.len() - SHOWN).to_string())],
        ));
    }
    text
}

/// The sizes of an entry whose size differs from the one in the manifest
/// (`size 12 differs from the manifest (10 bytes)`): found, expected.
fn size_mismatch(reason: &str) -> Option<(&str, &str)> {
    let rest = reason.strip_prefix("size ")?;
    let (found, rest) = rest.split_once(" differs from the manifest (")?;
    let expected = rest.strip_suffix(" bytes)")?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(found) && digits(expected)).then_some((found, expected))
}

/// `.dcprj` codecs for `module`: its [`Module::data_codec`] registered under
/// its id. Pass the result to [`dc_project::load`].
pub fn project_codecs(module: &dyn Module) -> ModuleCodecs {
    let mut codecs = ModuleCodecs::new();
    codecs.register(module.id(), module.data_codec());
    codecs
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_project::{DcprjError, LoadIssue};

    fn nonfinite() -> DcprjError {
        DcprjError::NonFinite {
            entry: "modules/aligners/state.json".to_owned(),
            path: "/setup/poses/11/t/0".to_owned(),
        }
    }

    #[test]
    fn a_save_error_is_worded_in_english_and_keeps_its_detail() {
        let text = error_text(&nonfinite());
        assert!(
            text.contains("modules/aligners/state.json") && text.contains("/setup/poses/11/t/0"),
            "{text}"
        );
        assert!(text.contains("previous file is unchanged"), "{text}");
    }

    #[test]
    fn the_new_errors_and_issues_have_their_own_words() {
        let dup = error_text(&DcprjError::Corrupt("duplicate archive entry names".into()));
        assert!(dup.contains("more than once"), "{dup}");
        // an error without a message of its own keeps its text
        let other = DcprjError::Corrupt("manifest.entries is missing".into());
        assert_eq!(error_text(&other), other.to_string());
        let blob = dc_case::BlobRef::of_bytes(b"x");
        let text = issue_text(&LoadIssue::MissingBlob(blob));
        assert!(text.contains(&blob.to_hex()[..12]), "{text}");
        let hist = issue_text(&LoadIssue::UnreadableEntry {
            path: dc_project::HISTORY_PATH.to_owned(),
            reason: "expected value".to_owned(),
        });
        assert!(
            hist.contains("expected value") && hist.contains("steps"),
            "{hist}"
        );
        let size = issue_text(&LoadIssue::UnreadableEntry {
            path: "case.json".to_owned(),
            reason: "size 12 differs from the manifest (10 bytes)".to_owned(),
        });
        assert!(
            size.contains("case.json") && size.contains("12") && size.contains("10"),
            "{size}"
        );
        // another reason keeps the path and the reason
        let io = issue_text(&LoadIssue::UnreadableEntry {
            path: "blobs/a.dcm".to_owned(),
            reason: "unexpected end of file".to_owned(),
        });
        assert!(io.contains("blobs/a.dcm") && io.contains("unexpected end of file"));
    }

    #[test]
    fn only_the_blocking_issues_are_listed_and_the_list_is_short() {
        let blob = |n: u8| LoadIssue::MissingBlob(dc_case::BlobRef::of_bytes(&[n]));
        let info = LoadIssue::ModuleKeptOpaque { id: "x".into() };
        assert_eq!(blocking_issues_text(std::slice::from_ref(&info)), "");
        let text = blocking_issues_text(&[blob(1), info, blob(2), blob(3), blob(4), blob(5)]);
        assert_eq!(text.matches("could not be loaded").count(), 3, "{text}");
        assert!(text.contains("(and 2 more problems)"), "{text}");
    }

    #[test]
    fn rfc3339_formatting() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_793_445_605), "2026-10-31T11:20:05Z");
        assert!(now_rfc3339().ends_with('Z'));
    }

    #[test]
    fn extension_is_added() {
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a")),
            PathBuf::from("/tmp/a.dcprj")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a.DCPRJ")),
            PathBuf::from("/tmp/a.DCPRJ")
        );
        assert_eq!(
            with_extension(PathBuf::from("/tmp/a.stl")),
            PathBuf::from("/tmp/a.stl.dcprj")
        );
    }
}
