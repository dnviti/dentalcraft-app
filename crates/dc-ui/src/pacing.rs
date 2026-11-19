//! Frame rate cap: the window draws at most [`MAX_FPS`] frames a second.
//!
//! The surface presents with vsync, but a high-refresh display (120, 144 Hz
//! ...) or a compositor that does not throttle would still let a continuous
//! repaint run faster. The shell paces every frame with [`frame_wait`]
//! before it runs, so no repaint request, whatever asked for it, draws
//! sooner than [`FRAME_INTERVAL`] after the previous frame. Animations ask
//! for their next frame with [`request_next_frame`] rather than an
//! immediate repaint. An idle window draws nothing: frames only come from
//! input or a repaint request.

use std::time::Duration;

/// The most frames the window draws in a second.
pub const MAX_FPS: u32 = 60;

/// The shortest time between the starts of two frames (1/60 s).
pub const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / MAX_FPS as u64);

/// How long to wait before a frame that comes `since_last` after the start
/// of the previous one: the rest of [`FRAME_INTERVAL`], or nothing.
pub fn frame_wait(since_last: Duration) -> Duration {
    FRAME_INTERVAL.saturating_sub(since_last)
}

/// Asks for the next frame of an animation, at the frame rate cap.
pub fn request_next_frame(ctx: &egui::Context) {
    ctx.request_repaint_after(FRAME_INTERVAL);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_never_starts_sooner_than_the_interval() {
        assert_eq!(FRAME_INTERVAL, Duration::from_nanos(16_666_666));
        assert_eq!(frame_wait(Duration::ZERO), FRAME_INTERVAL);
        assert_eq!(
            frame_wait(Duration::from_millis(10)),
            FRAME_INTERVAL - Duration::from_millis(10)
        );
        assert_eq!(frame_wait(FRAME_INTERVAL), Duration::ZERO);
        assert_eq!(frame_wait(Duration::from_secs(3)), Duration::ZERO);
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Law: after the wait, a frame starts at least [`FRAME_INTERVAL`] after
    /// the previous one (at most [`MAX_FPS`] frames a second), and the wait
    /// is never longer than one interval (an idle window answers at once).
    #[kani::proof]
    fn frames_never_come_faster_than_the_cap() {
        let since_last =
            Duration::new(kani::any::<u8>().into(), kani::any::<u32>() % 1_000_000_000);
        let wait = frame_wait(since_last);
        assert!(wait <= FRAME_INTERVAL);
        assert!(since_last + wait >= FRAME_INTERVAL);
        if since_last >= FRAME_INTERVAL {
            assert!(wait.is_zero());
        }
    }
}
