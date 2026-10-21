//! What the menu is asking the agent to do.
//!
//! The menu runs inside AppKit's own event handling, where there is no borrow
//! of the agent to hand it. So the controls push a request onto a queue and the
//! pump drains it a moment later, on the same thread, with the agent in hand.
//!
//! The queue is a `Mutex<Vec<_>>` rather than anything cleverer because both
//! ends are the main thread: it is never contended, and the lock is there to
//! satisfy `static` rather than to coordinate anything.

use std::sync::Mutex;

/// Something the menu wants the agent to do.
///
/// Setting a level is deliberately not here: that has to happen while the menu
/// is open, so it goes straight to [`crate::driver::Driver`] instead. What is
/// left is the two things that can only be done once the menu has closed —
/// rebuilding it, and stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Look for displays again.
    Refresh,
    /// Start with the session, or stop doing so.
    SetLoginItem(bool),
    /// Move the displays together by a delta rather than onto one level.
    SetKeepOffsets(bool),
    /// Stop the agent.
    Quit,
}

static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());

/// Queues a request for the pump.
pub fn push(request: Request) {
    if let Ok(mut pending) = PENDING.lock() {
        pending.push(request);
    }
}

/// Takes everything queued since the last call.
pub fn drain() -> Vec<Request> {
    PENDING
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

/// The level a slider at this position is asking for.
///
/// The slider's own range is already 0 to 100, so this is about what a `f64`
/// coming out of AppKit might be rather than about scaling: a drag past the end
/// of the track, or a value that rounds to 100.4.
#[must_use]
pub fn percent_from(position: f64) -> u8 {
    if position.is_nan() {
        return 0;
    }
    position.round().clamp(0.0, 100.0) as u8
}

/// The line above a display's slider.
#[must_use]
pub fn heading(name: &str, percent: u8, transient: bool) -> String {
    // The marker rather than the sentence, because the sentence is on its own
    // line underneath and repeating it in every heading would crowd them.
    let marker = if transient { " ·" } else { "" };
    format!("{name} — {percent}%{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slider_position_becomes_a_percentage() {
        assert_eq!(percent_from(0.0), 0);
        assert_eq!(percent_from(44.4), 44);
        assert_eq!(percent_from(44.5), 45);
        assert_eq!(percent_from(100.0), 100);
    }

    #[test]
    fn a_position_off_the_end_of_the_track_is_clamped() {
        assert_eq!(percent_from(-3.0), 0);
        assert_eq!(percent_from(100.4), 100);
        assert_eq!(percent_from(f64::INFINITY), 100);
        assert_eq!(percent_from(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn a_position_that_is_not_a_number_does_not_become_one() {
        // `f64::clamp` passes NaN through and the cast would then be zero
        // anyway, but by way of undefined-ish behaviour rather than a decision.
        assert_eq!(percent_from(f64::NAN), 0);
    }

    #[test]
    fn a_heading_says_the_level_and_marks_what_will_not_last() {
        assert_eq!(
            heading("Built-in Display", 44, false),
            "Built-in Display — 44%"
        );
        assert_eq!(heading("LS32AG55x", 100, true), "LS32AG55x — 100% ·");
    }

    #[test]
    fn the_queue_hands_back_what_was_put_on_it_and_then_empties() {
        push(Request::Refresh);
        push(Request::Quit);

        assert_eq!(drain(), vec![Request::Refresh, Request::Quit]);
        assert_eq!(drain(), vec![]);
    }
}
