//! Workspace state and logic (everything except drawing).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dc_case::{Case, CaseState, RoleSource, ScanRole};
use dc_command::{Command, CommandError, StepHistory, UndoStack};
use dc_jobs::{JobError, JobHandle, JobSystem, Progress};
use dc_module_api::{
    CommandDesc, CommandOutput, CommandSpec, MenuStatus, MenuTree, Module, ModuleCtx,
    NOT_AVAILABLE_KEY, Toast, ToastLevel, is_import_scans_command,
};
use dc_project::{Manifest, Project};
use dc_session::builtins::{CASE_IMPORT, CASE_SET_ROLE};
use egui::KeyboardShortcut;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::import;
use crate::menu::MenuView;
use crate::timeline::TimelineState;
use crate::view::ViewState;
use crate::{keys, project};

/// What the shell must do after a frame of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Nothing.
    None,
    /// Go back to Home (the user confirmed or there was nothing to save).
    GoHome,
    /// The project was saved to this path.
    Saved(PathBuf),
    /// The project was closed.
    Closed,
}

/// Where the user wants to go once the unsaved changes are dealt with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    Home,
    Close,
}

impl Exit {
    fn action(self) -> WorkspaceAction {
        match self {
            Exit::Home => WorkspaceAction::GoHome,
            Exit::Close => WorkspaceAction::Closed,
        }
    }
}

/// A notification on screen.
#[derive(Debug, Clone)]
pub(crate) struct ShownToast {
    pub toast: Toast,
    /// Time (egui seconds) it was first drawn.
    pub born: Option<f64>,
}

/// A registered command whose JSON output is shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecKind {
    /// The scan import: its output is the import report.
    Import,
    /// `case.set-role`: its output tells what the module cleared.
    SetRole,
}

impl SpecKind {
    /// Title of the notification of a failure.
    fn error_key(self) -> &'static str {
        match self {
            SpecKind::Import => "ws-toast-import-failed",
            SpecKind::SetRole => "ws-toast-command-failed",
        }
    }
}

pub(crate) enum Task {
    /// A registered command run as a job on a snapshot of the state, keeping
    /// its JSON output (`SpecCommand` drops it): the result is applied as one
    /// undo step and the output is reported.
    Spec {
        kind: SpecKind,
        label: String,
        handle: JobHandle<Result<CommandOutput, CommandError>>,
    },
    Command {
        label: String,
        handle: JobHandle<Result<CaseState, CommandError>>,
    },
    Save(Box<SaveTask>),
}

pub(crate) struct SaveTask {
    path: PathBuf,
    state: Arc<CaseState>,
    manifest: Manifest,
    then: Option<Exit>,
    handle: JobHandle<Result<(), String>>,
}

/// A background job shown in the status bar.
pub(crate) struct Running {
    pub name_key: &'static str,
    pub task: Task,
}

impl Running {
    pub fn progress(&self) -> Progress {
        match &self.task {
            Task::Spec { handle, .. } => handle.progress(),
            Task::Command { handle, .. } => handle.progress(),
            Task::Save(s) => s.handle.progress(),
        }
    }

    pub fn cancel(&self) {
        match &self.task {
            Task::Spec { handle, .. } => handle.cancel(),
            Task::Command { handle, .. } => handle.cancel(),
            // an atomic save is not interrupted
            Task::Save(_) => {}
        }
    }

    pub fn cancellable(&self) -> bool {
        !matches!(self.task, Task::Save(_))
    }

    fn is_finished(&self) -> bool {
        match &self.task {
            Task::Spec { handle, .. } => handle.is_finished(),
            Task::Command { handle, .. } => handle.is_finished(),
            Task::Save(s) => s.handle.is_finished(),
        }
    }
}

/// Answer of a native file dialog (run on its own thread).
pub(crate) enum DialogAnswer {
    Import(Vec<PathBuf>),
    SaveAs(Option<PathBuf>, Option<Exit>),
}

/// The workspace screen of an open project: menu tree, viewport, inspector,
/// stage timeline and status bar around the project's case state.
pub struct Workspace {
    pub(crate) module: Box<dyn Module>,
    pub(crate) menu: MenuTree,
    pub(crate) commands: Vec<CommandDesc>,
    pub(crate) shortcuts: Vec<(&'static str, KeyboardShortcut)>,
    pub(crate) shortcut_text: HashMap<&'static str, String>,
    pub(crate) undo: UndoStack,
    saved: Arc<CaseState>,
    pub(crate) history: StepHistory,
    history_json: Value,
    manifest: Option<Manifest>,
    thumbnail: Option<Vec<u8>>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) read_only: bool,
    jobs: JobSystem,
    pub(crate) running: Vec<Running>,
    dialog: Option<mpsc::Receiver<DialogAnswer>>,
    native_dialogs: bool,
    /// Dialogs requested while native dialogs are off (headless): for tests.
    pub(crate) dialog_requests: usize,
    pub(crate) egui_ctx: Option<egui::Context>,
    pub(crate) view: ViewState,
    pub(crate) manual: crate::manual::Manual,
    pub(crate) menu_view: MenuView,
    pub(crate) selected: Option<&'static str>,
    /// Role the user chose for the next import (`None` = auto): it applies
    /// to the next file dialog or drop and goes back to auto.
    pub(crate) import_role: Option<ScanRole>,
    import_spec: Option<CommandSpec>,
    set_role_spec: Option<CommandSpec>,
    pub(crate) timeline: TimelineState,
    pub(crate) toasts: Vec<ShownToast>,
    pub(crate) confirm_exit: Option<Exit>,
    pub(crate) show_help: bool,
    actions: VecDeque<WorkspaceAction>,
}

impl Workspace {
    /// Workspace for `module` on `project` (a new empty case when `None`)
    /// stored at `path` (`None` = not saved yet), with the 3D viewport on
    /// the GPU of `render_state`.
    pub fn new(
        module: Box<dyn Module>,
        project: Option<Project>,
        path: Option<PathBuf>,
        render_state: &egui_wgpu::RenderState,
    ) -> Self {
        let mut ws = Self::build(module, project, path);
        ws.view = ViewState::with_viewport(render_state);
        ws.native_dialogs = true;
        ws
    }

    /// Workspace without GPU viewport and without native file dialogs (the
    /// viewport area shows a placeholder): for tests and headless use.
    pub fn new_headless(
        module: Box<dyn Module>,
        project: Option<Project>,
        path: Option<PathBuf>,
    ) -> Self {
        Self::build(module, project, path)
    }

    fn build(module: Box<dyn Module>, project: Option<Project>, path: Option<PathBuf>) -> Self {
        crate::loader();
        let (state, history_json, manifest, thumbnail) = match project {
            Some(p) => (p.case_state, p.history, Some(p.manifest), p.thumbnail),
            None => (
                CaseState::new(Case::new("")),
                Value::Array(Vec::new()),
                None,
                None,
            ),
        };
        let history =
            serde_json::from_value::<StepHistory>(history_json.clone()).unwrap_or_default();
        let state = Arc::new(state);
        let menu = module.menu();
        let commands = module.commands();
        let mut shortcuts = Vec::new();
        let mut shortcut_text = HashMap::new();
        for c in &commands {
            if let Some(text) = c.shortcut
                && let Some(sc) = keys::parse(text)
            {
                shortcuts.push((c.id, sc));
                shortcut_text.insert(c.id, text.to_owned());
            }
        }
        // the module's own import when it has one, the core's otherwise
        let builtins = dc_session::builtins::specs();
        let builtin = |id: &str| builtins.iter().find(|s| s.id == id).copied();
        let import_spec = module
            .command_specs()
            .into_iter()
            .find(|s| is_import_scans_command(s.id))
            .or_else(|| builtin(CASE_IMPORT));
        let set_role_spec = builtin(CASE_SET_ROLE);
        let jobs = JobSystem::new(0)
            .or_else(|_| JobSystem::new(1))
            .expect("cannot start the background job threads");
        Self {
            menu_view: MenuView::new(&menu),
            module,
            menu,
            commands,
            shortcuts,
            shortcut_text,
            undo: UndoStack::new(state.clone(), UndoStack::DEFAULT_BUDGET),
            saved: state,
            history,
            history_json,
            manifest,
            thumbnail,
            path,
            read_only: false,
            jobs,
            running: Vec::new(),
            dialog: None,
            native_dialogs: false,
            dialog_requests: 0,
            egui_ctx: None,
            view: ViewState::default(),
            manual: crate::manual::Manual::default(),
            selected: None,
            import_role: None,
            import_spec,
            set_role_spec,
            timeline: TimelineState::default(),
            toasts: Vec::new(),
            confirm_exit: None,
            show_help: false,
            actions: VecDeque::new(),
        }
    }

    // ------------------------------------------------------------ accessors

    /// The module shown.
    pub fn module(&self) -> &dyn Module {
        self.module.as_ref()
    }

    /// The current case state.
    pub fn state(&self) -> &Arc<CaseState> {
        self.undo.current()
    }

    /// Path of the project file, if saved.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// `true` when the case differs from the saved (or opened) version.
    pub fn is_dirty(&self) -> bool {
        !Arc::ptr_eq(self.undo.current(), &self.saved)
    }

    /// Marks the project as read-only (opened from a damaged or newer
    /// file): saving then always asks for a new path.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// `true` while an import or a command runs in the background.
    pub fn is_busy(&self) -> bool {
        self.command_running()
    }

    /// Role the next import gives its files (`None` = auto: the import
    /// decides).
    pub fn import_role(&self) -> Option<ScanRole> {
        self.import_role
    }

    /// Chooses the role of the files of the next import (`None` = auto). It
    /// applies to the next file dialog or drop, then goes back to auto.
    pub fn set_import_role(&mut self, role: Option<ScanRole>) {
        self.import_role = role;
    }

    /// The selected menu entry.
    pub fn selected(&self) -> Option<&'static str> {
        self.selected
    }

    /// i18n keys of the notifications currently listed (oldest first).
    pub fn toast_keys(&self) -> Vec<String> {
        self.toasts
            .iter()
            .map(|t| t.toast.message_key.clone())
            .collect()
    }

    /// Untranslated detail of the notifications currently listed (oldest
    /// first), next to their i18n keys: the sentence an import warning or a
    /// failed file carries.
    pub fn toast_details(&self) -> Vec<(String, Option<String>)> {
        self.toasts
            .iter()
            .map(|t| (t.toast.message_key.clone(), t.toast.detail.clone()))
            .collect()
    }

    /// The menu filter text.
    pub fn filter(&self) -> &str {
        &self.menu_view.filter
    }

    /// Sets the menu filter text.
    pub fn set_filter(&mut self, text: impl Into<String>) {
        self.menu_view.filter = text.into();
    }

    /// Ids of the menu entries the current filter shows (all when blank).
    pub fn visible_menu_ids(&self) -> Vec<&'static str> {
        let ids = crate::menu::filter_ids(&self.menu, &self.menu_view.filter, |it| {
            crate::t(it.label_key)
        });
        self.menu
            .iter()
            .map(|(_, it)| it.id)
            .filter(|id| ids.as_ref().is_none_or(|s| s.contains(id)))
            .collect()
    }

    /// Expands every group of the menu.
    pub fn expand_all(&mut self) {
        self.menu_view.expand_all(&self.menu);
    }

    /// Native file dialogs requested while they are disabled (headless).
    pub fn dialog_requests(&self) -> usize {
        self.dialog_requests
    }

    // ------------------------------------------------------------ actions

    /// Undoes the last change (no-op while a command runs).
    pub fn undo(&mut self) -> bool {
        if self.command_running() {
            return false;
        }
        self.undo.undo().is_some()
    }

    /// Redoes the last undone change (no-op while a command runs).
    pub fn redo(&mut self) -> bool {
        if self.command_running() {
            return false;
        }
        self.undo.redo().is_some()
    }

    /// `true` while an import or a command changes the case in the
    /// background: undo, redo, commands and imports wait for it (a save only
    /// reads the state).
    fn command_running(&self) -> bool {
        self.running
            .iter()
            .any(|r| !matches!(r.task, Task::Save(_)))
    }

    /// Selects menu row `id` (a group, sub-menu or command) and tells the
    /// module.
    pub fn select_item(&mut self, id: &'static str) {
        self.selected = Some(id);
        self.module.select(id);
    }

    /// Runs menu command `command_id`: "Import scans" entries open the file
    /// dialog (handled by the workspace); everything else goes to the
    /// module, whose commands run in the background and whose toasts are
    /// shown.
    pub fn activate(&mut self, command_id: &str) {
        if let Some(id) = self
            .menu
            .iter()
            .map(|(_, i)| i)
            .find(|i| i.command_id() == Some(command_id))
            .map(|i| i.id)
        {
            self.select_item(id);
            self.menu_view.reveal(&self.menu, id);
        }
        // a manual tool opens its panel and works on the pointer; its command runs from there
        if let Some(tool) = crate::manual::Tool::from_command(command_id)
            && self
                .commands
                .iter()
                .any(|c| c.id == command_id && c.status == MenuStatus::Ready)
        {
            self.open_manual_tool(tool);
            return;
        }
        // the import needs its files: the workspace asks for them and runs the
        // shared import command, so the module is not invoked
        if is_import_scans_command(command_id) {
            if self.command_running() {
                self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            } else {
                self.request_import_dialog();
            }
            return;
        }
        let state = self.undo.current().clone();
        let mut ctx = ModuleCtx::new(&state, &self.jobs);
        let result = self.module.invoke(command_id, &mut ctx);
        let (cmds, toasts) = ctx.finish();
        for toast in toasts {
            self.toast(toast);
        }
        for cmd in cmds {
            self.run_command(cmd);
        }
        if let Err(e) = result {
            let stub = self
                .commands
                .iter()
                .any(|c| c.id == command_id && c.status == MenuStatus::Stub);
            if !stub {
                self.toast(
                    Toast::new(ToastLevel::Error, "ws-toast-command-failed")
                        .with_detail(e.to_string()),
                );
            }
        }
    }

    /// Imports `paths` (STL / OBJ / PLY) in the background with the shared
    /// import command (the module's `import-scans`, else `case.import`): it
    /// reads, cleans, allocates the roles and adds the scans on a snapshot of
    /// the state. The result lands as one undoable change and the report is
    /// shown ([`Workspace::show_import_report`]). The role chosen with
    /// [`Workspace::set_import_role`] applies to these files and goes back
    /// to auto. Refused (with a notification) while another import or
    /// command runs.
    pub fn import_files(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let Some(spec) = self.import_spec else {
            self.job_error("ws-toast-import-failed", "no import command".into());
            return;
        };
        let role = self.import_role.take();
        let params = import::params(&paths, role);
        self.run_spec(spec, params, SpecKind::Import, "ws-job-import");
    }

    /// Files dropped on the workspace: the STL, OBJ and PLY ones (any letter
    /// case) are imported like the ones of the file dialog, the others are
    /// ignored with a notification. Ignored while an import or command runs.
    pub fn import_dropped(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let (files, ignored) = import::split_scan_files(paths);
        if !ignored.is_empty() {
            let names: Vec<String> = ignored
                .iter()
                .map(|p| {
                    p.file_name()
                        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
                })
                .collect();
            self.toast(
                Toast::new(ToastLevel::Warning, "ws-toast-drop-ignored")
                    .with_detail(names.join(", ")),
            );
        }
        self.import_files(files);
    }

    /// Gives scan `scan_id`, now `from`, the role `to` (`case.set-role`: a
    /// scan already there takes `from`). One undoable change; the scan is
    /// marked as chosen by the user, and what the module cleared is reported.
    /// Refused while an import or command runs.
    pub fn set_scan_role(&mut self, scan_id: Uuid, from: ScanRole, to: ScanRole) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let Some(spec) = self.set_role_spec else {
            return;
        };
        let params = json!({
            "from": import::role_id(from),
            "to": import::role_id(to),
            "scan_id": scan_id.to_string(),
        });
        self.run_spec(spec, params, SpecKind::SetRole, "ws-job-command");
    }

    /// Runs `spec` with `params` on a snapshot of the current state in the
    /// background. The caller has checked that nothing else runs.
    fn run_spec(
        &mut self,
        spec: CommandSpec,
        params: Value,
        kind: SpecKind,
        name_key: &'static str,
    ) {
        let label = spec.label_key.to_owned();
        let state = self.undo.current().clone();
        let handle = self
            .jobs
            .spawn(name_key, move |ctx| spec.execute(&state, &params, ctx));
        self.running.push(Running {
            name_key,
            task: Task::Spec {
                kind,
                label,
                handle,
            },
        });
    }

    /// Shows what an import did, as notifications: one per failed file with
    /// its reason, a notice when a scan was replaced or a module cleared
    /// results (it lists only what was cleared), one per warning with its
    /// translated `message`, and a line with the number of scans added. More
    /// than a handful are grouped in one line.
    ///
    /// `report` is the JSON output of the import command (module
    /// `import-scans` or `case.import`): the arrays `imported` (the scans
    /// added), `failed` (`reason`), `warnings` (`code`, `key`, `args`,
    /// `message`), `module_cleared` (ids), and `replaced` (bool). Missing
    /// fields count as empty, so a serialized `ImportReport` works too.
    pub fn show_import_report(&mut self, report: &Value) {
        for toast in import::report_toasts(report, self.module.id()) {
            self.toast(toast);
        }
    }

    /// Notification of a role change: what the module cleared, if anything.
    fn show_role_report(&mut self, report: &Value) {
        if let Some(toast) = import::cleared_toast(self.module.id(), report) {
            self.toast(toast);
        }
    }

    /// Runs `cmd` on the current state in the background and pushes the
    /// result on the undo stack.
    pub fn run_command(&mut self, cmd: Box<dyn Command>) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let label = cmd.label_key().to_owned();
        let state = self.undo.current().clone();
        let handle = self
            .jobs
            .spawn(label.clone(), move |ctx| cmd.apply(&state, ctx));
        self.running.push(Running {
            name_key: "ws-job-command",
            task: Task::Command { label, handle },
        });
    }

    /// Saves to the current path, or asks for one (also when read-only).
    pub fn save(&mut self) {
        self.save_then(None);
    }

    /// Asks for a new path and saves there.
    pub fn save_as(&mut self) {
        self.request_save_dialog(None);
    }

    pub(crate) fn save_then(&mut self, then: Option<Exit>) {
        match self.path.clone() {
            Some(p) if !self.read_only => self.save_to_then(p, then),
            _ => self.request_save_dialog(then),
        }
    }

    /// Saves to `path` in the background (`.dcprj` added when missing).
    pub fn save_to(&mut self, path: PathBuf) {
        self.save_to_then(path, None);
    }

    fn save_to_then(&mut self, path: PathBuf, then: Option<Exit>) {
        if self.running.iter().any(|r| matches!(r.task, Task::Save(_))) {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let path = project::with_extension(path);
        let state = self.undo.current().clone();
        let proj = project::build(
            self.manifest.as_ref(),
            &state,
            &self.history_json,
            self.thumbnail.as_ref(),
        );
        let manifest = proj.manifest.clone();
        let p = path.clone();
        let handle = self.jobs.spawn("ws-job-save", move |ctx| {
            ctx.set_progress(0.1, Some("ws-job-save"));
            project::save(&p, &proj)
        });
        self.running.push(Running {
            name_key: "ws-job-save",
            task: Task::Save(Box::new(SaveTask {
                path,
                state,
                manifest,
                then,
                handle,
            })),
        });
    }

    /// Leaves towards `exit`, asking first when there are unsaved changes.
    pub(crate) fn request_exit(&mut self, exit: Exit) {
        if self.is_dirty() {
            self.confirm_exit = Some(exit);
        } else {
            self.actions.push_back(exit.action());
        }
    }

    /// Leaves without saving.
    pub(crate) fn discard_and_exit(&mut self, exit: Exit) {
        self.confirm_exit = None;
        self.actions.push_back(exit.action());
    }

    /// Waits (polling) until the background jobs end or `timeout` passes;
    /// returns `true` when none is left. Results are applied as usual.
    pub fn wait_for_jobs(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            self.poll();
            if self.running.is_empty() {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Takes the next pending action (after [`Workspace::wait_for_jobs`]
    /// outside the UI loop).
    pub fn take_action(&mut self) -> WorkspaceAction {
        self.actions.pop_front().unwrap_or(WorkspaceAction::None)
    }

    pub(crate) fn toast(&mut self, toast: Toast) {
        self.toasts.push(ShownToast { toast, born: None });
    }

    pub(crate) fn module_ctx_run(&mut self, f: impl FnOnce(&mut dyn Module, &mut ModuleCtx<'_>)) {
        let state = self.undo.current().clone();
        let mut ctx = ModuleCtx::new(&state, &self.jobs);
        f(self.module.as_mut(), &mut ctx);
        let (cmds, toasts) = ctx.finish();
        for t in toasts {
            self.toast(t);
        }
        for c in cmds {
            self.run_command(c);
        }
    }

    /// `true` when command `id` counts as done: its step has a successful
    /// record in the step history ("Import scans" = the case has scans and
    /// none is still a guess to check).
    pub(crate) fn is_done(&self, id: &str) -> bool {
        if is_import_scans_command(id) {
            let scans = &self.undo.current().case.scans;
            return !scans.is_empty()
                && scans
                    .iter()
                    .all(|s| s.role_source != RoleSource::Provisional);
        }
        self.history.record(id).is_some_and(|r| r.failed.is_none())
    }

    // ------------------------------------------------------------ dialogs

    fn request_import_dialog(&mut self) {
        if !self.native_dialogs {
            self.dialog_requests += 1;
            return;
        }
        let ctx = self.egui_ctx.clone();
        self.spawn_dialog(move || {
            let files = rfd::FileDialog::new()
                .set_title(crate::t("ws-import-scans"))
                .add_filter(crate::t("ws-filter-meshes"), &import::DIALOG_EXTENSIONS)
                .pick_files()
                .unwrap_or_default();
            if let Some(c) = ctx {
                c.request_repaint();
            }
            DialogAnswer::Import(files)
        });
    }

    fn request_save_dialog(&mut self, then: Option<Exit>) {
        if !self.native_dialogs {
            self.dialog_requests += 1;
            return;
        }
        let ctx = self.egui_ctx.clone();
        let name = format!("{}.{}", self.display_name(), dc_project::FILE_EXTENSION);
        self.spawn_dialog(move || {
            let path = rfd::FileDialog::new()
                .set_title(crate::t("save-as"))
                .add_filter("DentalCraft", &[dc_project::FILE_EXTENSION])
                .set_file_name(name)
                .save_file();
            if let Some(c) = ctx {
                c.request_repaint();
            }
            DialogAnswer::SaveAs(path, then)
        });
    }

    fn spawn_dialog(&mut self, f: impl FnOnce() -> DialogAnswer + Send + 'static) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.dialog = Some(rx);
    }

    /// Project name for the top bar: file stem, case code or "Untitled".
    pub(crate) fn display_name(&self) -> String {
        if let Some(stem) = self.path.as_ref().and_then(|p| p.file_stem()) {
            return stem.to_string_lossy().into_owned();
        }
        let code = &self.undo.current().case.patient_code;
        if code.is_empty() {
            crate::t("ws-untitled")
        } else {
            code.clone()
        }
    }

    // ------------------------------------------------------------ polling

    /// Applies finished jobs and dialog answers; returns the next action.
    pub(crate) fn poll(&mut self) {
        if let Some(rx) = &self.dialog {
            match rx.try_recv() {
                Ok(answer) => {
                    self.dialog = None;
                    match answer {
                        DialogAnswer::Import(files) => self.import_files(files),
                        DialogAnswer::SaveAs(Some(p), then) => self.save_to_then(p, then),
                        DialogAnswer::SaveAs(None, _) => {}
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.dialog = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let (done, still): (Vec<_>, Vec<_>) =
            self.running.drain(..).partition(Running::is_finished);
        self.running = still;
        for r in done {
            self.finish(r.task);
        }
    }

    fn finish(&mut self, task: Task) {
        match task {
            Task::Spec {
                kind,
                label,
                handle,
            } => match handle.try_take() {
                Some(Ok(Ok((state, out)))) => {
                    self.undo.push(Arc::new(state), label);
                    match kind {
                        SpecKind::Import => {
                            self.view.refit = true;
                            self.show_import_report(&out);
                        }
                        SpecKind::SetRole => self.show_role_report(&out),
                    }
                }
                Some(Ok(Err(CommandError::Cancelled))) => {
                    self.toast(Toast::new(ToastLevel::Info, "job-cancelled"));
                }
                Some(Ok(Err(e))) => self.job_error(kind.error_key(), error_text(e)),
                Some(Err(e)) => self.job_panic(&e),
                None => {}
            },
            Task::Command { label, handle } => match handle.try_take() {
                Some(Ok(Ok(state))) => self.undo.push(Arc::new(state), label),
                Some(Ok(Err(CommandError::Cancelled))) => {
                    self.toast(Toast::new(ToastLevel::Info, "job-cancelled"));
                }
                Some(Ok(Err(CommandError::NotAvailable(_)))) => {
                    self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                }
                Some(Ok(Err(e))) => self.job_error("ws-toast-command-failed", e.to_string()),
                Some(Err(e)) => self.job_panic(&e),
                None => {}
            },
            Task::Save(save) => match save.handle.try_take() {
                Some(Ok(Ok(()))) => {
                    let SaveTask {
                        path,
                        state,
                        manifest,
                        then,
                        ..
                    } = *save;
                    self.saved = state;
                    self.manifest = Some(manifest);
                    self.path = Some(path.clone());
                    self.read_only = false;
                    self.toast(Toast::new(ToastLevel::Success, "ws-toast-saved"));
                    self.actions.push_back(WorkspaceAction::Saved(path));
                    if let Some(exit) = then {
                        self.actions.push_back(exit.action());
                    }
                }
                Some(Ok(Err(msg))) => self.job_error("ws-toast-save-failed", msg),
                Some(Err(e)) => self.job_panic(&e),
                None => {}
            },
        }
    }

    fn job_error(&mut self, key: &str, detail: String) {
        self.toast(Toast::new(ToastLevel::Error, key).with_detail(detail));
    }

    fn job_panic(&mut self, e: &JobError) {
        self.job_error("ws-toast-job-failed", e.to_string());
    }

    pub(crate) fn next_action(&mut self) -> WorkspaceAction {
        self.take_action()
    }
}

/// The message of a failed command: the sentence the command wrote, without
/// the "failed:" or "invalid:" the error type puts in front.
fn error_text(e: CommandError) -> String {
    match e {
        CommandError::Failed(m) | CommandError::Invalid(m) => m,
        e => e.to_string(),
    }
}
