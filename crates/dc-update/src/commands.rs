//! The update commands that `dentalcraft-cli` (and its MCP server) and the
//! app bridge list, describe and run: [`CHECK`] and [`APPLY`].
//!
//! They act on the installation, not on the case: their [`CommandSpec`]
//! carries the id, the i18n keys and the parameter schema, and the host
//! runs them (`dc-cli` with [`run_check`] and [`run_apply`], the app through
//! its update dialog).

use std::path::Path;

use dc_case::CaseState;
use dc_command::{CommandError, JobCtx};
use dc_module_api::{CommandOutput, CommandSpec};
use serde_json::{Value, json};

use crate::{Action, Check, Installation, Source, UpdateError};

/// Source name of these commands in the command lists.
pub const SOURCE: &str = "app";
/// `app.update.check`.
pub const CHECK: &str = "app.update.check";
/// `app.update.apply`.
pub const APPLY: &str = "app.update.apply";

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

fn apply_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "ApplyParams",
        "type": "object",
        "properties": {
            "restart": {
                "description": "App only: once the update is ready, save the project, close and open the new version now (default false: it is installed when the app closes).",
                "type": "boolean",
                "default": false
            }
        },
        "additionalProperties": false
    })
}

/// The commands, in listing order. Registers the bundle of their labels.
pub fn specs() -> Vec<CommandSpec> {
    crate::loader();
    vec![
        CommandSpec {
            id: CHECK,
            label_key: "app-cmd-update-check",
            doc_key: "app-cmd-update-check-doc",
            params_schema: no_params,
            run: handled_by_host,
        },
        CommandSpec {
            id: APPLY,
            label_key: "app-cmd-update-apply",
            doc_key: "app-cmd-update-apply-doc",
            params_schema: apply_params,
            run: handled_by_host,
        },
    ]
}

/// `true` for [`CHECK`] and [`APPLY`].
pub fn is_update_command(id: &str) -> bool {
    id == CHECK || id == APPLY
}

/// Reads the `restart` flag of [`APPLY`] (`null` = no parameters).
pub fn restart_param(params: &Value) -> Result<bool, String> {
    match params {
        Value::Null => Ok(false),
        Value::Object(m) => {
            if let Some(k) = m.keys().find(|k| *k != "restart") {
                return Err(format!("unknown parameter `{k}`"));
            }
            match m.get("restart") {
                None | Some(Value::Null) => Ok(false),
                Some(Value::Bool(b)) => Ok(*b),
                Some(_) => Err("`restart` must be true or false".into()),
            }
        }
        _ => Err("parameters: expected an object".into()),
    }
}

/// [`CHECK`] for the installation of the running program.
pub fn run_check(source: &Source) -> Result<Value, UpdateError> {
    crate::check(source, Installation::detect()).map(|c| c.to_json())
}

/// [`APPLY`] without a window (the CLI): checks, and when a newer version
/// exists for an installation DentalCraft updates by itself, downloads it,
/// checks it and installs it now (a Windows installer runs once this
/// process ends). `progress(done, total)` follows the download. The output
/// is the check's, plus `updated` and, when installed, `installed`.
pub fn run_apply(
    source: &Source,
    work_dir: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<Value, UpdateError> {
    let check = crate::check(source, Installation::detect())?;
    apply_now(&check, work_dir, progress)
}

/// The install half of [`run_apply`], for a check already made.
pub fn apply_now(
    check: &Check,
    work_dir: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<Value, UpdateError> {
    let mut out = check.to_json();
    if !matches!(check.action, Action::Install { .. }) {
        out["updated"] = json!(false);
        return Ok(out);
    }
    let prepared = crate::prepare(
        check,
        work_dir,
        progress,
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let later = prepared.runs_installer();
    prepared.finish(false)?;
    out["updated"] = json!(true);
    out["installed"] = json!(if later {
        "when this program ends"
    } else {
        "now"
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InstallKind, Manifest};

    #[test]
    fn the_commands_are_listed_with_their_words_and_schemas() {
        let specs = specs();
        assert_eq!(
            specs.iter().map(|s| s.id).collect::<Vec<_>>(),
            [CHECK, APPLY]
        );
        for s in &specs {
            assert!(is_update_command(s.id));
            assert!(dc_i18n::has(s.label_key), "{}", s.label_key);
            assert!(dc_i18n::has(s.doc_key), "{}", s.doc_key);
            assert_eq!(s.schema()["type"], "object");
            // the case is never touched
            assert!(
                s.execute(&CaseState::default(), &Value::Null, &JobCtx::detached())
                    .is_err()
            );
        }
        assert!(!is_update_command("app.update"));
        assert_eq!(
            specs[1].schema()["properties"]["restart"]["type"],
            "boolean"
        );
    }

    #[test]
    fn restart_is_the_only_parameter() {
        assert_eq!(restart_param(&Value::Null), Ok(false));
        assert_eq!(restart_param(&json!({})), Ok(false));
        assert_eq!(restart_param(&json!({"restart": true})), Ok(true));
        assert!(restart_param(&json!({"restart": "yes"})).is_err());
        assert!(restart_param(&json!({"force": true})).is_err());
        assert!(restart_param(&json!([true])).is_err());
    }

    #[test]
    fn a_package_manager_installation_is_not_touched() {
        let manifest = Manifest::parse(include_bytes!("../tests/fixtures/newer.json")).unwrap();
        let check = Check::new(
            "0.1.0",
            manifest,
            Installation::new(InstallKind::Flatpak, "/app/bin"),
        );
        let dir = tempfile::tempdir().unwrap();
        let out = apply_now(&check, dir.path(), &|_, _| {}).unwrap();
        assert_eq!(out["updated"], false);
        assert_eq!(out["action"], "package-manager");
        assert_eq!(out["command"], "flatpak update com.dentalcraft.DentalCraft");
    }
}
