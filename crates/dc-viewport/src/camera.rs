//! Orbit camera, projections and the standard dental views.
//!
//! World frame (same convention as the old plugin's `worldframe.py` and
//! [`dc_case::WorldFrame`]): the occlusal plane is the XY plane, the midline
//! runs along Y with the incisors towards −Y, the patient's right is −X, the
//! lower arch has its teeth up (+Z) and lies below the upper arch, whose
//! teeth point down. Units are millimetres.
//!
//! The camera looks along its local −Z axis with local +Y up (right-handed,
//! like glam's `camera::rh` helpers); depth is mapped to `[0, 1]` with
//! NDC y up, as wgpu expects (glam's `directx` projections).

use glam::camera::rh::{proj::directx, view::look_to_mat4};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4Swizzles};

/// Projection type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Projection {
    /// Perspective projection with the camera's vertical field of view.
    #[default]
    Perspective,
    /// Orthographic projection; the visible height equals the height the
    /// perspective frustum has at the orbit target.
    Orthographic,
}

/// Standard clinical views in the DentalCraft world frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StdView {
    /// Frontal view: camera in front of the incisors (−Y) looking at +Y,
    /// up = +Z. The patient's right (−X) is on the left of the screen.
    Front,
    /// From behind (+Y) towards the incisors, up = +Z.
    Back,
    /// The patient's right side: camera on −X looking at +X, up = +Z.
    Right,
    /// The patient's left side: camera on +X looking at −X, up = +Z.
    Left,
    /// Occlusal view of the upper arch: camera below (−Z) looking up,
    /// incisors at the top of the screen.
    OcclusalUpper,
    /// Occlusal view of the lower arch: camera above (+Z) looking down,
    /// incisors at the top of the screen.
    OcclusalLower,
    /// Plain top view: camera above (+Z) looking down, +Y up on the screen
    /// (incisors at the bottom).
    Top,
}

impl StdView {
    /// All views, in menu order.
    pub const ALL: [StdView; 7] = [
        StdView::Front,
        StdView::Back,
        StdView::Right,
        StdView::Left,
        StdView::OcclusalUpper,
        StdView::OcclusalLower,
        StdView::Top,
    ];

    /// Viewing direction (from the camera towards the target) and screen-up
    /// direction, both in world coordinates.
    pub fn direction_and_up(self) -> (Vec3, Vec3) {
        match self {
            StdView::Front => (Vec3::Y, Vec3::Z),
            StdView::Back => (Vec3::NEG_Y, Vec3::Z),
            StdView::Right => (Vec3::X, Vec3::Z),
            StdView::Left => (Vec3::NEG_X, Vec3::Z),
            StdView::OcclusalUpper => (Vec3::Z, Vec3::NEG_Y),
            StdView::OcclusalLower => (Vec3::NEG_Z, Vec3::NEG_Y),
            StdView::Top => (Vec3::NEG_Z, Vec3::Y),
        }
    }

    /// Camera rotation (camera to world) for this view.
    pub fn rotation(self) -> Quat {
        let (dir, up) = self.direction_and_up();
        look_rotation(dir, up)
    }

    /// Short English name (the app translates its own labels).
    pub fn name(self) -> &'static str {
        match self {
            StdView::Front => "Front",
            StdView::Back => "Back",
            StdView::Right => "Right",
            StdView::Left => "Left",
            StdView::OcclusalUpper => "Occlusal upper",
            StdView::OcclusalLower => "Occlusal lower",
            StdView::Top => "Top",
        }
    }
}

/// Camera rotation (camera to world) looking along `dir` with `up` towards
/// the top of the screen. `up` must not be parallel to `dir`.
pub fn look_rotation(dir: Vec3, up: Vec3) -> Quat {
    let z = -dir.normalize();
    let x = up.cross(z).normalize();
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// A ray in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Origin.
    pub origin: Vec3,
    /// Unit direction.
    pub dir: Vec3,
}

impl Ray {
    /// Point at parameter `t`.
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// Bounding sphere used to place the clip planes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    /// Centre.
    pub center: Vec3,
    /// Radius.
    pub radius: f32,
}

const MIN_DISTANCE: f32 = 1e-3;
const MAX_DISTANCE: f32 = 1e7;

/// Orbit camera around a target point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Orbit centre (world, mm).
    pub target: Vec3,
    /// Camera-to-world rotation (the camera looks along its local −Z).
    pub rotation: Quat,
    /// Distance from the eye to the target.
    pub distance: f32,
    /// Vertical field of view in radians (perspective; for orthographic it
    /// sets the visible height together with `distance`).
    pub fov_y: f32,
    /// Projection type.
    pub projection: Projection,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            rotation: StdView::Front.rotation(),
            distance: 100.0,
            fov_y: 30f32.to_radians(),
            projection: Projection::Perspective,
        }
    }
}

impl Camera {
    /// Eye position.
    pub fn eye(&self) -> Vec3 {
        self.target + self.rotation * Vec3::Z * self.distance
    }

    /// Viewing direction (unit).
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    /// Screen right (unit, world).
    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// Screen up (unit, world).
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// Half of the visible height at the target depth.
    pub fn half_height(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// World to camera transform.
    pub fn view_matrix(&self) -> Mat4 {
        look_to_mat4(self.eye(), self.forward(), self.up())
    }

    /// Near and far planes that enclose `bounds` (or a default range around
    /// the target when there is nothing to show).
    pub fn clip_planes(&self, bounds: Option<Sphere>) -> (f32, f32) {
        let (mut near, mut far) = match bounds {
            Some(s) if s.radius > 0.0 => {
                let dc = (s.center - self.eye()).dot(self.forward());
                let pad = s.radius * 1.02 + 1e-3;
                (dc - pad, dc + pad)
            }
            _ => (self.distance * 0.01, self.distance * 100.0),
        };
        match self.projection {
            Projection::Perspective => {
                far = far.max(self.distance * 1e-2).max(1e-3);
                near = near.max(far * 1e-4);
            }
            Projection::Orthographic => {
                if far - near < 1e-3 {
                    far = near + 1e-3;
                }
            }
        }
        (near, far)
    }

    /// Camera to clip transform for the given aspect ratio (width / height).
    pub fn projection_matrix(&self, aspect: f32, near: f32, far: f32) -> Mat4 {
        let aspect = aspect.max(1e-6);
        match self.projection {
            Projection::Perspective => directx::perspective(self.fov_y, aspect, near, far),
            Projection::Orthographic => {
                let h = self.half_height();
                let w = h * aspect;
                directx::orthographic(-w, w, -h, h, near, far)
            }
        }
    }

    /// World to clip transform.
    pub fn view_proj(&self, aspect: f32, bounds: Option<Sphere>) -> Mat4 {
        let (near, far) = self.clip_planes(bounds);
        self.projection_matrix(aspect, near, far) * self.view_matrix()
    }

    /// Projects a world point to normalised device coordinates
    /// (x right, y up in `[-1, 1]`, z depth in `[0, 1]`).
    pub fn project_ndc(&self, p: Vec3, aspect: f32, bounds: Option<Sphere>) -> Vec3 {
        let c = self.view_proj(aspect, bounds) * p.extend(1.0);
        c.xyz() / c.w
    }

    /// Size in points of a length of `length_mm` across the view (along the
    /// screen's right axis) at world point `p`, in a viewport of `size`
    /// points: the radius of a brush circle drawn at `p`.
    pub fn screen_length(
        &self,
        p: Vec3,
        length_mm: f32,
        size: Vec2,
        bounds: Option<Sphere>,
    ) -> f32 {
        let aspect = size.x.max(1.0) / size.y.max(1.0);
        let a = self.project_ndc(p, aspect, bounds);
        let b = self.project_ndc(p + self.right() * length_mm, aspect, bounds);
        ((b.x - a.x) * 0.5 * size.x).hypot((b.y - a.y) * 0.5 * size.y)
    }

    /// Switches to a standard view, keeping target and distance.
    pub fn set_view(&mut self, view: StdView) {
        self.rotation = view.rotation();
    }

    /// Frames a sphere so that it is fully visible with `margin` (e.g. 1.1).
    pub fn fit_sphere(&mut self, sphere: Sphere, aspect: f32, margin: f32) {
        let half_y = self.fov_y * 0.5;
        let half_x = (half_y.tan() * aspect.max(1e-6)).atan();
        let half = half_y.min(half_x);
        self.target = sphere.center;
        let r = sphere.radius.max(1e-3) * margin;
        self.distance = (r / half.sin()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Frames an axis-aligned box seen from the current direction: the orbit
    /// centre moves to the box centre and the distance is the smallest that
    /// keeps every corner on screen, enlarged by `margin` (e.g. 1.1).
    pub fn fit_box(&mut self, min: Vec3, max: Vec3, aspect: f32, margin: f32) {
        let center = (min + max) * 0.5;
        let tan_y = (self.fov_y * 0.5).tan();
        let tan_x = tan_y * aspect.max(1e-6);
        let (right, up, back) = (self.right(), self.up(), -self.forward());
        let mut d: f32 = 0.0;
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            ) - center;
            let (x, y, z) = (
                p.dot(right).abs() * margin,
                p.dot(up).abs() * margin,
                p.dot(back),
            );
            let need = (x / tan_x).max(y / tan_y);
            d = d.max(match self.projection {
                // the corner at depth z (towards the eye) must fit the frustum
                Projection::Perspective => need + z,
                Projection::Orthographic => need,
            });
        }
        if d <= 0.0 {
            // a point: keep a sensible distance
            d = self.distance.min(10.0);
        }
        self.target = center;
        self.distance = d.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Turntable orbit: `yaw` about the world Z axis, `pitch` about the
    /// camera's right axis (radians).
    pub fn orbit(&mut self, yaw: f32, pitch: f32) {
        let r = Quat::from_rotation_z(yaw) * self.rotation * Quat::from_rotation_x(pitch);
        self.rotation = r.normalize();
    }

    /// Pans by a screen delta in pixels (x right, y down) for a viewport of
    /// `viewport_height` pixels: the point under the cursor follows it at the
    /// target depth.
    pub fn pan_pixels(&mut self, delta: Vec2, viewport_height: f32) {
        let wpp = 2.0 * self.half_height() / viewport_height.max(1.0);
        self.target += (-self.right() * delta.x + self.up() * delta.y) * wpp;
    }

    /// Zooms by `factor` (< 1 zooms in) keeping the point under `ndc` (on
    /// the target plane) fixed on screen.
    pub fn zoom_at(&mut self, factor: f32, ndc: Vec2, aspect: f32) {
        let factor = factor.clamp(
            MIN_DISTANCE / self.distance.max(MIN_DISTANCE),
            MAX_DISTANCE / self.distance.max(MIN_DISTANCE),
        );
        let h = self.half_height();
        let p = self.target + self.right() * (ndc.x * h * aspect) + self.up() * (ndc.y * h);
        self.target = p + (self.target - p) * factor;
        self.distance *= factor;
    }

    /// Ray through a point in normalised device coordinates. For the
    /// orthographic projection the origin is moved back so that it lies in
    /// front of everything in `bounds`.
    pub fn ray(&self, ndc: Vec2, aspect: f32, bounds: Option<Sphere>) -> Ray {
        let h = self.half_height();
        let (right, up, fwd) = (self.right(), self.up(), self.forward());
        let offset = right * (ndc.x * h * aspect) + up * (ndc.y * h);
        match self.projection {
            Projection::Perspective => Ray {
                origin: self.eye(),
                dir: (fwd * self.distance + offset).normalize(),
            },
            Projection::Orthographic => {
                let eye = self.eye();
                let back = bounds.map_or(0.0, |s| {
                    (s.radius - (s.center - eye).dot(fwd)).max(0.0) + s.radius * 0.01
                });
                Ray {
                    origin: eye + offset - fwd * back,
                    dir: fwd,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < EPS
    }

    #[test]
    fn a_length_on_screen_follows_the_projection() {
        let size = Vec2::new(800.0, 600.0);
        let mut c = Camera {
            projection: Projection::Orthographic,
            distance: 60.0,
            ..Camera::default()
        };
        // orthographic: the visible height is 2 * half_height, whatever the depth
        let px = c.screen_length(c.target, 5.0, size, None);
        assert!(
            (px - 5.0 / (2.0 * c.half_height()) * size.y).abs() < 0.05,
            "{px}"
        );
        let near = c.screen_length(c.target + c.forward() * 20.0, 5.0, size, None);
        assert!((near - px).abs() < 0.05);
        // perspective: closer to the eye is bigger
        c.projection = Projection::Perspective;
        let far = c.screen_length(c.target + c.forward() * 20.0, 5.0, size, None);
        let close = c.screen_length(c.target - c.forward() * 20.0, 5.0, size, None);
        assert!(close > far * 1.5, "{close} {far}");
    }

    #[test]
    fn view_matrix_moves_eye_to_origin() {
        let mut c = Camera {
            target: Vec3::new(3.0, -2.0, 5.0),
            distance: 40.0,
            ..Camera::default()
        };
        c.orbit(0.4, -0.3);
        let v = c.view_matrix();
        assert!(close(v.transform_point3(c.eye()), Vec3::ZERO));
        assert!(close(
            v.transform_point3(c.target),
            Vec3::new(0.0, 0.0, -40.0)
        ));
        // right and up map to the camera axes
        assert!(close(v.transform_vector3(c.right()), Vec3::X));
        assert!(close(v.transform_vector3(c.up()), Vec3::Y));
    }

    #[test]
    fn projection_maps_target_to_centre_and_depth_in_range() {
        for proj in [Projection::Perspective, Projection::Orthographic] {
            let c = Camera {
                projection: proj,
                ..Camera::default()
            };
            let bounds = Some(Sphere {
                center: Vec3::ZERO,
                radius: 20.0,
            });
            let ndc = c.project_ndc(c.target, 1.5, bounds);
            assert!(ndc.x.abs() < EPS && ndc.y.abs() < EPS, "{proj:?} {ndc}");
            assert!((0.0..=1.0).contains(&ndc.z), "{proj:?} {ndc}");
            // a point nearer to the camera has a smaller depth
            let near = c.project_ndc(c.target - c.forward() * 5.0, 1.5, bounds);
            assert!(near.z < ndc.z);
            // the sphere fits between the clip planes
            for p in [c.target + c.forward() * 19.0, c.target - c.forward() * 19.0] {
                let z = c.project_ndc(p, 1.5, bounds).z;
                assert!((0.0..=1.0).contains(&z), "{proj:?} {z}");
            }
        }
    }

    #[test]
    fn perspective_half_height_matches_fov() {
        let c = Camera::default();
        let top = c.target + c.up() * c.half_height();
        let ndc = c.project_ndc(top, 1.0, None);
        assert!((ndc.y - 1.0).abs() < EPS);
        let ortho = Camera {
            projection: Projection::Orthographic,
            ..c
        };
        let ndc = ortho.project_ndc(top, 1.0, None);
        assert!((ndc.y - 1.0).abs() < EPS);
    }

    fn view_cam(v: StdView) -> Camera {
        let mut c = Camera::default();
        c.set_view(v);
        c
    }

    #[test]
    fn front_view_orientation() {
        let c = view_cam(StdView::Front);
        // camera in front of the incisors (−Y), looking at +Y, Z up
        assert!(close(c.forward(), Vec3::Y));
        assert!(c.eye().y < 0.0);
        assert!(close(c.up(), Vec3::Z));
        // patient's right (−X) on the left of the screen
        assert!(c.project_ndc(Vec3::new(-10.0, 0.0, 0.0), 1.0, None).x < 0.0);
        // the upper arch (+Z, above the lower one) at the top of the screen
        assert!(c.project_ndc(Vec3::new(0.0, 0.0, 10.0), 1.0, None).y > 0.0);
    }

    #[test]
    fn side_views_orientation() {
        let r = view_cam(StdView::Right);
        assert!(
            r.eye().x < 0.0,
            "right view looks at the patient's right side (−X)"
        );
        assert!(close(r.up(), Vec3::Z));
        // incisors (−Y) on the right of the screen when looking at the patient's right side
        assert!(r.project_ndc(Vec3::new(0.0, -10.0, 0.0), 1.0, None).x > 0.0);
        let l = view_cam(StdView::Left);
        assert!(l.eye().x > 0.0);
        assert!(l.project_ndc(Vec3::new(0.0, -10.0, 0.0), 1.0, None).x < 0.0);
        let b = view_cam(StdView::Back);
        assert!(b.eye().y > 0.0 && close(b.up(), Vec3::Z));
    }

    #[test]
    fn occlusal_views_orientation() {
        let lo = view_cam(StdView::OcclusalLower);
        assert!(lo.eye().z > 0.0, "lower occlusal view from above");
        // incisors (−Y) at the top of the screen
        assert!(lo.project_ndc(Vec3::new(0.0, -10.0, 0.0), 1.0, None).y > 0.0);
        let up = view_cam(StdView::OcclusalUpper);
        assert!(up.eye().z < 0.0, "upper occlusal view from below");
        assert!(up.project_ndc(Vec3::new(0.0, -10.0, 0.0), 1.0, None).y > 0.0);
        // looking up at the palate the patient's right is on the left of the screen
        assert!(up.project_ndc(Vec3::new(-10.0, 0.0, 0.0), 1.0, None).x < 0.0);
        let top = view_cam(StdView::Top);
        assert!(top.eye().z > 0.0 && close(top.up(), Vec3::Y));
        for v in StdView::ALL {
            let r = v.rotation();
            assert!((r.length() - 1.0).abs() < EPS, "{v:?}");
        }
    }

    #[test]
    fn presets_follow_the_case_world_frame() {
        use dc_case::WorldFrame;
        let anterior = WorldFrame::ANTERIOR.as_vec3();
        let to_lower = WorldFrame::TOWARDS_LOWER.as_vec3();
        let lower_teeth = WorldFrame::OCCLUSAL_NORMAL.as_vec3();
        let upper_teeth = -lower_teeth;
        // a lower crown tip below an upper crown tip, both near the incisors
        let lower_tip = Vec3::new(0.0, -20.0, -1.0);
        let upper_tip = Vec3::new(0.0, -21.0, 1.0);
        let f = view_cam(StdView::Front);
        assert!(
            close(f.forward(), -anterior),
            "front looks from the incisors"
        );
        assert!(
            close(f.up(), -to_lower),
            "the upper arch is up on the screen"
        );
        assert!(
            f.project_ndc(upper_tip, 1.0, None).y > f.project_ndc(lower_tip, 1.0, None).y,
            "upper arch above the lower arch in the front view"
        );
        // occlusal views look at the occlusal surface of one arch
        let lo = view_cam(StdView::OcclusalLower);
        assert!(close(lo.forward(), to_lower));
        assert!(
            lo.forward().dot(lower_teeth) < 0.0,
            "lower teeth face the camera"
        );
        let up = view_cam(StdView::OcclusalUpper);
        assert!(close(up.forward(), -to_lower));
        assert!(
            up.forward().dot(upper_teeth) < 0.0,
            "upper teeth face the camera"
        );
        for v in [StdView::OcclusalLower, StdView::OcclusalUpper] {
            let c = view_cam(v);
            // incisors at the top of the screen
            assert!(close(c.up(), anterior), "{v:?}");
        }
        // side views: the patient's right (−X) side faces the right view
        assert!(view_cam(StdView::Right).forward().x > 0.9);
        assert!(view_cam(StdView::Left).forward().x < -0.9);
    }

    #[test]
    fn fit_box_makes_corners_visible() {
        let (min, max) = (Vec3::new(-30.0, -25.0, -5.0), Vec3::new(30.0, 20.0, 12.0));
        for v in StdView::ALL {
            for aspect in [0.5, 1.0, 2.0] {
                for proj in [Projection::Perspective, Projection::Orthographic] {
                    let mut c = view_cam(v);
                    c.projection = proj;
                    c.fit_box(min, max, aspect, 1.0);
                    let bounds = Some(Sphere {
                        center: (min + max) * 0.5,
                        radius: (max - min).length() * 0.5,
                    });
                    let mut extent: f32 = 0.0;
                    for i in 0..8 {
                        let p = Vec3::new(
                            if i & 1 == 0 { min.x } else { max.x },
                            if i & 2 == 0 { min.y } else { max.y },
                            if i & 4 == 0 { min.z } else { max.z },
                        );
                        let n = c.project_ndc(p, aspect, bounds);
                        assert!(
                            n.x.abs() <= 1.0 + EPS && n.y.abs() <= 1.0 + EPS,
                            "{v:?} {aspect} {proj:?} {n}"
                        );
                        assert!((0.0..=1.0).contains(&n.z), "{v:?} {proj:?} {n}");
                        extent = extent.max(n.x.abs()).max(n.y.abs());
                    }
                    // tight: some corner touches the border of the screen
                    assert!(
                        (extent - 1.0).abs() < 1e-3,
                        "{v:?} {aspect} {proj:?} {extent}"
                    );
                }
            }
        }
    }

    #[test]
    fn zoom_keeps_point_under_cursor() {
        for proj in [Projection::Perspective, Projection::Orthographic] {
            let mut c = Camera {
                projection: proj,
                ..Camera::default()
            };
            c.orbit(0.3, 0.2);
            let aspect = 1.6;
            let ndc = Vec2::new(0.4, -0.3);
            let h = c.half_height();
            let p = c.target + c.right() * (ndc.x * h * aspect) + c.up() * (ndc.y * h);
            c.zoom_at(0.5, ndc, aspect);
            assert!((c.distance - 50.0).abs() < EPS);
            let after = c.project_ndc(p, aspect, None);
            assert!((after.truncate() - ndc).length() < 1e-3, "{proj:?} {after}");
        }
    }

    #[test]
    fn pan_moves_target_in_screen_plane() {
        let mut c = Camera::default();
        let before = c.target;
        c.pan_pixels(Vec2::new(100.0, 0.0), 400.0);
        let d = c.target - before;
        assert!(d.dot(c.forward()).abs() < EPS);
        // dragging right moves the scene right, so the target moves left
        assert!(d.dot(c.right()) < 0.0);
        // 100 px of a 400 px high view = a quarter of the visible height
        assert!((d.length() - c.half_height() * 0.5).abs() < 1e-3);
    }

    #[test]
    fn orbit_keeps_distance_and_turntable_axis() {
        let mut c = view_cam(StdView::Front);
        let d0 = (c.eye() - c.target).length();
        c.orbit(std::f32::consts::FRAC_PI_2, 0.0);
        assert!(((c.eye() - c.target).length() - d0).abs() < 1e-3);
        // a quarter turn about Z keeps the up vector on Z
        assert!(close(c.up(), Vec3::Z));
    }

    #[test]
    fn rays_go_through_projected_points() {
        for proj in [Projection::Perspective, Projection::Orthographic] {
            let mut c = Camera {
                projection: proj,
                ..Camera::default()
            };
            c.orbit(-0.7, 0.4);
            let bounds = Some(Sphere {
                center: Vec3::ZERO,
                radius: 30.0,
            });
            let p = Vec3::new(4.0, -3.0, 7.0);
            let ndc = c.project_ndc(p, 1.3, bounds);
            let ray = c.ray(ndc.truncate(), 1.3, bounds);
            let t = (p - ray.origin).dot(ray.dir);
            assert!(t > 0.0, "{proj:?}");
            assert!((ray.at(t) - p).length() < 1e-2, "{proj:?}");
        }
    }
}
