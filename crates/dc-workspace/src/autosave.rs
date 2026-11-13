//! The automatic save of a project that has a file ([`Workspace::tick`]).
//!
//! The case is saved by itself once it has not changed for [`DELAY_S`]
//! seconds: each change (a command that lands, undo, redo) starts the timer
//! again. Nothing is saved by itself while a job that changes the case runs,
//! while a save runs or while a dialog is open; the save follows as soon as
//! they end. A project that was never saved has no file: nothing asks for one,
//! and it is saved by itself only once the user saved it. A file opened
//! read-only is never written by itself.
//!
//! [`Workspace::tick`]: crate::Workspace::tick

use std::sync::Arc;

use dc_case::CaseState;

/// Seconds without a change before the case is saved by itself.
pub const DELAY_S: f64 = 2.0;

/// The timer of the automatic save and the state whose save failed.
#[derive(Debug, Default)]
pub(crate) struct Autosave {
    /// The state of the case last seen and the time it was first seen (the
    /// clock of [`crate::Workspace::tick`]).
    seen: Option<(Arc<CaseState>, f64)>,
    /// The state whose last save failed: it is not saved by itself again
    /// (the error was shown); a new change, or Save, tries again.
    failed: Option<Arc<CaseState>>,
}

impl Autosave {
    /// Seconds `current` has been the state of the case at `now`: `0` when it
    /// is new (the timer starts again).
    pub fn idle_for(&mut self, current: &Arc<CaseState>, now: f64) -> f64 {
        match &self.seen {
            Some((state, since)) if Arc::ptr_eq(state, current) => (now - since).max(0.0),
            _ => {
                self.seen = Some((Arc::clone(current), now));
                0.0
            }
        }
    }

    /// The save of `state` failed.
    pub fn failed(&mut self, state: &Arc<CaseState>) {
        self.failed = Some(Arc::clone(state));
    }

    /// A save succeeded.
    pub fn succeeded(&mut self) {
        self.failed = None;
    }

    /// `true` when the last save of `current` failed.
    pub fn failed_on(&self, current: &Arc<CaseState>) -> bool {
        self.failed
            .as_ref()
            .is_some_and(|f| Arc::ptr_eq(f, current))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_case::Case;

    #[test]
    fn the_timer_starts_again_at_each_change() {
        let a = Arc::new(CaseState::new(Case::new("a")));
        let b = Arc::new(CaseState::new(Case::new("b")));
        let mut s = Autosave::default();
        assert_eq!(s.idle_for(&a, 10.0), 0.0);
        assert_eq!(s.idle_for(&a, 11.5), 1.5);
        // a change: from zero again
        assert_eq!(s.idle_for(&b, 12.0), 0.0);
        assert_eq!(s.idle_for(&b, 14.5), 2.5);
        // back to the first state (undo) is a change too
        assert_eq!(s.idle_for(&a, 15.0), 0.0);
        // a clock that goes back never gives a negative time
        assert_eq!(s.idle_for(&a, 14.0), 0.0);
    }

    #[test]
    fn a_failed_state_waits_for_a_change() {
        let a = Arc::new(CaseState::new(Case::new("a")));
        let b = Arc::new(CaseState::new(Case::new("b")));
        let mut s = Autosave::default();
        s.failed(&a);
        assert!(s.failed_on(&a));
        assert!(!s.failed_on(&b));
        s.succeeded();
        assert!(!s.failed_on(&a));
    }
}
