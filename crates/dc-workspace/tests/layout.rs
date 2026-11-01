//! The centre area keeps its overlays and the timeline apart at every size
//! (English labels; the Italian ones are checked in `italian.rs`).

mod common;

use common::{Labels, assert_layout};

#[test]
fn toolbar_and_timeline_hold_at_every_width() {
    assert_layout(&Labels {
        views: ["Occlusal", "Front", "Left", "Right", "Back"],
        fit: "Fit to view",
        hint: "No stages yet",
        toggles: ["Upper", "Lower", "Overlay stage 0"],
    });
}
