//! Brightness on Windows.
//!
//! Three mechanisms, and unlike macOS every one of them is documented, supported
//! public API:
//!
//! - external monitors, through the Monitor Configuration API in `dxva2`, which
//!   is DDC/CI with the driver doing the framing
//! - the gamma ramp, for anything that does not answer it
//! - the laptop panel, which is **not implemented** — see below
//!
//! Because the driver speaks DDC/CI on this platform's behalf, [`crate::ddc`] is
//! not used here at all. The protocol that macOS and Linux hand-roll is inside
//! `dxva2`, which also reports the monitor's own minimum and maximum rather than
//! making the caller discover them.
//!
//! Written against Microsoft's documentation. It has never run: see `PLAN.md`.
//!
//! # The laptop panel
//!
//! A built-in panel has no DDC/CI, and the documented way to reach it is the WMI
//! class `WmiMonitorBrightnessMethods` — which means COM, which means several
//! hundred lines that cannot be checked from here. It is deliberately left out
//! rather than guessed at, so an internal panel falls through to the gamma ramp
//! and gets a usable control that is not the right one. `PLAN.md` records it.

mod autostart;
mod gamma;
mod monitors;
mod physical;

use std::path::PathBuf;

use crate::backend::Backend;
use crate::diagnose::{Attempt, Note, Report, Verdict};
use crate::display::{Bounds, Display, DisplayKind, Found};
use crate::edid;
use crate::error::{Error, Result};
use crate::identity::Identity;

pub use self::autostart::{set as set_login_item, status as login_item};

use self::gamma::Gamma;
use self::monitors::Monitor;
use self::physical::Physical;

/// Every monitor attached to the machine.
///
/// # Errors
///
/// Never in practice; a machine with no monitors reports none.
pub(crate) fn displays() -> Result<Vec<Found>> {
    Ok(monitors::attached()
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| found(index, &monitor))
        .collect())
}

fn found(index: usize, monitor: &Monitor) -> Found {
    let parsed = edid::parse(&monitor.edid);

    Found {
        // An `HMONITOR` is a pointer-sized handle and not stable across a
        // reconnect, so this is a position in the list. `DisplayKey` is the
        // identity that matters and it does not come from here.
        id: u32::try_from(index).unwrap_or(0),
        identity: Identity {
            // Windows does not say outright whether a panel is internal. The
            // EDID does, by convention: a built-in panel's descriptor carries no
            // serial and its manufacturer is the machine's own. Rather than
            // guess, this reports everything as external and lets the mechanism
            // order sort it out — the panel will refuse DDC/CI and fall to the
            // ramp, which is what would happen anyway.
            built_in: false,
            manufacturer: parsed.as_ref().map_or(0, |found| found.manufacturer),
            product: parsed.as_ref().map_or(0, |found| found.product),
            serial: parsed.as_ref().map_or(0, |found| found.serial),
            printed_serial: parsed
                .as_ref()
                .and_then(|found| found.printed_serial.clone()),
        },
        name: parsed
            .and_then(|found| found.name)
            .or_else(|| Some(monitor.adapter.clone())),
        is_main: monitor.primary,
        bounds: Bounds {
            x: monitor.rect.left,
            y: monitor.rect.top,
            width: monitor
                .rect
                .right
                .saturating_sub(monitor.rect.left)
                .unsigned_abs(),
            height: monitor
                .rect
                .bottom
                .saturating_sub(monitor.rect.top)
                .unsigned_abs(),
        },
    }
}

/// Opens the best mechanism that will have this display.
///
/// DDC/CI first, then the gamma ramp, in the same order and for the same reason
/// as everywhere else: one moves a backlight and the other darkens a picture.
///
/// # Errors
///
/// When neither answers, which means the monitor has gone away.
pub(crate) fn open(display: &Display) -> Result<(Box<dyn Backend>, Vec<Error>)> {
    let mut refusals = Vec::new();
    let key = display.key().to_string();

    let Some(monitor) = monitor_for(display) else {
        return Err(Error::CannotReach {
            mechanism: "any",
            display: key,
        });
    };

    match Physical::open(monitor.handle, &key) {
        Ok(external) => return Ok((Box::new(external), refusals)),
        Err(refusal) => refusals.push(refusal),
    }

    Ok((Box::new(Gamma::open(&monitor.adapter)?), refusals))
}

/// The monitor a display came from, matched on its own EDID.
///
/// By what the display published rather than by its position, for the reason
/// Linux learned: the list is read again here, and a monitor plugged or
/// unplugged in between would shift every index after it.
fn monitor_for(display: &Display) -> Option<Monitor> {
    let wanted = display.identity();

    monitors::attached().into_iter().find(|monitor| {
        edid::parse(&monitor.edid).is_some_and(|found| {
            found.manufacturer == wanted.manufacturer
                && found.product == wanted.product
                && found.serial == wanted.serial
        })
    })
}

/// Where configuration belongs on this platform.
///
/// `%APPDATA%\klart`, which is the roaming profile and so follows a domain user
/// between machines — which is exactly what a file of display keys should do.
pub(crate) fn config_directory() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    if appdata.is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join("klart"))
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
            let mut notes = Vec::new();
            let mut attempts = Vec::new();

            let Some(monitor) = monitor_for(display) else {
                return Ok(Report {
                    display: display.name().to_owned(),
                    key: display.key().to_string(),
                    kind: display.kind(),
                    notes,
                    attempts,
                    address_honoured: None,
                    verdict: Verdict::NoChannel,
                });
            };

            notes.push(Note {
                label: "adapter".to_owned(),
                value: monitor.adapter.clone(),
            });
            notes.push(Note {
                label: "EDID".to_owned(),
                value: if monitor.edid.is_empty() {
                    "the driver stored none".to_owned()
                } else {
                    format!("{} bytes from the registry", monitor.edid.len())
                },
            });

            let verdict = match Physical::open(monitor.handle, display.key().as_str()) {
                Ok(_) => {
                    attempts.push(Attempt {
                        what: "GetMonitorBrightness".to_owned(),
                        outcome: Ok("answered".to_owned()),
                    });
                    Verdict::Answers
                }
                Err(problem) => {
                    attempts.push(Attempt {
                        what: "GetMonitorBrightness".to_owned(),
                        outcome: Err(problem.to_string()),
                    });
                    // Windows gives no way to tell a monitor that declined from
                    // a link that dropped the request — the driver reports one
                    // failure for both. macOS can distinguish them because it
                    // speaks the protocol itself.
                    if display.kind() == DisplayKind::BuiltIn {
                        Verdict::NotApplicable
                    } else {
                        Verdict::Unclear
                    }
                }
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
