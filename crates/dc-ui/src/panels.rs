//! Side panels the user resizes by dragging their inner edge, and panel
//! contents that never move the panel's edge.
//!
//! An egui panel whose contents grow wider than the panel reports a rect
//! moved by the overflow (a right panel anchors its rect at the far end of
//! the contents), so the central area, and the 3D view in it, slid under
//! the panel by however much the inspector of the moment overflowed.
//! [`contained`] runs the contents in a child `Ui` that does not size the
//! panel: the panel keeps exactly the width it was given and anything wider
//! is clipped at its edge.

use egui::{Frame, Id, Rangef, Rect, Ui, UiBuilder};

/// Which edge of the window a [`SidePanel`] sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Left edge; the user drags its right edge.
    Left,
    /// Right edge; the user drags its left edge.
    Right,
}

/// What a [`SidePanel`] drew.
#[derive(Debug, Clone, Copy)]
pub struct SidePanelResponse<R> {
    /// What the contents returned.
    pub inner: R,
    /// The panel (frame included) as drawn this frame.
    pub rect: Rect,
    /// How far the contents run past the panel's inner width (clipped).
    pub overflow: f32,
    /// The user finished a drag of the edge, or reset the width with a
    /// double-click: the width is worth saving.
    pub resized: bool,
}

/// A side panel the user resizes by dragging its inner edge; a
/// double-click on the edge goes back to the default width.
///
/// The caller keeps the width the user chose and passes it every frame:
/// the panel shows that width clamped to its range and to the room the
/// window leaves (a narrower window does not forget the choice).
#[derive(Debug, Clone)]
pub struct SidePanel {
    id: &'static str,
    side: Side,
    range: Rangef,
    default: f32,
    reserve: f32,
    frame: Frame,
}

impl SidePanel {
    /// A panel on `side` with egui id salt `id`, `default` wide, that the
    /// user resizes within `range`.
    pub fn new(side: Side, id: &'static str, default: f32, range: impl Into<Rangef>) -> Self {
        Self {
            id,
            side,
            range: range.into(),
            default,
            reserve: 0.0,
            frame: Frame::new(),
        }
    }

    /// Width the panel leaves free in the `Ui` it is shown in (the panels
    /// shown after it, the central area).
    pub fn reserve(mut self, width: f32) -> Self {
        self.reserve = width;
        self
    }

    /// Frame of the panel.
    pub fn frame(mut self, frame: Frame) -> Self {
        self.frame = frame;
        self
    }

    /// The width shown for the chosen `width` when the `Ui` has `available`
    /// width: within the range, leaving the reserve free when it can.
    pub fn shown_width(&self, width: f32, available: f32) -> f32 {
        clamp_width(width, self.range, available - self.reserve)
    }

    /// Shows the panel in `ui`; `width` is the width the user chose (it
    /// follows a drag of the edge and goes back to the default on a
    /// double-click).
    pub fn show<R>(
        self,
        ui: &mut Ui,
        width: &mut f32,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> SidePanelResponse<R> {
        let available = ui.available_width();
        let max = clamp_width(self.range.max, self.range, available - self.reserve);
        // the edge is the panel's resize handle: egui's id for it
        let handle = ui.ctx().read_response(resize_handle_id(ui, self.id));
        let dragging = handle
            .as_ref()
            .is_some_and(|r| r.dragged() || r.drag_stopped());
        let drag_stopped = handle.as_ref().is_some_and(egui::Response::drag_stopped);
        let mut resized = false;
        if handle.as_ref().is_some_and(egui::Response::double_clicked) {
            *width = self.default;
            resized = true;
        }
        let shown = self.shown_width(*width, available);
        let panel = match self.side {
            Side::Left => egui::Panel::left(self.id),
            Side::Right => egui::Panel::right(self.id),
        }
        .resizable(true)
        .frame(self.frame);
        // the panel follows the pointer only while the user drags its edge;
        // otherwise it is exactly the chosen width
        let panel = if dragging {
            panel
                .size_range(self.range.min.min(max)..=max)
                .default_size(shown)
        } else {
            panel.exact_size(shown)
        };
        let r = panel.show(ui, |ui| contained(ui, add_contents));
        let rect = r.response.rect;
        if dragging {
            *width = rect.width();
            resized |= drag_stopped;
        }
        let (inner, overflow) = r.inner;
        SidePanelResponse {
            inner,
            rect,
            overflow,
            resized,
        }
    }
}

/// `width` within `range` and at most `room`, never under the range's
/// minimum (the window's minimum size leaves room for it).
pub fn clamp_width(width: f32, range: Rangef, room: f32) -> f32 {
    let max = range.max.min(room).max(range.min);
    if width.is_finite() {
        width.clamp(range.min, max)
    } else {
        range.min
    }
}

/// The id egui gives the resize handle of the panel `id` shown in `ui`
/// (`egui::Panel`: the panel's id `.with("__resize")`).
fn resize_handle_id(ui: &Ui, id: &'static str) -> Id {
    ui.scope_id().with(id).with("__resize")
}

/// Runs `add_contents` in a child of `ui` that does not size `ui`: what
/// they draw past `ui`'s width is clipped instead of moving the panel's
/// edge. `ui` itself takes its whole `max_rect`, so the panel is exactly
/// the size it was given whatever the contents. Returns what they returned
/// and how far they ran past the width.
pub fn contained<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    let max = ui.max_rect();
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt("dc-contained")
            .max_rect(max)
            .layout(*ui.layout()),
    );
    let inner = add_contents(&mut child);
    let overflow = (child.min_rect().right() - max.right()).max(0.0);
    ui.expand_to_include_rect(max);
    (inner, overflow)
}

/// Runs `add_contents` as the next block of a top-down `ui` without letting
/// it widen `ui`: the block takes `ui`'s available width and the height
/// its contents use; what they draw past the width is clipped by the panel
/// and the blocks after it keep the panel's width. Use it around contents
/// this crate's caller does not lay out itself (a module's inspector).
/// Returns what they returned and how far they ran past the width.
pub fn fit_width<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> (R, f32) {
    let room = ui.available_rect_before_wrap();
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt("dc-fit-width")
            .max_rect(room)
            .layout(*ui.layout()),
    );
    let inner = add_contents(&mut child);
    let used = child.min_rect();
    let overflow = (used.right() - room.right()).max(0.0);
    ui.advance_cursor_after_rect(Rect::from_min_size(
        room.min,
        egui::vec2(room.width(), used.height()),
    ));
    (inner, overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn widths_stay_in_the_range_and_the_room() {
        let range = Rangef::new(200.0, 400.0);
        assert_eq!(clamp_width(300.0, range, 1000.0), 300.0);
        assert_eq!(clamp_width(100.0, range, 1000.0), 200.0);
        assert_eq!(clamp_width(900.0, range, 1000.0), 400.0);
        // a narrow window: the room wins over the maximum, not the minimum
        assert_eq!(clamp_width(390.0, range, 250.0), 250.0);
        assert_eq!(clamp_width(390.0, range, 120.0), 200.0);
        assert_eq!(clamp_width(f32::NAN, range, 1000.0), 200.0);
    }

    /// What the test window drew: the chosen width, the panel, the central
    /// area, the overflow.
    #[derive(Debug, Clone, Copy)]
    struct Drawn {
        width: f32,
        panel: Rect,
        central: Rect,
        overflow: f32,
    }

    /// A right panel with contents `wide` wide, and a central area.
    fn harness(width: f32, wide: f32) -> Harness<'static, Drawn> {
        Harness::builder()
            .with_size(vec2(1000.0, 600.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                move |ui, d: &mut Drawn| {
                    let r = SidePanel::new(Side::Right, "right", 300.0, 200.0..=500.0).show(
                        ui,
                        &mut d.width,
                        |ui| {
                            ui.label("Panel");
                            ui.add_sized(vec2(wide, 20.0), egui::Label::new("wide"));
                        },
                    );
                    d.panel = r.rect;
                    d.overflow = r.overflow;
                    egui::CentralPanel::no_frame().show(ui, |ui| {
                        d.central = ui.max_rect();
                    });
                },
                Drawn {
                    width,
                    panel: Rect::NOTHING,
                    central: Rect::NOTHING,
                    overflow: 0.0,
                },
            )
    }

    fn button(h: &Harness<'static, Drawn>, pos: egui::Pos2, pressed: bool) {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }

    /// Drags the panel's edge by `dx`.
    fn drag_edge(h: &mut Harness<'static, Drawn>, dx: f32) {
        let d = *h.state();
        let from = egui::pos2(d.panel.left(), d.panel.center().y);
        h.hover_at(from);
        h.run();
        button(h, from, true);
        h.run();
        for k in 1..=4 {
            h.hover_at(from + vec2(dx * k as f32 / 4.0, 0.0));
            h.run();
        }
        button(h, from + vec2(dx, 0.0), false);
        h.run();
    }

    #[test]
    fn wide_contents_never_move_the_edge() {
        for wide in [100.0, 300.0, 700.0] {
            let mut h = harness(300.0, wide);
            h.run();
            let d = *h.state();
            assert_eq!(d.width, 300.0);
            assert_eq!(d.panel.width(), 300.0, "contents {wide} wide");
            assert_eq!(d.central.right(), d.panel.left(), "contents {wide} wide");
            assert!(d.overflow >= wide - 300.0, "contents {wide} wide: {d:?}");
            assert!(h.get_by_label("Panel").rect().left() >= d.panel.left());
        }
    }

    #[test]
    fn dragging_the_edge_resizes_within_the_range() {
        let mut h = harness(300.0, 10.0);
        h.run();
        let right = h.state().panel.right();
        drag_edge(&mut h, -100.0);
        let d = *h.state();
        assert!((d.width - 400.0).abs() < 1.0, "dragged to 400: {d:?}");
        assert_eq!(d.panel.width(), d.width);
        assert_eq!(d.panel.right(), right);
        assert_eq!(d.central.right(), d.panel.left());
        // past the maximum: stops at 500
        drag_edge(&mut h, -400.0);
        let d = *h.state();
        assert_eq!(d.width, 500.0, "{d:?}");
        assert_eq!(d.central.right(), d.panel.left());
        // past the minimum: stops at 200
        drag_edge(&mut h, 450.0);
        let d = *h.state();
        assert_eq!(d.width, 200.0, "{d:?}");
        assert_eq!(d.central.right(), d.panel.left());
    }

    #[test]
    fn a_double_click_on_the_edge_restores_the_default() {
        let mut h = harness(450.0, 10.0);
        h.run();
        let d = *h.state();
        assert_eq!(d.panel.width(), 450.0);
        let edge = egui::pos2(d.panel.left(), d.panel.center().y);
        h.hover_at(edge);
        h.run();
        for _ in 0..2 {
            button(&h, edge, true);
            h.step();
            button(&h, edge, false);
            h.step();
        }
        h.run();
        let d = *h.state();
        assert_eq!(d.width, 300.0, "{d:?}");
        assert_eq!(d.panel.width(), 300.0);
        assert_eq!(d.central.right(), d.panel.left());
    }
}
