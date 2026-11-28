//! Background job of "Create project": imports the scans with the shared
//! pipeline (`import_scans`: read, units, axes, arch recognition, roles),
//! builds the `.dcprj` project and saves it. Progress and the result come
//! back over a channel polled by the UI.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use dc_case::import::{ImportFile, ImportReport, ImportRequest, import_files};
use dc_case::{Case, CaseState, Jaws, ScanRole};
use dc_command::JobCtx;
use dc_project::{AppInfo, Project};
use serde_json::Value;

/// What the user filled in on the New project screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectRequest {
    /// Module id (`aligners`).
    pub module_id: String,
    /// Lab case reference (also the file name), stored as the case's ID.
    pub case_id: String,
    /// Pseudonymised patient alias (stored as the case's patient code).
    pub alias: String,
    /// Jaws being treated (stored in the case; it does not pick any scan's
    /// role).
    pub jaws: Jaws,
    /// Notes for the technician.
    pub notes: String,
    /// Scan files with the role the user chose; `None` is auto (the file name,
    /// then the shape of the scan, decide).
    pub scans: Vec<(PathBuf, Option<ScanRole>)>,
    /// Where to save the project.
    pub path: PathBuf,
}

/// A saved project and the report of its import.
#[derive(Debug)]
pub struct Created {
    /// The saved project.
    pub project: Project,
    /// The serialised import report (what failed, warnings); `None` when no
    /// scan was picked, so there was nothing to import.
    pub report: Option<Value>,
}

/// Why no project was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    /// The user cancelled.
    Cancelled,
    /// No scan could be imported; one reason per file, in the current
    /// language.
    NothingImported(Vec<String>),
    /// Any other failure (saving, folder).
    Failed(String),
}

/// Messages from the job.
#[derive(Debug)]
pub enum JobMsg {
    /// Writing the project file.
    Saving,
    /// Finished: the saved project, or why there is none.
    Done(Result<Box<Created>, CreateError>),
}

/// Where a running job is, for the progress dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Not started reporting yet.
    Starting,
    /// Reading scan `index` (0-based) of `total`.
    Reading {
        /// Index.
        index: usize,
        /// Count.
        total: usize,
    },
    /// Writing the project file.
    Saving,
}

/// A running creation job.
pub struct CreateJob {
    rx: Receiver<JobMsg>,
    ctx: JobCtx,
    saving: bool,
    /// The request.
    pub request: NewProjectRequest,
}

impl CreateJob {
    /// Starts the job on a worker thread; `wake` is called after each message
    /// (request a repaint there).
    pub fn spawn(request: NewProjectRequest, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let ctx = JobCtx::detached();
        let (req, job_ctx) = (request.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("dc-create-project".into())
            .spawn(move || {
                let send = |m: JobMsg| {
                    let _ = tx.send(m);
                    wake();
                };
                let result = run(&req, &job_ctx, &send);
                send(JobMsg::Done(result.map(Box::new)));
            });
        let job = Self {
            rx,
            ctx,
            saving: false,
            request,
        };
        if let Err(e) = spawned {
            tracing::error!("cannot start the import thread: {e}");
        }
        job
    }

    /// Asks the job to stop: before the files are read, between them, and
    /// before the project is saved.
    pub fn cancel(&self) {
        self.ctx.cancel_token().cancel();
    }

    /// Drains the channel; returns the final result once.
    pub fn poll(&mut self) -> Option<Result<Box<Created>, CreateError>> {
        loop {
            match self.rx.try_recv() {
                Ok(JobMsg::Done(r)) => return Some(r),
                Ok(JobMsg::Saving) => self.saving = true,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(Err(CreateError::Failed(
                        "the import stopped unexpectedly".into(),
                    )));
                }
            }
        }
    }

    /// Where the job is.
    pub fn stage(&self) -> Stage {
        let total = self.request.scans.len();
        let progress = self.ctx.progress();
        let fraction = progress.fraction;
        if self.saving {
            Stage::Saving
        } else if (fraction > 0.0 || progress.message_key.is_some()) && total > 0 {
            let index = ((fraction * total as f32).round() as usize).min(total - 1);
            Stage::Reading { index, total }
        } else {
            Stage::Starting
        }
    }

    /// Progress fraction for the bar.
    pub fn fraction(&self) -> Option<f32> {
        match self.stage() {
            Stage::Starting => None,
            Stage::Reading { .. } => Some(self.ctx.progress().fraction * 0.9),
            Stage::Saving => Some(0.95),
        }
    }
}

/// Writer information of new projects.
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        commit: option_env!("DC_GIT_COMMIT").unwrap_or("dev").to_owned(),
    }
}

/// Imports the scans of the form into `state`, all in one call. The aligners
/// module recognises the arch of the files that have neither a role of the
/// user nor a name hint; another module gets the plain import.
fn import(state: &mut CaseState, req: &NewProjectRequest, ctx: &JobCtx) -> ImportReport {
    let request = ImportRequest {
        files: req
            .scans
            .iter()
            .map(|(path, role)| ImportFile {
                path: path.clone(),
                role: *role,
            })
            .collect(),
    };
    if req.module_id == "aligners" {
        dca_module::commands::import::import_scans(state, &request, ctx).report
    } else {
        import_files(state, &request, None, ctx)
    }
}

/// The job body (blocking).
pub fn run(
    req: &NewProjectRequest,
    ctx: &JobCtx,
    send: &dyn Fn(JobMsg),
) -> Result<Created, CreateError> {
    let mut case = Case::new(req.alias.trim());
    case.case_id = req.case_id.trim().to_owned();
    case.jaws = req.jaws;
    case.notes = req.notes.trim().to_owned();
    let mut state = CaseState::new(case);
    if ctx.is_cancelled() {
        return Err(CreateError::Cancelled);
    }
    // no scans picked: the project starts empty and the workspace offers the
    // import and the scan; picked files that all fail are a real error
    let report = if req.scans.is_empty() {
        None
    } else {
        let report = import(&mut state, req, ctx);
        if report.cancelled {
            return Err(CreateError::Cancelled);
        }
        for f in &report.failed {
            tracing::warn!(
                "not imported: {} ({}): {:?}",
                f.path.display(),
                f.code,
                f.detail
            );
        }
        if report.imported.is_empty() {
            let reasons = report.failed.iter().map(|f| f.reason.clone()).collect();
            return Err(CreateError::NothingImported(reasons));
        }
        for s in &report.imported {
            tracing::info!(
                "imported {}: {} faces, role {:?} ({:?}), source units {:?}",
                s.source_name,
                s.faces,
                s.role,
                s.role_source,
                s.source_units
            );
        }
        Some(report)
    };
    // the import of the module may have written its data already (the bases
    // it closed the scans with): a new case starts from it
    if req.module_id == "aligners" && state.module("aligners").is_none() {
        state.set_module("aligners", dca_module::AlignersModule::initial_state());
    }
    if ctx.is_cancelled() {
        return Err(CreateError::Cancelled);
    }
    let project = Project::new(app_info(), dc_workspace::now_rfc3339(), state);
    send(JobMsg::Saving);
    if let Some(dir) = req.path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CreateError::Failed(format!("{}: {e}", dir.display())))?;
    }
    dc_project::save(&req.path, &project)
        .map_err(|e| CreateError::Failed(dc_workspace::error_text(&e)))?;
    let report = report.map(|r| serde_json::to_value(&r).unwrap_or(Value::Null));
    Ok(Created { project, report })
}

/// The import report of a new project, for the workspace's notifications.
///
/// The scans of a freshly created project are its base state, not an undoable
/// change, so the parts of the report that tell the user to press Ctrl+Z (a
/// replaced scan, the results the replacement cleared) are dropped. The
/// failed files and the warnings stay.
pub fn base_state_report(mut report: Value) -> Value {
    if let Some(o) = report.as_object_mut() {
        for key in ["replaced", "replaced_roles", "module_cleared"] {
            o.remove(key);
        }
    }
    report
}

/// Safe file stem from the case id and the alias (`AL-2026-0142_BIA-07`).
pub fn file_stem(case_id: &str, alias: &str) -> String {
    let clean = |s: &str| -> String {
        s.trim()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    match (clean(case_id), clean(alias)) {
        (a, b) if a.is_empty() && b.is_empty() => "project".into(),
        (a, b) if b.is_empty() => a,
        (a, b) if a.is_empty() => b,
        (a, b) => format!("{a}_{b}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::RoleSource;
    use std::path::Path;

    const TRI_STL: &str = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid t\n";

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    fn request(dir: &Path, scans: Vec<(PathBuf, Option<ScanRole>)>) -> NewProjectRequest {
        NewProjectRequest {
            module_id: "aligners".into(),
            case_id: "AL-1".into(),
            alias: "BIA-07".into(),
            jaws: Jaws::Upper,
            notes: "check the 12".into(),
            scans,
            path: dir.join("p").join("AL-1_BIA-07.dcprj"),
        }
    }

    fn run_quiet(req: &NewProjectRequest) -> Result<Created, CreateError> {
        run(req, &JobCtx::detached(), &|_| {})
    }

    #[test]
    fn creates_and_saves_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let scan = write(dir.path(), "upper.stl", TRI_STL);
        let req = request(dir.path(), vec![(scan, Some(ScanRole::Upper))]);
        let msgs = std::cell::RefCell::new(Vec::new());
        let ctx = JobCtx::detached();
        let created = run(&req, &ctx, &|m| msgs.borrow_mut().push(format!("{m:?}"))).unwrap();
        let p = created.project;
        assert_eq!(p.case_state.case.patient_code, "BIA-07");
        assert_eq!(p.case_state.case.case_id, "AL-1");
        assert_eq!(p.case_state.case.jaws, Jaws::Upper);
        assert_eq!(p.case_state.case.notes, "check the 12");
        assert_eq!(p.case_state.case.scans.len(), 1);
        assert_eq!(p.case_state.case.scans[0].role, ScanRole::Upper);
        assert_eq!(p.case_state.case.scans[0].role_source, RoleSource::Explicit);
        assert!(p.case_state.module("aligners").is_some());
        assert!(req.path.exists());
        assert_eq!(msgs.borrow().len(), 1, "only the saving message");
        let report = created.report.as_ref().unwrap();
        assert_eq!(report["imported"].as_array().unwrap().len(), 1);
        assert!(report["failed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_project_without_scans_is_created_saved_and_reopens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path(), Vec::new());
        let created = run_quiet(&req).unwrap();
        assert!(created.report.is_none(), "nothing was imported");
        let p = &created.project;
        assert_eq!(p.case_state.case.patient_code, "BIA-07");
        assert_eq!(p.case_state.case.case_id, "AL-1");
        assert_eq!(p.case_state.case.notes, "check the 12");
        assert!(p.case_state.case.scans.is_empty());
        assert!(p.case_state.module("aligners").is_some());
        assert!(req.path.exists());

        // the saved file loads again, still without scans
        let module = dca_module::AlignersModule::new();
        let loaded = dc_project::load(&req.path, &dc_workspace::project_codecs(&module)).unwrap();
        let case = &loaded.project.case_state.case;
        assert!(case.scans.is_empty());
        assert_eq!(case.patient_code, "BIA-07");
        assert!(loaded.project.case_state.module("aligners").is_some());

        // and the workspace opens on it
        let ws =
            dc_workspace::Workspace::new_headless(Box::new(module), Some(loaded.project), None);
        assert!(ws.state().case.scans.is_empty());
    }

    #[test]
    fn a_cancelled_job_reads_nothing_and_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let scan = write(dir.path(), "a.stl", TRI_STL);
        let req = request(dir.path(), vec![(scan, None)]);
        // before the files are read
        let ctx = JobCtx::detached();
        ctx.cancel_token().cancel();
        assert_eq!(
            run(&req, &ctx, &|_| {}).unwrap_err(),
            CreateError::Cancelled
        );
        assert!(!req.path.exists());
    }

    #[test]
    fn explicit_roles_reach_the_import_and_beat_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "upper.stl", TRI_STL);
        let b = write(dir.path(), "x.stl", TRI_STL);
        // the name says upper, the user said lower; the other file is bite
        let req = request(
            dir.path(),
            vec![(a, Some(ScanRole::Lower)), (b, Some(ScanRole::Bite))],
        );
        let p = run_quiet(&req).unwrap().project;
        let scans = &p.case_state.case.scans;
        let lower = scans.iter().find(|s| s.role == ScanRole::Lower).unwrap();
        assert_eq!(lower.source_name, "upper.stl");
        assert_eq!(lower.role_source, RoleSource::Explicit);
        assert!(scans.iter().any(|s| s.role == ScanRole::Bite));
    }

    #[test]
    fn auto_rows_use_the_file_name_and_never_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let named = write(dir.path(), "caso_lower.stl", TRI_STL);
        let in_folder = write(dir.path(), "upper/scan.stl", TRI_STL);
        let req = request(dir.path(), vec![(named, None), (in_folder, None)]);
        let p = run_quiet(&req).unwrap().project;
        let scans = &p.case_state.case.scans;
        let named = scans
            .iter()
            .find(|s| s.source_name == "caso_lower.stl")
            .unwrap();
        assert_eq!(
            (named.role, named.role_source),
            (ScanRole::Lower, RoleSource::FileName)
        );
        // the folder's name is not a hint: the file takes the free arch, provisional
        let other = scans.iter().find(|s| s.source_name == "scan.stl").unwrap();
        assert_eq!(
            (other.role, other.role_source),
            (ScanRole::Upper, RoleSource::Provisional)
        );
    }

    #[test]
    fn a_bad_file_does_not_stop_the_good_one() {
        let dir = tempfile::tempdir().unwrap();
        let good = write(dir.path(), "good.stl", TRI_STL);
        let empty = write(dir.path(), "empty.stl", "");
        let req = request(dir.path(), vec![(empty, None), (good, None)]);
        let created = run_quiet(&req).unwrap();
        assert!(req.path.exists());
        assert_eq!(created.project.case_state.case.scans.len(), 1);
        let failed = created.report.as_ref().unwrap()["failed"]
            .as_array()
            .unwrap();
        assert_eq!(failed.len(), 1);
        assert!(failed[0]["path"].as_str().unwrap().ends_with("empty.stl"));
        assert!(!failed[0]["reason"].as_str().unwrap().is_empty());
    }

    #[test]
    fn all_files_bad_creates_nothing_and_gives_the_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write(dir.path(), "empty.stl", "");
        let junk = write(dir.path(), "junk.stl", "this is not a mesh");
        let req = request(dir.path(), vec![(empty, None), (junk, None)]);
        match run_quiet(&req) {
            Err(CreateError::NothingImported(reasons)) => {
                assert_eq!(reasons.len(), 2);
                assert!(reasons.iter().all(|r| !r.is_empty()));
            }
            other => panic!("expected NothingImported, got {other:?}"),
        }
        assert!(!req.path.exists(), "no project file");
        assert!(!req.path.parent().unwrap().exists(), "no folder either");
    }

    #[test]
    fn the_report_of_a_new_project_never_mentions_undo() {
        let report = serde_json::json!({
            "imported": [{"role": "upper"}],
            "failed": [{"reason": "x"}],
            "warnings": [{"code": "tiny", "message": "m"}],
            "replaced_roles": ["upper"],
            "module_cleared": ["staging"],
            "replaced": true,
        });
        let out = base_state_report(report);
        for key in ["replaced", "replaced_roles", "module_cleared"] {
            assert!(out.get(key).is_none(), "{key}");
        }
        for key in ["imported", "failed", "warnings"] {
            assert!(out.get(key).is_some(), "{key}");
        }
    }

    /// The lower demo arch with a closed base inside the arch, as a model has:
    /// the plugin flags the cue as "to be checked" (the same mesh the aligners
    /// import tests use). A synthetic model, never a patient file.
    fn lower_on_a_model_base() -> dc_mesh::TriMesh {
        use dca_algo::{anatomy::Arch, demo};
        use glam::Vec3;
        let params = demo::Params {
            arch: Arch::Lower,
            seed: 1,
            ..demo::Params::default()
        };
        let mut mesh = demo::run(&params).mesh;
        mesh.vertex_labels = None;
        let bb = mesh.aabb().unwrap();
        let (lo, hi) = (bb.min, bb.max);
        let n = 40u32;
        let first = mesh.positions.len() as u32;
        let z = lo.z + 0.2 * (hi.z - lo.z);
        for i in 0..n {
            for j in 0..n {
                let x = lo.x + (hi.x - lo.x) * (0.15 + 0.7 * i as f32 / (n - 1) as f32);
                let y = lo.y + (hi.y - lo.y) * (0.05 + 0.9 * j as f32 / (n - 1) as f32);
                mesh.positions.push(Vec3::new(x, y, z));
            }
        }
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                let a = first + i * n + j;
                mesh.tris.push([a, a + 1, a + n]);
                mesh.tris.push([a + 1, a + n + 1, a + n]);
            }
        }
        mesh
    }

    #[test]
    fn a_scan_recognised_from_a_weak_cue_reaches_the_workspace_to_check() {
        use dc_workspace::Workspace;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan_a.stl");
        dc_mesh::io::write(&path, &lower_on_a_model_base(), dc_mesh::MeshFormat::Stl).unwrap();
        // no role from the user and none in the name: the module recognises it
        let req = request(dir.path(), vec![(path, None)]);
        let created = run_quiet(&req).unwrap();
        let scan = &created.project.case_state.case.scans[0];
        assert_eq!(
            (scan.role, scan.role_source),
            (ScanRole::Lower, RoleSource::DetectedUnsure)
        );

        // the app shows the report of the base state: the arch warning stays
        let report = base_state_report(created.report.unwrap());
        let mut ws = Workspace::new_headless(
            Box::new(dca_module::AlignersModule::new()),
            Some(created.project),
            None,
        );
        ws.show_import_report(&report);
        ws.select_item("aligners.case.import-scans");
        assert_eq!(ws.toast_keys()[0], "ws-toast-warn-provisional");
        assert_eq!(ws.sticky_toast_keys(), ["ws-toast-warn-provisional"]);
        let note = ws.toast_details()[0].1.clone().unwrap();
        assert!(
            note.contains("scan_a.stl") && note.contains("weak"),
            "{note}"
        );

        // the scans panel marks it, with Confirm
        let mut h = Harness::builder()
            .with_size(egui::vec2(1920.0, 1080.0))
            .with_context_setup(|c| dc_ui::theme::install(c, dc_ui::Mode::Dark))
            .build_ui_state(
                |ui, ws: &mut Workspace| {
                    let _ = ws.show(ui);
                },
                ws,
            );
        h.run();
        assert!(h.query_by_label("Detected, to check").is_some());
        // Confirm is one undo step
        h.get_by_label("Confirm").click();
        h.step();
        assert!(
            h.state_mut()
                .wait_for_jobs(std::time::Duration::from_secs(20))
        );
        h.run();
        let source = |h: &Harness<'_, Workspace>| h.state().state().case.scans[0].role_source;
        assert_eq!(source(&h), RoleSource::Explicit);
        assert!(h.query_by_label("Detected, to check").is_none());
        assert!(h.state_mut().undo());
        assert_eq!(source(&h), RoleSource::DetectedUnsure);
        assert!(!h.state_mut().undo(), "nothing else to undo");
    }

    #[test]
    fn stems() {
        assert_eq!(file_stem("AL-2026-0142", "BIA-07"), "AL-2026-0142_BIA-07");
        assert_eq!(file_stem(" a/b ", ""), "a-b");
        assert_eq!(file_stem("", ""), "project");
    }
}
