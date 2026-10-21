//! The software half: a gamma ramp, plus an overlay where the ramp runs out.
//!
//! Windows refuses a gamma ramp that moves any entry more than 32768 from the
//! identity curve. Scaling an ordinary ramp towards black reaches that limit at
//! about half brightness, so a ramp alone cannot cover the bottom of klart's
//! software range: writes below it are dropped and the picture stops changing
//! while the slider keeps moving.
//!
//! This composes the two software mechanisms instead. The ramp does everything
//! Windows will accept; below its last accepted curve, the same click-through
//! overlay used for HDR carries the rest. The overlay is only shown when it is
//! doing work, so an SDR display above the gamma floor is still dimmed by the
//! ramp alone, with no window over it.

use std::cell::Cell;

use crate::Brightness;
use crate::backend::Backend;
use crate::display::Display;
use crate::error::{Error, Result};

use super::gamma::{self, Gamma};
use super::overlay::{self, Overlay};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "gamma+overlay";

/// Two levels this close together are the same position.
const UNCHANGED: f32 = 1e-4;

/// A display dimmed by a gamma ramp, with an overlay underneath it.
pub(crate) struct Software {
    gamma: Gamma,
    /// [`None`] when the overlay could not be opened. Dimming above the ramp's
    /// floor still works; a level below it is reported rather than silently
    /// left at the last curve Windows accepted.
    overlay: Option<Overlay>,
    gamma_level: Cell<f32>,
    overlay_level: Cell<f32>,
    /// The last level this was known to be at, used to choose which half moves
    /// first so a drag never passes through a brighter or darker state.
    level: Cell<f32>,
}

/// What each half should be asked for to reach one overall level.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    gamma: Brightness,
    overlay: Brightness,
}

impl Software {
    /// Binds to a display, with its gamma ramp already opened.
    ///
    /// Returns the overlay's refusal, if it could not be opened, so the caller
    /// can still report it in `klart list` without losing the gamma path.
    ///
    /// # Errors
    ///
    /// When the gamma ramp cannot be read, which means the adapter has gone.
    pub(crate) fn open(display: &Display, gamma: Gamma) -> Result<(Self, Option<Error>)> {
        let gamma_level = gamma.get()?;
        let (overlay, refusal) = match Overlay::open(display) {
            Ok(overlay) => (Some(overlay), None),
            Err(problem) => (None, Some(problem)),
        };
        let overlay_level = match &overlay {
            Some(overlay) => overlay.get()?,
            None => Brightness::MAX,
        };

        // The two halves multiply: the ramp leaves a fraction of the picture and
        // the overlay leaves a fraction of what the ramp passed through.
        let visible = gamma::to_factor(gamma_level) * overlay::visible_for(overlay_level);
        let level = Brightness::new(gamma::level_from_factor(visible));

        Ok((
            Self {
                gamma,
                overlay,
                gamma_level: Cell::new(gamma_level.fraction()),
                overlay_level: Cell::new(overlay_level.fraction()),
                level: Cell::new(level.fraction()),
            },
            refusal,
        ))
    }

    /// What each half should do for one overall level.
    fn place(&self, level: Brightness) -> Placement {
        place(level, self.gamma.lowest())
    }

    /// Writes the ramp, unless it is already there.
    fn write_gamma(&self, target: Brightness) -> Result<()> {
        if unchanged(self.gamma_level.get(), target) {
            return Ok(());
        }

        self.gamma.set(target)?;
        self.gamma_level.set(target.fraction());
        Ok(())
    }

    /// Writes the overlay, unless it is already there.
    fn write_overlay(&self, target: Brightness) -> Result<()> {
        if unchanged(self.overlay_level.get(), target) {
            return Ok(());
        }

        let Some(overlay) = &self.overlay else {
            if target == Brightness::MAX {
                return Ok(());
            }
            return Err(Error::MechanismUnavailable {
                mechanism: overlay::NAME,
            });
        };

        overlay.set(target)?;
        self.overlay_level.set(target.fraction());
        Ok(())
    }
}

impl Backend for Software {
    fn name(&self) -> &'static str {
        if self.overlay.is_some() {
            NAME
        } else {
            gamma::NAME
        }
    }

    fn persists(&self) -> bool {
        // The overlay is a window and goes away with this process; the ramp
        // stays. Once the overlay is doing any of the work, the level as a
        // whole is no longer held by something that outlives us.
        self.overlay_level.get() >= 1.0 - UNCHANGED
    }

    fn get(&self) -> Result<Brightness> {
        let gamma_level = self.gamma.get()?;
        let overlay_level = match &self.overlay {
            Some(overlay) => overlay.get()?,
            None => Brightness::MAX,
        };

        self.gamma_level.set(gamma_level.fraction());
        self.overlay_level.set(overlay_level.fraction());

        let visible = gamma::to_factor(gamma_level) * overlay::visible_for(overlay_level);
        let level = Brightness::new(gamma::level_from_factor(visible));
        self.level.set(level.fraction());
        Ok(level)
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let placement = self.place(level);

        if placement.overlay != Brightness::MAX && self.overlay.is_none() {
            return Err(Error::MechanismUnavailable {
                mechanism: overlay::NAME,
            });
        }

        // Going down, move the ramp first and then the overlay; going up, the
        // other way round. Either order keeps the intermediate state between
        // where the display was and where it is going, so a drag never flashes
        // brighter than both ends.
        if level.fraction() < self.level.get() {
            self.write_gamma(placement.gamma)?;
            self.write_overlay(placement.overlay)?;
        } else {
            self.write_overlay(placement.overlay)?;
            self.write_gamma(placement.gamma)?;
        }

        self.level.set(level.fraction());
        Ok(())
    }

    fn release(&self) -> Result<()> {
        // Both are attempted even if one fails: a gamma ramp left scaled is the
        // more dangerous of the two, and an overlay that would not hide should
        // not stop the ramp being restored.
        let overlay = match &self.overlay {
            Some(overlay) => overlay.release(),
            None => Ok(()),
        };
        if overlay.is_ok() {
            self.overlay_level.set(1.0);
        }

        let gamma = self.gamma.release();
        if gamma.is_ok() {
            self.gamma_level.set(1.0);
            self.level.set(1.0);
        }

        overlay?;
        gamma?;
        Ok(())
    }
}

/// Divides one software level between the ramp and the overlay.
///
/// The ramp covers everything down to its floor. Below that it stays at the
/// floor and the overlay makes up the difference, so the product of the two
/// fractions is exactly what the gamma ramp alone used to mean by a level.
fn place(level: Brightness, gamma_floor: Brightness) -> Placement {
    let visible = gamma::to_factor(level);
    let floor = gamma::to_factor(gamma_floor);

    if visible >= floor {
        Placement {
            gamma: level,
            overlay: Brightness::MAX,
        }
    } else {
        Placement {
            gamma: gamma_floor,
            overlay: overlay::level_for_visible(visible / floor),
        }
    }
}

/// Whether a cached level is the same one that would be written now.
fn unchanged(cached: f32, target: Brightness) -> bool {
    (cached - target.fraction()).abs() <= UNCHANGED
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the module: every level in the bottom quarter moves the
    /// picture, including the ones Windows used to reject.
    #[test]
    fn every_level_below_the_seam_has_a_ramp_windows_accepts_and_a_visible_step() {
        let baseline = gamma::identity_ramp();
        let floor_factor = gamma::lowest_factor(&baseline);
        let gamma_floor = Brightness::new(gamma::level_from_factor(floor_factor));

        let mut previous = None;

        for whole in 0..=25u8 {
            // The bottom quarter of a combined slider, in software terms.
            let software = Brightness::new(f32::from(whole) / 25.0);
            let placement = place(software, gamma_floor);
            let ramp = gamma::scale(&baseline, gamma::to_factor(placement.gamma));

            assert!(
                gamma::within_heuristic(&ramp),
                "{whole}% asked for a ramp Windows would drop"
            );

            let visible =
                gamma::to_factor(placement.gamma) * overlay::visible_for(placement.overlay);
            let wanted = gamma::to_factor(software);
            assert!(
                (visible - wanted).abs() < 1e-3,
                "{whole}% was asked for {wanted} and composed to {visible}"
            );

            if let Some(previous) = previous {
                assert!(
                    visible > previous,
                    "{whole}% did not make the picture brighter than the step below"
                );
            }
            previous = Some(visible);
        }
    }

    #[test]
    fn the_levels_that_used_to_be_dropped_now_use_the_overlay() {
        let baseline = gamma::identity_ramp();
        let gamma_floor =
            Brightness::new(gamma::level_from_factor(gamma::lowest_factor(&baseline)));

        for whole in [0_u8, 1, 5, 8, 9] {
            let software = Brightness::new(f32::from(whole) / 25.0);
            let placement = place(software, gamma_floor);

            assert_eq!(
                placement.gamma, gamma_floor,
                "{whole}% should leave the ramp at its floor"
            );
            assert!(
                placement.overlay < Brightness::MAX,
                "{whole}% should show the overlay"
            );
            assert!(
                overlay::visible_for(placement.overlay) >= 0.2,
                "the overlay must not go below its own recovery floor"
            );
        }

        // At 10% the original ramp still works, so no window should be over the
        // display at all.
        let ten = place(Brightness::new(10.0 / 25.0), gamma_floor);
        assert_eq!(ten.overlay, Brightness::MAX);
    }

    #[test]
    fn above_the_gamma_floor_only_the_ramp_is_used() {
        let baseline = gamma::identity_ramp();
        let gamma_floor =
            Brightness::new(gamma::level_from_factor(gamma::lowest_factor(&baseline)));

        for whole in 40..=100u8 {
            let placement = place(Brightness::from_percent(f32::from(whole)), gamma_floor);
            assert_eq!(
                placement.overlay,
                Brightness::MAX,
                "{whole}% should not put an overlay over the display"
            );
        }
    }
}
