//! Choosing a mechanism for a display, and driving it.
//!
//! Three mechanisms, in a fixed order of preference: the built-in panel's
//! framework, then DDC/CI, then the gamma ramp. The order is by how real the
//! result is. The first two move a backlight; the third only darkens the
//! picture, so it is what is left when nothing better answers rather than a
//! peer of the other two.

use crate::backend::{Backend, BuiltIn, Ddc, Gamma};
use crate::display::Display;
use crate::error::{Error, Result};
use crate::{Brightness, displays};

/// A display together with the mechanism that reaches it.
pub struct Control {
    display: Display,
    backend: Box<dyn Backend>,
    refusals: Vec<Error>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control")
            .field("display", &self.display)
            .field("mechanism", &self.backend.name())
            .field("refusals", &self.refusals)
            .finish()
    }
}

impl Control {
    /// Opens a display with the best mechanism that will have it.
    ///
    /// # Errors
    ///
    /// Only if all three refuse, which in practice means the display went away
    /// between being listed and being opened — the gamma ramp is available on
    /// any display that exists, which is why it is last.
    pub fn open(display: Display) -> Result<Self> {
        let mut refusals = Vec::new();

        match BuiltIn::open(&display) {
            Ok(panel) => {
                return Ok(Self {
                    display,
                    backend: Box::new(panel),
                    refusals,
                });
            }
            Err(refusal) => refusals.push(refusal),
        }

        match Ddc::open(&display) {
            Ok(monitor) => {
                return Ok(Self {
                    display,
                    backend: Box::new(monitor),
                    refusals,
                });
            }
            Err(refusal) => refusals.push(refusal),
        }

        Ok(Self {
            backend: Box::new(Gamma::open(&display)?),
            display,
            refusals,
        })
    }

    /// The display this drives.
    #[must_use]
    pub fn display(&self) -> &Display {
        &self.display
    }

    /// The name of the mechanism that answered.
    #[must_use]
    pub fn mechanism(&self) -> &'static str {
        self.backend.name()
    }

    /// Whether a change made here outlives the process that made it.
    ///
    /// False on the gamma ramp. Worth showing rather than hiding: it is the
    /// difference between a command that works and one that appears to.
    #[must_use]
    pub fn persists(&self) -> bool {
        self.backend.persists()
    }

    /// Why the mechanisms ahead of this one declined, in the order they were
    /// tried.
    ///
    /// The answer to "why is this monitor on the gamma ramp", which is the
    /// question this tool will be asked most.
    #[must_use]
    pub fn refusals(&self) -> &[Error] {
        &self.refusals
    }

    /// Reads the display's current level.
    ///
    /// # Errors
    ///
    /// As the underlying [`Backend`].
    pub fn get(&self) -> Result<Brightness> {
        self.backend.get()
    }

    /// Sets the display's level.
    ///
    /// # Errors
    ///
    /// As the underlying [`Backend`].
    pub fn set(&self, level: Brightness) -> Result<()> {
        self.backend.set(level)
    }

    /// Moves the display's level by `delta`, saturating at both ends, and
    /// reports where it landed.
    ///
    /// # Errors
    ///
    /// As the underlying [`Backend`]. A failure to read leaves the level alone.
    pub fn adjust(&self, delta: f32) -> Result<Brightness> {
        let moved = self.get()?.stepped(delta);
        self.set(moved)?;
        Ok(moved)
    }
}

/// Every attached display, each with the mechanism that reaches it.
///
/// # Errors
///
/// As [`displays`].
pub fn controls() -> Result<Vec<Control>> {
    displays()?.into_iter().map(Control::open).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DisplayKind;

    /// Runs against whatever is attached; vacuous on a headless runner.
    #[test]
    fn every_display_gets_a_mechanism() {
        for control in controls().expect("displays should open") {
            assert!(
                !control.mechanism().is_empty(),
                "{:?} opened with no mechanism",
                control.display()
            );
        }
    }

    #[test]
    fn the_built_in_panel_is_never_left_on_the_gamma_ramp() {
        // It has a real mechanism, so resolving it to the fallback would mean
        // the order is wrong or `DisplayServices` has stopped answering.
        for control in controls().expect("displays should open") {
            if control.display().kind() == DisplayKind::BuiltIn {
                assert!(
                    control.persists(),
                    "the built-in panel resolved to {}, refusals: {:?}",
                    control.mechanism(),
                    control.refusals()
                );
            }
        }
    }
}
