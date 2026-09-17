//! Software dimming, for displays no hardware mechanism reaches.
//!
//! This is not brightness. It scales the values on their way to the panel, so
//! the backlight stays where it was and the picture gets darker — contrast and
//! the number of distinguishable dark tones both go with it. On a display that
//! answers DDC/CI it would be the wrong tool. On one behind an adaptor that does
//! not carry DDC, it is the only tool, and a dimmable display is better than a
//! display stuck at whatever its own buttons were last set to.
//!
//! It also lasts only as long as the process that set it. macOS reverts a
//! display's ramp when that process exits — verified for a clean exit and for
//! `SIGKILL`, while a second process reads the dimmed value for as long as the
//! setter is alive. So a short-lived command that dims a display this way has
//! done nothing by the time it returns, and only a resident process can hold a
//! display dim. [`Backend::persists`] is how a caller finds that out.

use crate::Brightness;
use crate::backend::Backend;
use crate::display::Display;
use crate::error::Result;
use crate::sys::graphics::{self, IDENTITY, Ramp};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "gamma";

/// The dimmest ramp this will write.
///
/// A gamma maximum of zero is a black screen, and a black screen cannot be
/// undone from a menu the user can no longer see. Everything above this floor is
/// recoverable by eye; below it, the only way back is another machine or a
/// reboot. So the whole `0..=100%` range is mapped onto `FLOOR..=1.0` rather
/// than letting the bottom of it be unusable.
const FLOOR: f32 = 0.2;

/// A display's brightness, faked with its gamma ramp.
#[derive(Debug, Clone, Copy)]
pub struct Gamma {
    display: u32,
}

impl Gamma {
    /// Binds to a display.
    ///
    /// Always succeeds where the display exists: every display has a gamma ramp,
    /// which is exactly why this is the last mechanism tried and never the
    /// first.
    ///
    /// # Errors
    ///
    /// Fails only if Core Graphics will not report the display's current ramp,
    /// which means the display has gone away.
    pub fn open(display: &Display) -> Result<Self> {
        graphics::transfer_formula(display.id())?;
        Ok(Self {
            display: display.id(),
        })
    }

    /// Puts every display's ramp back to what ColorSync says it should be.
    ///
    /// Takes no display because it is the panic button rather than an ordinary
    /// operation: it undoes this crate's dimming everywhere at once.
    ///
    /// Not a shutdown path: macOS already reverts a display's ramp when the
    /// process that set it exits, `SIGKILL` included. This is for the case where
    /// something else has left a ramp dark and a caller wants the screens back
    /// now.
    pub fn restore_everything() {
        graphics::restore_colour_sync();
    }
}

impl Backend for Gamma {
    fn name(&self) -> &'static str {
        NAME
    }

    fn persists(&self) -> bool {
        false
    }

    fn get(&self) -> Result<Brightness> {
        Ok(from_ramp(graphics::transfer_formula(self.display)?.max))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        graphics::set_transfer_formula(
            self.display,
            Ramp {
                max: to_ramp(level),
                ..IDENTITY
            },
        )
    }
}

/// The ramp maximum that shows a given level.
fn to_ramp(level: Brightness) -> f32 {
    FLOOR + (1.0 - FLOOR) * level.fraction()
}

/// The level a given ramp maximum is showing.
fn from_ramp(max: f32) -> Brightness {
    Brightness::new((max - FLOOR) / (1.0 - FLOOR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::displays;

    #[test]
    fn the_dimmest_level_still_leaves_something_on_the_screen() {
        assert_eq!(to_ramp(Brightness::MIN), FLOOR);
        assert!(
            to_ramp(Brightness::MIN) > 0.0,
            "a ramp of zero is a screen nobody can undo this from"
        );
    }

    #[test]
    fn the_brightest_level_is_the_ramp_untouched() {
        assert_eq!(to_ramp(Brightness::MAX), 1.0);
    }

    #[test]
    fn levels_round_trip_through_the_ramp() {
        for whole in 0..=100u8 {
            let level = Brightness::from_percent(f32::from(whole));
            assert_eq!(from_ramp(to_ramp(level)).percent_rounded(), whole);
        }
    }

    #[test]
    fn a_ramp_below_the_floor_reads_as_the_dimmest_level_rather_than_a_negative() {
        // Something else — a colour profile, another tool — can leave a ramp
        // darker than this crate would ever write.
        assert_eq!(from_ramp(0.0), Brightness::MIN);
        assert_eq!(from_ramp(-1.0), Brightness::MIN);
    }
    /// Ignored because it visibly darkens a real screen. Run it with
    /// `cargo test --package klart-core -- --ignored --nocapture`.
    #[test]
    #[ignore = "dims a real screen"]
    fn it_dims_a_display_and_puts_it_back() {
        let attached = displays().expect("Core Graphics should enumerate");
        let Some(display) = attached.first() else {
            panic!("no displays attached");
        };
        let ramp = Gamma::open(display).expect("every display has a ramp");

        let original = ramp.get().expect("a ramp should be readable");
        ramp.set(Brightness::from_percent(60.0))
            .expect("a ramp should be writable");
        let observed = ramp.get().expect("a ramp should be readable");

        // Restored before any assertion, so a failure does not leave the screen
        // dim for the rest of the run.
        ramp.set(original).expect("a ramp should be writable");

        assert_eq!(observed.percent_rounded(), 60);
        assert_eq!(
            ramp.get()
                .expect("a ramp should be readable")
                .percent_rounded(),
            original.percent_rounded()
        );
    }
}
