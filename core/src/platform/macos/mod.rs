//! Brightness on macOS.
//!
//! Three mechanisms, none of which have anything in common:
//!
//! - the built-in panel, through the private `DisplayServices` framework
//! - external monitors, through DDC/CI over IOKit's private `IOAVService`
//! - anything else, through the gamma ramp, which is not brightness at all
//!
//! Both hardware paths are undocumented Apple interfaces. There is no supported
//! alternative that reaches an external monitor's backlight, and this is what
//! every tool in this space uses, but it is why so much of this module is about
//! failing clearly rather than about doing the work.

mod av_service;
mod built_in;
mod diagnose;
mod display_services;
mod gamma;
mod graphics;
mod ioreg;
mod link;
mod login;

use crate::backend::Backend;
use crate::combined::Combined;
use crate::ddc::Ddc;
use crate::display::{Display, Found};
use crate::error::{Error, Result};
use crate::identity::Identity;

pub(crate) use self::diagnose::diagnose;
pub use self::login::{LoginItem, set as set_login_item, status as login_item};

use self::built_in::BuiltIn;
use self::gamma::Gamma;
use self::link::AvLink;

/// Every display that is on and drawing.
///
/// Core Graphics owns the list and the EDID numbers; the IORegistry owns the
/// names. The two share no identifier, so they are joined on the EDID numbers
/// both of them happen to carry.
pub(crate) fn displays() -> Result<Vec<Found>> {
    let attached = graphics::active_displays()?;
    let published = ioreg::display_nodes();

    Ok(attached
        .into_iter()
        .map(|display| {
            let node = ioreg::node_for(&published, display.vendor, display.model, display.serial);

            Found {
                id: display.id,
                identity: Identity {
                    built_in: display.is_builtin,
                    manufacturer: display.vendor,
                    product: display.model,
                    serial: display.serial,
                    printed_serial: node
                        .and_then(|found| found.attributes.alphanumeric_serial.clone()),
                },
                name: node.and_then(|found| found.attributes.name.clone()),
                is_main: display.is_main,
                bounds: display.bounds,
            }
        })
        .collect())
}

/// Where configuration belongs on this platform.
///
/// `~/Library/Application Support/klart`, as Apple's file system conventions
/// have it. [`None`] when there is no home directory to hang it off, which
/// happens in a sandbox and means remembering is simply off.
pub(crate) fn config_directory() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("klart"),
    )
}

/// Opens the best mechanism that will have this display.
///
/// In order of how real the result is: the panel's own framework, then DDC/CI,
/// then the gamma ramp on its own. The first two move a backlight; the third
/// only darkens the picture, so it is what is left rather than a peer of them.
///
/// Where a backlight is found it is *paired* with the ramp rather than used
/// alone, so that the range carries on below the point the backlight stops at.
/// See [`crate::combined`].
///
/// # Errors
///
/// Only if everything refuses, which in practice means the display went away
/// between being listed and being opened — the gamma ramp is available on any
/// display that exists, which is why it is last.
pub(crate) fn open(display: &Display) -> Result<(Box<dyn Backend>, Vec<Error>)> {
    let mut refusals = Vec::new();

    let backlight: Option<Box<dyn Backend>> = match BuiltIn::open(display) {
        Ok(panel) => Some(Box::new(panel)),
        Err(refusal) => {
            refusals.push(refusal);
            match AvLink::open(display).and_then(|link| Ddc::open(link, display.key().as_str())) {
                Ok(monitor) => Some(Box::new(monitor)),
                Err(refusal) => {
                    refusals.push(refusal);
                    None
                }
            }
        }
    };

    let ramp = Gamma::open(display)?;

    match backlight {
        Some(backlight) => Ok((
            Box::new(Combined::new(backlight, Box::new(ramp))?),
            refusals,
        )),
        None => Ok((Box::new(ramp), refusals)),
    }
}
