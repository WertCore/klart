//! A brightness shortcut that needs no permission.
//!
//! The event tap in `keys.rs` is the good path, but it needs Accessibility, and
//! someone may not grant it — or may be on a keyboard with no brightness keys to
//! take over in the first place. So klart also registers a plain global hotkey,
//! `⌃⌥↑` and `⌃⌥↓`, through Carbon's `RegisterEventHotKey`. That API asks for no
//! permission and works the moment it is installed; the cost is that the chord is
//! invented rather than the keys already on the keyboard, which is why it is the
//! fallback and not the headline.
//!
//! It is registered whether or not the tap is, because there is no harm in both
//! — the chord is different keys — and a keyboard without brightness keys has
//! nothing else. The menu shows the chord when the tap is not available, so it is
//! discoverable exactly when it is the only way in.
//!
//! Carbon is old and deprecated, and this is the one corner of klart that uses
//! it, because `RegisterEventHotKey` has no modern replacement that avoids the
//! permission. It acts on the display under the pointer, through the same
//! [`Driver`] the tap and the scroll use.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::rc::Rc;

use objc2_core_graphics::CGEvent;

use crate::driver::Driver;
use crate::keys::STEP;

/// A four-character code, as Carbon's event constants are written.
const fn four_cc(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | (code[3] as u32)
}

const K_EVENT_CLASS_KEYBOARD: u32 = four_cc(b"keyb");
const K_EVENT_HOTKEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = four_cc(b"----");
const TYPE_EVENT_HOTKEY_ID: u32 = four_cc(b"hkid");

/// The Carbon modifier masks, from `<Carbon/Carbon.h>`.
const CONTROL_KEY: u32 = 1 << 12;
const OPTION_KEY: u32 = 1 << 11;

/// The arrow-key virtual keycodes.
const KEYCODE_UP: u32 = 126;
const KEYCODE_DOWN: u32 = 125;

/// Tells klart's hotkeys apart from any other program's, and our two from each
/// other.
const SIGNATURE: u32 = four_cc(b"klrt");
const ID_UP: u32 = 1;
const ID_DOWN: u32 = 2;

/// Whether the chord is registered, read by the menu to know whether to offer
/// it. Written once at startup and read on the same thread.
///
/// `Cell` is not `Sync`, which a `static` demands, so it is wrapped: every access
/// is on the main thread, as the whole crate assumes.
struct MainThreadOnly(Cell<bool>);
// SAFETY: every access is on the main thread. See the module docs.
unsafe impl Sync for MainThreadOnly {}
static ACTIVE_FLAG: MainThreadOnly = MainThreadOnly(Cell::new(false));

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type OsStatus = i32;
type EventHandlerUpp =
    unsafe extern "C-unwind" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OsStatus;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUpp,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OsStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        param_type: u32,
        out_actual_type: *mut u32,
        buffer_size: usize,
        out_actual_size: *mut usize,
        out_data: *mut c_void,
    ) -> OsStatus;
}

/// Whether the chord is registered and will move the brightness.
pub fn active() -> bool {
    ACTIVE_FLAG.0.get()
}

/// Registers the chord, if the system will have it.
///
/// The `driver` outlives this: its address is the handler's context, and the
/// registration lives until the process exits (the OS drops it then), so there
/// is nothing to hold onto here. The agent keeps the driver, which is what keeps
/// the pointer valid.
pub fn install(driver: &Rc<Driver>) {
    // SAFETY: returns the process-wide application event target, or null.
    let target = unsafe { GetApplicationEventTarget() };
    if target.is_null() {
        return;
    }

    let spec = EventTypeSpec {
        event_class: K_EVENT_CLASS_KEYBOARD,
        event_kind: K_EVENT_HOTKEY_PRESSED,
    };
    let context = Rc::as_ptr(driver).cast::<c_void>().cast_mut();

    // SAFETY: a valid target, our handler with the required signature, one event
    // spec, and a context that outlives the handler.
    let mut handler: EventHandlerRef = null_mut();
    let installed =
        unsafe { InstallEventHandler(target, on_hotkey, 1, &spec, context, &mut handler) };
    if installed != 0 {
        return;
    }

    let modifiers = CONTROL_KEY | OPTION_KEY;
    let up = register(KEYCODE_UP, modifiers, ID_UP, target);
    let down = register(KEYCODE_DOWN, modifiers, ID_DOWN, target);

    // Both, or the pair is lopsided — brighter without dimmer is not worth
    // offering, and something else already holds the chord. Say so quietly; the
    // tap may still cover it, and the menu will simply not mention the chord.
    if up && down {
        ACTIVE_FLAG.0.set(true);
    } else {
        eprintln!("klart-tray: the brightness chord is already taken by another app");
    }
}

/// Registers one hotkey, returning whether it took.
fn register(code: u32, modifiers: u32, id: u32, target: EventTargetRef) -> bool {
    let identity = EventHotKeyID {
        signature: SIGNATURE,
        id,
    };
    let mut reference: EventHotKeyRef = null_mut();

    // SAFETY: a valid target and an out pointer; the reference is left to the OS,
    // which drops it at exit.
    let status =
        unsafe { RegisterEventHotKey(code, modifiers, identity, target, 0, &mut reference) };
    status == 0
}

/// Handles a chord press: move the display under the pointer.
///
/// # Safety
///
/// Called by Carbon with a live event and the context given to [`install`].
unsafe extern "C-unwind" fn on_hotkey(
    _next: EventHandlerCallRef,
    event: EventRef,
    user_data: *mut c_void,
) -> OsStatus {
    let mut identity = EventHotKeyID {
        signature: 0,
        id: 0,
    };
    let mut actual_size = 0usize;

    // SAFETY: reading the hotkey id the event carries into a matching struct.
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOTKEY_ID,
            null_mut(),
            size_of::<EventHotKeyID>(),
            &mut actual_size,
            (&raw mut identity).cast(),
        )
    };
    if status != 0 || identity.signature != SIGNATURE {
        return status;
    }

    let delta = match identity.id {
        ID_UP => STEP,
        ID_DOWN => -STEP,
        _ => return 0,
    };

    if !user_data.is_null() {
        // SAFETY: the driver outlives the registration, as install requires.
        let driver = unsafe { &*user_data.cast::<Driver>() };
        // A fresh event created with no source carries the current pointer
        // location, in the global top-left coordinates the bounds use.
        if let Some(now) = CGEvent::new(None) {
            let at = CGEvent::location(Some(&now));
            let index = driver.display_at(at.x as i32, at.y as i32);
            driver.key_step(index, delta);
        }
    }

    // Consumed.
    0
}
