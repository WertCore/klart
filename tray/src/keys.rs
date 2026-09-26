//! Taking over the brightness keys.
//!
//! The reason to want any of this is that F1 and F2 do nothing to an external
//! monitor: macOS sends their brightness to the built-in panel and stops there.
//! So klart installs a `CGEventTap` — the same route Lunar takes — catches the
//! brightness keys before the system acts on them, drives the display under the
//! pointer instead, and swallows the event so the built-in panel does not also
//! move. The spike in `PLAN.md` entry 10 confirmed the tap sees these keys on
//! this hardware and can swallow them.
//!
//! It needs Accessibility, which is not klart's to grant. Without it the tap is
//! refused and this stays dormant rather than half-working; the menu says so and
//! the whole feature is otherwise absent. Every competitor pays the same price,
//! so it is what a person expects rather than a surprise.
//!
//! The keys arrive as *system-defined* events — a media key, not a function key
//! — which is the case when "Use F1, F2… as standard function keys" is off, its
//! default. When it is on, F1/F2 are ordinary function keys that mean brightness
//! to nobody, so there is deliberately nothing to catch.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;

use objc2_app_kit::NSEvent;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CFString,
    kCFBooleanTrue, kCFRunLoopCommonModes, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use objc2_core_graphics::{
    CGEvent, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType,
};

use crate::driver::Driver;

/// The `CGEventType` of a system-defined event, which is what a media key is.
/// The cut-down binding names `KeyDown` but not this; `NX_SYSDEFINED` is 14.
const SYSTEM_DEFINED: u32 = 14;

/// The event types the system uses to say it has switched a tap off: too slow to
/// answer, or disabled after a burst of input. Both are recoverable by turning
/// it back on.
const DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS`, the system-defined subtype a media key
/// carries. objc2 names the value 8 `ScreenChanged`; it is the same number.
const AUX_BUTTONS_SUBTYPE: i16 = 8;

/// The aux key codes for the two keys this is about, from
/// `<IOKit/hidsystem/ev_keymap.h>`. `isize` to compare against a field of
/// `NSEvent::data1`, which is an `NSInteger`.
const NX_KEYTYPE_BRIGHTNESS_UP: isize = 2;
const NX_KEYTYPE_BRIGHTNESS_DOWN: isize = 3;

/// How far one press moves the level, in percentage points.
///
/// A sixteenth of the range, which is the step macOS itself uses for the
/// built-in panel — so a key does the same amount here as it does there, and the
/// two do not feel like different controls.
const STEP: f32 = 100.0 / 16.0;

/// The installed tap, held for as long as the keys are wanted.
///
/// Dropping it removes the run loop source and lets the tap go, which is exactly
/// what stopping the feature would mean. The agent keeps it for its whole life.
pub struct Keys {
    _tap: CFRetained<CFMachPort>,
    _source: CFRetained<CFRunLoopSource>,
}

// The tap, reached by the callback only to turn it back on after the system has
// switched it off. Main-thread only, like everything in this crate, so a
// thread-local is all the sharing it needs — and it keeps the callback's
// `user_info` free to be the one thing it needs on the hot path, the driver.
thread_local! {
    static TAP: RefCell<Option<CFRetained<CFMachPort>>> = const { RefCell::new(None) };
}

// AXIsProcessTrusted and its options form live in ApplicationServices. They
// return a Core Foundation `Boolean`, which is a byte rather than a Rust `bool`,
// so they are typed as `u8` and read as non-zero rather than transmuted.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

/// Whether Accessibility has been granted, so the tap may be installed.
pub fn trusted() -> bool {
    // SAFETY: a plain predicate with no arguments.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Installs the tap, if Accessibility allows it.
///
/// Returns [`None`] when it does not — the feature is then simply absent, and
/// [`trusted`] is what the menu uses to explain that. On the first run this also
/// asks macOS to show its grant dialog; the grant takes effect on the next
/// launch, as it does for every app that needs this.
///
/// The `driver` outlives the returned [`Keys`]: its address is handed to the tap
/// as the callback's context. The agent owns both and drops them together, so
/// the pointer is valid for as long as the callback can run.
pub fn install(driver: &Rc<Driver>) -> Option<Keys> {
    if !trusted() {
        prompt_for_trust();
        return None;
    }

    let context = Rc::as_ptr(driver).cast::<c_void>().cast_mut();

    // Only system-defined events: the brightness keys are media keys, and asking
    // for anything else would put every keystroke through this callback for no
    // reason.
    let mask = 1u64 << SYSTEM_DEFINED;

    // Session level so it sees the keys wherever focus is; head of the queue so
    // it acts before anything downstream; and not ListenOnly, because the point
    // is to swallow.
    // SAFETY: the callback has the required signature and the context pointer is
    // valid for the tap's life, as described above.
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            mask,
            Some(on_event),
            context,
        )
    }?;

    let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0)?;
    let run_loop = CFRunLoop::current()?;

    // SAFETY: on the main thread, with a live source and the common modes.
    unsafe {
        run_loop.add_source(Some(&source), kCFRunLoopCommonModes);
        CGEvent::tap_enable(&tap, true);
    }

    TAP.with(|held| *held.borrow_mut() = Some(tap.clone()));

    Some(Keys {
        _tap: tap,
        _source: source,
    })
}

/// Asks macOS to show the Accessibility grant dialog, with klart pre-listed.
///
/// The options dictionary is a single flag, `AXTrustedCheckOptionPrompt`. Built
/// by hand because that key is not among the constants the bindings expose; its
/// string value is the constant's documented spelling.
fn prompt_for_trust() {
    let key = CFString::from_static_str("AXTrustedCheckOptionPrompt");
    // SAFETY: an immutable Core Foundation constant, read only.
    let Some(truth) = (unsafe { kCFBooleanTrue }) else {
        return;
    };

    let mut keys = [(&*key as *const CFString).cast::<c_void>()];
    let mut values = [(truth as *const CFBoolean).cast::<c_void>()];

    // SAFETY: one key and one value, both live for the call, with Core
    // Foundation's own callbacks for retaining them.
    let options = unsafe {
        CFDictionary::new(
            None,
            keys.as_mut_ptr(),
            values.as_mut_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    let Some(options) = options else {
        return;
    };

    // SAFETY: a valid options dictionary. The return — current trust, which is
    // still false here — is not the point; the side effect of showing the dialog
    // is.
    unsafe {
        AXIsProcessTrustedWithOptions((&*options as *const CFDictionary).cast());
    }
}

/// The tap callback: act on a brightness key, pass everything else along.
///
/// # Safety
///
/// Called by the system with the event and the context pointer given to
/// [`install`]. Returning the event passes it on; returning null swallows it.
unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    match event_type.0 {
        SYSTEM_DEFINED => {
            if let Some(delta) = brightness(event) {
                // A press moves a display; a release does nothing but is still
                // swallowed, so the system never sees half of a key it did not
                // get the other half of.
                if delta != 0.0 && !user_info.is_null() {
                    // SAFETY: the driver outlives the tap, as install requires.
                    let driver = unsafe { &*user_info.cast::<Driver>() };
                    // SAFETY: a live event; its location is the pointer's, in the
                    // same global top-left coordinates as the displays' bounds.
                    let at = CGEvent::location(Some(unsafe { event.as_ref() }));
                    let index = driver.display_at(at.x as i32, at.y as i32);
                    driver.key_step(index, delta);
                }
                return null_mut();
            }
        }
        // The system switched the tap off; turn it back on. Without this a slow
        // moment or a burst of input would leave the keys dead until relaunch.
        DISABLED_BY_TIMEOUT | DISABLED_BY_USER_INPUT => {
            TAP.with(|held| {
                if let Some(tap) = held.borrow().as_ref() {
                    CGEvent::tap_enable(tap, true);
                }
            });
        }
        _ => {}
    }

    event.as_ptr()
}

/// How far a brightness key asks the level to move, or [`None`] if the event is
/// not a brightness key.
///
/// [`Some(0.0)`] for the release: the caller swallows it without moving anything.
fn brightness(event: NonNull<CGEvent>) -> Option<f32> {
    // The subtype and packed data of a system-defined event are read through the
    // NSEvent that wraps it; CGEvent has no accessor for them.
    // SAFETY: a live CGEvent.
    let ns = unsafe { NSEvent::eventWithCGEvent(event.as_ref()) }?;

    if ns.subtype().0 != AUX_BUTTONS_SUBTYPE {
        return None;
    }

    let data1 = ns.data1();
    // The aux keycode is the high half of data1; the low half holds flags whose
    // second byte is the key state, 0x0A being a press.
    let key = (data1 >> 16) & 0xFFFF;
    let pressed = (data1 & 0xFF00) >> 8 == 0x0A;

    match key {
        NX_KEYTYPE_BRIGHTNESS_UP => Some(if pressed { STEP } else { 0.0 }),
        NX_KEYTYPE_BRIGHTNESS_DOWN => Some(if pressed { -STEP } else { 0.0 }),
        _ => None,
    }
}
