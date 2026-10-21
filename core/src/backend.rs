//! The interface every mechanism that can change a display's brightness meets.
//!
//! One trait, and nothing platform-specific. An implementation is bound to a
//! single display when it is opened, because whether a mechanism can reach a
//! display is a question with a different answer per display, and answering it
//! once is cheaper than answering it per call.
//!
//! The implementations live in [`crate::platform`], except DDC/CI, whose
//! protocol is the same everywhere and so lives in [`crate::ddc`].

use crate::Brightness;
use crate::error::Result;

/// One way of reaching one display's brightness.
pub trait Backend {
    /// The mechanism's name, for diagnostics and for machine-readable output.
    fn name(&self) -> &'static str;

    /// Whether a change made through this mechanism outlives the process.
    ///
    /// True for anything that moves a backlight, because the setting lives in
    /// the display, and for a Windows gamma ramp, which Windows leaves in place.
    /// False for the macOS gamma ramp, which the window server reverts when the
    /// process that set it exits, and for an HDR overlay, which is a window and
    /// goes away with its process.
    fn persists(&self) -> bool {
        true
    }

    /// Reads the display's current level.
    ///
    /// # Errors
    ///
    /// Fails if the display has gone away since this backend was opened, or if
    /// the mechanism refuses the read for a reason only its own status code
    /// explains.
    fn get(&self) -> Result<Brightness>;

    /// Sets the display's level.
    ///
    /// Clamping has already happened: a [`Brightness`] cannot be out of range.
    ///
    /// # Errors
    ///
    /// As [`Backend::get`].
    fn set(&self, level: Brightness) -> Result<()>;

    /// Where a hardware floor meets a software fallback, as a percentage.
    ///
    /// Most mechanisms drive one thing and have no seam. A composed control
    /// does: below this point the picture is being dimmed rather than the
    /// backlight, and a person dragging it deserves to see where that starts.
    fn seam(&self) -> Option<u8> {
        None
    }

    /// Gives back anything this mechanism is holding for the process.
    ///
    /// Called when the agent is about to exit. A backlight has nothing to give
    /// back; a software overlay or a gamma ramp does, and leaving one in place
    /// would hold a display at a level nobody is left to change.
    ///
    /// # Errors
    ///
    /// As [`Backend::set`]. A release that fails is still worth attempting on
    /// the way out, so callers report it and carry on.
    fn release(&self) -> Result<()> {
        Ok(())
    }
}
