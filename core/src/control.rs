//! Choosing a mechanism for a display, and driving it.
//!
//! Which mechanism reaches a display, and driving it once one does.
//!
//! The order they are tried in belongs to [`crate::platform`], because it is not
//! the same list on every operating system. What is the same everywhere is the
//! policy behind it — hardware before software — and what this module adds: the
//! refusals collected along the way, and the stepping.

use crate::backend::Backend;
use crate::display::Display;
use crate::error::{Error, Result};
use crate::remembered::Remembered;
use crate::{Brightness, displays, platform};

/// A display together with the mechanism that reaches it.
pub struct Control {
    display: Display,
    /// The mechanism that answered, if any did.
    ///
    /// [`None`] is an ordinary state rather than a failure. A display that
    /// nothing reaches still exists, still has a name and a key, and still
    /// belongs in a listing — saying so is more use than leaving it out, and far
    /// more use than refusing to list anything because of it.
    backend: Option<Box<dyn Backend>>,
    refusals: Vec<Error>,
    /// What the person calls this display, when that is not what it calls
    /// itself.
    chosen_name: Option<String>,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control")
            .field("display", &self.display)
            .field("name", &self.name())
            .field("mechanism", &self.mechanism())
            .field("refusals", &self.refusals)
            .finish()
    }
}

impl Control {
    /// Opens a display with the best mechanism that will have it.
    ///
    /// Cannot fail. A display nothing reaches is a [`Control`] with no
    /// mechanism, carrying the refusals that explain why — which is the whole
    /// answer someone wants in that situation.
    #[must_use]
    pub fn open(display: Display) -> Self {
        let (backend, refusals) = match platform::open(&display) {
            Ok((backend, refusals)) => (Some(backend), refusals),
            Err(refusal) => (None, vec![refusal]),
        };

        Self {
            display,
            backend,
            refusals,
            chosen_name: None,
        }
    }

    /// What to call this display.
    ///
    /// The name it was given if it has one, and otherwise the name it publishes
    /// for itself. Two monitors of the same model publish the same name, which
    /// is the case this exists for.
    #[must_use]
    pub fn name(&self) -> &str {
        self.chosen_name
            .as_deref()
            .unwrap_or_else(|| self.display.name())
    }

    /// The display this drives.
    #[must_use]
    pub fn display(&self) -> &Display {
        &self.display
    }

    /// The name of the mechanism that answered, if one did.
    #[must_use]
    pub fn mechanism(&self) -> Option<&'static str> {
        self.backend.as_ref().map(|backend| backend.name())
    }

    /// Whether a change made here outlives the process that made it.
    ///
    /// False on the gamma ramp. Worth showing rather than hiding: it is the
    /// difference between a command that works and one that appears to.
    #[must_use]
    pub fn persists(&self) -> bool {
        // A display nothing reaches has nothing that could be lost, so there is
        // nothing to warn about.
        self.backend
            .as_ref()
            .is_none_or(|backend| backend.persists())
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
        self.reachable()?.get()
    }

    /// Sets the display's level.
    ///
    /// # Errors
    ///
    /// As the underlying [`Backend`].
    pub fn set(&self, level: Brightness) -> Result<()> {
        self.reachable()?.set(level)
    }

    /// The mechanism, or the reason there is none.
    fn reachable(&self) -> Result<&dyn Backend> {
        self.backend.as_deref().ok_or_else(|| Error::CannotReach {
            mechanism: "any",
            display: self.display.key().to_string(),
        })
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
/// Names chosen for a display are applied here rather than in [`displays`],
/// because a [`Display`] is what the hardware says it is and a [`Control`] is
/// how a person deals with it.
///
/// # Errors
///
/// As [`displays`].
pub fn controls() -> Result<Vec<Control>> {
    let chosen = Remembered::load();

    Ok(displays()?
        .into_iter()
        .map(|display| {
            let mut control = Control::open(display);
            control.chosen_name = chosen.name_for(control.display().key()).map(str::to_owned);
            control
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DisplayKind;

    /// Runs against whatever is attached; vacuous on a headless runner.
    #[test]
    fn a_display_nothing_reaches_is_still_listed() {
        // A headless runner has a connector with no backlight and no I2C bus.
        // Refusing to list anything because of it would be the wrong answer, and
        // was the answer until CI on Linux said so.
        for control in controls().expect("displays should list") {
            if control.mechanism().is_none() {
                assert!(
                    !control.refusals().is_empty(),
                    "{:?} has no mechanism and no reason",
                    control.display()
                );
            }
        }
    }

    #[test]
    fn the_built_in_panel_is_never_left_on_the_gamma_ramp() {
        // It has a real mechanism, so resolving it to the fallback would mean
        // the order is wrong or `DisplayServices` has stopped answering.
        for control in controls().expect("displays should list") {
            if control.display().kind() == DisplayKind::BuiltIn && control.mechanism().is_some() {
                assert!(
                    control.persists(),
                    "the built-in panel resolved to {:?}, refusals: {:?}",
                    control.mechanism(),
                    control.refusals()
                );
            }
        }
    }
}
