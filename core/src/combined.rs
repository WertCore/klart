//! Dimming past the point where the backlight stops.
//!
//! A backlight has a minimum, and on a laptop panel in a dark room that minimum
//! is still too bright. Below it the only thing left is a software half, which
//! darkens the picture rather than the light behind it — a poor way to set
//! brightness and the only way to go lower.
//!
//! So the two are composed rather than chosen between. The bottom
//! [`SOFTWARE_SHARE`] of the range holds the backlight at its minimum and dims
//! with the software half — a gamma ramp in SDR, an overlay in HDR; everything
//! above it drives the backlight and leaves that half alone. The seam is
//! continuous: at exactly the share, the backlight is at its minimum and the
//! software half is untouched, which is the same display state either branch
//! would produce.
//!
//! Nothing here is platform-specific. It composes two [`Backend`]s and does not
//! care which.

use std::cell::Cell;

use crate::Brightness;
use crate::backend::Backend;
use crate::error::Result;

/// How much of the range is given to the software half.
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

/// A backlight and a software half, driven as one control.
pub(crate) struct Combined {
    hardware: Box<dyn Backend>,
    software: Box<dyn Backend>,
    /// The last level this was known to be at.
    ///
    /// Kept so that [`Backend::persists`] can answer without touching the
    /// display: it is asked every time a menu is drawn, and on a DDC/CI link a
    /// read costs the better part of a tenth of a second.
    level: Cell<f32>,
    /// What each half was last asked for, or [`None`] when it has not been
    /// read or written yet.
    ///
    /// A drag below the seam holds the backlight at its floor for every step,
    /// and writing that floor again is a DDC/CI round trip for no change. This
    /// is what lets the expensive half be left alone while the ramp or overlay
    /// does the moving.
    hardware_level: Cell<Option<f32>>,
    software_level: Cell<Option<f32>>,
}

impl Combined {
    /// Composes a backlight with a software half from levels already read.
    ///
    /// Taking the readings rather than making them lets the caller fall back to
    /// whichever half did answer when the other one has gone away.
    pub(crate) fn from_levels(
        hardware: Box<dyn Backend>,
        software: Box<dyn Backend>,
        hardware_level: Brightness,
        software_level: Brightness,
    ) -> Self {
        let hardware_level = hardware_level.fraction();
        let software_level = software_level.fraction();

        let level = if hardware_level > AT_MINIMUM {
            SOFTWARE_SHARE + hardware_level * (1.0 - SOFTWARE_SHARE)
        } else {
            software_level * SOFTWARE_SHARE
        };

        Self {
            hardware,
            software,
            level: Cell::new(level),
            hardware_level: Cell::new(Some(hardware_level)),
            software_level: Cell::new(Some(software_level)),
        }
    }

    /// Composes a backlight with a software half.
    ///
    /// # Errors
    ///
    /// Fails if the current level cannot be read, which means one of the two has
    /// already gone away.
    #[cfg(test)]
    pub(crate) fn new(hardware: Box<dyn Backend>, software: Box<dyn Backend>) -> Result<Self> {
        let hardware_level = hardware.get()?;
        let software_level = software.get()?;
        Ok(Self::from_levels(
            hardware,
            software,
            hardware_level,
            software_level,
        ))
    }

    /// Reads the backlight, and the software half when it is doing any of the
    /// work.
    fn read(&self) -> Result<Brightness> {
        let hardware = self.hardware.get()?.fraction();
        self.hardware_level.set(Some(hardware));

        if hardware > AT_MINIMUM {
            // Above the seam, where the software half is left at its brightest.
            return Ok(Brightness::new(
                SOFTWARE_SHARE + hardware * (1.0 - SOFTWARE_SHARE),
            ));
        }

        let software = self.software.get()?.fraction();
        self.software_level.set(Some(software));
        Ok(Brightness::new(software * SOFTWARE_SHARE))
    }
}

impl Backend for Combined {
    fn name(&self) -> &'static str {
        // Both halves, because which one is doing the work depends on where the
        // level is and a reader deserves to know both are involved. In SDR the
        // software half is a gamma ramp with an overlay under its floor; in HDR
        // it is an overlay alone. Those are not the same promise about
        // persistence.
        match self.software.name() {
            "overlay" => "hardware+overlay",
            "gamma+overlay" => "hardware+gamma+overlay",
            _ => "hardware+gamma",
        }
    }

    fn seam(&self) -> Option<u8> {
        Some((SOFTWARE_SHARE * 100.0).round() as u8)
    }

    fn persists(&self) -> bool {
        // Above the seam the whole setting is in the display, which keeps it.
        // Below it, part of the setting is in the software half. A Windows gamma
        // ramp outlives the process just as the backlight does; an overlay and
        // the macOS ramp do not, and the tray gives those back when it quits.
        // The half knows which it is, so it answers rather than this guessing
        // from the level alone.
        if self.level.get() >= SOFTWARE_SHARE {
            true
        } else {
            self.software.persists()
        }
    }

    fn release(&self) -> Result<()> {
        self.software.release()?;
        self.software_level.set(Some(1.0));
        // The backlight is left where it is. At the seam that is its floor, so
        // releasing the software half makes the display as bright as the
        // hardware alone can be — which is a screen someone can still see.
        self.level.set(self.level.get().max(SOFTWARE_SHARE));
        Ok(())
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
            self.write_software(software)?;
            self.write_hardware(hardware)?;
        } else {
            self.write_hardware(hardware)?;
            self.write_software(software)?;
        }

        self.level.set(level.fraction());
        Ok(())
    }
}

impl Combined {
    /// Writes the backlight, unless it is already there.
    fn write_hardware(&self, target: Brightness) -> Result<()> {
        if unchanged(self.hardware_level.get(), target) {
            return Ok(());
        }

        self.hardware.set(target)?;
        self.hardware_level.set(Some(target.fraction()));
        Ok(())
    }

    /// Writes the ramp or overlay, unless it is already there.
    fn write_software(&self, target: Brightness) -> Result<()> {
        if unchanged(self.software_level.get(), target) {
            return Ok(());
        }

        self.software.set(target)?;
        self.software_level.set(Some(target.fraction()));
        Ok(())
    }
}

/// Whether a cached level is the same one that would be written now.
///
/// Two levels separated only by float noise are the same position; at the
/// bottom of the range a monitor's own integer scale makes that noise common.
fn unchanged(cached: Option<f32>, target: Brightness) -> bool {
    cached.is_some_and(|cached| (cached - target.fraction()).abs() <= 1e-4)
}

/// Splits a level into what the backlight does and what the software half does.
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
    use std::rc::Rc;

    /// A backend that remembers what it was told, standing in for a display.
    struct Fake {
        level: StdCell<f32>,
        writes: Rc<StdCell<u32>>,
        persists: bool,
    }

    impl Fake {
        fn new() -> Box<Self> {
            Box::new(Self {
                level: StdCell::new(1.0),
                writes: Rc::new(StdCell::new(0)),
                persists: false,
            })
        }

        /// A software half that outlives the process, as Windows' ramp does.
        fn persistent() -> Box<Self> {
            Box::new(Self {
                level: StdCell::new(1.0),
                writes: Rc::new(StdCell::new(0)),
                persists: true,
            })
        }

        /// A fake whose writer can be watched without holding the box.
        fn counted() -> (Box<Self>, Rc<StdCell<u32>>) {
            let writes = Rc::new(StdCell::new(0));
            let fake = Box::new(Self {
                level: StdCell::new(1.0),
                writes: Rc::clone(&writes),
                persists: false,
            });
            (fake, writes)
        }
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn persists(&self) -> bool {
            self.persists
        }
        fn get(&self) -> Result<Brightness> {
            Ok(Brightness::new(self.level.get()))
        }
        fn set(&self, level: Brightness) -> Result<()> {
            self.level.set(level.fraction());
            self.writes.set(self.writes.get() + 1);
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
    fn a_software_half_that_outlives_the_process_keeps_the_low_level() {
        let combined = Combined::new(Fake::new(), Fake::persistent()).expect("fakes always read");

        combined.set(Brightness::from_percent(5.0)).expect("accept");
        assert!(
            combined.persists(),
            "a Windows ramp stays where it was written"
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

    #[test]
    fn a_drag_below_the_seam_does_not_rewrite_the_backlight() {
        let (hardware, hardware_writes) = Fake::counted();
        let (software, software_writes) = Fake::counted();
        let combined = Combined::new(hardware, software).expect("fakes always read");

        // First move sets both halves: the backlight goes to its floor and the
        // ramp takes the rest.
        combined
            .set(Brightness::from_percent(20.0))
            .expect("fakes always accept");
        let hardware_after_seam = hardware_writes.get();
        let software_after_seam = software_writes.get();

        for whole in (0..=19u8).rev() {
            combined
                .set(Brightness::from_percent(f32::from(whole)))
                .expect("fakes always accept");
        }

        assert_eq!(
            hardware_writes.get(),
            hardware_after_seam,
            "the floor was written again for every step of the drag"
        );
        assert!(
            software_writes.get() > software_after_seam,
            "the ramp should be doing the moving"
        );
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
    /// the process that set it exits and Windows leaves it in place, so the
    /// second command sees a different state on each; neither is a failure, and
    /// both are what the warning on `klart set` is about.
    #[test]
    #[ignore = "moves a real backlight"]
    fn every_level_below_the_seam_reaches_past_the_backlight() {
        let attached = controls().expect("displays should open");
        let Some(control) = attached.iter().find(|found| {
            matches!(
                found.mechanism(),
                Some("hardware+gamma" | "hardware+gamma+overlay" | "hardware+overlay")
            )
        }) else {
            panic!("no display on this machine pairs a backlight with a software fallback");
        };

        let was = control.get().expect("a level should be readable");

        let above = Brightness::from_percent(60.0);
        control.set(above).expect("a level should be settable");
        let read_above = control.get().expect("a level should be readable");

        // The whole software range, not only the top of it. Levels below the
        // gamma ramp's Windows-imposed floor are the ones this exists for: they
        // used to fail their write and leave the previous ramp on screen.
        for whole in [25, 20, 15, 12, 11, 10, 9, 8, 5, 1, 0] {
            let asked = Brightness::from_percent(f32::from(whole));
            control.set(asked).expect("a level should be settable");
            let read = control.get().expect("a level should be readable");

            assert_eq!(
                read.percent_rounded(),
                whole,
                "a level a backlight alone cannot express should still read back"
            );
            assert!(
                read < read_above,
                "{whole}% should be dimmer than the level above the seam"
            );
        }

        control.set(was).expect("a level should be settable");
    }
}
