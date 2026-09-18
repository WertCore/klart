//! What a display says about itself, and the key derived from it.
//!
//! Every platform can read this, because every platform reads it from the same
//! place: the EDID block the display publishes over its own connector. macOS
//! surfaces it through the IORegistry, Linux through `/sys/class/drm/*/edid`,
//! Windows through SetupAPI. The numbers are the display's, not the operating
//! system's.
//!
//! That is what makes [`DisplayKey`] portable, and the portability is a contract
//! rather than a coincidence: a key written on one operating system has to be
//! byte-for-byte the key computed on another, or a configuration file does not
//! survive a dual boot. The rules are spelled out on [`DisplayKey::of`] and
//! tested here, so that a second platform has something to conform to rather
//! than something to guess at.

use std::fmt;

/// What a display publishes about itself.
///
/// Built by the platform layer and consumed by everything above it. Every field
/// is an EDID field, deliberately: anything an operating system invents about a
/// display cannot be part of an identity two operating systems must agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    /// Whether this is the machine's own panel rather than something plugged in.
    pub built_in: bool,
    /// The EDID manufacturer code: three letters packed into fifteen bits.
    pub manufacturer: u32,
    /// The EDID product code.
    pub product: u32,
    /// The EDID serial number, which is zero on plenty of real monitors.
    pub serial: u32,
    /// The serial printed on the case, from EDID's descriptor block.
    pub printed_serial: Option<String>,
}

/// A display's identity, as stable as the display will allow.
///
/// Not the operating system's handle for a display — macOS reassigns
/// `CGDirectDisplayID` freely, and the others are no better — but something
/// derived from what the panel says about itself, so that a level stored against
/// it can be found again tomorrow, and on another operating system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DisplayKey(String);

impl DisplayKey {
    /// Derives the key for a display.
    ///
    /// # The contract
    ///
    /// Any platform implementing `klart` must produce exactly these bytes from
    /// the same EDID, or configuration stops being portable between them.
    ///
    /// 1. A built-in panel is the string `builtin`, and nothing else. A machine
    ///    has one and it cannot be swapped, so anything more specific would
    ///    distinguish nothing — and the EDID of a soldered panel is the least
    ///    consistent of all of them between operating systems.
    /// 2. Otherwise the key is `{maker}-{product}-{serial}`.
    /// 3. `{maker}` is the manufacturer code decoded to three uppercase ASCII
    ///    letters, or, if it does not decode to three letters, the sixteen-bit
    ///    code as four lowercase hex digits.
    /// 4. `{product}` is the product code as four lowercase hex digits.
    /// 5. `{serial}` is the printed serial if the display publishes one, and
    ///    otherwise the numeric serial as eight lowercase hex digits. The
    ///    printed one is preferred because two units of the same model are far
    ///    likelier to differ in it.
    /// 6. A printed serial is trimmed of ASCII whitespace, and any character
    ///    outside `A-Z a-z 0-9 . _ -` becomes `_`. EDID strings are padded and
    ///    terminated differently by different readers, and a key that ends up in
    ///    a file name or a configuration key cannot carry arbitrary bytes. `#`
    ///    in particular is excluded because [`disambiguate`] uses it.
    /// 7. A printed serial that is empty after that is treated as absent.
    #[must_use]
    pub(crate) fn of(identity: &Identity) -> Self {
        if identity.built_in {
            return Self("builtin".to_owned());
        }

        let maker = pnp_code(identity.manufacturer)
            .unwrap_or_else(|| format!("{:04x}", identity.manufacturer & 0xffff));

        let serial = identity
            .printed_serial
            .as_deref()
            .and_then(normalise_serial)
            .unwrap_or_else(|| format!("{:08x}", identity.serial));

        Self(format!(
            "{maker}-{:04x}-{serial}",
            identity.product & 0xffff
        ))
    }

    /// The key in the form it is written to configuration.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Decodes the three-letter manufacturer code EDID packs into fifteen bits.
///
/// Five bits a letter, `A` at 1, most significant letter first. Anything that
/// decodes outside `A..=Z` is not a code at all: an adaptor that invents an EDID
/// for a display behind it often gets this wrong, and a garbled string is worse
/// in a menu than an honest fallback.
pub(crate) fn pnp_code(manufacturer: u32) -> Option<String> {
    let packed = u16::try_from(manufacturer).ok()?;
    [(packed >> 10) & 0x1f, (packed >> 5) & 0x1f, packed & 0x1f]
        .into_iter()
        .map(|slot| {
            let letter = u8::try_from(slot).ok()?.checked_add(b'A' - 1)?;
            letter.is_ascii_uppercase().then_some(char::from(letter))
        })
        .collect()
}

/// Puts a printed serial into the form the contract requires.
fn normalise_serial(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();

    (!cleaned.is_empty()).then_some(cleaned)
}

/// Appends a positional suffix to any key that more than one display produced.
///
/// Two monitors of the same model with the same printed serial do occur, because
/// some manufacturers ship every unit with the serial left at zero. Without this
/// they would share an identity, and whatever was stored against one would be
/// read back for the other.
///
/// The suffix follows screen order, left to right and then top to bottom. That
/// survives a reboot but not rearranging the displays, which is the best
/// available: such displays are, by construction, publishing nothing that tells
/// them apart.
///
/// Quadratic, over a list that cannot exceed the eight displays a Mac Pro drives.
pub(crate) fn disambiguate(identities: &mut [(DisplayKey, (i32, i32))]) {
    let mut collisions: Vec<DisplayKey> = Vec::new();
    for (key, _) in identities.iter() {
        let shared = identities.iter().filter(|(other, _)| other == key).count() > 1;
        if shared && !collisions.contains(key) {
            collisions.push(key.clone());
        }
    }

    for key in collisions {
        let mut sharing: Vec<usize> = identities
            .iter()
            .enumerate()
            .filter(|(_, (other, _))| *other == key)
            .map(|(index, _)| index)
            .collect();
        sharing.sort_by_key(|&index| identities[index].1);

        for (ordinal, index) in sharing.into_iter().enumerate() {
            identities[index].0 = DisplayKey(format!("{key}#{}", ordinal + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external() -> Identity {
        Identity {
            built_in: false,
            manufacturer: 0x4c2d,
            product: 0x71e3,
            serial: 810_043_474,
            printed_serial: Some("HNAW900001".to_owned()),
        }
    }

    #[test]
    fn it_decodes_the_packed_manufacturer_code() {
        // The two this machine reports: Samsung on the external monitor, Apple
        // on the built-in panel.
        assert_eq!(pnp_code(0x4c2d).as_deref(), Some("SAM"));
        assert_eq!(pnp_code(0x0610).as_deref(), Some("APP"));
    }

    #[test]
    fn it_rejects_bit_patterns_that_are_not_three_letters() {
        assert_eq!(pnp_code(0), None, "slot zero is not a letter");
        assert_eq!(pnp_code(0xffff), None, "31 is past Z");
        assert_eq!(
            pnp_code(0x1_0000),
            None,
            "wider than the field EDID gives it"
        );
    }

    // The tests below are the contract a second platform has to conform to.

    #[test]
    fn rule_1_the_built_in_panel_is_one_fixed_string() {
        let panel = Identity {
            built_in: true,
            ..external()
        };
        assert_eq!(DisplayKey::of(&panel).as_str(), "builtin");
    }

    #[test]
    fn rules_2_to_5_an_external_key_is_maker_product_and_printed_serial() {
        assert_eq!(DisplayKey::of(&external()).as_str(), "SAM-71e3-HNAW900001");
    }

    #[test]
    fn rule_3_an_undecodable_manufacturer_becomes_lowercase_hex() {
        let odd = Identity {
            manufacturer: 0xffff,
            ..external()
        };
        assert_eq!(DisplayKey::of(&odd).as_str(), "ffff-71e3-HNAW900001");
    }

    #[test]
    fn rule_5_no_printed_serial_falls_back_to_the_numeric_one() {
        let bare = Identity {
            printed_serial: None,
            serial: 0x0c0f,
            ..external()
        };
        assert_eq!(DisplayKey::of(&bare).as_str(), "SAM-71e3-00000c0f");
    }

    #[test]
    fn rule_6_a_printed_serial_is_trimmed_and_confined_to_safe_characters() {
        // EDID descriptor strings are padded, terminated and re-encoded
        // differently by different readers. Two platforms reading the same panel
        // have to arrive at the same key regardless.
        let padded = Identity {
            printed_serial: Some("  HNAW900001\n".to_owned()),
            ..external()
        };
        assert_eq!(DisplayKey::of(&padded).as_str(), "SAM-71e3-HNAW900001");

        let awkward = Identity {
            printed_serial: Some("A B/C#1".to_owned()),
            ..external()
        };
        assert_eq!(
            DisplayKey::of(&awkward).as_str(),
            "SAM-71e3-A_B_C_1",
            "a serial must not be able to forge a disambiguator"
        );
    }

    #[test]
    fn rule_7_a_serial_that_is_only_padding_counts_as_absent() {
        let blank = Identity {
            printed_serial: Some("   ".to_owned()),
            serial: 0x0c0f,
            ..external()
        };
        assert_eq!(DisplayKey::of(&blank).as_str(), "SAM-71e3-00000c0f");
    }

    #[test]
    fn identical_displays_are_separated_by_screen_order() {
        let shared = DisplayKey("SAM-71e3-00000000".to_owned());
        // Listed right-hand first, to show the suffix follows position rather
        // than the order the platform happened to enumerate in.
        let mut identities = vec![(shared.clone(), (2560, 0)), (shared, (0, 0))];

        disambiguate(&mut identities);

        assert_eq!(identities[1].0.as_str(), "SAM-71e3-00000000#1");
        assert_eq!(identities[0].0.as_str(), "SAM-71e3-00000000#2");
    }

    #[test]
    fn displays_that_already_differ_keep_their_keys() {
        let mut identities = vec![
            (DisplayKey("builtin".to_owned()), (-1470, 0)),
            (DisplayKey("SAM-71e3-HNAW900001".to_owned()), (0, 0)),
        ];

        disambiguate(&mut identities);

        assert_eq!(identities[0].0.as_str(), "builtin");
        assert_eq!(identities[1].0.as_str(), "SAM-71e3-HNAW900001");
    }
}
