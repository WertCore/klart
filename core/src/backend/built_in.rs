//! The built-in panel, through `DisplayServices`.

use crate::Brightness;
use crate::backend::Backend;
use crate::display::Display;
use crate::error::{Error, Result};
use crate::sys::display_services::{self, NAME};

/// The built-in panel's brightness, through the `DisplayServices` framework.
///
/// Bound to one display. Opening it is the only place that asks whether the
/// framework can drive that display at all, so a `BuiltIn` that exists is one
/// whose `get` and `set` are expected to work.
#[derive(Debug, Clone, Copy)]
pub struct BuiltIn {
    display: u32,
}

impl BuiltIn {
    /// Binds to a display, if `DisplayServices` will drive it.
    ///
    /// Not restricted to displays that report as built-in: the framework is the
    /// authority on what it can reach, and a check against
    /// [`crate::DisplayKind`] would only add a second, less reliable opinion.
    ///
    /// # Errors
    ///
    /// Fails if the framework is absent or has lost a symbol, and if the
    /// framework says it cannot change this display's brightness — which is the
    /// ordinary answer for an external monitor, and how the caller knows to try
    /// something else.
    pub fn open(display: &Display) -> Result<Self> {
        let framework = display_services::display_services()
            .ok_or(Error::MechanismUnavailable { mechanism: NAME })?;

        if !framework.can_change(display.id()) {
            return Err(Error::CannotReach {
                mechanism: NAME,
                display: display.key().to_string(),
            });
        }

        Ok(Self {
            display: display.id(),
        })
    }
}

impl Backend for BuiltIn {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        let framework = display_services::display_services()
            .ok_or(Error::MechanismUnavailable { mechanism: NAME })?;

        framework
            .brightness(self.display)
            .map(Brightness::new)
            .map_err(|code| Error::MechanismFailed {
                mechanism: NAME,
                call: "DisplayServicesGetBrightness",
                code,
            })
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let framework = display_services::display_services()
            .ok_or(Error::MechanismUnavailable { mechanism: NAME })?;

        framework
            .set_brightness(self.display, level.fraction())
            .map_err(|code| Error::MechanismFailed {
                mechanism: NAME,
                call: "DisplayServicesSetBrightness",
                code,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::DisplayKind;
    use crate::displays;

    /// Whatever this mechanism drives, it is the panel in the lid.
    ///
    /// Holds vacuously on a headless runner, which has no displays at all. If it
    /// ever fails on some other Mac that would be worth knowing: it would mean
    /// `DisplayServices` reaches further than assumed and the resolution order
    /// in entry 5 should lean on it harder.
    #[test]
    fn only_the_built_in_panel_answers_this_mechanism() {
        let attached = displays().expect("Core Graphics should enumerate");
        let driven: Vec<_> = attached
            .into_iter()
            .filter(|display| BuiltIn::open(display).is_ok())
            .collect();

        assert!(
            driven.len() <= 1,
            "{} displays answered DisplayServices: {driven:#?}",
            driven.len()
        );
        for display in driven {
            assert_eq!(
                display.kind(),
                DisplayKind::BuiltIn,
                "an external display answered DisplayServices: {display:#?}"
            );
        }
    }

    /// Ignored because it moves a real backlight, and because a headless runner
    /// has none to move. Run it with
    /// `cargo test --package klart-core -- --ignored --nocapture`.
    #[test]
    #[ignore = "moves the real backlight"]
    fn it_sets_the_panel_and_puts_it_back() {
        let attached = displays().expect("Core Graphics should enumerate");
        let Some(panel) = attached
            .iter()
            .find_map(|display| BuiltIn::open(display).ok())
        else {
            panic!("no display on this machine answers DisplayServices");
        };

        let original = panel.get().expect("the panel should report a level");

        // Away from wherever it already is, so that a `set` which silently does
        // nothing cannot pass by coincidence.
        let target = if original.percent() > 50.0 {
            Brightness::from_percent(30.0)
        } else {
            Brightness::from_percent(70.0)
        };

        panel.set(target).expect("the panel should accept a level");
        std::thread::sleep(std::time::Duration::from_millis(150));
        let observed = panel.get().expect("the panel should report a level");

        // Restored before any assertion, so that a failure does not leave the
        // machine sitting at 30%.
        panel
            .set(original)
            .expect("the panel should accept a level");

        assert!(
            (observed.percent() - target.percent()).abs() < 2.0,
            "asked for {target} and read back {observed}"
        );
    }
}
