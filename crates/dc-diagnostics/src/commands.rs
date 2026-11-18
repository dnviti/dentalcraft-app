//! The diagnostics commands that `dentalcraft-cli` (and its MCP server)
//! and the app bridge list, describe and run: [`STATUS`], [`SET`],
//! [`PREVIEW`] and [`SEND`].
//!
//! They act on the installation, not on the case: the host runs them
//! (`dc-cli` with [`Headless`], the app with its own consent, so its
//! account menu follows).

use dc_case::CaseState;
use dc_command::{CommandError, JobCtx};
use dc_module_api::{CommandOutput, CommandSpec};
use serde_json::{Value, json};

use crate::settings::{self, Consent, Paths};
use crate::{Config, Diagnostics};

/// Source name of these commands in the command lists.
pub const SOURCE: &str = "app";
/// `app.diagnostics.status`.
pub const STATUS: &str = "app.diagnostics.status";
/// `app.diagnostics.set`.
pub const SET: &str = "app.diagnostics.set";
/// `app.diagnostics.preview`.
pub const PREVIEW: &str = "app.diagnostics.preview";
/// `app.diagnostics.send`.
pub const SEND: &str = "app.diagnostics.send";

fn handled_by_host(_: &CaseState, _: &Value, _: &JobCtx) -> Result<CommandOutput, CommandError> {
    Err(CommandError::Invalid(
        "app command: the CLI or the app runs it".into(),
    ))
}

fn no_params() -> Value {
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
        "title": "SetDiagnosticsParams",
        "type": "object",
        "properties": {
            "enabled": {
                "description": "true: send anonymous diagnostic data (a new random installation number is made); false: stop, and delete the reports not sent yet and the number.",
                "type": "boolean"
            }
        },
        "required": ["enabled"],
        "additionalProperties": false
    })
}

/// The commands, in listing order. Registers the bundle of their labels.
pub fn specs() -> Vec<CommandSpec> {
    crate::loader();
    let spec = |id, label_key, doc_key, params_schema| CommandSpec {
        id,
        label_key,
        doc_key,
        params_schema,
        run: handled_by_host,
    };
    vec![
        spec(
            STATUS,
            "app-cmd-diagnostics-status",
            "app-cmd-diagnostics-status-doc",
            no_params as fn() -> Value,
        ),
        spec(
            SET,
            "app-cmd-diagnostics-set",
            "app-cmd-diagnostics-set-doc",
            set_params,
        ),
        spec(
            PREVIEW,
            "app-cmd-diagnostics-preview",
            "app-cmd-diagnostics-preview-doc",
            no_params,
        ),
        spec(
            SEND,
            "app-cmd-diagnostics-send",
            "app-cmd-diagnostics-send-doc",
            no_params,
        ),
    ]
}

/// `true` for the diagnostics commands.
pub fn is_diagnostics_command(id: &str) -> bool {
    [STATUS, SET, PREVIEW, SEND].contains(&id)
}

/// Checks that command `id` got no parameters (`null` or `{}`).
pub fn no_params_given(params: &Value) -> Result<(), String> {
    match params {
        Value::Null => Ok(()),
        Value::Object(m) if m.is_empty() => Ok(()),
        Value::Object(m) => Err(format!(
            "unknown parameter `{}`",
            m.keys().next().map_or("", String::as_str)
        )),
        _ => Err("parameters: expected an object".into()),
    }
}

/// The `enabled` flag of [`SET`].
pub fn enabled_param(params: &Value) -> Result<bool, String> {
    let Value::Object(m) = params else {
        return Err("parameters: expected an object with `enabled`".into());
    };
    if let Some(k) = m.keys().find(|k| *k != "enabled") {
        return Err(format!("unknown parameter `{k}`"));
    }
    match m.get("enabled") {
        Some(Value::Bool(b)) => Ok(*b),
        None | Some(Value::Null) => Err("`enabled` is required".into()),
        Some(_) => Err("`enabled` must be true or false".into()),
    }
}

/// The output of [`STATUS`]: `asked` is the stored consent (`None`: the
/// user was not asked yet).
pub fn status_json(diag: &Diagnostics, asked: Option<bool>) -> Value {
    json!({
        "enabled": asked,
        "install_id": diag.install_id(),
        "pending": diag.pending(),
        "endpoint": diag.endpoint(),
    })
}

/// The commands without the app: the consent in the app's settings file,
/// the queue in its data folder, reports to its sign-in server. The
/// running app reads the setting at its next start.
pub struct Headless {
    paths: Paths,
    diag: Diagnostics,
}

impl Headless {
    /// On the app's standard folders.
    pub fn standard() -> Self {
        Self::new(Paths::standard())
    }

    /// On the folders `paths`.
    pub fn new(paths: Paths) -> Self {
        let file = paths.settings();
        let consent = settings::read_consent(&file);
        let issuer = settings::resolve_issuer(settings::read_saved_issuer(&file).as_deref());
        let diag = Diagnostics::new(Config::standard(&paths, &issuer), &consent);
        Self { paths, diag }
    }

    /// The diagnostics.
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diag
    }

    /// Runs command `id` (blocking: [`SEND`] waits for the server).
    pub fn run(&self, id: &str, params: &Value) -> Result<Value, String> {
        let file = self.paths.settings();
        match id {
            SET => {
                let on = enabled_param(params)?;
                let install_id = self.diag.set_enabled(on);
                settings::write_consent(
                    &file,
                    &Consent {
                        enabled: Some(on),
                        install_id,
                    },
                )
                .map_err(|e| format!("cannot save the setting: {e}"))?;
                Ok(status_json(&self.diag, Some(on)))
            }
            STATUS | PREVIEW | SEND => {
                no_params_given(params)?;
                match id {
                    STATUS => Ok(status_json(
                        &self.diag,
                        settings::read_consent(&file).enabled,
                    )),
                    PREVIEW => Ok(self.diag.preview()),
                    _ => send_result(self.diag.flush()),
                }
            }
            _ => Err(format!("unknown command {id}")),
        }
    }
}

/// The answer of [`SEND`] for a [`Diagnostics::flush`].
pub fn send_result(r: Result<crate::Flush, String>) -> Result<Value, String> {
    let f = r?;
    match &f.retry {
        Some(reason) => Err(format!(
            "the diagnostics were not sent ({reason}); {} events wait and are sent later",
            f.remaining
        )),
        None => Ok(f.to_json()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_are_listed_with_their_words_and_schemas() {
        let specs = specs();
        assert_eq!(
            specs.iter().map(|s| s.id).collect::<Vec<_>>(),
            [STATUS, SET, PREVIEW, SEND]
        );
        for s in &specs {
            assert!(is_diagnostics_command(s.id));
            assert!(dc_i18n::has(s.label_key), "{}", s.label_key);
            assert!(dc_i18n::has(s.doc_key), "{}", s.doc_key);
            assert_eq!(s.schema()["type"], "object");
            assert!(
                s.execute(&CaseState::default(), &Value::Null, &JobCtx::detached())
                    .is_err()
            );
        }
        assert!(!is_diagnostics_command("app.diagnostics"));
        assert_eq!(specs[1].schema()["required"][0], "enabled");
    }

    #[test]
    fn parameters() {
        assert_eq!(enabled_param(&json!({"enabled": true})), Ok(true));
        assert_eq!(enabled_param(&json!({"enabled": false})), Ok(false));
        assert!(enabled_param(&json!({})).is_err());
        assert!(enabled_param(&Value::Null).is_err());
        assert!(enabled_param(&json!({"enabled": "yes"})).is_err());
        assert!(enabled_param(&json!({"enabled": true, "x": 1})).is_err());
        assert!(no_params_given(&Value::Null).is_ok());
        assert!(no_params_given(&json!({})).is_ok());
        assert!(no_params_given(&json!({"x": 1})).is_err());
    }

    #[test]
    fn without_the_app_the_settings_file_holds_the_consent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config"),
            data: dir.path().join("data"),
        };
        let h = Headless::new(paths.clone());
        let st = h.run(STATUS, &Value::Null).unwrap();
        assert_eq!(
            (st["enabled"].clone(), st["pending"].clone()),
            (Value::Null, json!(0))
        );
        assert!(h.run(SEND, &Value::Null).is_err());
        let on = h.run(SET, &json!({"enabled": true})).unwrap();
        assert_eq!(on["enabled"], true);
        let id = on["install_id"].as_str().unwrap().to_owned();
        assert_eq!(
            settings::read_consent(&paths.settings())
                .install_id
                .as_deref(),
            Some(id.as_str())
        );
        // the next run reads it
        let again = Headless::new(paths.clone());
        assert_eq!(again.run(STATUS, &json!({})).unwrap()["install_id"], id);
        assert_eq!(
            again.run(PREVIEW, &Value::Null).unwrap()["report"],
            Value::Null
        );
        let off = again.run(SET, &json!({"enabled": false})).unwrap();
        assert_eq!(
            (off["enabled"].clone(), off["install_id"].clone()),
            (json!(false), Value::Null)
        );
        assert!(again.run(PREVIEW, &json!({"x": 1})).is_err());
        assert!(again.run("app.diagnostics.nope", &Value::Null).is_err());
    }
}
