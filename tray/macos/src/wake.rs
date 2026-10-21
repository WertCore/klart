//! Noticing that the machine, or just its screens, woke up.
//!
//! A monitor does not reliably keep its brightness across a sleep. Plenty of
//! them come back at full, and the person who dimmed it has to dim it again —
//! which is the single most common complaint made about every tool in this
//! category. The agent is already running and already knows what the level was,
//! so it is in a position to put it back.
//!
//! Two notifications rather than one, because there are two sleeps.
//! `NSWorkspaceDidWake` is the machine waking; `NSWorkspaceScreensDidWake` is
//! the displays coming back while the machine stayed up, which is what a display
//! sleep timeout produces and is just as likely to have reset a monitor. Either
//! one sets the same flag, and the flag is read once per pass, so both firing
//! together costs one restore.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceScreensDidWakeNotification,
};
use objc2_foundation::{MainThreadMarker, NSNotification};

/// Set from the notification, read by the pump.
///
/// The same shape as the display reconfiguration flag in `main`, for the same
/// reason: what the agent has to do about it needs the driver and the main
/// thread, and a notification callback is not the place to reach for either.
static WOKE: AtomicBool = AtomicBool::new(false);

define_class!(
    // SAFETY:
    // - `NSObject` imposes no subclassing requirements.
    // - `Observer` implements no `Drop`.
    #[unsafe(super(NSObject))]
    // `NSWorkspace` delivers its notifications on the main thread.
    #[thread_kind = MainThreadOnly]
    #[name = "KlartWakeObserver"]
    #[ivars = ()]
    pub struct Observer;

    impl Observer {
        #[unsafe(method(woke:))]
        fn woke(&self, _notification: &NSNotification) {
            WOKE.store(true, Ordering::Relaxed);
        }
    }

    unsafe impl NSObjectProtocol for Observer {}
);

/// Registers for both wake notifications.
///
/// The returned observer must be kept: `NSNotificationCenter` does not retain
/// what it sends to, so dropping it leaves a centre holding a dangling pointer
/// and the next wake is a use after free.
#[must_use]
pub fn watch(mtm: MainThreadMarker) -> Retained<Observer> {
    let this = Observer::alloc(mtm).set_ivars(());
    let observer: Retained<Observer> = unsafe { msg_send![super(this), init] };

    // The selector below is a string on both sides — the one `define_class!`
    // registers and the one handed to the notification centre — and nothing
    // checks that they match. If they ever stop matching, the symptom is an
    // unrecognized selector the first time the machine wakes, which is hours
    // away from anyone running this and looks like a crash on resume rather than
    // a typo. Asking here costs nothing and fails where the mistake is.
    debug_assert!(
        observer.respondsToSelector(sel!(woke:)),
        "the wake observer does not answer the selector it is about to register"
    );

    let centre = NSWorkspace::sharedWorkspace().notificationCenter();

    for name in [
        // SAFETY: both are `NSNotificationName` constants exported by AppKit,
        // read while AppKit is loaded.
        unsafe { NSWorkspaceDidWakeNotification },
        unsafe { NSWorkspaceScreensDidWakeNotification },
    ] {
        // SAFETY: the observer is the right type and is kept alive by the
        // caller, and `woke:` is defined above with the signature a notification
        // is sent with.
        unsafe {
            centre.addObserver_selector_name_object(&observer, sel!(woke:), Some(name), None);
        }
    }

    observer
}

/// Whether a wake has happened since this was last asked.
pub fn happened() -> bool {
    WOKE.swap(false, Ordering::Relaxed)
}
