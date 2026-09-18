//! The macOS end of a DDC/CI link.
//!
//! All this does is put bytes on the I2C pair and take them off again. The
//! protocol they carry is in [`crate::ddc`], where it is shared with every other
//! platform, because DDC/CI is the same everywhere and only the transport is
//! not.

use crate::ddc::{CHIP_ADDRESS, DATA_ADDRESS, Link, NAME};
use crate::display::Display;
use crate::error::{Error, Result};

use super::av_service::{self, AvService};
use super::ioreg;

/// A DDC/CI link over IOKit's `IOAVService`.
pub(crate) struct AvLink {
    service: AvService,
}

impl AvLink {
    /// Finds the I2C channel for a display, if it has one.
    ///
    /// # Errors
    ///
    /// [`Error::MechanismUnavailable`] if the private calls are missing, and
    /// [`Error::CannotReach`] if the display has no registry node or no channel
    /// on it. A channel existing is not a promise that anything answers on it;
    /// that is [`crate::ddc::Ddc::open`]'s question.
    pub(crate) fn open(display: &Display) -> Result<Self> {
        if !av_service::available() {
            return Err(Error::MechanismUnavailable { mechanism: NAME });
        }

        let cannot_reach = || Error::CannotReach {
            mechanism: NAME,
            display: display.key().to_string(),
        };

        let identity = display.identity();
        let nodes = ioreg::display_nodes();
        let node = ioreg::node_for(
            &nodes,
            identity.manufacturer,
            identity.product,
            identity.serial,
        )
        .ok_or_else(cannot_reach)?;

        let channel = node.av_service.as_ref().ok_or_else(cannot_reach)?;
        let service = AvService::open(channel.raw()).ok_or_else(cannot_reach)?;

        Ok(Self { service })
    }
}

impl Link for AvLink {
    fn write(&self, bytes: &[u8]) -> std::result::Result<(), i32> {
        self.service.write(CHIP_ADDRESS, DATA_ADDRESS, bytes)
    }

    fn read(&self, into: &mut [u8]) -> std::result::Result<(), i32> {
        self.service.read(CHIP_ADDRESS, DATA_ADDRESS, into)
    }
}
