//! Asking a display why it will not answer.

use crate::ddc::{CHIP_ADDRESS, HOST_ADDRESS, REPLY_DELAY, REPLY_LEN, VCP_BRIGHTNESS};
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

/// How many bytes to compare between the two addresses.
///
/// A whole EDID block, because a short read could match by coincidence where a
/// hundred and twenty-eight bytes cannot.
const SAMPLE: usize = 128;

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
    )
    .or_else(|| built_in_node(&nodes, display));

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
            address_honoured: None,
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
            address_honoured: None,
            verdict: if display.kind() == DisplayKind::BuiltIn {
                Verdict::NotApplicable
            } else {
                Verdict::NoChannel
            },
        });
    };

    // The reference: whatever the EDID address returns.
    let mut edid = [0_u8; SAMPLE];
    let edid_read = service.read(EDID_CHIP, 0x00, &mut edid);
    attempts.push(Attempt {
        what: format!("read {SAMPLE} bytes at the EDID address ({EDID_CHIP:#04x})"),
        outcome: match &edid_read {
            Ok(()) if edid[..8] == EDID_HEADER => Ok(format!(
                "valid EDID header, version {}.{}",
                edid[18], edid[19]
            )),
            Ok(()) => Ok(format!("{:02x?}, which is not an EDID header", &edid[..8])),
            Err(code) => Err(describe(*code)),
        },
    });

    // The control: the same offset at the DDC/CI address. On a link doing real
    // I2C these differ, because they are two different devices. Identical bytes
    // mean the address was ignored and both came from the same cache.
    let mut at_ddc = [0_u8; SAMPLE];
    let ddc_read = service.read(CHIP_ADDRESS, 0x00, &mut at_ddc);
    let address_ignored = edid_read.is_ok() && ddc_read.is_ok() && at_ddc == edid;
    attempts.push(Attempt {
        what: format!("read the same {SAMPLE} bytes at the DDC/CI address ({CHIP_ADDRESS:#04x})"),
        outcome: match &ddc_read {
            Ok(()) if address_ignored => Err(
                "byte for byte identical to the EDID address — the chip address is being ignored"
                    .to_owned(),
            ),
            Ok(()) => Ok("differs from the EDID address, so the address is honoured".to_owned()),
            Err(code) => Err(describe(*code)),
        },
    });

    let reaches_monitor = ddc_read.is_ok() && !address_ignored;

    // The thing actually wanted, and the whole exchange rather than half of it.
    // A write being accepted says only that the request left the machine; what
    // makes a display answer DDC/CI is a reply that decodes.
    let what = format!(
        "DDC/CI Get VCP {VCP_BRIGHTNESS:#04x} (chip {CHIP_ADDRESS:#04x}, offset {HOST_ADDRESS:#04x})"
    );
    let request = crate::ddc::get_request(VCP_BRIGHTNESS);
    let answers = match service.write(CHIP_ADDRESS, u32::from(HOST_ADDRESS), &request) {
        Err(code) => {
            attempts.push(Attempt {
                what,
                outcome: Err(describe(code)),
            });
            false
        }
        Ok(()) => {
            std::thread::sleep(REPLY_DELAY);
            let mut reply = [0_u8; REPLY_LEN];

            match service.read(CHIP_ADDRESS, u32::from(HOST_ADDRESS), &mut reply) {
                Err(code) => {
                    attempts.push(Attempt {
                        what,
                        outcome: Err(format!(
                            "write accepted, reading the reply: {}",
                            describe(code)
                        )),
                    });
                    false
                }
                Ok(()) => match crate::ddc::decode_reply(VCP_BRIGHTNESS, &reply) {
                    Some(reading) => {
                        attempts.push(Attempt {
                            what,
                            outcome: Ok(format!(
                                "answered: {} of {}",
                                reading.current, reading.maximum
                            )),
                        });
                        true
                    }
                    None => {
                        attempts.push(Attempt {
                            what,
                            outcome: Err(format!(
                                "write accepted but the reply is not one: {:02x?}",
                                &reply[..8.min(reply.len())]
                            )),
                        });
                        false
                    }
                },
            }
        }
    };

    let verdict = match (display.kind(), answers) {
        (_, true) => Verdict::Answers,
        (DisplayKind::BuiltIn, false) => Verdict::NotApplicable,
        (_, false) if address_ignored => Verdict::EdidOnly,
        (_, false) if reaches_monitor => Verdict::MonitorDeclines,
        (_, false) if edid_read.is_err() && ddc_read.is_err() => Verdict::NoI2c,
        (_, false) => Verdict::Unclear,
    };

    Ok(Report {
        display: display.name().to_owned(),
        key: display.key().to_string(),
        kind: display.kind(),
        notes,
        attempts,
        address_honoured: (edid_read.is_ok() && ddc_read.is_ok()).then_some(!address_ignored),
        verdict,
    })
}

/// The registry node for the built-in panel, which the EDID join cannot find.
///
/// The panel publishes a product identifier far wider than an EDID product code
/// — forty-six bits on this machine — so it never equals what Core Graphics
/// reports and the ordinary join misses it. Nothing depends on finding it to
/// control a display, because the panel has its own mechanism. It matters here
/// because the panel's I2C channel is the control in this experiment: a link
/// that honours the chip address, on the same machine, through the same calls.
///
/// Matching on the manufacturer alone is safe for exactly one display, and a
/// machine has exactly one built-in panel.
fn built_in_node<'a>(
    nodes: &'a [ioreg::DisplayNode],
    display: &Display,
) -> Option<&'a ioreg::DisplayNode> {
    if display.kind() != DisplayKind::BuiltIn {
        return None;
    }

    let manufacturer = u64::from(display.identity().manufacturer);
    let mut candidates = nodes.iter().filter(|node| {
        node.attributes.legacy_manufacturer_id == Some(manufacturer)
            && node.attributes.name.is_none()
    });

    let only = candidates.next()?;
    // More than one and the match means nothing.
    candidates.next().is_none().then_some(only)
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
