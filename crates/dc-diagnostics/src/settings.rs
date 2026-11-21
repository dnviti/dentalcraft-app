//! The app's folders, its sign-in server and the consent as stored in its
//! `settings.json`, for the app and for `dentalcraft-cli`, which reads and
//! changes the same file.
//!
//! The consent is two keys of the settings object: `diagnostics` (`true`,
//! `false`, or absent / `null` while the user was not asked) and
//! `install_id`. [`write_consent`] changes only those two keys and keeps
//! everything else of the file.
//!
//! The commands [`GET`] and [`SET`] (`app.settings.*`, source `app`) read
//! and change the settings of the Settings dialog: the CLI on this file
//! ([`run_headless`], keeping its other keys), the app bridge on the
//! running app, which saves the same file.

use std::path::{Path, PathBuf};

use dc_case::CaseState;
use dc_command::{CommandError, JobCtx};
use dc_module_api::{CommandOutput, CommandSpec};
use serde_json::{Map, Value, json};

use crate::commands::no_params_given;

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
    write_object(path, m)
}

/// Writes the settings object `m` to `path` (a temporary file, then
/// renamed).
fn write_object(path: &Path, m: Map<String, Value>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(m))?)?;
    std::fs::rename(tmp, path)
}

// ------------------------------------------------------------ app.settings.*

/// `app.settings.get`.
pub const GET: &str = "app.settings.get";
/// `app.settings.set`.
pub const SET: &str = "app.settings.set";

/// What `app.settings.set` changes (`None`: left as it is).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Change {
    /// The advanced options and parameters of the inspectors are shown.
    pub advanced: Option<bool>,
}

fn handled_by_host(_: &CaseState, _: &Value, _: &JobCtx) -> Result<CommandOutput, CommandError> {
    Err(CommandError::Invalid(
        "app command: the CLI or the app runs it".into(),
    ))
}

fn get_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "NoParams",
        "type": "object",
        "additionalProperties": false
    })
}

fn set_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SetSettingsParams",
        "type": "object",
        "properties": {
            "advanced": {
                "description": "true: the inspectors show every option and parameter; false (the default): only the automatic runs and the most useful manual corrections.",
                "type": "boolean"
            }
        },
        "minProperties": 1,
        "additionalProperties": false
    })
}

/// The settings commands, in listing order. Registers the bundle of their
/// labels.
pub fn specs() -> Vec<CommandSpec> {
    crate::loader();
    vec![
        CommandSpec {
            id: GET,
            label_key: "app-cmd-settings-get",
            doc_key: "app-cmd-settings-get-doc",
            params_schema: get_params,
            run: handled_by_host,
        },
        CommandSpec {
            id: SET,
            label_key: "app-cmd-settings-set",
            doc_key: "app-cmd-settings-set-doc",
            params_schema: set_params,
            run: handled_by_host,
        },
    ]
}

/// `true` for the settings commands.
pub fn is_settings_command(id: &str) -> bool {
    [GET, SET].contains(&id)
}

/// The parameters of [`SET`]: at least one known key, nothing else.
pub fn change_param(params: &Value) -> Result<Change, String> {
    let Value::Object(m) = params else {
        return Err("parameters: expected an object with `advanced`".into());
    };
    if let Some(k) = m.keys().find(|k| *k != "advanced") {
        return Err(format!("unknown parameter `{k}`"));
    }
    let advanced = match m.get("advanced") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err("`advanced` must be true or false".into()),
    };
    if advanced.is_none() {
        return Err("nothing to change: give `advanced`".into());
    }
    Ok(Change { advanced })
}

/// The output of [`GET`] for the settings object `s` (the app's defaults
/// for the missing keys): `advanced`, `language` (the system's when not
/// chosen), `theme`, `check_updates` and `diagnostics` (`null` while the
/// user was not asked).
pub fn summary(s: &Map<String, Value>) -> Value {
    let language = s
        .get("language")
        .and_then(Value::as_str)
        .and_then(dc_i18n::Lang::from_code)
        .unwrap_or_else(dc_i18n::Lang::from_system);
    let theme = s
        .get("theme")
        .and_then(Value::as_str)
        .filter(|t| ["dark", "light", "system"].contains(t))
        .unwrap_or("dark");
    json!({
        "advanced": s.get("advanced").and_then(Value::as_bool).unwrap_or(false),
        "language": language.code(),
        "theme": theme,
        "check_updates": s.get("check_updates").and_then(Value::as_bool).unwrap_or(true),
        "diagnostics": s.get("diagnostics").and_then(Value::as_bool),
    })
}

/// Stores `change` in the settings file `path`, keeping its other keys.
pub fn write_change(path: &Path, change: Change) -> std::io::Result<()> {
    let mut m = read_object(path);
    if let Some(on) = change.advanced {
        m.insert("advanced".into(), Value::Bool(on));
    }
    write_object(path, m)
}

/// Runs settings command `id` on the settings file `path` (without the
/// app: the running app reads the file at its next start).
pub fn run_headless(path: &Path, id: &str, params: &Value) -> Result<Value, String> {
    match id {
        GET => {
            no_params_given(params)?;
            Ok(summary(&read_object(path)))
        }
        SET => {
            let change = change_param(params)?;
            write_change(path, change).map_err(|e| format!("cannot save the setting: {e}"))?;
            Ok(summary(&read_object(path)))
        }
        _ => Err(format!("unknown command {id}")),
    }
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

    #[test]
    fn the_settings_commands_keep_the_other_keys_and_refuse_bad_params() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("settings.json");
        // a fresh installation: the defaults
        let s = run_headless(&path, GET, &Value::Null).unwrap();
        assert_eq!(
            (
                s["advanced"].clone(),
                s["theme"].clone(),
                s["check_updates"].clone(),
                s["diagnostics"].clone()
            ),
            (json!(false), json!("dark"), json!(true), Value::Null)
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"theme":"light","language":"it","menu_width":300}"#,
        )
        .unwrap();
        let s = run_headless(&path, SET, &json!({"advanced": true})).unwrap();
        assert_eq!(
            (
                s["advanced"].clone(),
                s["theme"].clone(),
                s["language"].clone()
            ),
            (json!(true), json!("light"), json!("it"))
        );
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            (v["advanced"].clone(), v["menu_width"].clone()),
            (json!(true), json!(300))
        );
        assert_eq!(
            run_headless(&path, GET, &json!({})).unwrap()["advanced"],
            true
        );
        for (bad, why) in [
            (json!({}), "nothing to change"),
            (Value::Null, "expected an object"),
            (json!({"advanced": "yes"}), "true or false"),
            (
                json!({"advanced": true, "theme": "dark"}),
                "unknown parameter `theme`",
            ),
        ] {
            let e = run_headless(&path, SET, &bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert!(run_headless(&path, GET, &json!({"x": 1})).is_err());
        assert!(specs().iter().all(|s| is_settings_command(s.id)));
    }
}
