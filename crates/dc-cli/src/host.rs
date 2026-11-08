//! [`CliHost`]: the headless [`Session`] with the Aligners module, plus a
//! CPU render (dc-raster) of the case.

use std::collections::BTreeMap;

use dc_case::{BlobRef, CaseState, ScanRole};
use dc_mcp::{Host, HostError, HostResult};
use dc_raster::RenderRequest;
use dc_scene::SceneEntry;
use dc_session::Session;
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

    /// PNG of the case: the `view.set` defaults overridden by `params` (see
    /// [`RenderRequest`]). Besides its keys, `model` (`"scan"` or `"teeth"`,
    /// default `"teeth"`) chooses between the scans and the built teeth and
    /// gingiva, and `stage` (default none: the initial position) poses the
    /// teeth at a treatment stage.
    pub fn render_png(&self, params: &Value) -> HostResult<Vec<u8>> {
        let empty = Map::new();
        let call = match params {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => return Err(HostError::new("render parameters must be an object")),
        };
        let req = RenderRequest::from_layers(&[self.session.view(), call]).map_err(HostError)?;
        let state = self.session.state();
        let entries = render_entries(&[self.session.view(), call], true, None, |stage| {
            self.session.modules().scene_at(state, stage)
        })
        .map_err(HostError)?;
        req.render_case_png_with(state, &aligner_labels(state), &entries)
            .map_err(HostError)
    }
}

/// What the render draws of the modules' entries (the built teeth and
/// gingiva), from the `model` and `stage` keys of the request (the last of
/// `layers` that has one wins; `view.set` stores them as defaults):
///
/// - `model`: `"teeth"` draws the entries (they replace the scan of their
///   arch), `"scan"` draws the scans and none of the entries of an arch
///   they replace.
/// - `stage`: the treatment stage that poses the 3D teeth and moves the
///   gingiva (`0` or absent: the initial position). The scans are drawn at
///   rest whatever the stage.
///
/// `at` gives the entries at a stage; `default_teeth` and `default_stage`
/// apply when the keys are absent.
fn render_entries(
    layers: &[&Map<String, Value>],
    default_teeth: bool,
    default_stage: Option<u32>,
    at: impl Fn(Option<u32>) -> Vec<SceneEntry>,
) -> Result<Vec<SceneEntry>, String> {
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
    let rest = at(None);
    if teeth {
        return Ok(match stage {
            Some(n) if n > 0 => at(Some(n)),
            _ => rest,
        });
    }
    let replaced: Vec<_> = rest
        .iter()
        .filter(|e| e.hides_scan)
        .filter_map(|e| e.arch)
        .collect();
    Ok(rest
        .into_iter()
        .filter(|e| !e.arch.is_some_and(|a| replaced.contains(&a)))
        .collect())
}

impl Host for CliHost {
    fn execute(&mut self, id: &str, params: Value) -> HostResult {
        Host::execute(&mut self.session, id, params)
    }
    fn batch(&mut self, commands: Vec<(String, Value)>) -> HostResult {
        Host::batch(&mut self.session, commands)
    }
    fn list(&mut self) -> HostResult {
        Host::list(&mut self.session)
    }
    fn describe(&mut self, id: &str) -> HostResult {
        Host::describe(&mut self.session, id)
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

    /// A tooth of the lower arch (hides its scan) and an untagged entry; at
    /// a stage the tooth is moved.
    fn entries_at(stage: Option<u32>) -> Vec<SceneEntry> {
        let mut state = CaseState::new(Case::new("T"));
        let blob = state.blobs.insert(TriMesh::new(vec![], vec![]));
        let item = |at: f64| SceneItem::Mesh {
            blob,
            coloring: MeshColoring::Uniform(Color::TOOTH),
            xform: Pose::from_translation(glam::DVec3::X * at),
            pickable: true,
        };
        vec![
            SceneEntry::new(item(f64::from(stage.unwrap_or(0))))
                .with_arch(ScanRole::Lower)
                .with_tooth(31)
                .hiding_scan(),
            SceneEntry::new(item(0.0)),
        ]
    }

    fn pick(params: Value, teeth: bool, stage: Option<u32>) -> Result<Vec<SceneEntry>, String> {
        let m = params.as_object().unwrap().clone();
        render_entries(&[&m], teeth, stage, entries_at)
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
        assert_eq!((e.len(), shift(&e[0])), (2, 0.0));
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
            1
        );
        // scan: the entries of an arch whose scan they replace are left out
        let e = pick(json!({"model": "Scan", "stage": 3}), true, None).unwrap();
        assert_eq!(e.len(), 1);
        assert!(e[0].tooth.is_none());
        assert_eq!(pick(json!({}), false, None).unwrap().len(), 1);
        assert_eq!(pick(json!({"model": null}), false, None).unwrap().len(), 1);
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
}
