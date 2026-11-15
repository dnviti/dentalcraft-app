//! Updates in the app (`dc-update`): the check at startup (the "Check for
//! updates at startup" setting turns it off) and from the account menu or
//! the support page, the dialog that offers the newer version, the download
//! with its progress, and the switch to the new version when the app
//! closes.
//!
//! Nothing here blocks the window: the check and the download run on worker
//! threads. A check at startup that fails (offline) says nothing; a check
//! the user asked for says what happened. The new version takes the place
//! of this one when the window closes, after the project is saved
//! ([`DentalCraftApp::finish_update_on_close`]): at once with "Restart now",
//! else whenever the user closes DentalCraft.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use dc_ui::widgets::{self, Button};
use dc_ui::{fonts, icons, theme, tokens};
use dc_update::{Action, Check, InstallKind, Installation, Prepared, Source, UpdateError};
use egui::{RichText, Ui};
use serde_json::{Value, json};

use crate::app::DentalCraftApp;
use crate::config::Dirs;

/// Where the app reads the update information and puts the downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updates {
    /// The manifest address and the key that must sign it.
    pub source: Source,
    /// Where the downloads wait before they are installed.
    pub work_dir: PathBuf,
    /// Start the new version after "Restart now" (off in tests).
    pub relaunch: bool,
}

impl Updates {
    /// The DentalCraft website and the release key; downloads in
    /// `<data>/updates`.
    pub fn standard(dirs: &Dirs) -> Self {
        Self {
            source: Source::standard(),
            work_dir: dirs.data.join("updates"),
            relaunch: true,
        }
    }
}

/// Who asked for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The app at startup: a failure says nothing, a skipped version stays
    /// quiet.
    Startup,
    /// The user (menu, support page): the dialog shows the answer.
    User,
    /// An agent (`app.update.check` / `app.update.apply` on the bridge).
    Agent,
}

/// Where an update stands.
pub(crate) enum Phase {
    /// Nothing asked yet (or the update is installed).
    Idle,
    /// Asking the website.
    Checking {
        origin: Origin,
        rx: Receiver<Result<Check, UpdateError>>,
    },
    /// The running version is the newest.
    UpToDate(Box<Check>),
    /// The website could not be asked.
    CheckFailed(UpdateError),
    /// A newer version exists.
    Available(Box<Check>),
    /// It is downloaded and checked.
    Downloading {
        check: Box<Check>,
        done: Arc<AtomicU64>,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<Prepared, UpdateError>>,
    },
    /// It is ready: installed when the window closes.
    Ready {
        check: Box<Check>,
        prepared: Prepared,
    },
    /// The download or its checks failed: nothing changed.
    Failed {
        check: Box<Check>,
        error: UpdateError,
    },
}

/// The update state of the app.
pub(crate) struct Updater {
    /// `None`: updates are off (tests).
    pub(crate) config: Option<Updates>,
    pub(crate) phase: Phase,
    /// The dialog is on screen.
    pub(crate) open: bool,
    /// The user asked for "Restart now": the window closes once the project
    /// is saved, and the new version starts.
    pub(crate) restart: bool,
    /// Download as soon as the running check finds an update
    /// (`app.update.apply`).
    pub(crate) apply_after_check: bool,
    /// Restart once the update is ready (`app.update.apply` with
    /// `restart`).
    pub(crate) agent_restart: bool,
}

impl Updater {
    pub(crate) fn new(config: Option<Updates>) -> Self {
        Self {
            config,
            phase: Phase::Idle,
            open: false,
            restart: false,
            apply_after_check: false,
            agent_restart: false,
        }
    }

    /// The check the dialog is about, if any.
    fn check(&self) -> Option<&Check> {
        match &self.phase {
            Phase::UpToDate(c) | Phase::Available(c) => Some(c),
            Phase::Downloading { check, .. }
            | Phase::Ready { check, .. }
            | Phase::Failed { check, .. } => Some(check),
            Phase::Idle | Phase::Checking { .. } | Phase::CheckFailed(_) => None,
        }
    }
}

/// What the user can click in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    UpdateNow,
    Later,
    Skip,
    Copy,
    OpenDownloadPage,
    WhatsNew,
    Cancel,
    Hide,
    RestartNow,
    TryAgain,
    Close,
}

/// Megabytes with one decimal.
fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_000_000.0)
}

impl DentalCraftApp {
    /// `true` while the update dialog is on screen.
    pub fn update_dialog_open(&self) -> bool {
        self.updater.open
    }

    /// The newest version the last check found, when it is newer.
    pub fn update_available(&self) -> Option<String> {
        self.updater
            .check()
            .filter(|c| c.update_available())
            .map(|c| c.latest().to_owned())
    }

    /// `true` once a downloaded update waits for the window to close.
    pub fn update_ready(&self) -> bool {
        matches!(self.updater.phase, Phase::Ready { .. })
    }

    /// Shows the dialog for `check` as if a check had just found it (tests,
    /// tools): its version, the notes link, and what the user can do.
    pub fn offer_update(&mut self, check: Check) {
        self.updater.phase = if check.update_available() {
            Phase::Available(Box::new(check))
        } else {
            Phase::UpToDate(Box::new(check))
        };
        self.updater.open = true;
    }

    /// At the first frame: removes what an earlier update left behind,
    /// then checks for a newer version when the setting says so.
    pub(crate) fn start_updates(&mut self) {
        let Some(cfg) = &self.updater.config else {
            return;
        };
        if self.settings.check_updates {
            self.start_check(Origin::Startup);
        } else {
            let work = cfg.work_dir.clone();
            let _ = std::thread::Builder::new()
                .name("dc-update-cleanup".into())
                .spawn(move || dc_update::cleanup(&Installation::detect(), &work));
        }
    }

    /// "Check for updates": asks the website now and shows the answer (or
    /// shows the update already found, downloading or ready).
    pub fn check_for_updates(&mut self) {
        match &mut self.updater.phase {
            Phase::Checking { origin, .. } => *origin = Origin::User,
            Phase::Downloading { .. } | Phase::Ready { .. } => {}
            _ => self.start_check(Origin::User),
        }
        self.updater.open = true;
    }

    /// Turns the check at startup on or off (saved at once).
    pub fn set_check_updates(&mut self, on: bool) {
        self.settings.check_updates = on;
        self.store.save_settings(&self.settings);
    }

    fn start_check(&mut self, origin: Origin) {
        let Some(cfg) = &self.updater.config else {
            self.updater.phase =
                Phase::CheckFailed(UpdateError::Server("updates are off in this app".into()));
            return;
        };
        let (source, work) = (cfg.source.clone(), cfg.work_dir.clone());
        let (tx, rx) = channel();
        let wake = self.wake();
        let spawned = std::thread::Builder::new()
            .name("dc-update-check".into())
            .spawn(move || {
                let installation = Installation::detect();
                if origin == Origin::Startup {
                    dc_update::cleanup(&installation, &work);
                }
                let _ = tx.send(dc_update::check(&source, installation));
                wake();
            });
        self.updater.phase = match spawned {
            Ok(_) => Phase::Checking { origin, rx },
            Err(e) => Phase::CheckFailed(UpdateError::Install(e.to_string())),
        };
    }

    /// "Update now": downloads and checks the update on a worker thread.
    fn start_download(&mut self) {
        let Some(cfg) = &self.updater.config else {
            return;
        };
        let work = cfg.work_dir.clone();
        let phase = std::mem::replace(&mut self.updater.phase, Phase::Idle);
        let check = match phase {
            Phase::Available(c) | Phase::Failed { check: c, .. }
                if matches!(c.action, Action::Install { .. }) =>
            {
                c
            }
            other => {
                self.updater.phase = other;
                return;
            }
        };
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (d, stop, c) = (Arc::clone(&done), Arc::clone(&cancel), (*check).clone());
        let wake = self.wake();
        let spawned = std::thread::Builder::new()
            .name("dc-update-download".into())
            .spawn(move || {
                let r = dc_update::prepare(&c, &work, &|n, _| d.store(n, Ordering::Relaxed), &stop);
                if let Err(e) = &r {
                    tracing::warn!("update {} not prepared: {e}", c.latest());
                }
                let _ = tx.send(r);
                wake();
            });
        self.updater.phase = match spawned {
            Ok(_) => Phase::Downloading {
                check,
                done,
                cancel,
                rx,
            },
            Err(e) => Phase::Failed {
                check,
                error: UpdateError::Install(e.to_string()),
            },
        };
        self.updater.open = true;
    }

    /// Takes the answers of the worker threads (every frame).
    pub(crate) fn poll_update(&mut self) {
        match &self.updater.phase {
            Phase::Checking { origin, rx } => {
                let origin = *origin;
                let r = match rx.try_recv() {
                    Ok(r) => r,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => {
                        Err(UpdateError::Install("the check stopped".into()))
                    }
                };
                self.check_done(origin, r);
            }
            Phase::Downloading { rx, .. } => {
                let r = match rx.try_recv() {
                    Ok(r) => r,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => {
                        Err(UpdateError::Install("the download stopped".into()))
                    }
                };
                let Phase::Downloading { check, .. } =
                    std::mem::replace(&mut self.updater.phase, Phase::Idle)
                else {
                    return;
                };
                self.updater.phase = match r {
                    Ok(prepared) => {
                        tracing::info!("update {} ready", prepared.version);
                        self.updater.open = true;
                        Phase::Ready { check, prepared }
                    }
                    Err(UpdateError::Cancelled) => {
                        self.updater.open = false;
                        Phase::Available(check)
                    }
                    Err(error) => {
                        self.updater.open = true;
                        Phase::Failed { check, error }
                    }
                };
            }
            _ => {}
        }
    }

    fn check_done(&mut self, origin: Origin, r: Result<Check, UpdateError>) {
        match r {
            Ok(check) if check.update_available() => {
                tracing::info!(
                    "DentalCraft {} is available ({} installation: {})",
                    check.latest(),
                    check.installation.kind.name(),
                    check.action.name()
                );
                let skipped = self.settings.skipped_update.as_deref() == Some(check.latest());
                self.updater.open = match origin {
                    Origin::Startup => !skipped,
                    Origin::User => true,
                    Origin::Agent => self.updater.open,
                };
                self.updater.phase = Phase::Available(Box::new(check));
                if std::mem::take(&mut self.updater.apply_after_check) {
                    self.start_download();
                }
            }
            Ok(check) => {
                tracing::info!("DentalCraft {} is the newest version", check.current);
                self.updater.open = origin == Origin::User;
                self.updater.apply_after_check = false;
                self.updater.phase = Phase::UpToDate(Box::new(check));
            }
            Err(e) => {
                tracing::info!("update check failed: {e}");
                self.updater.open = origin == Origin::User;
                self.updater.apply_after_check = false;
                self.updater.phase = Phase::CheckFailed(e);
            }
        }
    }

    /// The window closes: a downloaded update takes the place of this
    /// version now, and starts with "Restart now". `false` when the window
    /// must stay open: the restart the user is waiting for failed (the
    /// dialog says why).
    pub(crate) fn finish_update_on_close(&mut self) -> bool {
        if !matches!(self.updater.phase, Phase::Ready { .. }) {
            return true;
        }
        let Phase::Ready { check, prepared } =
            std::mem::replace(&mut self.updater.phase, Phase::Idle)
        else {
            return true;
        };
        let restart = self.updater.restart;
        let relaunch = restart && self.updater.config.as_ref().is_some_and(|c| c.relaunch);
        match prepared.finish(relaunch) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!("update {} not installed: {error}", check.latest());
                if !restart {
                    // installed at close: the next start offers it again
                    return true;
                }
                self.updater.restart = false;
                self.updater.open = true;
                self.updater.phase = Phase::Failed { check, error };
                false
            }
        }
    }

    // ------------------------------------------------------------ the bridge

    /// `app.update.check` from an agent: a check without the dialog.
    pub(crate) fn agent_update_check(&mut self) -> Result<(), String> {
        if self.updater.config.is_none() {
            return Err("updates are off in this app".into());
        }
        if !matches!(
            self.updater.phase,
            Phase::Checking { .. } | Phase::Downloading { .. } | Phase::Ready { .. }
        ) {
            self.start_check(Origin::Agent);
        }
        Ok(())
    }

    /// The answer of `app.update.check`, once the check is done.
    pub(crate) fn agent_update_check_poll(&self) -> Option<Result<Value, String>> {
        match &self.updater.phase {
            Phase::Checking { .. } => None,
            Phase::CheckFailed(e) => Some(Err(e.to_string())),
            Phase::Idle => Some(Err("no update check ran".into())),
            _ => self.updater.check().map(|c| Ok(c.to_json())),
        }
    }

    /// `app.update.apply` from an agent: checks if needed, then downloads
    /// (the dialog shows the progress); with `restart` the app restarts
    /// into the new version once it is ready.
    pub(crate) fn agent_update_apply(&mut self, restart: bool) -> Result<(), String> {
        if self.updater.config.is_none() {
            return Err("updates are off in this app".into());
        }
        self.updater.agent_restart = restart;
        match &self.updater.phase {
            Phase::Available(_) | Phase::Failed { .. } => self.start_download(),
            Phase::Checking { .. } => self.updater.apply_after_check = true,
            Phase::Downloading { .. } | Phase::Ready { .. } => {}
            Phase::Idle | Phase::UpToDate(_) | Phase::CheckFailed(_) => {
                self.updater.apply_after_check = true;
                self.start_check(Origin::Agent);
            }
        }
        Ok(())
    }

    /// The answer of `app.update.apply`: when the update is ready (and the
    /// restart started, if asked), or when there is nothing to install.
    pub(crate) fn agent_update_apply_poll(&mut self) -> Option<Result<Value, String>> {
        let out = match &self.updater.phase {
            Phase::Checking { .. } | Phase::Downloading { .. } => return None,
            Phase::CheckFailed(e) | Phase::Failed { error: e, .. } => {
                return Some(Err(e.to_string()));
            }
            Phase::Idle => return Some(Err("no update check ran".into())),
            Phase::UpToDate(c) | Phase::Available(c) => {
                let mut v = c.to_json();
                v["updated"] = json!(false);
                v
            }
            Phase::Ready { check, .. } => {
                let mut v = check.to_json();
                v["updated"] = json!(true);
                v["installed"] = json!(if self.updater.agent_restart {
                    "now: the app saves the project, closes and opens the new version"
                } else {
                    "when the app closes"
                });
                v
            }
        };
        if self.update_ready() && std::mem::take(&mut self.updater.agent_restart) {
            self.restart_into_update();
        }
        Some(Ok(out))
    }

    // ------------------------------------------------------------ the dialog

    /// Draws the update dialog when it is open.
    pub(crate) fn update_dialog(&mut self, ctx: &egui::Context) {
        if !self.updater.open {
            return;
        }
        let c = *theme::ctx_colors(ctx);
        let width = 540.0_f32.min(ctx.content_rect().width() - 80.0).max(320.0);
        let mut click = None;
        let lang = dc_i18n::current();
        let resp = egui::Modal::new(egui::Id::unique("dc-update")).show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 8.0;
            let title = |ui: &mut Ui, icon: &str, text: String, color: egui::Color32| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).font(fonts::icon(22.0)).color(color));
                    ui.label(
                        RichText::new(text)
                            .font(fonts::style(&tokens().text.title_s))
                            .color(c.text_primary),
                    );
                });
            };
            let body = |ui: &mut Ui, text: String| {
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(fonts::ui(13.0))
                            .color(c.text_secondary),
                    )
                    .wrap(),
                );
            };
            let mut buttons: Vec<(Click, Button)> = Vec::new();
            match &self.updater.phase {
                Phase::Idle => {
                    buttons.push((Click::Close, Button::ghost(t!("update-close"))));
                }
                Phase::Checking { .. } => {
                    title(
                        ui,
                        icons::ARROWS_CLOCKWISE,
                        t!("update-checking-title"),
                        c.accent_amber_text,
                    );
                    ui.horizontal(|ui| {
                        widgets::spinner(ui, 18.0);
                        body(ui, t!("update-checking-body"));
                    });
                    buttons.push((Click::Close, Button::ghost(t!("update-close"))));
                }
                Phase::UpToDate(check) => {
                    title(
                        ui,
                        icons::CHECK_CIRCLE,
                        t!("update-latest-title"),
                        c.status_success,
                    );
                    body(
                        ui,
                        t!("update-latest-body", version = check.current.clone()),
                    );
                    buttons.push((Click::Close, Button::primary(t!("update-close"))));
                }
                Phase::CheckFailed(e) => {
                    title(
                        ui,
                        icons::WARNING_CIRCLE,
                        t!("update-check-failed-title"),
                        c.status_warning,
                    );
                    body(ui, e.message());
                    buttons.push((Click::TryAgain, Button::primary(t!("update-try-again"))));
                    buttons.push((Click::Close, Button::ghost(t!("update-close"))));
                }
                Phase::Available(check) => {
                    title(
                        ui,
                        icons::ARROW_CIRCLE_UP,
                        t!("update-available-title"),
                        c.accent_amber_text,
                    );
                    body(
                        ui,
                        t!(
                            "update-available-body",
                            latest = check.latest().to_owned(),
                            current = check.current.clone()
                        ),
                    );
                    if widgets::link(ui, t!("update-whats-new")).clicked() {
                        click = Some(Click::WhatsNew);
                    }
                    ui.add_space(4.0);
                    match &check.action {
                        Action::Install { .. } => {
                            body(ui, t!("update-install-note"));
                            buttons.push((
                                Click::UpdateNow,
                                Button::primary(t!("update-now")).icon(icons::DOWNLOAD_SIMPLE),
                            ));
                        }
                        Action::PackageManager { command } => {
                            body(
                                ui,
                                if check.installation.kind == InstallKind::Flatpak {
                                    t!("update-flatpak-body")
                                } else {
                                    t!("update-package-body")
                                },
                            );
                            widgets::sunken_frame(ui).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(*command)
                                            .font(fonts::mono(13.0))
                                            .color(c.text_primary),
                                    )
                                    .selectable(true)
                                    .wrap(),
                                );
                            });
                            buttons.push((
                                Click::Copy,
                                Button::primary(t!("update-copy-command")).icon(icons::COPY),
                            ));
                        }
                        Action::Manual | Action::None => {
                            body(ui, t!("update-manual-body"));
                            buttons.push((
                                Click::OpenDownloadPage,
                                Button::primary(t!("update-open-download"))
                                    .icon(icons::ARROW_SQUARE_OUT),
                            ));
                        }
                    }
                    buttons.push((Click::Later, Button::secondary(t!("update-later"))));
                    buttons.push((Click::Skip, Button::ghost(t!("update-skip"))));
                }
                Phase::Downloading { check, done, .. } => {
                    title(
                        ui,
                        icons::CLOUD_ARROW_DOWN,
                        t!(
                            "update-downloading-title",
                            version = check.latest().to_owned()
                        ),
                        c.accent_amber_text,
                    );
                    let total = match &check.action {
                        Action::Install { asset, .. } => asset.size,
                        _ => 0,
                    };
                    let done = done.load(Ordering::Relaxed);
                    let fraction = (total > 0).then(|| done as f32 / total as f32);
                    ui.add_space(4.0);
                    widgets::progress_bar(ui, fraction, width);
                    body(
                        ui,
                        t!(
                            "update-downloading-body",
                            done = mb(done),
                            total = mb(total)
                        ),
                    );
                    buttons.push((
                        Click::Cancel,
                        Button::secondary(t!("cancel")).icon(icons::X),
                    ));
                    buttons.push((Click::Hide, Button::ghost(t!("update-hide"))));
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
                Phase::Ready { check, prepared } => {
                    title(
                        ui,
                        icons::SEAL_CHECK,
                        t!("update-ready-title", version = check.latest().to_owned()),
                        c.status_success,
                    );
                    body(
                        ui,
                        if prepared.runs_installer() {
                            t!("update-ready-installer-body")
                        } else {
                            t!("update-ready-body")
                        },
                    );
                    body(ui, t!("update-ready-later"));
                    buttons.push((
                        Click::RestartNow,
                        Button::primary(t!("update-restart-now")).icon(icons::ARROWS_CLOCKWISE),
                    ));
                    buttons.push((Click::Later, Button::ghost(t!("update-later"))));
                }
                Phase::Failed { error, .. } => {
                    title(
                        ui,
                        icons::WARNING_CIRCLE,
                        t!("update-failed-title"),
                        c.status_danger,
                    );
                    body(ui, error.message());
                    if let Some(detail) = error.detail() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(detail)
                                    .font(fonts::mono(11.0))
                                    .color(c.text_secondary),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    }
                    if error.retryable() {
                        buttons.push((Click::TryAgain, Button::primary(t!("update-try-again"))));
                    }
                    buttons.push((
                        Click::OpenDownloadPage,
                        Button::secondary(t!("update-open-download")).icon(icons::ARROW_SQUARE_OUT),
                    ));
                    buttons.push((Click::Close, Button::ghost(t!("update-close"))));
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (what, button) in buttons {
                    if button.show(ui).clicked() {
                        click = Some(what);
                    }
                }
            });
        });
        if resp.should_close() && click.is_none() {
            click = Some(match self.updater.phase {
                Phase::Downloading { .. } => Click::Hide,
                Phase::Available(_) | Phase::Ready { .. } => Click::Later,
                _ => Click::Close,
            });
        }
        if let Some(what) = click {
            self.update_clicked(ctx, what, lang);
        }
    }

    fn update_clicked(&mut self, ctx: &egui::Context, what: Click, lang: dc_i18n::Lang) {
        match what {
            Click::UpdateNow | Click::TryAgain
                if matches!(self.updater.phase, Phase::Failed { .. }) =>
            {
                let retry_download = matches!(
                    &self.updater.phase,
                    Phase::Failed { check, .. } if matches!(check.action, Action::Install { .. })
                );
                if retry_download {
                    self.start_download();
                } else {
                    self.start_check(Origin::User);
                }
            }
            Click::UpdateNow => self.start_download(),
            Click::TryAgain => self.start_check(Origin::User),
            Click::Later | Click::Hide | Click::Close => {
                self.updater.open = false;
                if matches!(
                    self.updater.phase,
                    Phase::UpToDate(_) | Phase::CheckFailed(_) | Phase::Failed { .. }
                ) {
                    self.updater.phase = Phase::Idle;
                }
                if let Phase::Checking { origin, .. } = &mut self.updater.phase {
                    // the answer comes quietly, like the one at startup
                    *origin = Origin::Startup;
                }
            }
            Click::Skip => {
                if let Some(v) = self.update_available() {
                    self.settings.skipped_update = Some(v);
                    self.store.save_settings(&self.settings);
                }
                self.updater.open = false;
            }
            Click::Copy => {
                if let Some(Action::PackageManager { command }) =
                    self.updater.check().map(|c| c.action.clone())
                {
                    ctx.copy_text(command.to_owned());
                    self.toast(widgets::BadgeKind::Success, t!("copied"));
                }
            }
            Click::OpenDownloadPage => {
                ctx.open_url(egui::OpenUrl::new_tab(dc_update::download_page(
                    lang.code(),
                )));
            }
            Click::WhatsNew => {
                if let Some(url) = self.updater.check().map(|c| c.notes_url().to_owned()) {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
            }
            Click::Cancel => {
                if let Phase::Downloading { cancel, .. } = &self.updater.phase {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Click::RestartNow => self.restart_into_update(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_in_megabytes() {
        assert_eq!(mb(0), "0.0");
        assert_eq!(mb(48_260_000), "48.3");
        assert_eq!(mb(999_999), "1.0");
    }
}
