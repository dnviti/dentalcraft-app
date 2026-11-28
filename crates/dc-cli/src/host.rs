//! [`CliHost`]: the headless [`Session`] with the Aligners module, plus a
//! CPU render (dc-raster) of the case and the app's commands: updates
//! (`app.update.check`, `app.update.apply`, run on this installation),
//! anonymous diagnostics (`app.diagnostics.*`, on the app's settings and
//! queue) and settings (`app.settings.*`, on the app's settings file).

use std::collections::BTreeMap;

use dc_case::{BlobRef, CaseState, ScanRole};
use dc_diagnostics::commands::{self as diagnostics, is_diagnostics_command};
use dc_diagnostics::settings::{self as app_settings, is_settings_command};
use dc_mcp::{Host, HostError, HostResult};
use dc_module_api::CommandSpec;
use dc_raster::RenderRequest;
use dc_scene::SceneEntry;
use dc_session::{CommandDescription, CommandInfo, Session};
use dc_update::commands::{self as update, is_update_command};
use dca_model::AlignersState;
use serde_json::{Map, Value};

/// Tooth label meshes of the Aligners module state, by arch (empty when
/// the case has no labels yet).
pub fn aligner_labels(state: &CaseState) -> BTreeMap<ScanRole, BlobRef> {
    state
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .map(|s| s.labels.clone())
        .unwrap_or_default()
}

/// Headless host: a [`Session`] with the Aligners module registered.
pub struct CliHost {
    /// The session.
    pub session: Session,
}

impl CliHost {
    /// New session on an empty case, with the Aligners module.
    pub fn new() -> Result<Self, HostError> {
        dca_module::loader();
        let mut session = Session::new();
        session
            .register_module(Box::new(dca_module::AlignersModule::new()))
            .map_err(|e| HostError::new(e.to_string()))?;
        Ok(Self { session })
    }

    /// Every command: the session's and the module's, then the app's
    /// update, diagnostics and settings commands.
    pub fn list_commands(&self) -> Vec<CommandInfo> {
        let mut list = self.session.list_commands();
        list.extend(app_specs().into_iter().map(|s| CommandInfo {
            id: s.id.to_owned(),
            source: update::SOURCE.to_owned(),
            label: dc_i18n::tr(s.label_key),
        }));
        list
    }

    /// Schema and documentation of command `id`.
    pub fn describe_command(&self, id: &str) -> Result<CommandDescription, String> {
        let Some(s) = app_specs().into_iter().find(|s| s.id == id) else {
            return self.session.describe(id).map_err(|e| e.to_string());
        };
        Ok(CommandDescription {
            id: s.id.to_owned(),
            source: update::SOURCE.to_owned(),
            label_key: s.label_key.to_owned(),
            label: dc_i18n::tr(s.label_key),
            doc_key: s.doc_key.to_owned(),
            doc: dc_i18n::tr(s.doc_key),
            params_schema: s.schema(),
        })
    }

    /// `app.diagnostics.*` on the app's settings file and queue (`send`
    /// waits for the server).
    fn run_diagnostics(&self, id: &str, params: &Value) -> HostResult {
        diagnostics::Headless::standard()
            .run(id, params)
            .map_err(|e| HostError::new(format!("{id}: {e}")))
    }

    /// `app.settings.*` on the app's settings file (the running app reads
    /// it at its next start).
    fn run_settings(&self, id: &str, params: &Value) -> HostResult {
        let file = app_settings::Paths::standard().settings();
        app_settings::run_headless(&file, id, params)
            .map_err(|e| HostError::new(format!("{id}: {e}")))
    }

    /// `app.update.check` / `app.update.apply` on the installation of this
    /// program (`apply` installs at once: `restart` is for the app).
    fn run_update(&self, id: &str, params: &Value) -> HostResult {
        let source = dc_update::Source::standard();
        let r = if id == update::CHECK {
            if !params.as_object().is_none_or(Map::is_empty) {
                return Err(HostError::new(format!("{id}: takes no parameters")));
            }
            update::run_check(&source)
        } else {
            update::restart_param(params).map_err(|e| HostError::new(format!("{id}: {e}")))?;
            update::run_apply(&source, &dc_update::work_dir(), &|_, _| {})
        };
        r.map_err(|e| HostError::new(format!("{id}: {e}")))
    }

    /// PNG of the case: the `view.set` defaults overridden by `params` (see
    /// [`RenderRequest`]). Besides its keys, `model` (`"scan"` or `"teeth"`,
    /// default `"teeth"`) chooses between the scans and the built teeth and
    /// gingiva, `stage` (default none: the initial position) poses the
    /// teeth at a treatment stage, and `study` (default `true`) draws the
    /// study models at the initial position, each in place of its arch. The
    /// aligner the case shows in the view (`aligners.view.aligner`) is drawn
    /// over whatever its arch shows.
    pub fn render_png(&self, params: &Value) -> HostResult<Vec<u8>> {
        let empty = Map::new();
        let call = match params {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => return Err(HostError::new("render parameters must be an object")),
        };
        let req = RenderRequest::from_layers(&[self.session.view(), call]).map_err(HostError)?;
        let state = self.session.state();
        let over = |e: &SceneEntry| is_view_aligner(state, e);
        let (entries, study) = render_entries(
            &[self.session.view(), call],
            true,
            None,
            true,
            |stage| self.session.modules().scene_at(state, stage),
            over,
        )
        .map_err(HostError)?;
        // the aligner of the view replaces nothing: over the study model too
        let (aligner, entries): (Vec<_>, Vec<_>) = entries.into_iter().partition(over);
        let mut entries = dca_module::study_models_drawn(state, entries, study);
        entries.extend(aligner);
        req.render_case_png_with(state, &aligner_labels(state), &entries)
            .map_err(HostError)
    }
}

/// The app's commands (source `app`): updates, diagnostics, then settings.
fn app_specs() -> Vec<CommandSpec> {
    update::specs()
        .into_iter()
        .chain(diagnostics::specs())
        .chain(app_settings::specs())
        .collect()
}

/// What the render draws of the modules' entries (the built teeth and
/// gingiva), from the `model`, `stage` and `study` keys of the request (the
/// last of `layers` that has one wins; `view.set` stores them as defaults):
///
/// - `model`: `"teeth"` draws the entries (they replace the scan of their
///   arch), `"scan"` draws the scans and none of the entries of an arch
///   they replace, but those `over` names: they replace nothing and go over
///   whatever the arch shows (the aligner of the view).
/// - `stage`: the treatment stage that poses the 3D teeth and moves the
///   gingiva (`0` or absent: the initial position). The scans are drawn at
///   rest whatever the stage.
/// - `study`: the study models are drawn (`dca_module::study_models_drawn`),
///   only at the initial position; the flag returned says so.
///
/// `at` gives the entries at a stage; `default_teeth`, `default_stage` and
/// `default_study` apply when the keys are absent.
fn render_entries(
    layers: &[&Map<String, Value>],
    default_teeth: bool,
    default_stage: Option<u32>,
    default_study: bool,
    at: impl Fn(Option<u32>) -> Vec<SceneEntry>,
    over: impl Fn(&SceneEntry) -> bool,
) -> Result<(Vec<SceneEntry>, bool), String> {
    let get = |key: &str| {
        layers
            .iter()
            .rev()
            .find_map(|l| l.get(key).filter(|v| !v.is_null()))
    };
    let teeth = match get("model") {
        None => default_teeth,
        Some(v) => match v.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("teeth") => true,
            Some("scan") => false,
            _ => return Err("`model` must be \"scan\" or \"teeth\"".into()),
        },
    };
    let stage = match get("stage") {
        None => default_stage,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("`stage` must be a non-negative integer")?,
        ),
    };
    let study = match get("study") {
        None => default_study,
        Some(v) => v.as_bool().ok_or("`study` must be true or false")?,
    } && stage.unwrap_or(0) == 0;
    let rest = at(None);
    if teeth {
        let entries = match stage {
            Some(n) if n > 0 => at(Some(n)),
            _ => rest,
        };
        return Ok((entries, study));
    }
    let replaced: Vec<_> = rest
        .iter()
        .filter(|e| e.hides_scan)
        .filter_map(|e| e.arch)
        .collect();
    let entries = rest
        .into_iter()
        .filter(|e| over(e) || !e.arch.is_some_and(|a| replaced.contains(&a)))
        .collect();
    Ok((entries, study))
}

/// `e` is the aligner of the 3D view (`aligners.view.aligner`, while the
/// case shows it), drawn over whatever its arch shows, as the app's
/// viewport draws it.
fn is_view_aligner(state: &CaseState, e: &SceneEntry) -> bool {
    let dc_scene::SceneItem::Mesh { blob, .. } = &e.item else {
        return false;
    };
    state
        .module_data::<AlignersState>(dca_model::MODULE_ID)
        .is_some_and(|d| d.aligner_view.bases.iter().any(|b| b.mesh == *blob))
}

impl Host for CliHost {
    fn execute(&mut self, id: &str, params: Value) -> HostResult {
        if is_update_command(id) {
            return self.run_update(id, &params);
        }
        if is_diagnostics_command(id) {
            return self.run_diagnostics(id, &params);
        }
        if is_settings_command(id) {
            return self.run_settings(id, &params);
        }
        Host::execute(&mut self.session, id, params)
    }
    fn batch(&mut self, commands: Vec<(String, Value)>) -> HostResult {
        let app = |id: &str| {
            is_update_command(id) || is_diagnostics_command(id) || is_settings_command(id)
        };
        if !commands.iter().any(|(id, _)| app(id)) {
            return Host::batch(&mut self.session, commands);
        }
        // one at a time, as the session does: the first failure stops it
        let mut out = Vec::with_capacity(commands.len());
        for (index, (id, params)) in commands.into_iter().enumerate() {
            let v = self
                .execute(&id, params)
                .map_err(|e| HostError::new(format!("command {index} ({id}) failed: {}", e.0)))?;
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn list(&mut self) -> HostResult {
        serde_json::to_value(self.list_commands()).map_err(|e| HostError::new(e.to_string()))
    }
    fn describe(&mut self, id: &str) -> HostResult {
        let d = self.describe_command(id).map_err(HostError)?;
        serde_json::to_value(d).map_err(|e| HostError::new(e.to_string()))
    }
    fn inspect(&mut self, path: Option<&str>) -> HostResult {
        Host::inspect(&mut self.session, path)
    }
    fn undo(&mut self) -> HostResult {
        Host::undo(&mut self.session)
    }
    fn redo(&mut self) -> HostResult {
        Host::redo(&mut self.session)
    }
    fn open(&mut self, path: &str) -> HostResult {
        Host::open(&mut self.session, path)
    }
    fn save(&mut self, path: Option<&str>) -> HostResult {
        Host::save(&mut self.session, path)
    }
    fn render(&mut self, params: &Value) -> HostResult<Option<Vec<u8>>> {
        self.render_png(params).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::{Case, Pose};
    use dc_mesh::TriMesh;
    use dc_scene::{Color, MeshColoring, SceneItem};
    use serde_json::json;

    /// A tooth of the lower arch (hides its scan), an untagged entry and the
    /// aligner of the view over the lower arch; at a stage the tooth and the
    /// aligner are moved.
    fn entries_at(stage: Option<u32>) -> Vec<SceneEntry> {
        let mut state = CaseState::new(Case::new("T"));
        let blob = state.blobs.insert(TriMesh::new(vec![], vec![]));
        let item = |blob, at: f64| SceneItem::Mesh {
            blob,
            coloring: MeshColoring::Uniform(Color::TOOTH),
            xform: Pose::from_translation(glam::DVec3::X * at),
            pickable: true,
        };
        let at = f64::from(stage.unwrap_or(0));
        vec![
            SceneEntry::new(item(blob, at))
                .with_arch(ScanRole::Lower)
                .with_tooth(31)
                .hiding_scan(),
            SceneEntry::new(item(blob, 0.0)),
            SceneEntry::new(item(aligner_blob(), at)).with_arch(ScanRole::Lower),
        ]
    }

    /// The mesh of the aligner of the view in [`entries_at`].
    fn aligner_blob() -> BlobRef {
        BlobRef::of_bytes(b"aligner")
    }

    fn is_aligner(e: &SceneEntry) -> bool {
        matches!(&e.item, SceneItem::Mesh { blob, .. } if *blob == aligner_blob())
    }

    fn pick(params: Value, teeth: bool, stage: Option<u32>) -> Result<Vec<SceneEntry>, String> {
        let m = params.as_object().unwrap().clone();
        render_entries(&[&m], teeth, stage, true, entries_at, is_aligner).map(|(e, _)| e)
    }

    /// Whether the study models are drawn for `params`.
    fn study(params: Value, default: bool, stage: Option<u32>) -> Result<bool, String> {
        let m = params.as_object().unwrap().clone();
        render_entries(&[&m], true, stage, default, entries_at, is_aligner).map(|(_, s)| s)
    }

    fn shift(e: &SceneEntry) -> f64 {
        match &e.item {
            SceneItem::Mesh { xform, .. } => xform.t.x,
            _ => unreachable!(),
        }
    }

    #[test]
    fn model_and_stage_choose_the_entries() {
        // teeth (the default), no stage: at rest
        let e = pick(json!({}), true, None).unwrap();
        assert_eq!((e.len(), shift(&e[0])), (3, 0.0));
        // a stage poses them; stage 0 is the rest
        assert_eq!(
            shift(&pick(json!({"stage": 3}), true, None).unwrap()[0]),
            3.0
        );
        assert_eq!(
            shift(&pick(json!({"stage": 0}), true, Some(2)).unwrap()[0]),
            0.0
        );
        // the app's defaults apply when the keys are absent, the call overrides them
        assert_eq!(shift(&pick(json!({}), true, Some(2)).unwrap()[0]), 2.0);
        assert_eq!(
            pick(json!({"model": "scan"}), true, Some(2)).unwrap().len(),
            2
        );
        // scan: the entries of an arch whose scan they replace are left out,
        // but the aligner of the view, which replaces nothing
        let e = pick(json!({"model": "Scan", "stage": 3}), true, None).unwrap();
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|e| e.tooth.is_none()));
        assert!(e.iter().any(is_aligner));
        assert_eq!(pick(json!({}), false, None).unwrap().len(), 2);
        assert_eq!(pick(json!({"model": null}), false, None).unwrap().len(), 2);
        // nothing goes over the scan when nothing is named
        let m = json!({"model": "scan"}).as_object().unwrap().clone();
        let (e, _) = render_entries(&[&m], true, None, true, entries_at, |_| false).unwrap();
        assert_eq!(e.len(), 1);
        // bad values are explained
        for bad in [
            json!({"model": "gums"}),
            json!({"model": 1}),
            json!({"stage": -1}),
            json!({"stage": "2"}),
        ] {
            assert!(pick(bad.clone(), true, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_study_models_are_drawn_at_the_initial_position_unless_turned_off() {
        assert_eq!(study(json!({}), true, None), Ok(true));
        assert_eq!(study(json!({"stage": 0}), true, Some(3)), Ok(true));
        assert_eq!(study(json!({"model": "scan"}), true, None), Ok(true));
        // a later stage moves the teeth: the cast of the start is not drawn
        assert_eq!(study(json!({}), true, Some(2)), Ok(false));
        assert_eq!(study(json!({"stage": 2}), true, None), Ok(false));
        // the key, else the app's layer
        assert_eq!(study(json!({"study": false}), true, None), Ok(false));
        assert_eq!(study(json!({}), false, None), Ok(false));
        assert_eq!(study(json!({"study": true}), false, None), Ok(true));
        assert!(study(json!({"study": "yes"}), true, None).is_err());
    }

    /// The render draws the study models built by the command: with them
    /// the image differs, with `study: false` it is the one without them.
    #[test]
    fn a_render_draws_the_study_models_of_the_case() {
        let mut host = CliHost::new().unwrap();
        for arch in ["lower", "upper"] {
            Host::execute(&mut host, "aligners.demo", json!({ "arch": arch })).unwrap();
        }
        Host::execute(&mut host, "aligners.occlusion.virtual-bite", Value::Null).unwrap();
        let view = json!({"width": 64, "height": 48, "view": "frontal"});
        let before = host.render_png(&view).unwrap();
        Host::execute(
            &mut host,
            "aligners.output.study-models",
            json!({ "voxel_mm": 1.0, "tolerance_mm": 0.1 }),
        )
        .unwrap();
        let state = host.session.state();
        let data = state
            .module_data::<AlignersState>(dca_model::MODULE_ID)
            .unwrap();
        assert_eq!(data.study_models.len(), 2);
        let entries = host.session.modules().scene_at(state, None);
        let drawn = dca_module::study_models_drawn(state, entries, true);
        assert_eq!(
            drawn
                .iter()
                .filter(|e| dca_module::is_study_model(data, e) && e.hides_scan)
                .count(),
            2
        );
        assert_ne!(host.render_png(&view).unwrap(), before);
        let mut off = view.clone();
        off["study"] = json!(false);
        assert_eq!(host.render_png(&off).unwrap(), before);
    }

    #[test]
    fn a_render_of_the_empty_case_takes_model_and_stage() {
        let host = CliHost::new().unwrap();
        for p in [
            json!({"model": "scan", "stage": 2, "width": 8, "height": 8}),
            json!({"width": 8, "height": 8}),
        ] {
            assert!(host.render_png(&p).unwrap().starts_with(b"\x89PNG"));
        }
        assert!(host.render_png(&json!({"model": "x"})).is_err());
    }

    #[test]
    fn the_aligner_of_the_view_is_known_by_its_mesh() {
        let shell = BlobRef::of_bytes(b"shell");
        let entry = |blob| {
            SceneEntry::new(SceneItem::Mesh {
                blob,
                coloring: MeshColoring::Uniform(Color::TOOTH),
                xform: Pose::IDENTITY,
                pickable: false,
            })
            .with_arch(ScanRole::Lower)
        };
        let mut state = CaseState::new(Case::new("T"));
        assert!(!is_view_aligner(&state, &entry(shell)), "no aligners data");
        let mut al = AlignersState::default();
        al.aligner_view.shown = true;
        al.aligner_view.bases.push(dca_model::AlignerBase {
            arch: ScanRole::Lower,
            stage: 1,
            mesh: shell,
            exact: false,
            voxel_mm: 0.4,
            tolerance_mm: 0.08,
            key: String::new(),
            seconds: 1.0,
        });
        state.set_module(dca_model::MODULE_ID, dc_case::ModuleState::new(al));
        assert!(is_view_aligner(&state, &entry(shell)));
        assert!(!is_view_aligner(
            &state,
            &entry(BlobRef::of_bytes(b"tooth"))
        ));
    }
}
