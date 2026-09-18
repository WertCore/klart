//! `klart` in the menu bar.
//!
//! An agent rather than an application: no Dock tile, no window, no main menu.
//! It exists to hold a menu, and — because macOS reverts a gamma ramp the moment
//! the process that set it exits — to be the thing that stays running so a
//! display with no hardware brightness control can stay dim.

mod driver;
mod menu;
mod request;
mod status;

use std::ffi::c_void;
use std::process::ExitCode;
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use klart_core::controls;
use objc2::rc::Retained;
use objc2_app_kit::NSStatusItem;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayChangeSummaryFlags, CGDisplayRegisterReconfigurationCallback,
    CGError,
};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode};

use crate::driver::Driver;
use crate::menu::Built;
use crate::request::Request;

/// How long the pump waits before looking around of its own accord.
///
/// Menu clicks do not depend on this — they arrive as events and are drained the
/// moment AppKit hands control back. It is only the ceiling on how long a
/// display can be plugged in before the menu knows, so it trades one wakeup a
/// second against a menu that is briefly out of date.
const IDLE_WAIT: f64 = 1.0;

/// Set from the display reconfiguration callback, read by the pump.
///
/// An atomic rather than anything larger because the callback runs inside a
/// display reconfiguration, where it may not allocate or take a lock that
/// anything else might hold.
static DISPLAYS_CHANGED: AtomicBool = AtomicBool::new(false);

fn main() -> ExitCode {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("klart-tray: must run on the main thread");
        return ExitCode::FAILURE;
    };

    let app = NSApplication::sharedApplication(mtm);
    // Accessory, so there is no Dock tile and no menu bar of its own. This is
    // what makes it an agent rather than an application, and it is also what
    // `LSUIElement` will say in the bundle — both, because the agent can be run
    // straight from a shell with no bundle at all.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut agent = Agent::start(mtm);

    watch_for_display_changes();

    // The status item exists by now, which is why `finishLaunching` is called by
    // hand rather than letting `run` do it: `run` never returns, and the pump
    // below has to be the thing draining the request queue.
    app.finishLaunching();

    loop {
        pump(&app);

        // Anything a drag could not land while the menu was open, and then the
        // levels it left behind.
        agent.driver.flush();
        agent.driver.persist();

        for asked in request::drain() {
            match asked {
                Request::Quit => return ExitCode::SUCCESS,
                Request::Refresh => agent.look_again(mtm),
            }
        }

        if DISPLAYS_CHANGED.swap(false, Ordering::Relaxed) {
            agent.look_again(mtm);
        }
    }
}

/// Waits for one event and hands it to the application.
fn pump(app: &NSApplication) {
    let deadline = NSDate::dateWithTimeIntervalSinceNow(IDLE_WAIT);

    // SAFETY: called on the main thread, which is where `NSApplication` requires
    // it, with a live deadline and the standard run loop mode.
    let event = unsafe {
        app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&deadline),
            NSDefaultRunLoopMode,
            true,
        )
    };

    if let Some(event) = event {
        app.sendEvent(&event);
    }
}

/// Asks Core Graphics to say when the displays change.
fn watch_for_display_changes() {
    // SAFETY: the callback has the signature the type demands and touches
    // nothing but a static atomic, which is all it is allowed to do from inside
    // a display reconfiguration.
    let status =
        unsafe { CGDisplayRegisterReconfigurationCallback(Some(on_display_change), null_mut()) };

    if status != CGError::Success {
        // Worth saying, not worth stopping for: the pump notices within its idle
        // wait anyway, and "Look for displays again" is in the menu.
        eprintln!(
            "klart-tray: could not register for display changes ({}); the menu will notice them \
             within a second anyway",
            status.0
        );
    }
}

unsafe extern "C-unwind" fn on_display_change(
    _display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    _user_info: *mut c_void,
) {
    // The beginning of a reconfiguration says only that one is coming; the
    // displays are still as they were, and enumerating now would read the old
    // set and then never be told about the new one.
    if flags.contains(CGDisplayChangeSummaryFlags::BeginConfigurationFlag) {
        return;
    }

    DISPLAYS_CHANGED.store(true, Ordering::Relaxed);
}

/// The status item, the displays behind it, and the menu in front of them.
struct Agent {
    item: Retained<NSStatusItem>,
    /// Shared with the menu's controller, which needs it during a drag.
    driver: Rc<Driver>,
    /// Kept because `NSControl` holds its target weakly: drop this and the
    /// sliders stop reporting.
    menu: Built,
}

impl Agent {
    fn start(mtm: MainThreadMarker) -> Self {
        // A failure to enumerate is not a failure to start. The agent's whole
        // job is to be there when a display appears, so it starts with an empty
        // menu and picks them up when they arrive.
        let driver = Rc::new(Driver::new(controls().unwrap_or_default()));

        // Before the menu is built, so the levels in it are the ones the
        // displays are actually sitting at.
        driver.restore();

        let menu = menu::build(mtm, &driver);

        let item = status::install(mtm);
        item.setMenu(Some(&menu.menu));

        Self { item, driver, menu }
    }

    /// Enumerates again, for when the displays have changed under it.
    fn look_again(&mut self, mtm: MainThreadMarker) {
        match controls() {
            Ok(found) => self.driver.replace(found),
            Err(problem) => eprintln!("klart-tray: {problem}"),
        }

        // A display that has just appeared has whatever level it powered on
        // with, which for a gamma-dimmed one is none of this process's doing.
        self.driver.restore();

        self.menu = menu::build(mtm, &self.driver);
        self.item.setMenu(Some(&self.menu.menu));
    }
}
