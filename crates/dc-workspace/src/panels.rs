//! The side panels of the workspace: the widths the user chose for the left
//! menu and the inspector, and where the last frame drew them and the 3D
//! view.

use dc_ui::panels::{Side, SidePanel, clamp_width};
use dc_ui::tokens::{Sizes, tokens};
use egui::{Rangef, Rect};

use crate::workspace::Workspace;

/// egui id of the left menu panel.
pub(crate) const PANEL_MENU: &str = "dc-workspace-menu";
/// egui id of the inspector panel.
pub(crate) const PANEL_INSPECTOR: &str = "dc-workspace-inspector";

/// Widths (points) of the left menu and of the inspector the user chose by
/// dragging their edges. They are shown within their ranges of the design
/// tokens, and narrower when the window leaves no room.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelWidths {
    /// The left menu.
    pub menu: f32,
    /// The inspector (right).
    pub inspector: f32,
}

impl Default for PanelWidths {
    /// The widths of the design (`left_menu_width`, `inspector_width`).
    fn default() -> Self {
        let sz = tokens().size;
        Self {
            menu: sz.left_menu_width,
            inspector: sz.inspector_width,
        }
    }
}

impl PanelWidths {
    /// The widths within their ranges (a width that is not a number goes
    /// back to the default).
    pub fn clamped(self) -> Self {
        let sz = tokens().size;
        let d = Self::default();
        let one = |w: f32, default: f32, range: Rangef| {
            if w.is_finite() {
                clamp_width(w, range, f32::INFINITY)
            } else {
                default
            }
        };
        Self {
            menu: one(self.menu, d.menu, menu_range(&sz)),
            inspector: one(self.inspector, d.inspector, inspector_range(&sz)),
        }
    }
}

/// Where the workspace drew its parts in the last frame (window points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceLayout {
    /// The left menu.
    pub menu: Rect,
    /// The inspector.
    pub inspector: Rect,
    /// The 3D view: all the room the panels leave in the centre.
    pub view: Rect,
    /// How far the contents of the menu run past its width (clipped).
    pub menu_overflow: f32,
    /// How far the contents of the inspector run past its width (clipped).
    pub inspector_overflow: f32,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            menu: Rect::NOTHING,
            inspector: Rect::NOTHING,
            view: Rect::NOTHING,
            menu_overflow: 0.0,
            inspector_overflow: 0.0,
        }
    }
}

/// What the workspace keeps of its side panels.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Panels {
    /// The widths the user chose.
    pub widths: PanelWidths,
    /// The user resized a panel since the shell last took the widths.
    pub resized: bool,
    /// The last frame.
    pub layout: WorkspaceLayout,
    /// How far the module's part of the inspector ran past its width this
    /// frame (it is fitted to the width apart from the rest).
    pub module_overflow: f32,
}

impl Panels {
    /// Notes what the panel `id` drew.
    pub fn note(&mut self, id: &str, rect: Rect, overflow: f32, resized: bool) {
        if id == PANEL_MENU {
            self.layout.menu = rect;
            self.layout.menu_overflow = overflow;
        } else {
            self.layout.inspector = rect;
            self.layout.inspector_overflow =
                overflow.max(std::mem::take(&mut self.module_overflow));
        }
        self.resized |= resized;
    }
}

impl Workspace {
    /// Runs the module's part of the inspector fitted to the inspector's
    /// width: what it draws past the width is clipped, and what follows
    /// keeps the width.
    pub(crate) fn module_inspector(&mut self, ui: &mut egui::Ui) {
        let ((), overflow) = dc_ui::panels::fit_width(ui, |ui| {
            self.module_ctx_run(|m, ctx| m.inspector(ui, ctx));
        });
        self.panels.module_overflow = self.panels.module_overflow.max(overflow);
    }
}

fn menu_range(sz: &Sizes) -> Rangef {
    Rangef::new(sz.left_menu_min_width, sz.left_menu_max_width)
}

fn inspector_range(sz: &Sizes) -> Rangef {
    Rangef::new(sz.inspector_min_width, sz.inspector_max_width)
}

/// The panel `id` (`PANEL_MENU` on the left, else the inspector on the
/// right) with its default width and range.
pub(crate) fn side_panel(id: &'static str, sz: &Sizes) -> SidePanel {
    if id == PANEL_MENU {
        SidePanel::new(Side::Left, id, sz.left_menu_width, menu_range(sz))
    } else {
        SidePanel::new(Side::Right, id, sz.inspector_width, inspector_range(sz))
    }
}

impl Workspace {
    /// The widths of the side panels the user chose.
    pub fn panel_widths(&self) -> PanelWidths {
        self.panels.widths
    }

    /// Sets the widths of the side panels (the saved ones, or an agent's),
    /// from the next frame.
    pub fn set_panel_widths(&mut self, widths: PanelWidths) {
        self.panels.widths = widths.clamped();
    }

    /// The widths of the side panels once the user finished resizing one
    /// (a drag of its edge, or a double-click that restores the default):
    /// the shell saves them.
    pub fn take_resized_panels(&mut self) -> Option<PanelWidths> {
        std::mem::take(&mut self.panels.resized).then_some(self.panels.widths)
    }

    /// Where the last frame drew the menu, the inspector and the 3D view.
    pub fn layout(&self) -> WorkspaceLayout {
        self.panels.layout
    }
}
