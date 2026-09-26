//! A spike for PLAN entry 10: can a `CGEventTap` see the brightness keys?
//!
//! The whole "take over F1/F2" feature rests on one unknown that no amount of
//! reading settles, because it depends on the machine: on this Mac, does a
//! session-level `CGEventTap` actually receive the brightness keys, and can it
//! swallow them? Lunar's route (a tap, needing Accessibility) works on the Macs
//! Lunar's users have; MonitorControl reaches for the HID layer instead, which
//! reads as evidence that a tap does not always see these keys. The only way to
//! know which is true here is to install one and press the key.
//!
//! This is exploratory and deliberately not part of the shipped binary — it is
//! an example so that it compiles under CI without carrying an event tap into
//! `klart-tray`. It only observes: every event is passed through untouched, so
//! running it changes nothing about how the keys behave. Whether the tap *could*
//! swallow is answered without doing it — see the note by the callback's return.
//!
//! Run it from a terminal that has been granted Accessibility (System Settings →
//! Privacy & Security → Accessibility). Without that grant `tap_create` returns
//! `None`, which is itself the first thing this is here to show. Then press the
//! brightness keys, F1 and F2, and watch what is logged. Ctrl-C to stop.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this spike is for macOS event taps and has nothing to do on this platform");
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_app_kit::NSEvent;
    use objc2_core_foundation::{CFMachPort, CFRunLoop, kCFRunLoopCommonModes};
    use objc2_core_graphics::{
        CGEvent, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
        CGEventType,
    };

    /// The `CGEventType` for a system-defined event, which is what a media key —
    /// brightness included — arrives as. The cut-down binding names `KeyDown`
    /// but not this one, so it is spelled out. `NX_SYSDEFINED` is 14.
    const SYSTEM_DEFINED: u32 = 14;

    /// `NX_SUBTYPE_AUX_CONTROL_BUTTONS`: the system-defined subtype a media key
    /// carries. objc2 happens to name the value 8 `ScreenChanged`; it is the
    /// same number.
    const AUX_BUTTONS_SUBTYPE: i16 = 8;

    /// The aux key codes, from `<IOKit/hidsystem/ev_keymap.h>`. Only the two
    /// this feature is about are named; the rest are logged by number. `isize`
    /// because they are compared against a field of `NSEvent::data1`, which is an
    /// `NSInteger`.
    const NX_KEYTYPE_BRIGHTNESS_UP: isize = 2;
    const NX_KEYTYPE_BRIGHTNESS_DOWN: isize = 3;

    /// The keycodes the brightness keys carry when they arrive as plain function
    /// keys instead — i.e. with "Use F1, F2… as standard function keys" ticked.
    const KEYCODE_F1: i64 = 122;
    const KEYCODE_F2: i64 = 120;

    /// `kCGKeyboardEventKeycode`, the field holding a key-down event's keycode.
    const KEYBOARD_KEYCODE_FIELD: u32 = 9;

    pub fn run() {
        // Both kinds, because which one the brightness keys use depends on the
        // "standard function keys" setting and finding out is half the point:
        // a key-down (10) if they are plain F-keys, a system-defined (14) if
        // they are media keys.
        let mask = mask_bit(CGEventType::KeyDown.0) | mask_bit(SYSTEM_DEFINED);

        // Session level, head of the queue, and *not* ListenOnly — an observing
        // tap could never swallow, and swallow-ability is exactly what is in
        // question. It still passes everything through (see the callback), so
        // nothing is actually swallowed while this runs.
        let tap = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(callback),
                std::ptr::null_mut(),
            )
        };

        let Some(tap) = tap else {
            eprintln!(
                "tap_create returned None.\n\
                 \n\
                 This is finding #1: without Accessibility permission the tap is \
                 refused outright. Grant it to this terminal in System Settings → \
                 Privacy & Security → Accessibility and run again. If it is already \
                 granted and this still prints, that is itself the answer — a \
                 session tap is not permitted here and the HID route is the one \
                 to take."
            );
            return;
        };

        let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0)
            .expect("a fresh mach port yields a run loop source");

        let run_loop = CFRunLoop::current().expect("a thread has a run loop");
        unsafe {
            run_loop.add_source(Some(&source), kCFRunLoopCommonModes);
            CGEvent::tap_enable(&tap, true);
        }

        println!(
            "tap installed at the session level.\n\
             press the brightness keys, and F1 / F2, and watch below.\n\
             every event is passed through untouched. Ctrl-C to stop.\n"
        );

        CFRunLoop::run();
    }

    /// One bit of a `CGEventMask` for an event type.
    fn mask_bit(event_type: u32) -> u64 {
        1u64 << event_type
    }

    /// Logs anything of interest and passes every event through unchanged.
    ///
    /// # The swallow question, answered without swallowing
    ///
    /// This returns the event's own pointer, so nothing is dropped. To *swallow*
    /// an event a callback returns null instead — a one-line change. It is left
    /// undone on purpose: making the brightness keys briefly dead is a poor thing
    /// to do to whoever is running the spike, and the API guarantees that an
    /// event visible to a non-`ListenOnly` tap can be swallowed by returning
    /// null. So if the brightness keys show up in the log at all, swallowing them
    /// is settled too.
    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: NonNull<CGEvent>,
        _user_info: *mut c_void,
    ) -> *mut CGEvent {
        match event_type.0 {
            keydown if keydown == CGEventType::KeyDown.0 => {
                let keycode =
                    CGEvent::integer_value_field(Some(unsafe { event.as_ref() }), keyboard_field());
                let label = match keycode {
                    KEYCODE_F1 => " — F1 (brightness down, as a plain function key)",
                    KEYCODE_F2 => " — F2 (brightness up, as a plain function key)",
                    _ => "",
                };
                println!("key-down: keycode {keycode}{label}");
            }
            SYSTEM_DEFINED => report_system_defined(event),
            // The system disables a tap that is too slow or when the user does
            // something drastic; it says so through the callback. Nothing here is
            // slow, but log it rather than silently going deaf.
            0xFFFF_FFFE => eprintln!("tap disabled by timeout"),
            0xFFFF_FFFF => eprintln!("tap disabled by user input"),
            other => println!("event type {other}"),
        }

        event.as_ptr()
    }

    /// Decodes a system-defined event far enough to say whether it was a
    /// brightness key and whether it was a press.
    fn report_system_defined(event: NonNull<CGEvent>) {
        // The clean way to read a system-defined event's subtype and packed
        // data1 is through the NSEvent that wraps the same CGEvent; CGEvent has
        // no field accessor for them.
        let Some(ns_event) = (unsafe { NSEvent::eventWithCGEvent(event.as_ref()) }) else {
            return;
        };
        // `ns_event` retains the wrapped event; reading from it below is safe.

        if ns_event.subtype().0 != AUX_BUTTONS_SUBTYPE {
            // Some other system-defined event — screen changes and the like.
            return;
        }

        let data1 = ns_event.data1();
        // data1 packs the aux keycode in the high half and flags in the low
        // half; within the flags, byte 2 is the key state and 0x0A means down.
        let key = (data1 >> 16) & 0xFFFF;
        let flags = data1 & 0xFFFF;
        let pressed = (flags & 0xFF00) >> 8 == 0x0A;

        let state = if pressed { "down" } else { "up" };
        match key {
            NX_KEYTYPE_BRIGHTNESS_UP => {
                println!("system-defined: BRIGHTNESS UP ({state}) — the tap sees it");
            }
            NX_KEYTYPE_BRIGHTNESS_DOWN => {
                println!("system-defined: BRIGHTNESS DOWN ({state}) — the tap sees it");
            }
            other => println!("system-defined aux key {other} ({state})"),
        }
    }

    /// The keycode field, wrapped so the raw constant is named once.
    fn keyboard_field() -> objc2_core_graphics::CGEventField {
        objc2_core_graphics::CGEventField(KEYBOARD_KEYCODE_FIELD)
    }
}
