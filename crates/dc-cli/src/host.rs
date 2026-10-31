//! [`CliHost`]: the headless [`Session`] with the Aligners module, plus a
//! CPU render (dc-raster) of the case.

use std::collections::BTreeMap;

use dc_case::{BlobRef, CaseState, ScanRole};
use dc_mcp::{Host, HostError, HostResult};
use dc_raster::RenderRequest;
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
    /// [`RenderRequest`]).
    pub fn render_png(&self, params: &Value) -> HostResult<Vec<u8>> {
        let empty = Map::new();
        let call = match params {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => return Err(HostError::new("render parameters must be an object")),
        };
        let req = RenderRequest::from_layers(&[self.session.view(), call]).map_err(HostError)?;
        let state = self.session.state();
        req.render_case_png(state, &aligner_labels(state))
            .map_err(HostError)
    }
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
