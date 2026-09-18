//! Asking a display why it will not answer.

use crate::ddc::{CHIP_ADDRESS, DATA_ADDRESS, VCP_BRIGHTNESS};
use crate::diagnose::{Attempt, Note, Report, Verdict};
use crate::display::{Display, DisplayKind, displays};
use crate::error::Result;

use super::av_service::{self, AvService};
use super::ioreg;

/// The I2C address a display's EDID sits at.
///
/// The same two wires DDC/CI uses, a different address, and always populated on
/// a link that carries I2C — the machine could not be drawing a picture
/// otherwise. That is what makes it the control in this experiment.
const EDID_CHIP: u32 = 0x50;

/// How much of it to read. The first block is all that is needed to tell a real
/// EDID from noise.
const EDID_LEN: usize = 128;

/// The eight bytes every EDID begins with.
const EDID_HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

pub(crate) fn diagnose() -> Result<Vec<Report>> {
    displays()?.iter().map(probe).collect::<Result<Vec<_>>>()
}

fn probe(display: &Display) -> Result<Report> {
    let mut notes = Vec::new();
    let mut attempts = Vec::new();

    notes.push(Note {
        label: "Core Graphics id".to_owned(),
        value: display.id().to_string(),
    });

    let identity = display.identity();
    let nodes = ioreg::display_nodes();
    let node = ioreg::node_for(
        &nodes,
        identity.manufacturer,
        identity.product,
        identity.serial,
    );

    // Whether the registry describes this display at all. The built-in panel
    // publishes a product identifier far too wide to be an EDID product code, so
    // it never joins — worth saying plainly rather than reporting it as a
    // missing channel, which is a different thing.
    notes.push(Note {
        label: "registry node".to_owned(),
        value: match node {
            Some(_) => "matched".to_owned(),
            None => "no node matches this display's EDID numbers".to_owned(),
        },
    });

    // What the link is made of. A DisplayPort upstream with an HDMI downstream
    // means the conversion is happening inside the cable or adaptor, which is
    // exactly where I2C tends to get dropped.
    match node.and_then(|found| found.transport.clone()) {
        Some(transport) => notes.push(Note {
            label: "link".to_owned(),
            value: transport,
        }),
        None => notes.push(Note {
            label: "link".to_owned(),
            value: "not published".to_owned(),
        }),
    }

    if !av_service::available() {
        return Ok(Report {
            display: display.name().to_owned(),
            key: display.key().to_string(),
            kind: display.kind(),
            notes,
            attempts,
            verdict: Verdict::NoChannel,
        });
    }

    let channel = node.and_then(|found| found.av_service.as_ref());
    notes.push(Note {
        label: "I2C channel".to_owned(),
        value: match (node, channel) {
            (_, Some(_)) => "published".to_owned(),
            (Some(_), None) => "the node publishes none".to_owned(),
            (None, None) => "unknown, no node matched".to_owned(),
        },
    });

    let Some(service) = channel.and_then(|found| AvService::open(found.raw())) else {
        return Ok(Report {
            display: display.name().to_owned(),
            key: display.key().to_string(),
            kind: display.kind(),
            notes,
            attempts,
            verdict: if display.kind() == DisplayKind::BuiltIn {
                Verdict::NotApplicable
            } else {
                Verdict::NoChannel
            },
        });
    };

    // The control: can anything at all be read over this bus?
    let mut edid = [0_u8; EDID_LEN];
    let carries_i2c = match service.read(EDID_CHIP, 0x00, &mut edid) {
        Ok(()) if edid[..8] == EDID_HEADER => {
            attempts.push(Attempt {
                what: format!("read EDID over I2C (chip {EDID_CHIP:#04x})"),
                outcome: Ok(format!(
                    "{EDID_LEN} bytes, header valid, EDID version {}.{}",
                    edid[18], edid[19]
                )),
            });
            true
        }
        Ok(()) => {
            attempts.push(Attempt {
                what: format!("read EDID over I2C (chip {EDID_CHIP:#04x})"),
                outcome: Err(format!(
                    "read succeeded but the bytes are not an EDID: {:02x?}",
                    &edid[..8]
                )),
            });
            false
        }
        Err(code) => {
            attempts.push(Attempt {
                what: format!("read EDID over I2C (chip {EDID_CHIP:#04x})"),
                outcome: Err(describe(code)),
            });
            false
        }
    };

    // The thing actually wanted.
    let request = crate::ddc::get_request(VCP_BRIGHTNESS);
    let answers = match service.write(CHIP_ADDRESS, DATA_ADDRESS, &request) {
        Ok(()) => {
            attempts.push(Attempt {
                what: format!(
                    "DDC/CI Get VCP {VCP_BRIGHTNESS:#04x} (chip {CHIP_ADDRESS:#04x}, offset {DATA_ADDRESS:#04x})"
                ),
                outcome: Ok("write accepted".to_owned()),
            });
            true
        }
        Err(code) => {
            attempts.push(Attempt {
                what: format!(
                    "DDC/CI Get VCP {VCP_BRIGHTNESS:#04x} (chip {CHIP_ADDRESS:#04x}, offset {DATA_ADDRESS:#04x})"
                ),
                outcome: Err(describe(code)),
            });
            false
        }
    };

    let verdict = match (display.kind(), carries_i2c, answers) {
        (DisplayKind::BuiltIn, _, false) => Verdict::NotApplicable,
        (_, _, true) => Verdict::Answers,
        (_, true, false) => Verdict::MonitorDeclines,
        (_, false, false) => Verdict::LinkDoesNotCarryI2c,
    };

    Ok(Report {
        display: display.name().to_owned(),
        key: display.key().to_string(),
        kind: display.kind(),
        notes,
        attempts,
        verdict,
    })
}

/// Turns an `IOReturn` into something a person can act on.
///
/// The interesting part is the subsystem. A failure from `audio_video` is the
/// display coprocessor's own refusal and means the request got that far; a
/// failure from `common` is the ordinary IOKit vocabulary and usually means it
/// did not.
fn describe(code: i32) -> String {
    let raw = code as u32;
    let system = (raw >> 26) & 0x3f;
    let subsystem = (raw >> 14) & 0xfff;
    let detail = raw & 0x3fff;

    if system != 0x38 {
        return format!("{raw:#010x} (not an IOReturn)");
    }

    // From IOKit's IOReturn.h. Only the ones a display can plausibly produce.
    let named = match subsystem {
        0x00 => "common",
        0x05 => "graphics",
        0x0b => "smbus",
        0x1d => "thunderbolt",
        0x1e => "graphics_acceleration",
        0x45 => "audio_video",
        _ => "unknown",
    };

    let common = if subsystem == 0 {
        match detail {
            0x2bc => " (kIOReturnError)",
            0x2c0 => " (kIOReturnNoDevice)",
            0x2c2 => " (kIOReturnBadArgument)",
            0x2c7 => " (kIOReturnUnsupported)",
            0x2cf => " (kIOReturnNotOpen)",
            0x2d6 => " (kIOReturnTimeout)",
            0x2e2 => " (kIOReturnNotPermitted)",
            _ => "",
        }
    } else {
        ""
    };

    format!("{raw:#010x} — sub_iokit_{named}({subsystem:#04x}), code {detail}{common}")
}
