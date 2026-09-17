//! The mechanisms that can actually change a display's brightness.
//!
//! Each one is bound to a single display when it is opened, because whether a
//! mechanism can reach a display is a question with a different answer per
//! display and answering it once is cheaper than answering it per call.

use crate::Brightness;
use crate::error::Result;

mod built_in;

pub use built_in::BuiltIn;

/// One way of reaching one display's brightness.
pub trait Backend {
    /// The mechanism's name, for diagnostics and for machine-readable output.
    fn name(&self) -> &'static str;

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
}
