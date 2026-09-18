//! Finding out why a display will not answer.
//!
//! Every tool in this space reports the same thing when DDC/CI fails: that it
//! failed. That is the least useful true statement available, because the four
//! ordinary causes want four different responses from the person reading it —
//! turn a setting on, change a cable, use a different port, or give up and let
//! the software fallback have it.
//!
//! So this asks a series of questions whose answers separate those cases, and
//! reports what it found rather than a yes or a no.
//!
//! The question that does most of the work is whether the link does general I2C
//! at all, and the way to find out is not the obvious one.
//!
//! Reading a display's EDID looks like the right control — it lives on the same
//! two wires as DDC/CI at a different address, and it is always there. But it is
//! not, because a successful EDID read does not prove an I2C transaction
//! happened. The display coprocessor reads and caches the EDID when the link
//! comes up, and on a link where it cannot run I2C it will serve that cache and
//! ignore the address it was asked for.
//!
//! So the control is to read the *same offset* at two different chip addresses.
//! On a link doing real I2C those answer differently, because one is the EDID
//! EEPROM and the other is the DDC/CI slave. On a link serving a cache they are
//! byte for byte identical, and that identity is the tell.

use crate::display::DisplayKind;
use crate::error::Result;
use crate::platform;

/// What probing one display found.
#[derive(Debug, Clone)]
pub struct Report {
    /// The display's name.
    pub display: String,
    /// Its key, as `klart list` prints it.
    pub key: String,
    /// Whether it is the built-in panel.
    pub kind: DisplayKind,
    /// Facts gathered about the link before anything was tried.
    pub notes: Vec<Note>,
    /// What was tried, in order, and what came back.
    pub attempts: Vec<Attempt>,
    /// Whether this link honoured the I2C chip address it was given.
    ///
    /// The finding every verdict here rests on, exposed so that a caller can
    /// corroborate it: a machine where one link honours the address and another
    /// does not has proved the difference is the link rather than the API.
    pub address_honoured: Option<bool>,
    /// What the above adds up to.
    pub verdict: Verdict,
}

/// One fact about a display or its link.
#[derive(Debug, Clone)]
pub struct Note {
    /// What it is.
    pub label: String,
    /// What it says.
    pub value: String,
}

/// One thing that was tried.
#[derive(Debug, Clone)]
pub struct Attempt {
    /// What was asked.
    pub what: String,
    /// A summary of what came back, or the error, already decoded.
    pub outcome: std::result::Result<String, String>,
}

/// What a display's answers add up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The display answered DDC/CI.
    Answers,
    /// The built-in panel, which is not supposed to answer DDC/CI.
    NotApplicable,
    /// Nothing on this machine offers an I2C channel to this display.
    NoChannel,
    /// The channel exists and reads nothing at all.
    NoI2c,
    /// The channel serves the cached EDID and does no I2C.
    ///
    /// Reads return the same bytes whichever address is asked for, and writes
    /// are refused outright — so nothing on this link ever reaches the monitor.
    EdidOnly,
    /// The channel reaches the monitor, which will not talk DDC/CI.
    MonitorDeclines,
    /// The display is in HDR, which is why brightness control is not answering.
    ///
    /// Not a fault in the link or the monitor. HDR changes the display pipeline
    /// underneath brightness control, and a monitor in an HDR picture mode
    /// commonly pins its brightness or stops honouring the feature entirely.
    HdrInTheWay,
    /// The answers do not fit any of the above.
    Unclear,
}

impl Verdict {
    /// What the person reading this should do about it.
    #[must_use]
    pub fn advice(&self) -> &'static str {
        match self {
            Self::Answers => "Nothing — DDC/CI is working on this display.",

            Self::NotApplicable => {
                "Nothing — the built-in panel has its own mechanism and does not use DDC/CI."
            }

            Self::NoChannel => {
                "This display has no I2C channel on this machine at all. That is what a virtual \
                 screen looks like — AirPlay, Sidecar, or a DisplayLink adaptor, none of which \
                 carry DDC/CI by design. Software dimming is the only option."
            }

            Self::NoI2c => {
                "Nothing could be read over this link at all, not even the EDID. Something \
                 between this Mac and the monitor is not passing I2C. Try a different cable, and \
                 prefer one with no protocol conversion in it."
            }

            Self::EdidOnly => {
                "This link serves a cached copy of the monitor's EDID and does no I2C: reads \
                 return the same bytes whichever address they ask for, and every write is \
                 refused. Nothing here ever reaches the monitor, so this is not the monitor's \
                 DDC/CI setting and no software can change it — the monitor is never asked. \
                 The display coprocessor behaves this way when it cannot run I2C over the link, \
                 which is what a DisplayPort-to-HDMI conversion inside a cable or adaptor \
                 causes. Use a link with no conversion in it: USB-C to DisplayPort, into the \
                 monitor's DisplayPort input."
            }

            Self::MonitorDeclines => {
                "The link does real I2C — two addresses answered differently, so transactions \
                 are reaching the monitor — and the monitor is refusing DDC/CI itself. Most \
                 monitors ship with it switched off. Look in the on-screen menu under System, \
                 General or Setup for an entry called DDC/CI, Monitor Control, External Control \
                 or PC Control, and turn it on."
            }

            Self::HdrInTheWay => {
                "This display is in HDR, and brightness control did not answer. Those two facts \
                 go together: a monitor in an HDR picture mode commonly pins its brightness to a \
                 preset or stops honouring the brightness feature at all, and the panel's own \
                 controls are what set the level instead. Software dimming is not a way around \
                 it either — Windows does not guarantee gamma ramp behaviour while HDR is on, so \
                 the fallback may be weakened or ignored as well. Turn HDR off to get brightness \
                 control back, or set the level in the monitor's own menu and leave it there."
            }

            Self::Unclear => {
                "The answers do not fit a known pattern. The attempts above are the raw evidence; \
                 the exact IOReturn codes are worth reporting in an issue."
            }
        }
    }
}

/// Probes every attached display and reports what it found.
///
/// # Errors
///
/// Fails only if the displays cannot be enumerated at all.
pub fn diagnose() -> Result<Vec<Report>> {
    platform::diagnose()
}
