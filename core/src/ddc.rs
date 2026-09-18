//! DDC/CI, which is the same protocol everywhere.
//!
//! A request-and-reply protocol carried on the display connector's I2C pair. The
//! monitor answers at chip address `0x37`; a request names a VCP feature —
//! `0x10` is brightness — and a reply carries the current value together with
//! the maximum the monitor has chosen for it, which is not the same number on
//! any two models.
//!
//! None of that is platform-specific. What differs between macOS, Windows and
//! Linux is only how the bytes reach the wire, which is [`Link`]. Everything
//! above it — the framing, the checksums, the validation and the retries — lives
//! here and is tested without any hardware at all.
//!
//! It is also slow and lossy in a way that most buses are not. The
//! specification's timings are tens of milliseconds, a monitor that is busy
//! simply does not answer, and there is no flow control to notice that with. So
//! every exchange is retried, and every reply is checked against the request
//! that prompted it rather than trusted for having arrived.

use std::thread::sleep;
use std::time::Duration;

use crate::Brightness;
use crate::backend::Backend;
use crate::error::{Error, Result};

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "DDC/CI";

/// The DDC/CI chip address, which every transport needs and none of them
/// interpret.
pub(crate) const CHIP_ADDRESS: u32 = 0x37;

/// The offset every DDC/CI message is written at and read from.
pub(crate) const DATA_ADDRESS: u32 = 0x51;

/// The monitor's address on the bus, in the eight-bit form the checksum covers.
///
/// Not carried in the buffer — a [`Link`] supplies it — but the monitor still
/// folds it into the checksum, so it has to be folded in here.
const DISPLAY_ADDRESS: u8 = 0x6e;

/// The host's address, likewise.
const HOST_ADDRESS: u8 = 0x51;

/// The host's *receive* address, which is what a reply's checksum is seeded
/// with.
///
/// Not a typo for [`HOST_ADDRESS`]. The asymmetry is in the specification and is
/// easy to get wrong in a way that shows up only as intermittent rejection.
const HOST_RECEIVE_ADDRESS: u8 = 0x50;

/// The VCP feature code for luminance.
pub(crate) const VCP_BRIGHTNESS: u8 = 0x10;

const OP_GET: u8 = 0x01;
const OP_SET: u8 = 0x03;
const OP_GET_REPLY: u8 = 0x02;

/// The length of a Get VCP Feature reply, including the leading source address.
pub(crate) const REPLY_LEN: usize = 11;

/// The specification's minimum wait between a request and its reply.
const REPLY_DELAY: Duration = Duration::from_millis(40);

/// The specification's minimum wait between one message and the next.
const MESSAGE_GAP: Duration = Duration::from_millis(50);

/// How many times an exchange is worth repeating.
///
/// A monitor that is asleep, switching input or simply busy drops a request
/// without saying so. Three is where the tools in this space have settled: it
/// covers the ordinary case of one dropped message without making a truly
/// unreachable display take a noticeable time to give up on.
const ATTEMPTS: usize = 3;

/// A way of carrying DDC/CI bytes to one display.
///
/// The only part of this protocol that differs by platform: `IOAVServiceWriteI2C`
/// on macOS, an `I2C_RDWR` ioctl on a `/dev/i2c-*` node on Linux. The error is an
/// `i32` because every one of those reports a different integer and none of them
/// means anything above this line.
pub(crate) trait Link {
    /// Puts a message on the bus.
    fn write(&self, bytes: &[u8]) -> std::result::Result<(), i32>;

    /// Fills a buffer from the bus.
    fn read(&self, into: &mut [u8]) -> std::result::Result<(), i32>;
}

/// A display's brightness over DDC/CI, on whatever link reaches it.
///
/// The maximum is read once when this is opened and kept, because a monitor's
/// own scale does not change and asking again would put another request on a
/// slow bus for every set.
pub(crate) struct Ddc<L: Link> {
    link: L,
    maximum: u16,
    display: String,
}

impl<L: Link> Ddc<L> {
    /// Opens a link and confirms the display answers about brightness.
    ///
    /// The confirmation is the point. A display having a link does not mean it
    /// speaks DDC/CI — a built-in panel has one and answers nothing, and plenty
    /// of adaptors carry the wires without carrying the protocol. The only
    /// reliable test is to ask something and see whether a reply comes back.
    ///
    /// # Errors
    ///
    /// [`Error::CannotReach`] when the display does not answer, which is the
    /// ordinary case rather than an exceptional one.
    pub(crate) fn open(link: L, display: &str) -> Result<Self> {
        let reading = read_feature(&link, VCP_BRIGHTNESS).ok_or_else(|| Error::CannotReach {
            mechanism: NAME,
            display: display.to_owned(),
        })?;

        Ok(Self {
            link,
            maximum: reading.maximum,
            display: display.to_owned(),
        })
    }
}

impl<L: Link> Backend for Ddc<L> {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self) -> Result<Brightness> {
        read_feature(&self.link, VCP_BRIGHTNESS)
            .map(|reading| Brightness::from_range(reading.current, reading.maximum))
            .ok_or_else(|| Error::NoReply {
                mechanism: NAME,
                display: self.display.clone(),
                attempts: ATTEMPTS,
            })
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let frame = set_request(VCP_BRIGHTNESS, level.to_range(self.maximum));

        // A set is unacknowledged: DDC/CI has no reply to a Set VCP Feature, so
        // there is nothing to check and nothing to retry against. A write the
        // bus itself rejects is still worth repeating.
        let mut last = 0;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                sleep(MESSAGE_GAP);
            }
            match self.link.write(&frame) {
                Ok(()) => return Ok(()),
                Err(code) => last = code,
            }
        }

        Err(Error::MechanismFailed {
            mechanism: NAME,
            call: "DDC/CI write",
            code: last,
        })
    }
}

/// What a monitor reports about one feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reading {
    pub current: u16,
    pub maximum: u16,
}

/// Asks a monitor for one feature, retrying a bus that drops messages.
fn read_feature<L: Link>(link: &L, feature: u8) -> Option<Reading> {
    let request = get_request(feature);

    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            sleep(MESSAGE_GAP);
        }
        if link.write(&request).is_err() {
            continue;
        }

        sleep(REPLY_DELAY);

        let mut reply = [0_u8; REPLY_LEN];
        if link.read(&mut reply).is_err() {
            continue;
        }

        if let Some(reading) = decode_reply(feature, &reply) {
            return Some(reading);
        }
    }
    None
}

/// The checksum a DDC/CI frame carries.
///
/// Computed over the whole frame including the two bus addresses, which a
/// [`Link`] carries out of band — so they are seeded here rather than written
/// into the buffer.
fn checksum(seed: u8, body: &[u8]) -> u8 {
    body.iter().fold(seed, |sum, byte| sum ^ byte)
}

/// A Get VCP Feature request.
pub(crate) fn get_request(feature: u8) -> [u8; 4] {
    let mut frame = [0x80 | 2, OP_GET, feature, 0];
    frame[3] = checksum(DISPLAY_ADDRESS ^ HOST_ADDRESS, &frame[..3]);
    frame
}

/// A Set VCP Feature request.
fn set_request(feature: u8, value: u16) -> [u8; 6] {
    let [high, low] = value.to_be_bytes();
    let mut frame = [0x80 | 4, OP_SET, feature, high, low, 0];
    frame[5] = checksum(DISPLAY_ADDRESS ^ HOST_ADDRESS, &frame[..5]);
    frame
}

/// Reads a Get VCP Feature reply, or rejects it.
///
/// Everything here is a reason a reply that arrived is still not an answer. A
/// bus that drops messages also delivers stale ones, and a reply to the previous
/// request looks exactly like a reply to this one except in the feature it
/// echoes.
fn decode_reply(feature: u8, frame: &[u8]) -> Option<Reading> {
    if frame.len() < REPLY_LEN {
        return None;
    }

    if checksum(
        HOST_RECEIVE_ADDRESS ^ DISPLAY_ADDRESS,
        &frame[1..REPLY_LEN - 1],
    ) != frame[REPLY_LEN - 1]
    {
        return None;
    }

    if frame[0] != DISPLAY_ADDRESS || frame[2] != OP_GET_REPLY {
        return None;
    }

    // A non-zero result code means the monitor understood the request and does
    // not support the feature.
    if frame[3] != 0 {
        return None;
    }

    // The echo is what distinguishes this reply from a stale one.
    if frame[4] != feature {
        return None;
    }

    let maximum = u16::from_be_bytes([frame[6], frame[7]]);
    if maximum == 0 {
        return None;
    }

    Some(Reading {
        current: u16::from_be_bytes([frame[8], frame[9]]),
        maximum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed reply: feature `0x10`, maximum 100, current 50.
    ///
    /// Written out rather than built by the encoder, so that a change to the
    /// checksum convention fails this test instead of moving in step with it.
    const REPLY: [u8; REPLY_LEN] = [
        0x6e, 0x88, 0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x32, 0xf2,
    ];

    #[test]
    fn a_get_request_is_framed_the_way_the_specification_says() {
        // 0x82 is 0x80 | two data bytes; 0x01 is Get VCP Feature; 0x10 is
        // luminance. The checksum covers the two bus addresses a `Link` carries
        // out of band: 0x6e ^ 0x51 ^ 0x82 ^ 0x01 ^ 0x10.
        assert_eq!(get_request(VCP_BRIGHTNESS), [0x82, 0x01, 0x10, 0xac]);
    }

    #[test]
    fn a_set_request_carries_its_value_big_endian() {
        assert_eq!(
            set_request(VCP_BRIGHTNESS, 0x1234),
            [0x84, 0x03, 0x10, 0x12, 0x34, 0x8e]
        );
        assert_eq!(
            set_request(VCP_BRIGHTNESS, 100),
            [0x84, 0x03, 0x10, 0x00, 0x64, 0xcc]
        );
    }

    #[test]
    fn a_well_formed_reply_reads_back_its_two_numbers() {
        assert_eq!(
            decode_reply(VCP_BRIGHTNESS, &REPLY),
            Some(Reading {
                current: 50,
                maximum: 100
            })
        );
    }

    #[test]
    fn a_reply_about_another_feature_is_not_an_answer_to_this_one() {
        // The case this check exists for: a bus with no flow control delivers
        // the previous request's reply, which is well formed in every other way.
        assert_eq!(decode_reply(0x12, &REPLY), None);
    }

    #[test]
    fn a_reply_reporting_an_unsupported_feature_is_rejected() {
        let mut frame = REPLY;
        frame[3] = 0x01;
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &frame), None);
    }

    #[test]
    fn a_reply_with_a_corrupt_byte_is_rejected() {
        let mut frame = REPLY;
        frame[9] ^= 0x01;
        assert_eq!(
            decode_reply(VCP_BRIGHTNESS, &frame),
            None,
            "the checksum should have caught a flipped bit in the value"
        );
    }

    #[test]
    fn a_reply_from_the_wrong_address_or_opcode_is_rejected() {
        let mut wrong_source = REPLY;
        wrong_source[0] = 0x51;
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &wrong_source), None);

        let mut wrong_opcode = REPLY;
        wrong_opcode[2] = 0x03;
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &wrong_opcode), None);
    }

    #[test]
    fn a_reply_claiming_a_zero_maximum_is_rejected() {
        // Monitors report this for controls they do not really implement, and it
        // would otherwise divide the scale by nothing.
        let mut frame = REPLY;
        frame[6] = 0;
        frame[7] = 0;
        frame[10] = 0x96; // 0xf2 ^ 0x64, the byte the old maximum contributed.
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &frame), None);

        // And the rejection is about the maximum rather than the checksum, which
        // would otherwise reject this frame for the wrong reason and pass the
        // test anyway.
        frame[7] = 1;
        frame[10] = 0x97;
        assert!(decode_reply(VCP_BRIGHTNESS, &frame).is_some());
    }

    #[test]
    fn a_truncated_reply_is_rejected_rather_than_indexed_into() {
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &REPLY[..REPLY_LEN - 1]), None);
        assert_eq!(decode_reply(VCP_BRIGHTNESS, &[]), None);
    }
}
