//! Dimming past the point where the backlight stops.
//!
//! A backlight has a minimum, and on a laptop panel in a dark room that minimum
//! is still too bright. Below it the only thing left is the gamma ramp, which
//! darkens the picture rather than the light behind it — a poor way to set
//! brightness and the only way to go lower.
//!
//! So the two are composed rather than chosen between. The bottom
//! [`SOFTWARE_SHARE`] of the range holds the backlight at its minimum and dims
//! with the ramp; everything above it drives the backlight and leaves the ramp
//! alone. The seam is continuous: at exactly the share, the backlight is at its
//! minimum and the ramp is untouched, which is the same display state either
//! branch would produce.
//!
//! Nothing here is platform-specific. It composes two [`Backend`]s and does not
//! care which.

use std::cell::Cell;

use crate::Brightness;
use crate::backend::Backend;
use crate::error::Result;

/// How much of the range is given to the ramp.
///
/// The bottom quarter. Enough to be worth having — a quarter of a slider is a
/// usable amount of travel — without spending so much of it on a mechanism that
/// costs contrast.
const SOFTWARE_SHARE: f32 = 0.25;

/// Below this the backlight is treated as being at its minimum.
///
/// A backend reads back what it was set to, but through a float and, for DDC/CI,
/// through a monitor's own integer scale. Comparing against zero exactly would
/// make the seam depend on rounding.
const AT_MINIMUM: f32 = 1e-4;

/// A backlight and a gamma ramp, driven as one control.
pub(crate) struct Combined {
    hardware: Box<dyn Backend>,
    software: Box<dyn Backend>,
    /// The last level this was known to be at.
    ///
    /// Kept so that [`Backend::persists`] can answer without touching the
    /// display: it is asked every time a menu is drawn, and on a DDC/CI link a
    /// read costs the better part of a tenth of a second.
    level: Cell<f32>,
}

impl Combined {
    /// Composes a backlight with a ramp.
    ///
    /// # Errors
    ///
    /// Fails if the current level cannot be read, which means one of the two has
    /// already gone away.
    pub(crate) fn new(hardware: Box<dyn Backend>, software: Box<dyn Backend>) -> Result<Self> {
        let combined = Self {
            hardware,
            software,
            level: Cell::new(1.0),
        };
        let level = combined.read()?;
        combined.level.set(level.fraction());
        Ok(combined)
    }

    /// Reads both halves and works out what they add up to.
    fn read(&self) -> Result<Brightness> {
        let hardware = self.hardware.get()?.fraction();

        if hardware > AT_MINIMUM {
            // Above the seam, where the ramp is left at its brightest.
            return Ok(Brightness::new(
                SOFTWARE_SHARE + hardware * (1.0 - SOFTWARE_SHARE),
            ));
        }

        let software = self.software.get()?.fraction();
        Ok(Brightness::new(software * SOFTWARE_SHARE))
    }
}

impl Backend for Combined {
    fn name(&self) -> &'static str {
        // Both halves, because which one is doing the work depends on where the
        // level is and a reader deserves to know both are involved.
        "hardware+gamma"
    }

    fn persists(&self) -> bool {
        // Above the seam the whole setting is in the display, which keeps it.
        // Below it, part of the setting is in a ramp the system discards when
        // this process exits.
        self.level.get() >= SOFTWARE_SHARE
    }

    fn get(&self) -> Result<Brightness> {
        let level = self.read()?;
        self.level.set(level.fraction());
        Ok(level)
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let (hardware, software) = split(level);

        // The ramp first when going down and the backlight first when going up,
        // so that neither order passes through a state brighter than both the
        // one being left and the one being asked for.
        if level.fraction() < self.level.get() {
            self.software.set(software)?;
            self.hardware.set(hardware)?;
        } else {
            self.hardware.set(hardware)?;
            self.software.set(software)?;
        }

        self.level.set(level.fraction());
        Ok(())
    }
}

/// Splits a level into what the backlight does and what the ramp does.
fn split(level: Brightness) -> (Brightness, Brightness) {
    let level = level.fraction();

    if level >= SOFTWARE_SHARE {
        let hardware = (level - SOFTWARE_SHARE) / (1.0 - SOFTWARE_SHARE);
        (Brightness::new(hardware), Brightness::MAX)
    } else {
        (Brightness::MIN, Brightness::new(level / SOFTWARE_SHARE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell as StdCell;

    /// A backend that remembers what it was told, standing in for a display.
    struct Fake {
        level: StdCell<f32>,
    }

    impl Fake {
        fn new() -> Box<Self> {
            Box::new(Self {
                level: StdCell::new(1.0),
            })
        }
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn get(&self) -> Result<Brightness> {
            Ok(Brightness::new(self.level.get()))
        }
        fn set(&self, level: Brightness) -> Result<()> {
            self.level.set(level.fraction());
            Ok(())
        }
    }

    fn combined() -> Combined {
        Combined::new(Fake::new(), Fake::new()).expect("fakes always read")
    }

    #[test]
    fn the_top_of_the_range_is_all_backlight() {
        let (hardware, software) = split(Brightness::MAX);
        assert_eq!(hardware, Brightness::MAX);
        assert_eq!(software, Brightness::MAX, "the ramp is left alone");
    }

    #[test]
    fn the_bottom_of_the_range_is_all_ramp() {
        let (hardware, software) = split(Brightness::MIN);
        assert_eq!(hardware, Brightness::MIN);
        assert_eq!(software, Brightness::MIN);
    }

    #[test]
    fn the_seam_is_the_backlight_at_its_minimum_and_the_ramp_untouched() {
        // The one level both branches have to agree about.
        let (hardware, software) = split(Brightness::new(SOFTWARE_SHARE));
        assert_eq!(hardware, Brightness::MIN);
        assert_eq!(software, Brightness::MAX);
    }

    #[test]
    fn just_above_the_seam_the_ramp_is_still_untouched() {
        let (_, software) = split(Brightness::new(SOFTWARE_SHARE + 0.01));
        assert_eq!(
            software,
            Brightness::MAX,
            "a ramp left dim above the seam would cost contrast for nothing"
        );
    }

    #[test]
    fn every_level_survives_being_split_and_put_back_together() {
        let combined = combined();

        for whole in 0..=100u8 {
            let asked = Brightness::from_percent(f32::from(whole));
            combined.set(asked).expect("fakes always accept");

            assert_eq!(
                combined.get().expect("fakes always read").percent_rounded(),
                whole
            );
        }
    }

    #[test]
    fn the_range_below_the_seam_is_reachable_at_all() {
        // The point of the whole module: levels a backlight alone cannot express.
        let combined = combined();

        combined
            .set(Brightness::from_percent(10.0))
            .expect("accept");

        assert_eq!(
            combined.hardware.get().expect("read"),
            Brightness::MIN,
            "the backlight should be as low as it goes"
        );
        assert!(
            combined.software.get().expect("read") < Brightness::MAX,
            "and the ramp should be carrying the rest"
        );
    }

    #[test]
    fn a_level_above_the_seam_outlives_the_process_and_one_below_does_not() {
        let combined = combined();

        combined
            .set(Brightness::from_percent(80.0))
            .expect("accept");
        assert!(combined.persists(), "the backlight keeps this on its own");

        combined.set(Brightness::from_percent(5.0)).expect("accept");
        assert!(
            !combined.persists(),
            "part of this is in a ramp the system will discard"
        );
    }

    #[test]
    fn dimming_never_passes_through_something_brighter_than_either_end() {
        // Setting the backlight before the ramp on the way down would flash: the
        // ramp is still bright while the backlight has not yet dropped.
        let combined = combined();
        combined
            .set(Brightness::from_percent(100.0))
            .expect("accept");

        // Going down across the seam.
        combined.set(Brightness::from_percent(5.0)).expect("accept");
        assert_eq!(combined.get().expect("read").percent_rounded(), 5);

        // And back up across it.
        combined
            .set(Brightness::from_percent(90.0))
            .expect("accept");
        assert_eq!(combined.get().expect("read").percent_rounded(), 90);
    }
}

#[cfg(test)]
mod hardware {
    use super::*;
    use crate::controls;

    /// Ignored because it moves a real backlight and a real ramp.
    ///
    /// The unit tests above prove the arithmetic against fakes. This proves the
    /// thing the arithmetic is for: that a level below the seam really does put
    /// the backlight at its floor and carry the rest in the ramp, on hardware.
    ///
    /// It has to read back inside one process. macOS reverts a gamma ramp when
    /// the process that set it exits, so a second command would see the ramp
    /// already gone — which is not a failure, and is exactly what the warning on
    /// `klart set` is about.
    #[test]
    #[ignore = "moves a real backlight"]
    fn a_level_below_the_seam_reaches_past_the_backlight() {
        let attached = controls().expect("displays should open");
        let Some(control) = attached
            .iter()
            .find(|found| found.mechanism() == "hardware+gamma")
        else {
            panic!("no display on this machine pairs a backlight with a ramp");
        };

        let was = control.get().expect("a level should be readable");

        let above = Brightness::from_percent(60.0);
        control.set(above).expect("a level should be settable");
        let read_above = control.get().expect("a level should be readable");

        let below = Brightness::from_percent(10.0);
        control.set(below).expect("a level should be settable");
        let read_below = control.get().expect("a level should be readable");

        control.set(was).expect("a level should be settable");

        assert_eq!(read_above.percent_rounded(), 60);
        assert_eq!(
            read_below.percent_rounded(),
            10,
            "a level a backlight alone cannot express should still read back"
        );
        assert!(
            read_below < read_above,
            "and it should be the dimmer of the two"
        );
    }
}
