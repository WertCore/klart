//! Software dimming, through the display's gamma ramp.
//!
//! `SetDeviceGammaRamp` on a device context for the adapter. The same trade as
//! everywhere else: it darkens the picture rather than the backlight, so
//! contrast goes with it, and it is what is left when nothing better answers.
//!
//! Unlike macOS, Windows does not put the ramp back when the process that set it
//! exits — it stays until something else changes it or the session ends. That is
//! the better behaviour for a command line and the reason this platform's
//! [`Backend::persists`] is true where macOS's is false.

use windows_sys::Win32::Graphics::Gdi::{CreateDCW, DeleteDC};
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

use super::monitors::to_wide;

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "gamma";

/// The dimmest ramp this will write.
///
/// The same floor, and for the same reason, as every other platform: a ramp of
/// zero is a black screen, and a black screen cannot be undone from a menu
/// nobody can see.
const FLOOR: f32 = 0.2;

/// How many entries a Windows gamma ramp has, per channel.
const ENTRIES: usize = 256;

/// A display's brightness, faked with its gamma ramp.
pub(crate) struct Gamma {
    adapter: String,
}

impl Gamma {
    /// Binds to an adapter.
    ///
    /// # Errors
    ///
    /// When a device context cannot be made for it, which means the adapter has
    /// gone away.
    pub(crate) fn open(adapter: &str) -> Result<Self> {
        let opened = Self {
            adapter: adapter.to_owned(),
        };
        opened.with_context(|_| Ok(()))?;
        Ok(opened)
    }

    /// Runs something against a device context for this adapter.
    ///
    /// The context is created and destroyed around each use rather than held.
    /// A device context is a scarce, session-scoped resource and this is an
    /// agent that lives for the whole login; keeping one open for hours to save
    /// a microsecond would be the wrong trade.
    fn with_context<T>(&self, body: impl FnOnce(isize) -> Result<T>) -> Result<T> {
        let name = to_wide(&self.adapter);

        // SAFETY: `name` is NUL terminated and live across the call. The three
        // nulls are the documented way to ask for a context on the whole
        // adapter rather than a particular device or mode.
        let context = unsafe {
            CreateDCW(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };

        if context.is_null() {
            return Err(Error::CannotReach {
                mechanism: NAME,
                display: self.adapter.clone(),
            });
        }

        let outcome = body(context as isize);

        // SAFETY: the context came from `CreateDCW` and is deleted exactly once.
        unsafe { DeleteDC(context) };
        outcome
    }
}

impl Backend for Gamma {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        self.with_context(|context| {
            let mut ramp = [0_u16; ENTRIES * 3];

            // SAFETY: the buffer is the 3 × 256 × `u16` this call documents.
            let ok = unsafe { GetDeviceGammaRamp(context as *mut _, ramp.as_mut_ptr().cast()) };
            if ok == 0 {
                return Err(Error::MechanismFailed {
                    mechanism: NAME,
                    call: "GetDeviceGammaRamp",
                    code: -1,
                });
            }

            // The last entry of the red channel is the ramp's ceiling, which is
            // what `set` scales.
            Ok(from_ramp(
                f32::from(ramp[ENTRIES - 1]) / f32::from(u16::MAX),
            ))
        })
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let ceiling = to_ramp(level);

        self.with_context(|context| {
            let mut ramp = [0_u16; ENTRIES * 3];
            for entry in 0..ENTRIES {
                // A linear ramp to the ceiling. The same shape macOS asks for
                // with a maximum and a gamma of one, written out because this
                // API takes the table rather than the formula.
                let value = (entry as f32 / (ENTRIES - 1) as f32) * ceiling * f32::from(u16::MAX);
                let value = value.round().clamp(0.0, f32::from(u16::MAX)) as u16;

                ramp[entry] = value;
                ramp[ENTRIES + entry] = value;
                ramp[ENTRIES * 2 + entry] = value;
            }

            // SAFETY: the buffer is the 3 × 256 × `u16` this call documents.
            let ok = unsafe { SetDeviceGammaRamp(context as *mut _, ramp.as_ptr().cast()) };
            if ok == 0 {
                return Err(Error::MechanismFailed {
                    mechanism: NAME,
                    call: "SetDeviceGammaRamp",
                    code: -1,
                });
            }
            Ok(())
        })
    }
}

/// The ramp ceiling that shows a given level.
fn to_ramp(level: Brightness) -> f32 {
    FLOOR + (1.0 - FLOOR) * level.fraction()
}

/// The level a given ceiling is showing.
fn from_ramp(ceiling: f32) -> Brightness {
    Brightness::new((ceiling - FLOOR) / (1.0 - FLOOR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip_through_the_ramp() {
        for whole in 0..=100u8 {
            let level = Brightness::from_percent(f32::from(whole));
            assert_eq!(from_ramp(to_ramp(level)).percent_rounded(), whole);
        }
    }

    #[test]
    fn the_dimmest_level_still_leaves_something_on_the_screen() {
        assert_eq!(to_ramp(Brightness::MIN), FLOOR);
        assert!(to_ramp(Brightness::MIN) > 0.0);
    }

    #[test]
    fn the_brightest_level_is_the_ramp_untouched() {
        assert_eq!(to_ramp(Brightness::MAX), 1.0);
    }
}
