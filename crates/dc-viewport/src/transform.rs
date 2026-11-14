//! The transform gizmo of a selected object: translate arrows along the world
//! axes with plane squares and a free handle, rotation rings about the world
//! axes, and a uniform scale circle. It is drawn with the egui painter over
//! the 3D image ([`crate::Viewport::paint_transform_gizmo`]) and dragged with
//! the primary button ([`crate::Viewport::ui`] reports the drag as
//! [`GizmoEvent`]s). Its handles keep their size on the screen whatever the
//! zoom.
//!
//! The geometry is in world coordinates and goes through a [`Projector`]
//! (camera and viewport rectangle), so everything here is tested without a
//! GPU.

use std::f32::consts::TAU;

use egui::{Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2 as EVec2};
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::camera::{Camera, Projection, Ray, Sphere};

/// What the gizmo does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GizmoMode {
    /// Move: arrows along the axes, squares in the planes, a free handle in
    /// the plane of the screen.
    #[default]
    Translate,
    /// Turn: rings about the axes.
    Rotate,
    /// Uniform scale: a circle whose distance to the centre gives the factor.
    Scale,
}

/// The gizmo to show for the selected object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformGizmo {
    /// Centre of the gizmo, of its rotations and of its scale (world, mm).
    pub pivot: Vec3,
    /// What it does.
    pub mode: GizmoMode,
}

/// The change a drag gives, about `pivot`: `T(pivot + translation) · R ·
/// S · T(−pivot)` (world).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoDelta {
    /// Centre of the rotation and of the scale (world, mm).
    pub pivot: Vec3,
    /// Translation (world, mm).
    pub translation: Vec3,
    /// Rotation about the pivot.
    pub rotation: Quat,
    /// Uniform scale about the pivot.
    pub scale: f32,
}

impl GizmoDelta {
    /// No change, about `pivot`.
    pub fn identity(pivot: Vec3) -> Self {
        Self {
            pivot,
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: 1.0,
        }
    }

    /// The change as a world matrix.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_translation(self.pivot + self.translation)
            * Mat4::from_quat(self.rotation)
            * Mat4::from_scale(Vec3::splat(self.scale))
            * Mat4::from_translation(-self.pivot)
    }

    /// `true` when it changes nothing.
    pub fn is_identity(&self) -> bool {
        self.translation == Vec3::ZERO && self.rotation == Quat::IDENTITY && self.scale == 1.0
    }
}

/// What the gizmo did this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoEvent {
    /// A handle is being dragged: the total change since the drag started
    /// (preview only).
    Drag(GizmoDelta),
    /// The handle was let go: the total change of the drag.
    Commit(GizmoDelta),
}

/// Length of the arrows on the screen (points).
pub(crate) const AXIS_LEN: f32 = 90.0;
/// Radius of the rotation rings on the screen (points).
const RING_RADIUS: f32 = 80.0;
/// Radius of the scale circle on the screen (points).
const SCALE_RADIUS: f32 = 70.0;
/// Radius of the free move handle (points).
const CENTER_RADIUS: f32 = 7.0;
/// How far from a handle the pointer may be to take it (points).
const HIT_TOLERANCE: f32 = 7.0;
/// Plane squares, as fractions of the arrow length.
const PLANE_FROM: f32 = 0.22;
const PLANE_TO: f32 = 0.42;
/// An arrow seen end on (shorter than this on the screen, points) is hidden.
const MIN_AXIS_PX: f32 = 18.0;
/// Segments of a ring.
const RING_SEGMENTS: usize = 72;

/// The world axes and their colours (as the axis gizmo of the corner).
const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];
const AXIS_COLORS: [Color32; 3] = [
    Color32::from_rgb(220, 70, 70),
    Color32::from_rgb(90, 180, 80),
    Color32::from_rgb(70, 120, 230),
];
const NEUTRAL: Color32 = Color32::from_rgb(235, 235, 240);

/// A part of the gizmo the pointer can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handle {
    /// Move along an axis.
    Axis(usize),
    /// Move in the plane normal to an axis.
    Plane(usize),
    /// Move in the plane of the screen.
    Screen,
    /// Turn about an axis.
    Ring(usize),
    /// Scale.
    Scale,
}

/// Maps world points to the screen and back, for a camera shown in `rect`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Projector {
    pub camera: Camera,
    pub rect: Rect,
    pub bounds: Option<Sphere>,
}

impl Projector {
    fn aspect(&self) -> f32 {
        self.rect.width().max(1.0) / self.rect.height().max(1.0)
    }

    /// Screen position (points) of a world point.
    pub fn screen(&self, p: Vec3) -> Pos2 {
        let ndc = self.camera.project_ndc(p, self.aspect(), self.bounds);
        Pos2::new(
            self.rect.left() + (ndc.x * 0.5 + 0.5) * self.rect.width(),
            self.rect.top() + (0.5 - ndc.y * 0.5) * self.rect.height(),
        )
    }

    /// The ray under a screen position.
    pub fn ray(&self, pos: Pos2) -> Ray {
        let ndc = Vec2::new(
            (pos.x - self.rect.left()) / self.rect.width().max(1.0) * 2.0 - 1.0,
            1.0 - (pos.y - self.rect.top()) / self.rect.height().max(1.0) * 2.0,
        );
        self.camera.ray(ndc, self.aspect(), self.bounds)
    }

    /// Millimetres per point at world point `p` (across the view).
    pub fn mm_per_point(&self, p: Vec3) -> f32 {
        let size = Vec2::new(self.rect.width(), self.rect.height());
        let px = self.camera.screen_length(p, 1.0, size, self.bounds);
        if px > 1e-6 { 1.0 / px } else { 1.0 }
    }

    /// Direction the viewer looks along at `p` (unit).
    pub fn view_dir(&self, p: Vec3) -> Vec3 {
        match self.camera.projection {
            Projection::Perspective => (p - self.camera.eye())
                .try_normalize()
                .unwrap_or(self.camera.forward()),
            Projection::Orthographic => self.camera.forward(),
        }
    }
}

/// The gizmo laid out on the screen.
struct Layout {
    center: Pos2,
    /// Screen ends of the arrows, `None` when seen end on.
    ends: [Option<Pos2>; 3],
    /// Screen corners of the plane squares, `None` when seen edge on.
    planes: [Option<[Pos2; 4]>; 3],
    /// Screen points of the rings and whether each one faces the viewer.
    rings: [Vec<(Pos2, bool)>; 3],
}

/// The two axes spanning the plane normal to axis `i`.
fn plane_axes(i: usize) -> (Vec3, Vec3) {
    (AXES[(i + 1) % 3], AXES[(i + 2) % 3])
}

fn layout(g: &TransformGizmo, proj: &Projector) -> Layout {
    let c = g.pivot;
    let center = proj.screen(c);
    let mmpp = proj.mm_per_point(c);
    let len = AXIS_LEN * mmpp;
    let view = proj.view_dir(c);
    let ends = AXES.map(|a| {
        let e = proj.screen(c + a * len);
        (e.distance(center) >= MIN_AXIS_PX).then_some(e)
    });
    let planes = [0, 1, 2].map(|i| {
        let n = AXES[i];
        if view.dot(n).abs() < 0.2 {
            return None;
        }
        let (u, v) = plane_axes(i);
        let (a, b) = (PLANE_FROM * len, PLANE_TO * len);
        Some([
            proj.screen(c + u * a + v * a),
            proj.screen(c + u * b + v * a),
            proj.screen(c + u * b + v * b),
            proj.screen(c + u * a + v * b),
        ])
    });
    let radius = RING_RADIUS * mmpp;
    let rings = [0, 1, 2].map(|i| {
        let (u, v) = plane_axes(i);
        (0..=RING_SEGMENTS)
            .map(|k| {
                let t = k as f32 / RING_SEGMENTS as f32 * TAU;
                let p = c + (u * t.cos() + v * t.sin()) * radius;
                // the near half of the ring faces the viewer
                (proj.screen(p), view.dot(p - c) <= 0.0)
            })
            .collect()
    });
    Layout {
        center,
        ends,
        planes,
        rings,
    }
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let cross = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// The handle under `pos`, the nearest first.
pub(crate) fn hit(g: &TransformGizmo, proj: &Projector, pos: Pos2) -> Option<Handle> {
    let l = layout(g, proj);
    match g.mode {
        GizmoMode::Translate => {
            if pos.distance(l.center) <= CENTER_RADIUS + 2.0 {
                return Some(Handle::Screen);
            }
            if let Some(i) = (0..3).find(|&i| l.planes[i].is_some_and(|q| in_quad(pos, &q))) {
                return Some(Handle::Plane(i));
            }
            (0..3)
                .filter_map(|i| {
                    let e = l.ends[i]?;
                    let d = segment_distance(pos, l.center, e);
                    (d <= HIT_TOLERANCE).then_some((d, i))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, i)| Handle::Axis(i))
        }
        GizmoMode::Rotate => (0..3)
            .filter_map(|i| {
                let ring = &l.rings[i];
                let d = ring
                    .windows(2)
                    .map(|w| {
                        // the near half wins over the far one
                        let far = if w[0].1 || w[1].1 { 0.0 } else { 2.0 };
                        segment_distance(pos, w[0].0, w[1].0) + far
                    })
                    .fold(f32::INFINITY, f32::min);
                (d <= HIT_TOLERANCE).then_some((d, i))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| Handle::Ring(i)),
        GizmoMode::Scale => {
            let r = pos.distance(l.center);
            ((r - SCALE_RADIUS).abs() <= HIT_TOLERANCE + 2.0 || r <= CENTER_RADIUS + 2.0)
                .then_some(Handle::Scale)
        }
    }
}

/// A drag of a handle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drag {
    pub handle: Handle,
    pivot: Vec3,
    /// Where the press was (points).
    start: Pos2,
    /// What the drag reads at the press: the parameter along the axis, the
    /// point in the plane, the vector from the pivot in the ring plane.
    anchor: Vec3,
    /// The ring is seen edge on: the turn follows the pointer along the
    /// ring's tangent on the screen instead of the ring's plane.
    edge_on: bool,
    /// Tangent of the ring on the screen at the press (unit) and points per
    /// radian there, for an edge-on ring.
    tangent: (EVec2, f32),
    /// The last change given.
    pub last: GizmoDelta,
}

/// Point of the plane through `c` with normal `n` under `ray`.
fn on_plane(ray: &Ray, c: Vec3, n: Vec3) -> Option<Vec3> {
    let den = ray.dir.dot(n);
    if den.abs() < 1e-4 {
        return None;
    }
    let t = (c - ray.origin).dot(n) / den;
    Some(ray.at(t))
}

/// Parameter along the line through `c` with direction `a` (unit) of the
/// point nearest to `ray`.
fn on_line(ray: &Ray, c: Vec3, a: Vec3) -> Option<f32> {
    let w = c - ray.origin;
    let b = a.dot(ray.dir);
    let den = 1.0 - b * b;
    if den < 1e-5 {
        return None;
    }
    Some((b * ray.dir.dot(w) - a.dot(w)) / den)
}

/// The normal of the plane a [`Handle::Plane`] or [`Handle::Screen`] moves in.
fn plane_normal(handle: Handle, proj: &Projector, c: Vec3) -> Vec3 {
    match handle {
        Handle::Plane(i) => AXES[i],
        _ => proj.view_dir(c),
    }
}

impl Drag {
    /// Starts dragging `handle`, pressed at `start`.
    pub fn start(handle: Handle, g: &TransformGizmo, proj: &Projector, start: Pos2) -> Self {
        let c = g.pivot;
        let ray = proj.ray(start);
        let mut drag = Self {
            handle,
            pivot: c,
            start,
            anchor: Vec3::ZERO,
            edge_on: false,
            tangent: (EVec2::X, 1.0),
            last: GizmoDelta::identity(c),
        };
        match handle {
            Handle::Axis(i) => {
                drag.anchor = Vec3::splat(on_line(&ray, c, AXES[i]).unwrap_or(0.0));
            }
            Handle::Plane(_) | Handle::Screen => {
                drag.anchor = on_plane(&ray, c, plane_normal(handle, proj, c)).unwrap_or(c);
            }
            Handle::Ring(i) => {
                let a = AXES[i];
                drag.edge_on = proj.view_dir(c).dot(a).abs() < 0.25;
                let (u, v) = plane_axes(i);
                let radius = RING_RADIUS * proj.mm_per_point(c);
                // the point of the ring nearest to the press, and its tangent
                let (q, dq) = (0..RING_SEGMENTS)
                    .map(|k| k as f32 / RING_SEGMENTS as f32 * TAU)
                    .map(|t| {
                        let q = c + (u * t.cos() + v * t.sin()) * radius;
                        let dq = (v * t.cos() - u * t.sin()) * radius;
                        (q, dq)
                    })
                    .min_by(|x, y| {
                        proj.screen(x.0)
                            .distance(start)
                            .total_cmp(&proj.screen(y.0).distance(start))
                    })
                    .unwrap_or((c + u * radius, v * radius));
                let step = 0.01;
                let s0 = proj.screen(q);
                let s1 = proj.screen(q + dq * step);
                let t = s1 - s0;
                let px_per_rad = (t.length() / step).max(1e-3);
                drag.tangent = (t.normalized(), px_per_rad);
                drag.anchor = on_plane(&ray, c, a).map_or(q - c, |h| h - c);
            }
            Handle::Scale => {}
        }
        drag
    }

    /// The change for the pointer at `pos`; `snap` rounds it (0.5 mm, 5°,
    /// 0.05).
    pub fn update(&mut self, proj: &Projector, pos: Pos2, snap: bool) -> GizmoDelta {
        let c = self.pivot;
        let ray = proj.ray(pos);
        let mut d = GizmoDelta::identity(c);
        match self.handle {
            Handle::Axis(i) => {
                if let Some(s) = on_line(&ray, c, AXES[i]) {
                    let mut t = s - self.anchor.x;
                    if snap {
                        t = (t / 0.5).round() * 0.5;
                    }
                    d.translation = AXES[i] * t;
                } else {
                    d = self.last;
                }
            }
            Handle::Plane(_) | Handle::Screen => {
                match on_plane(&ray, c, plane_normal(self.handle, proj, c)) {
                    Some(h) => {
                        let mut t = h - self.anchor;
                        if snap {
                            t = (t / 0.5).round() * 0.5;
                        }
                        d.translation = t;
                    }
                    None => d = self.last,
                }
            }
            Handle::Ring(i) => {
                let a = AXES[i];
                let angle = if self.edge_on {
                    Some((pos - self.start).dot(self.tangent.0) / self.tangent.1)
                } else {
                    on_plane(&ray, c, a).map(|h| {
                        let (v0, v1) = (self.anchor, h - c);
                        v0.cross(v1).dot(a).atan2(v0.dot(v1))
                    })
                };
                // a line of sight along the plane of the ring keeps the last turn as it
                // was (its angle read back from the quaternion would lose its sign)
                match angle {
                    Some(angle) => {
                        let angle = if snap {
                            let step = 5f32.to_radians();
                            (angle / step).round() * step
                        } else {
                            angle
                        };
                        d.rotation = Quat::from_axis_angle(a, angle);
                    }
                    None => d = self.last,
                }
            }
            Handle::Scale => {
                let center = proj.screen(c);
                let r0 = self.start.distance(center).max(4.0);
                let mut s = (pos.distance(center) / r0).clamp(0.05, 20.0);
                if snap {
                    s = ((s / 0.05).round() * 0.05).max(0.05);
                }
                d.scale = s;
            }
        }
        self.last = d;
        d
    }
}

/// Brighter colour of a handle under the pointer or dragged.
fn lit(c: Color32, on: bool) -> Color32 {
    if on {
        c.lerp_to_gamma(Color32::WHITE, 0.45)
    } else {
        c
    }
}

/// Draws the gizmo; `hot` is the handle under the pointer or dragged, and
/// `drag` the change of a drag going on (its value is written next to the
/// pointer at `pointer`).
pub(crate) fn paint(
    painter: &Painter,
    g: &TransformGizmo,
    proj: &Projector,
    hot: Option<Handle>,
    drag: Option<&GizmoDelta>,
    pointer: Option<Pos2>,
) {
    let l = layout(g, proj);
    let shadow = Color32::from_black_alpha(110);
    match g.mode {
        GizmoMode::Translate => {
            for (i, (plane, color)) in l.planes.iter().zip(AXIS_COLORS).enumerate() {
                if let Some(q) = plane {
                    let on = hot == Some(Handle::Plane(i));
                    let col = lit(color, on);
                    painter.add(Shape::convex_polygon(
                        q.to_vec(),
                        col.gamma_multiply(if on { 0.55 } else { 0.3 }),
                        Stroke::new(1.0, col),
                    ));
                }
            }
            // the arrows, far ones first
            let mut order: Vec<usize> = (0..3).collect();
            let view = proj.view_dir(g.pivot);
            order.sort_by(|a, b| view.dot(AXES[*b]).total_cmp(&view.dot(AXES[*a])));
            for i in order {
                let Some(e) = l.ends[i] else { continue };
                let on = hot == Some(Handle::Axis(i));
                let col = lit(AXIS_COLORS[i], on);
                let w = if on { 4.0 } else { 3.0 };
                painter.line_segment([l.center, e], Stroke::new(w + 2.0, shadow));
                painter.line_segment([l.center, e], Stroke::new(w, col));
                let dir = (e - l.center).normalized();
                let side = EVec2::new(-dir.y, dir.x);
                let tip = e + dir * 12.0;
                painter.add(Shape::convex_polygon(
                    vec![tip, e + side * 6.0, e - side * 6.0],
                    col,
                    Stroke::new(1.0, shadow),
                ));
            }
            let on = hot == Some(Handle::Screen);
            painter.circle(
                l.center,
                CENTER_RADIUS,
                lit(NEUTRAL, on).gamma_multiply(if on { 0.9 } else { 0.6 }),
                Stroke::new(1.5, shadow),
            );
        }
        GizmoMode::Rotate => {
            painter.circle_stroke(
                l.center,
                RING_RADIUS + 10.0,
                Stroke::new(1.0, NEUTRAL.gamma_multiply(0.35)),
            );
            // the far halves first, then the near ones
            for near in [false, true] {
                for (i, (ring, color)) in l.rings.iter().zip(AXIS_COLORS).enumerate() {
                    let on = hot == Some(Handle::Ring(i));
                    let col = lit(color, on);
                    let (w, col) = if near {
                        (if on { 4.0 } else { 3.0 }, col)
                    } else {
                        (2.0, col.gamma_multiply(0.35))
                    };
                    for s in ring.windows(2) {
                        if (s[0].1 || s[1].1) == near {
                            painter.line_segment([s[0].0, s[1].0], Stroke::new(w, col));
                        }
                    }
                }
            }
            painter.circle_filled(l.center, 3.0, NEUTRAL);
        }
        GizmoMode::Scale => {
            let on = hot == Some(Handle::Scale);
            let col = lit(NEUTRAL, on);
            let r = drag.map_or(SCALE_RADIUS, |d| SCALE_RADIUS * d.scale.clamp(0.2, 3.0));
            painter.circle_stroke(l.center, r, Stroke::new(4.0, shadow));
            painter.circle_stroke(l.center, r, Stroke::new(if on { 3.0 } else { 2.0 }, col));
            for k in 0..4 {
                let t = k as f32 * TAU / 4.0 + TAU / 8.0;
                let p = l.center + EVec2::new(t.cos(), t.sin()) * r;
                painter.rect_filled(Rect::from_center_size(p, EVec2::splat(9.0)), 1.0, col);
            }
            painter.circle_filled(l.center, 3.0, col);
        }
    }
    // the value of the drag, next to the pointer
    if let (Some(d), Some(p)) = (drag, pointer) {
        let text = match g.mode {
            GizmoMode::Translate => format!("{:.2} mm", d.translation.length()),
            GizmoMode::Rotate => format!("{:.1}°", d.rotation.to_axis_angle().1.to_degrees()),
            GizmoMode::Scale => format!("×{:.3}", d.scale),
        };
        let at = p + EVec2::new(14.0, -14.0);
        let font = FontId::monospace(12.0);
        let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
        let bg = Rect::from_min_size(
            at - EVec2::new(4.0, 2.0),
            galley.size() + EVec2::new(8.0, 4.0),
        );
        painter.rect_filled(bg, 3.0, Color32::from_black_alpha(170));
        painter.galley(at, galley, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::StdView;

    /// A camera looking at the origin from the front (+Z up, +X right).
    fn front() -> Projector {
        let mut camera = Camera::default();
        camera.set_view(StdView::Front);
        camera.target = Vec3::ZERO;
        camera.distance = 100.0;
        Projector {
            camera,
            rect: Rect::from_min_size(Pos2::ZERO, EVec2::new(800.0, 600.0)),
            bounds: Some(Sphere {
                center: Vec3::ZERO,
                radius: 30.0,
            }),
        }
    }

    fn gizmo(mode: GizmoMode) -> TransformGizmo {
        TransformGizmo {
            pivot: Vec3::ZERO,
            mode,
        }
    }

    #[test]
    fn the_arrows_point_along_the_screen_axes_and_one_seen_end_on_is_hidden() {
        let p = front();
        let l = layout(&gizmo(GizmoMode::Translate), &p);
        let c = l.center;
        assert!((c - Pos2::new(400.0, 300.0)).length() < 0.5, "{c:?}");
        let x = l.ends[0].unwrap();
        let z = l.ends[2].unwrap();
        assert!(x.x > c.x + 80.0 && (x.y - c.y).abs() < 1.0, "{x:?}");
        assert!(z.y < c.y - 80.0 && (z.x - c.x).abs() < 1.0, "{z:?}");
        // the front view looks along +Y: that arrow is seen end on
        assert!(l.ends[1].is_none());
        // the arrows keep their size on the screen when zooming
        let mut zoomed = p;
        zoomed.camera.distance = 25.0;
        let lz = layout(&gizmo(GizmoMode::Translate), &zoomed);
        let len = |l: &Layout| l.ends[0].unwrap().distance(l.center);
        assert!((len(&l) - len(&lz)).abs() < 0.5);
        assert!(zoomed.mm_per_point(Vec3::ZERO) < p.mm_per_point(Vec3::ZERO));
    }

    #[test]
    fn the_handles_are_found_under_the_pointer() {
        let p = front();
        let g = gizmo(GizmoMode::Translate);
        let c = Pos2::new(400.0, 300.0);
        assert_eq!(hit(&g, &p, c), Some(Handle::Screen));
        assert_eq!(
            hit(&g, &p, c + EVec2::new(60.0, 2.0)),
            Some(Handle::Axis(0))
        );
        assert_eq!(
            hit(&g, &p, c + EVec2::new(-2.0, -60.0)),
            Some(Handle::Axis(2))
        );
        // the XZ square (normal Y, facing the viewer)
        assert_eq!(
            hit(&g, &p, c + EVec2::new(29.0, -29.0)),
            Some(Handle::Plane(1))
        );
        assert_eq!(hit(&g, &p, c + EVec2::new(-60.0, 60.0)), None);
        let r = gizmo(GizmoMode::Rotate);
        // the ring about Y faces the viewer: a circle of 80 points (off the
        // two rings seen edge on, the lines through the centre)
        assert_eq!(
            hit(&r, &p, c + EVec2::new(56.57, 56.57)),
            Some(Handle::Ring(1))
        );
        assert_eq!(hit(&r, &p, c + EVec2::new(30.0, 30.0)), None);
        let s = gizmo(GizmoMode::Scale);
        assert_eq!(hit(&s, &p, c + EVec2::new(70.0, 0.0)), Some(Handle::Scale));
        assert_eq!(hit(&s, &p, c + EVec2::new(35.0, 0.0)), None);
    }

    #[test]
    fn dragging_an_arrow_moves_along_its_axis_only() {
        let p = front();
        let g = gizmo(GizmoMode::Translate);
        let c = p.screen(Vec3::ZERO);
        let start = c + EVec2::new(50.0, 0.0);
        let mut d = Drag::start(Handle::Axis(0), &g, &p, start);
        // 40 points to the right and some up: only the X part counts
        let delta = d.update(&p, start + EVec2::new(40.0, -25.0), false);
        let mm = 40.0 * p.mm_per_point(Vec3::ZERO);
        assert!(
            (delta.translation - Vec3::X * mm).length() < 1e-2 * mm,
            "{delta:?}"
        );
        assert_eq!(delta.rotation, Quat::IDENTITY);
        // snapped to half millimetres
        let snapped = d.update(&p, start + EVec2::new(40.0, 0.0), true);
        let t = snapped.translation.x;
        assert!((t / 0.5 - (t / 0.5).round()).abs() < 1e-4, "{t}");
        // the free handle follows the pointer in the plane of the screen
        let mut free = Drag::start(Handle::Screen, &g, &p, c);
        let delta = free.update(&p, c + EVec2::new(20.0, -10.0), false);
        let k = p.mm_per_point(Vec3::ZERO);
        assert!((delta.translation - Vec3::new(20.0 * k, 0.0, 10.0 * k)).length() < 1e-2);
    }

    #[test]
    fn dragging_a_ring_turns_about_its_axis_the_right_way() {
        let p = front();
        let g = gizmo(GizmoMode::Rotate);
        let c = p.screen(Vec3::ZERO);
        // the ring about Y faces the viewer; from its right point, going up
        let start = c + EVec2::new(80.0, 0.0);
        let mut d = Drag::start(Handle::Ring(1), &g, &p, start);
        let end = c + EVec2::new(0.0, -80.0);
        let delta = d.update(&p, end, false);
        let (axis, angle) = delta.rotation.to_axis_angle();
        assert!(
            (angle - std::f32::consts::FRAC_PI_2).abs() < 0.05,
            "{angle}"
        );
        // X (screen right) turned to Z (screen up): a turn about −Y... or +Y
        // with the angle the other way; whichever, it maps the press to the pointer
        let moved = delta.rotation * Vec3::X;
        assert!((moved - Vec3::Z).length() < 0.06, "{axis:?} {moved:?}");
        // the matrix turns about the pivot
        let m = delta.matrix();
        assert!((m.transform_point3(Vec3::ZERO)).length() < 1e-5);
    }

    #[test]
    fn an_edge_on_ring_follows_the_pointer_along_its_tangent() {
        let p = front();
        let g = gizmo(GizmoMode::Rotate);
        let c = p.screen(Vec3::ZERO);
        // the ring about Z is seen edge on: a horizontal line through the centre
        let start = c + EVec2::new(79.0, 0.0);
        assert_eq!(hit(&g, &p, start), Some(Handle::Ring(2)));
        let mut d = Drag::start(Handle::Ring(2), &g, &p, start);
        assert!(d.edge_on);
        let small = d.update(&p, start + EVec2::new(0.0, 0.0), false);
        assert!(small.rotation.to_axis_angle().1.abs() < 1e-4);
        let moved = d.update(&p, start + EVec2::new(-20.0, 0.0), false);
        assert!(moved.rotation.to_axis_angle().1.abs() > 0.05);
    }

    #[test]
    fn a_ring_keeps_its_turn_when_the_pointer_leaves_its_plane() {
        let p = front();
        let g = gizmo(GizmoMode::Rotate);
        let c = p.screen(Vec3::ZERO);
        // the ring about Y faces the viewer; from its right point going up turns X to
        // Z: a negative angle about +Y
        let start = c + EVec2::new(80.0, 0.0);
        let mut d = Drag::start(Handle::Ring(1), &g, &p, start);
        let turned = d.update(&p, c + EVec2::new(0.0, -80.0), false);
        assert!(((turned.rotation * Vec3::X) - Vec3::Z).length() < 0.06);
        // seen from the side, the line of sight through the centre runs along the
        // plane of the ring: no point on it, the turn stays as it was
        let mut side = front();
        side.camera.set_view(StdView::Right);
        let kept = d.update(&side, side.screen(Vec3::ZERO), false);
        assert!(
            kept.rotation.abs_diff_eq(turned.rotation, 1e-6),
            "{:?} {:?}",
            kept.rotation,
            turned.rotation
        );
    }

    #[test]
    fn the_scale_is_the_ratio_of_the_distances_to_the_centre() {
        let p = front();
        let g = gizmo(GizmoMode::Scale);
        let c = p.screen(Vec3::ZERO);
        let start = c + EVec2::new(70.0, 0.0);
        let mut d = Drag::start(Handle::Scale, &g, &p, start);
        let delta = d.update(&p, c + EVec2::new(84.0, 0.0), false);
        assert!((delta.scale - 1.2).abs() < 1e-3, "{delta:?}");
        assert_eq!(delta.translation, Vec3::ZERO);
        let m = delta.matrix();
        assert!((m.transform_point3(Vec3::X) - Vec3::X * 1.2).length() < 1e-4);
        assert!(GizmoDelta::identity(Vec3::ONE).is_identity());
    }
}
