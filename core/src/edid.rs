//! Reading a display's own description of itself.
//!
//! An EDID block is the same 128 bytes on every operating system, because it
//! comes off the display rather than out of the machine. macOS hands it over
//! already parsed, through the IORegistry; Linux and Windows hand over the raw
//! bytes, from `/sys/class/drm/*/edid` and from the driver's registry key
//! respectively. So this is the one place that parsing happens, and both of
//! those platforms build the same [`Identity`] from it that macOS does.
//!
//! That matters more than it sounds. [`crate::identity::DisplayKey`] promises
//! that a display keys the same on every platform; a second parser written
//! against the same specification would be a second chance to disagree.
//!
//! The layout is EDID 1.3/1.4, which is what everything since about 2000
//! publishes:
//!
//! | bytes | |
//! | --- | --- |
//! | 0–7 | a fixed header, which is how a block is recognised |
//! | 8–9 | manufacturer, three letters in fifteen bits, big endian |
//! | 10–11 | product code, little endian |
//! | 12–15 | serial number, little endian |
//! | 54, 72, 90, 108 | four eighteen-byte descriptors |
//! | 127 | checksum: the whole block sums to zero |

/// What a display says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edid {
    /// The manufacturer code, comparable with what macOS calls the legacy
    /// manufacturer identifier.
    pub manufacturer: u32,
    /// The product code.
    pub product: u32,
    /// The serial number, which is zero on plenty of real monitors.
    pub serial: u32,
    /// The name, from a descriptor rather than a fixed field.
    pub name: Option<String>,
    /// The printed serial, likewise.
    pub printed_serial: Option<String>,
}

/// The length of the base block. Extensions follow it and are not read here.
const BLOCK: usize = 128;

/// The eight bytes every EDID begins with.
const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

/// Where the four descriptors start.
const DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];

/// How long each one is.
const DESCRIPTOR: usize = 18;

/// The descriptor tag for the display's name.
const TAG_NAME: u8 = 0xfc;

/// The descriptor tag for its printed serial.
const TAG_SERIAL: u8 = 0xff;

/// Reads an EDID block.
///
/// Returns [`None`] for anything that is not one. The checksum is enforced
/// rather than merely read: these bytes arrive over an I2C bus or out of a
/// registry key written by a driver, and a block that does not sum to zero is
/// one whose manufacturer and serial cannot be trusted either — which would put
/// a wrong display key into a configuration file and be very hard to notice.
pub(crate) fn parse(bytes: &[u8]) -> Option<Edid> {
    let block = bytes.get(..BLOCK)?;

    if block[..8] != HEADER {
        return None;
    }
    if block.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte)) != 0 {
        return None;
    }

    Some(Edid {
        // Big endian, uniquely in this block, because the three letters are
        // packed most significant first.
        manufacturer: u32::from(u16::from_be_bytes([block[8], block[9]])),
        product: u32::from(u16::from_le_bytes([block[10], block[11]])),
        serial: u32::from_le_bytes([block[12], block[13], block[14], block[15]]),
        name: descriptor(block, TAG_NAME),
        printed_serial: descriptor(block, TAG_SERIAL),
    })
}

/// Reads the text out of whichever descriptor carries a tag.
///
/// A descriptor is either a detailed timing — which begins with a non-zero pixel
/// clock — or one of these tagged blocks. The text runs to a line feed and is
/// padded with spaces to the end, so both have to be trimmed.
fn descriptor(block: &[u8], tag: u8) -> Option<String> {
    for start in DESCRIPTORS {
        let found = block.get(start..start + DESCRIPTOR)?;

        if found[0..3] != [0, 0, 0] || found[3] != tag {
            continue;
        }

        let text: String = found[5..]
            .iter()
            .copied()
            .take_while(|byte| *byte != 0x0a)
            .map(char::from)
            .collect();

        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read off the Samsung LS32AG55x attached to the machine this was written
    /// on, over I2C.
    const SAMSUNG: [u8; 128] = [
        0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x4c, 0x2d, 0xe3, 0x71, 0x52, 0x48, 0x48,
        0x30, 0x26, 0x21, 0x01, 0x03, 0x80, 0x46, 0x28, 0x78, 0x2a, 0x43, 0xa5, 0xae, 0x52, 0x44,
        0xb0, 0x26, 0x0f, 0x50, 0x54, 0xbf, 0xef, 0x80, 0x71, 0x4f, 0x81, 0x00, 0x81, 0xc0, 0x81,
        0x80, 0x95, 0x00, 0xa9, 0xc0, 0xb3, 0x00, 0x01, 0x01, 0x4c, 0x71, 0x00, 0xa0, 0xa0, 0xa0,
        0x29, 0x50, 0x08, 0x40, 0x35, 0x00, 0xba, 0x89, 0x21, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00,
        0xfd, 0x00, 0x32, 0x90, 0x1e, 0xd6, 0x3b, 0x00, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x00, 0x00, 0x00, 0xfc, 0x00, 0x4c, 0x53, 0x33, 0x32, 0x41, 0x47, 0x35, 0x35, 0x78, 0x0a,
        0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xff, 0x00, 0x48, 0x4e, 0x41, 0x57, 0x39, 0x30, 0x30,
        0x30, 0x30, 0x31, 0x0a, 0x20, 0x20, 0x01, 0xf4,
    ];

    /// The built-in panel of the same machine.
    const APPLE: [u8; 128] = [
        0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x06, 0x10, 0x53, 0xa0, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x20, 0x01, 0x04, 0xa5, 0x1d, 0x13, 0x78, 0x20, 0x3e, 0x51, 0xae, 0x51, 0x43,
        0xb0, 0x26, 0x0f, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xfc, 0x00, 0x41, 0x70, 0x70, 0x6c, 0x65, 0x20, 0x44, 0x69, 0x73, 0x70, 0x6c, 0x61, 0x79,
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xec,
    ];

    /// The numbers below are not this parser's opinion. They are what macOS's
    /// own IORegistry reported for the same monitor — `LegacyManufacturerID`
    /// 19501, `ProductID` 29155, `SerialNumber` 810043474, `ProductName`
    /// "LS32AG55x", `AlphanumericSerialNumber` "HNAW900001".
    ///
    /// So this test is a cross-check against Apple's parser rather than against
    /// my reading of the specification, which is the whole reason a real block
    /// was captured instead of a synthetic one.
    #[test]
    fn it_agrees_with_what_macos_reported_for_the_same_monitor() {
        let parsed = parse(&SAMSUNG).expect("a real EDID should parse");

        assert_eq!(parsed.manufacturer, 19501, "0x4c2d, which decodes to SAM");
        assert_eq!(parsed.product, 29155);
        assert_eq!(parsed.serial, 810_043_474);
        assert_eq!(parsed.name.as_deref(), Some("LS32AG55x"));
        assert_eq!(parsed.printed_serial.as_deref(), Some("HNAW900001"));
    }

    #[test]
    fn a_panel_with_no_serial_descriptor_reports_none() {
        let parsed = parse(&APPLE).expect("a real EDID should parse");

        assert_eq!(parsed.manufacturer, 0x0610, "which decodes to APP");
        assert_eq!(parsed.serial, 0);
        assert_eq!(parsed.printed_serial, None);
        // Worth noting: the IORegistry publishes no name at all for this panel,
        // and its own EDID carries one.
        assert_eq!(parsed.name.as_deref(), Some("Apple Display"));
    }

    #[test]
    fn the_key_this_produces_is_the_key_macos_produces() {
        // The contract entry 8 exists for, checked end to end: raw bytes from a
        // display, through this parser, to the same string macOS arrived at by a
        // completely different route.
        let parsed = parse(&SAMSUNG).expect("parse");
        let key = crate::identity::DisplayKey::of(&crate::identity::Identity {
            built_in: false,
            manufacturer: parsed.manufacturer,
            product: parsed.product,
            serial: parsed.serial,
            printed_serial: parsed.printed_serial,
        });

        assert_eq!(key.as_str(), "SAM-71e3-HNAW900001");
    }

    #[test]
    fn a_block_that_is_not_an_edid_is_refused() {
        assert_eq!(parse(&[0_u8; 128]), None, "no header");
        assert_eq!(parse(&SAMSUNG[..127]), None, "too short");
        assert_eq!(parse(&[]), None, "empty");
    }

    #[test]
    fn a_block_that_does_not_sum_to_zero_is_refused() {
        // These bytes come off an I2C bus or out of a registry key a driver
        // wrote. A corrupt block whose manufacturer still looked plausible would
        // put a wrong key into a configuration file, which is very hard to
        // notice afterwards.
        let mut corrupt = SAMSUNG;
        corrupt[10] ^= 0x01;
        assert_eq!(parse(&corrupt), None);
    }

    #[test]
    fn extension_blocks_after_the_first_are_ignored_rather_than_refused() {
        let mut long = SAMSUNG.to_vec();
        long.extend_from_slice(&[0x02; 128]);

        assert_eq!(parse(&long), parse(&SAMSUNG));
    }
}
