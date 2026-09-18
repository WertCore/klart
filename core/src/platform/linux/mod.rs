//! Brightness on Linux.
//!
//! Two mechanisms, and they divide cleanly by what a display is:
//!
//! - a panel built into the machine, through `/sys/class/backlight`
//! - anything plugged in, through DDC/CI over `/dev/i2c-*`
//!
//! Both are documented, supported interfaces, which is the opposite of the
//! situation on macOS. What Linux does not offer without linking a display
//! server is a gamma ramp, so there is no software fallback here: a display that
//! answers neither mechanism has no control rather than a poor one.
//!
//! Written against the kernel's interfaces and the `ddcutil` project's account
//! of them. It has never run: see `PLAN.md`.

mod autostart;
mod backlight;
mod drm;
mod i2c;

use std::path::PathBuf;

use crate::backend::Backend;
use crate::ddc::Ddc;
use crate::diagnose::{Attempt, Note, Report, Verdict};
use crate::display::{Bounds, Display, DisplayKind, Found};
use crate::edid;
use crate::error::{Error, Result};
use crate::identity::Identity;

pub use self::autostart::{set as set_login_item, status as login_item};

use self::backlight::Sysfs;
use self::drm::Connector;
use self::i2c::I2cLink;

/// Every connector with a display attached.
///
/// # Errors
///
/// Never, in practice. A machine whose sysfs cannot be read reports no displays
/// rather than failing, because that is what a container without the host's
/// sysfs looks like and it is not this crate's business to object.
pub(crate) fn displays() -> Result<Vec<Found>> {
    Ok(drm::connected()
        .into_iter()
        .enumerate()
        .map(|(index, connector)| found(index, &connector))
        .collect())
}

fn found(index: usize, connector: &Connector) -> Found {
    let parsed = edid::parse(&connector.edid);

    Found {
        // No stable integer handle exists the way `CGDirectDisplayID` does, so
        // this is a position in the list. `DisplayKey` is the identity that
        // matters and it does not come from here.
        id: u32::try_from(index).unwrap_or(0),
        identity: Identity {
            built_in: connector.built_in,
            manufacturer: parsed.as_ref().map_or(0, |found| found.manufacturer),
            product: parsed.as_ref().map_or(0, |found| found.product),
            serial: parsed.as_ref().map_or(0, |found| found.serial),
            printed_serial: parsed
                .as_ref()
                .and_then(|found| found.printed_serial.clone()),
        },
        // Falling back to the connector's own name, which is at least something
        // a person can match against what is plugged in where.
        name: parsed
            .and_then(|found| found.name)
            .or_else(|| Some(connector.name.clone())),
        is_main: index == 0,
        // The kernel does not know where a compositor put a display. See
        // `drm` for why that is tolerable.
        bounds: Bounds {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
    }
}

/// Opens the mechanism that will have this display.
///
/// A built-in panel goes to its backlight and an external one to DDC/CI. Unlike
/// macOS there is no third mechanism to fall back on, so a display that answers
/// neither reports why rather than being dimmed badly.
///
/// # Errors
///
/// When nothing reaches the display. The refusals say which was tried.
pub(crate) fn open(display: &Display) -> Result<(Box<dyn Backend>, Vec<Error>)> {
    let mut refusals = Vec::new();
    let key = display.key().to_string();

    if display.kind() == DisplayKind::BuiltIn {
        match Sysfs::open(&key) {
            Ok(panel) => return Ok((Box::new(panel), refusals)),
            Err(refusal) => refusals.push(refusal),
        }
    }

    match link(display) {
        Ok(link) => match Ddc::open(link, &key) {
            Ok(monitor) => return Ok((Box::new(monitor), refusals)),
            Err(refusal) => refusals.push(refusal),
        },
        Err(refusal) => refusals.push(refusal),
    }

    // A built-in panel with no backlight is worth trying I2C on anyway — some
    // do publish one — which is why this comes after rather than instead.
    if display.kind() != DisplayKind::BuiltIn {
        match Sysfs::open(&key) {
            Ok(panel) => return Ok((Box::new(panel), refusals)),
            Err(refusal) => refusals.push(refusal),
        }
    }

    Err(refusals.pop().unwrap_or(Error::CannotReach {
        mechanism: "any",
        display: key,
    }))
}

/// The I2C link for a display, if its connector publishes one.
fn link(display: &Display) -> Result<I2cLink> {
    let cannot_reach = || Error::CannotReach {
        mechanism: crate::ddc::NAME,
        display: display.key().to_string(),
    };

    // By what the display published rather than by its position in the list.
    // The list is read again here, and a monitor plugged or unplugged in between
    // would shift every index after it — which would quietly drive the wrong
    // display.
    let bus = connector_for(display)
        .ok_or_else(cannot_reach)?
        .i2c_bus()
        .ok_or_else(cannot_reach)?;

    I2cLink::open(bus).map_err(|problem| Error::MechanismFailed {
        mechanism: crate::ddc::NAME,
        call: "open /dev/i2c",
        code: problem.raw_os_error().unwrap_or(-1),
    })
}

/// The connector a display came from, matched on its own EDID.
fn connector_for(display: &Display) -> Option<Connector> {
    let wanted = display.identity();

    drm::connected().into_iter().find(|connector| {
        edid::parse(&connector.edid).is_some_and(|found| {
            found.manufacturer == wanted.manufacturer
                && found.product == wanted.product
                && found.serial == wanted.serial
        })
    })
}

/// Where configuration belongs on this platform.
///
/// `$XDG_CONFIG_HOME/klart`, or `~/.config/klart` when that is unset, as the XDG
/// base directory specification has it.
pub(crate) fn config_directory() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("klart"));
    }

    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".config").join("klart"))
}

/// Probes every display and reports what it found.
///
/// # Errors
///
/// As [`displays`].
pub(crate) fn diagnose() -> Result<Vec<Report>> {
    crate::display::displays()?
        .iter()
        .map(|display| {
            let mut notes = vec![Note {
                label: "connector".to_owned(),
                value: connector_for(display).map_or_else(|| "gone".to_owned(), |found| found.name),
            }];
            let mut attempts = Vec::new();

            if display.kind() == DisplayKind::BuiltIn {
                notes.push(Note {
                    label: "kind".to_owned(),
                    value: "built-in panel, which has no I2C bus".to_owned(),
                });
                return Ok(Report {
                    display: display.name().to_owned(),
                    key: display.key().to_string(),
                    kind: display.kind(),
                    notes,
                    attempts,
                    address_honoured: None,
                    verdict: Verdict::NotApplicable,
                });
            }

            let verdict = match link(display) {
                Err(problem) => {
                    attempts.push(Attempt {
                        what: "open the connector's I2C bus".to_owned(),
                        outcome: Err(problem.to_string()),
                    });
                    Verdict::NoChannel
                }
                Ok(link) => match Ddc::open(link, display.key().as_str()) {
                    Ok(_) => {
                        attempts.push(Attempt {
                            what: "DDC/CI Get VCP 0x10".to_owned(),
                            outcome: Ok("answered".to_owned()),
                        });
                        Verdict::Answers
                    }
                    Err(problem) => {
                        attempts.push(Attempt {
                            what: "DDC/CI Get VCP 0x10".to_owned(),
                            outcome: Err(problem.to_string()),
                        });
                        Verdict::MonitorDeclines
                    }
                },
            };

            Ok(Report {
                display: display.name().to_owned(),
                key: display.key().to_string(),
                kind: display.kind(),
                notes,
                attempts,
                address_honoured: None,
                verdict,
            })
        })
        .collect()
}
