//! Background job of "Create project": reads the scans (STL / PLY / OBJ),
//! converts them to millimetres, builds the `.dcprj` project and saves it.
//! Progress and the result come back over a channel polled by the UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use dc_case::{Case, CaseState, Jaws, Pose, Scan, ScanRole};
use dc_project::{AppInfo, Project};

/// What the user filled in on the New project screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectRequest {
    /// Module id (`aligners`).
    pub module_id: String,
    /// Lab case reference (also the file name), stored as the case's ID.
    pub case_id: String,
    /// Pseudonymised patient alias (stored as the case's patient code).
    pub alias: String,
    /// Jaws being treated.
    pub jaws: Jaws,
    /// Notes for the technician.
    pub notes: String,
    /// Scan files with their role.
    pub scans: Vec<(PathBuf, ScanRole)>,
    /// Where to save the project.
    pub path: PathBuf,
}

/// Messages from the job.
#[derive(Debug)]
pub enum JobMsg {
    /// Reading file `index` (0-based) of `total`, named `name`.
    Reading {
        /// Index.
        index: usize,
        /// Count.
        total: usize,
        /// File name.
        name: String,
    },
    /// Writing the project file.
    Saving,
    /// Finished: the saved project, or an error message.
    Done(Result<Box<Project>, String>),
}

/// A running creation job.
pub struct CreateJob {
    rx: Receiver<JobMsg>,
    cancel: Arc<AtomicBool>,
    /// Last progress message.
    pub last: Option<JobMsg>,
    /// The request.
    pub request: NewProjectRequest,
}

impl CreateJob {
    /// Starts the job on a worker thread; `wake` is called after each message
    /// (request a repaint there).
    pub fn spawn(request: NewProjectRequest, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (req, flag) = (request.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("dc-create-project".into())
            .spawn(move || {
                let send = |m: JobMsg| {
                    let _ = tx.send(m);
                    wake();
                };
                let result = run(&req, &flag, &send);
                send(JobMsg::Done(result.map(Box::new)));
            });
        let job = Self {
            rx,
            cancel,
            last: None,
            request,
        };
        if let Err(e) = spawned {
            tracing::error!("cannot start the import thread: {e}");
        }
        job
    }

    /// Asks the job to stop at the next file.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Drains the channel; returns the final result once.
    pub fn poll(&mut self) -> Option<Result<Box<Project>, String>> {
        loop {
            match self.rx.try_recv() {
                Ok(JobMsg::Done(r)) => return Some(r),
                Ok(m) => self.last = Some(m),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(Err("the import stopped unexpectedly".into()));
                }
            }
        }
    }

    /// Progress fraction for the bar.
    pub fn fraction(&self) -> Option<f32> {
        match &self.last {
            Some(JobMsg::Reading { index, total, .. }) => {
                Some(*index as f32 / (*total).max(1) as f32 * 0.9)
            }
            Some(JobMsg::Saving) => Some(0.95),
            _ => None,
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

/// The job body (blocking).
pub fn run(
    req: &NewProjectRequest,
    cancel: &AtomicBool,
    send: &dyn Fn(JobMsg),
) -> Result<Project, String> {
    let mut case = Case::new(req.alias.trim());
    case.case_id = req.case_id.trim().to_owned();
    case.jaws = req.jaws;
    case.notes = req.notes.trim().to_owned();
    let mut state = CaseState::new(case);
    let total = req.scans.len();
    for (index, (path, role)) in req.scans.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        send(JobMsg::Reading {
            index,
            total,
            name: name.clone(),
        });
        let mut mesh = dc_mesh::io::read(path).map_err(|e| format!("{name}: {e}"))?;
        let units = dc_mesh::io::normalize_units(&mut mesh).map_err(|e| format!("{name}: {e}"))?;
        tracing::info!(
            "imported {name}: {} triangles, source units {units:?}",
            mesh.face_count()
        );
        let blob = state.blobs.insert(mesh);
        state.case.scans.retain(|s| s.role != *role);
        state.case.scans.push(Scan {
            id: uuid::Uuid::new_v4(),
            role: *role,
            source_name: name,
            source_units: units,
            import_xform: Pose::default(),
            mesh: blob,
        });
    }
    if req.module_id == "aligners" {
        state.set_module("aligners", dca_module::AlignersModule::initial_state());
    }
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    let project = Project::new(app_info(), dc_workspace::now_rfc3339(), state);
    send(JobMsg::Saving);
    if let Some(dir) = req.path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    dc_project::save(&req.path, &project).map_err(|e| e.to_string())?;
    Ok(project)
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

    const TRI_STL: &str = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid t\n";

    #[test]
    fn creates_and_saves_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let scan = dir.path().join("upper.stl");
        std::fs::write(&scan, TRI_STL).unwrap();
        let req = NewProjectRequest {
            module_id: "aligners".into(),
            case_id: "AL-1".into(),
            alias: "BIA-07".into(),
            jaws: Jaws::Upper,
            notes: "check the 12".into(),
            scans: vec![(scan, ScanRole::Upper)],
            path: dir.path().join("p").join("AL-1_BIA-07.dcprj"),
        };
        let msgs = std::cell::RefCell::new(Vec::new());
        let p = run(&req, &AtomicBool::new(false), &|m| {
            msgs.borrow_mut().push(format!("{m:?}"))
        })
        .unwrap();
        assert_eq!(p.case_state.case.patient_code, "BIA-07");
        assert_eq!(p.case_state.case.case_id, "AL-1");
        assert_eq!(p.case_state.case.jaws, Jaws::Upper);
        assert_eq!(p.case_state.case.notes, "check the 12");
        assert_eq!(p.case_state.case.scans.len(), 1);
        assert!(p.case_state.module("aligners").is_some());
        assert!(req.path.exists());
        assert_eq!(msgs.borrow().len(), 2);
        assert!(run(&req, &AtomicBool::new(true), &|_| {}).is_err());
    }

    #[test]
    fn stems() {
        assert_eq!(file_stem("AL-2026-0142", "BIA-07"), "AL-2026-0142_BIA-07");
        assert_eq!(file_stem(" a/b ", ""), "a-b");
        assert_eq!(file_stem("", ""), "project");
    }
}
