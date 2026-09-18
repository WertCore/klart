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
//! The question that does most of the work is whether the link carries I2C *at
//! all*. A display's EDID lives on the same two wires as DDC/CI, at a different
//! address, and it is readable by definition — the machine is already using the
//! picture. So an EDID that reads back over I2C proves the wires are connected
//! end to end, which means a DDC/CI refusal is the monitor's decision. An EDID
//! that does not proves the opposite: something between here and the monitor is
//! not carrying I2C, and no amount of DDC/CI framing will change that.

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
    /// The channel exists but does not reach the monitor.
    ///
    /// The EDID could not be read over it either, and the EDID is the one thing
    /// on that bus which is always there.
    LinkDoesNotCarryI2c,
    /// The channel reaches the monitor, which will not talk DDC/CI.
    MonitorDeclines,
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

            Self::LinkDoesNotCarryI2c => {
                "The monitor's own EDID could not be read over I2C, and the EDID is always there \
                 on a working bus — so something between this Mac and the monitor is not passing \
                 I2C through. That is the cable, adaptor, hub or dock. Cheap USB-C-to-HDMI \
                 converters and DisplayLink docks commonly strip DDC/CI while passing the \
                 picture perfectly. Try a USB-C-to-DisplayPort cable into the monitor's \
                 DisplayPort input, which needs no protocol conversion at all."
            }

            Self::MonitorDeclines => {
                "The link carries I2C — the monitor's EDID was read over it — so the wires are \
                 fine and the monitor is refusing DDC/CI itself. Most monitors ship with it \
                 switched off. Look in the on-screen menu under System, General or Setup for an \
                 entry called DDC/CI, Monitor Control, External Control or PC Control, and turn \
                 it on."
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
