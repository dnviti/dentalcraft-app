//! Layout of the workspace: top bar, left menu, inspector, timeline, status
//! bar, dialogs and keyboard shortcuts.

use std::path::PathBuf;
use std::sync::Arc;

use dc_case::{BlobRef, CaseState, RoleSource, ScanRole};
use dc_module_api::MenuItem;
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::panels::contained;
use dc_ui::theme::colors;
use dc_ui::tokens::tokens;
use dc_ui::widgets::{self, Button};
use dca_model::{ClosedBy, ScanBase};
use egui::{
    Align, Align2, CentralPanel, CornerRadius, Frame, Id, Key, KeyboardShortcut, Layout, Margin,
    Modal, Modifiers, Panel, RichText, Role, Sense, Stroke, Ui, WidgetInfo, vec2,
};

use crate::function::caption;
use crate::leaf;
use crate::menu::{self, MenuCtx, MenuEvent};
use crate::panels::{PANEL_INSPECTOR, PANEL_MENU, side_panel};
use crate::t;
use crate::timeline::{self, TimelineModel};
use crate::workspace::{Exit, SaveStatus, Workspace, WorkspaceAction};

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
        // the automatic save: a frame comes when it is due
        if let Some(wait) = self.tick(ui.input(|i| i.time)) {
            ui.ctx().request_repaint_after(wait);
        }
        // files dropped on the window go to the import (not under a dialog)
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        // the scan and print windows, its own dialogs and the shell's take them
        let covered = self.confirm_exit.is_some()
            || self.confirm_remove.is_some()
            || self.show_help
            || self.shell_dialog
            || self.scan.is_open()
            || self.print.is_open();
        if !dropped.is_empty() && !covered {
            self.import_dropped(dropped);
        }
        if !covered {
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
            .show(ui, |ui| contained(ui, |ui| self.status_bar(ui)));
        // the side panels: as wide as the user dragged them, the 3D view
        // keeps the rest (and never less than its minimum while it can)
        let inspector = side_panel(PANEL_INSPECTOR, &sz)
            .frame(Frame::new().fill(c.bg_panel).inner_margin(Margin::same(16)))
            .reserve(sz.viewport_min_width);
        let inspector_room = inspector.shown_width(self.panels.widths.inspector, f32::INFINITY);
        let mut width = self.panels.widths.menu;
        let r = side_panel(PANEL_MENU, &sz)
            .frame(Frame::new().fill(c.bg_panel))
            .reserve(inspector_room + sz.viewport_min_width)
            .show(ui, &mut width, |ui| self.left_panel(ui));
        self.panels.widths.menu = width;
        self.panels.note(PANEL_MENU, r.rect, r.overflow, r.resized);
        let mut width = self.panels.widths.inspector;
        let r = inspector.show(ui, &mut width, |ui| self.inspector_panel(ui));
        self.panels.widths.inspector = width;
        self.panels
            .note(PANEL_INSPECTOR, r.rect, r.overflow, r.resized);
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
                contained(ui, |ui| {
                    let model = TimelineModel::from_state(self.undo.current());
                    timeline::show(ui, &model, &mut self.timeline);
                })
            });
        CentralPanel::no_frame().show(ui, |ui| self.center_ui(ui));

        // the scan and print popups (under the dialogs of the workspace)
        self.scan.show(ui);
        self.print.show(ui);
        self.dialogs(ui);
        if !self.running.is_empty() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.next_action()
    }

    fn keyboard(&mut self, ui: &Ui) {
        self.sequence_keys(ui);
        self.bite_points_keys(ui);
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
        // a key of the module selects its function (and opens its tool): it
        // never runs anything
        for (id, sc) in self.shortcuts.clone() {
            if crate::keys::is_plain(&sc) && ui.ctx().egui_wants_keyboard_input() {
                continue;
            }
            if ui.input_mut(|i| i.consume_shortcut(&sc)) {
                self.press_key(id);
            }
        }
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        // W, E, R: the mode of the gizmo of the selected component
        self.selection_keys(ui);
        // Delete (Backspace on a Mac keyboard): the selected scan goes, after
        // the confirmation, unless a tool or the numbering holds the keys
        if self.manual.tool.is_none()
            && self.manual.sequence.is_none()
            && let Some(crate::select::Component::Scan(id)) = self.selection.component
            && (consume(Modifiers::NONE, Key::Delete) || consume(Modifiers::NONE, Key::Backspace))
        {
            self.ask_remove_scan(id);
        }
        if consume(Modifiers::NONE, Key::Escape) {
            if std::mem::take(&mut self.menu_view.filter_focused) {
                // Escape in the filter field: clear it
                self.menu_view.filter.clear();
            } else if !self.selection_escape() {
                // a drag of the gizmo, then the selected component, go first
                self.clear_selection();
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
            self.save_status_ui(ui);
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
                if widgets::icon_button(ui, ic::GEAR_SIX, t("ws-settings")).clicked() {
                    self.open_settings();
                }
                if Button::ghost(t("ws-help"))
                    .icon(ic::QUESTION)
                    .show(ui)
                    .clicked()
                {
                    self.show_help = true;
                }
                widgets::divider_v(ui, 24.0);
                // always there: a save asked for while another runs follows it
                if Button::primary(t("save"))
                    .icon(ic::FLOPPY_DISK)
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

    /// The quiet line about saving, after the project name: saving, saved,
    /// unsaved changes, or not saved (its error is on screen). Its tooltip
    /// says whether the changes are saved by themselves.
    fn save_status_ui(&self, ui: &mut Ui) {
        let c = *colors(ui);
        let (dot, key) = match self.save_status() {
            SaveStatus::Quiet => return,
            SaveStatus::Saving => (c.text_secondary, "ws-saving"),
            SaveStatus::Unsaved => (c.accent_amber, "ws-unsaved"),
            SaveStatus::Failed => (c.status_danger, "ws-not-saved"),
            SaveStatus::Saved => (c.status_success, "ws-saved"),
        };
        let tip = if self.autosaves() {
            "ws-autosave-on"
        } else {
            "ws-autosave-off"
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            widgets::status_dot(ui, dot);
            ui.label(
                RichText::new(t(key))
                    .font(fonts::ui(12.0))
                    .color(c.text_secondary),
            );
        })
        .response
        .on_hover_text(t(tip));
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
        for MenuEvent::Select(id) in events {
            self.select_item(id);
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
                // the component selected in the viewport, above the function
                self.selection_inspector(ui);
                match &item {
                    // a function: the workspace lays it out, by the menu law
                    Some(item) if item.command_id().is_some() => self.function_inspector(ui, item),
                    // a group: what the module says of it
                    Some(_) => self.module_inspector(ui),
                    None => {
                        ui.heading(t(self.module.name_key()));
                        ui.label(t("ws-inspector-select"));
                    }
                }
            });
    }

    /// The parameters of "Import scans": the role of the next import (it
    /// applies to the next file dialog or drop, then goes back to auto).
    pub(crate) fn import_role_params(&mut self, ui: &mut Ui) {
        sub_heading(ui, "ws-import-role-title");
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
    }

    /// The manual corrections of "Import scans": the scans of the case, each
    /// with how its role was decided and a control to change it, the
    /// occlusal side of an arch scan to turn over, and the base that closes
    /// an open arch scan.
    pub(crate) fn import_scan_list(&mut self, ui: &mut Ui) {
        let c = *colors(ui);
        sub_heading(ui, "ws-import-scans-title");
        ui.add_space(6.0);
        let state = Arc::clone(self.undo.current());
        let flipped = crate::overlay::aligners(&state)
            .map(|d| d.occlusal_flipped.clone())
            .unwrap_or_default();
        let scans: Vec<_> = state
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
        let can_flip = self.module_spec(leaf::SET_ARCH).is_some();
        // how a role was decided, turning a scan over and closing it with a
        // base are advanced options; the roles and "Confirm role" are not
        let advanced = dc_module_api::advanced_ui(ui.ctx());
        let mut change = None;
        let mut flip = None;
        let mut remove = None;
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
                            if self.scan_bin(ui, id, busy) {
                                remove = Some(id);
                            }
                            ui.label(
                                RichText::new(role_label(role))
                                    .font(fonts::mono(12.0))
                                    .color(c.text_secondary),
                            );
                        });
                    });
                    // a guess, or a recognition worth a look: its badge says
                    // why to confirm it, whatever the options; the badge of a
                    // sure role is an advanced detail
                    let guess =
                        matches!(source, RoleSource::Provisional | RoleSource::DetectedUnsure);
                    if advanced || guess {
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
                            // the same set-role with the role it has confirms it
                            if guess
                                && Button::ghost(t("ws-import-confirm-role"))
                                    .enabled(!busy)
                                    .show(ui)
                                    .clicked()
                            {
                                change = Some((id, role, role));
                            }
                        });
                    }
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
                    // the side of the teeth recognised the wrong way round
                    if advanced && can_flip && role != ScanRole::Bite {
                        ui.horizontal(|ui| {
                            if Button::ghost(t("ws-import-flip"))
                                .enabled(!busy)
                                .show(ui)
                                .on_hover_text(t("ws-import-flip-tip"))
                                .clicked()
                            {
                                flip = Some(role);
                            }
                            if flipped.contains(&role) {
                                widgets::badge(
                                    ui,
                                    widgets::BadgeKind::Warning,
                                    t("ws-import-flipped"),
                                );
                            }
                        });
                    }
                });
            ui.add_space(6.0);
        }
        if let Some(id) = remove {
            self.ask_remove_scan(id);
        } else if let Some((id, from, to)) = change {
            self.set_scan_role(id, from, to);
        } else if let Some(role) = flip {
            self.flip_occlusal(role);
        }
        if advanced {
            self.import_base(ui, &state);
        }
    }

    /// "Close with a base" (the plugin's fold "Scansione aperta: chiudi con
    /// una base"): the thickness of the base, then per arch scan how it is
    /// closed ([`base_status`]) and the button that closes it, "Run again"
    /// once it has a base. A scan the scanner closed has nothing to close.
    fn import_base(&mut self, ui: &mut Ui, state: &CaseState) {
        let arches: Vec<(ScanRole, BlobRef)> = [ScanRole::Upper, ScanRole::Lower]
            .into_iter()
            .filter_map(|r| state.case.scan(r).map(|s| (r, s.mesh)))
            .collect();
        if arches.is_empty() || self.module_spec(leaf::MAKE_BASE).is_none() {
            return;
        }
        ui.add_space(8.0);
        sub_heading(ui, "ws-import-base-title");
        caption(ui, t("ws-import-base-hint"));
        ui.add_space(6.0);
        let c = *colors(ui);
        let busy = self.is_busy();
        let (lo, hi) = (
            dca_algo::scan_base::HEIGHT_MIN_MM as f32,
            dca_algo::scan_base::HEIGHT_MAX_MM as f32,
        );
        let default = dca_algo::scan_base::HEIGHT_DEFAULT_MM as f32;
        let records = crate::overlay::aligners(state).map(|d| &d.scan_bases);
        // asked once per scan blob; the blobs the case no longer has go
        self.scan_open
            .retain(|b, _| arches.iter().any(|(_, m)| m == b));
        let statuses: Vec<(ScanRole, BaseStatus)> = arches
            .iter()
            .map(|&(role, blob)| {
                let record = records.and_then(|r| r.get(&blob)).copied();
                let status = base_status(record, || {
                    *self.scan_open.entry(blob).or_insert_with(|| {
                        state
                            .mesh(&blob)
                            .is_some_and(|m| dca_algo::scan_base::is_open(m))
                    })
                });
                (role, status)
            })
            .collect();
        let mut close = None;
        ui.add_enabled_ui(!busy, |ui| {
            widgets::labeled_slider_mm(
                ui,
                &t("ws-import-base-height"),
                &mut self.base_height_mm,
                lo..=hi,
                default,
            );
            for (role, status) in statuses {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(role_label(role))
                            .font(fonts::semibold(13.0))
                            .color(c.text_primary),
                    );
                    widgets::badge(ui, status.kind(), t(status.key()));
                });
                caption(ui, t(status.hint_key()));
                if let Some(key) = status.button_key() {
                    let label = format!("{} \u{b7} {}", t(key), role_label(role));
                    if Button::secondary(label).full_width(true).show(ui).clicked() {
                        close = Some(role);
                    }
                }
            }
        });
        if let Some(role) = close {
            let h = f64::from(self.base_height_mm.clamp(lo, hi));
            self.close_with_base(role, h);
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
                // the technical readouts: with the advanced options only
                if dc_module_api::advanced_ui(ui.ctx()) {
                    ui.label(mono(format_bytes(self.undo.memory_bytes())));
                    let dt = ui.input(|i| i.stable_dt).max(1e-3);
                    ui.label(mono(format!("{:.0} fps", 1.0 / dt)));
                    ui.label(mono(t("ws-numbering")));
                    ui.label(mono(t("ws-units")));
                }
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
        if let Some(id) = self.confirm_remove {
            let name = self
                .undo
                .current()
                .case
                .scan_by_id(id)
                .map(|s| s.source_name.clone());
            match name {
                // the scan left the case meanwhile (an undo): nothing to ask
                None => self.confirm_remove = None,
                Some(name) => self.remove_dialog(ui, id, &name),
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
                                    RichText::new(dc_ui::keys::platform_keys(keys))
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

    /// The bin of a scan row ("Remove the scan"): `true` when clicked. Off
    /// while `busy`, and while the arches are locked for an arch scan, with
    /// the reason on hover.
    pub(crate) fn scan_bin(&self, ui: &mut Ui, id: uuid::Uuid, busy: bool) -> bool {
        let locked = self.scan_removal_locked(id);
        let r = widgets::icon_button_sized(
            ui,
            ic::TRASH,
            t("ws-scan-remove"),
            28.0,
            !busy && locked.is_none(),
        );
        match locked {
            Some(why) => r.on_disabled_hover_text(why).clicked(),
            None => r.clicked(),
        }
    }

    /// "Remove the scan?": Remove runs `case.remove-scan`, Cancel, Esc or a
    /// click outside keep the scan.
    fn remove_dialog(&mut self, ui: &mut Ui, id: uuid::Uuid, name: &str) {
        let c = *colors(ui);
        let busy = self.is_busy();
        let resp = Modal::new(Id::unique("dc-workspace-confirm-remove"))
            .frame(widgets::card_frame(ui).inner_margin(Margin::same(24)))
            .show(ui.ctx(), |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(t("ws-scan-remove-title"))
                        .font(fonts::semibold(18.0))
                        .color(c.text_primary),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(dc_i18n::tr_args("ws-scan-remove-body", &[("name", name)]))
                        .font(fonts::ui(13.0))
                        .color(c.text_secondary),
                );
                ui.add_space(16.0);
                let mut choice = None;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::primary(t("ws-scan-remove-confirm"))
                        .icon(ic::TRASH)
                        .enabled(!busy)
                        .show(ui)
                        .clicked()
                    {
                        choice = Some(true);
                    }
                    if Button::ghost(t("cancel")).show(ui).clicked() {
                        choice = Some(false);
                    }
                });
                choice
            });
        match resp.inner {
            Some(true) => {
                self.confirm_remove = None;
                self.remove_scan(id);
            }
            Some(false) => self.confirm_remove = None,
            None if resp.should_close() => self.confirm_remove = None,
            None => {}
        }
    }
}

/// A heading inside a section of the inspector.
fn sub_heading(ui: &mut Ui, key: &str) {
    let c = *colors(ui);
    ui.label(
        RichText::new(t(key))
            .font(fonts::semibold(13.0))
            .color(c.text_primary),
    );
}

/// Translated name of a scan role.
fn role_label(role: ScanRole) -> String {
    t(match role {
        ScanRole::Upper => "ws-role-upper",
        ScanRole::Lower => "ws-role-lower",
        ScanRole::Bite => "ws-role-bite",
    })
}

/// How an arch scan of the case is closed, as "Close with a base" shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaseStatus {
    /// It has a base, made by the import, by hand, or on the tooth marks.
    Base(ClosedBy),
    /// Open underneath, with no base.
    Open,
    /// Closed as it was read (by the scanner): nothing to close.
    ClosedAsRead,
}

impl BaseStatus {
    /// The badge of the scan (short: badges do not wrap).
    fn key(self) -> &'static str {
        match self {
            Self::Base(ClosedBy::Import) => "ws-base-status-import",
            Self::Base(ClosedBy::Hand) => "ws-base-status-hand",
            Self::Base(ClosedBy::ToothMarks) => "ws-base-status-marks",
            Self::Open => "ws-base-status-open",
            Self::ClosedAsRead => "ws-base-status-read",
        }
    }

    /// The line under the badge.
    fn hint_key(self) -> &'static str {
        match self {
            Self::Base(ClosedBy::Import | ClosedBy::Hand) => "ws-base-hint-scan",
            Self::Base(ClosedBy::ToothMarks) => "ws-base-hint-marks",
            Self::Open => "ws-base-hint-open",
            Self::ClosedAsRead => "ws-base-hint-read",
        }
    }

    fn kind(self) -> widgets::BadgeKind {
        match self {
            Self::Base(_) => widgets::BadgeKind::Success,
            Self::Open => widgets::BadgeKind::Warning,
            Self::ClosedAsRead => widgets::BadgeKind::Neutral,
        }
    }

    /// The run button: "Close with a base", "Run again" once it has one;
    /// none for a scan closed as it was read.
    fn button_key(self) -> Option<&'static str> {
        match self {
            Self::Base(_) => Some("ws-btn-run-again"),
            Self::Open => Some("ws-import-base"),
            Self::ClosedAsRead => None,
        }
    }
}

/// The status of an arch scan with the base record `record`; `open` (asked
/// only without a record) says whether it is open underneath.
fn base_status(record: Option<ScanBase>, open: impl FnOnce() -> bool) -> BaseStatus {
    match record {
        Some(b) => BaseStatus::Base(b.closed_by),
        None if open() => BaseStatus::Open,
        None => BaseStatus::ClosedAsRead,
    }
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl Z", "undo"),
    ("Ctrl Shift Z", "redo"),
    ("Ctrl S", "save"),
    ("Ctrl Shift S", "save-as"),
    ("/", "ws-key-find"),
    ("Esc", "ws-key-clear"),
    ("W E R", "ws-key-gizmo"),
    ("Delete Backspace", "ws-key-remove"),
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
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(super::format_bytes(1_234_000_000), "1.2 GB");
        assert_eq!(super::format_bytes(340_000_000), "340 MB");
        assert_eq!(super::format_bytes(12_000), "12 kB");
    }

    #[test]
    fn a_scan_says_how_it_is_closed_and_only_a_scan_to_close_has_a_button() {
        let record = |closed_by| ScanBase {
            vertices: 3,
            faces: 1,
            flipped: false,
            closed_by,
        };
        // a record says who closed it; the border is not asked then
        for (by, key) in [
            (ClosedBy::Import, "ws-base-status-import"),
            (ClosedBy::Hand, "ws-base-status-hand"),
            (ClosedBy::ToothMarks, "ws-base-status-marks"),
        ] {
            let s = base_status(Some(record(by)), || panic!("asked"));
            assert_eq!(s, BaseStatus::Base(by));
            assert_eq!(s.key(), key);
            assert_eq!(s.button_key(), Some("ws-btn-run-again"));
        }
        let open = base_status(None, || true);
        assert_eq!(open, BaseStatus::Open);
        assert_eq!(open.button_key(), Some("ws-import-base"));
        let read = base_status(None, || false);
        assert_eq!(read, BaseStatus::ClosedAsRead);
        assert_eq!(read.button_key(), None);
        // every text is in English and Italian
        for s in [
            BaseStatus::Base(ClosedBy::Import),
            BaseStatus::Base(ClosedBy::Hand),
            BaseStatus::Base(ClosedBy::ToothMarks),
            open,
            read,
        ] {
            for key in [Some(s.key()), Some(s.hint_key()), s.button_key()]
                .into_iter()
                .flatten()
            {
                for lang in ["en", "it"] {
                    let ftl = match lang {
                        "en" => include_str!("../i18n/en/dc-workspace.ftl"),
                        _ => include_str!("../i18n/it/dc-workspace.ftl"),
                    };
                    assert!(
                        ftl.lines().any(|l| l.starts_with(&format!("{key} ="))),
                        "{lang}: {key}"
                    );
                }
            }
        }
    }
}
