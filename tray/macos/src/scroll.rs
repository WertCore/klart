//! Changing brightness by scrolling over the menu bar icon.
//!
//! Opening a menu to move a slider is three actions for a change that is
//! usually one step. A wheel over the icon is one, and it is what the volume
//! and brightness items in the menu bar have trained everyone to expect.
//!
//! This reads the event rather than installing a monitor or subclassing the
//! status item's button. The agent's pump already takes every event the
//! application is sent and hands it on, so the one place that sees every scroll
//! already exists — a scroll over the icon is recognised there and absorbed, and
//! everything else is passed along untouched.

use objc2_app_kit::{NSEvent, NSEventType, NSStatusItem};
use objc2_foundation::MainThreadMarker;

/// How much brightness one click of a wheel moves, in percent.
///
/// A mouse wheel reports whole notches, so this is the step size directly.
/// Enough that a couple of clicks are worth doing and small enough that the
/// whole range is not two of them.
const PER_NOTCH: f32 = 4.0;

/// How much brightness one point of trackpad scrolling moves, in percent.
///
/// A trackpad reports continuous distances rather than notches, and many more
/// of them, so the same step size would make the icon unusable. A comfortable
/// swipe is a couple of hundred points, which this turns into most of the
/// range.
const PER_POINT: f32 = 0.5;

/// How much brightness this event asks for, if it is a scroll over `item`.
///
/// [`None`] for everything else, which is almost every event: the caller passes
/// those on to the application untouched.
pub fn over(event: &NSEvent, item: &NSStatusItem, mtm: MainThreadMarker) -> Option<f32> {
    if event.r#type() != NSEventType::ScrollWheel {
        return None;
    }

    // Which window, rather than which coordinates. A status item's button has a
    // window of its own, so the window number answers "was this over the icon"
    // exactly, and keeps answering it when the item moves — which it does
    // whenever anything to its right is added or removed.
    let window = item.button(mtm)?.window()?;
    if event.windowNumber() != window.windowNumber() {
        return None;
    }

    // Up is brighter. `scrollingDeltaY` has already had the person's scroll
    // direction preference applied to it by AppKit, so this follows whatever
    // "up" means on their machine rather than imposing a second opinion.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a scroll distance in points, which does not reach f32's limits"
    )]
    let distance = event.scrollingDeltaY() as f32;

    // A wheel reports notches and a trackpad reports points, and they differ by
    // two orders of magnitude. Treating them alike makes one of the two useless.
    let per = if event.hasPreciseScrollingDeltas() {
        PER_POINT
    } else {
        PER_NOTCH
    };

    Some(distance * per)
}
