//! Frame pacing of the window: at most [`dc_ui::pacing::MAX_FPS`] frames a
//! second.
//!
//! eframe calls `App::raw_input_hook` before every frame, whatever asked
//! for it (input, a job, the scan window, the bridge, an animation). There
//! [`FramePacer::pace`] waits for the rest of [`FRAME_INTERVAL`] since the
//! start of the previous frame, so a continuous repaint runs at 60 fps even
//! on a display faster than 60 Hz or where vsync does not hold it back. A
//! frame after a pause starts at once. With `RUST_LOG=dc_app::pacing=debug`
//! the log says, every few seconds of drawing, how many frames were drawn and
//! how many were held back.

use std::time::{Duration, Instant};

use dc_ui::pacing::{FRAME_INTERVAL, frame_wait};

/// How often the frame statistics go to the debug log.
const STATS_EVERY: Duration = Duration::from_secs(2);

/// Paces the frames of the window (see the module docs).
#[derive(Debug, Default)]
pub struct FramePacer {
    /// Start of the previous frame.
    last: Option<Instant>,
    /// Statistics since `since`: frames, frames held back, time held back.
    since: Option<Instant>,
    frames: u32,
    held: u32,
    waited: Duration,
}

impl FramePacer {
    /// Waits, before a frame, for the rest of the interval since the start
    /// of the previous one; the frame's input time follows the wait.
    pub fn pace(&mut self, raw_input: &mut egui::RawInput) {
        let wait = self
            .last
            .map_or(Duration::ZERO, |last| frame_wait(last.elapsed()));
        if !wait.is_zero() {
            std::thread::sleep(wait);
            if let Some(t) = raw_input.time.as_mut() {
                *t += wait.as_secs_f64();
            }
        }
        let now = Instant::now();
        self.last = Some(now);
        self.count(now, wait);
    }

    fn count(&mut self, now: Instant, wait: Duration) {
        let since = *self.since.get_or_insert(now);
        self.frames += 1;
        if !wait.is_zero() {
            self.held += 1;
            self.waited += wait;
        }
        let span = now - since;
        if span >= STATS_EVERY {
            tracing::debug!(
                "frames: {} in {:.2} s ({:.1} fps), {} held back {:.0} ms in all (cap {:.1} ms)",
                self.frames,
                span.as_secs_f64(),
                f64::from(self.frames) / span.as_secs_f64(),
                self.held,
                self.waited.as_secs_f64() * 1000.0,
                FRAME_INTERVAL.as_secs_f64() * 1000.0,
            );
            self.since = Some(now);
            self.frames = 0;
            self.held = 0;
            self.waited = Duration::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_to_back_frames_are_held_to_the_cap() {
        let mut pacer = FramePacer::default();
        let mut input = egui::RawInput {
            time: Some(0.0),
            ..Default::default()
        };
        pacer.pace(&mut input);
        let start = Instant::now();
        for _ in 0..6 {
            pacer.pace(&mut input);
        }
        // six frames after the first: at least six intervals
        assert!(
            start.elapsed() >= FRAME_INTERVAL * 5,
            "{:?}",
            start.elapsed()
        );
        assert!(input.time.unwrap() > 0.0);
    }

    #[test]
    fn a_frame_after_a_pause_starts_at_once() {
        let mut pacer = FramePacer::default();
        let mut input = egui::RawInput::default();
        pacer.pace(&mut input);
        std::thread::sleep(FRAME_INTERVAL * 2);
        let start = Instant::now();
        pacer.pace(&mut input);
        assert!(start.elapsed() < FRAME_INTERVAL / 2);
    }
}
