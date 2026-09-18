//! Applying levels, at a rate the displays can take.
//!
//! Shared between the menu's controller and the agent, because a slider drag has
//! to reach a display *while the menu is open* and the agent is not running
//! then: AppKit tracks an open menu in a loop of its own, and the pump in
//! `main` does not get control back until the menu closes. Anything queued for
//! the pump would land after the fact, all at once, which is no use to someone
//! watching the screen as they drag.

use std::cell::{Cell, Ref, RefCell};
use std::time::{Duration, Instant};

use klart_core::{Brightness, Control, Remembered};

/// The shortest gap between two writes to one display.
///
/// A drag produces a value every few milliseconds and a DDC/CI link needs tens
/// of milliseconds a message, so handing it every value would queue writes
/// faster than the bus drains them and the monitor would trail the pointer by
/// seconds.
const WRITE_GAP: Duration = Duration::from_millis(60);

/// The displays, and what has recently been asked of them.
pub struct Driver {
    controls: RefCell<Vec<Control>>,
    last_written: RefCell<Vec<Instant>>,
    /// The level each display was last left at, across runs.
    remembered: RefCell<Remembered>,
    /// A value that arrived too soon after the last write.
    ///
    /// Held rather than dropped because the value most likely to fall inside the
    /// gap is the last one of a drag, which is the only one the person actually
    /// chose. Dropping it leaves the display a step away from the slider.
    pending: Cell<Option<(usize, u8)>>,
}

impl Driver {
    pub fn new(controls: Vec<Control>) -> Self {
        let last_written = vec![far_enough_back(); controls.len()];
        Self {
            controls: RefCell::new(controls),
            last_written: RefCell::new(last_written),
            remembered: RefCell::new(Remembered::load()),
            pending: Cell::new(None),
        }
    }

    /// The displays, for reading levels out of when the menu is built.
    pub fn controls(&self) -> Ref<'_, Vec<Control>> {
        self.controls.borrow()
    }

    /// Takes a new set of displays, discarding anything owed to the old ones.
    pub fn replace(&self, controls: Vec<Control>) {
        *self.last_written.borrow_mut() = vec![far_enough_back(); controls.len()];
        *self.controls.borrow_mut() = controls;
        self.pending.set(None);
    }

    /// Puts back the levels of displays that could not keep their own.
    ///
    /// Deliberately not every display. A monitor whose backlight this can move
    /// remembers its own setting, through a reconnect and through a reboot, so
    /// there is nothing to put back — and putting one back anyway would overrule
    /// whatever the person did with the brightness keys or the monitor's own
    /// buttons since. A display dimmed with its gamma ramp really has lost the
    /// setting, every time this process exits, and is the case this exists for.
    ///
    /// `klart restore` on the command line does restore everything, because
    /// there it was asked for rather than assumed.
    pub fn restore(&self) {
        let controls = self.controls.borrow();
        let remembered = self.remembered.borrow();

        for control in controls.iter() {
            if control.persists() {
                continue;
            }
            let Some(level) = remembered.level_for(control.display().key()) else {
                continue;
            };
            if let Err(problem) = control.set(level) {
                eprintln!("klart-tray: {}: {problem}", control.display().name());
            }
        }
    }

    /// Writes out anything remembered since the last time, if there is any.
    ///
    /// Called from the pump rather than from the write path: a drag records a
    /// level every sixty milliseconds and the file system has no reason to hear
    /// about all of them.
    pub fn persist(&self) {
        if let Err(problem) = self.remembered.borrow_mut().save() {
            eprintln!("klart-tray: could not save levels: {problem}");
        }
    }

    /// Asks for a level, now if the display will take it and shortly if not.
    pub fn request(&self, display: usize, percent: u8) {
        if !self.write(display, percent) {
            self.pending.set(Some((display, percent)));
        }
    }

    /// Lands whatever was held back, once its display will take it.
    ///
    /// Called from the pump, which in practice means the moment the menu closes.
    pub fn flush(&self) {
        if let Some((display, percent)) = self.pending.get()
            && self.write(display, percent)
        {
            self.pending.set(None);
        }
    }

    /// Writes a level, or reports that it was too soon.
    ///
    /// A display that is no longer there counts as written: there is nothing to
    /// retry against, and holding the value would keep retrying forever.
    fn write(&self, display: usize, percent: u8) -> bool {
        let controls = self.controls.borrow();
        let mut last_written = self.last_written.borrow_mut();

        let (Some(control), Some(last)) = (controls.get(display), last_written.get_mut(display))
        else {
            return true;
        };

        if last.elapsed() < WRITE_GAP {
            return false;
        }
        *last = Instant::now();

        let level = Brightness::from_percent(f32::from(percent));
        if let Err(problem) = control.set(level) {
            eprintln!("klart-tray: {}: {problem}", control.display().name());
            return true;
        }

        self.remembered
            .borrow_mut()
            .remember(control.display().key(), level);
        true
    }
}

/// An instant far enough in the past that the first write is never held back.
fn far_enough_back() -> Instant {
    Instant::now()
        .checked_sub(WRITE_GAP)
        .unwrap_or_else(Instant::now)
}
