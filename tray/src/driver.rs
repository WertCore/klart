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

/// How long to keep trying to put levels back after a wake.
///
/// A monitor does not answer DDC/CI the moment the machine wakes. The link comes
/// back before the scaler behind it does, and in that window a write is accepted
/// and dropped — so a single write on the notification reports success and
/// changes nothing. Fifteen seconds is longer than any display seen here has
/// taken and short enough that a monitor which is simply never going to answer
/// stops being asked.
const RESTORE_WINDOW: Duration = Duration::from_secs(15);

/// The gap between attempts inside that window.
///
/// Shorter than the pump's idle wait, so in practice this is once a pass and the
/// gap only matters when something else is driving the loop faster.
const RESTORE_RETRY: Duration = Duration::from_millis(500);

/// How far a read-back may sit from what was asked for and still count.
///
/// A monitor with a range that is not a hundred quantises: asking for 40% of a
/// range of 64 gives 26, which reads back as 41%. That is the display agreeing,
/// not disagreeing, and retrying it for fifteen seconds would be wrong.
const TOLERANCE: u8 = 2;

/// A restore that is still being attempted.
///
/// The clock lives here rather than in the loop that drives it, so that the
/// rules about when to try again and when to stop can be tested without a
/// display to try them against.
struct Restoring {
    /// Indices of displays not yet confirmed back at their level.
    outstanding: Vec<usize>,
    /// When to give up on whatever is left.
    until: Instant,
    /// When the next attempt may go out.
    next: Instant,
}

impl Restoring {
    /// Everything on the list, from now until the window closes.
    fn new(outstanding: Vec<usize>, now: Instant) -> Self {
        Self {
            outstanding,
            until: now + RESTORE_WINDOW,
            // The first attempt goes out on the next pass rather than after a
            // gap: the caller has just been told the machine woke.
            next: now,
        }
    }

    /// Whether an attempt may go out, booking the next one if so.
    fn may_attempt(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + RESTORE_RETRY;
        true
    }

    /// Whether there is no more time for the displays still on the list.
    fn out_of_time(&self, now: Instant) -> bool {
        now >= self.until
    }
}

/// Whether a display that was asked for one level and reports another is
/// nonetheless agreeing.
///
/// A monitor whose range is not a hundred quantises: asking for 40% of a range
/// of 64 gives 26, which reads back as 41%. Retrying that for the whole window
/// and then reporting it as a failure would be wrong twice over.
fn close_enough(reached: Brightness, wanted: Brightness) -> bool {
    reached.percent_rounded().abs_diff(wanted.percent_rounded()) <= TOLERANCE
}

/// The displays, and what has recently been asked of them.
pub struct Driver {
    controls: RefCell<Vec<Control>>,
    last_written: RefCell<Vec<Instant>>,
    /// The level each display was last left at, across runs.
    remembered: RefCell<Remembered>,
    /// A value for every display that arrived too soon after the last write.
    pending_all: Cell<Option<u8>>,
    /// A value that arrived too soon after the last write.
    ///
    /// Held rather than dropped because the value most likely to fall inside the
    /// gap is the last one of a drag, which is the only one the person actually
    /// chose. Dropping it leaves the display a step away from the slider.
    pending: Cell<Option<(usize, u8)>>,
    /// A wake restore in progress, if there is one.
    restoring: RefCell<Option<Restoring>>,
}

impl Driver {
    pub fn new(controls: Vec<Control>) -> Self {
        let last_written = vec![far_enough_back(); controls.len()];
        Self {
            controls: RefCell::new(controls),
            last_written: RefCell::new(last_written),
            remembered: RefCell::new(Remembered::load()),
            pending_all: Cell::new(None),
            pending: Cell::new(None),
            restoring: RefCell::new(None),
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
        self.pending_all.set(None);
        // A restore in flight is a list of positions in the set being replaced,
        // and those positions now mean different displays. Dropping it is not a
        // loss: whatever prompted the new set prompts its own restore.
        *self.restoring.borrow_mut() = None;
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
                eprintln!("klart-tray: {}: {problem}", control.name());
            }
        }
    }

    /// Begins putting every remembered level back, after a wake.
    ///
    /// Every display this time, not only the ones that cannot hold a level
    /// themselves. [`restore`](Self::restore) skips a monitor whose backlight
    /// persists, on the grounds that it keeps its own setting and overruling it
    /// would undo whatever its buttons had been used for since. Across a sleep
    /// that reasoning does not hold: a great many monitors come back at full
    /// brightness of their own accord, and nobody pressed anything while the
    /// machine was asleep. So the balance tips the other way and the level goes
    /// back.
    ///
    /// The cost of that is a monitor adjusted by its own buttons and then slept
    /// comes back where klart last left it rather than where its buttons did.
    /// That is a real regression for somebody, and it is the lesser one: the
    /// setting klart restores is one the person chose too, and it is the one
    /// they chose most recently through the only route klart can see.
    ///
    /// Nothing is written here. The first attempt goes out on the next pass,
    /// because the point of this is that the displays are not ready yet.
    pub fn restore_after_wake(&self) {
        let controls = self.controls.borrow();
        let remembered = self.remembered.borrow();

        let outstanding = controls
            .iter()
            .enumerate()
            .filter(|(_, control)| {
                // A display nothing reaches would fail every attempt and be
                // reported at the end as though something had gone wrong, when
                // the truth is there was never a way in.
                control.mechanism().is_some()
                    && remembered.level_for(control.display().key()).is_some()
            })
            .map(|(display, _)| display)
            .collect::<Vec<_>>();

        if outstanding.is_empty() {
            return;
        }

        *self.restoring.borrow_mut() = Some(Restoring::new(outstanding, Instant::now()));
    }

    /// Carries on a restore begun by [`restore_after_wake`](Self::restore_after_wake).
    ///
    /// Called from the pump, so a display that is not ready is simply asked
    /// again on the next pass rather than blocking the agent in a sleep loop.
    pub fn drive_restore(&self) {
        let mut slot = self.restoring.borrow_mut();
        let Some(restoring) = slot.as_mut() else {
            return;
        };

        let now = Instant::now();
        if !restoring.may_attempt(now) {
            return;
        }

        let controls = self.controls.borrow();
        let remembered = self.remembered.borrow();

        restoring.outstanding.retain(|&display| {
            let Some(control) = controls.get(display) else {
                return false;
            };
            let Some(level) = remembered.level_for(control.display().key()) else {
                return false;
            };
            !settle(control, level)
        });

        if restoring.outstanding.is_empty() {
            *slot = None;
            return;
        }

        // Out of time. Say which displays would not take it — once, here, rather
        // than on every attempt, because thirty identical lines describe one
        // problem and read like thirty.
        if restoring.out_of_time(now) {
            for &display in &restoring.outstanding {
                if let Some(control) = controls.get(display) {
                    eprintln!(
                        "klart-tray: {}: did not go back to its level after waking",
                        control.name()
                    );
                }
            }
            *slot = None;
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

    /// Asks for a level on every display at once.
    ///
    /// Absolute rather than relative: every display goes to the level asked for,
    /// rather than each moving by the same amount from where it was. Moving them
    /// by a delta would preserve whatever balance had been set between them,
    /// which is the nicer property right up until one of them saturates and the
    /// balance is silently lost anyway. Setting them all is predictable at every
    /// point in the range, and predictable wins in a control someone drags.
    pub fn request_all(&self, percent: u8) {
        let count = self.controls.borrow().len();

        // Every display, not up to the first that is rate limited: `all` and
        // `any` both short circuit, which would leave the rest unwritten.
        let mut landed = true;
        for display in 0..count {
            landed &= self.write(display, percent);
        }

        // Held as one value rather than per display: the last position of a drag
        // is the one that was chosen, and it is the same for all of them.
        if !landed {
            self.pending_all.set(Some(percent));
        }
    }

    /// The level to start a combined control at.
    ///
    /// The mean of what the displays are at. Any single display's level would be
    /// an arbitrary choice, and a fixed position would jump the moment it was
    /// touched.
    pub fn average(&self) -> u8 {
        let controls = self.controls.borrow();
        let levels: Vec<u16> = controls
            .iter()
            .filter_map(|control| control.get().ok())
            .map(|level| u16::from(level.percent_rounded()))
            .collect();

        if levels.is_empty() {
            return 0;
        }
        // `u16` and integer division: the sum of eight percentages cannot
        // overflow it, and the result is a percentage either way.
        u8::try_from(levels.iter().sum::<u16>() / u16::try_from(levels.len()).unwrap_or(1))
            .unwrap_or(100)
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

        if let Some(percent) = self.pending_all.get() {
            let count = self.controls.borrow().len();

            // Same reason as `request_all`: short circuiting here would land the
            // held value on one display and drop it for the others.
            let mut landed = true;
            for display in 0..count {
                landed &= self.write(display, percent);
            }
            if landed {
                self.pending_all.set(None);
            }
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
            eprintln!("klart-tray: {}: {problem}", control.name());
            return true;
        }

        self.remembered
            .borrow_mut()
            .remember(control.display().key(), level);
        true
    }
}

/// Puts a level back, and says whether the display is now at it.
///
/// The read-back is the whole point. A link that has come back before the panel
/// behind it takes a write and drops it, and reports no error for doing so —
/// which is indistinguishable from success at the moment of writing and is the
/// reason "restore on wake" is usually reported as not working rather than as
/// missing. Asking the display what it is at afterwards is the only thing that
/// tells the two apart.
fn settle(control: &Control, level: Brightness) -> bool {
    // Silent: a refusal here means "not yet", which is the expected state for
    // the first second or two and is not worth a line each time. The one that
    // matters is reported by the caller when the window closes.
    if control.set(level).is_err() {
        return false;
    }

    match control.get() {
        Ok(reached) => close_enough(reached, level),
        // The write was accepted and the display will not say where it landed.
        // There is nothing further to learn by asking again, and retrying for
        // the whole window on a display that took the write would be a made-up
        // failure at the end of it.
        Err(_) => true,
    }
}

/// An instant far enough in the past that the first write is never held back.
fn far_enough_back() -> Instant {
    Instant::now()
        .checked_sub(WRITE_GAP)
        .unwrap_or_else(Instant::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restore that nothing answers must stop of its own accord.
    ///
    /// The agent runs for weeks. A monitor that will never take a DDC write —
    /// and this repository was written against one — would otherwise be asked
    /// twice a second forever, for every wake, with nothing ever clearing it.
    #[test]
    fn a_restore_gives_up_when_the_window_closes() {
        let start = Instant::now();
        let restoring = Restoring::new(vec![0], start);

        assert!(
            !restoring.out_of_time(start),
            "a restore must have time at the moment it begins"
        );
        assert!(
            !restoring.out_of_time(start + RESTORE_WINDOW - Duration::from_millis(1)),
            "a display answering just inside the window has not run out of time"
        );
        assert!(
            restoring.out_of_time(start + RESTORE_WINDOW),
            "the window has to close, or a display nothing reaches is retried forever"
        );
    }

    /// The first attempt is not delayed.
    ///
    /// A display on the gamma ramp is ready immediately and is the common case;
    /// making it wait for a retry gap would put a visible flash of the wrong
    /// brightness on every wake.
    #[test]
    fn the_first_attempt_goes_out_at_once() {
        let start = Instant::now();
        let mut restoring = Restoring::new(vec![0], start);

        assert!(restoring.may_attempt(start));
    }

    #[test]
    fn attempts_are_spaced_by_the_retry_gap() {
        let start = Instant::now();
        let mut restoring = Restoring::new(vec![0], start);

        assert!(restoring.may_attempt(start));
        assert!(
            !restoring.may_attempt(start + RESTORE_RETRY - Duration::from_millis(1)),
            "a second attempt inside the gap would queue writes the bus cannot drain"
        );
        assert!(restoring.may_attempt(start + RESTORE_RETRY));
    }

    /// Asking and not booking the next attempt would spin.
    #[test]
    fn a_refused_attempt_does_not_move_the_clock() {
        let start = Instant::now();
        let mut restoring = Restoring::new(vec![0], start);

        assert!(restoring.may_attempt(start));
        let too_soon = start + Duration::from_millis(1);
        assert!(!restoring.may_attempt(too_soon));
        assert!(
            !restoring.may_attempt(too_soon),
            "a refusal must not push the next attempt further out"
        );
        assert!(restoring.may_attempt(start + RESTORE_RETRY));
    }

    /// The quantisation case from `close_enough`'s own documentation.
    #[test]
    fn a_quantised_read_back_counts_as_agreement() {
        // 40% of a range of 64 is 25.6, which the display stores as 26 and
        // reports back as 40.6% — a number klart never asked for and must not
        // treat as a refusal.
        let wanted = Brightness::from_percent(40.0);
        let reached = Brightness::from_range(26, 64);

        assert!(
            close_enough(reached, wanted),
            "{}% read back from a range of 64 should agree with 40%",
            reached.percent_rounded()
        );
    }

    #[test]
    fn a_display_still_at_full_does_not_count_as_agreement() {
        // The failure this whole mechanism exists for: the monitor came back at
        // 100% and the write was accepted and dropped.
        assert!(!close_enough(
            Brightness::from_percent(100.0),
            Brightness::from_percent(40.0)
        ));
    }

    #[test]
    fn the_tolerance_is_not_wide_enough_to_hide_a_real_difference() {
        let wanted = Brightness::from_percent(40.0);

        assert!(close_enough(
            Brightness::from_percent(f32::from(40 + TOLERANCE)),
            wanted
        ));
        assert!(
            !close_enough(
                Brightness::from_percent(f32::from(40 + TOLERANCE + 1)),
                wanted
            ),
            "a difference past the tolerance is the display disagreeing"
        );
    }
}
