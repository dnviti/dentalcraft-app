//! Workspace state and logic (everything except drawing).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use dc_case::{Case, CaseState, RoleSource, ScanRole};
use dc_command::{Command, CommandError, StepHistory, UndoStack};
use dc_jobs::{JobError, JobHandle, JobSystem, Progress};
use dc_module_api::{
    CommandDesc, CommandOutput, CommandSpec, LeafInput, MenuStatus, MenuTree, Module, ModuleCtx,
    NOT_AVAILABLE_KEY, Toast, ToastLevel, is_import_scans_command,
};
use dc_project::{Manifest, Project};
use dc_session::builtins::{CASE_IMPORT, CASE_REMOVE_SCAN, CASE_SET_ROLE};
use dca_algo::diagnosis::edit::EditField;
use egui::KeyboardShortcut;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::autosave::{self, Autosave};
use crate::history::{By, Steps};
use crate::menu::MenuView;
use crate::timeline::TimelineState;
use crate::view::ViewState;
use crate::{import, leaf};
use crate::{keys, project};

/// What the shell must do after a frame of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Nothing.
    None,
    /// Go back to Home (the user confirmed or there was nothing to save).
    GoHome,
    /// The project was saved to this path (by the user or by itself).
    Saved(PathBuf),
    /// The project was closed.
    Closed,
    /// The window may close (after [`Workspace::request_quit`]): the case is
    /// saved, or the user chose not to save it.
    Quit,
    /// The user clicked the gear of the top bar: the shell opens its
    /// Settings dialog (and tells it with [`Workspace::set_shell_dialog`]).
    OpenSettings,
}

/// Where the user wants to go once the unsaved changes are dealt with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    Home,
    Close,
    Quit,
}

impl Exit {
    fn action(self) -> WorkspaceAction {
        match self {
            Exit::Home => WorkspaceAction::GoHome,
            Exit::Close => WorkspaceAction::Closed,
            Exit::Quit => WorkspaceAction::Quit,
        }
    }
}

/// What the top bar says about saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveStatus {
    /// Nothing to say: a new case never saved and not changed, or a file
    /// opened read-only (its badge says it).
    Quiet,
    /// A save runs.
    Saving,
    /// The case changed since it was last saved.
    Unsaved,
    /// The last save of the case as it is failed (the error was shown).
    Failed,
    /// The file holds the case as it is.
    Saved,
}

/// A notification on screen.
#[derive(Debug, Clone)]
pub(crate) struct ShownToast {
    pub toast: Toast,
    /// Time (egui seconds) it was first drawn.
    pub born: Option<f64>,
    /// Stays until the user closes it (errors always do), and is always
    /// drawn: an arch warning must not time out or be pushed off the stack.
    pub sticky: bool,
    /// The failure of a step (a refused command): it goes when another
    /// command succeeds or the selection changes, not only when closed.
    pub until_next: bool,
}

impl ShownToast {
    /// `true` when it never times out.
    pub fn stays(&self) -> bool {
        self.sticky || self.toast.level == ToastLevel::Error
    }
}

/// Indices of the notifications to draw, oldest first: every one that stays
/// until closed, plus the newest of the others up to `max` in all. The bound
/// never hides a sticky one.
pub(crate) fn visible_toasts(toasts: &[ShownToast], max: usize) -> Vec<usize> {
    let stay = toasts.iter().filter(|t| t.stays()).count();
    let mut room = max.saturating_sub(stay);
    let mut keep = vec![false; toasts.len()];
    for (i, t) in toasts.iter().enumerate().rev() {
        if t.stays() {
            keep[i] = true;
        } else if room > 0 {
            keep[i] = true;
            room -= 1;
        }
    }
    (0..toasts.len()).filter(|&i| keep[i]).collect()
}

/// A registered command whose JSON output is shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecKind {
    /// The scan import: its output is the import report.
    Import,
    /// `case.set-role`: its output tells what the module cleared.
    SetRole,
    /// `case.remove-scan`: its output names the scan removed and what the
    /// module cleared.
    RemoveScan,
    /// The command of a menu leaf (by its id): the output of some is shown
    /// ([`leaf::report`]).
    Leaf(&'static str),
    /// The import of a surface of the scan popup (`scan.import`, by its
    /// output): an import, whose end the popup follows.
    ScanImport(dc_scan_ui::ScanOutputId),
}

impl SpecKind {
    /// Title of the notification of a failure.
    fn error_key(self) -> &'static str {
        match self {
            SpecKind::Import | SpecKind::ScanImport(_) => "ws-toast-import-failed",
            SpecKind::SetRole | SpecKind::RemoveScan | SpecKind::Leaf(_) => {
                "ws-toast-command-failed"
            }
        }
    }

    /// Where a failure is reported in the anonymous diagnostics.
    fn source(self, command: &str) -> &'static str {
        match self {
            SpecKind::Import | SpecKind::ScanImport(_) => "import",
            SpecKind::Leaf(_) if command.contains("export") => "export",
            SpecKind::SetRole | SpecKind::RemoveScan | SpecKind::Leaf(_) => "command",
        }
    }
}

pub(crate) enum Task {
    /// A registered command run as a job on a snapshot of the state, keeping
    /// its JSON output (`SpecCommand` drops it): the result is applied as one
    /// undo step and the output is reported.
    Spec {
        kind: SpecKind,
        /// The command id (`aligners.planning.setup`).
        command: &'static str,
        label: String,
        handle: JobHandle<Result<CommandOutput, CommandError>>,
    },
    Command {
        /// The command id, for the step history.
        command: &'static str,
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
    /// Asked for by the user (Save, Save as, leaving); `false`: the
    /// automatic save, which says nothing when it works.
    explicit: bool,
    handle: JobHandle<Result<Vec<dc_project::SaveWarning>, String>>,
}

/// A save asked for while another one runs: one save at a time, so it
/// starts when that one ends.
struct SaveRequest {
    path: PathBuf,
    then: Option<Exit>,
    explicit: bool,
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

    /// `true` once the user asked to stop it and it has not stopped yet (a
    /// job stops at its next look at the flag, which can take a while).
    pub fn cancelling(&self) -> bool {
        match &self.task {
            Task::Spec { handle, .. } => handle.is_cancelled(),
            Task::Command { handle, .. } => handle.is_cancelled(),
            Task::Save(_) => false,
        }
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
    /// The scan files chosen, and the role they are imported with (`None`:
    /// the role chosen in the import inspector, else auto).
    Import(Vec<PathBuf>, Option<ScanRole>),
    SaveAs(Option<PathBuf>, Option<Exit>),
    /// The folder or file a menu leaf asked for (`None`: cancelled).
    Leaf(&'static str, Option<PathBuf>),
}

/// The workspace screen of an open project: menu tree, viewport, inspector,
/// stage timeline and status bar around the project's case state.
pub struct Workspace {
    pub(crate) module: Box<dyn Module>,
    pub(crate) menu: MenuTree,
    pub(crate) commands: Vec<CommandDesc>,
    /// The registered commands of the module, to run a leaf with its output.
    pub(crate) specs: Vec<CommandSpec>,
    pub(crate) shortcuts: Vec<(&'static str, KeyboardShortcut)>,
    pub(crate) shortcut_text: HashMap<&'static str, String>,
    pub(crate) undo: UndoStack,
    /// The state the file holds (as saved or opened).
    saved: Arc<CaseState>,
    /// The timer of the automatic save ([`Workspace::tick`]).
    autosave: Autosave,
    /// A save asked for while another one runs.
    queued_save: Option<SaveRequest>,
    /// Where to go once the save that runs ends (nothing else to save).
    leave_after_save: Option<Exit>,
    /// The step history of each version of the case (`history/steps.json`).
    pub(crate) steps: Steps,
    manifest: Option<Manifest>,
    thumbnail: Option<Vec<u8>>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) read_only: bool,
    pub(crate) jobs: JobSystem,
    pub(crate) running: Vec<Running>,
    dialog: Option<mpsc::Receiver<DialogAnswer>>,
    native_dialogs: bool,
    /// Dialogs requested while native dialogs are off (headless): for tests.
    pub(crate) dialog_requests: usize,
    pub(crate) egui_ctx: Option<egui::Context>,
    pub(crate) view: ViewState,
    pub(crate) manual: crate::manual::Manual,
    /// The component selected in the viewport and its gizmo.
    pub(crate) selection: crate::select::Selection,
    /// The view of the region of the selected tooth on the scan.
    pub(crate) region: crate::region::RegionView,
    /// The arch line of the final position: shown, its points moved.
    pub(crate) archline: crate::archline::ArchLine,
    /// The separation parameters of "Separate teeth and gum" being dragged.
    pub(crate) segment: leaf::SegmentUi,
    /// The final position parameter being dragged.
    pub(crate) setup_ui: crate::setup::SetupUi,
    /// The parameter of "Aligners" being dragged or typed, and the speed
    /// chosen for the selected tooth.
    pub(crate) staging_ui: crate::staging::StagingUi,
    /// What the last runs of the aligners and of the tooth marks warned about.
    pub(crate) plan_warnings: crate::plan_warnings::PlanWarnings,
    /// The production setting being dragged or typed.
    pub(crate) production_ui: crate::production::ProductionUi,
    /// The filter and the record chosen in "Clinical review".
    pub(crate) review_ui: crate::review::ReviewUi,
    /// The value of "Manual corrections" of the case analysis being typed
    /// or dragged.
    pub(crate) diagnosis_ui: leaf::DiagnosisUi,
    /// Whether the case needs the bite before "Final position".
    pub(crate) bite_check: leaf::BiteCheck,
    pub(crate) menu_view: MenuView,
    /// The widths of the side panels and where the last frame drew them.
    pub(crate) panels: crate::panels::Panels,
    pub(crate) selected: Option<&'static str>,
    /// Role the user chose for the next import (`None` = auto): it applies
    /// to the next file dialog or drop and goes back to auto.
    pub(crate) import_role: Option<ScanRole>,
    /// Thickness of the base of "Close with a base" in the Import scans
    /// inspector (mm; the plugin's `base_height`, default 3).
    pub(crate) base_height_mm: f32,
    /// Whether each arch scan blob with no base is open underneath, asked
    /// once per blob (`dca_algo::scan_base::is_open`) for the status of
    /// "Close with a base".
    pub(crate) scan_open: std::collections::BTreeMap<dc_case::BlobRef, bool>,
    import_spec: Option<CommandSpec>,
    set_role_spec: Option<CommandSpec>,
    remove_scan_spec: Option<CommandSpec>,
    /// The scan the user asked to remove, waiting for the confirmation.
    pub(crate) confirm_remove: Option<Uuid>,
    pub(crate) timeline: TimelineState,
    pub(crate) toasts: Vec<ShownToast>,
    pub(crate) confirm_exit: Option<Exit>,
    pub(crate) show_help: bool,
    /// A dialog of the shell (Settings) is over the workspace: it takes the
    /// keys and the dropped files.
    pub(crate) shell_dialog: bool,
    /// The 3D scanner and its popup (`scan`).
    pub(crate) scan: dc_scan_ui::ScanPopup,
    /// The surface of the popup being imported: its file lives until the
    /// import job has read it.
    pub(crate) scan_output: Option<dc_scan_ui::ScanOutput>,
    /// The print popup (`print`), opened by a leaf whose output lists parts
    /// to print.
    pub(crate) print: dc_print_ui::PrintPopup,
    /// Records of saved print files waiting for their turn
    /// (`aligners.print.mark` parameters).
    pub(crate) print_marks: VecDeque<Value>,
    /// The command of a menu leaf an agent runs
    /// ([`Workspace::run_leaf_command`]) and its answer once it ended.
    agent_leaf: Option<(&'static str, Option<Result<Value, String>>)>,
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
        ws.scan = dc_scan_ui::ScanPopup::new(Some(render_state.clone()));
        ws.print = dc_print_ui::PrintPopup::new(dc_print_ui::ResinStore::standard())
            .with_native_dialogs(true);
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
        let (state, history, manifest, thumbnail) = match project {
            Some(p) => (p.case_state, p.history, Some(p.manifest), p.thumbnail),
            None => (
                CaseState::new(Case::new("")),
                Value::Array(Vec::new()),
                None,
                None,
            ),
        };
        let state = Arc::new(state);
        let menu = module.menu();
        let steps = Steps::open(&menu, history, &state, |id| module.leaf_done(&state, id));
        let unreadable = steps.unreadable().map(str::to_owned);
        let commands = module.commands();
        let specs = module.command_specs();
        let mut shortcuts = Vec::new();
        let mut shortcut_text = HashMap::new();
        for c in &commands {
            if let Some(text) = c.shortcut
                && let Some(sc) = keys::parse(text)
            {
                shortcuts.push((c.id, sc));
                shortcut_text.insert(c.id, dc_ui::keys::platform_keys(text).into_owned());
            }
        }
        // the module's own import when it has one, the core's otherwise
        let builtins = dc_session::builtins::specs();
        let builtin = |id: &str| builtins.iter().find(|s| s.id == id).copied();
        let import_spec = specs
            .iter()
            .copied()
            .find(|s| is_import_scans_command(s.id))
            .or_else(|| builtin(CASE_IMPORT));
        let set_role_spec = builtin(CASE_SET_ROLE);
        let remove_scan_spec = builtin(CASE_REMOVE_SCAN);
        let jobs = JobSystem::new(0)
            .or_else(|_| JobSystem::new(1))
            .expect("cannot start the background job threads");
        let mut ws = Self {
            menu_view: MenuView::new(&menu),
            panels: crate::panels::Panels::default(),
            module,
            menu,
            commands,
            specs,
            shortcuts,
            shortcut_text,
            undo: UndoStack::new(state.clone(), UndoStack::DEFAULT_BUDGET),
            saved: state,
            autosave: Autosave::default(),
            queued_save: None,
            leave_after_save: None,
            steps,
            manifest,
            thumbnail,
            path,
            read_only: unreadable.is_some(),
            jobs,
            running: Vec::new(),
            dialog: None,
            native_dialogs: false,
            dialog_requests: 0,
            egui_ctx: None,
            view: ViewState::default(),
            manual: crate::manual::Manual::default(),
            selection: crate::select::Selection::default(),
            region: crate::region::RegionView::default(),
            archline: crate::archline::ArchLine::default(),
            segment: leaf::SegmentUi::default(),
            setup_ui: crate::setup::SetupUi::default(),
            staging_ui: crate::staging::StagingUi::default(),
            plan_warnings: crate::plan_warnings::PlanWarnings::default(),
            production_ui: crate::production::ProductionUi::default(),
            review_ui: crate::review::ReviewUi::default(),
            diagnosis_ui: leaf::DiagnosisUi::default(),
            bite_check: leaf::BiteCheck::default(),
            selected: None,
            import_role: None,
            base_height_mm: dca_algo::scan_base::HEIGHT_DEFAULT_MM as f32,
            scan_open: std::collections::BTreeMap::new(),
            import_spec,
            set_role_spec,
            remove_scan_spec,
            confirm_remove: None,
            timeline: TimelineState::default(),
            toasts: Vec::new(),
            confirm_exit: None,
            show_help: false,
            shell_dialog: false,
            scan: dc_scan_ui::ScanPopup::default(),
            scan_output: None,
            print: dc_print_ui::PrintPopup::default(),
            print_marks: VecDeque::new(),
            agent_leaf: None,
            actions: VecDeque::new(),
        };
        if let Some(why) = unreadable {
            // never written over with a history that lost its records
            tracing::warn!(
                code = "history-unreadable",
                source = "project",
                module = ws.module.id(),
                "the step history of the project cannot be read: {why}"
            );
            ws.toast_sticky(
                Toast::new(ToastLevel::Warning, "ws-toast-history-unreadable").with_detail(
                    dc_i18n::tr_args("ws-history-unreadable-detail", &[("reason", &why)]),
                ),
            );
        }
        // the doubts the automatic numbering left on the marks the case
        // holds: said again, they stay until the marks change
        for toast in leaf::numbering_doubt_toasts(ws.undo.current()) {
            ws.toast_sticky(toast);
        }
        ws
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

    /// `true` when the changes are saved by themselves ([`Workspace::tick`]):
    /// the project has a file and was not opened read-only.
    pub fn autosaves(&self) -> bool {
        self.path.is_some() && !self.read_only
    }

    /// `true` while a save runs (asked for, or the automatic one).
    pub fn is_saving(&self) -> bool {
        self.running.iter().any(|r| matches!(r.task, Task::Save(_)))
    }

    /// What the top bar says about saving.
    pub(crate) fn save_status(&self) -> SaveStatus {
        if self.is_saving() {
            SaveStatus::Saving
        } else if self.is_dirty() {
            if self.autosave.failed_on(self.undo.current()) {
                SaveStatus::Failed
            } else {
                SaveStatus::Unsaved
            }
        } else if self.autosaves() {
            SaveStatus::Saved
        } else {
            SaveStatus::Quiet
        }
    }

    /// Marks the project as read-only (opened from a damaged or newer
    /// file): saving then always asks for a new path. A project whose step
    /// history could not be read stays read-only: its file is never written
    /// over with a history that lost its records.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only || self.steps.unreadable().is_some();
    }

    /// `true` when the file of the project must not be written over
    /// ([`Workspace::set_read_only`]): saving asks for another file.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// `true` when saving to `path` would write over the file opened
    /// read-only ([`Workspace::is_read_only`]): the same file however it is
    /// written (a relative path, `..`, a folder reached through a link, the
    /// letter case on Windows), as long as both exist.
    pub fn writes_over_read_only(&self, path: &Path) -> bool {
        self.read_only && self.path.as_deref().is_some_and(|p| same_file(p, path))
    }

    /// The step history of the case as it is now, saved as
    /// `history/steps.json`: the functions that ran, automatically or by
    /// hand, whose results the case still holds.
    pub fn history(&self) -> &StepHistory {
        self.steps.history()
    }

    /// Tells the workspace whether a dialog of the shell (Settings) is on
    /// screen: while it is, the keyboard shortcuts and the files dropped on
    /// the window are not the workspace's.
    pub fn set_shell_dialog(&mut self, open: bool) {
        self.shell_dialog = open;
    }

    /// The gear of the top bar: asks the shell for its Settings dialog.
    pub(crate) fn open_settings(&mut self) {
        self.actions.push_back(WorkspaceAction::OpenSettings);
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

    /// i18n keys of the notifications currently listed that stay until the
    /// user closes them (oldest first): errors and the arch warnings.
    pub fn sticky_toast_keys(&self) -> Vec<String> {
        self.toasts
            .iter()
            .filter(|t| t.stays())
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

    /// Undoes the last change (no-op while a command runs); the step history
    /// goes back with it.
    pub fn undo(&mut self) -> bool {
        if self.command_running() || self.undo.undo().is_none() {
            return false;
        }
        self.follow_history();
        true
    }

    /// Redoes the last undone change (no-op while a command runs); the step
    /// history comes back with it.
    pub fn redo(&mut self) -> bool {
        if self.command_running() || self.undo.redo().is_none() {
            return false;
        }
        self.follow_history();
        true
    }

    /// The step history of the version of the case the undo stack is on.
    fn follow_history(&mut self) {
        let state = Arc::clone(self.undo.current());
        let module = &self.module;
        self.steps.follow(&state, |id| module.leaf_done(&state, id));
    }

    /// `true` while an import or a command changes the case in the
    /// background: undo, redo, commands and imports wait for it (a save only
    /// reads the state).
    pub(crate) fn command_running(&self) -> bool {
        self.running
            .iter()
            .any(|r| !matches!(r.task, Task::Save(_)))
    }

    /// Selects menu row `id` (a group, sub-menu or function), shows it in the
    /// menu and tells the module. It never runs anything (the menu law, see
    /// `function`): the inspector of a function has the button that runs it.
    /// The tool of another function closes.
    pub fn select_item(&mut self, id: &'static str) {
        // a refused step's message belongs to the function it was shown for
        if self.selected != Some(id) {
            self.clear_step_errors();
        }
        let command = self.menu.find(id).and_then(|i| i.command_id());
        if let Some(tool) = self.manual.tool
            && crate::function::owner(tool) != command
        {
            self.close_tool();
        }
        // the view of a tooth region belongs to the 3D teeth
        if command != Some(leaf::BUILD_TEETH) {
            self.set_region_view(false);
        }
        self.selected = Some(id);
        self.module.select(id);
        self.menu_view.reveal(&self.menu, id);
    }

    /// Selects the menu entry `id`, or else the leaf that runs command `id`,
    /// as a click on it does ([`Workspace::select_item`]): its id, or
    /// `None` when the menu has neither (an agent's click).
    pub fn select_entry(&mut self, id: &str) -> Option<&'static str> {
        let items = || self.menu.iter().map(|(_, i)| i);
        let found = items()
            .find(|i| i.id == id)
            .or_else(|| items().find(|i| i.command_id() == Some(id)))?
            .id;
        self.select_item(found);
        Some(found)
    }

    /// Selects the leaf of the menu that runs `command_id`; `false` when the
    /// menu has none.
    pub(crate) fn select_command(&mut self, command_id: &str) -> bool {
        let id = self
            .menu
            .iter()
            .map(|(_, i)| i)
            .find(|i| i.command_id() == Some(command_id))
            .map(|i| i.id);
        match id {
            Some(id) => {
                self.select_item(id);
                true
            }
            None => false,
        }
    }

    /// Nothing selected (Escape): the inspector shows the overview and the
    /// open tool closes.
    pub(crate) fn clear_selection(&mut self) {
        if self.selected.is_some() {
            self.clear_step_errors();
        }
        self.selected = None;
        self.close_tool();
        self.set_region_view(false);
    }

    /// The key of a command of the module ([`CommandDesc::shortcut`]): it
    /// selects its function, and the key of a tool also opens that tool in
    /// it. It never runs anything.
    pub(crate) fn press_key(&mut self, command_id: &'static str) {
        match crate::manual::Tool::from_command(command_id) {
            Some(tool) => {
                self.open_tool(tool);
            }
            None => {
                self.select_command(command_id);
            }
        }
    }

    /// The command of the manual tool that is open, if any.
    pub fn active_tool(&self) -> Option<&'static str> {
        self.manual.tool.map(crate::manual::Tool::command_id)
    }

    /// Runs the function `command_id` of the menu, as the button of its
    /// inspector does (a click on the menu only selects it): "Import scans"
    /// opens the file dialog (handled by the workspace), and so do the leaves
    /// that need a folder or a file ([`LeafInput`]), which run with the
    /// answer; everything else goes to the module, whose commands run in the
    /// background (the output of the ones that have something to say is
    /// shown) and whose toasts are shown. While a command runs it only shows
    /// the "busy" notice.
    pub fn run_leaf(&mut self, command_id: &str) {
        // the import needs its files: the workspace asks for them and runs the
        // shared import command, so the module is not invoked
        if is_import_scans_command(command_id) {
            if self.command_running() {
                self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            } else {
                self.request_import_dialog(None);
            }
            return;
        }
        let item = self
            .menu
            .iter()
            .map(|(_, i)| i)
            .find(|i| i.command_id() == Some(command_id))
            .and_then(|i| Some((i.command_id()?, i.input)));
        // a leaf that needs a folder or a file: ask, then run the command with
        // the answer
        if let Some((id, Some(input))) = item
            && self
                .commands
                .iter()
                .any(|c| c.id == command_id && c.status == MenuStatus::Ready)
        {
            if self.command_running() {
                self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            } else {
                self.request_leaf_dialog(id, input);
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
        // a leaf's one command runs as a spec, to read its output
        let spec = item.filter(|_| cmds.len() == 1).and_then(|(id, _)| {
            self.specs
                .iter()
                .copied()
                .find(|s| s.id == id)
                .map(|s| (id, s))
        });
        match spec {
            Some((id, spec)) => {
                let params = leaf::run_params(id, &state);
                self.spawn_leaf(id, spec, params);
            }
            None => {
                for cmd in cmds {
                    self.run_command(cmd);
                }
            }
        }
        if let Err(e) = result {
            let stub = self
                .commands
                .iter()
                .any(|c| c.id == command_id && c.status == MenuStatus::Stub);
            if !stub {
                tracing::error!(
                    code = "command-failed",
                    source = "command",
                    command = command_id,
                    module = self.module.id(),
                    "{command_id} did not start: {e}"
                );
                self.toast(
                    Toast::new(ToastLevel::Error, "ws-toast-command-failed")
                        .with_detail(e.to_string()),
                );
            }
        }
    }

    /// Runs the command of a menu leaf for an agent connected to the app
    /// (`dentalcraft --control`) with `params`, as the button of its
    /// inspector runs it: in the background, one undo step when it changes
    /// the case, its report, and what it shows in the view (the study models
    /// just built, the scan with new tooth colours). The answer comes from
    /// [`Workspace::take_leaf_answer`]. `false` when `command_id` is not the
    /// command of a menu leaf.
    pub fn run_leaf_command(&mut self, command_id: &str, params: Value) -> bool {
        let Some(id) = self
            .menu
            .iter()
            .find_map(|(_, i)| i.command_id().filter(|c| *c == command_id))
        else {
            return false;
        };
        let Some(spec) = self.module_spec(id) else {
            return false;
        };
        self.agent_leaf = Some((id, None));
        self.spawn_leaf(id, spec, params);
        true
    }

    /// The output of the command [`Workspace::run_leaf_command`] started
    /// (its failure as text) once it ended; `None` while it runs, or when it
    /// did not start.
    pub fn take_leaf_answer(&mut self) -> Option<Result<Value, String>> {
        if self.agent_leaf.as_ref().is_some_and(|(_, a)| a.is_some()) {
            return self.agent_leaf.take().and_then(|(_, a)| a);
        }
        None
    }

    /// The button "Import the bite scans" of Bite (`oa.import_bite`): the
    /// file dialog, then the files are imported with the role bite. Nothing
    /// runs before the answer; refused while a command runs.
    pub(crate) fn import_bite_scans(&mut self) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
        } else {
            self.request_import_dialog(Some(ScanRole::Bite));
        }
    }

    /// The answer to the folder or file dialog of leaf `command_id` (the
    /// workspace calls it when the dialog closes): runs the command with the
    /// path as the parameter the leaf names, in the background, one undo step
    /// when the command changes the case. `None` is a cancelled dialog and
    /// does nothing. Refused (with a notification) while another command
    /// runs.
    pub fn answer_leaf_input(&mut self, command_id: &str, path: Option<PathBuf>) {
        let Some(path) = path else { return };
        let Some((id, input)) = self
            .menu
            .iter()
            .map(|(_, i)| i)
            .find(|i| i.command_id() == Some(command_id))
            .and_then(|i| Some((i.command_id()?, i.input?)))
        else {
            return;
        };
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let Some(spec) = self.specs.iter().copied().find(|s| s.id == id) else {
            self.job_error("ws-toast-command-failed", format!("no command {id} to run"));
            return;
        };
        let (LeafInput::Folder { param } | LeafInput::ScanFile { param }) = input;
        let params = json!({ param: path.to_string_lossy() });
        self.spawn_leaf(id, spec, params);
    }

    /// Starts the command of menu leaf `id` as a job and shows its output
    /// when it ends ([`leaf::report`]). The caller has checked that it is
    /// idle (here too: busy gets the notification).
    fn spawn_leaf(&mut self, id: &'static str, spec: CommandSpec, params: Value) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some((title, line)) = leaf::long_notice(id) {
            self.toast(Toast::new(ToastLevel::Info, title).with_detail(crate::t(line)));
        }
        self.run_spec(spec, params, SpecKind::Leaf(id), leaf::job_name_key(id));
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

    /// Imports the surface the scan popup wrote to `path` (`scan.import`,
    /// its output `id`) with the import of a file chosen by hand: the same
    /// command, one undoable change, the import report. Refused while
    /// another import or command runs.
    pub(crate) fn import_scan_file(
        &mut self,
        path: PathBuf,
        role: Option<ScanRole>,
        id: dc_scan_ui::ScanOutputId,
    ) -> Result<(), dc_scan_ui::ScanFailure> {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return Err(dc_scan_ui::ScanFailure::new(
                "busy",
                "another operation is still running on the case",
            ));
        }
        let Some(spec) = self.import_spec else {
            return Err(dc_scan_ui::ScanFailure::new("import", "no import command"));
        };
        let params = import::params(&[path], role);
        self.run_spec(spec, params, SpecKind::ScanImport(id), "ws-job-import");
        Ok(())
    }

    /// Gives scan `scan_id`, now `from`, the role `to` (`case.set-role`: a
    /// scan already there takes `from`). One undoable change; the scan is
    /// marked as chosen by the user, and what the module cleared is reported.
    /// An arch scan moved to the other arch goes through the module's
    /// `aligners.case.set-arch` when it has it: its tooth marks, colours and
    /// teeth are renumbered instead of cleared. Refused while an import or
    /// command runs.
    pub fn set_scan_role(&mut self, scan_id: Uuid, from: ScanRole, to: ScanRole) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        let arches = from != ScanRole::Bite && to != ScanRole::Bite;
        if from != to
            && arches
            && let Some(spec) = self.module_spec(leaf::SET_ARCH)
        {
            let params = json!({ "scan": import::role_id(from), "arch": import::role_id(to) });
            self.run_spec(
                spec,
                params,
                SpecKind::Leaf(leaf::SET_ARCH),
                "ws-job-command",
            );
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

    /// Asks the user to confirm the removal of scan `scan_id` (a dialog);
    /// nothing changes until they do. Ignored for a scan the case no longer
    /// has; refused, with the reason, for an arch scan the arches lock
    /// ([`Workspace::scan_removal_locked`]).
    pub fn ask_remove_scan(&mut self, scan_id: Uuid) {
        if self.undo.current().case.scan_by_id(scan_id).is_none() {
            return;
        }
        if let Some(why) = self.scan_removal_locked(scan_id) {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-scan-locked").with_detail(why));
            return;
        }
        self.confirm_remove = Some(scan_id);
    }

    /// The scan whose removal waits for the user's confirmation.
    pub fn pending_scan_removal(&self) -> Option<Uuid> {
        self.confirm_remove
    }

    /// Removes scan `scan_id` from the case (`case.remove-scan`). One
    /// undoable change: what the module computed from the scan is cleared
    /// and reported, Ctrl+Z brings both back. The open tool closes first (the
    /// points of "Three points" may lie on that scan). Refused while an
    /// import or command runs.
    pub fn remove_scan(&mut self, scan_id: Uuid) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some(spec) = self.remove_scan_spec {
            self.close_tool();
            let params = json!({ "scan_id": scan_id.to_string() });
            self.run_spec(spec, params, SpecKind::RemoveScan, "ws-job-command");
        }
    }

    /// Why scan `scan_id` cannot be removed now, in words: an arch scan
    /// while a module holds data this version cannot read (the same refusal
    /// as `case.remove-scan`). `None` when it can.
    pub(crate) fn scan_removal_locked(&self, scan_id: Uuid) -> Option<String> {
        let state = self.undo.current();
        let scan = state.case.scan_by_id(scan_id)?;
        if scan.role == ScanRole::Bite {
            return None;
        }
        let module = state
            .modules
            .iter()
            .find_map(|(id, m)| m.is_opaque().then(|| id.clone()))?;
        Some(dc_case::import::ImportError::ArchesLocked { module }.to_string())
    }

    /// Turns the occlusal side of the arch scan `role` over, or back
    /// (`aligners.case.set-arch` with `flip_occlusal`). One undoable change.
    /// Refused while an import or command runs.
    pub fn flip_occlusal(&mut self, role: ScanRole) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some(spec) = self.module_spec(leaf::SET_ARCH) {
            let params = json!({ "scan": import::role_id(role), "flip_occlusal": true });
            self.run_spec(
                spec,
                params,
                SpecKind::Leaf(leaf::SET_ARCH),
                "ws-job-command",
            );
        }
    }

    /// Closes the arch scan `role` with a flat base `height_mm` thick
    /// (`aligners.case.make-base`). One undoable change. Refused while an
    /// import or command runs.
    pub fn close_with_base(&mut self, role: ScanRole, height_mm: f64) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some(spec) = self.module_spec(leaf::MAKE_BASE) {
            let params = json!({ "arch": import::role_id(role), "height_mm": height_mm });
            self.run_spec(
                spec,
                params,
                SpecKind::Leaf(leaf::MAKE_BASE),
                "ws-job-command",
            );
        }
    }

    /// Writes by hand (`Some`, mm) or resets to the computed one (`None`) a
    /// value of the case analysis of the arch `role`
    /// (`aligners.diagnosis.edit-values`). One undoable change; the
    /// complexity follows. Refused while an import or command runs.
    pub fn edit_diagnosis(&mut self, role: ScanRole, field: EditField, value: Option<f64>) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some(spec) = self.module_spec(leaf::DIAGNOSIS_EDIT) {
            let params = json!({
                "arch": import::role_id(role),
                "values": { field.key(): value },
            });
            self.run_spec(
                spec,
                params,
                SpecKind::Leaf(leaf::DIAGNOSIS_EDIT),
                "ws-job-command",
            );
        }
    }

    /// The module's command `id`, when it has one.
    pub(crate) fn module_spec(&self, id: &str) -> Option<CommandSpec> {
        self.specs.iter().copied().find(|s| s.id == id)
    }

    /// Runs the module's command `id` with `params` in the background as the
    /// command of a button (`name_key` in the status bar, with its progress
    /// and a cancel; one undo step when it changes the case, its report when
    /// it ends). Refused, with a notification, while another command runs.
    pub(crate) fn run_button_command(
        &mut self,
        id: &'static str,
        params: Value,
        name_key: &'static str,
    ) {
        if self.command_running() {
            self.toast(Toast::new(ToastLevel::Warning, "ws-toast-busy"));
            return;
        }
        if let Some(spec) = self.module_spec(id) {
            self.run_spec(spec, params, SpecKind::Leaf(id), name_key);
        }
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
        let (command, label) = (spec.id, spec.label_key.to_owned());
        let state = self.undo.current().clone();
        let handle = self
            .jobs
            .spawn(name_key, move |ctx| spec.execute(&state, &params, ctx));
        self.running.push(Running {
            name_key,
            task: Task::Spec {
                kind,
                command,
                label,
                handle,
            },
        });
    }

    /// Shows what an import did, as notifications, most serious first: one
    /// per failed file with its reason; one per arch warning (role guessed,
    /// to check, taken or not recognised); a notice when a scan was replaced
    /// or a module cleared results (it lists only what was affected); the
    /// units warnings; the other warnings, each with its translated
    /// `message`; and a line with the number of scans added. Failed files
    /// and arch warnings are never cut or grouped and stay until closed; the
    /// rest times out and, past a handful, is grouped in one line.
    ///
    /// `report` is the JSON output of the import command (module
    /// `import-scans` or `case.import`): the arrays `imported` (the scans
    /// added), `failed` (`reason`), `warnings` (`code`, `key`, `args`,
    /// `message`), `module_cleared` (ids), and `replaced` (bool). Missing
    /// fields count as empty, so a serialized `ImportReport` works too.
    pub fn show_import_report(&mut self, report: &Value) {
        for t in import::report_toasts(report, self.module.id()) {
            if t.sticky {
                self.toast_sticky(t.toast);
            } else {
                self.toast(t.toast);
            }
        }
    }

    /// Notification of a role change: what the module cleared or marked to
    /// be redone, if anything.
    fn show_role_report(&mut self, report: &Value) {
        if let Some(toast) = import::role_changed_toast(self.module.id(), report) {
            self.toast(toast);
        }
    }

    /// Notifications of a removed scan: which one went, and what the module
    /// cleared with it. The selection of the scan goes too.
    fn show_remove_report(&mut self, report: &Value) {
        let removed = report
            .get("scan_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());
        if removed
            .is_some_and(|id| self.selection.component == Some(crate::select::Component::Scan(id)))
        {
            self.select_component(None);
        }
        let name = report
            .get("source_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.toast(
            Toast::new(ToastLevel::Success, "ws-toast-scan-removed").with_detail(dc_i18n::tr_args(
                "ws-scan-removed-detail",
                &[("name", name)],
            )),
        );
        if let Some(toast) = import::removed_toast(self.module.id(), report) {
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
        let command = cmd.id();
        let label = cmd.label_key().to_owned();
        let state = self.undo.current().clone();
        let handle = self
            .jobs
            .spawn(label.clone(), move |ctx| cmd.apply(&state, ctx));
        self.running.push(Running {
            name_key: "ws-job-command",
            task: Task::Command {
                command,
                label,
                handle,
            },
        });
    }

    /// Saves to the current path, or asks for one (also when read-only).
    /// While another save runs it starts when that one ends.
    pub fn save(&mut self) {
        self.save_then(None);
    }

    /// Asks for a new path and saves there.
    pub fn save_as(&mut self) {
        self.request_save_dialog(None);
    }

    pub(crate) fn save_then(&mut self, then: Option<Exit>) {
        match self.path.clone() {
            Some(p) if !self.read_only => self.save_to_then(p, then, true),
            _ => self.request_save_dialog(then),
        }
    }

    /// Saves to `path` in the background (`.dcprj` added when missing).
    /// While another save runs it starts when that one ends.
    pub fn save_to(&mut self, path: PathBuf) {
        self.save_to_then(path, None, true);
    }

    /// Saves the case as it is now to `path`, then goes to `then`. One save
    /// runs at a time: a save asked for while another runs joins it when
    /// that one writes this same case to the same file, else it starts when
    /// that one ends (and saves the case as it is then). The file opened
    /// read-only is never written over: the user is told to save to another
    /// file, and an exit waiting for the save asks again.
    fn save_to_then(&mut self, path: PathBuf, then: Option<Exit>, explicit: bool) {
        let path = project::with_extension(path);
        let current = Arc::clone(self.undo.current());
        // damaged, newer, or with a step history that cannot be read: the
        // user's situation, not a failure to report
        if self.writes_over_read_only(&path) {
            self.refuse_read_only("ws-save-read-only", then);
            return;
        }
        let queued = self.queued_save.is_some();
        let running = self.running.iter_mut().find_map(|r| match &mut r.task {
            Task::Save(s) => Some(s),
            _ => None,
        });
        if let Some(task) = running {
            if !queued && task.path == path && Arc::ptr_eq(&task.state, &current) {
                task.then = task.then.or(then);
                task.explicit |= explicit;
            } else {
                let q = self.queued_save.take();
                self.queued_save = Some(SaveRequest {
                    path,
                    then: then.or(q.as_ref().and_then(|q| q.then)),
                    explicit: explicit || q.is_some_and(|q| q.explicit),
                });
            }
            return;
        }
        // the history of the case saved, whatever put it on the undo stack
        self.follow_history();
        let built = project::build(
            self.manifest.as_ref(),
            &current,
            self.steps.history(),
            self.thumbnail.as_ref(),
        );
        let proj = match built {
            Ok(p) => p,
            Err(detail) => {
                self.save_failed(&current, then, explicit, detail);
                return;
            }
        };
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
                state: current,
                manifest,
                then,
                explicit,
                handle,
            })),
        });
    }

    /// A save to the file opened read-only did not happen: the user is told
    /// why (`key`), and an exit waiting for the save asks again.
    fn refuse_read_only(&mut self, key: &str, then: Option<Exit>) {
        self.job_error("ws-toast-save-failed", crate::t(key));
        if let Some(exit) = then {
            self.confirm_exit = Some(exit);
        }
    }

    /// The automatic save, called every frame ([`Workspace::show`] does)
    /// with the time `now` in seconds, on any clock that only goes forward.
    ///
    /// A project that has a file and was not opened read-only is saved by
    /// itself once the case has not changed for [`autosave::DELAY_S`]
    /// seconds (each change starts the timer again). It waits while a job
    /// that changes the case runs, while a save runs and while a dialog is
    /// open, and saves as soon as they end. A save that failed is not tried
    /// again by itself until the case changes (the error was shown). A
    /// project never saved is not saved by itself and nothing asks for a
    /// file: Save does.
    ///
    /// Returns how long to wait before a call can save (the shell asks for a
    /// frame then); `None` when nothing waits for the time.
    pub fn tick(&mut self, now: f64) -> Option<Duration> {
        let current = Arc::clone(self.undo.current());
        let idle = self.autosave.idle_for(&current, now);
        if !self.autosaves() || !self.is_dirty() || self.autosave.failed_on(&current) {
            return None;
        }
        if self.command_running() || self.is_saving() || self.modal_open() {
            return None;
        }
        let left = autosave::DELAY_S - idle;
        if left > 0.0 {
            return Some(Duration::from_secs_f64(left));
        }
        if let Some(path) = self.path.clone() {
            self.save_to_then(path, None, false);
        }
        None
    }

    /// `true` while a dialog waits for the user: leaving with unsaved
    /// changes, the keyboard shortcuts, or a native file dialog.
    fn modal_open(&self) -> bool {
        self.confirm_exit.is_some()
            || self.confirm_remove.is_some()
            || self.show_help
            || self.dialog.is_some()
    }

    /// `true` while a request to leave ([`Workspace::request_quit`]) may
    /// still end in its action: a save runs or waits, a dialog asks the
    /// user (unsaved changes, where to save), or an action waits for the
    /// shell. `false` once that is over without leaving (the user chose to
    /// stay).
    pub fn is_leaving(&self) -> bool {
        self.is_saving()
            || self.queued_save.is_some()
            || self.leave_after_save.is_some()
            || self.confirm_exit.is_some()
            || self.dialog.is_some()
            || !self.actions.is_empty()
    }

    /// Leaves towards `exit` once nothing is left to save: a project that
    /// has a file is saved first (as the automatic save would), one never
    /// saved or opened read-only asks the user; a save that runs ends first.
    pub(crate) fn request_exit(&mut self, exit: Exit) {
        if self.is_dirty() {
            if self.autosaves() {
                self.save_then(Some(exit));
            } else {
                self.confirm_exit = Some(exit);
            }
        } else if self.is_saving() {
            self.leave_after_save = Some(exit);
        } else {
            self.actions.push_back(exit.action());
        }
    }

    /// The window is asked to close. `true`: nothing is left to save, it may
    /// close now. `false`: keep it open; the case is saved first (a project
    /// with a file) or the user is asked (a project never saved, or opened
    /// read-only, or a save that failed), and [`WorkspaceAction::Quit`]
    /// comes when it may close.
    pub fn request_quit(&mut self) -> bool {
        if !self.is_dirty() && !self.is_saving() {
            return true;
        }
        match self.confirm_exit {
            // the dialog already asks: leaving now closes the window
            Some(_) => self.confirm_exit = Some(Exit::Quit),
            None => self.request_exit(Exit::Quit),
        }
        false
    }

    /// Leaves without saving.
    pub(crate) fn discard_and_exit(&mut self, exit: Exit) {
        self.confirm_exit = None;
        self.leave_after_save = None;
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
        self.toasts.push(ShownToast {
            toast,
            born: None,
            sticky: false,
            until_next: false,
        });
    }

    /// A notification that stays until the user closes it.
    pub(crate) fn toast_sticky(&mut self, toast: Toast) {
        self.toasts.push(ShownToast {
            toast,
            born: None,
            sticky: true,
            until_next: false,
        });
    }

    /// Drops the failure messages of refused steps: another command
    /// succeeded, or the selection changed.
    fn clear_step_errors(&mut self) {
        self.toasts.retain(|t| !t.until_next);
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

    /// `true` when command `id` counts as done: the case holds its result
    /// ([`Module::leaf_done`]), it is "Import scans" and the case has scans
    /// none of which is still a guess to check, or it is a function that is
    /// not a step of the workflow and whose run, in the step history, worked
    /// on the case as it is (an export leaves nothing in the case).
    pub(crate) fn is_done(&self, id: &str) -> bool {
        if is_import_scans_command(id) {
            let scans = &self.undo.current().case.scans;
            if !scans.is_empty()
                && scans
                    .iter()
                    .all(|s| s.role_source != RoleSource::Provisional)
            {
                return true;
            }
        }
        self.module.leaf_done(self.undo.current(), id)
            || self.steps.done_by_record(self.undo.current(), id)
    }

    // ------------------------------------------------------------ dialogs

    fn request_import_dialog(&mut self, role: Option<ScanRole>) {
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
            DialogAnswer::Import(files, role)
        });
    }

    fn request_leaf_dialog(&mut self, id: &'static str, input: LeafInput) {
        if !self.native_dialogs {
            self.dialog_requests += 1;
            return;
        }
        let ctx = self.egui_ctx.clone();
        self.spawn_dialog(move || {
            let path = leaf::pick(input);
            if let Some(c) = ctx {
                c.request_repaint();
            }
            DialogAnswer::Leaf(id, path)
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
                        DialogAnswer::Import(files, role) => {
                            if role.is_some() && !files.is_empty() {
                                self.import_role = role;
                            }
                            self.import_files(files);
                        }
                        // Save as on the very file opened read-only
                        DialogAnswer::SaveAs(Some(p), then)
                            if self.writes_over_read_only(&project::with_extension(p.clone())) =>
                        {
                            self.refuse_read_only("ws-save-as-read-only", then);
                        }
                        DialogAnswer::SaveAs(Some(p), then) => self.save_to_then(p, then, true),
                        DialogAnswer::SaveAs(None, _) => {}
                        DialogAnswer::Leaf(id, path) => self.answer_leaf_input(id, path),
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
        self.poll_scan();
        self.poll_print();
    }

    fn finish(&mut self, task: Task) {
        match task {
            Task::Spec {
                kind,
                command,
                label,
                handle,
            } => {
                let taken = handle.try_take();
                if let SpecKind::ScanImport(id) = kind {
                    let r = match &taken {
                        Some(Ok(Ok((_, out)))) => Ok(out.clone()),
                        Some(Ok(Err(e))) => Err(e.to_string()),
                        Some(Err(e)) => Err(e.to_string()),
                        None => Err("the import did not finish".to_owned()),
                    };
                    self.scan_import_finished(id, r);
                }
                self.finish_spec(kind, command, label, taken);
            }
            Task::Command {
                command,
                label,
                handle,
            } => match handle.try_take() {
                Some(Ok(Ok(state))) => {
                    self.clear_step_errors();
                    // a command that only reads (a region, a listing) gives
                    // the case back as it got it: no undo step, nothing to
                    // save, nothing in the step history
                    if !same_case(&state, self.undo.current()) {
                        let by = self.change_by(command, false);
                        self.push_state(Arc::new(state), label, by);
                    }
                }
                Some(Ok(Err(CommandError::Cancelled))) => {
                    self.toast(Toast::new(ToastLevel::Info, "job-cancelled"));
                }
                Some(Ok(Err(CommandError::NotAvailable(_)))) => {
                    self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
                }
                Some(Ok(Err(e))) => {
                    self.log_failure("command", Some(command), &label, &e);
                    self.job_error("ws-toast-command-failed", error_text(e));
                }
                Some(Err(e)) => self.job_panic(Some(command), &label, &e),
                None => {}
            },
            Task::Save(save) => self.finish_save(*save),
        }
    }

    /// What command `command` did to the case, for the step history: a run
    /// of a function of the menu (`import`: of the function that imports
    /// scans), a correction by hand of the function it corrects, or neither.
    fn change_by(&self, command: &'static str, import: bool) -> Option<By> {
        if import {
            return self.steps.import().map(By::Run);
        }
        // the aligner of the view is how the case is shown: no result of
        // the case depends on it
        if command == crate::aligner::SHOW || command == crate::aligner::EXACT {
            return Some(By::Look);
        }
        if let Some(f) = self.steps.function(command) {
            // a function that is not a step adds its own result; the whole
            // pipeline recomputes the others
            return Some(if self.steps.is_step(f) || f == leaf::AUTO {
                By::Run(f)
            } else {
                By::Add(f)
            });
        }
        crate::function::corrected(command)
            .and_then(|f| self.steps.function(f))
            .map(By::Hand)
    }

    /// Puts the version `state` of the case on the undo stack as the change
    /// `label`, made by `by`, with its step history.
    fn push_state(&mut self, state: Arc<CaseState>, label: String, by: Option<By>) {
        let module = &self.module;
        self.steps
            .changed(&state, by, |id| module.leaf_done(&state, id));
        self.undo.push(state, label);
    }

    /// A registered command run as a job ended: its state lands as one undo
    /// step and its output is reported, or its failure is shown. The leaf an
    /// agent runs keeps its answer ([`Workspace::take_leaf_answer`]).
    fn finish_spec(
        &mut self,
        kind: SpecKind,
        command: &'static str,
        label: String,
        taken: Option<Result<Result<CommandOutput, CommandError>, JobError>>,
    ) {
        if let SpecKind::Leaf(id) = kind
            && let Some((asked, answer)) = self.agent_leaf.as_mut()
            && *asked == id
            && answer.is_none()
        {
            *answer = Some(match &taken {
                Some(Ok(Ok((_, out)))) => Ok(out.clone()),
                Some(Ok(Err(e))) => Err(e.to_string()),
                Some(Err(e)) => Err(e.to_string()),
                None => Err("the command did not end".to_owned()),
            });
        }
        match taken {
            Some(Ok(Ok((state, out)))) => {
                self.clear_step_errors();
                if !matches!(kind, SpecKind::Leaf(id) if !leaf::changes_case(id)) {
                    let import = matches!(kind, SpecKind::Import | SpecKind::ScanImport(_));
                    let by = self.change_by(command, import);
                    // a leaf that only reads or reports (a listing, a bite
                    // that does not apply) gives the case back as it got it:
                    // no undo step, nothing to save. The run of a function
                    // that leaves no trace in the case (an export) is
                    // recorded all the same: its step is the record.
                    let recorded = match by {
                        Some(By::Add(_)) => true,
                        Some(By::Run(f)) => !self.steps.is_step(f),
                        _ => false,
                    };
                    if recorded || !same_case(&state, self.undo.current()) {
                        self.push_state(Arc::new(state), label, by);
                    }
                }
                match kind {
                    SpecKind::Import | SpecKind::ScanImport(_) => {
                        self.view.refit = true;
                        self.show_import_report(&out);
                    }
                    SpecKind::SetRole => self.show_role_report(&out),
                    SpecKind::RemoveScan => self.show_remove_report(&out),
                    SpecKind::Leaf(id) => self.show_leaf_report(id, &out),
                }
            }
            Some(Ok(Err(CommandError::Cancelled))) => {
                self.toast(Toast::new(ToastLevel::Info, "job-cancelled"));
            }
            Some(Ok(Err(CommandError::NotAvailable(_)))) => {
                self.toast(Toast::new(ToastLevel::Info, NOT_AVAILABLE_KEY));
            }
            Some(Ok(Err(e))) => {
                self.log_failure(kind.source(command), Some(command), &label, &e);
                self.job_error(kind.error_key(), error_text(e));
            }
            Some(Err(e)) => self.job_panic(Some(command), &label, &e),
            None => {}
        }
    }

    /// A save ended. Saved: the file holds that state (what changed since
    /// stays unsaved and is saved by itself later), the user's save says so,
    /// and the exit waiting for it follows once nothing is left to save.
    /// Failed: never silent, the error stays on screen; an exit waiting for
    /// it asks the user (save again, or leave without saving). Then the save
    /// asked for meanwhile starts.
    fn finish_save(&mut self, save: SaveTask) {
        let SaveTask {
            path,
            state,
            manifest,
            then,
            explicit,
            handle,
        } = save;
        let then = then.or(self.leave_after_save.take());
        let (failure, warnings) = match handle.try_take() {
            Some(Ok(Ok(w))) => (None, w),
            Some(Ok(Err(msg))) => (Some(msg), Vec::new()),
            Some(Err(e)) => (Some(e.to_string()), Vec::new()),
            None => (Some(crate::t("ws-save-lost")), Vec::new()),
        };
        match failure {
            None => {
                self.saved = state;
                self.autosave.succeeded();
                self.manifest = Some(manifest);
                self.path = Some(path.clone());
                self.read_only = false;
                self.steps.saved();
                if explicit {
                    self.toast(Toast::new(ToastLevel::Success, "ws-toast-saved"));
                }
                // the file is saved, but the user must know its safety net
                // (the backup, the disk sync) did not work: never silent
                for w in &warnings {
                    tracing::warn!(
                        code = "save-warning",
                        source = "save",
                        module = self.module.id(),
                        "{w}"
                    );
                    let (key, reason) = match w {
                        dc_project::SaveWarning::Backup { reason, .. } => {
                            ("ws-save-warning-backup", reason)
                        }
                        dc_project::SaveWarning::DirSync(reason) => {
                            ("ws-save-warning-sync", reason)
                        }
                    };
                    self.toast(
                        Toast::new(ToastLevel::Warning, "ws-toast-save-warning")
                            .with_detail(format!("{} ({reason})", crate::t(key))),
                    );
                }
                self.actions.push_back(WorkspaceAction::Saved(path));
                match then {
                    // what changed while it saved goes in too, then it leaves
                    Some(exit) if self.is_dirty() => self.save_then(Some(exit)),
                    Some(exit) => self.actions.push_back(exit.action()),
                    None => {}
                }
            }
            Some(detail) => self.save_failed(&state, then, explicit, detail),
        }
        if let Some(q) = self.queued_save.take() {
            self.save_to_then(q.path, q.then, q.explicit);
        }
    }

    /// A save of `state` did not happen (`detail` says why): never silent,
    /// the error stays on screen and the automatic save waits for the next
    /// change; an exit waiting for it asks the user (save again, or leave
    /// without saving).
    fn save_failed(
        &mut self,
        state: &Arc<CaseState>,
        then: Option<Exit>,
        explicit: bool,
        detail: String,
    ) {
        self.autosave.failed(state);
        let key = if explicit {
            "ws-toast-save-failed"
        } else {
            "ws-toast-autosave-failed"
        };
        tracing::error!(
            code = if explicit {
                "save-failed"
            } else {
                "autosave-failed"
            },
            source = "save",
            module = self.module.id(),
            "the project was not saved: {detail}"
        );
        self.job_error(key, detail);
        if let Some(exit) = then {
            self.confirm_exit = Some(exit);
        }
    }

    /// What a leaf's command says when it ends: the sentence of its output,
    /// and for the whole pipeline the summary and the next leaf.
    pub(crate) fn show_leaf_report(&mut self, id: &'static str, out: &Value) {
        // a leaf that lists parts to print opens the print popup with them
        self.open_print_from(out);
        // the aligners it computed: what they warn about, for the inspector
        if (id == leaf::ALIGNERS || id == leaf::AUTO)
            && let Some(staging) = leaf::step_output(out, crate::plan_warnings::STAGING_STEP)
        {
            let state = self.undo.current().clone();
            self.plan_warnings.record_staging(staging, &state);
        }
        // an arch chosen by hand: what it cleared, as a role change says it
        if id == leaf::SET_ARCH {
            self.show_role_report(out);
        }
        // new tooth marks: what they dropped, the arches not numbered, and
        // what the numbering left in doubt on the teeth it found (to check
        // now, it stays until the marks change)
        if id == leaf::FIND_TEETH {
            for toast in crate::manual::marks_toasts(out) {
                self.toast(toast);
            }
            for toast in leaf::step_warning_toasts(out) {
                self.toast_sticky(toast);
            }
        }
        if id == leaf::AUTO {
            self.show_auto_report(out);
        } else if let Some(r) = leaf::report(id, out) {
            if r.sticky {
                self.toast_sticky(r.toast);
            } else {
                self.toast(r.toast);
            }
        }
        // new tooth colours: what they left to redo, and the scan shows them
        if (id == leaf::SEGMENT || id == leaf::SEGMENT_CHECK)
            && let Some(toast) = leaf::outdated_toast(out)
        {
            self.toast(toast);
        }
        if id == leaf::SEGMENT {
            self.view.model = crate::view::Model::Scan;
        }
        // the study models just built are shown: their layer, at the initial
        // position (where they stand for their arch)
        if id == leaf::STUDY_MODELS {
            self.view.layers.study = true;
            self.timeline.current = 0;
            self.timeline.playing = false;
        }
        // what to check before delivery (the doubts the separation left, and
        // a study model that is not a clean solid)
        if id == leaf::AUTO || id == leaf::EXPORT_ALL || id == leaf::STUDY_MODELS {
            for toast in leaf::warning_toasts(out) {
                self.toast_sticky(toast);
            }
        }
    }

    /// The first step of the workflow the case has no result for (the one
    /// the menu marks "Next").
    pub(crate) fn next_step(&self) -> Option<&'static str> {
        crate::menu::next_step(&self.menu, &|id| self.is_done(id))
    }

    /// The pipeline ended well: refresh the viewport, say how many steps ran
    /// and how long they took, and select what comes next.
    fn show_auto_report(&mut self, out: &Value) {
        self.view.refit = true;
        let steps = out["steps"].as_array().map_or(0, Vec::len);
        let time = leaf::duration_text(out["seconds"].as_f64().unwrap_or(0.0));
        let label = |ws: &Self, id: &str| {
            ws.menu
                .find(id)
                .map_or_else(|| id.to_owned(), |i| crate::t(i.label_key))
        };
        let (next, detail) = match self.next_step() {
            Some(next) => {
                let text = dc_i18n::tr_args(
                    "ws-auto-done-next",
                    &[
                        ("steps", &steps.to_string()),
                        ("time", &time),
                        ("next", &label(self, next)),
                    ],
                );
                (next, text)
            }
            None => {
                let text = dc_i18n::tr_args(
                    "ws-auto-done-review",
                    &[
                        ("steps", &steps.to_string()),
                        ("time", &time),
                        ("analysis", &label(self, leaf::CASE_ANALYSIS)),
                        ("export", &label(self, leaf::EXPORT_ALL)),
                    ],
                );
                (leaf::CASE_ANALYSIS, text)
            }
        };
        self.toast_sticky(
            Toast::new(ToastLevel::Success, "ws-toast-auto-done").with_detail(detail),
        );
        if let Some(item) = self.menu.find(next) {
            let id = item.id;
            self.select_item(id);
            self.menu_view.reveal(&self.menu, id);
        }
    }

    fn job_error(&mut self, key: &str, detail: String) {
        // a refused step says why until the next one works or the user
        // selects something else
        self.toasts.push(ShownToast {
            toast: Toast::new(ToastLevel::Error, key).with_detail(detail),
            born: None,
            sticky: false,
            until_next: key == "ws-toast-command-failed",
        });
    }

    fn job_panic(&mut self, command: Option<&str>, label: &str, e: &JobError) {
        tracing::error!(
            code = "job-panicked",
            source = "job",
            command = command.unwrap_or_default(),
            module = self.module.id(),
            "{label}: {e}"
        );
        self.job_error("ws-toast-job-failed", e.to_string());
    }

    /// Logs a failed command for the log file and the anonymous
    /// diagnostics: a failure as an error, refused parameters as a flagged
    /// warning.
    fn log_failure(&self, source: &str, command: Option<&str>, label: &str, e: &CommandError) {
        let command = command.unwrap_or_default();
        let module = self.module.id();
        match e {
            CommandError::Invalid(m) => tracing::warn!(
                diag = true,
                code = "command-invalid",
                source,
                command,
                module,
                "{label}: {m}"
            ),
            e => tracing::error!(
                code = %format!("{source}-failed"),
                source,
                command,
                module,
                "{label}: {e}"
            ),
        }
    }

    pub(crate) fn next_action(&mut self) -> WorkspaceAction {
        self.take_action()
    }
}

/// `a` holds the same case as `b`: the case, the same meshes (a mesh is
/// keyed by the hash of its content) and the same data of every module. Data
/// that cannot be written as JSON counts as changed.
fn same_case(a: &CaseState, b: &CaseState) -> bool {
    a.case == b.case
        && a.blobs.len() == b.blobs.len()
        && a.blobs.iter().all(|(r, _)| b.blobs.contains(r))
        && a.modules.len() == b.modules.len()
        && a.modules.iter().all(|(id, m)| {
            b.modules
                .get(id)
                .is_some_and(|n| match (m.to_json(), n.to_json()) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
        })
}

/// `a` and `b` are the same file: written the same, or both exist and lead
/// to the same place on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// The message of a failed command: the sentence the command wrote, without
/// the "failed:" or "invalid:" the error type puts in front.
fn error_text(e: CommandError) -> String {
    match e {
        CommandError::Failed(m) | CommandError::Invalid(m) => m,
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(level: ToastLevel, sticky: bool) -> ShownToast {
        ShownToast {
            toast: Toast::new(level, "k"),
            born: None,
            sticky,
            until_next: false,
        }
    }

    #[test]
    fn errors_and_sticky_toasts_stay_until_closed() {
        assert!(shown(ToastLevel::Error, false).stays());
        assert!(shown(ToastLevel::Warning, true).stays());
        assert!(!shown(ToastLevel::Warning, false).stays());
        assert!(!shown(ToastLevel::Success, false).stays());
    }

    #[test]
    fn the_bound_never_hides_a_sticky_toast() {
        use ToastLevel::{Info, Warning};
        // plain, sticky, plain, plain, plain, plain: four fit, the sticky one counts
        let list = [
            shown(Info, false),
            shown(Warning, true),
            shown(Info, false),
            shown(Info, false),
            shown(Info, false),
            shown(Info, false),
        ];
        assert_eq!(visible_toasts(&list, 4), [1, 3, 4, 5]);
        // many sticky ones go past the bound, and no plain one is left
        let list: Vec<_> = (0..7)
            .map(|n| shown(Warning, n != 6))
            .chain([shown(Info, false)])
            .collect();
        let v = visible_toasts(&list, 4);
        assert_eq!(v, [0, 1, 2, 3, 4, 5]);
        // below the bound everything is drawn
        assert_eq!(visible_toasts(&list[..2], 4), [0, 1]);
        assert!(visible_toasts(&[], 4).is_empty());
    }

    /// The file opened read-only (damaged or newer) is never written over:
    /// a save to it is refused, and Save as on that same file says so.
    #[test]
    fn save_as_on_the_file_opened_read_only_writes_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("newer.dcprj");
        let project = dc_project::Project::new(
            dc_project::AppInfo {
                version: "0.1.0".into(),
                commit: "test".into(),
            },
            crate::now_rfc3339(),
            CaseState::new(Case::new("A")),
        );
        dc_project::save(&path, &project).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut ws = Workspace::new_headless(
            Box::new(dca_module::AlignersModule::new()),
            Some(project),
            Some(path.clone()),
        );
        ws.set_read_only(true);
        let wait = Duration::from_secs(120);
        let said = |ws: &Workspace| ws.toast_details().last().and_then(|(_, d)| d.clone());
        let answer = |ws: &mut Workspace, p: PathBuf| {
            let (tx, rx) = mpsc::channel();
            tx.send(DialogAnswer::SaveAs(Some(p), None)).unwrap();
            ws.dialog = Some(rx);
            ws.poll();
        };
        // a save to the file itself (an agent, a shortcut of the shell)
        ws.save_to(path.clone());
        assert!(ws.wait_for_jobs(wait));
        assert_eq!(said(&ws), Some(crate::t("ws-save-read-only")));
        // Save as on that file (the extension added as the save adds it)
        answer(&mut ws, dir.path().join("newer"));
        assert!(ws.wait_for_jobs(wait));
        assert_eq!(said(&ws), Some(crate::t("ws-save-as-read-only")));
        assert!(ws.is_read_only());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // Save as another file: written, and that file is not read-only
        let other = dir.path().join("other.dcprj");
        answer(&mut ws, other.clone());
        assert!(ws.wait_for_jobs(wait));
        assert!(other.is_file() && !ws.is_read_only());
        assert_eq!(ws.path(), Some(other.as_path()));
    }
}
