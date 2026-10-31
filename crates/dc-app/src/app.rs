//! Application state, navigation between screens, top bar, status bar,
//! background work (project loading, creation, file dialogs) and toasts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use chrono::{Datelike, Timelike};
use dc_auth_client::{AuthClient, AuthState, MemoryTokenStore, UserInfo};
use dc_i18n::Lang;
use dc_module_api::ModuleRegistry;
use dc_project::LoadOutcome;
use dc_ui::widgets::{self, BadgeKind, Button};
use dc_ui::{fonts, icons, theme, tokens};
use dc_workspace::{Workspace, WorkspaceAction};
use egui::{
    Align, Align2, CornerRadius, Frame, Key, Layout, Margin, Panel, RichText, Sense, Stroke,
    TextureHandle, Ui, vec2,
};

use crate::config::{RecentList, Settings, Store, ThemePref};
use crate::create_job::{CreateJob, NewProjectRequest};
use crate::diagnostics::{GpuInfo, SystemInfo};
use crate::dialogs::{self, PendingDialog, Purpose, Spec};
use crate::modules::{self, ModuleInfo};
use crate::paint;
use crate::screens::new_project::NewProjectState;
use crate::screens::sign_in::SignInState;
use crate::screens::support::SupportState;

/// i18n keys of the weekdays, Monday first.
pub const WEEKDAY_KEYS: [&str; 7] = [
    "weekday-mon",
    "weekday-tue",
    "weekday-wed",
    "weekday-thu",
    "weekday-fri",
    "weekday-sat",
    "weekday-sun",
];

/// i18n keys of the months, January first.
pub const MONTH_KEYS: [&str; 12] = [
    "month-1", "month-2", "month-3", "month-4", "month-5", "month-6", "month-7", "month-8",
    "month-9", "month-10", "month-11", "month-12",
];

/// The screens of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Launcher.
    Home,
    /// Sign-in (browser flow).
    SignIn,
    /// Help & support.
    Support,
    /// New project / module chooser.
    NewProject,
    /// Module workspace (`dc-workspace`).
    Workspace,
}

/// What the app needs from outside.
pub struct AppDeps {
    /// Settings and recent projects.
    pub store: Store,
    /// Sign-in client; `None` when the issuer is invalid.
    pub auth: Option<AuthClient>,
    /// wgpu render state (eframe); `None` in tests (headless workspace).
    pub render_state: Option<egui_wgpu::RenderState>,
    /// Use native file dialogs (off in tests).
    pub native_dialogs: bool,
}

impl AppDeps {
    /// Headless dependencies for tests: files under `root`, in-memory token
    /// store, a browser that never opens, an unreachable issuer.
    pub fn headless(root: &Path) -> Self {
        let auth = AuthClient::with_parts(
            "http://127.0.0.1:9",
            dc_auth_client::DEFAULT_CLIENT_ID,
            Arc::new(MemoryTokenStore::new()),
            Arc::new(|_: &str| false),
        )
        .ok();
        Self {
            store: Store::new(crate::config::Dirs::under(root)),
            auth,
            render_state: None,
            native_dialogs: false,
        }
    }
}

/// A toast message (bottom right, auto-dismissed).
pub(crate) struct Toast {
    pub kind: BadgeKind,
    pub text: String,
    pub until: f64,
}

struct OpenJob {
    path: PathBuf,
    rx: Receiver<Result<(Box<LoadOutcome>, String), String>>,
}

/// The application.
pub struct DentalCraftApp {
    pub(crate) store: Store,
    pub(crate) settings: Settings,
    pub(crate) recent: RecentList,
    pub(crate) auth: Option<AuthClient>,
    pub(crate) auth_state: AuthState,
    pub(crate) render_state: Option<egui_wgpu::RenderState>,
    pub(crate) gpu: Option<GpuInfo>,
    pub(crate) sysinfo: SystemInfo,
    pub(crate) registry: ModuleRegistry,
    pub(crate) module_cards: Vec<ModuleInfo>,
    pub(crate) screen: Screen,
    pub(crate) native_dialogs: bool,
    pub(crate) sign_in: SignInState,
    pub(crate) support: SupportState,
    pub(crate) new_project: NewProjectState,
    pub(crate) show_all_recent: bool,
    pub(crate) show_shortcuts: bool,
    pub(crate) toasts: Vec<Toast>,
    pub(crate) dialog: Option<PendingDialog>,
    pub(crate) create_job: Option<CreateJob>,
    workspace: Option<Workspace>,
    open_job: Option<OpenJob>,
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    ctx: Option<egui::Context>,
}

impl DentalCraftApp {
    /// Builds the app: loads settings and recent projects, applies the
    /// language, starts the silent sign-in restore.
    pub fn new(deps: AppDeps) -> Self {
        crate::register_bundles();
        let settings = deps.store.load_settings();
        if let Err(e) = dc_i18n::set_language(settings.lang()) {
            tracing::warn!("cannot switch language: {e}");
        }
        let recent = deps.store.load_recent();
        let gpu = deps.render_state.as_ref().map(GpuInfo::from_render_state);
        let sysinfo = SystemInfo::collect(gpu.clone(), &deps.store.dirs().logs());
        let registry = modules::registry();
        let module_cards = modules::cards(&registry);
        if let Some(a) = &deps.auth {
            a.try_restore();
        }
        let auth_state = deps
            .auth
            .as_ref()
            .map_or(AuthState::SignedOut, AuthClient::state);
        let new_project = NewProjectState::new(&deps.store.dirs().projects);
        Self {
            store: deps.store,
            settings,
            recent,
            auth: deps.auth,
            auth_state,
            render_state: deps.render_state,
            gpu,
            sysinfo,
            registry,
            module_cards,
            screen: Screen::Home,
            native_dialogs: deps.native_dialogs,
            sign_in: SignInState::default(),
            support: SupportState::default(),
            new_project,
            show_all_recent: false,
            show_shortcuts: false,
            toasts: Vec::new(),
            dialog: None,
            create_job: None,
            workspace: None,
            open_job: None,
            thumbs: HashMap::new(),
            ctx: None,
        }
    }

    /// The current screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Switches screen.
    pub fn go(&mut self, screen: Screen) {
        if screen == Screen::Workspace && self.workspace.is_none() {
            return;
        }
        self.screen = screen;
    }

    /// The installed modules registry.
    pub fn registry(&self) -> &ModuleRegistry {
        &self.registry
    }

    /// The open workspace, if any.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    /// The open workspace, mutable (the app bridge drives it).
    pub fn workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspace.as_mut()
    }

    /// Opens a workspace of `module_id` on a new empty, unsaved case
    /// (replacing the open one without asking); `false` when the module is
    /// not installed.
    pub fn new_workspace(&mut self, module_id: &str) -> bool {
        self.enter_workspace(module_id, None, None, false);
        self.workspace.is_some() && self.screen == Screen::Workspace
    }

    /// `true` while [`DentalCraftApp::open_project`] is loading a file.
    pub fn is_opening(&self) -> bool {
        self.open_job.is_some()
    }

    /// Draws one frame into the root `ui`.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if self.ctx.is_none() {
            theme::install(&ctx, theme::Mode::Dark);
            ctx.set_theme(self.settings.theme.egui());
            if let Some(a) = &self.auth {
                let c = ctx.clone();
                a.on_change(move || c.request_repaint());
            }
            self.ctx = Some(ctx.clone());
        }
        self.poll_background(&ctx);
        self.keyboard(&ctx);

        if self.screen == Screen::Workspace {
            if let Some(ws) = &mut self.workspace {
                match ws.show(ui) {
                    WorkspaceAction::None => {}
                    WorkspaceAction::GoHome | WorkspaceAction::Closed => {
                        self.workspace = None;
                        self.screen = Screen::Home;
                    }
                    WorkspaceAction::Saved(path) => {
                        self.remember(&path, "aligners");
                    }
                }
            } else {
                self.screen = Screen::Home;
            }
        } else if self.screen == Screen::SignIn {
            self.sign_in_ui(ui);
        } else {
            let c = *dc_ui::colors(ui);
            self.top_bar(ui);
            if self.screen == Screen::Home {
                self.status_bar(ui);
            }
            egui::CentralPanel::default()
                .frame(Frame::new().fill(c.bg_canvas))
                .show(ui, |ui| match self.screen {
                    Screen::Support => self.support_ui(ui),
                    Screen::NewProject => self.new_project_ui(ui),
                    _ => self.home_ui(ui),
                });
        }
        self.overlays(&ctx);
    }

    // ------------------------------------------------------------ state helpers

    pub(crate) fn user(&self) -> Option<&UserInfo> {
        match &self.auth_state {
            AuthState::SignedIn(u) => Some(u),
            _ => None,
        }
    }

    pub(crate) fn wake(&self) -> impl Fn() + Send + 'static {
        let ctx = self.ctx.clone();
        move || {
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        }
    }

    pub(crate) fn toast(&mut self, kind: BadgeKind, text: impl Into<String>) {
        let now = self.ctx.as_ref().map_or(0.0, |c| c.input(|i| i.time));
        let secs = if kind == BadgeKind::Danger { 12.0 } else { 6.0 };
        self.toasts.push(Toast {
            kind,
            text: text.into(),
            until: now + secs,
        });
    }

    pub(crate) fn set_language(&mut self, lang: Lang) {
        if let Err(e) = dc_i18n::set_language(lang) {
            tracing::warn!("cannot switch language: {e}");
            return;
        }
        self.settings.language = Some(lang.code().to_owned());
        self.store.save_settings(&self.settings);
    }

    pub(crate) fn set_theme(&mut self, ctx: &egui::Context, pref: ThemePref) {
        self.settings.theme = pref;
        ctx.set_theme(pref.egui());
        self.store.save_settings(&self.settings);
    }

    pub(crate) fn auth_lang() -> dc_auth_client::Lang {
        dc_auth_client::Lang::from_tag(dc_i18n::current().code())
    }

    pub(crate) fn begin_sign_in(&mut self) {
        if let Some(a) = &self.auth {
            self.sign_in.started_at = self.ctx.as_ref().map(|c| c.input(|i| i.time));
            self.sign_in.opened_clock = Some(chrono::Local::now().format("%H:%M").to_string());
            a.begin_sign_in(Self::auth_lang());
            self.auth_state = a.state();
        }
        self.screen = Screen::SignIn;
    }

    pub(crate) fn sign_out(&mut self) {
        if let Some(a) = &self.auth {
            a.sign_out();
            self.auth_state = a.state();
        }
    }

    pub(crate) fn open_dialog(&mut self, purpose: Purpose, spec: Spec) {
        if !self.native_dialogs || self.dialog.is_some() {
            return;
        }
        let wake = self.wake();
        self.dialog = Some(dialogs::open(purpose, spec, wake));
    }

    pub(crate) fn pick_project_to_open(&mut self) {
        let spec = Spec {
            title: t!("dialog-open-project"),
            filter: Some((
                t!("dialog-filter-project"),
                vec![dc_project::FILE_EXTENSION],
            )),
            directory: Some(self.store.dirs().projects.clone()),
            ..Spec::default()
        };
        self.open_dialog(Purpose::OpenProject, spec);
    }

    pub(crate) fn remember(&mut self, path: &Path, module: &str) {
        self.recent.touch(path, module);
        self.store.save_recent(&self.recent);
        self.thumbs.remove(path);
    }

    /// Loads `path` on a worker thread, then opens the workspace.
    pub fn open_project(&mut self, path: PathBuf) {
        if self.open_job.is_some() {
            return;
        }
        tracing::info!("opening {}", path.display());
        let (tx, rx) = channel();
        let p = path.clone();
        let wake = self.wake();
        let spawned = std::thread::Builder::new()
            .name("dc-open-project".into())
            .spawn(move || {
                let result = (|| {
                    let module = modules::instantiate("aligners")
                        .ok_or_else(|| "no module can read this project".to_owned())?;
                    let codecs = dc_workspace::project_codecs(module.as_ref());
                    let outcome = dc_project::load(&p, &codecs).map_err(|e| e.to_string())?;
                    let id = outcome
                        .project
                        .manifest
                        .modules
                        .first()
                        .map_or_else(|| "aligners".to_owned(), |m| m.id.clone());
                    Ok((Box::new(outcome), id))
                })();
                let _ = tx.send(result);
                wake();
            });
        match spawned {
            Ok(_) => self.open_job = Some(OpenJob { path, rx }),
            Err(e) => self.toast(BadgeKind::Danger, e.to_string()),
        }
    }

    fn enter_workspace(
        &mut self,
        module_id: &str,
        project: Option<dc_project::Project>,
        path: Option<PathBuf>,
        read_only: bool,
    ) {
        let Some(module) = modules::instantiate(module_id) else {
            self.toast(
                BadgeKind::Danger,
                t!("error-module-missing", module = module_id),
            );
            return;
        };
        let ws = match &self.render_state {
            Some(rs) => Workspace::new(module, project, path, rs),
            None => Workspace::new_headless(module, project, path),
        };
        let mut ws = ws;
        ws.set_read_only(read_only);
        self.workspace = Some(ws);
        self.screen = Screen::Workspace;
    }

    pub(crate) fn start_create(&mut self, request: NewProjectRequest) {
        let wake = self.wake();
        self.create_job = Some(CreateJob::spawn(request, wake));
    }

    /// Project thumbnail (`thumbnail.png` inside the `.dcprj`), cached.
    pub(crate) fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(path) {
            return t.clone();
        }
        let tex = read_thumbnail(path).map(|img| {
            ctx.load_texture(
                format!("thumb:{}", path.display()),
                img,
                egui::TextureOptions::LINEAR,
            )
        });
        self.thumbs.insert(path.to_path_buf(), tex.clone());
        tex
    }

    // ------------------------------------------------------------ background

    fn poll_background(&mut self, ctx: &egui::Context) {
        if let Some(a) = &self.auth
            && let Some(s) = a.poll()
        {
            if let AuthState::SignedIn(u) = &s {
                tracing::info!("signed in as {}", u.email);
            }
            self.auth_state = s;
        }
        if let Some(d) = &self.dialog
            && let Some(paths) = d.poll()
        {
            let purpose = d.purpose.clone();
            self.dialog = None;
            self.dialog_done(ctx, purpose, paths);
        }
        if let Some(job) = &self.open_job {
            match job.rx.try_recv() {
                Ok(Ok((outcome, module_id))) => {
                    let path = job.path.clone();
                    self.open_job = None;
                    let outcome = *outcome;
                    if outcome.read_only {
                        self.toast(BadgeKind::Warning, t!("open-read-only"));
                    }
                    self.remember(&path, &module_id);
                    self.enter_workspace(
                        &module_id,
                        Some(outcome.project),
                        Some(path),
                        outcome.read_only,
                    );
                }
                Ok(Err(e)) => {
                    let name = job.path.display().to_string();
                    self.open_job = None;
                    tracing::warn!("cannot open {name}: {e}");
                    self.toast(
                        BadgeKind::Danger,
                        t!("open-failed", path = name, reason = e),
                    );
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.open_job = None,
            }
        }
        if let Some(job) = &mut self.create_job
            && let Some(result) = job.poll()
        {
            let req = job.request.clone();
            self.create_job = None;
            match result {
                Ok(project) => {
                    self.remember(&req.path, &req.module_id);
                    self.new_project = NewProjectState::new(&self.store.dirs().projects);
                    self.enter_workspace(&req.module_id, Some(*project), Some(req.path), false);
                }
                Err(e) if e == "cancelled" => {}
                Err(e) => {
                    tracing::warn!("cannot create the project: {e}");
                    self.new_project.error = Some(e);
                }
            }
        }
        self.poll_support();
        let now = ctx.input(|i| i.time);
        self.toasts.retain(|t| t.until > now);
    }

    fn dialog_done(&mut self, ctx: &egui::Context, purpose: Purpose, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match purpose {
            Purpose::OpenProject => self.open_project(paths[0].clone()),
            Purpose::PickScans => self.new_project.add_scans(paths),
            Purpose::NewProjectLocation => {
                self.new_project.location = Some(dc_workspace_extension(paths[0].clone()));
            }
            Purpose::ExportDiagnostics => {
                let path = paths[0].clone();
                match crate::diagnostics::export_zip(&path, &self.sysinfo) {
                    Ok(()) => self.toast(
                        BadgeKind::Success,
                        t!(
                            "support-diagnostics-exported",
                            path = path.display().to_string()
                        ),
                    ),
                    Err(e) => self.toast(BadgeKind::Danger, e.to_string()),
                }
            }
            Purpose::TicketAttachments => self.support.attachments.extend(paths),
        }
        ctx.request_repaint();
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if self.screen == Screen::Workspace {
            return;
        }
        let (new, open, f1, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, Key::N),
                i.consume_key(egui::Modifiers::COMMAND, Key::O),
                i.consume_key(egui::Modifiers::NONE, Key::F1),
                i.key_pressed(Key::Escape),
            )
        });
        if new {
            self.screen = Screen::NewProject;
        }
        if open {
            self.pick_project_to_open();
        }
        if f1 {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if esc {
            if self.show_shortcuts {
                self.show_shortcuts = false;
            } else if self.screen == Screen::SignIn {
                self.sign_in_escape();
            } else if self.screen != Screen::Home && self.create_job.is_none() {
                self.screen = Screen::Home;
            }
        }
    }

    // ------------------------------------------------------------ chrome

    fn top_bar(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        let t = tokens();
        Panel::top("dc-app-topbar")
            .exact_size(t.size.topbar_home)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(c.bg_panel)
                    .inner_margin(Margin::symmetric(24, 0)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        r.left_bottom() - vec2(24.0, 0.5),
                        r.right_bottom() + vec2(24.0, -0.5),
                    ],
                    Stroke::new(1.0, c.border_subtle),
                );
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let logo = paint::logo_tile(ui, &c, vec2(28.0, 32.0));
                    let logo = ui.interact(logo.rect, logo.id.with("home"), Sense::click());
                    logo.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Link, true, t!("nav-home-logo"))
                    });
                    if logo.on_hover_text(t!("nav-home-logo")).clicked() {
                        self.screen = Screen::Home;
                    }
                    ui.label(
                        RichText::new(t!("app-name"))
                            .font(fonts::extrabold(18.0))
                            .color(c.text_primary),
                    );
                    ui.label(
                        RichText::new(env!("CARGO_PKG_VERSION"))
                            .font(fonts::medium(12.0))
                            .color(c.text_secondary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        self.account_chip(ui);
                        widgets::divider_v(ui, 24.0);
                        if Button::ghost(t!("nav-support"))
                            .icon(icons::LIFEBUOY)
                            .show(ui)
                            .clicked()
                        {
                            self.screen = Screen::Support;
                        }
                        self.language_switch(ui);
                    });
                });
            });
    }

    pub(crate) fn language_switch(&mut self, ui: &mut Ui) {
        let mut idx = usize::from(dc_i18n::current() == Lang::It);
        let resp = widgets::segmented(ui, &mut idx, &["EN", "IT"], 28.0)
            .on_hover_text(t!("language-switch"));
        if resp.changed() {
            self.set_language(if idx == 1 { Lang::It } else { Lang::En });
        }
    }

    fn account_menu(&mut self, ui: &mut Ui) {
        let signed_in = self.user().is_some();
        ui.set_min_width(220.0);
        if let Some(u) = self.user() {
            ui.label(RichText::new(u.name.clone()).font(fonts::semibold(13.0)));
            ui.label(
                RichText::new(u.email.clone())
                    .font(fonts::ui(12.0))
                    .color(dc_ui::colors(ui).text_secondary),
            );
            ui.separator();
        }
        ui.label(
            RichText::new(t!("settings-theme"))
                .font(fonts::medium(12.0))
                .color(dc_ui::colors(ui).text_secondary),
        );
        let ctx = ui.ctx().clone();
        for (pref, key) in [
            (ThemePref::Dark, "ui-theme-dark"),
            (ThemePref::Light, "ui-theme-light"),
            (ThemePref::System, "ui-theme-system"),
        ] {
            if ui
                .radio(self.settings.theme == pref, dc_ui::tr(key))
                .clicked()
            {
                self.set_theme(&ctx, pref);
            }
        }
        ui.separator();
        if signed_in {
            if ui
                .button(format!(
                    "{}  {}",
                    icons::ARROW_SQUARE_OUT,
                    t!("account-manage")
                ))
                .clicked()
            {
                self.open_account_page(&ctx);
                ui.close();
            }
            if ui
                .button(format!("{}  {}", icons::SIGN_OUT, t!("account-sign-out")))
                .clicked()
            {
                self.sign_out();
                ui.close();
            }
        } else if ui
            .button(format!("{}  {}", icons::SIGN_IN, t!("account-sign-in")))
            .clicked()
        {
            self.screen = Screen::SignIn;
            ui.close();
        }
    }

    pub(crate) fn open_account_page(&self, ctx: &egui::Context) {
        if let Some(a) = &self.auth {
            // TODO(server): dedicated account page; the issuer root for now.
            ctx.open_url(egui::OpenUrl::new_tab(format!("{}/account", a.issuer())));
        }
    }

    fn account_chip(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        let user = self.user().cloned();
        let resp = match &user {
            Some(u) => {
                let name = if u.name.is_empty() {
                    u.email.clone()
                } else {
                    u.name.clone()
                };
                let status = t!("account-status-signed-in");
                let name_g = ui.painter().layout_no_wrap(
                    name.clone(),
                    fonts::semibold(13.0),
                    c.text_primary,
                );
                let status_g =
                    ui.painter()
                        .layout_no_wrap(status, fonts::ui(11.0), c.text_secondary);
                let text_w = name_g.size().x.max(status_g.size().x + 12.0);
                let (rect, resp) = ui.allocate_exact_size(
                    vec2(32.0 + 10.0 + text_w + 10.0 + 16.0, 40.0),
                    Sense::click(),
                );
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, t!("account-menu"))
                });
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(
                        rect.expand2(vec2(6.0, 0.0)),
                        CornerRadius::same(4),
                        c.bg_hover,
                    );
                }
                paint::avatar(p, &c, rect.left_center() + vec2(16.0, 0.0), &name);
                let x = rect.left() + 42.0;
                p.galley(egui::pos2(x, rect.top() + 3.0), name_g, c.text_primary);
                p.circle_filled(
                    egui::pos2(x + 3.0, rect.top() + 29.0),
                    3.0,
                    c.status_success,
                );
                p.galley(
                    egui::pos2(x + 10.0, rect.top() + 22.0),
                    status_g,
                    c.text_secondary,
                );
                p.text(
                    rect.right_center() - vec2(8.0, 0.0),
                    Align2::CENTER_CENTER,
                    icons::CARET_DOWN,
                    fonts::icon(12.0),
                    c.text_secondary,
                );
                widgets::focus_ring(ui, &resp, rect, CornerRadius::same(4));
                resp
            }
            None => {
                let menu = widgets::icon_button_sized(
                    ui,
                    icons::CARET_DOWN,
                    t!("account-menu"),
                    28.0,
                    true,
                );
                if Button::ghost(t!("account-sign-in"))
                    .icon(icons::USER)
                    .show(ui)
                    .clicked()
                {
                    self.screen = Screen::SignIn;
                }
                menu
            }
        };
        egui::Popup::menu(&resp)
            .width(240.0)
            .show(|ui| self.account_menu(ui));
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let c = *dc_ui::colors(ui);
        let signed_in = self.user().is_some();
        Panel::bottom("dc-app-statusbar")
            .exact_size(tokens().size.statusbar)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::new()
                    .fill(c.bg_panel)
                    .inner_margin(Margin::symmetric(24, 0)),
            )
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().line_segment(
                    [
                        r.left_top() - vec2(24.0, -0.5),
                        r.right_top() + vec2(24.0, 0.5),
                    ],
                    Stroke::new(1.0, c.border_subtle),
                );
                ui.horizontal_centered(|ui| {
                    let (icon, color, text) = if signed_in {
                        (
                            icons::CLOUD_CHECK,
                            c.status_success,
                            t!("status-licence-ok"),
                        )
                    } else {
                        (icons::CLOUD_SLASH, c.text_secondary, t!("status-offline"))
                    };
                    ui.label(RichText::new(icon).font(fonts::icon(14.0)).color(color));
                    ui.label(
                        RichText::new(text)
                            .font(fonts::ui(11.0))
                            .color(c.text_secondary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let gpu = self
                            .gpu
                            .as_ref()
                            .map_or_else(|| t!("status-no-gpu"), GpuInfo::short);
                        ui.label(
                            RichText::new(gpu)
                                .font(fonts::mono(11.0))
                                .color(c.text_secondary),
                        );
                    });
                });
            });
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        let c = *theme::ctx_colors(ctx);
        // project creation progress
        let mut cancel = false;
        if let Some(job) = &self.create_job {
            let text = match &job.last {
                Some(crate::create_job::JobMsg::Reading { index, total, name }) => t!(
                    "create-progress-reading",
                    index = (index + 1).to_string(),
                    total = total.to_string(),
                    name = name.clone()
                ),
                Some(crate::create_job::JobMsg::Saving) => t!("create-progress-saving"),
                _ => t!("create-progress-starting"),
            };
            let fraction = job.fraction();
            egui::Modal::new(egui::Id::unique("dc-create-job")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(t!("create-progress-title"))
                        .font(fonts::style(&tokens().text.title_s))
                        .color(c.text_primary),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(text)
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
                ui.add_space(12.0);
                widgets::progress_bar(ui, fraction, 420.0);
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::ghost(t!("cancel"))
                        .icon(icons::X)
                        .show(ui)
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        }
        if cancel && let Some(job) = &self.create_job {
            job.cancel();
        }
        if let Some(job) = &self.open_job {
            let name = job
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            egui::Modal::new(egui::Id::unique("dc-open-job")).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    widgets::spinner(ui, 20.0);
                    ui.label(RichText::new(t!("open-progress", name = name)).font(fonts::ui(13.0)));
                });
            });
        }
        if self.show_shortcuts {
            let mut open = true;
            egui::Window::new(t!("shortcuts-title"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    egui::Grid::new("dc-shortcuts")
                        .num_columns(2)
                        .spacing(vec2(24.0, 8.0))
                        .show(ui, |ui| {
                            for (keys, what) in [
                                ("Ctrl N", t!("shortcut-new-project")),
                                ("Ctrl O", t!("shortcut-open-project")),
                                ("F1", t!("shortcut-help")),
                                ("Esc", t!("shortcut-back")),
                            ] {
                                ui.label(RichText::new(keys).font(fonts::mono(12.0)));
                                ui.label(RichText::new(what).font(fonts::ui(13.0)));
                                ui.end_row();
                            }
                        });
                });
            self.show_shortcuts = open;
        }
        // toasts, bottom right
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::unique("dc-toasts"))
                .anchor(Align2::RIGHT_BOTTOM, vec2(-24.0, -44.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    for toast in &self.toasts {
                        let tone = toast.kind.color(&c);
                        Frame::new()
                            .fill(c.bg_raised)
                            .stroke(Stroke::new(1.0, c.border_subtle))
                            .corner_radius(CornerRadius::same(8))
                            .shadow(theme::shadow(&tokens().shadow.e1_popover))
                            .inner_margin(Margin {
                                left: 16,
                                right: 16,
                                top: 12,
                                bottom: 12,
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(380.0);
                                let r = ui.max_rect();
                                ui.painter().line_segment(
                                    [
                                        r.left_top() - vec2(16.0, 12.0),
                                        r.left_bottom() - vec2(16.0, -12.0),
                                    ],
                                    Stroke::new(3.0, tone),
                                );
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(toast.kind.default_icon())
                                            .font(fonts::icon(20.0))
                                            .color(tone),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&toast.text).font(fonts::ui(13.0)),
                                        )
                                        .wrap(),
                                    );
                                });
                            });
                        ui.add_space(8.0);
                    }
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    // ------------------------------------------------------------ helpers for screens

    /// Greeting key and date line for the hero.
    pub(crate) fn greeting(&self) -> (String, String) {
        let now = chrono::Local::now();
        let first = self
            .user()
            .map(|u| u.name.split_whitespace().next().unwrap_or("").to_owned())
            .filter(|n| !n.is_empty());
        let h = now.hour();
        let greeting = match (h, first) {
            (5..=11, Some(n)) => t!("home-greeting-morning", name = n),
            (12..=17, Some(n)) => t!("home-greeting-afternoon", name = n),
            (_, Some(n)) => t!("home-greeting-evening", name = n),
            (5..=11, None) => t!("home-greeting-morning-anon"),
            (12..=17, None) => t!("home-greeting-afternoon-anon"),
            (_, None) => t!("home-greeting-evening-anon"),
        };
        let weekday = dc_i18n::tr(WEEKDAY_KEYS[now.weekday().num_days_from_monday() as usize]);
        let month = dc_i18n::tr(MONTH_KEYS[now.month0() as usize]);
        let date = t!(
            "home-date",
            weekday = weekday,
            day = now.day().to_string(),
            month = month
        );
        (greeting, date)
    }
}

fn dc_workspace_extension(path: PathBuf) -> PathBuf {
    let ok = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(dc_project::FILE_EXTENSION));
    if ok {
        path
    } else {
        path.with_extension(dc_project::FILE_EXTENSION)
    }
}

/// Reads `thumbnail.png` from a `.dcprj` (zip) and decodes it.
fn read_thumbnail(path: &Path) -> Option<egui::ColorImage> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    let mut entry = zip.by_name(dc_project::THUMBNAIL_PATH).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

impl eframe::App for DentalCraftApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}
