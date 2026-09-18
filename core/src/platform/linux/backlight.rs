//! The panel's own backlight, through sysfs.
//!
//! `/sys/class/backlight/<device>/brightness` against `max_brightness`. Plain
//! files, no ioctl, no library — and the one thing the DDC path cannot reach,
//! because a laptop panel has no I2C bus to talk to. `ddcutil` says the same:
//! laptop displays "use a special API, not I2C".
//!
//! Reading is world readable. Writing is not: the file belongs to root, and a
//! desktop user gets there through a udev rule, membership of a group the
//! distribution chose, or `logind`. That is a packaging decision rather than a
//! code one, so a refusal is reported as what it is.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "sysfs backlight";

/// Where the kernel publishes them.
const BACKLIGHT: &str = "/sys/class/backlight";

/// A panel's backlight.
#[derive(Debug)]
pub(crate) struct Sysfs {
    path: PathBuf,
    maximum: u32,
    display: String,
}

impl Sysfs {
    /// Finds the backlight for a display.
    ///
    /// There is rarely more than one on a machine and no reliable way to bind a
    /// particular one to a particular connector, so the first that reports a
    /// sane maximum is taken. That is right for a laptop, which has one panel;
    /// it would be wrong on a machine with two internal panels, which is not a
    /// machine that exists.
    ///
    /// # Errors
    ///
    /// [`Error::CannotReach`] when there is no backlight, which is every
    /// external monitor.
    pub(crate) fn open(display: &str) -> Result<Self> {
        let cannot_reach = || Error::CannotReach {
            mechanism: NAME,
            display: display.to_owned(),
        };

        let entries = fs::read_dir(BACKLIGHT).map_err(|_| cannot_reach())?;

        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            let Some(maximum) = number(&path.join("max_brightness")) else {
                continue;
            };
            if maximum == 0 {
                continue;
            }
            return Ok(Self {
                path,
                maximum,
                display: display.to_owned(),
            });
        }

        Err(cannot_reach())
    }
}

impl Backend for Sysfs {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        let raw = number(&self.path.join("brightness")).ok_or_else(|| Error::CannotReach {
            mechanism: NAME,
            display: self.display.clone(),
        })?;

        Ok(Brightness::from_range(
            u16::try_from(raw).unwrap_or(u16::MAX),
            u16::try_from(self.maximum).unwrap_or(u16::MAX),
        ))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        // Scaled through the panel's own range, the same way DDC/CI is scaled
        // through a monitor's reported maximum.
        let raw = (f64::from(level.fraction()) * f64::from(self.maximum)).round() as u32;

        fs::write(self.path.join("brightness"), raw.to_string()).map_err(|problem| {
            Error::MechanismFailed {
                mechanism: NAME,
                call: "write to sysfs brightness",
                // `raw_os_error` is `None` only for errors this crate
                // constructed, and this one came from the kernel.
                code: problem.raw_os_error().unwrap_or(-1),
            }
        })
    }
}

fn number(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}
