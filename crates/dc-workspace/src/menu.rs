//! Left panel: the "Find a tool" filter, the module's multi-level menu tree
//! with per-group progress, and the treatment workflow progress.

use std::collections::{HashMap, HashSet};

use dc_module_api::{MenuItem, MenuStatus, MenuTree};
use dc_ui::fonts;
use dc_ui::icons as ic;
use dc_ui::theme::colors;
use dc_ui::tokens::tokens;
use egui::{
    Align2, CornerRadius, Id, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo,
    pos2, vec2,
};

use crate::icons::glyph;
use crate::t;

/// Id of the filter text field (used to focus it with `/`).
pub(crate) fn filter_id() -> Id {
    Id::unique("dc-workspace-menu-filter")
}

/// Ids of the entries shown for `query`: entries whose label contains it
/// (case-insensitive), their descendants and their ancestors. `None` when
/// the query is blank (everything follows the collapse state).
pub fn filter_ids(
    tree: &MenuTree,
    query: &str,
    label: impl Fn(&MenuItem) -> String,
) -> Option<HashSet<&'static str>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    fn walk(
        items: &[MenuItem],
        q: &str,
        label: &dyn Fn(&MenuItem) -> String,
        inherited: bool,
        out: &mut HashSet<&'static str>,
    ) -> bool {
        let mut any = false;
        for it in items {
            let hit = inherited || label(it).to_lowercase().contains(q);
            let below = walk(it.children(), q, label, hit, out);
            if hit || below {
                out.insert(it.id);
                any = true;
            }
        }
        any
    }
    let mut out = HashSet::new();
    walk(&tree.roots, &q, &label, false, &mut out);
    Some(out)
}

/// Command ids of the leaves under `item` (itself when it is a leaf).
#[cfg(test)]
pub(crate) fn leaf_commands(item: &MenuItem) -> Vec<&'static str> {
    match item.command_id() {
        Some(c) => vec![c],
        None => item.children().iter().flat_map(leaf_commands).collect(),
    }
}

/// The counter of a group: how many of its workflow steps are done out of how
/// many it has, `None` for a group with no step (the shortcuts and the
/// corrections do not count).
pub(crate) fn progress_of(item: &MenuItem, done: &dyn Fn(&str) -> bool) -> Option<(usize, usize)> {
    let steps = item.step_ids();
    (!steps.is_empty()).then(|| (steps.iter().filter(|id| done(id)).count(), steps.len()))
}

/// What the user did in the menu this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuEvent {
    /// A leaf was clicked: menu item id and command id.
    Activate(&'static str, &'static str),
    /// A group or sub-menu header was clicked (it also toggles).
    Select(&'static str),
}

/// State of the left panel (kept in memory for the session).
#[derive(Debug, Default)]
pub(crate) struct MenuView {
    /// Filter text.
    pub filter: String,
    /// Expanded groups and sub-menus.
    pub expanded: HashSet<&'static str>,
    /// Focus the filter at the next frame.
    pub focus_filter: bool,
    /// The filter had the keyboard focus at the end of the last frame.
    pub filter_focused: bool,
}

impl MenuView {
    pub fn new(tree: &MenuTree) -> Self {
        let mut v = Self::default();
        // open the first group (case set-up) so a new case shows "Import scans"
        if let Some(first) = tree.roots.first() {
            v.expanded.insert(first.id);
        }
        v
    }

    /// Expands every group and sub-menu.
    pub fn expand_all(&mut self, tree: &MenuTree) {
        for (_, it) in tree.iter() {
            if !it.children().is_empty() {
                self.expanded.insert(it.id);
            }
        }
    }

    /// Expands the ancestors of `id`.
    pub fn reveal(&mut self, tree: &MenuTree, id: &str) {
        if let Some(path) = tree.path_to(id) {
            for p in &path[..path.len().saturating_sub(1)] {
                self.expanded.insert(p);
            }
        }
    }
}

/// Inputs for drawing the menu.
pub(crate) struct MenuCtx<'a> {
    pub tree: &'a MenuTree,
    pub selected: Option<&'static str>,
    pub done: &'a dyn Fn(&str) -> bool,
    pub shortcuts: &'a HashMap<&'static str, String>,
}

/// Draws the filter field, the tree and the workflow footer.
pub(crate) fn show(ui: &mut Ui, view: &mut MenuView, mc: &MenuCtx<'_>) -> Vec<MenuEvent> {
    let mut events = Vec::new();
    let c = *colors(ui);
    let footer_h = 56.0;

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        filter_field(ui, view, ui.available_width() - 12.0);
    });
    ui.add_space(12.0);

    let visible = filter_ids(mc.tree, &view.filter, |it| t(it.label_key));
    let list_h = (ui.available_height() - footer_h).max(0.0);
    egui::ScrollArea::vertical()
        .id_salt("dc-workspace-menu-scroll")
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if visible.as_ref().is_some_and(HashSet::is_empty) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(t("ws-menu-no-match"))
                            .font(fonts::ui(13.0))
                            .color(c.text_secondary),
                    );
                });
            }
            for item in &mc.tree.roots {
                rows(ui, view, mc, visible.as_ref(), item, 0, &mut events);
            }
        });

    // footer: treatment workflow progress, in the steps of the workflow
    let all: Vec<&'static str> = mc.tree.step_ids();
    let done = all.iter().filter(|id| (mc.done)(id)).count();
    let rect = Rect::from_min_size(
        pos2(ui.min_rect().left(), ui.max_rect().bottom() - footer_h),
        vec2(ui.max_rect().width(), footer_h),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, c.border_subtle),
    );
    let p = ui.painter();
    p.text(
        pos2(rect.left() + 16.0, rect.top() + 20.0),
        Align2::LEFT_CENTER,
        t("ws-menu-workflow"),
        fonts::medium(12.0),
        c.text_secondary,
    );
    p.text(
        pos2(rect.right() - 16.0, rect.top() + 20.0),
        Align2::RIGHT_CENTER,
        format!("{done}/{}", all.len()),
        fonts::mono(12.0),
        c.text_primary,
    );
    let bar = Rect::from_min_size(
        pos2(rect.left() + 16.0, rect.top() + 34.0),
        vec2(rect.width() - 32.0, 4.0),
    );
    p.rect_filled(bar, CornerRadius::same(2), c.bg_sunken);
    if !all.is_empty() && done > 0 {
        let w = bar.width() * done as f32 / all.len() as f32;
        p.rect_filled(
            Rect::from_min_size(bar.min, vec2(w, 4.0)),
            CornerRadius::same(2),
            c.status_success,
        );
    }
    let footer = ui.interact(rect, Id::unique("dc-workspace-workflow"), Sense::hover());
    let label = format!("{} {done}/{}", t("ws-menu-workflow"), all.len());
    footer.widget_info(|| WidgetInfo::labeled(Role::ProgressIndicator, true, &label));
    events
}

fn filter_field(ui: &mut Ui, view: &mut MenuView, width: f32) {
    let c = *colors(ui);
    let h = 32.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    // text between the magnifier (left) and the `/` hint (right)
    let text_rect = Rect::from_min_max(
        pos2(rect.left() + 32.0, rect.top()),
        pos2(rect.right() - 26.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let edit = egui::TextEdit::singleline(&mut view.filter)
        .id(filter_id())
        .hint_text(t("ws-menu-filter"))
        .font(fonts::ui(13.0))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric(0, 8))
        .desired_width(text_rect.width());
    let resp = child.add(edit);
    if view.focus_filter {
        resp.request_focus();
        view.focus_filter = false;
    }
    // Escape is handled by the workspace (egui drops the focus first)
    view.filter_focused = resp.has_focus();
    let p = ui.painter();
    let stroke = if resp.has_focus() {
        Stroke::new(2.0, c.focus_ring)
    } else {
        Stroke::new(1.0, c.border_strong)
    };
    p.rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Inside);
    p.text(
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::CENTER_CENTER,
        ic::MAGNIFYING_GLASS,
        fonts::icon(16.0),
        c.text_secondary,
    );
    p.text(
        pos2(rect.right() - 14.0, rect.center().y),
        Align2::CENTER_CENTER,
        "/",
        fonts::mono(12.0),
        c.text_secondary,
    );
}

fn rows(
    ui: &mut Ui,
    view: &mut MenuView,
    mc: &MenuCtx<'_>,
    visible: Option<&HashSet<&'static str>>,
    item: &MenuItem,
    depth: usize,
    events: &mut Vec<MenuEvent>,
) {
    if visible.is_some_and(|v| !v.contains(item.id)) {
        return;
    }
    let has_children = !item.children().is_empty();
    let expanded = has_children && (visible.is_some() || view.expanded.contains(item.id));
    let progress = progress_of(item, mc.done);
    let resp = row(
        ui,
        &Row {
            depth,
            label: t(item.label_key),
            icon: item.icon,
            expandable: has_children,
            expanded,
            meta: progress
                .filter(|_| depth == 0)
                .map(|(done, all)| format!("{done}/{all}")),
            // a leaf has its tick when the case holds its result; a group
            // when all its steps are done
            done: match item.command_id() {
                Some(cmd) => (mc.done)(cmd),
                None => progress.is_some_and(|(done, all)| done == all),
            },
            stub: item.status == MenuStatus::Stub,
            selected: mc.selected == Some(item.id),
            shortcut: item
                .command_id()
                .and_then(|c| mc.shortcuts.get(c).map(String::as_str)),
        },
    );
    if resp.clicked() {
        match item.command_id() {
            Some(cmd) => events.push(MenuEvent::Activate(item.id, cmd)),
            None => {
                if !view.expanded.remove(item.id) {
                    view.expanded.insert(item.id);
                }
                events.push(MenuEvent::Select(item.id));
            }
        }
    }
    if expanded {
        for child in item.children() {
            rows(ui, view, mc, visible, child, depth + 1, events);
        }
    }
}

struct Row<'a> {
    depth: usize,
    label: String,
    icon: Option<&'a str>,
    expandable: bool,
    expanded: bool,
    meta: Option<String>,
    done: bool,
    stub: bool,
    selected: bool,
    shortcut: Option<&'a str>,
}

/// One full-width menu row (Group 36 / Item 32 / Sub 28 pt).
fn row(ui: &mut Ui, r: &Row<'_>) -> Response {
    let c = *colors(ui);
    let sz = &tokens().size;
    let (h, indent, font) = match r.depth {
        0 => (
            sz.menu_group_row,
            sz.menu_indent[0],
            if r.selected {
                fonts::bold(13.0)
            } else {
                fonts::semibold(13.0)
            },
        ),
        1 => (
            sz.menu_item_row,
            sz.menu_indent[1],
            if r.selected {
                fonts::semibold(13.0)
            } else {
                fonts::ui(13.0)
            },
        ),
        d => (
            sz.menu_sub_row,
            sz.menu_indent[2] + 16.0 * (d - 2) as f32,
            if r.selected {
                fonts::semibold(12.0)
            } else {
                fonts::ui(12.0)
            },
        ),
    };
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let label = r.label.clone();
    let selected = r.selected;
    resp.widget_info(|| WidgetInfo::selected(Role::TreeItem, true, selected, &label));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if r.selected {
        p.rect_filled(rect, CornerRadius::ZERO, c.select_soft);
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(tokens().stroke.active_bar, h)),
            CornerRadius::ZERO,
            c.select_blue,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::ZERO, c.bg_hover);
    }
    if resp.has_focus() {
        p.rect_stroke(
            rect.shrink(1.0),
            CornerRadius::same(2),
            Stroke::new(2.0, c.focus_ring),
            StrokeKind::Inside,
        );
    }
    let text_color = if r.stub {
        c.text_disabled
    } else if r.depth == 0 || r.selected {
        c.text_primary
    } else {
        c.text_secondary.lerp_to_gamma(c.text_primary, 0.6)
    };
    let cy = rect.center().y;
    let mut x = rect.left() + indent;
    if r.depth == 0 {
        // caret 12 + icon 18 + label
        let caret = if r.expanded {
            ic::CARET_DOWN
        } else {
            ic::CARET_RIGHT
        };
        p.text(
            pos2(x + 6.0, cy),
            Align2::CENTER_CENTER,
            caret,
            fonts::icon(12.0),
            c.text_secondary,
        );
        x += 22.0;
        p.text(
            pos2(x + 9.0, cy),
            Align2::CENTER_CENTER,
            r.icon.map_or(ic::DOT_OUTLINE, glyph),
            fonts::icon(18.0),
            text_color,
        );
        x += 26.0;
    } else {
        if r.expandable {
            let caret = if r.expanded {
                ic::CARET_DOWN
            } else {
                ic::CARET_RIGHT
            };
            p.text(
                pos2(x - 12.0, cy),
                Align2::CENTER_CENTER,
                caret,
                fonts::icon(10.0),
                c.text_secondary,
            );
        }
        // a sub-group shows its own icon: it has no result to tick
        let (glyph_s, color) = if r.expandable && r.icon.is_some() {
            (r.icon.map_or(ic::DOT_OUTLINE, glyph), c.text_secondary)
        } else if r.done {
            (ic::CHECK_CIRCLE, c.status_success)
        } else if r.stub {
            (ic::CIRCLE_DASHED, c.text_disabled)
        } else {
            (ic::CIRCLE, c.text_secondary)
        };
        let gs = if r.depth == 1 { 16.0 } else { 13.0 };
        p.text(
            pos2(x + gs / 2.0, cy),
            Align2::CENTER_CENTER,
            glyph_s,
            fonts::icon(gs),
            color,
        );
        x += gs + 8.0;
    }
    let mut right = rect.right() - 12.0;
    if let Some(meta) = &r.meta {
        let g = p.layout_no_wrap(meta.clone(), fonts::mono(11.0), c.text_secondary);
        right -= g.size().x;
        p.galley(pos2(right, cy - g.size().y / 2.0), g, c.text_secondary);
        right -= 8.0;
    }
    if let Some(sc) = r.shortcut {
        let g = p.layout_no_wrap(sc.to_owned(), fonts::mono(11.0), c.text_secondary);
        right -= g.size().x;
        p.galley(pos2(right, cy - g.size().y / 2.0), g, c.text_secondary);
        right -= 8.0;
    }
    let galley = p.layout(r.label.clone(), font, text_color, (right - x).max(10.0));
    let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom()));
    ui.painter().with_clip_rect(clip).galley(
        pos2(x, cy - galley.size().y / 2.0),
        galley,
        text_color,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> MenuTree {
        MenuTree::new(vec![
            MenuItem::submenu(
                "case",
                "Case",
                vec![
                    MenuItem::command("case.import", "Import scans", "case.import"),
                    MenuItem::command("case.info", "Case info", "case.info"),
                ],
            ),
            MenuItem::submenu(
                "plan",
                "Planning",
                vec![MenuItem::submenu(
                    "plan.stage",
                    "Staging",
                    vec![MenuItem::command("plan.stage.ipr", "IPR", "plan.stage.ipr")],
                )],
            ),
        ])
    }

    fn ids(q: &str) -> Option<Vec<&'static str>> {
        filter_ids(&tree(), q, |i| i.label_key.to_owned()).map(|s| {
            let mut v: Vec<_> = s.into_iter().collect();
            v.sort_unstable();
            v
        })
    }

    #[test]
    fn filter_keeps_ancestors_and_descendants() {
        assert_eq!(ids("  "), None);
        assert_eq!(ids("import"), Some(vec!["case", "case.import"]));
        assert_eq!(
            ids("ipr"),
            Some(vec!["plan", "plan.stage", "plan.stage.ipr"])
        );
        assert_eq!(
            ids("STAGING"),
            Some(vec!["plan", "plan.stage", "plan.stage.ipr"])
        );
        assert_eq!(ids("nothing"), Some(vec![]));
    }

    #[test]
    fn leaves_and_reveal() {
        let t = tree();
        assert_eq!(leaf_commands(&t.roots[1]), vec!["plan.stage.ipr"]);
        let mut v = MenuView::new(&t);
        assert!(v.expanded.contains("case"));
        v.reveal(&t, "plan.stage.ipr");
        assert!(v.expanded.contains("plan") && v.expanded.contains("plan.stage"));
        v.expand_all(&t);
        assert_eq!(v.expanded.len(), 3);
    }

    #[test]
    fn a_group_counts_its_steps_and_one_without_steps_has_no_counter() {
        let t = MenuTree::new(vec![
            MenuItem::submenu(
                "g",
                "G",
                vec![
                    MenuItem::command("a", "A", "a"),
                    MenuItem::command("b", "B", "b").not_a_step(),
                    MenuItem::command("c", "C", "c"),
                    MenuItem::submenu(
                        "m",
                        "M",
                        vec![MenuItem::command("d", "D", "d").not_a_step()],
                    ),
                ],
            ),
            MenuItem::submenu(
                "e",
                "E",
                vec![MenuItem::command("f", "F", "f").not_a_step()],
            ),
        ]);
        // what is done among the shortcuts does not count
        let done = |id: &str| id == "a" || id == "b" || id == "f";
        assert_eq!(progress_of(&t.roots[0], &done), Some((1, 2)));
        assert_eq!(progress_of(&t.roots[1], &done), None);
        assert_eq!(progress_of(&t.roots[0].children()[3], &done), None);
    }
}
