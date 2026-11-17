//! Rectangle selection of scan points and the cutting level, projected with
//! the camera of the popup's 3D view.

use dc_viewport::{Camera, Sphere, Viewport};
use egui::{Pos2, Rect, pos2};
use glam::{Mat4, Vec3};

/// The world-to-clip transform the viewport draws `view` with (its bounds
/// set the clip planes).
pub(crate) fn view_proj(vp: &Viewport, view: Rect) -> Mat4 {
    projection(vp.camera(), vp.bounds(), view)
}

fn projection(camera: &Camera, bounds: Option<(Vec3, Vec3)>, view: Rect) -> Mat4 {
    let sphere = bounds.map(|(lo, hi)| Sphere {
        center: (lo + hi) * 0.5,
        radius: (hi - lo).length() * 0.5,
    });
    camera.view_proj(view.width() / view.height().max(1.0), sphere)
}

/// Screen position in `view` of world point `p`; `None` behind the camera.
pub(crate) fn screen(view_proj: Mat4, view: Rect, p: Vec3) -> Option<Pos2> {
    let c = view_proj * p.extend(1.0);
    if c.w <= f32::EPSILON {
        return None;
    }
    let n = c.truncate() / c.w;
    Some(pos2(
        view.left() + (n.x * 0.5 + 0.5) * view.width(),
        view.top() + (0.5 - n.y * 0.5) * view.height(),
    ))
}

/// Indices of the `points` whose projection falls inside `sel` (every depth:
/// the selection goes through the scan).
pub(crate) fn in_rect(points: &[Vec3], view_proj: Mat4, view: Rect, sel: Rect) -> Vec<u32> {
    points
        .iter()
        .enumerate()
        .filter_map(|(i, &p)| {
            screen(view_proj, view, p)
                .filter(|s| sel.contains(*s))
                .and_then(|_| u32::try_from(i).ok())
        })
        .collect()
}

/// The cutting level: the plane through the centre of the box `(lo, hi)`
/// moved `offset_mm` towards the top of the screen, facing the bottom of the
/// screen (what lies under it on screen is deleted). Returns its point and
/// its normal.
pub(crate) fn cut_plane(camera: &Camera, lo: Vec3, hi: Vec3, offset_mm: f32) -> (Vec3, Vec3) {
    let up = camera.up();
    ((lo + hi) * 0.5 + up * offset_mm, -up)
}

/// Corners of the square of half side `half` that draws the plane through
/// `point` facing `-camera.up()` (along the screen's right and depth).
pub(crate) fn cut_square(camera: &Camera, point: Vec3, half: f32) -> [Vec3; 4] {
    let (r, f) = (camera.right() * half, camera.forward() * half);
    [point - r - f, point + r - f, point + r + f, point - r + f]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rectangle_keeps_the_points_it_covers() {
        let camera = Camera::default();
        let bounds = Some((Vec3::splat(-10.0), Vec3::splat(10.0)));
        let view = Rect::from_min_size(pos2(100.0, 50.0), egui::vec2(800.0, 600.0));
        let vp = projection(&camera, bounds, view);
        // the orbit centre is drawn at the centre of the view
        let centre = screen(vp, view, Vec3::ZERO).unwrap();
        assert!((centre - view.center()).length() < 0.5, "{centre:?}");
        let right = camera.right() * 5.0;
        let points = [Vec3::ZERO, right, -right, camera.up() * 50.0];
        let sel = Rect::from_center_size(view.center(), egui::vec2(20.0, 20.0));
        assert_eq!(in_rect(&points, vp, view, sel), [0]);
        // a wider rectangle on the right side takes the right point too
        let sel = Rect::from_min_max(pos2(view.center().x - 5.0, 0.0), pos2(2000.0, 2000.0));
        assert_eq!(in_rect(&points, vp, view, sel), [0, 1]);
    }

    #[test]
    fn the_cut_faces_the_bottom_of_the_screen() {
        let camera = Camera::default();
        let (p, n) = cut_plane(&camera, Vec3::splat(-1.0), Vec3::splat(1.0), 2.0);
        assert!((p - camera.up() * 2.0).length() < 1e-5);
        assert!((n + camera.up()).length() < 1e-5);
        for c in cut_square(&camera, p, 3.0) {
            assert!((c - p).dot(n).abs() < 1e-4, "corners lie on the plane");
        }
    }
}
