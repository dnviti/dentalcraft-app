//! Native file dialogs (rfd, XDG desktop portal on Linux) run on a worker
//! thread so the UI keeps painting; the answer is polled each frame.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// What a dialog is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// Open a `.dcprj`.
    OpenProject,
    /// Pick scans for a new project.
    PickScans,
    /// Choose where to save a new project.
    NewProjectLocation,
    /// Choose where to write the diagnostics zip.
    ExportDiagnostics,
    /// Attach files to a support ticket.
    TicketAttachments,
}

/// A dialog waiting for the user.
pub struct PendingDialog {
    /// What the answer is for.
    pub purpose: Purpose,
    rx: Receiver<Vec<PathBuf>>,
}

impl PendingDialog {
    /// The chosen paths once the dialog closed (empty when cancelled).
    pub fn poll(&self) -> Option<Vec<PathBuf>> {
        match self.rx.try_recv() {
            Ok(paths) => Some(paths),
            Err(TryRecvError::Disconnected) => Some(Vec::new()),
            Err(TryRecvError::Empty) => None,
        }
    }
}

/// File dialog description.
#[derive(Debug, Clone, Default)]
pub struct Spec {
    /// Window title.
    pub title: String,
    /// Filter name and extensions.
    pub filter: Option<(String, Vec<&'static str>)>,
    /// Start folder.
    pub directory: Option<PathBuf>,
    /// Suggested file name (save dialogs).
    pub file_name: Option<String>,
    /// Save dialog instead of open.
    pub save: bool,
    /// Several files.
    pub multiple: bool,
}

fn run(spec: Spec) -> Vec<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(&spec.title);
    if let Some((name, exts)) = &spec.filter {
        d = d.add_filter(name, exts);
    }
    if let Some(dir) = &spec.directory {
        d = d.set_directory(dir);
    }
    if let Some(f) = &spec.file_name {
        d = d.set_file_name(f);
    }
    if spec.save {
        d.save_file().into_iter().collect()
    } else if spec.multiple {
        d.pick_files().unwrap_or_default()
    } else {
        d.pick_file().into_iter().collect()
    }
}

/// Opens a dialog. On Linux and Windows it runs on a worker thread; macOS
/// needs the main thread, so there it blocks for the dialog's lifetime.
pub fn open(purpose: Purpose, spec: Spec, wake: impl Fn() + Send + 'static) -> PendingDialog {
    let (tx, rx) = channel();
    if cfg!(target_os = "macos") {
        let _ = tx.send(run(spec));
    } else {
        let spawned = std::thread::Builder::new()
            .name("dc-file-dialog".into())
            .spawn(move || {
                let _ = tx.send(run(spec));
                wake();
            });
        if let Err(e) = spawned {
            tracing::error!("cannot open the file dialog: {e}");
        }
    }
    PendingDialog { purpose, rx }
}
