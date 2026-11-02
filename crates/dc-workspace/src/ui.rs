//! Layout of the workspace: top bar, left menu, inspector, timeline, status
//! bar, dialogs and keyboard shortcuts.

use std::path::PathBuf;

use dc_case::{RoleSource, ScanRole};
use dc_module_api::{MenuItem, is_import_scans_command};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::tokens::tokens;
use dc_ui::widgets::{self, Button};
use egui::{
    Align, Align2, CentralPanel, CornerRadius, Frame, Id, Key, KeyboardShortcut, Layout, Margin,
    Modal, Modifiers, Panel, RichText, Role, Sense, Stroke, Ui, WidgetInfo, vec2,
};

use crate::menu::{self, MenuCtx, MenuEvent};
use crate::t;
use crate::timeline::{self, TimelineModel};
use crate::workspace::{Exit, Workspace, WorkspaceAction};

impl Workspace {
    /// Draws the workspace for one frame in the root `Ui` of `ctx` and
    /// returns what the shell must do. Use [`Workspace::show`] instead when
    /// the caller already holds the root `Ui` (e.g. `eframe::App::ui`).
    pub fn ui(&mut self, ctx: &egui::Context) -> WorkspaceAction {
        ctx.root_ui(|ui| self.show(ui))
    }

    /// Draws the workspace filling `ui` and returns what the shell must do.
    pub fn show(&mut self, ui: &mut Ui) -> WorkspaceAction {
        self.egui_ctx = Some(ui.ctx().clone());
        self.poll();
        // files dropped on the window go to the import (not under a dialog)
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() && self.confirm_exit.is_none() && !self.show_help {
            self.import_dropped(dropped);
        }
        if self.confirm_exit.is_none() && !self.show_help {
            self.keyboard(ui);
        }
        let c = *colors(ui);
        let sz = tokens().size;

        Panel::top("dc-workspace-topbar")
            .exact_size(sz.topbar_workspace)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(c.bg_panel)
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| self.top_bar(ui));
        Panel::bottom("dc-workspace-status")
            .exact_size(sz.statusbar)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(c.bg_panel)
                    .inner_margin(Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| self.status_bar(ui));
        Panel::left("dc-workspace-menu")
            .exact_size(sz.left_menu_width)
            .resizable(false)
            .frame(Frame::new().fill(c.bg_panel))
            .show(ui, |ui| self.left_panel(ui));
        Panel::right("dc-workspace-inspector")
            .exact_size(sz.inspector_width)
            .resizable(false)
            .frame(Frame::new().fill(c.bg_panel).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.inspector_panel(ui));
        Panel::bottom("dc-workspace-timeline")
            .exact_size(sz.timeline_height)
            .resizable(false)
            .frame(Frame::new().fill(c.bg_panel).inner_margin(Margin {
                left: 16,
                right: 16,
                top: 8,
                bottom: 10,
            }))
            .show(ui, |ui| {
                let model = TimelineModel::from_state(self.undo.current());
                timeline::show(ui, &model, &mut self.timeline);
            });
        CentralPanel::no_frame().show(ui, |ui| self.center_ui(ui));

        self.dialogs(ui);
        if !self.running.is_empty() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.next_action()
    }

    fn keyboard(&mut self, ui: &Ui) {
        self.sequence_keys(ui);
        let consume = |mods: Modifiers, key: Key| {
            ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key)))
        };
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if consume(cmd_shift, Key::Z) || consume(Modifiers::COMMAND, Key::Y) {
            self.redo();
        }
        if consume(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if consume(cmd_shift, Key::S) {
            self.save_as();
        }
        if consume(Modifiers::COMMAND, Key::S) {
            self.save();
        }
        if consume(Modifiers::COMMAND, Key::W) {
            self.request_exit(Exit::Close);
        }
        if consume(Modifiers::NONE, Key::F1) {
            self.show_help = true;
        }
        for (id, sc) in self.shortcuts.clone() {
            if crate::keys::is_plain(&sc) && ui.ctx().egui_wants_keyboard_input() {
                continue;
            }
            if ui.input_mut(|i| i.consume_shortcut(&sc)) {
                self.activate(id);
            }
        }
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if consume(Modifiers::NONE, Key::Escape) {
            if std::mem::take(&mut self.menu_view.filter_focused) {
                // Escape in the filter field: clear it
                self.menu_view.filter.clear();
            } else {
                self.selected = None;
            }
        }
        if consume(Modifiers::NONE, Key::Slash) {
            self.menu_view.focus_filter = true;
        }
        let last = TimelineModel::from_state(self.undo.current()).last();
        self.timeline.keys(ui, last);
    }

    // ------------------------------------------------------------ top bar

    fn top_bar(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            // logo tile: back to Home
            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 32.0), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, t("ws-home")));
            let p = ui.painter();
            p.rect_filled(r, widgets::shade_tab_radius(10.0, 3.0), c.accent_amber);
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                ic::TOOTH,
                fonts::icon(18.0),
                c.accent_amber_on,
            );
            widgets::focus_ring(ui, &resp, r, widgets::shade_tab_radius(10.0, 3.0));
            if resp.on_hover_text(t("ws-home")).clicked() {
                self.request_exit(Exit::Home);
            }

            // module chip
            let code: String = self
                .module
                .id()
                .chars()
                .take(2)
                .collect::<String>()
                .to_uppercase();
            Frame::new()
                .fill(c.bg_sunken)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            RichText::new(code)
                                .font(fonts::mono_bold(12.0))
                                .color(c.accent_amber_text),
                        );
                        ui.label(
                            RichText::new(t(self.module.name_key()))
                                .font(fonts::semibold(13.0))
                                .color(c.text_primary),
                        );
                    });
                });
            ui.label(
                RichText::new("/")
                    .font(fonts::ui(13.0))
                    .color(c.text_disabled),
            );
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.label(
                    RichText::new(self.display_name())
                        .font(fonts::semibold(13.0))
                        .color(c.text_primary),
                );
                ui.label(
                    RichText::new(format!(".{}", dc_project::FILE_EXTENSION))
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
            });
            if self.is_dirty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    widgets::status_dot(ui, c.accent_amber);
                    ui.label(
                        RichText::new(t("ws-unsaved"))
                            .font(fonts::ui(12.0))
                            .color(c.text_secondary),
                    );
                });
            }
            if self.read_only {
                widgets::badge(ui, widgets::BadgeKind::Warning, t("ws-read-only"));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // account avatar
                let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, t("ws-account")));
                ui.painter().circle(
                    r.center(),
                    14.0,
                    c.bg_raised,
                    Stroke::new(2.0, c.select_blue),
                );
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    ic::USER,
                    fonts::icon(16.0),
                    c.text_primary,
                );
                if Button::ghost(t("ws-help"))
                    .icon(ic::QUESTION)
                    .show(ui)
                    .clicked()
                {
                    self.show_help = true;
                }
                widgets::divider_v(ui, 24.0);
                let saving = self.running.iter().any(|r| r.name_key == "ws-job-save");
                if Button::primary(t("save"))
                    .icon(ic::FLOPPY_DISK)
                    .enabled(!saving)
                    .show(ui)
                    .clicked()
                {
                    self.save();
                }
                let can_redo = self.undo.can_redo() && !self.is_busy();
                if Button::ghost(t("redo"))
                    .icon(ic::ARROW_CLOCKWISE)
                    .enabled(can_redo)
                    .show(ui)
                    .clicked()
                {
                    self.redo();
                }
                let can_undo = self.undo.can_undo() && !self.is_busy();
                if Button::ghost(t("undo"))
                    .icon(ic::ARROW_COUNTER_CLOCKWISE)
                    .enabled(can_undo)
                    .show(ui)
                    .clicked()
                {
                    self.undo();
                }
            });
        });
    }

    // ------------------------------------------------------------ left menu

    fn left_panel(&mut self, ui: &mut Ui) {
        let tree = self.menu.clone();
        let done_ids: std::collections::HashSet<&'static str> = tree
            .command_ids()
            .into_iter()
            .filter(|id| self.is_done(id))
            .collect();
        let done = |id: &str| done_ids.contains(id);
        let mc = MenuCtx {
            tree: &tree,
            selected: self.selected,
            done: &done,
            shortcuts: &self.shortcut_text,
        };
        let mut view = std::mem::take(&mut self.menu_view);
        let events = menu::show(ui, &mut view, &mc);
        self.menu_view = view;
        for e in events {
            match e {
                MenuEvent::Activate(_, cmd) => self.activate(cmd),
                MenuEvent::Select(id) => self.select_item(id),
            }
        }
    }

    // ------------------------------------------------------------ inspector

    fn inspector_panel(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        let item: Option<MenuItem> = self.selected.and_then(|id| self.menu.find(id).cloned());
        let tooltip = item
            .as_ref()
            .and_then(|it| it.command_id())
            .and_then(|cmd| self.commands.iter().find(|d| d.id == cmd))
            .and_then(|d| d.tooltip_key);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let icon = item.as_ref().and_then(|i| i.icon).or_else(|| {
                self.selected
                    .and_then(|id| self.menu.path_to(id))
                    .and_then(|p| self.menu.find(p[0]).and_then(|g| g.icon))
            });
            ui.label(
                RichText::new(icon.map_or(ic::TOOTH, crate::icons::glyph))
                    .font(fonts::icon(20.0))
                    .color(c.select_blue),
            );
            let crumbs = match self.selected.and_then(|id| self.menu.path_to(id)) {
                Some(path) => path
                    .iter()
                    .filter_map(|id| self.menu.find(id))
                    .map(|i| t(i.label_key))
                    .collect::<Vec<_>>()
                    .join("  ›  "),
                None => t(self.module.name_key()),
            };
            ui.add(
                egui::Label::new(
                    RichText::new(crumbs)
                        .font(fonts::medium(12.0))
                        .color(c.text_secondary),
                )
                .truncate(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = tooltip.map_or_else(|| t("ws-inspector-help"), t);
                if widgets::icon_button(ui, ic::QUESTION, tip).clicked() {
                    self.show_help = true;
                }
            });
        });
        ui.add_space(8.0);
        widgets::hairline(ui);
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("dc-workspace-inspector-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.module_ctx_run(|m, ctx| m.inspector(ui, ctx));
                self.leaf_inspector(ui, item.as_ref());
                if item
                    .as_ref()
                    .and_then(|it| it.command_id())
                    .is_some_and(is_import_scans_command)
                {
                    self.import_inspector(ui);
                }
            });
    }

    /// The part of the inspector the workspace adds under the module's text
    /// when "Import scans" is selected: the role of the next import and the
    /// scans of the case, each with how its role was decided and a control
    /// to change it.
    fn import_inspector(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        ui.add_space(12.0);
        widgets::hairline(ui);
        ui.add_space(12.0);

        let heading = |ui: &mut Ui, key: &str| {
            ui.label(
                RichText::new(t(key))
                    .font(fonts::semibold(13.0))
                    .color(c.text_primary),
            );
        };
        let caption = |ui: &mut Ui, text: String| {
            ui.label(
                RichText::new(text)
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        };

        // role of the next import
        heading(ui, "ws-import-role-title");
        caption(ui, t("ws-import-role-hint"));
        ui.add_space(6.0);
        let choices = [
            None,
            Some(ScanRole::Upper),
            Some(ScanRole::Lower),
            Some(ScanRole::Bite),
        ];
        let mut idx = choices
            .iter()
            .position(|r| *r == self.import_role)
            .unwrap_or(0);
        let labels = [
            t("ws-role-auto"),
            role_label(ScanRole::Upper),
            role_label(ScanRole::Lower),
            role_label(ScanRole::Bite),
        ];
        if widgets::segmented_fill(ui, &mut idx, &labels, 32.0).changed() {
            self.import_role = choices[idx];
        }
        ui.add_space(16.0);

        // the scans of the case
        heading(ui, "ws-import-scans-title");
        ui.add_space(6.0);
        let scans: Vec<_> = self
            .undo
            .current()
            .case
            .scans
            .iter()
            .map(|s| (s.id, s.source_name.clone(), s.role, s.role_source))
            .collect();
        if scans.is_empty() {
            caption(ui, t("ws-import-no-scans"));
            return;
        }
        let busy = self.is_busy();
        let mut change = None;
        for (id, name, role, source) in scans {
            Frame::new()
                .fill(c.bg_raised)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(ic::CUBE)
                                .font(fonts::icon(18.0))
                                .color(c.text_secondary),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(&name)
                                    .font(fonts::semibold(13.0))
                                    .color(c.text_primary),
                            )
                            .truncate(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(role_label(role))
                                    .font(fonts::mono(12.0))
                                    .color(c.text_secondary),
                            );
                        });
                    });
                    ui.horizontal(|ui| {
                        let (kind, icon, key) = match source {
                            RoleSource::Explicit => (
                                widgets::BadgeKind::Neutral,
                                ic::CHECK,
                                "ws-role-source-explicit",
                            ),
                            RoleSource::FileName => (
                                widgets::BadgeKind::Neutral,
                                ic::FILE,
                                "ws-role-source-file-name",
                            ),
                            RoleSource::Detected => (
                                widgets::BadgeKind::Info,
                                ic::SPARKLE,
                                "ws-role-source-detected",
                            ),
                            RoleSource::DetectedUnsure => (
                                widgets::BadgeKind::Warning,
                                ic::WARNING,
                                "ws-role-source-detected-unsure",
                            ),
                            RoleSource::Provisional => (
                                widgets::BadgeKind::Warning,
                                ic::WARNING,
                                "ws-role-source-provisional",
                            ),
                        };
                        widgets::badge_icon(ui, kind, Some(icon), t(key));
                        // a guess, or a recognition worth a look: the same set-role
                        // with the role it has confirms it
                        if matches!(source, RoleSource::Provisional | RoleSource::DetectedUnsure)
                            && Button::ghost(t("ws-import-confirm-role"))
                                .enabled(!busy)
                                .show(ui)
                                .clicked()
                        {
                            change = Some((id, role, role));
                        }
                    });
                    match source {
                        RoleSource::Provisional => caption(ui, t("ws-import-provisional-hint")),
                        RoleSource::DetectedUnsure => caption(ui, t("ws-import-unsure-hint")),
                        _ => {}
                    }
                    let roles = [ScanRole::Upper, ScanRole::Lower, ScanRole::Bite];
                    let mut idx = roles.iter().position(|r| *r == role).unwrap_or(0);
                    let labels = roles.map(role_label);
                    ui.add_enabled_ui(!busy, |ui| {
                        if widgets::segmented_fill(ui, &mut idx, &labels, 28.0).changed() {
                            change = Some((id, role, roles[idx]));
                        }
                    });
                });
            ui.add_space(6.0);
        }
        if let Some((id, from, to)) = change {
            self.set_scan_role(id, from, to);
        }
    }

    // ------------------------------------------------------------ status bar

    fn status_bar(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.label(
                RichText::new(t("ws-hint-default"))
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mono = |s: String| {
                    RichText::new(s)
                        .font(fonts::mono(11.0))
                        .color(c.text_secondary)
                };
                ui.label(mono(format_bytes(self.undo.memory_bytes())));
                let dt = ui.input(|i| i.stable_dt).max(1e-3);
                ui.label(mono(format!("{:.0} fps", 1.0 / dt)));
                ui.label(mono(t("ws-numbering")));
                ui.label(mono(t("ws-units")));
                let mut cancel = None;
                if let Some((k, job)) = self.running.iter().enumerate().next() {
                    widgets::divider_v(ui, 16.0);
                    let prog = job.progress();
                    let cancelling = job.cancelling();
                    if job.cancellable()
                        && !cancelling
                        && widgets::icon_button_sized(ui, ic::X, t("ws-cancel-job"), 20.0, true)
                            .clicked()
                    {
                        cancel = Some(k);
                    }
                    ui.label(mono(format!("{:.0} %", prog.fraction * 100.0)));
                    widgets::progress_bar(ui, Some(prog.fraction), 120.0);
                    // the job, then the step it is on when the command says
                    let name = t(job.name_key);
                    let label = match prog.message_key.as_deref().map(t) {
                        // asked to stop: it ends at the end of the step it is in
                        _ if cancelling => format!("{name} \u{b7} {}", t("ws-job-stopping")),
                        Some(step) if step != name => format!("{name} \u{b7} {step}"),
                        _ => name,
                    };
                    ui.label(
                        RichText::new(label)
                            .font(fonts::ui(12.0))
                            .color(c.text_primary),
                    );
                    widgets::spinner(ui, 12.0);
                }
                if let Some(k) = cancel {
                    self.running[k].cancel();
                }
            });
        });
    }

    // ------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        if let Some(exit) = self.confirm_exit {
            let name = self.display_name();
            let resp = Modal::new(Id::unique("dc-workspace-confirm-exit"))
                .frame(widgets::card_frame(ui).inner_margin(Margin::same(24)))
                .show(ui.ctx(), |ui| {
                    ui.set_width(420.0);
                    ui.label(
                        RichText::new(t("ws-confirm-title"))
                            .font(fonts::semibold(18.0))
                            .color(c.text_primary),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(dc_i18n::tr_args("ws-confirm-body", &[("name", &name)]))
                            .font(fonts::ui(13.0))
                            .color(c.text_secondary),
                    );
                    ui.add_space(16.0);
                    let mut choice = None;
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if Button::primary(t("ws-confirm-save")).show(ui).clicked() {
                            choice = Some(0);
                        }
                        if Button::secondary(t("ws-confirm-discard"))
                            .show(ui)
                            .clicked()
                        {
                            choice = Some(1);
                        }
                        if Button::ghost(t("cancel")).show(ui).clicked() {
                            choice = Some(2);
                        }
                    });
                    choice
                });
            match resp.inner {
                Some(0) => {
                    self.confirm_exit = None;
                    self.save_then(Some(exit));
                }
                Some(1) => self.discard_and_exit(exit),
                Some(_) => self.confirm_exit = None,
                None if resp.should_close() => self.confirm_exit = None,
                None => {}
            }
        }
        if self.show_help {
            let resp = Modal::new(Id::unique("dc-workspace-help"))
                .frame(widgets::card_frame(ui).inner_margin(Margin::same(24)))
                .show(ui.ctx(), |ui| {
                    ui.set_width(460.0);
                    ui.label(
                        RichText::new(t("ws-help-title"))
                            .font(fonts::semibold(18.0))
                            .color(c.text_primary),
                    );
                    ui.add_space(12.0);
                    egui::Grid::new("dc-workspace-help-grid")
                        .num_columns(2)
                        .spacing(vec2(24.0, 8.0))
                        .show(ui, |ui| {
                            for (keys, what) in SHORTCUTS {
                                ui.label(
                                    RichText::new(*keys)
                                        .font(fonts::mono(12.0))
                                        .color(c.text_primary),
                                );
                                ui.label(
                                    RichText::new(t(what))
                                        .font(fonts::ui(13.0))
                                        .color(c.text_secondary),
                                );
                                ui.end_row();
                            }
                            // the keys of the module's tools, in key order
                            let mut keys: Vec<_> = self.shortcut_text.iter().collect();
                            keys.sort_by(|a, b| a.1.cmp(b.1));
                            for (id, text) in keys {
                                if let Some(d) = self.commands.iter().find(|d| d.id == *id) {
                                    ui.label(
                                        RichText::new(text)
                                            .font(fonts::mono(12.0))
                                            .color(c.text_primary),
                                    );
                                    ui.label(
                                        RichText::new(t(d.label_key))
                                            .font(fonts::ui(13.0))
                                            .color(c.text_secondary),
                                    );
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(16.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        Button::secondary(t("close")).show(ui).clicked()
                    })
                    .inner
                });
            if resp.inner || resp.should_close() {
                self.show_help = false;
            }
        }
    }
}

/// Translated name of a scan role.
fn role_label(role: ScanRole) -> String {
    t(match role {
        ScanRole::Upper => "ws-role-upper",
        ScanRole::Lower => "ws-role-lower",
        ScanRole::Bite => "ws-role-bite",
    })
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl Z", "undo"),
    ("Ctrl Shift Z", "redo"),
    ("Ctrl S", "save"),
    ("Ctrl Shift S", "save-as"),
    ("/", "ws-key-find"),
    ("Esc", "ws-key-clear"),
    (", .", "ws-key-stage"),
    ("Space", "ws-key-play"),
    ("Home End", "ws-key-first-last"),
    ("Ctrl W", "ws-key-close"),
    ("F1", "ws-key-help"),
];

/// `1.2 GB`, `340 MB`, `12 kB`.
pub(crate) fn format_bytes(b: usize) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bytes() {
        assert_eq!(super::format_bytes(1_234_000_000), "1.2 GB");
        assert_eq!(super::format_bytes(340_000_000), "340 MB");
        assert_eq!(super::format_bytes(12_000), "12 kB");
    }
}
