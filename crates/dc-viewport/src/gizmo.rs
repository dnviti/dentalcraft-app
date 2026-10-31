//! Axis gizmo drawn with the egui painter in a corner of the viewport.
//! Clicking an axis end switches to the view looking from that side.

use egui::{Align2, Color32, FontId, Pos2, Stroke, Vec2};
use glam::{Quat, Vec3};

use crate::camera::StdView;

/// Radius of the gizmo in points.
pub(crate) const RADIUS: f32 = 34.0;
const END_RADIUS: f32 = 8.0;

struct AxisEnd {
    pos: Pos2,
    depth: f32,
    color: Color32,
    label: &'static str,
    positive: bool,
    view: StdView,
}

fn ends(center: Pos2, rotation: Quat) -> Vec<AxisEnd> {
    let inv = rotation.inverse();
    let axes = [
        (Vec3::X, Color32::from_rgb(220, 70, 70), "X", StdView::Left),
        (Vec3::Y, Color32::from_rgb(90, 180, 80), "Y", StdView::Back),
        (Vec3::Z, Color32::from_rgb(70, 120, 230), "Z", StdView::Top),
        (
            Vec3::NEG_X,
            Color32::from_rgb(220, 70, 70),
            "",
            StdView::Right,
        ),
        (
            Vec3::NEG_Y,
            Color32::from_rgb(90, 180, 80),
            "",
            StdView::Front,
        ),
        (
            Vec3::NEG_Z,
            Color32::from_rgb(70, 120, 230),
            "",
            StdView::OcclusalUpper,
        ),
    ];
    let mut out: Vec<AxisEnd> = axes
        .into_iter()
        .map(|(axis, color, label, view)| {
            let v = inv * axis;
            AxisEnd {
                pos: center + Vec2::new(v.x, -v.y) * (RADIUS - END_RADIUS),
                depth: v.z,
                color,
                label,
                positive: !label.is_empty(),
                view,
            }
        })
        .collect();
    // back to front
    out.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    out
}

/// Draws the gizmo centred at `center`.
pub(crate) fn paint(painter: &egui::Painter, center: Pos2, rotation: Quat, hover: Option<Pos2>) {
    painter.circle_filled(center, RADIUS, Color32::from_black_alpha(40));
    for e in ends(center, rotation) {
        let hovered = hover.is_some_and(|h| h.distance(e.pos) <= END_RADIUS);
        if e.positive {
            painter.line_segment([center, e.pos], Stroke::new(2.0, e.color));
            painter.circle_filled(e.pos, END_RADIUS, e.color);
            painter.text(
                e.pos,
                Align2::CENTER_CENTER,
                e.label,
                FontId::proportional(10.0),
                Color32::WHITE,
            );
        } else {
            painter.circle(
                e.pos,
                END_RADIUS * 0.75,
                e.color.gamma_multiply(0.35),
                Stroke::new(1.5, e.color),
            );
        }
        if hovered {
            painter.circle_stroke(e.pos, END_RADIUS + 2.0, Stroke::new(1.5, Color32::WHITE));
        }
    }
}

/// The view of the axis end under `pos`, front-most first.
pub(crate) fn hit(center: Pos2, rotation: Quat, pos: Pos2) -> Option<StdView> {
    ends(center, rotation)
        .into_iter()
        .rev()
        .find(|e| pos.distance(e.pos) <= END_RADIUS)
        .map(|e| e.view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_view_axes_point_the_right_way() {
        let c = Pos2::new(100.0, 100.0);
        let e = ends(c, StdView::Front.rotation());
        let find = |l: &str| e.iter().find(|x| x.label == l).unwrap().pos;
        assert!(find("X").x > c.x, "X to the right in the front view");
        assert!(find("Z").y < c.y, "Z up in the front view");
        // the −Y end points at the viewer and is clickable for the front view
        let toward = e.last().unwrap();
        assert_eq!(toward.view, StdView::Front);
        assert_eq!(
            hit(c, StdView::Front.rotation(), find("X")),
            Some(StdView::Left)
        );
        assert_eq!(hit(c, StdView::Front.rotation(), Pos2::new(0.0, 0.0)), None);
    }
}
