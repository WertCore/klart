//! An external monitor's backlight, through the Monitor Configuration API.
//!
//! `GetMonitorBrightness` and `SetMonitorBrightness` in `dxva2`, which are
//! DDC/CI underneath — the same VCP feature `0x10` this crate speaks by hand on
//! macOS and Linux. Here the driver does the framing, the checksums and the
//! retries, so [`crate::ddc`] is not used on this platform at all.
//!
//! That is the one genuinely pleasant surface of the three. It is documented,
//! supported, and it reports the monitor's own minimum and maximum rather than
//! making the caller discover them.

use windows_sys::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows_sys::Win32::Graphics::Gdi::HMONITOR;

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "DDC/CI";

/// A monitor that answers the Monitor Configuration API.
pub(crate) struct Physical {
    monitor: PHYSICAL_MONITOR,
    display: String,
}

// SAFETY: a `PHYSICAL_MONITOR` is a handle and a name buffer. It is not tied to
// the thread that opened it, and this type hands it to no one.
unsafe impl Send for Physical {}

impl Physical {
    /// Opens the first physical monitor behind a handle, if it answers.
    ///
    /// One `HMONITOR` can front several physical monitors where a card drives
    /// them as one surface. That arrangement cannot be addressed separately
    /// through the rest of this crate, whose displays come from
    /// `EnumDisplayMonitors`, so the first is taken and the rest are released.
    ///
    /// # Errors
    ///
    /// [`Error::CannotReach`] when the monitor does not answer, which is the
    /// ordinary answer for a laptop panel and for anything behind an adaptor
    /// that does not carry DDC/CI.
    pub(crate) fn open(handle: HMONITOR, display: &str) -> Result<Self> {
        let cannot_reach = || Error::CannotReach {
            mechanism: NAME,
            display: display.to_owned(),
        };

        let mut count: u32 = 0;
        // SAFETY: `count` is a live out parameter.
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(handle, std::ptr::addr_of_mut!(count)) }
            == 0
            || count == 0
        {
            return Err(cannot_reach());
        }

        let mut monitors: Vec<PHYSICAL_MONITOR> =
            vec![unsafe { std::mem::zeroed() }; count as usize];
        // SAFETY: `monitors` has exactly `count` elements, which is what this
        // call was told and what it fills.
        if unsafe { GetPhysicalMonitorsFromHMONITOR(handle, count, monitors.as_mut_ptr()) } == 0 {
            return Err(cannot_reach());
        }

        let monitor = monitors.remove(0);
        if !monitors.is_empty() {
            // SAFETY: the remainder came from the call above and is released
            // exactly once, here.
            unsafe {
                DestroyPhysicalMonitors(
                    u32::try_from(monitors.len()).unwrap_or(0),
                    monitors.as_ptr(),
                )
            };
        }

        let opened = Self {
            monitor,
            display: display.to_owned(),
        };

        // Asking is the only reliable test. A monitor having a handle is not a
        // promise that it speaks DDC/CI — the same lesson macOS taught, from the
        // other end.
        opened.read().map_err(|_| cannot_reach())?;
        Ok(opened)
    }

    /// The monitor's current, minimum and maximum, as it reports them.
    fn read(&self) -> Result<(u32, u32, u32)> {
        let (mut minimum, mut current, mut maximum) = (0_u32, 0_u32, 0_u32);

        // SAFETY: the handle came from `GetPhysicalMonitorsFromHMONITOR` and the
        // three out parameters are live.
        let ok = unsafe {
            GetMonitorBrightness(
                self.monitor.hPhysicalMonitor,
                std::ptr::addr_of_mut!(minimum),
                std::ptr::addr_of_mut!(current),
                std::ptr::addr_of_mut!(maximum),
            )
        };

        if ok == 0 {
            return Err(Error::NoReply {
                mechanism: NAME,
                display: self.display.clone(),
                attempts: 1,
            });
        }
        Ok((minimum, current, maximum))
    }
}

impl Drop for Physical {
    fn drop(&mut self) {
        // SAFETY: released exactly once, and the handle has not been used since.
        unsafe { DestroyPhysicalMonitors(1, std::ptr::addr_of!(self.monitor)) };
    }
}

impl Backend for Physical {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        let (minimum, current, maximum) = self.read()?;

        // Unlike the raw protocol, this reports a minimum as well, and it is not
        // always zero. The level is where the current value sits *within* that
        // range rather than against the maximum alone.
        Ok(scale(current, minimum, maximum))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let (minimum, _, maximum) = self.read()?;
        let span = f64::from(maximum.saturating_sub(minimum));
        let wanted = minimum + (f64::from(level.fraction()) * span).round() as u32;

        // SAFETY: the handle is live and `wanted` is within the range the
        // monitor just reported.
        if unsafe { SetMonitorBrightness(self.monitor.hPhysicalMonitor, wanted) } == 0 {
            return Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "SetMonitorBrightness",
                // This API reports failure through `GetLastError`, and the value
                // is only meaningful immediately after.
                code: last_error(),
            });
        }
        Ok(())
    }
}

fn last_error() -> i32 {
    // SAFETY: no arguments, and the value is read immediately after the call
    // that set it.
    unsafe { windows_sys::Win32::Foundation::GetLastError() as i32 }
}

/// Where a reading sits within the range the monitor reported.
///
/// Separated out because a monitor with a non-zero minimum is the case that
/// would otherwise be got wrong, and it cannot be reproduced without one.
fn scale(current: u32, minimum: u32, maximum: u32) -> Brightness {
    if maximum <= minimum {
        return Brightness::MIN;
    }
    let span = maximum - minimum;
    Brightness::from_range(
        u16::try_from(current.saturating_sub(minimum)).unwrap_or(u16::MAX),
        u16::try_from(span).unwrap_or(u16::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_is_placed_within_the_range_the_monitor_reported() {
        assert_eq!(scale(0, 0, 100), Brightness::MIN);
        assert_eq!(scale(100, 0, 100), Brightness::MAX);
        assert_eq!(scale(50, 0, 100).percent_rounded(), 50);
    }

    #[test]
    fn a_monitor_whose_minimum_is_not_zero_is_still_placed_correctly() {
        // The case this function exists for. Treating `current` as a fraction of
        // `maximum` would call this 60%, when the monitor is at its dimmest.
        assert_eq!(scale(60, 60, 100), Brightness::MIN);
        assert_eq!(scale(80, 60, 100).percent_rounded(), 50);
        assert_eq!(scale(100, 60, 100), Brightness::MAX);
    }

    #[test]
    fn a_range_that_is_not_one_yields_the_minimum_rather_than_dividing_by_it() {
        assert_eq!(scale(5, 10, 10), Brightness::MIN);
        assert_eq!(scale(5, 10, 0), Brightness::MIN);
    }
}
