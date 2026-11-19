//! The side panels: the user resizes them by dragging their inner edge,
//! they stay within their ranges, and the 3D view fills exactly the room
//! they leave, at every window size.

use dc_workspace::{PanelWidths, Workspace, WorkspaceLayout};
use dca_module::AlignersModule;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// The workspace in a window of `size`, with the panels `widths` wide.
fn harness(size: Vec2, widths: Option<PanelWidths>) -> Harness<'static, Workspace> {
    let mut ws = Workspace::new_headless(Box::new(AlignersModule::new()), None, None);
    if let Some(w) = widths {
        ws.set_panel_widths(w);
    }
    let mut h = Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        .build_ui_state(
            |ui, ws: &mut Workspace| {
                let _ = ws.show(ui);
            },
            ws,
        );
    h.run();
    h
}

fn layout(h: &Harness<'static, Workspace>) -> WorkspaceLayout {
    h.state().layout()
}

/// The view touches both panels, does not overlap them, and keeps the
/// whole height between the top bar and the timeline.
fn assert_view_fills_the_centre(l: &WorkspaceLayout, what: &str) {
    assert_eq!(l.view.left(), l.menu.right(), "{what}: {l:?}");
    assert_eq!(l.view.right(), l.inspector.left(), "{what}: {l:?}");
    assert!(!l.view.intersects(l.menu.shrink(0.5)), "{what}: {l:?}");
    assert!(!l.view.intersects(l.inspector.shrink(0.5)), "{what}: {l:?}");
    assert!(
        l.view.width() > 0.0 && l.view.height() > 0.0,
        "{what}: {l:?}"
    );
}

fn button(h: &Harness<'static, Workspace>, pos: Pos2, pressed: bool) {
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// Drags from `from` by `dx`, in small steps, as a hand does.
fn drag(h: &mut Harness<'static, Workspace>, from: Pos2, dx: f32) {
    h.hover_at(from);
    h.run();
    button(h, from, true);
    h.run();
    for k in 1..=6 {
        h.hover_at(from + vec2(dx * k as f32 / 6.0, 0.0));
        h.run();
    }
    button(h, from + vec2(dx, 0.0), false);
    h.run();
}

fn edge_of(r: Rect, left_edge: bool) -> Pos2 {
    pos2(if left_edge { r.left() } else { r.right() }, r.center().y)
}

#[test]
fn the_view_fills_the_room_the_panels_leave_at_every_size() {
    let sz = dc_ui::tokens::tokens().size;
    for (w, hgt) in [(1920.0, 1080.0), (1600.0, 960.0), (1280.0, 800.0)] {
        let h = harness(vec2(w, hgt), None);
        let l = layout(&h);
        assert_view_fills_the_centre(&l, &format!("{w}x{hgt}"));
        assert_eq!(l.menu.width(), sz.left_menu_width, "{w}x{hgt}");
        assert_eq!(l.inspector.width(), sz.inspector_width, "{w}x{hgt}");
        // the inspector sits at the right edge of the window (the test
        // harness keeps a small margin around its root)
        assert!(w - l.inspector.right() <= 8.0, "{w}x{hgt}: {l:?}");
    }
}

#[test]
fn chosen_widths_stay_in_their_ranges_and_leave_the_view_its_room() {
    let sz = dc_ui::tokens::tokens().size;
    // wider than the maximum, narrower than the minimum
    for (asked, menu, inspector) in [
        (
            (5000.0, 5000.0),
            sz.left_menu_max_width,
            sz.inspector_max_width,
        ),
        ((10.0, 10.0), sz.left_menu_min_width, sz.inspector_min_width),
        ((350.0, 450.0), 350.0, 450.0),
    ] {
        let widths = PanelWidths {
            menu: asked.0,
            inspector: asked.1,
        };
        let h = harness(vec2(1920.0, 1080.0), Some(widths));
        let l = layout(&h);
        assert_eq!(l.menu.width(), menu, "{asked:?}");
        assert_eq!(l.inspector.width(), inspector, "{asked:?}");
        assert_view_fills_the_centre(&l, &format!("{asked:?}"));
    }
    // both panels at their maximum in the smallest window: the menu gives
    // way first, the view keeps its minimum width
    let widths = PanelWidths {
        menu: sz.left_menu_max_width,
        inspector: sz.inspector_max_width,
    };
    let h = harness(vec2(1280.0, 800.0), Some(widths));
    let l = layout(&h);
    assert_view_fills_the_centre(&l, "1280x800 at the maximum");
    assert!(l.view.width() >= sz.viewport_min_width - 0.5, "{l:?}");
    assert!(l.menu.width() >= sz.left_menu_min_width, "{l:?}");
    // the timeline under the narrowest view keeps every control in it: the
    // overlay toggle drops its label but stays (its name is the label)
    let toggle = h
        .get_all_by_label("Overlay stage 0")
        .map(|n| n.rect())
        .max_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the overlay toggle");
    assert!(
        toggle.left() >= l.view.left() && toggle.right() <= l.view.right(),
        "{toggle:?} is cut by {l:?}"
    );
    // the choice is kept for a larger window
    assert_eq!(h.state().panel_widths(), widths);
}

#[test]
fn dragging_an_edge_resizes_the_panel_and_the_view_follows() {
    let sz = dc_ui::tokens::tokens().size;
    let mut h = harness(vec2(1600.0, 960.0), None);
    let before = layout(&h);
    // the inspector: its left edge, 100 to the left
    drag(&mut h, edge_of(before.inspector, true), -100.0);
    let l = layout(&h);
    assert!(
        (l.inspector.width() - (sz.inspector_width + 100.0)).abs() <= 1.0,
        "{l:?}"
    );
    assert_view_fills_the_centre(&l, "after the inspector drag");
    let saved = h.state_mut().take_resized_panels().expect("resized");
    assert_eq!(saved.inspector, l.inspector.width());
    assert_eq!(h.state_mut().take_resized_panels(), None);
    // the menu: its right edge, 60 to the right
    drag(&mut h, edge_of(l.menu, false), 60.0);
    let l = layout(&h);
    assert!(
        (l.menu.width() - (sz.left_menu_width + 60.0)).abs() <= 1.0,
        "{l:?}"
    );
    assert_view_fills_the_centre(&l, "after the menu drag");
    // far past the maximum: the menu stops at it
    drag(&mut h, edge_of(l.menu, false), 900.0);
    let l = layout(&h);
    assert_eq!(l.menu.width(), sz.left_menu_max_width, "{l:?}");
    assert_view_fills_the_centre(&l, "after the long drag");
    assert_eq!(h.state().panel_widths().menu, sz.left_menu_max_width);
}

#[test]
fn a_double_click_on_an_edge_restores_the_default_width() {
    let sz = dc_ui::tokens::tokens().size;
    let widths = PanelWidths {
        menu: 400.0,
        inspector: 500.0,
    };
    let mut h = harness(vec2(1600.0, 960.0), Some(widths));
    let edge = edge_of(layout(&h).inspector, true);
    h.hover_at(edge);
    h.run();
    for _ in 0..2 {
        button(&h, edge, true);
        h.step();
        button(&h, edge, false);
        h.step();
    }
    h.run();
    let l = layout(&h);
    assert_eq!(l.inspector.width(), sz.inspector_width, "{l:?}");
    assert_eq!(l.menu.width(), 400.0);
    assert_view_fills_the_centre(&l, "after the reset");
    assert_eq!(
        h.state_mut().take_resized_panels(),
        Some(PanelWidths {
            menu: 400.0,
            inspector: sz.inspector_width
        })
    );
}
