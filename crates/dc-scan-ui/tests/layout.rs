//! The scan window fits the screen: nothing of its steps column runs past
//! its right edge (seen on Windows at 1600x960, in Italian: the turntable's
//! state badge, the values of the sliders and the cloud settings cut off).
//! Own test binary: the language is process-wide.

use std::time::{Duration, Instant};

use dc_case::ScanRole;
use dc_scan_ui::{ScanPopup, ScanTarget};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(600);

/// Runs the scan command `id` as an agent does and waits for its answer.
fn run(popup: &mut ScanPopup, id: &str, params: Value) {
    let ticket = popup.start_command(id, params).unwrap();
    let start = Instant::now();
    loop {
        popup.poll();
        if let Some(r) = popup.take_result(ticket) {
            r.unwrap_or_else(|e| panic!("{id}: {e}"));
            return;
        }
        assert!(start.elapsed() < WAIT, "{id} did not end");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn harness(size: egui::Vec2, popup: ScanPopup) -> Harness<'static, ScanPopup> {
    Harness::builder()
        .with_size(size)
        .with_context_setup(|ctx| dc_ui::theme::install(ctx, dc_ui::Mode::Dark))
        // this test is about the layout; the open window's device pickers
        // have no accessible name of their own yet
        .with_accessibility_check(false)
        .build_ui_state(|ui, popup: &mut ScanPopup| popup.show(ui), popup)
}

/// Draws until the window's own commands ended, then checks that every
/// widget ends left of the window's right edge (where its close button
/// ends).
fn assert_fits(h: &mut Harness<'static, ScanPopup>, size: egui::Vec2, what: &str) {
    let start = Instant::now();
    while h.state().is_busy() {
        assert!(start.elapsed() < WAIT, "{what}: the window stays busy");
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..4 {
        h.step();
    }
    let edge = h.get_by_label(&dc_i18n::tr("scan-close")).rect().max.x;
    assert!(
        edge <= size.x,
        "{what}: the window is wider than the screen"
    );
    let mut checked = 0;
    for node in h.root().children_recursive() {
        // the containers (screen, window, panels) are not widgets; some
        // nodes have no bounds
        if node.accesskit_node().bounding_box().is_none() {
            continue;
        }
        let r = node.rect();
        if r.width() <= 0.0 || r.width() >= 0.5 * size.x {
            continue;
        }
        checked += 1;
        assert!(
            r.max.x <= edge + 1.0,
            "{what}: {:?} {:?} ends at x {} past the window's edge {edge}",
            node.accesskit_node().role(),
            node.accesskit_node().label(),
            r.max.x,
        );
    }
    assert!(checked > 20, "{what}: only {checked} widgets");
}

#[test]
fn the_scan_window_fits_the_screen_in_italian() {
    dc_i18n::set_language(dc_i18n::Lang::It).unwrap();
    for (w, hgt) in [(1600.0, 960.0), (1280.0, 800.0)] {
        let size = egui::vec2(w, hgt);
        // nothing connected (the window as it opens)
        let mut popup = ScanPopup::new(None);
        popup.open(ScanTarget::new([ScanRole::Upper]).with_context("Nuovo progetto PT-7"));
        let mut h = harness(size, popup);
        assert_fits(&mut h, size, &format!("{w}x{hgt} nothing connected"));
        // a scanner and a turntable connected: every control of the table
        let mut popup = ScanPopup::new(None);
        popup.open(ScanTarget::new([ScanRole::Upper]));
        run(
            &mut popup,
            "scan.connect",
            json!({"backend": "sim", "options": {"realtime": false, "frames": 4, "width": 64, "height": 40}}),
        );
        run(
            &mut popup,
            "turntable.connect",
            json!({"backend": "sim", "options": {"fast": true}}),
        );
        let mut h = harness(size, popup);
        assert_fits(&mut h, size, &format!("{w}x{hgt} connected"));
    }
}
