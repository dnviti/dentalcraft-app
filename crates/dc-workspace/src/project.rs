//! Project file helpers: building the [`Project`] to save, timestamps and
//! the `.dcprj` codecs of a module.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dc_case::CaseState;
use dc_module_api::Module;
use dc_project::{AppInfo, Manifest, ModuleCodecs, Project};
use serde_json::Value;

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
/// time set to now, `state`, the step history JSON and the thumbnail.
pub fn build(
    manifest: Option<&Manifest>,
    state: &Arc<CaseState>,
    history: &Value,
    thumbnail: Option<&Vec<u8>>,
) -> Project {
    let now = now_rfc3339();
    let manifest = match manifest {
        Some(m) => {
            let mut m = m.clone();
            m.modified = now;
            m
        }
        None => Manifest::new(app_info(), now),
    };
    Project {
        manifest,
        case_state: CaseState::clone(state),
        history: history.clone(),
        thumbnail: thumbnail.cloned(),
    }
}

/// Saves `project` to `path` (atomic, see [`dc_project::save`]).
pub fn save(path: &Path, project: &Project) -> Result<(), String> {
    dc_project::save(path, project).map_err(|e| e.to_string())
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
