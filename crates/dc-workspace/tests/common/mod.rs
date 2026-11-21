//! Shared checks of the layout of the centre area at several window sizes.

use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui::{Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// Window sizes the layout must hold at: the minimum window, and narrower
/// ones for a smaller screen or a larger inspector.
pub const SIZES: [(f32, f32); 4] = [
    (1920.0, 1080.0),
    (1280.0, 800.0),
    (1100.0, 700.0),
    (960.0, 640.0),
];

/// Labels the checks look for, in the language under test.
pub struct Labels {
    /// The view segments of the toolbar.
    pub views: [&'static str; 5],
    /// The "fit to view" button (the last widget of the toolbar).
    pub fit: &'static str,
    /// The start of the timeline hint of a case without stages.
    pub hint: &'static str,
    /// The arch buttons of the toolbar, Upper then Lower.
    pub arches: [&'static str; 2],
    /// The icon buttons of the toolbar by their tooltip, in its order
    /// (display, tools, layers, marks, then the three of `solids`).
    pub buttons: [&'static str; 15],
    /// The group of the whole models over the arches (study models, final
    /// position, aligner): its name for screen readers.
    pub solids: &'static str,
    /// The overlay toggle of the timeline (the one at the right).
    pub overlay: &'static str,
}

fn rect_of(h: &Harness<'_, ()>, label: &str) -> Rect {
    h.get_by_label(label).rect()
}

/// The toolbar (arch and icon buttons included) stays left of the view cube
/// and the timeline hint stays left of the overlay toggle, which keeps its
/// own room, at every size.
pub fn assert_layout(labels: &Labels) {
    for (w, hgt) in SIZES {
        let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
        let mut h = Harness::builder()
            .with_size(vec2(w, hgt))
            // every button of the toolbar: the advanced options on
            .with_context_setup(|ctx| {
                dc_ui::theme::install(ctx, dc_ui::Mode::Dark);
                dc_module_api::set_advanced_ui(ctx, true);
            })
            .build_ui(move |ui| {
                let _ = ws.show(ui);
            });
        h.run();
        let size = dc_ui::tokens::tokens().size;
        let centre_right = w - size.inspector_width;
        // the cube is 100 wide at the top right of the centre area, 16 pt in
        let cube_left = centre_right - 16.0 - 100.0;
        let mut widgets: Vec<(&str, Rect)> =
            labels.views.iter().map(|l| (*l, rect_of(&h, l))).collect();
        widgets.push((labels.fit, rect_of(&h, labels.fit)));
        // the inspector may use the same words: the toolbar ones are in the centre area
        for l in labels.arches {
            let r = h
                .get_all_by_label(l)
                .map(|n| n.rect())
                .filter(|r| r.left() >= size.left_menu_width && r.right() <= centre_right)
                .min_by(|a, b| a.top().total_cmp(&b.top()))
                .unwrap_or_else(|| panic!("{w}x{hgt}: no {l} button in the toolbar"));
            widgets.push((l, r));
        }
        for l in labels.buttons {
            let r = h
                .get_all_by_label(l)
                .map(|n| n.rect())
                .filter(|r| r.left() >= size.left_menu_width && r.left() < centre_right)
                .min_by(|a, b| a.top().total_cmp(&b.top()))
                .unwrap_or_else(|| panic!("{w}x{hgt}: no {l} button in the toolbar"));
            widgets.push((l, r));
        }
        for (l, r) in &widgets {
            assert!(
                r.right() <= cube_left,
                "{w}x{hgt}: the toolbar widget {l} ({r:?}) runs under the view cube (left edge {cube_left})"
            );
            assert!(
                r.left() >= size.left_menu_width,
                "{w}x{hgt}: the toolbar widget {l} is left of the centre area: {r:?}"
            );
        }
        // the last three buttons are one named group
        let group = h.get_by_label(labels.solids);
        assert_eq!(group.accesskit_node().role(), egui::accesskit::Role::Group);
        let g = group.rect();
        for (l, r) in &widgets[widgets.len() - 3..] {
            assert!(
                g.contains_rect(*r),
                "{w}x{hgt}: {l} ({r:?}) is not in the group {} ({g:?})",
                labels.solids
            );
        }
        // the overlay toggle keeps its room (the timeline one is the lowest)
        let toggle = h
            .get_all_by_label(labels.overlay)
            .map(|n| n.rect())
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap();
        assert!(
            toggle.right() <= centre_right,
            "{w}x{hgt}: the overlay toggle leaves the centre area"
        );
        let hint = h.get_by_label_contains(labels.hint).rect();
        assert!(
            hint.right() <= centre_right,
            "{w}x{hgt}: the timeline hint leaves the centre area: {hint:?}"
        );
        assert!(
            !hint.intersects(toggle),
            "{w}x{hgt}: the timeline hint ({hint:?}) runs into the overlay toggle ({toggle:?})"
        );
    }
}
