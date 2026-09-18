//! Brightness on Windows.
//!
//! Three mechanisms, and unlike macOS every one of them is documented, supported
//! public API:
//!
//! - external monitors, through the Monitor Configuration API in `dxva2`, which
//!   is DDC/CI with the driver doing the framing
//! - the machine's own panel, through WMI
//! - the gamma ramp, for anything that answers neither
//!
//! Because the driver speaks DDC/CI on this platform's behalf, [`crate::ddc`] is
//! not used here at all. The protocol that macOS and Linux hand-roll is inside
//! `dxva2`, which also reports the monitor's own minimum and maximum rather than
//! making the caller discover them.
//!
//! Written against Microsoft's documentation. It has never run: see `PLAN.md`.
//!
mod autostart;
mod gamma;
mod hdr;
mod monitors;
mod panel;
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
use self::panel::Wmi;
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

    // Only ever the panel this display *is*. WMI's brightness applies to a
    // monitor of its choosing unless told which, so `Wmi::open` matches on the
    // device instance path — without that, dimming an external monitor would
    // dim the laptop screen instead.
    match monitor.instance.as_deref() {
        Some(instance) => match Wmi::open(instance, &key) {
            Ok(built_in) => return Ok((Box::new(built_in), refusals)),
            Err(refusal) => refusals.push(refusal),
        },
        None => refusals.push(Error::CannotReach {
            mechanism: panel::NAME,
            display: key.clone(),
        }),
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

            // Asked of every display rather than only the ones that fail,
            // because a reader wants to know the display is in HDR whether or
            // not it turned out to matter.
            let hdr = monitor.instance.as_deref().and_then(hdr::enabled_for);
            notes.push(Note {
                label: "HDR".to_owned(),
                value: match hdr {
                    Some(true) => "on".to_owned(),
                    Some(false) => "off".to_owned(),
                    None => "could not be read".to_owned(),
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
                    why_it_failed(display.kind(), hdr)
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

/// What a refused `GetMonitorBrightness` amounts to.
///
/// Windows gives no way to tell a monitor that declined from a link that dropped
/// the request — the driver reports one failure for both, where macOS can
/// separate them because it speaks the protocol itself. So most of the time the
/// honest answer is that it is not clear.
///
/// HDR is the one case that can be lifted out of that, and it is worth lifting:
/// it is common, it is not a fault, and what to do about it is none of the
/// things someone would try on the strength of `Unclear`.
fn why_it_failed(kind: DisplayKind, hdr: Option<bool>) -> Verdict {
    if kind == DisplayKind::BuiltIn {
        return Verdict::NotApplicable;
    }

    // Only a definite yes. A query that could not be answered says nothing about
    // whether HDR is on, and naming it as the cause on the strength of silence
    // would be the same error as ruling it out on the strength of silence.
    if hdr == Some(true) {
        Verdict::HdrInTheWay
    } else {
        Verdict::Unclear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_panel_is_never_blamed_on_hdr() {
        // It does not use DDC/CI at all, so a refusal from it means nothing —
        // whatever the colour pipeline is doing.
        for hdr in [Some(true), Some(false), None] {
            assert_eq!(
                why_it_failed(DisplayKind::BuiltIn, hdr),
                Verdict::NotApplicable,
                "with hdr = {hdr:?}"
            );
        }
    }

    #[test]
    fn a_display_in_hdr_is_told_that_is_why() {
        assert_eq!(
            why_it_failed(DisplayKind::External, Some(true)),
            Verdict::HdrInTheWay
        );
    }

    #[test]
    fn hdr_that_could_not_be_read_is_not_evidence_that_it_is_on() {
        // The trap this exists for. `None` means the query failed, and treating
        // it as a yes would send someone to turn off an HDR mode they are not
        // in, while the real cause went unnamed.
        assert_eq!(
            why_it_failed(DisplayKind::External, None),
            Verdict::Unclear,
            "an unanswerable HDR query must not become an HDR verdict"
        );
    }

    #[test]
    fn a_display_not_in_hdr_keeps_the_honest_answer() {
        assert_eq!(
            why_it_failed(DisplayKind::External, Some(false)),
            Verdict::Unclear
        );
    }
}
