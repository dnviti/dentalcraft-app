//! Shared checks of the layout of the centre area at several window sizes.

use dc_workspace::Workspace;
use dca_module::AlignersModule;
use egui::{Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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
    /// The three jaw toggles of the timeline, left to right.
    pub toggles: [&'static str; 3],
}

fn rect_of(h: &Harness<'_, ()>, label: &str) -> Rect {
    h.get_by_label(label).rect()
}

/// The toolbar stays left of the view cube and the timeline hint stays left
/// of the jaw toggles, which keep their own room, at every size.
pub fn assert_layout(labels: &Labels) {
    for (w, hgt) in SIZES {
        let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
        let mut h = Harness::builder()
            .with_size(vec2(w, hgt))
            .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
            .build_ui(move |ui| {
                let _ = ws.show(ui);
            });
        h.run();
        let size = dc_ui::tokens::tokens().size;
        let centre_right = w - size.inspector_width;
        // the cube is 100 wide at the top right of the centre area, 16 pt in
        let cube_left = centre_right - 16.0 - 100.0;
        let mut widgets: Vec<Rect> = labels.views.iter().map(|l| rect_of(&h, l)).collect();
        widgets.push(rect_of(&h, labels.fit));
        for r in &widgets {
            assert!(
                r.right() <= cube_left,
                "{w}x{hgt}: a toolbar widget ({r:?}) runs under the view cube (left edge {cube_left})"
            );
            assert!(
                r.left() >= size.left_menu_width,
                "{w}x{hgt}: toolbar widget left of the centre area: {r:?}"
            );
        }
        // the toggles keep their room, left to right, without touching
        let toggles: Vec<Rect> = labels
            .toggles
            .iter()
            .map(|l| {
                // the timeline one is the lowest (the inspector may use the same word)
                h.get_all_by_label(l)
                    .map(|n| n.rect())
                    .max_by(|a, b| a.top().total_cmp(&b.top()))
                    .unwrap()
            })
            .collect();
        for pair in toggles.windows(2) {
            assert!(
                pair[0].right() <= pair[1].left(),
                "{w}x{hgt}: jaw toggles overlap: {pair:?}"
            );
        }
        assert!(
            toggles[2].right() <= centre_right,
            "{w}x{hgt}: the toggles leave the centre area"
        );
        let hint = h.get_by_label_contains(labels.hint).rect();
        assert!(
            hint.right() <= centre_right,
            "{w}x{hgt}: the timeline hint leaves the centre area: {hint:?}"
        );
        for t in &toggles {
            assert!(
                !hint.intersects(*t),
                "{w}x{hgt}: the timeline hint ({hint:?}) runs into a toggle ({t:?})"
            );
        }
    }
}
