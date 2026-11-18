//! Keeping the object framed in the live view.
//!
//! The view frames the scan when it starts, when an automatic scan starts
//! and when a calibration ends, and again while the fused cloud grows, as
//! long as the user has not moved the camera since. What it frames is the
//! cylinder of the region of interest when the rig is calibrated, else the
//! robust bounds (2nd to 98th percentile) of the fused cloud; the points of
//! the latest frame are left out, they are the first to be wrong.

use std::time::{Duration, Instant};

use glam::Vec3;

/// The view frames again at most this often while the cloud grows.
const REFIT_EVERY: Duration = Duration::from_secs(1);
/// ... and only once the cloud has grown by this factor.
const REFIT_GROWTH: f32 = 1.25;
/// Below this many points the bounds are the plain bounds (a percentile of
/// a handful of points is noise).
const MIN_TRIMMED: usize = 200;
/// At most this many points are sorted for the percentiles.
const MAX_SAMPLED: usize = 16_384;
/// Lower and upper percentile of the bounds of the cloud.
const TRIM: (f32, f32) = (0.02, 0.98);
/// The trimmed box grows by this part of its size on each side, so the
/// points cut off at its edge stay in view.
const PAD: f32 = 0.05;
/// ... and by at least this much (mm).
const PAD_MIN_MM: f32 = 1.0;

/// Whether the view follows the growing scan.
#[derive(Default)]
pub(super) struct Follow {
    /// The user moved the camera since the last framing: the view stays.
    pub user_moved: bool,
    /// Points of the cloud and time of the last framing.
    last: Option<(usize, Instant)>,
}

impl Follow {
    /// The view is framed again from the next content on.
    pub fn restart(&mut self) {
        self.user_moved = false;
        self.last = None;
    }

    /// The view was framed over a cloud of `points`.
    pub fn framed(&mut self, points: usize) {
        self.last = Some((points, Instant::now()));
    }

    /// The cloud of `points` grew enough since the last framing to frame
    /// it again (never after the user moved the camera).
    pub fn due(&self, points: usize) -> bool {
        !self.user_moved && refit_due(self.last.map(|(n, at)| (n, at.elapsed())), points)
    }
}

/// `true` when a cloud of `points` is to be framed again: `last` is the
/// size of the cloud at the last framing and the time since.
fn refit_due(last: Option<(usize, Duration)>, points: usize) -> bool {
    match last {
        None => points > 0,
        Some((n, since)) => {
            since >= REFIT_EVERY
                && points > n
                && ((n < MIN_TRIMMED && points >= MIN_TRIMMED)
                    || points as f32 >= n as f32 * REFIT_GROWTH)
        }
    }
}

/// The plain bounds of `points`; `None` when there are none.
pub(super) fn bounds(points: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let (first, rest) = points.split_first()?;
    Some(
        rest.iter()
            .fold((*first, *first), |(lo, hi), p| (lo.min(*p), hi.max(*p))),
    )
}

/// The box that holds the 2nd to the 98th percentile of the points on each
/// axis, grown a little; `None` without a finite point.
pub(super) fn robust_box(points: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let stride = points.len().div_ceil(MAX_SAMPLED).max(1);
    let finite: Vec<Vec3> = points
        .iter()
        .step_by(stride)
        .copied()
        .filter(|p| p.is_finite())
        .collect();
    if finite.len() < MIN_TRIMMED {
        return bounds(&finite);
    }
    let n = finite.len();
    let at = |axis: usize, q: f32| -> f32 {
        let mut v: Vec<f32> = finite.iter().map(|p| p[axis]).collect();
        let i = (q * (n - 1) as f32).round() as usize;
        *v.select_nth_unstable_by(i, f32::total_cmp).1
    };
    let mut lo = Vec3::ZERO;
    let mut hi = Vec3::ZERO;
    for axis in 0..3 {
        let (a, b) = (at(axis, TRIM.0), at(axis, TRIM.1));
        let pad = ((b - a) * PAD).max(PAD_MIN_MM);
        lo[axis] = a - pad;
        hi[axis] = b + pad;
    }
    Some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_follows_the_growing_cloud_until_the_user_moves_it() {
        let s = Duration::from_secs;
        assert!(refit_due(None, 1), "the first points are framed");
        assert!(!refit_due(None, 0));
        assert!(!refit_due(Some((1000, s(5))), 1100), "not grown enough");
        assert!(refit_due(Some((1000, s(5))), 1300));
        assert!(
            !refit_due(Some((1000, Duration::from_millis(200))), 5000),
            "too soon"
        );
        assert!(
            refit_due(Some((50, s(2))), 250),
            "from a handful to a cloud"
        );
        assert!(
            !refit_due(Some((1000, s(5))), 900),
            "a smaller cloud is left"
        );

        let mut f = Follow::default();
        assert!(f.due(10));
        f.framed(10);
        assert!(!f.due(10_000), "framed a moment ago");
        f.user_moved = true;
        f.last = Some((10, Instant::now() - s(10)));
        assert!(!f.due(10_000), "the user's view stays");
        f.restart();
        assert!(f.due(10_000));
    }

    #[test]
    fn stray_points_do_not_stretch_the_box() {
        // a 10 mm cube of points and 1 % of strays 400 mm away
        let mut pts = Vec::new();
        for i in 0..40u32 {
            for j in 0..40u32 {
                for k in 0..10u32 {
                    pts.push(Vec3::new(i as f32 * 0.25, j as f32 * 0.25, k as f32 * 1.1));
                }
            }
        }
        let cube = pts.len();
        for i in 0..cube / 100 {
            pts.push(Vec3::new(400.0 + i as f32, -300.0, 500.0));
        }
        let (lo, hi) = robust_box(&pts).unwrap();
        assert!(hi.x < 14.0 && hi.y < 14.0 && hi.z < 14.0, "{hi}");
        assert!(lo.x > -4.0 && lo.y > -4.0 && lo.z > -4.0, "{lo}");
        let (plain_lo, plain_hi) = bounds(&pts).unwrap();
        assert!(plain_hi.x > 400.0 && plain_lo.y < -299.0);
    }

    #[test]
    fn few_points_and_non_finite_ones() {
        assert_eq!(robust_box(&[]), None);
        let one = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(robust_box(&[one, Vec3::NAN]), Some((one, one)));
        assert_eq!(bounds(&[one]), Some((one, one)));
    }
}
