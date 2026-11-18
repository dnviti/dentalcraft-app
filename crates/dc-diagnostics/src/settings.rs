//! The app's folders, its sign-in server and the consent as stored in its
//! `settings.json`, for the app and for `dentalcraft-cli`, which reads and
//! changes the same file.
//!
//! The consent is two keys of the settings object: `diagnostics` (`true`,
//! `false`, or absent / `null` while the user was not asked) and
//! `install_id`. [`write_consent`] changes only those two keys and keeps
//! everything else of the file.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Default sign-in server of debug builds (`dentalcraft-server dev`).
pub const DEV_ISSUER: &str = "http://127.0.0.1:8787";
/// Default sign-in server of release builds (`dentalcraft-server`).
/// `DC_AUTH_ISSUER` or the saved setting override it.
pub const RELEASE_ISSUER: &str = "https://auth.dentalcraft.app";
/// Environment variable overriding the sign-in server.
pub const ISSUER_ENV: &str = "DC_AUTH_ISSUER";

/// The default sign-in server of this build.
pub fn default_issuer() -> &'static str {
    if cfg!(debug_assertions) {
        DEV_ISSUER
    } else {
        RELEASE_ISSUER
    }
}

/// The sign-in server: `DC_AUTH_ISSUER`, else `saved` (the setting), else
/// the build default. The diagnostics go to the same server.
pub fn resolve_issuer(saved: Option<&str>) -> String {
    std::env::var(ISSUER_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| saved.map(str::to_owned))
        .unwrap_or_else(|| default_issuer().to_owned())
}

/// The consent to anonymous diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Consent {
    /// `None`: not asked yet.
    pub enabled: Option<bool>,
    /// The random number of this installation while diagnostics are on.
    pub install_id: Option<String>,
}

/// The app's folders (the same as the app's `Dirs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Settings (`settings.json`).
    pub config: PathBuf,
    /// Logs, crash reports and the diagnostics queue.
    pub data: PathBuf,
}

impl Paths {
    /// The standard folders (`~/.config/dentalcraft`,
    /// `~/.local/share/dentalcraft` on Linux), or under
    /// `./dentalcraft-data` without a home folder.
    pub fn standard() -> Self {
        let pd = directories::ProjectDirs::from("app", "DentalCraft", "dentalcraft");
        let fallback = PathBuf::from("dentalcraft-data");
        Self {
            config: pd
                .as_ref()
                .map_or(fallback.join("config"), |p| p.config_dir().to_path_buf()),
            data: pd
                .as_ref()
                .map_or(fallback.join("data"), |p| p.data_dir().to_path_buf()),
        }
    }

    /// The settings file.
    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// The log folder.
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }

    /// The diagnostics folder (its `queue` and state).
    pub fn diagnostics(&self) -> PathBuf {
        self.data.join("diagnostics")
    }
}

fn read_object(path: &Path) -> Map<String, Value> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The consent stored in the settings file `path` (not asked when the file
/// or the keys are missing).
pub fn read_consent(path: &Path) -> Consent {
    let m = read_object(path);
    Consent {
        enabled: m.get("diagnostics").and_then(Value::as_bool),
        install_id: m
            .get("install_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

/// The sign-in server saved in the settings file `path`, if any.
pub fn read_saved_issuer(path: &Path) -> Option<String> {
    read_object(path)
        .get("auth_issuer")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Stores `consent` in the settings file `path`, keeping its other keys
/// (written to a temporary file, then renamed).
pub fn write_consent(path: &Path, consent: &Consent) -> std::io::Result<()> {
    let mut m = read_object(path);
    m.insert(
        "diagnostics".into(),
        consent.enabled.map_or(Value::Null, Value::Bool),
    );
    m.insert(
        "install_id".into(),
        consent
            .install_id
            .clone()
            .map_or(Value::Null, Value::String),
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(m))?)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_consent_keeps_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("settings.json");
        assert_eq!(read_consent(&path), Consent::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"theme":"light","auth_issuer":"http://x"}"#).unwrap();
        assert_eq!(read_consent(&path).enabled, None);
        assert_eq!(read_saved_issuer(&path).as_deref(), Some("http://x"));
        let on = Consent {
            enabled: Some(true),
            install_id: Some("id-1".into()),
        };
        write_consent(&path, &on).unwrap();
        assert_eq!(read_consent(&path), on);
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "light");
        write_consent(
            &path,
            &Consent {
                enabled: Some(false),
                install_id: None,
            },
        )
        .unwrap();
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            (v["diagnostics"].clone(), v["install_id"].clone()),
            (Value::Bool(false), Value::Null)
        );
    }

    #[test]
    fn issuer_constants() {
        assert_eq!(RELEASE_ISSUER, "https://auth.dentalcraft.app");
        assert_eq!(DEV_ISSUER, "http://127.0.0.1:8787");
        assert_eq!(
            default_issuer(),
            if cfg!(debug_assertions) {
                DEV_ISSUER
            } else {
                RELEASE_ISSUER
            }
        );
    }

    #[test]
    fn the_folders_and_the_server() {
        let p = Paths::standard();
        assert!(p.settings().ends_with("settings.json"));
        assert!(p.diagnostics().starts_with(&p.data));
        assert_eq!(
            resolve_issuer(Some("http://saved")),
            std::env::var(ISSUER_ENV)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "http://saved".into())
        );
    }
}
